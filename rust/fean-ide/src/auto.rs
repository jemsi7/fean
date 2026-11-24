//! The automatic mode (`fean-ide --shots DIR`, DR-33): the IDE drives itself through a fixed
//! scenario, saves the screen after each scene and writes down what it observed. The report is
//! what the IDE's tests read, and the pictures are the figures of the manual.
//!
//! The scenario uses the application the way a user does: the same methods and the same `Act`s
//! that menus and clicks use. Nothing here reaches into the engine on its own.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use eframe::egui;
use fean_core::lower::SourceMap;
use fean_core::solve::Verdict;
use serde_json::{json, Map, Value};

use crate::app::{Act, IdeApp, Layout, View, EXAMPLES, FEATURES};
use crate::editor;

/// The window of the automatic mode, in points.
pub const WINDOW: [f32; 2] = [1440.0, 900.0];
const FRAME_LIMIT: u64 = 6_000;

pub enum Wait {
    Frames(u32),
    /// Until nothing is being checked or run.
    Idle,
    /// Save the screen under this name, then go on.
    Shot(&'static str),
}

#[derive(Default)]
pub struct Notes {
    map: Map<String, Value>,
    /// Run the same stage again after its wait (a stage that walks through many scenes).
    again: bool,
    /// How far such a stage has come.
    cursor: usize,
}

impl Notes {
    fn put(&mut self, key: &str, value: Value) {
        self.map.insert(key.to_string(), value);
    }
    fn put_in(&mut self, group: &str, key: &str, value: Value) {
        let entry = self.map.entry(group.to_string()).or_insert_with(|| json!({}));
        if let Some(object) = entry.as_object_mut() {
            object.insert(key.to_string(), value);
        }
    }
}

pub struct Auto {
    dir: PathBuf,
    stage: usize,
    wait: Wait,
    asked: bool,
    settle: u32,
    frames: u64,
    notes: Notes,
    shots: Vec<String>,
    size: [usize; 2],
}

impl Auto {
    pub fn new(dir: PathBuf) -> Self {
        Auto { dir, stage: 0, wait: Wait::Frames(6), asked: false, settle: 0, frames: 0, notes: Notes::default(), shots: Vec::new(), size: [0, 0] }
    }
}

type Stage = fn(&mut IdeApp, &egui::Context, &mut Notes) -> Wait;

fn example() -> &'static str {
    EXAMPLES[5].2
}

fn answers(app: &IdeApp) -> Value {
    let mut map = Map::new();
    if let Some(a) = &app.analysis {
        for answer in a.answers() {
            map.insert(answer.name.clone(), json!(answer.text));
        }
    }
    Value::Object(map)
}

fn findings(app: &IdeApp) -> Value {
    let rows: Vec<Value> = app
        .findings
        .iter()
        .map(|f| json!({"kind": f.kind, "code": f.code, "title": f.title, "line": SourceMap::line(f.loc), "assignment": f.assignment, "run": f.run, "solve": f.solve}))
        .collect();
    json!(rows)
}

fn run_notes(app: &IdeApp) -> Value {
    json!({
        "answers": answers(app), "findings": findings(app), "stale": app.stale(),
        "has_results": app.analysis.is_some(), "runs_done": app.runs_done,
        "error": app.run_error.as_ref().map(|f| json!({"kind": f.kind, "code": f.code, "line": SourceMap::line(f.loc)})),
    })
}

fn diagnostics(app: &IdeApp) -> Value {
    let rows: Vec<Value> = editor::problems(app)
        .iter()
        .map(|d| json!({"kind": d.kind, "code": d.code, "line": d.line, "message": d.message}))
        .collect();
    json!({"text": app.text, "found": rows, "checked": app.checked()})
}

/// What the solver's view shows: every solve with its answer and its regions.
fn solves(app: &IdeApp) -> Value {
    let Some(a) = &app.analysis else { return Value::Null };
    let rows: Vec<Value> = a
        .solves()
        .iter()
        .map(|sv| {
            let rows: Vec<Value> = sv
                .rows
                .iter()
                .map(|row| {
                    let s = &row.solved;
                    json!({"label": row.label, "text": s.text, "runs": s.runs, "complete": s.complete, "best": s.best,
                           "counter": s.counter, "solution_case": row.solution, "counter_case": row.counter,
                           "regions": s.leaves.iter().map(|l| json!([l.verdict.word(), s.region_text(&l.region)])).collect::<Vec<_>>()})
                })
                .collect();
            json!({"name": sv.name, "form": sv.form, "text": sv.text, "line": SourceMap::line(sv.loc), "rows": rows})
        })
        .collect();
    json!(rows)
}

/// The case that is selected: where it comes from, at which values.
fn case_notes(app: &IdeApp) -> Value {
    let Some(a) = &app.analysis else { return Value::Null };
    let Some(slot) = a.slots.get(app.sel.assignment) else { return Value::Null };
    json!({
        "case": app.sel.assignment, "label": slot.label, "origin": slot.origin.name(), "solve": slot.origin.solve(),
        "point": slot.point, "unknowns": slot.vm.as_ref().map(|vm| vm.uv.iter().map(|x| x.v).collect::<Vec<_>>()),
        "runs": slot.runs.len(), "view": app.view.name(), "cases": a.slots.len(),
    })
}

fn identity(app: &IdeApp) -> Value {
    match app.identity() {
        Some((assignment, actor, key, step)) => json!({"assignment": assignment, "actor": actor, "run_key": key, "step": step}),
        None => Value::Null,
    }
}

/// Choose an assignment by its label, a history, a step and an actor by name.
fn select(app: &mut IdeApp, label: &str, run: usize, step: usize, actor: &str) {
    let Some(a) = &app.analysis else { return };
    app.sel.assignment = a.slots.iter().position(|s| s.label == label).unwrap_or(0);
    app.sel.actor = a.image.actors.iter().position(|n| n == actor).unwrap_or(0);
    app.sel.run = run;
    app.sel.step = step;
    app.playing = false;
}

fn jump(app: &mut IdeApp, ctx: &egui::Context, line: Option<u32>) {
    if let Some(line) = line {
        app.apply(Act::Jump(line), ctx);
    }
}

const STAGES: &[Stage] = &[
    // -- the editor
    |app, ctx, notes| {
        ctx.set_theme(egui::Theme::Light);
        app.dark = false;
        app.load_example(5);
        app.layout = Layout::Edit;
        notes.put("example", json!(EXAMPLES[5].0));
        Wait::Idle
    },
    |app, _, notes| {
        notes.put("editor", json!({"clean": diagnostics(app), "lines": app.text.lines().count(), "dirty": app.dirty()}));
        Wait::Shot("01-editor.png")
    },
    // -- run, and the two halves side by side
    |app, ctx, _| {
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put("first_run", run_notes(app));
        app.layout = Layout::Split;
        app.view = View::Overview;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("02-split.png"),
    // -- the seven views, on their own
    |app, _, _| {
        app.layout = Layout::Results;
        app.view = View::Overview;
        select(app, "early=true", 0, 12, "D");
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("03-overview.png"),
    |app, _, _| {
        app.view = View::Findings;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("04-findings.png"),
    |app, _, _| {
        app.view = View::Graph;
        select(app, "early=true", 0, 15, "D");
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("05-graph.png"),
    |app, _, _| {
        app.view = View::Timeline;
        select(app, "early=true", 0, 12, "D");
        Wait::Frames(10)
    },
    |_, _, _| Wait::Shot("06-timeline.png"),
    |app, _, _| {
        app.view = View::Actor;
        select(app, "early=false", 0, 6, "D");
        Wait::Frames(8)
    },
    |app, _, notes| {
        let view = app.cache.view.as_ref();
        notes.put(
            "actor_view",
            json!({
                "actor": "D", "assignment": "early=false",
                "worlds": view.map(|v| v.worlds.clone()),
                "unknown": view.map(|v| v.cells.iter().filter(|c| !c.known).map(|c| c.name.clone()).collect::<Vec<_>>()),
                "menu": view.and_then(|v| v.menu.as_ref().map(|m| m.1.len())),
            }),
        );
        Wait::Shot("07-actor.png")
    },
    |app, _, _| {
        app.view = View::Histories;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("08-histories.png"),
    |app, _, _| {
        app.view = View::Compile;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("09-compile.png"),
    // -- from a result back into the code
    |app, ctx, _| {
        app.layout = Layout::Split;
        app.view = View::Timeline;
        select(app, "early=true", 0, 15, "D");
        let line = app.analysis.as_ref().and_then(|a| {
            let event = a.image.events.iter().position(|e| e.name == "collect")?;
            SourceMap::line(a.map.events.get(event).copied().flatten())
        });
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put_in("jumps", "event", json!({"name": "collect", "line": app.cursor.0, "layout": app.layout.name()}));
        Wait::Shot("10-jump.png")
    },
    |app, ctx, _| {
        let line = app.analysis.as_ref().and_then(|a| a.answers().iter().find(|q| q.name == "canD").and_then(|q| SourceMap::line(q.loc)));
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, ctx, notes| {
        notes.put_in("jumps", "query", json!({"name": "canD", "line": app.cursor.0}));
        let line = app.findings.first().and_then(|f| SourceMap::line(f.loc));
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, ctx, notes| {
        let title = app.findings.first().map(|f| f.title.clone());
        notes.put_in("jumps", "finding", json!({"title": title, "line": app.cursor.0}));
        let line = app.cache.steps.get(app.sel.step).and_then(|s| SourceMap::line(s.loc));
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, _, notes| {
        let title = app.cache.steps.get(app.sel.step).map(|s| s.title.clone());
        notes.put_in("jumps", "statement", json!({"title": title, "line": app.cursor.0}));
        // jumping while only the results are shown brings the editor back
        app.layout = Layout::Results;
        Wait::Frames(4)
    },
    |app, ctx, _| {
        jump(app, ctx, Some(2));
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put_in("jumps", "from_results", json!({"line": app.cursor.0, "layout": app.layout.name()}));
        Wait::Frames(2)
    },
    // -- edit, run again: the selection stays, the results change
    |app, _, _| {
        app.view = View::Actor;
        select(app, "early=true", 3, 12, "L");
        Wait::Frames(8)
    },
    |app, _, notes| {
        notes.put_in("selection", "before", identity(app));
        app.set_text(example().replace("init I = 100", "init I = 60"));
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("selection", "stale_after_edit", json!(app.stale()));
        notes.put("edited_text", json!(app.text));
        Wait::Shot("11-stale.png")
    },
    |app, ctx, _| {
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("selection", "after", identity(app));
        notes.put("second_run", run_notes(app));
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("12-rerun.png"),
    |app, _, _| {
        app.view = View::Findings;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("13-rerun-findings.png"),
    // -- errors where they are written
    |app, _, _| {
        app.set_text(example().replace("actor D, L, I, J", "actor D, L, I, J $"));
        app.goto = Some(1);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("diagnostics", "syntax", diagnostics(app));
        Wait::Shot("14-syntax-error.png")
    },
    |app, _, _| {
        app.set_text(example().replace("require selected == none", "require selected == 1[KRW]"));
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("diagnostics", "type", diagnostics(app));
        Wait::Shot("15-type-error.png")
    },
    |app, ctx, _| {
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put("failed_run", run_notes(app));
        notes.put_in("diagnostics", "after_run", diagnostics(app));
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("16-failed-run.png"),
    // -- back to a text that runs; find; the dark screen
    |app, ctx, _| {
        app.set_text(example().to_string());
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put("third_run", run_notes(app));
        app.find.open = true;
        app.find.query = "collect".to_string();
        app.find.go = Some(true);
        app.view = View::Overview;
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put("find", json!({"query": app.find.query, "count": app.find.count, "line": app.cursor.0}));
        Wait::Shot("17-find.png")
    },
    |app, ctx, _| {
        app.find.open = false;
        app.dark = true;
        ctx.set_theme(egui::Theme::Dark);
        app.view = View::Timeline;
        select(app, "early=false", 2, 12, "I");
        Wait::Frames(10)
    },
    |_, _, _| Wait::Shot("18-dark.png"),
    |app, ctx, _| {
        app.dark = false;
        ctx.set_theme(egui::Theme::Light);
        app.layout = Layout::Results;
        Wait::Frames(2)
    },
    // -- solving backwards: unknowns, the solver's view, cases
    |app, ctx, notes| {
        app.load_example(8);
        app.layout = Layout::Split;
        app.view = View::Solve;
        notes.put_in("solver", "example", json!(EXAMPLES[8].0));
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("solver", "run", run_notes(app));
        notes.put_in("solver", "solves", solves(app));
        notes.put_in("solver", "cases", json!(app.analysis.as_ref().map(|a| a.slots.iter().map(|s| s.label.clone()).collect::<Vec<_>>())));
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("19-solver.png"),
    // the solution of `short`, as a case; then the other views show that case
    |app, ctx, notes| {
        let case = app.analysis.as_ref().and_then(|a| a.solves().iter().find(|sv| sv.name == "short").and_then(|sv| sv.rows[0].solution));
        if let Some(case) = case {
            app.apply(Act::Assignment(case), ctx);
        }
        notes.put_in("solver", "picked_solution", case_notes(app));
        app.view = View::Timeline;
        app.layout = Layout::Results;
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put_in("solver", "solution_steps", json!(app.cache.steps.iter().map(|s| s.title.clone()).collect::<Vec<_>>()));
        Wait::Shot("20-solver-case.png")
    },
    // "이 값으로 보기" on a region of solutions of `need`: the value the solver ran there
    |app, ctx, notes| {
        let sample = app.analysis.as_ref().and_then(|a| {
            let solved = &a.solves().first()?.rows.first()?.solved;
            let leaf = solved.leaves.iter().find(|leaf| leaf.verdict == Verdict::Yes)?;
            Some(leaf.sample[..solved.vars.len().min(leaf.sample.len())].to_vec())
        });
        if let Some(point) = sample {
            app.apply(Act::Case { solve: 0, row: 0, point }, ctx);
        }
        notes.put_in("solver", "picked_value", case_notes(app));
        app.view = View::Overview;
        Wait::Frames(10)
    },
    |app, _, notes| {
        let texts = app.analysis.as_ref().map(|a| {
            a.answers().iter().map(|q| json!([q.name, q.rows.get(app.sel.assignment).map(|r| r.text.clone())])).collect::<Vec<_>>()
        });
        notes.put_in("solver", "answers_in_case", json!(texts));
        Wait::Shot("21-solver-overview.png")
    },
    // from the solver's view into the code: a solve, an unknown
    |app, ctx, _| {
        app.layout = Layout::Split;
        app.view = View::Solve;
        let line = app.analysis.as_ref().and_then(|a| a.solves().iter().find(|sv| sv.name == "spare").and_then(|sv| SourceMap::line(sv.loc)));
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, ctx, notes| {
        notes.put_in("solver", "jump_solve", json!({"name": "spare", "line": app.cursor.0}));
        let line = app.analysis.as_ref().and_then(|a| SourceMap::line(a.map.unknowns.first().copied().flatten()));
        jump(app, ctx, line);
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put_in("solver", "jump_unknown", json!({"name": "cap", "line": app.cursor.0}));
        Wait::Frames(2)
    },
    // the state at a point, and a solve that has no solution
    |app, ctx, _| {
        app.load_example(9);
        app.layout = Layout::Results;
        app.view = View::Solve;
        app.run(ctx);
        Wait::Idle
    },
    |app, _, notes| {
        notes.put_in("solver", "point", json!({"example": EXAMPLES[9].0, "solves": solves(app), "findings": findings(app)}));
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("22-solver-point.png"),
    |app, _, _| {
        app.view = View::Findings;
        Wait::Frames(8)
    },
    |_, _, _| Wait::Shot("23-solver-findings.png"),
    |app, ctx, notes| {
        // the finding of the unsolved solve leads to a value that is no solution
        let found = app.findings.iter().find(|f| f.kind == "unsolved").map(|f| (f.assignment, f.solve));
        if let Some((Some(case), _)) = found {
            app.apply(Act::Show { assignment: case, run: 0, step: None, view: View::Timeline }, ctx);
        }
        notes.put_in("solver", "unsolved", json!({"solve": found.and_then(|f| f.1), "case": case_notes(app)}));
        Wait::Frames(10)
    },
    |app, _, notes| {
        notes.put_in("solver", "unsolved_steps", json!(app.cache.steps.iter().map(|s| (s.kind, s.title.clone())).collect::<Vec<_>>()));
        Wait::Shot("24-solver-counter.png")
    },
    |app, _, _| {
        app.layout = Layout::Results;
        Wait::Frames(2)
    },
    // -- every built-in example through every view (no view may fail on any of them)
    tour,
];

/// Scenes of the tour that are kept as pictures: (example, view, file).
const TOUR_SHOTS: &[(&str, View, &str)] = &[
    ("Support", View::Findings, "25-support-findings.png"),
    ("Support", View::Timeline, "26-support-timeline.png"),
    ("CallExample", View::Graph, "27-call-graph.png"),
    ("Workout", View::Graph, "28-workout-graph.png"),
    ("Straddle", View::Overview, "29-straddle-overview.png"),
    ("LoanLifecycle", View::Actor, "30-loan-actor.png"),
];

/// One example after another: open it, run it, show each of the eight views.
fn tour(app: &mut IdeApp, _ctx: &egui::Context, notes: &mut Notes) -> Wait {
    const PHASES: usize = 2 + 2 * View::ALL.len(); // open and run, note, then (show, keep) for every view
    let (index, phase) = (notes.cursor / PHASES, notes.cursor % PHASES);
    if index >= EXAMPLES.len() {
        notes.again = false;
        return Wait::Frames(1);
    }
    notes.again = true;
    notes.cursor += 1;
    let (name, _, text) = EXAMPLES[index];
    match phase {
        0 => {
            app.load_example(index);
            app.run(_ctx);
            Wait::Idle
        }
        1 => {
            let runs = app.analysis.as_ref().map(|a| a.slots.iter().map(|s| s.runs.len()).sum::<usize>());
            notes.put_in(
                "examples",
                name,
                json!({"answers": answers(app), "findings": findings(app), "runs": runs, "text_is_example": app.text == text,
                       "assignments": app.analysis.as_ref().map(|a| a.slots.len()), "views": [],
                       "solves": app.analysis.as_ref().map(|a| a.solves().iter().map(|sv| json!([sv.name, sv.text])).collect::<Vec<_>>())}),
            );
            Wait::Frames(1)
        }
        _ => {
            let view = View::ALL[(phase - 2) / 2];
            if phase % 2 == 0 {
                app.view = view;
                // a history that fails, if there is one: the views must cope with it
                let failing = app.analysis.as_ref().and_then(|a| {
                    a.slots.iter().enumerate().find_map(|(i, s)| s.runs.iter().position(|r| r.outcome.is_some()).map(|r| (i, r)))
                });
                let (assignment, run) = failing.unwrap_or((0, 0));
                app.sel = crate::app::Selection { assignment, run, step: usize::MAX, actor: 0 };
                if view == View::Actor {
                    app.sel.actor = app.analysis.as_ref().map(|a| a.image.actors.len().saturating_sub(1)).unwrap_or(0);
                }
                return Wait::Frames(4);
            }
            if let Some(Value::Array(seen)) = notes.map.get_mut("examples").and_then(|e| e.get_mut(name)).and_then(|e| e.get_mut("views")) {
                seen.push(json!(view.name()));
            }
            match TOUR_SHOTS.iter().find(|(example, v, _)| *example == name && *v == view) {
                Some((_, _, file)) => Wait::Shot(file),
                None => Wait::Frames(0),
            }
        }
    }
}

fn save(dir: &PathBuf, name: &str, image: &egui::ColorImage) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    image::save_buffer(dir.join(name), image.as_raw(), image.size[0] as u32, image.size[1] as u32, image::ColorType::Rgba8)
        .map_err(|e| e.to_string())
}

fn finish(app: &mut IdeApp, auto: &mut Auto, ctx: &egui::Context, error: Option<String>) {
    let mut report = std::mem::take(&mut auto.notes.map);
    report.insert("ok".into(), json!(error.is_none()));
    report.insert("error".into(), json!(error));
    report.insert("shots".into(), json!(auto.shots));
    report.insert("size".into(), json!(auto.size));
    report.insert("pixels_per_point".into(), json!(ctx.pixels_per_point()));
    report.insert("views".into(), json!(View::ALL.iter().map(|v| v.name()).collect::<Vec<_>>()));
    report.insert("layouts".into(), json!([Layout::Edit.name(), Layout::Split.name(), Layout::Results.name()]));
    report.insert("features".into(), json!(FEATURES.iter().map(|f| f.1).collect::<Vec<_>>()));
    report.insert("frames".into(), json!(auto.frames));
    report.insert("korean_font".into(), json!(ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), '한'))));
    app.allow_close = true; // never ask about unsaved changes on the way out
    let text = serde_json::to_string_pretty(&Value::Object(report)).unwrap_or_default();
    let written = std::fs::create_dir_all(&auto.dir).and_then(|()| std::fs::write(auto.dir.join("report.json"), text));
    if error.is_some() || written.is_err() {
        crate::EXIT.store(1, Ordering::SeqCst);
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// One frame of the automatic mode.
pub fn tick(app: &mut IdeApp, ctx: &egui::Context) {
    let Some(mut auto) = app.auto.take() else { return };
    ctx.request_repaint();
    auto.frames += 1;
    if auto.frames > FRAME_LIMIT {
        let stage = auto.stage;
        finish(app, &mut auto, ctx, Some(format!("the scenario did not finish (stage {stage})")));
        return;
    }
    // is what we were waiting for there?
    let ready = match &mut auto.wait {
        Wait::Frames(n) => {
            if *n == 0 {
                true
            } else {
                *n -= 1;
                false
            }
        }
        Wait::Idle => {
            if app.running || !app.checked() {
                auto.settle = 0;
                false
            } else {
                auto.settle += 1;
                auto.settle > 6
            }
        }
        Wait::Shot(name) => {
            let name: &'static str = name;
            if !auto.asked {
                auto.asked = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                false
            } else {
                let image = ctx.input(|i| {
                    i.raw.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                match image {
                    Some(image) => {
                        auto.asked = false;
                        auto.size = image.size;
                        match save(&auto.dir, name, &image) {
                            Ok(()) => {
                                auto.shots.push(name.to_string());
                                true
                            }
                            Err(e) => {
                                finish(app, &mut auto, ctx, Some(format!("{name}: {e}")));
                                return;
                            }
                        }
                    }
                    None => false,
                }
            }
        }
    };
    if ready {
        if auto.stage == STAGES.len() {
            finish(app, &mut auto, ctx, None);
            return;
        }
        auto.settle = 0;
        auto.notes.again = false;
        auto.wait = STAGES[auto.stage](app, ctx, &mut auto.notes);
        if !auto.notes.again {
            auto.stage += 1;
        }
    }
    app.auto = Some(auto);
}
