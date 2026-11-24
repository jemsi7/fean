//! The eight result views. They only read the state and say what a click asks for (`Act`);
//! the application applies it after the frame. Clicking an element of a result moves the editor
//! to the line that wrote it (DR-29): results are looked at here and changed in the code.

use eframe::egui;
use egui::epaint::QuadraticBezierShape;
use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use egui_extras::{Column, TableBuilder};
use fean_core::lower::SourceMap;
use fean_core::session::Verdict;
use fean_core::solve::Verdict as SolveVerdict;
use fean_core::trace::{Analysis, Finding, Origin, Severity, StepInfo};
use fean_core::vm::Status;

use crate::app::{actor_color, palette, Act, IdeApp, Palette, View};

pub fn kind_name(kind: &str) -> &'static str {
    match kind {
        "init" => "시작",
        "tick" => "시각",
        "commit" => "커밋",
        "reject" => "거절",
        "fail" => "실패",
        "branch" => "가지",
        "show" => "메뉴",
        "pick" => "선택",
        "mark" => "지점",
        _ => "끝",
    }
}

fn query_kind(kind: &str) -> &'static str {
    match kind {
        "exists" => "가능성 (exists)",
        "forall" => "보장 (forall)",
        "can" => "전략 (can)",
        "num" => "수치",
        "region" => "영역 (region)",
        _ => "선호 (prefer)",
    }
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

/// Columns stretch what is put in them; this keeps buttons and tables at their own size.
fn plain() -> egui::Layout {
    egui::Layout::top_down(egui::Align::LEFT)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong().size(ui.text_style_height(&egui::TextStyle::Body) + 1.0));
    ui.add_space(2.0);
}

fn note(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).color(pal.muted));
}

fn card(ui: &mut egui::Ui, pal: &Palette, bar: Color32, add: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::NONE
        .fill(pal.panel)
        .stroke(Stroke::new(1.0, pal.faint))
        .corner_radius(6)
        .inner_margin(egui::Margin { left: 14, right: 10, top: 8, bottom: 8 });
    let inner = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    let r = inner.response.rect;
    ui.painter().rect_filled(Rect::from_min_max(r.min, egui::pos2(r.min.x + 4.0, r.max.y)), 2.0, bar);
    ui.add_space(4.0);
}

fn jump_link(ui: &mut egui::Ui, acts: &mut Vec<Act>, text: impl Into<egui::WidgetText>, line: Option<u32>) {
    match line {
        Some(line) => {
            if ui.link(text).on_hover_text(format!("코드로 이동 (줄 {line})")).clicked() {
                acts.push(Act::Jump(line));
            }
        }
        None => {
            ui.label(text).on_hover_text("표준 계약이 쓴 요소라 이 글에 줄이 없습니다");
        }
    }
}

/// What a case is called where it is chosen: its label, or (the model as declared, without
/// parameters) a name for that.
fn case_name(label: &str) -> &str {
    if label.is_empty() {
        "모델 그대로 (처음부터 실행)"
    } else {
        label
    }
}

fn line_of(loc: Option<fean_core::Loc>) -> Option<u32> {
    SourceMap::line(loc)
}

// ---- the frame of the results ------------------------------------------------------------------

pub fn draw(app: &IdeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let pal = palette(app.dark);
    ui.horizontal_wrapped(|ui| {
        for view in View::ALL {
            let mut label = RichText::new(view.name());
            if view == View::Findings {
                let n = app.findings.iter().filter(|f| f.kind != "unknown").count() + app.run_error.is_some() as usize;
                if n > 0 {
                    label = RichText::new(format!("{}  {n}", view.name())).color(pal.error);
                }
            }
            if ui.selectable_label(app.view == view, label).clicked() {
                acts.push(Act::View(view));
            }
        }
    });
    ui.separator();
    let Some(a) = &app.analysis else {
        empty(app, ui, acts, &pal);
        return;
    };
    if app.stale() {
        let frame = egui::Frame::NONE.fill(pal.warn.gamma_multiply(0.16)).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 5));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("결과 낡음").strong().color(pal.warn));
                let why = if app.run_error.is_some() {
                    "지금 글은 컴파일되지 않습니다. 아래는 마지막으로 성공한 실행의 결과입니다."
                } else {
                    "글이 바뀌었습니다. 아래는 마지막 실행의 결과입니다."
                };
                ui.label(why);
                if ui.add_enabled(!app.running, egui::Button::new("다시 실행")).clicked() {
                    acts.push(Act::Run);
                }
            });
        });
        ui.add_space(2.0);
    }
    // which case, which history
    let cases = a.slots.iter().any(|s| s.origin != Origin::Assignment);
    ui.horizontal(|ui| {
        ui.label(RichText::new(if cases { "사례" } else { "배정" }).color(pal.muted));
        if a.slots.len() > 1 {
            let current = case_name(&a.slots[app.sel.assignment].label).to_string();
            egui::ComboBox::from_id_salt("assignment_combo").selected_text(current).show_ui(ui, |ui| {
                for (i, slot) in a.slots.iter().enumerate() {
                    let mark = match slot.status() {
                        "ok" => "",
                        "unknown" => "  (미판정)",
                        _ => "  (오류)",
                    };
                    if ui.selectable_label(i == app.sel.assignment, format!("{}{mark}", case_name(&slot.label))).clicked() {
                        acts.push(Act::Assignment(i));
                    }
                }
            });
        } else if cases {
            ui.label(RichText::new(&a.slots[0].label).monospace());
        } else {
            ui.label("매개변수 없음");
        }
        if matches!(app.view, View::Graph | View::Timeline | View::Actor) {
            ui.separator();
            let runs = a.runs();
            ui.label(RichText::new("이력").color(pal.muted));
            if ui.add_enabled(app.sel.run > 0, egui::Button::new("◀")).clicked() {
                acts.push(Act::SelectRun(app.sel.run - 1));
            }
            ui.label(format!("{} / {}", if runs.is_empty() { 0 } else { app.sel.run + 1 }, runs.len()));
            if ui.add_enabled(app.sel.run + 1 < runs.len(), egui::Button::new("▶")).clicked() {
                acts.push(Act::SelectRun(app.sel.run + 1));
            }
            if let Some(run) = runs.get(app.sel.run) {
                let color = if run.outcome.is_some() { pal.error } else { pal.text };
                ui.label(RichText::new(short(&run.label, 60)).color(color)).on_hover_text(&run.label);
            }
        }
    });
    ui.add_space(2.0);
    let slot = a.slot();
    if let (Some(e), true) = (&slot.error, !matches!(app.view, View::Overview | View::Findings | View::Solve | View::Compile)) {
        card(ui, &pal, if slot.status() == "unknown" { pal.muted } else { pal.error }, |ui| {
            let what = if slot.origin == Origin::Assignment { "배정" } else { "사례" };
            ui.label(RichText::new(format!("이 {what}은 실행되지 않았습니다: [{}]", e.code)).strong());
            ui.label(&e.message);
            if slot.origin.solve().is_some() {
                note(ui, &pal, "이 값에서는 모델이 유효하지 않습니다. 그래서 해가 아닙니다.");
            }
        });
        return;
    }
    match app.view {
        View::Overview => overview(app, a, ui, acts, &pal),
        View::Findings => findings(app, a, ui, acts, &pal),
        View::Solve => solving(app, a, ui, acts, &pal),
        View::Graph => graph(app, a, ui, acts, &pal),
        View::Timeline => timeline(app, a, ui, acts, &pal),
        View::Actor => actor(app, a, ui, acts, &pal),
        View::Histories => histories(app, a, ui, acts, &pal),
        View::Compile => compiled(app, a, ui, acts, &pal),
    }
}

fn empty(app: &IdeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    ui.add_space(24.0);
    if let Some(f) = &app.run_error {
        card(ui, pal, pal.error, |ui| {
            ui.label(RichText::new(&f.title).strong().color(pal.error));
            ui.label(&f.detail);
            jump_link(ui, acts, "코드로 이동", line_of(f.loc));
        });
        note(ui, pal, "글을 고친 뒤 다시 실행하십시오.");
    } else if app.running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("컴파일하고 실행하는 중…");
        });
    } else {
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("아직 결과가 없습니다").heading());
            ui.add_space(6.0);
            note(ui, pal, "실행을 누르면 글을 컴파일하고 모든 이력을 따라가, 결과를 이 자리에 보여 줍니다.");
            ui.add_space(8.0);
            if ui.button("▶  실행  (⌘R)").clicked() {
                acts.push(Act::Run);
            }
        });
    }
}

// ---- 1. overview and queries -------------------------------------------------------------------

fn stat(ui: &mut egui::Ui, pal: &Palette, label: &str, value: String) {
    egui::Frame::NONE.fill(pal.panel).stroke(Stroke::new(1.0, pal.faint)).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 6)).show(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).small().color(pal.muted));
                ui.label(RichText::new(value).strong());
            });
        },
    );
}

fn overview(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    egui::ScrollArea::vertical().id_salt("overview_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            stat(ui, pal, "모델", im.model.clone());
            stat(ui, pal, "구성원", im.actors.len().to_string());
            stat(ui, pal, "자산", im.assets.len().to_string());
            stat(ui, pal, "셀", im.cells.len().to_string());
            stat(ui, pal, "사건", im.events.len().to_string());
            stat(ui, pal, "질의", im.queries.len().to_string());
            if !im.unknowns.is_empty() {
                stat(ui, pal, "미지수", im.unknowns.len().to_string());
            }
            if !im.solves.is_empty() {
                stat(ui, pal, "역산 질의", im.solves.len().to_string());
            }
            let cases = a.slots.iter().any(|s| s.origin != Origin::Assignment);
            stat(ui, pal, if cases { "사례" } else { "배정" }, a.slots.len().to_string());
            let slot = a.slot();
            if let Some(vm) = &slot.vm {
                stat(ui, pal, "초기 세계", vm.worlds.len().to_string());
                stat(ui, pal, "관찰 지점", vm.nodes.len().to_string());
                stat(ui, pal, "완결 이력", slot.runs.len().to_string());
            }
            stat(ui, pal, "실행 시간", format!("{:.0} ms", app.run_ms));
        });

        if !im.unknowns.is_empty() {
            note(
                ui,
                pal,
                "이 모델에는 미지수가 있습니다. 질의는 역산 질의마다 그 해에서 답하고, 아래 줄은 지금 고른 사례에서의 답입니다. 역산 보기에서 해를 고릅니다.",
            );
        }
        section(ui, "질의");
        if a.answers().is_empty() {
            note(ui, pal, "이 모델에는 질의가 없습니다. query 선언을 더하면 결과가 여기에 나옵니다.");
        }
        let many = a.slots.len() > 1;
        for (qi, answer) in a.answers().iter().enumerate() {
            let row = answer.rows.get(app.sel.assignment);
            let bar = match row.and_then(|r| r.verdict.as_ref()) {
                _ if row.is_some_and(|r| r.error.is_some()) => pal.error,
                Some(Verdict::Bool(false)) if answer.kind == "forall" || answer.kind == "can" => pal.warn,
                Some(Verdict::Unknown) => pal.muted,
                _ => pal.ok,
            };
            card(ui, pal, bar, |ui| {
                ui.horizontal_wrapped(|ui| {
                    jump_link(ui, acts, RichText::new(&answer.name).strong().monospace(), line_of(answer.loc));
                    ui.label(RichText::new(query_kind(answer.kind)).small().color(pal.muted));
                    if answer.kind == "region" {
                        ui.label(RichText::new(format!("안쪽: {}", query_kind(answer.inner_kind))).small().color(pal.muted));
                    }
                });
                ui.label(RichText::new(&answer.text).monospace().strong());
                let Some(row) = row else { return };
                ui.horizontal_wrapped(|ui| {
                    if many {
                        let label = case_name(&a.slots[row.assignment].label);
                        ui.label(RichText::new(format!("{label} 에서: {}", row.text)).color(pal.muted));
                    }
                    if let Some(e) = &row.error {
                        ui.label(RichText::new(format!("[{}] {}", e.code, e.message)).color(pal.error));
                    }
                    if let Some(run) = row.witness {
                        let text = if answer.inner_kind == "num" { "이 값을 내는 이력 보기" } else { "증인 이력 보기" };
                        if ui.button(text).on_hover_text("식이 성립하는 이력으로 갑니다").clicked() {
                            acts.push(Act::Show { assignment: row.assignment, run, step: None, view: View::Timeline });
                        }
                    }
                    if let Some(run) = row.counter {
                        if ui.button("반례 이력 보기").on_hover_text("식이 깨지는 이력으로 갑니다").clicked() {
                            acts.push(Act::Show { assignment: row.assignment, run, step: None, view: View::Timeline });
                        }
                    }
                });
                if !row.policy.is_empty() {
                    ui.label(RichText::new("찾은 전략 (정보 집합 → 행동)").small().color(pal.muted));
                    for p in &row.policy {
                        ui.horizontal(|ui| {
                            ui.painter().circle_filled(ui.cursor().min + egui::vec2(5.0, 9.0), 4.0, actor_color(p.actor));
                            ui.add_space(14.0);
                            let link = ui.link(RichText::new(&p.label).monospace());
                            if let (true, Some(run)) = (link.on_hover_text("이 정보 집합의 지점으로 갑니다").clicked(), p.run) {
                                acts.push(Act::Actor(p.actor));
                                acts.push(Act::Show { assignment: row.assignment, run, step: Some(p.step), view: View::Actor });
                            }
                        });
                    }
                }
                let _ = qi;
            });
        }

        if many {
            let cases = a.slots.iter().any(|s| s.origin != Origin::Assignment);
            section(ui, if cases { "사례" } else { "배정" });
            egui::Grid::new("assignment_grid").striped(true).num_columns(6).spacing([18.0, 4.0]).show(ui, |ui| {
                for head in [if cases { "사례" } else { "배정" }, "상태", "초기 세계", "관찰 지점", "완결 이력", ""] {
                    ui.label(RichText::new(head).small().color(pal.muted));
                }
                ui.end_row();
                for (i, slot) in a.slots.iter().enumerate() {
                    ui.label(RichText::new(case_name(&slot.label)).monospace());
                    match &slot.error {
                        None => ui.label(RichText::new("실행됨").color(pal.ok)),
                        Some(e) if slot.status() == "unknown" => ui.label(RichText::new("미판정").color(pal.muted)).on_hover_text(&e.message),
                        Some(e) => ui.label(RichText::new(format!("오류 [{}]", e.code)).color(pal.error)).on_hover_text(&e.message),
                    };
                    let count = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_else(|| "–".to_string());
                    ui.label(count(slot.vm.as_ref().map(|vm| vm.worlds.len())));
                    ui.label(count(slot.vm.as_ref().map(|vm| vm.nodes.len())));
                    ui.label(count(slot.vm.as_ref().map(|_| slot.runs.len())));
                    if i == app.sel.assignment {
                        ui.label(RichText::new("보는 중").color(pal.accent));
                    } else if ui.small_button("보기").clicked() {
                        acts.push(Act::Assignment(i));
                    }
                    ui.end_row();
                }
            });
        }
    });
}

// ---- 2. contradictions -------------------------------------------------------------------------

fn finding_card(ui: &mut egui::Ui, pal: &Palette, f: &Finding, acts: &mut Vec<Act>, stale: bool) {
    let (color, tag) = match (f.severity, f.kind) {
        (Severity::Error, "compile") => (pal.error, "컴파일 오류"),
        (Severity::Error, "model") => (pal.error, "실행 오류"),
        (Severity::Error, _) => (pal.error, "실패한 필수 사건"),
        (Severity::Warning, "unsolved") => (pal.warn, "해 없음"),
        (Severity::Warning, _) => (pal.warn, "깨진 보장"),
        (Severity::Info, _) => (pal.muted, "미판정"),
    };
    card(ui, pal, color, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tag).small().strong().color(color));
            ui.label(RichText::new(&f.title).strong());
        });
        ui.label(&f.detail);
        ui.horizontal(|ui| {
            if let Some(line) = line_of(f.loc) {
                if ui.button(format!("코드로 이동 (줄 {line})")).clicked() {
                    acts.push(Act::Jump(line));
                }
            }
            if let (Some(assignment), Some(run), false) = (f.assignment, f.run, stale && f.kind == "compile") {
                let text = if f.kind == "guarantee" { "반례 이력 보기" } else { "이력 보기" };
                if ui.button(text).clicked() {
                    acts.push(Act::Show { assignment, run, step: f.step, view: View::Timeline });
                }
            }
            if f.solve.is_some() {
                if ui.button("역산 보기").on_hover_text("이 역산 질의의 영역을 봅니다").clicked() {
                    acts.push(Act::View(View::Solve));
                }
                if let Some(case) = f.assignment {
                    if ui.button("해가 아닌 값 보기").on_hover_text("목표가 성립하지 않는 값에서 실행한 이력으로 갑니다").clicked() {
                        acts.push(Act::Show { assignment: case, run: 0, step: None, view: View::Timeline });
                    }
                }
            }
            if f.count > 1 {
                ui.label(RichText::new(format!("같은 실패 {}건", f.count)).small().color(pal.muted));
            }
        });
    });
}

fn findings(app: &IdeApp, _a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    egui::ScrollArea::vertical().id_salt("findings_scroll").auto_shrink([false, false]).show(ui, |ui| {
        if let Some(f) = &app.run_error {
            finding_card(ui, pal, f, acts, true);
        }
        let real: Vec<&Finding> = app.findings.iter().collect();
        if real.is_empty() && app.run_error.is_none() {
            card(ui, pal, pal.ok, |ui| {
                ui.label(RichText::new("모순이 없습니다").strong().color(pal.ok));
                ui.label("모든 필수 사건이 성립하고, 보장 질의(forall, can)가 모두 참이며, 역산 질의마다 해가 있습니다.");
            });
            return;
        }
        if !real.is_empty() {
            let contradictions = real.iter().filter(|f| f.kind != "unknown").count();
            note(ui, pal, &format!("모순 {contradictions}건, 미판정 {}건. 항목의 단추로 원인이 된 코드와 이력으로 갑니다.", real.len() - contradictions));
            ui.add_space(4.0);
        }
        for f in real {
            finding_card(ui, pal, f, acts, false);
        }
    });
}

// ---- 3. solving backwards ----------------------------------------------------------------------

fn verdict_name(v: &SolveVerdict) -> (&'static str, String) {
    match v {
        SolveVerdict::Yes => ("해", String::new()),
        SolveVerdict::No => ("해 아님", "목표가 성립하지 않습니다".into()),
        SolveVerdict::Outside => ("후보 아님", "제약(where), 불변식 또는 범위 밖입니다".into()),
        SolveVerdict::Invalid(code) => ("무효", format!("이 값에서는 모델이 유효하지 않습니다 [{code}]")),
        SolveVerdict::Undecided(why) => ("미판정", why.clone()),
    }
}

fn form_name(form: &str) -> &'static str {
    match form {
        "find" => "해 전체 (find)",
        "any" => "해 하나 (any)",
        "min" => "최소 (min)",
        _ => "최대 (max)",
    }
}

fn solving(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    egui::ScrollArea::vertical().id_salt("solve_scroll").auto_shrink([false, false]).show(ui, |ui| {
        if a.solves().is_empty() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("이 모델에는 역산 질의가 없습니다").heading());
                ui.add_space(6.0);
                note(ui, pal, "값을 정하지 않을 상수를 unknown 으로 선언하고, solve 로 목표가 성립하는 값을 묻습니다.");
                ui.add_space(4.0);
                ui.label(RichText::new("unknown cap : Num in 0..200").monospace());
                ui.label(RichText::new("solve need = min cap: repaid").monospace());
                ui.add_space(4.0);
                note(ui, pal, "파일 › 예제 열기의 Capital 과 Escrow 에 본보기가 있습니다.");
            });
            return;
        }
        if !im.unknowns.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("미지수").color(pal.muted));
                for (i, u) in im.unknowns.iter().enumerate() {
                    let range = match u.kind.as_str() {
                        "bool" => "참·거짓".to_string(),
                        "enum" => u.enum_.map(|e| im.enums[e].1.join(" | ")).unwrap_or_default(),
                        "actor" => im.actors.join(" | "),
                        "time" => format!("@{}..@{}", u.lo, u.hi),
                        _ => format!("{}..{}", u.lo, u.hi),
                    };
                    jump_link(ui, acts, RichText::new(&u.name).monospace().strong(), line_of(a.map.unknowns.get(i).copied().flatten()));
                    ui.label(RichText::new(range).small().color(pal.muted));
                    ui.add_space(8.0);
                }
            });
        }
        let current = a.slot();
        for (s, sv) in a.solves().iter().enumerate() {
            let solved_all = sv.rows.iter().all(|r| r.solved.complete && !r.solved.unknown && r.solved.error.is_none());
            let any_solution = sv.rows.iter().any(|r| r.solved.leaves.iter().any(|l| l.verdict == SolveVerdict::Yes));
            let bar = if !solved_all {
                pal.muted
            } else if any_solution {
                pal.ok
            } else {
                pal.warn
            };
            card(ui, pal, bar, |ui| {
                ui.horizontal_wrapped(|ui| {
                    jump_link(ui, acts, RichText::new(&sv.name).strong().monospace(), line_of(sv.loc));
                    ui.label(RichText::new(form_name(&sv.form)).small().color(pal.muted));
                    if let Some(mark) = im.solves[s].point {
                        let line = line_of(a.map.marks.get(mark).copied().flatten());
                        ui.label(RichText::new("지점").small().color(pal.muted));
                        jump_link(ui, acts, RichText::new(&im.marks[mark]).small().monospace(), line);
                    }
                });
                ui.label(RichText::new(short(&sv.text, 400)).monospace().strong()).on_hover_text(&sv.text);
                for (r, row) in sv.rows.iter().enumerate() {
                    let solved = &row.solved;
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        if !row.label.is_empty() {
                            ui.label(RichText::new(&row.label).monospace().color(pal.accent));
                            ui.label(RichText::new(short(&solved.text, 120)).monospace());
                        }
                        let state = if solved.error.is_some() {
                            "풀지 못함"
                        } else if solved.complete {
                            "판정 완료"
                        } else {
                            "일부 미판정"
                        };
                        ui.label(RichText::new(format!("모델 실행 {}번 · 영역 {}개 · {state}", solved.runs, solved.leaves.len())).small().color(pal.muted));
                    });
                    if let Some(why) = &solved.error {
                        ui.label(RichText::new(why).color(pal.error));
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (case, point, text, hint) in [
                            (row.solution, &solved.best, "해 보기", "이 해에서 실행한 모델을 사례로 고릅니다"),
                            (row.counter, &solved.counter, "해가 아닌 값 보기", "목표가 성립하지 않는 가장 작은 값에서 실행한 모델을 사례로 고릅니다"),
                        ] {
                            let (Some(case), Some(point)) = (case, point) else { continue };
                            let chosen = app.sel.assignment == case;
                            let label = format!("{text}: {}", short(&solved.point_text(point), 60));
                            if ui.add(egui::Button::new(label).selected(chosen)).on_hover_text(hint).clicked() {
                                acts.push(Act::Assignment(case));
                            }
                        }
                    });
                    if solved.leaves.is_empty() {
                        continue;
                    }
                    egui::CollapsingHeader::new(format!("영역 {}개", solved.leaves.len()))
                        .id_salt(("regions", s, r))
                        .default_open(solved.leaves.len() <= 12)
                        .show(ui, |ui| {
                            egui::Grid::new(("region_grid", s, r)).striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                                for head in ["판정", "조건 (이 조건을 모두 지키는 후보)", "실행한 값", ""] {
                                    ui.label(RichText::new(head).small().color(pal.muted));
                                }
                                ui.end_row();
                                for leaf in solved.leaves.iter().take(200) {
                                    let (name, why) = verdict_name(&leaf.verdict);
                                    let color = match leaf.verdict {
                                        SolveVerdict::Yes => pal.ok,
                                        SolveVerdict::No => pal.text,
                                        SolveVerdict::Invalid(_) => pal.error,
                                        _ => pal.muted,
                                    };
                                    let shown = ui.label(RichText::new(name).color(color).strong());
                                    if !why.is_empty() {
                                        shown.on_hover_text(why);
                                    }
                                    let text = solved.region_text(&leaf.region);
                                    ui.label(RichText::new(short(&text, 110)).monospace()).on_hover_text(text);
                                    let real = &leaf.sample[..solved.vars.len().min(leaf.sample.len())];
                                    let at = solved.point_text(real);
                                    ui.label(RichText::new(short(&at, 60)).monospace().color(pal.muted)).on_hover_text(&at);
                                    let picked = current.origin.solve() == Some(s) && current.point.as_deref() == Some(real);
                                    if real.len() != solved.vars.len() || matches!(leaf.verdict, SolveVerdict::Outside) {
                                        ui.label("");
                                    } else if picked {
                                        ui.label(RichText::new("보는 중").color(pal.accent));
                                    } else if ui.small_button("이 값으로 보기").on_hover_text("이 값에서 모델을 실행해 사례로 엽니다").clicked() {
                                        acts.push(Act::Case { solve: s, row: r, point: real.to_vec() });
                                    }
                                    ui.end_row();
                                }
                            });
                            if solved.leaves.len() > 200 {
                                note(ui, pal, &format!("영역이 많아 처음 200개만 보입니다 (모두 {}개).", solved.leaves.len()));
                            }
                        });
                }
            });
        }
        if current.origin.solve().is_some() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("지금 고른 사례").color(pal.muted));
                ui.label(RichText::new(short(&current.label, 90)).monospace()).on_hover_text(&current.label);
                for (view, text) in [(View::Timeline, "재생 보기"), (View::Graph, "거래 그래프 보기"), (View::Histories, "이력 보기")] {
                    if ui.button(text).clicked() {
                        acts.push(Act::View(view));
                    }
                }
            });
        }
    });
}

// ---- 4. the transaction graph ------------------------------------------------------------------

fn actor_chips(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        for (i, name) in a.image.actors.iter().enumerate() {
            let selected = i == app.sel.actor;
            let text = RichText::new(format!("●  {name}")).color(actor_color(i));
            let text = if selected { text.strong() } else { text };
            if ui.selectable_label(selected, text).clicked() {
                acts.push(Act::Actor(i));
            }
        }
    });
}

fn graph(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    let steps = &app.cache.steps;
    let n = im.actors.len();
    ui.horizontal_wrapped(|ui| {
        actor_chips(app, a, ui, acts);
        let mut only = app.only_actor;
        if ui.checkbox(&mut only, "선택한 주체의 거래만").changed() {
            acts.push(Act::OnlyActor(only));
        }
        if let Some(step) = steps.get(app.sel.step) {
            ui.label(RichText::new(format!("재생 위치: 걸음 {} / {} · 시각 @{}", app.sel.step + 1, steps.len(), step.time)).color(pal.muted));
        }
    });
    replay_bar(app, ui, acts, pal);
    let height = (ui.available_height() - 150.0).clamp(240.0, 640.0);
    let (response, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), height), Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 6.0, pal.panel);
    let center = rect.center();
    let radius = (rect.width().min(rect.height()) / 2.0 - 70.0).max(60.0);
    let node_r = 27.0;
    let place = |i: usize| -> Pos2 {
        if n == 1 {
            return center;
        }
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
        center + radius * egui::vec2(angle.cos(), angle.sin())
    };
    let now: Vec<(Option<usize>, Option<usize>, usize)> = steps
        .get(app.sel.step)
        .map(|s| {
            let mut v: Vec<_> = s.moves.iter().map(|m| (Some(m.from), Some(m.to), m.asset)).collect();
            v.extend(s.mints.iter().map(|m| if m.amount > 0 { (None, Some(m.actor), m.asset) } else { (Some(m.actor), None, m.asset) }));
            v
        })
        .unwrap_or_default();
    let small = FontId::proportional(ui.text_style_height(&egui::TextStyle::Small));
    let body = FontId::proportional(ui.text_style_height(&egui::TextStyle::Body) - 1.0);
    let mut seen_pairs: Vec<(usize, usize)> = Vec::new();
    for edge in &app.cache.graph {
        let touches = edge.from == Some(app.sel.actor) || edge.to == Some(app.sel.actor);
        let fade = if app.only_actor && !touches { 0.15 } else { 1.0 };
        let live = now.contains(&(edge.from, edge.to, edge.asset));
        let base = actor_color(edge.from.or(edge.to).unwrap_or(0));
        let color = if edge.amount == 0 { pal.muted.gamma_multiply(fade) } else { base.gamma_multiply(fade) };
        let width = if live { 3.5 } else { 1.8 };
        let amount = format!("{}{}", a.amount_text(edge.asset, edge.amount), if edge.count > 1 { format!("  ×{}", edge.count) } else { String::new() });
        // where the label hangs: a point on the mark, and the side it is pushed out to
        let (label_at, outward): (egui::Vec2, egui::Vec2);
        match (edge.from, edge.to) {
            (Some(f), Some(t)) if f != t => {
                let (p, q) = (place(f), place(t));
                let pair = (f.min(t), f.max(t));
                let nth = seen_pairs.iter().filter(|x| **x == pair).count();
                seen_pairs.push(pair);
                let d = q - p;
                // the normal turns with the direction, so the way back bends to the other side
                let normal = egui::vec2(-d.y, d.x).normalized();
                let bend = 30.0 + 26.0 * nth as f32;
                let control = p + d * 0.5 + normal * bend;
                let start = p + (control - p).normalized() * node_r;
                let end = q + (control - q).normalized() * (node_r + 3.0);
                painter.add(QuadraticBezierShape::from_points_stroke([start, control, end], false, Color32::TRANSPARENT, Stroke::new(width, color)));
                let dir = (end - control).normalized();
                let perp = egui::vec2(-dir.y, dir.x);
                painter.add(Shape::convex_polygon(vec![end, end - dir * 11.0 + perp * 5.0, end - dir * 11.0 - perp * 5.0], color, Stroke::NONE));
                label_at = start.to_vec2() * 0.25 + control.to_vec2() * 0.5 + end.to_vec2() * 0.25;
                outward = normal;
            }
            (Some(f), Some(_)) => {
                let p = place(f);
                painter.circle_stroke(p + egui::vec2(0.0, -node_r - 10.0), 12.0, Stroke::new(width, color));
                label_at = (p + egui::vec2(0.0, -node_r - 24.0)).to_vec2();
                outward = egui::vec2(0.0, -1.0);
            }
            (from, to) => {
                // creation (into an actor) or destruction (out of one): a stub pointing away from the centre
                let who = from.or(to).unwrap_or(0);
                let p = place(who);
                // to the side of the node, so the stub stays clear of the holdings written above or below it
                let away = if n == 1 { egui::vec2(0.0, -1.0) } else { (p - center).normalized() };
                let out = egui::vec2(-away.y, away.x);
                let (near, far) = (p + out * (node_r + 3.0), p + out * (node_r + 34.0));
                let (tail, tip) = if to.is_some() { (far, near) } else { (near, far) };
                painter.arrow(tail, tip - tail, Stroke::new(width, color));
                label_at = (p + out * (node_r + 36.0)).to_vec2();
                outward = out;
            }
        }
        let text = match (edge.from, edge.to) {
            (None, _) => format!("발행 {amount}"),
            (_, None) => format!("소각 {amount}"),
            _ => amount,
        };
        let galley = painter.layout_no_wrap(text, if live { body.clone() } else { small.clone() }, color);
        // beside the mark, not on it: far enough out that the label clears the line
        let reach = 0.5 * (outward.x.abs() * galley.size().x + outward.y.abs() * galley.size().y) + 7.0;
        let spot = Rect::from_center_size((label_at + outward * reach).to_pos2(), galley.size() + egui::vec2(8.0, 4.0));
        painter.rect_filled(spot, 4.0, pal.panel.gamma_multiply(0.92));
        painter.galley(spot.min + egui::vec2(4.0, 2.0), galley, color);
    }
    let state = a.vm().zip(steps.get(app.sel.step)).map(|(vm, s)| &vm.nodes[s.node as usize].state);
    for i in 0..n {
        let p = place(i);
        let selected = i == app.sel.actor;
        let color = actor_color(i);
        painter.circle_filled(p, node_r, color.gamma_multiply(if app.only_actor && !selected { 0.35 } else { 1.0 }));
        if selected {
            painter.circle_stroke(p, node_r + 4.0, Stroke::new(2.5, color));
        }
        painter.text(p, Align2::CENTER_CENTER, short(&im.actors[i], 8), body.clone(), Color32::WHITE);
        if let Some(state) = state {
            let held: Vec<String> = (0..im.assets.len()).map(|x| a.amount_text(x, state.cells[im.holding(x, i)])).collect();
            let below = if p.y >= center.y { 1.0 } else { -1.0 };
            let anchor = if below > 0.0 { Align2::CENTER_TOP } else { Align2::CENTER_BOTTOM };
            painter.text(p + egui::vec2(0.0, below * (node_r + 8.0)), anchor, held.join("  "), small.clone(), pal.text);
        }
        let hit = ui.interact(Rect::from_center_size(p, Vec2::splat(node_r * 2.0)), ui.id().with(("graph_node", i)), Sense::click());
        if hit.on_hover_text(format!("{} 을 고릅니다", im.actors[i])).clicked() {
            acts.push(Act::Actor(i));
        }
    }
    if app.cache.graph.is_empty() {
        painter.text(rect.center_bottom() + egui::vec2(0.0, -14.0), Align2::CENTER_BOTTOM, "이 위치까지 이전이 없습니다", small, pal.muted);
    }

    section(ui, "이전 합계 (재생 위치까지)");
    egui::ScrollArea::vertical().id_salt("edge_scroll").auto_shrink([false, false]).show(ui, |ui| {
        if app.cache.graph.is_empty() {
            note(ui, pal, "이전이 없습니다.");
            return;
        }
        egui::Grid::new("edge_grid").striped(true).num_columns(5).spacing([18.0, 4.0]).show(ui, |ui| {
            for head in ["보낸 이", "받은 이", "자산", "합계", "횟수"] {
                ui.label(RichText::new(head).small().color(pal.muted));
            }
            ui.end_row();
            for edge in &app.cache.graph {
                let name = |who: Option<usize>, none: &str| match who {
                    Some(i) => RichText::new(&im.actors[i]).color(actor_color(i)).strong(),
                    None => RichText::new(none).color(pal.muted),
                };
                ui.label(name(edge.from, "(발행)"));
                ui.label(name(edge.to, "(소각)"));
                ui.label(&im.assets[edge.asset].name);
                ui.label(RichText::new(edge.amount.to_string()).monospace());
                ui.label(edge.count.to_string());
                ui.end_row();
            }
        });
    });
}

// ---- 4. the timeline and replay ----------------------------------------------------------------

fn replay_bar(app: &IdeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let steps = &app.cache.steps;
    if steps.is_empty() {
        return;
    }
    let last = steps.len() - 1;
    ui.horizontal(|ui| {
        if ui.button("⏮").on_hover_text("처음").clicked() {
            acts.push(Act::Play(false));
            acts.push(Act::Step(0));
        }
        if ui.button("◀").on_hover_text("이전 걸음 (←)").clicked() {
            acts.push(Act::Play(false));
            acts.push(Act::Step(app.sel.step.saturating_sub(1)));
        }
        let (text, tip) = if app.playing { ("⏸  멈춤", "재생을 멈춥니다 (Space)") } else { ("▶  재생", "한 걸음씩 재생합니다 (Space)") };
        if ui.button(text).on_hover_text(tip).clicked() {
            acts.push(Act::Play(!app.playing));
        }
        if ui.button("▶").on_hover_text("다음 걸음 (→)").clicked() {
            acts.push(Act::Play(false));
            acts.push(Act::Step((app.sel.step + 1).min(last)));
        }
        if ui.button("⏭").on_hover_text("끝").clicked() {
            acts.push(Act::Play(false));
            acts.push(Act::Step(last));
        }
        let mut at = app.sel.step;
        ui.spacing_mut().slider_width = (ui.available_width() - 250.0).clamp(80.0, 420.0);
        if ui.add(egui::Slider::new(&mut at, 0..=last).show_value(false)).changed() {
            acts.push(Act::Step(at));
        }
        ui.label(RichText::new(format!("걸음 {} / {}", app.sel.step + 1, steps.len())).monospace());
        egui::ComboBox::from_id_salt("speed_combo").width(64.0).selected_text(format!("{}×", app.speed)).show_ui(ui, |ui| {
            for speed in [0.5, 1.0, 2.0, 4.0] {
                if ui.selectable_label(app.speed == speed, format!("{speed}×")).clicked() {
                    acts.push(Act::Speed(speed));
                }
            }
        });
        let _ = pal;
    });
}

fn timeline(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    let steps = &app.cache.steps;
    if steps.is_empty() {
        note(ui, pal, "이 배정에는 이력이 없습니다.");
        return;
    }
    replay_bar(app, ui, acts, pal);
    let n = im.actors.len();
    let (left_w, col_w, head_h, lane_h) = (104.0, 76.0, 48.0, 46.0);
    let size = egui::vec2(left_w + col_w * steps.len() as f32 + 16.0, head_h + lane_h * n as f32 + 10.0);
    let small = FontId::proportional(ui.text_style_height(&egui::TextStyle::Small));
    let tiny = FontId::proportional(ui.text_style_height(&egui::TextStyle::Small) - 1.0);
    let body = FontId::proportional(ui.text_style_height(&egui::TextStyle::Body));
    // keep the current step in sight: when it moves out of the visible columns, centre it
    let (step_key, offset_key) = (ui.id().with("timeline_step"), ui.id().with("timeline_offset"));
    let view_w = ui.available_width();
    let current = (app.result_gen, app.sel.run, app.sel.step, (view_w / 16.0) as i32);
    let last: Option<(u64, usize, usize, i32)> = ui.data(|d| d.get_temp(step_key));
    let mut area = egui::ScrollArea::horizontal().id_salt("timeline_scroll").auto_shrink([false, true]);
    if last != Some(current) {
        ui.data_mut(|d| d.insert_temp(step_key, current));
        let offset: f32 = ui.data(|d| d.get_temp(offset_key)).unwrap_or(0.0);
        let (from, to) = (col_w * app.sel.step as f32, col_w * (app.sel.step as f32 + 1.0));
        let room = (view_w - left_w).max(col_w);
        if from < offset + col_w * 0.5 || to > offset + room - col_w * 0.5 {
            area = area.horizontal_scroll_offset((from + col_w / 2.0 - room / 2.0).max(0.0));
        }
    }
    let shown = area.show(ui, |ui| {
        let (response, painter) = ui.allocate_painter(size, Sense::click());
        let o = response.rect.min;
        let lane_y = |i: usize| o.y + head_h + lane_h * (i as f32 + 0.5);
        let col_x = |k: usize| o.x + left_w + col_w * (k as f32 + 0.5);
        let bottom = o.y + head_h + lane_h * n as f32;
        painter.rect_filled(response.rect, 6.0, pal.panel);
        // bands of public time
        let mut from = 0;
        while from < steps.len() {
            let mut to = from;
            while to + 1 < steps.len() && steps[to + 1].time == steps[from].time {
                to += 1;
            }
            if steps[from].time % 2 == 1 {
                let band = Rect::from_min_max(egui::pos2(col_x(from) - col_w / 2.0, o.y), egui::pos2(col_x(to) + col_w / 2.0, bottom));
                painter.rect_filled(band, 0.0, pal.faint.gamma_multiply(0.45));
            }
            from = to + 1;
        }
        for i in 0..n {
            painter.line_segment([egui::pos2(o.x + left_w, lane_y(i)), egui::pos2(o.x + size.x, lane_y(i))], Stroke::new(1.0, pal.faint));
        }
        // the current step
        let here = Rect::from_min_max(egui::pos2(col_x(app.sel.step) - col_w / 2.0, o.y), egui::pos2(col_x(app.sel.step) + col_w / 2.0, bottom));
        painter.rect_filled(here, 4.0, pal.accent.gamma_multiply(0.13));
        painter.rect_filled(Rect::from_min_max(here.min, egui::pos2(here.max.x, here.min.y + 3.0)), 0.0, pal.accent);
        for (k, step) in steps.iter().enumerate() {
            let x = col_x(k);
            let fade = if k > app.sel.step { 0.4 } else { 1.0 };
            let ink = |c: Color32| c.gamma_multiply(fade);
            let tag_color = match step.kind {
                "fail" => pal.error,
                "reject" => pal.muted,
                "commit" => pal.text,
                _ => pal.muted,
            };
            painter.text(egui::pos2(x, o.y + 14.0), Align2::CENTER_CENTER, kind_name(step.kind), tiny.clone(), ink(tag_color));
            let title = match step.kind {
                "init" => String::new(),
                "end" => String::new(),
                _ => short(&step.title, 10),
            };
            let title_color = match step.kind {
                "fail" => pal.error,
                "reject" => pal.muted,
                "tick" => pal.accent,
                _ => pal.text,
            };
            painter.text(egui::pos2(x, o.y + 32.0), Align2::CENTER_CENTER, title, small.clone(), ink(title_color));
            match step.kind {
                "tick" => {
                    painter.line_segment([egui::pos2(x - col_w / 2.0, o.y + head_h - 6.0), egui::pos2(x - col_w / 2.0, bottom)], Stroke::new(1.5, ink(pal.accent)));
                }
                "commit" => {
                    let count = step.moves.len();
                    for (j, m) in step.moves.iter().enumerate() {
                        let dx = (j as f32 - (count as f32 - 1.0) / 2.0) * 16.0;
                        let (y0, y1) = (lane_y(m.from), lane_y(m.to));
                        let color = ink(if m.amount == 0 { pal.muted } else { actor_color(m.from) });
                        if m.from == m.to {
                            painter.circle_stroke(egui::pos2(x + dx, y0), 7.0, Stroke::new(2.0, color));
                        } else {
                            let dir = if y1 > y0 { 1.0 } else { -1.0 };
                            let (tail, tip) = (egui::pos2(x + dx, y0 + dir * 6.0), egui::pos2(x + dx, y1 - dir * 6.0));
                            painter.line_segment([tail, tip], Stroke::new(2.2, color));
                            painter.add(Shape::convex_polygon(
                                vec![tip, tip + egui::vec2(-5.0, -dir * 9.0), tip + egui::vec2(5.0, -dir * 9.0)],
                                color,
                                Stroke::NONE,
                            ));
                            painter.circle_filled(egui::pos2(x + dx, y0), 4.5, color);
                        }
                        let text = if im.assets.len() > 1 { format!("{} {}", m.amount, short(&im.assets[m.asset].name, 5)) } else { m.amount.to_string() };
                        let galley = painter.layout_no_wrap(text, tiny.clone(), color);
                        let at = egui::pos2(x + dx + 6.0, (y0 + y1) / 2.0 - galley.size().y / 2.0 + if count > 1 { (j as f32 - 0.5) * 12.0 } else { 0.0 });
                        painter.rect_filled(Rect::from_min_size(at, galley.size()).expand(1.5), 2.0, pal.panel.gamma_multiply(0.9));
                        painter.galley(at, galley, color);
                    }
                    for m in &step.mints {
                        let text = if m.amount > 0 { format!("+{}", m.amount) } else { m.amount.to_string() };
                        painter.text(egui::pos2(x, lane_y(m.actor) - 12.0), Align2::CENTER_CENTER, text, tiny.clone(), ink(actor_color(m.actor)));
                        painter.circle_stroke(egui::pos2(x, lane_y(m.actor)), 5.0, Stroke::new(2.0, ink(actor_color(m.actor))));
                    }
                    for t in &step.tells {
                        let c = egui::pos2(x + 14.0, lane_y(t.actor) + 11.0);
                        painter.rect_stroke(Rect::from_center_size(c, egui::vec2(10.0, 8.0)), 1.0, Stroke::new(1.5, ink(actor_color(t.actor))), egui::StrokeKind::Inside);
                    }
                    for &who in &step.actors {
                        let moved = step.moves.iter().any(|m| m.from == who || m.to == who) || step.mints.iter().any(|m| m.actor == who);
                        if !moved {
                            painter.circle_filled(egui::pos2(x, lane_y(who)), 4.0, ink(actor_color(who)));
                        }
                    }
                    if !step.changes.is_empty() {
                        painter.text(egui::pos2(x + col_w / 2.0 - 5.0, o.y + 14.0), Align2::RIGHT_CENTER, "Δ", tiny.clone(), ink(pal.accent));
                    }
                }
                "reject" | "fail" => {
                    let color = ink(if step.kind == "fail" { pal.error } else { pal.muted });
                    let c = egui::pos2(x, (o.y + head_h + bottom) / 2.0);
                    let r = if step.kind == "fail" { 9.0 } else { 6.0 };
                    painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], Stroke::new(2.5, color));
                    painter.line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], Stroke::new(2.5, color));
                    if step.kind == "fail" {
                        painter.rect_filled(Rect::from_min_max(egui::pos2(x - col_w / 2.0, o.y + head_h), egui::pos2(x + col_w / 2.0, bottom)), 0.0, pal.error.gamma_multiply(0.10));
                    }
                }
                "show" => {
                    for &who in &step.actors {
                        painter.circle_stroke(egui::pos2(x, lane_y(who)), 9.0, Stroke::new(2.0, ink(actor_color(who))));
                        painter.text(egui::pos2(x, lane_y(who)), Align2::CENTER_CENTER, "?", small.clone(), ink(actor_color(who)));
                    }
                }
                "pick" => {
                    for &who in &step.actors {
                        let c = egui::pos2(x, lane_y(who));
                        painter.add(Shape::convex_polygon(
                            vec![c + egui::vec2(0.0, -9.0), c + egui::vec2(9.0, 0.0), c + egui::vec2(0.0, 9.0), c + egui::vec2(-9.0, 0.0)],
                            ink(actor_color(who)),
                            Stroke::NONE,
                        ));
                    }
                }
                "mark" => {
                    let c = egui::pos2(x, o.y + head_h + 2.0);
                    painter.add(Shape::convex_polygon(vec![c + egui::vec2(-5.0, 0.0), c + egui::vec2(5.0, 0.0), c + egui::vec2(0.0, 8.0)], ink(pal.accent), Stroke::NONE));
                    painter.line_segment([egui::pos2(x, o.y + head_h + 8.0), egui::pos2(x, bottom)], Stroke::new(1.0, ink(pal.accent.gamma_multiply(0.5))));
                }
                _ => {}
            }
        }
        // actor names stay at the left edge while the columns scroll
        let left = ui.clip_rect().left().max(o.x);
        painter.rect_filled(Rect::from_min_max(egui::pos2(left, o.y), egui::pos2(left + left_w - 6.0, bottom + 6.0)), 6.0, pal.panel);
        painter.line_segment([egui::pos2(left + left_w - 6.0, o.y + 6.0), egui::pos2(left + left_w - 6.0, bottom)], Stroke::new(1.0, pal.faint));
        painter.text(egui::pos2(left + 10.0, o.y + 23.0), Align2::LEFT_CENTER, "주체 ＼ 걸음", tiny.clone(), pal.muted);
        for i in 0..n {
            let selected = i == app.sel.actor;
            painter.circle_filled(egui::pos2(left + 14.0, lane_y(i)), 5.0, actor_color(i));
            let font = if selected { body.clone() } else { small.clone() };
            painter.text(egui::pos2(left + 26.0, lane_y(i)), Align2::LEFT_CENTER, short(&im.actors[i], 9), font, if selected { actor_color(i) } else { pal.text });
        }
        // clicks: a column sets the replay position, a name picks the actor
        if let Some(p) = response.interact_pointer_pos().filter(|_| response.clicked()) {
            if p.x < left + left_w - 6.0 {
                let i = ((p.y - o.y - head_h) / lane_h).floor();
                if i >= 0.0 && (i as usize) < n {
                    acts.push(Act::Actor(i as usize));
                }
            } else {
                let k = ((p.x - o.x - left_w) / col_w).floor();
                if k >= 0.0 && (k as usize) < steps.len() {
                    acts.push(Act::Play(false));
                    acts.push(Act::Step(k as usize));
                }
            }
        }
        if let Some(p) = response.hover_pos() {
            let k = ((p.x - o.x - left_w) / col_w).floor();
            if p.x >= left + left_w - 6.0 && k >= 0.0 && (k as usize) < steps.len() {
                let step = &steps[k as usize];
                response.clone().on_hover_text_at_pointer(format!("걸음 {} · {} · {} · 시각 @{}", k as usize + 1, kind_name(step.kind), step.title, step.time));
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(offset_key, shown.state.offset.x));
    ui.add_space(4.0);
    let step = &steps[app.sel.step];
    egui::ScrollArea::vertical().id_salt("step_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.columns(2, |columns| {
            columns[0].with_layout(plain(), |ui| step_detail(app, a, step, ui, acts, pal));
            columns[1].with_layout(plain(), |ui| state_table(app, a, step, ui, pal));
        });
    });
}

fn step_detail(app: &IdeApp, a: &Analysis, step: &StepInfo, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    let name = |i: usize| RichText::new(&im.actors[i]).color(actor_color(i)).strong();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("걸음 {}", app.sel.step + 1)).strong());
        ui.label(RichText::new(kind_name(step.kind)).strong().color(if step.kind == "fail" { pal.error } else { pal.accent }));
        if !matches!(step.kind, "init" | "end") {
            ui.label(RichText::new(&step.title).monospace().strong());
        }
        ui.label(RichText::new(format!("시각 @{}", step.time)).color(pal.muted));
    });
    if let Some(line) = line_of(step.loc) {
        if ui.button(format!("이 문장의 코드로 이동 (줄 {line})")).on_hover_text("프로그램에서 이 걸음을 만든 문장").clicked() {
            acts.push(Act::Jump(line));
        }
    }
    match step.kind {
        "init" => note(ui, pal, &a.runs().get(app.sel.run).map(|r| r.label.clone()).unwrap_or_default()),
        "end" => note(ui, pal, "프로그램이 끝까지 실행되었습니다."),
        "tick" => note(ui, pal, "공개 시각이 바뀝니다. 모든 구성원이 이 시각을 봅니다."),
        "branch" => note(ui, pal, "alt 의 한 가지입니다. 어느 가지로 갈지는 누구도 정하지 않습니다."),
        "mark" => note(ui, pal, "label 로 이름 붙인 지점입니다. 질의의 at(…)과 reach(…)가 이 지점을 가리킵니다."),
        "show" => {
            if let (Some(vm), Some(&who)) = (a.vm(), step.actors.first()) {
                if let Some(d) = vm.nodes[step.node as usize].display {
                    let actions: Vec<&str> = vm.displays[d as usize].2.iter().map(|x| im.actions[*x].as_str()).collect();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(who));
                        ui.label(format!("에게 메뉴를 보입니다. 고를 수 있는 행동: {}", actions.join(", ")));
                    });
                }
            }
        }
        "pick" => {
            if let Some(&who) = step.actors.first() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(name(who));
                    ui.label(format!("가 {} 을(를) 골랐습니다.", step.title));
                });
            }
        }
        "reject" | "fail" => {
            let (color, what) = match step.status {
                Some(Status::NoSettlement) => (pal.error, "조건은 통과했지만 가능한 사후 상태가 없습니다 (NO_SETTLEMENT). 이력은 여기서 끝납니다."),
                _ if step.kind == "fail" => (pal.error, "필수 사건이 막혔습니다 (BLOCKED). 이력은 여기서 끝납니다."),
                _ => (pal.muted, "시도한 사건이 막혔습니다 (BLOCKED). 상태는 그대로이고 else 가지로 갑니다."),
            };
            ui.label(RichText::new(what).color(color));
            ui.label(format!("사유: {}", step.reason));
        }
        _ => {}
    }
    if let Some(unit) = step.unit {
        ui.add_space(4.0);
        ui.label(RichText::new("사건").small().color(pal.muted));
        for (j, (event, _)) in im.units[unit].members.iter().enumerate() {
            let text = step.events.get(j).cloned().unwrap_or_else(|| im.events[*event].name.clone());
            jump_link(ui, acts, RichText::new(text).monospace(), line_of(a.map.events.get(*event).copied().flatten()));
        }
    }
    if !step.moves.is_empty() || !step.mints.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("이전").small().color(pal.muted));
        for m in &step.moves {
            ui.horizontal(|ui| {
                ui.label(name(m.from));
                ui.label("→");
                ui.label(name(m.to));
                ui.label(RichText::new(a.amount_text(m.asset, m.amount)).monospace().strong());
            });
        }
        for m in &step.mints {
            ui.horizontal(|ui| {
                ui.label(name(m.actor));
                ui.label(if m.amount > 0 { "에게 발행" } else { "에게서 소각" });
                ui.label(RichText::new(a.amount_text(m.asset, m.amount.abs())).monospace().strong());
            });
        }
    }
    if !step.changes.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("바뀐 값").small().color(pal.muted));
        egui::Grid::new("change_grid").num_columns(4).spacing([10.0, 3.0]).show(ui, |ui| {
            for c in &step.changes {
                let loc = if c.cell < im.cells.len() {
                    a.map.cells.get(c.cell).copied().flatten()
                } else {
                    a.map.assets.get((c.cell - im.cells.len()) / im.actors.len().max(1)).copied().flatten()
                };
                jump_link(ui, acts, RichText::new(&c.name).monospace(), line_of(loc));
                ui.label(RichText::new(&c.before_text).monospace().color(pal.muted));
                ui.label("→");
                ui.label(RichText::new(&c.after_text).monospace().strong());
                ui.end_row();
            }
        });
    }
    if !step.tells.is_empty() || !step.forgets.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("통지").small().color(pal.muted));
        for t in &step.tells {
            ui.horizontal_wrapped(|ui| {
                ui.label(name(t.actor));
                ui.label(RichText::new(format!(".{} := {}  (asof @{})", t.key, t.text, t.asof)).monospace());
            });
        }
        for (who, key) in &step.forgets {
            ui.horizontal_wrapped(|ui| {
                ui.label(name(*who));
                ui.label(RichText::new(format!(".{key} 을 잊음")).monospace());
            });
        }
    }
}

fn state_table(app: &IdeApp, a: &Analysis, step: &StepInfo, ui: &mut egui::Ui, pal: &Palette) {
    let im = &a.image;
    let Some(vm) = a.vm() else { return };
    let state = &vm.nodes[step.node as usize].state;
    let changed = |cell: usize| step.changes.iter().any(|c| c.cell == cell);
    ui.label(RichText::new("이 걸음 뒤의 상태").strong());
    if !im.assets.is_empty() {
        ui.label(RichText::new("보유량").small().color(pal.muted));
        egui::Grid::new("holding_grid").striped(true).num_columns(im.assets.len() + 1).spacing([16.0, 3.0]).show(ui, |ui| {
            ui.label("");
            for asset in &im.assets {
                ui.label(RichText::new(&asset.name).small().color(pal.muted));
            }
            ui.end_row();
            for (i, who) in im.actors.iter().enumerate() {
                let text = RichText::new(who).color(actor_color(i));
                ui.label(if i == app.sel.actor { text.strong() } else { text });
                for x in 0..im.assets.len() {
                    let cell = im.holding(x, i);
                    let value = RichText::new(state.cells[cell].to_string()).monospace();
                    ui.label(if changed(cell) { value.strong().color(pal.accent) } else { value });
                }
                ui.end_row();
            }
        });
    }
    if !im.cells.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("셀").small().color(pal.muted));
        egui::Grid::new("cell_grid").striped(true).num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
            for (c, cell) in im.cells.iter().enumerate() {
                ui.label(RichText::new(&cell.name).monospace());
                let value = RichText::new(a.cell_text(c, state.cells[c])).monospace();
                ui.label(if changed(c) { value.strong().color(pal.accent) } else { value });
                ui.end_row();
            }
        });
    }
}

// ---- 5. one actor ------------------------------------------------------------------------------

fn actor(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    let steps = &app.cache.steps;
    actor_chips(app, a, ui, acts);
    replay_bar(app, ui, acts, pal);
    let Some(view) = &app.cache.view else {
        note(ui, pal, "이 배정에는 이력이 없습니다.");
        return;
    };
    let who = app.sel.actor;
    let color = actor_color(who);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&im.actors[who]).heading().color(color));
        if let Some(step) = steps.get(app.sel.step) {
            ui.label(RichText::new(format!("걸음 {} ({} {}) 뒤 · 시각 @{}", app.sel.step + 1, kind_name(step.kind), step.title, view.time)).color(pal.muted));
        }
    });
    egui::ScrollArea::vertical().id_salt("actor_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.columns(2, |columns| {
            let (first, second) = columns.split_at_mut(1);
            first[0].with_layout(plain(), |ui| {
            section(ui, "보유량");
            if view.holdings.is_empty() {
                note(ui, pal, "자산이 없는 모델입니다.");
            }
            ui.horizontal_wrapped(|ui| {
                for (asset, amount) in &view.holdings {
                    stat(ui, pal, &im.assets[*asset].name, amount.to_string());
                }
            });
            if let Some((menu, actions)) = &view.menu {
                section(ui, "지금 보이는 메뉴");
                let names: Vec<&str> = actions.iter().map(|x| im.actions[*x].as_str()).collect();
                ui.label(RichText::new(format!("{}: {}", im.menus[*menu], names.join(", "))).monospace());
            }
            section(ui, "받은 통지");
            if view.memory.is_empty() {
                note(ui, pal, "이 주체에게 선언된 슬롯이 없습니다.");
            }
            for m in &view.memory {
                ui.horizontal_wrapped(|ui| {
                    jump_link(ui, acts, RichText::new(&m.key).monospace(), line_of(a.map.slots.get(m.slot).copied().flatten()));
                    let text = RichText::new(&m.text).monospace();
                    ui.label(if m.content.is_some() { text.strong() } else { text.color(pal.muted) });
                });
            }
            section(ui, "지식");
            let worlds: Vec<String> = view.worlds.iter().map(|w| format!("w{w}")).collect();
            ui.horizontal_wrapped(|ui| {
                ui.label("가능하다고 보는 초기 세계:");
                ui.label(RichText::new(worlds.join(", ")).monospace().strong());
            });
            note(ui, pal, &format!("지금 보는 것만으로는 구별하지 못하는 지점이 {}개 있습니다. 그 모든 지점에서 같은 값이면 ‘안다’, 다르면 ‘모른다’입니다.", view.points));
            egui::Grid::new("knowledge_grid").striped(true).num_columns(4).spacing([14.0, 3.0]).show(ui, |ui| {
                for head in ["셀", "실제 값", "이 주체에게", "가능하다고 보는 값"] {
                    ui.label(RichText::new(head).small().color(pal.muted));
                }
                ui.end_row();
                for row in &view.cells {
                    let loc = if row.cell < im.cells.len() {
                        a.map.cells.get(row.cell).copied().flatten()
                    } else {
                        a.map.assets.get((row.cell - im.cells.len()) / im.actors.len().max(1)).copied().flatten()
                    };
                    jump_link(ui, acts, RichText::new(&row.name).monospace(), line_of(loc));
                    ui.label(RichText::new(&row.text).monospace());
                    if row.observed {
                        ui.label(RichText::new("직접 봄").color(pal.ok));
                    } else if row.known {
                        ui.label(RichText::new("추론으로 앎").color(pal.accent));
                    } else {
                        ui.label(RichText::new("모름").color(pal.warn).strong());
                    }
                    ui.label(RichText::new(row.possible.join(", ")).monospace().color(pal.muted));
                    ui.end_row();
                }
            });

            });
            second[0].with_layout(plain(), |ui| {
            section(ui, "거래 내역");
            let ledger = &app.cache.ledger;
            if ledger.is_empty() {
                note(ui, pal, "이 이력에서 이 주체가 낀 이전이 없습니다.");
            } else {
                note(ui, pal, "줄을 누르면 재생 위치가 그 걸음으로 갑니다. 흐린 줄은 아직 일어나지 않은 거래입니다.");
                let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // one line per row
                TableBuilder::new(ui)
                    .id_salt("ledger_table")
                    .striped(true)
                    .sense(Sense::click())
                    .vscroll(false)
                    .columns(Column::auto().at_least(34.0), 2)
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(60.0))
                    .columns(Column::auto().at_least(44.0), 4)
                    .header(height, |mut header| {
                        for head in ["걸음", "시각", "사건", "상대", "자산", "받음", "보냄", "잔액"] {
                            header.col(|ui| {
                                ui.label(RichText::new(head).small().color(pal.muted));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(height, ledger.len(), |mut row| {
                            let l = &ledger[row.index()];
                            let later = l.step > app.sel.step;
                            row.set_selected(l.step == app.sel.step);
                            let ink = |t: RichText| if later { t.color(pal.muted.gamma_multiply(0.6)) } else { t };
                            row.col(|ui| {
                                ui.label(ink(RichText::new((l.step + 1).to_string()).monospace()));
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(format!("@{}", l.time)).monospace()));
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(short(&l.title, 18)).monospace()));
                            });
                            row.col(|ui| {
                                let text = match l.counterparty {
                                    Some(other) if later => RichText::new(&im.actors[other]).color(pal.muted.gamma_multiply(0.6)),
                                    Some(other) => RichText::new(&im.actors[other]).color(actor_color(other)).strong(),
                                    None => ink(RichText::new(if l.inflow > 0 { "(발행)" } else { "(소각)" })),
                                };
                                ui.label(text);
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(&im.assets[l.asset].name)));
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(if l.inflow > 0 || l.outflow == 0 { l.inflow.to_string() } else { String::new() }).monospace()));
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(if l.outflow > 0 { l.outflow.to_string() } else { String::new() }).monospace()));
                            });
                            row.col(|ui| {
                                ui.label(ink(RichText::new(l.balance.to_string()).monospace().strong()));
                            });
                            if row.response().clicked() {
                                acts.push(Act::Play(false));
                                acts.push(Act::Step(l.step));
                            }
                        });
                    });
                ui.style_mut().wrap_mode = None;
                section(ui, "상대별 합계 (재생 위치까지)");
                let mut totals: Vec<(Option<usize>, usize, i64, i64)> = Vec::new();
                for l in ledger.iter().filter(|l| l.step <= app.sel.step) {
                    match totals.iter_mut().find(|t| t.0 == l.counterparty && t.1 == l.asset) {
                        Some(t) => {
                            t.2 += l.inflow;
                            t.3 += l.outflow;
                        }
                        None => totals.push((l.counterparty, l.asset, l.inflow, l.outflow)),
                    }
                }
                if totals.is_empty() {
                    note(ui, pal, "아직 거래가 없습니다.");
                }
                egui::Grid::new("total_grid").striped(true).num_columns(4).spacing([16.0, 3.0]).show(ui, |ui| {
                    for (other, asset, inflow, outflow) in &totals {
                        match other {
                            Some(o) => ui.label(RichText::new(&im.actors[*o]).color(actor_color(*o)).strong()),
                            None => ui.label(RichText::new("(발행·소각)").color(pal.muted)),
                        };
                        ui.label(&im.assets[*asset].name);
                        ui.label(RichText::new(format!("받음 {inflow}")).monospace());
                        ui.label(RichText::new(format!("보냄 {outflow}")).monospace());
                        ui.end_row();
                    }
                });
            }
            });
        });
    });
}

// ---- 6. the histories --------------------------------------------------------------------------

fn histories(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let runs = a.runs();
    let failed = runs.iter().filter(|r| r.outcome.is_some()).count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("완결 이력 {}개 (끝까지 {}개, 실패 {}개)", runs.len(), runs.len() - failed, failed));
        let mut only = app.failed_only;
        if ui.checkbox(&mut only, "실패한 이력만").changed() {
            acts.push(Act::FailedOnly(only));
        }
    });
    let shown: Vec<usize> = (0..runs.len()).filter(|&i| !app.failed_only || runs[i].outcome.is_some()).collect();
    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    ui.columns(2, |columns| {
        let (first, second) = columns.split_at_mut(1);
        first[0].with_layout(plain(), |ui| {
        section(ui, "이력");
        if shown.is_empty() {
            note(ui, pal, if app.failed_only { "실패한 이력이 없습니다." } else { "이력이 없습니다." });
        } else {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // one line per row
            TableBuilder::new(ui)
                .id_salt("run_table")
                .striped(true)
                .sense(Sense::click())
                .auto_shrink([false, false])
                .column(Column::auto().at_least(30.0))
                .column(Column::remainder().at_least(160.0).clip(true))
                .column(Column::auto().at_least(110.0))
                .column(Column::auto().at_least(34.0))
                .header(height, |mut header| {
                    for head in ["#", "세계와 선택", "끝", "걸음"] {
                        header.col(|ui| {
                            ui.label(RichText::new(head).small().color(pal.muted));
                        });
                    }
                })
                .body(|body| {
                    body.rows(height, shown.len(), |mut row| {
                        let i = shown[row.index()];
                        let run = &runs[i];
                        row.set_selected(i == app.sel.run);
                        row.col(|ui| {
                            ui.label(RichText::new((i + 1).to_string()).monospace());
                        });
                        row.col(|ui| {
                            ui.label(&run.label).on_hover_text(&run.label);
                        });
                        row.col(|ui| {
                            match run.outcome {
                                None => ui.label(RichText::new("끝까지").color(pal.ok)),
                                Some(status) => ui.label(RichText::new(status.name()).color(pal.error).strong()).on_hover_text(&run.reason),
                            };
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(run.steps.to_string()).monospace());
                        });
                        if row.response().clicked() {
                            acts.push(Act::SelectRun(i));
                        }
                    });
                });
        }

        });
        second[0].with_layout(plain(), |ui| {
        section(ui, "이력 나무");
        note(ui, pal, "이력이 갈리는 지점만 남긴 나무입니다. 줄을 누르면 그 아래 첫 이력을 고릅니다.");
        let tree = &app.cache.tree;
        let current = runs.get(app.sel.run).map(|r| r.leaf);
        egui::ScrollArea::vertical().id_salt("tree_scroll").auto_shrink([false, false]).show_rows(ui, height, tree.len(), |ui, range| {
            for k in range {
                let row = &tree[k];
                ui.horizontal(|ui| {
                    ui.add_space(row.depth as f32 * 18.0);
                    let color = match row.kind {
                        "fail" => pal.error,
                        "end" => pal.ok,
                        "init" => pal.accent,
                        _ => pal.text,
                    };
                    let mark = match row.kind {
                        "init" => "◆",
                        "pick" => "◇",
                        "end" => "●",
                        "fail" => "✖",
                        _ => "▸",
                    };
                    let mut text = RichText::new(format!("{mark} {}", row.label)).color(color);
                    if Some(row.node) == current {
                        text = text.strong().underline();
                    }
                    let target = tree[k..].iter().find_map(|t| t.run);
                    if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() {
                        if let Some(run) = target {
                            acts.push(Act::SelectRun(run));
                        }
                    }
                    if row.run.is_none() {
                        ui.label(RichText::new(format!("이력 {}개", row.leaves)).small().color(pal.muted));
                    }
                });
            }
        });
        });
    });
}

// ---- 7. what the compiler made -----------------------------------------------------------------

fn compiled(app: &IdeApp, a: &Analysis, ui: &mut egui::Ui, acts: &mut Vec<Act>, pal: &Palette) {
    let im = &a.image;
    ui.horizontal_wrapped(|ui| {
        stat(ui, pal, "이미지", format!("feanc {} · 언어 {}", fean_core::image::VERSION, fean_core::image::LANGUAGE));
        stat(ui, pal, "크기", format!("{} 바이트", a.image_text.len()));
        stat(ui, pal, "셀", format!("{} + 보유 {}", im.cells.len(), im.assets.len() * im.actors.len()));
        stat(ui, pal, "슬롯", im.slots.len().to_string());
        stat(ui, pal, "사건", im.events.len().to_string());
        stat(ui, pal, "커밋 단위", im.units.len().to_string());
        stat(ui, pal, "블록", im.blocks.len().to_string());
        stat(ui, pal, "코드 블록", im.codes.len().to_string());
        stat(ui, pal, "질의", im.queries.len().to_string());
        stat(ui, pal, "컴파일과 실행", format!("{:.0} ms", app.run_ms));
    });
    ui.horizontal(|ui| {
        if ui.button("이미지 저장…  (.feanc)").on_hover_text("이 이미지를 파일로 씁니다. fean run 으로 실행할 수 있습니다.").clicked() {
            acts.push(Act::SaveImage);
        }
        note(ui, pal, "아래는 이미지의 목록입니다 (fean dump 와 같은 글).");
    });
    ui.separator();
    egui::ScrollArea::both().id_salt("listing_scroll").auto_shrink([false, false]).show(ui, |ui| {
        let mut text = a.listing.as_str();
        ui.add(egui::TextEdit::multiline(&mut text).code_editor().frame(false).desired_width(f32::INFINITY));
    });
}
