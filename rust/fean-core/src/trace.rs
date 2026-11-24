//! History views: everything the IDE shows about a model, computed from the machine's history tree.
//!
//! Map unit I-10-a. What counts as a contradiction is DR-32: a compile error, a model error while
//! running, a mandatory step that fails, and a guarantee (`forall`, `can`) that turns out false.
//! Every query is answered with its grounds: a witness history, a counterexample, a strategy.
//!
//! Solves (Map unit I-17-a, DR-55): each is answered by the solver, and its representative
//! solution and its first non-solution become cases: the model run there, to be looked at with the
//! same views as an assignment. A model with unknowns has no other cases than these.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::check::{base, unit_text, Spec, Ty};
use crate::image::{self, Image, QueryI, NONE};
use crate::lower::SourceMap;
use crate::session::{self, Session, Verdict};
use crate::solve::{self, Solved};
use crate::syntax::{Expr, FeanError, Kind, Loc, R};
use crate::vm::{truthy, Game, Mint, Move, Ratio, Status, Step, Tell, Vm};
use crate::{compile, Compiled};

// ---- what the views are made of ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// A contradiction, or something that could not be decided.
#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    /// "compile" | "model" | "failure" | "guarantee" | "unknown" | "unsolved"
    pub kind: &'static str,
    /// The rule (`T-Eq`, `D-Move`), the status (`NO_SETTLEMENT`) or the kind of query (`forall`).
    pub code: String,
    pub title: String,
    pub detail: String,
    pub loc: Option<Loc>,
    pub assignment: Option<usize>,
    pub run: Option<usize>,
    pub step: Option<usize>,
    pub query: Option<usize>,
    /// The solve this finding is about.
    pub solve: Option<usize>,
    /// How many histories this finding stands for.
    pub count: usize,
}

/// One complete history.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub leaf: u32,
    pub world: usize,
    /// `None` when the history reached its end; otherwise how its last step failed.
    pub outcome: Option<Status>,
    pub reason: String,
    pub steps: usize,
    /// (actor, menu, action) of every choice on the way.
    pub picks: Vec<(usize, usize, usize)>,
    pub label: String,
    /// The world and every branch taken: finds the same history again after the model is rerun.
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub cell: usize,
    pub name: String,
    pub before: i64,
    pub after: i64,
    pub before_text: String,
    pub after_text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TellInfo {
    pub actor: usize,
    pub slot: usize,
    pub key: String,
    pub value: i64,
    pub text: String,
    pub asof: i64,
}

/// One step of a history, with what it did.
#[derive(Clone, Debug)]
pub struct StepInfo {
    pub node: u32,
    pub kind: &'static str,
    pub time: i64,
    pub title: String,
    /// The actors that take part in this step.
    pub actors: Vec<usize>,
    pub unit: Option<usize>,
    /// The events of a committed unit, written with their arguments.
    pub events: Vec<String>,
    pub moves: Vec<Move>,
    pub mints: Vec<Mint>,
    pub changes: Vec<Change>,
    pub tells: Vec<TellInfo>,
    /// (actor, slot key)
    pub forgets: Vec<(usize, String)>,
    pub reason: String,
    pub status: Option<Status>,
    pub loc: Option<Loc>,
}

/// Transfers between two parties in one asset, added up. A missing side is creation or destruction.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub asset: usize,
    pub amount: i64,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LedgerRow {
    pub step: usize,
    pub time: i64,
    pub title: String,
    /// The other party; `None` for creation or destruction.
    pub counterparty: Option<usize>,
    pub asset: usize,
    pub inflow: i64,
    pub outflow: i64,
    /// The actor's holding of the asset after this row.
    pub balance: i64,
}

#[derive(Clone, Debug)]
pub struct MemRow {
    pub slot: usize,
    pub key: String,
    pub content: Option<Tell>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct CellRow {
    pub cell: usize,
    pub name: String,
    pub value: i64,
    pub text: String,
    /// The actor sees this cell directly.
    pub observed: bool,
    /// The value is the same at every point the actor cannot tell apart from this one.
    pub known: bool,
    /// When not known: the values the actor considers possible.
    pub possible: Vec<String>,
}

/// What an actor has, sees, remembers and knows at one point of a history.
#[derive(Clone, Debug)]
pub struct ActorView {
    pub actor: usize,
    pub node: u32,
    pub time: i64,
    /// (asset, amount)
    pub holdings: Vec<(usize, i64)>,
    /// The menu shown to the actor at this point: (menu, actions).
    pub menu: Option<(usize, Vec<usize>)>,
    pub memory: Vec<MemRow>,
    /// The initial worlds the actor still considers possible.
    pub worlds: Vec<usize>,
    /// How many points the actor cannot tell apart from this one.
    pub points: usize,
    pub cells: Vec<CellRow>,
}

impl ActorView {
    pub fn known(&self) -> Vec<(usize, i64)> {
        self.cells.iter().filter(|c| c.known).map(|c| (c.cell, c.value)).collect()
    }
    pub fn unknown(&self) -> Vec<usize> {
        self.cells.iter().filter(|c| !c.known).map(|c| c.cell).collect()
    }
}

/// A row of the history tree: only points where histories part, and their ends.
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub node: u32,
    pub depth: usize,
    pub kind: &'static str,
    pub label: String,
    pub run: Option<usize>,
    /// How many complete histories pass through this point.
    pub leaves: usize,
}

#[derive(Clone, Debug)]
pub struct PolicyRow {
    pub actor: usize,
    pub menu: usize,
    /// The first point of the information set.
    pub node: u32,
    pub action: usize,
    pub label: String,
    /// A history that passes through that point, and the step at which it does.
    pub run: Option<usize>,
    pub step: usize,
}

/// The answer to a query for one assignment, with its grounds.
#[derive(Clone, Debug)]
pub struct AnswerRow {
    pub assignment: usize,
    pub verdict: Option<Verdict>,
    pub text: String,
    /// A history on which the expression holds (or that reaches the value).
    pub witness: Option<usize>,
    /// A history on which the expression fails.
    pub counter: Option<usize>,
    pub policy: Vec<PolicyRow>,
    pub error: Option<FeanError>,
}

#[derive(Clone, Debug)]
pub struct QueryAnswer {
    pub name: String,
    pub kind: &'static str,
    /// For a region: the kind of the query inside; otherwise the same as `kind`.
    pub inner_kind: &'static str,
    pub text: String,
    pub unit: String,
    pub loc: Option<Loc>,
    pub rows: Vec<AnswerRow>,
}

/// Where a case comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// An assignment of the public parameters (a model without unknowns).
    Assignment,
    /// The representative solution of a solve.
    Solution(usize),
    /// The first candidate of a solve that is not a solution.
    Counter(usize),
    /// A candidate of a solve picked to be looked at.
    Picked(usize),
    /// A model with unknowns and no solution to run it at.
    Open,
}

impl Origin {
    pub fn name(&self) -> &'static str {
        match self {
            Origin::Assignment => "assignment",
            Origin::Solution(_) => "solution",
            Origin::Counter(_) => "counter",
            Origin::Picked(_) => "picked",
            Origin::Open => "open",
        }
    }
    pub fn solve(&self) -> Option<usize> {
        match self {
            Origin::Solution(s) | Origin::Counter(s) | Origin::Picked(s) => Some(*s),
            _ => None,
        }
    }
}

/// One case: the model with every parameter and every unknown given a value (and, for a solve
/// at a point, begun in a given state), and its history tree.
pub struct Slot {
    pub pvals: Vec<i64>,
    pub label: String,
    pub vm: Option<Vm>,
    pub error: Option<FeanError>,
    pub runs: Vec<RunInfo>,
    pub origin: Origin,
    /// The number of the assignment of the parameters this case belongs to.
    pub assignment: usize,
    /// For a case of a solve: the candidate it is the model at.
    pub point: Option<Vec<i64>>,
}

/// The answer to a solve under one assignment, and its cases.
pub struct SolveRow {
    pub assignment: usize,
    /// The assignment as text (empty when the model has no parameters).
    pub label: String,
    pub solved: Arc<Solved>,
    /// The cases (numbers of slots) of the representative solution and of the first non-solution.
    pub solution: Option<usize>,
    pub counter: Option<usize>,
}

pub struct SolveAnswer {
    pub name: String,
    pub form: String,
    pub text: String,
    pub loc: Option<Loc>,
    pub rows: Vec<SolveRow>,
}

impl Slot {
    pub fn status(&self) -> &'static str {
        match &self.error {
            None => "ok",
            Some(e) if e.kind == Kind::Limit => "unknown",
            Some(_) => "error",
        }
    }
}

// ---- writing values the way the model writes them ----------------------------------------------

enum By {
    Actor(usize),
    Param(usize),
}

struct Names {
    /// The type of every cell, holdings included.
    cell_ty: Vec<Ty>,
    slot_ty: Vec<Ty>,
    event_params: Vec<Vec<Ty>>,
    event_by: Vec<Vec<By>>,
}

impl Names {
    fn new(spec: &Spec) -> Names {
        let mut cell_ty: Vec<Ty> = spec.cells.values().iter().map(|c| c.ty.clone()).collect();
        for asset in spec.assets.keys() {
            for _ in &spec.actors {
                cell_ty.push(Ty::Qty(vec![(asset.clone(), 1)]));
            }
        }
        let mut event_params = Vec::new();
        let mut event_by = Vec::new();
        for ev in spec.events.values() {
            event_params.push(ev.params.iter().map(|(_, t)| t.clone()).collect());
            let mut by = Vec::new();
            for b in &ev.by {
                if let Expr::Name(q) = b {
                    if let Some(i) = spec.actors.iter().position(|a| a == q) {
                        by.push(By::Actor(i));
                    } else if let Some(i) = ev.params.iter().position(|(p, _)| p == q) {
                        by.push(By::Param(i));
                    }
                }
            }
            event_by.push(by);
        }
        Names { cell_ty, slot_ty: spec.slots.iter().map(|(_, t)| t.clone()).collect(), event_params, event_by }
    }
}

/// A machine value written as in the model: `80[KRW]`, `true`, `boom`, `@3`, `D`, `empty`.
pub fn value_text(im: &Image, ty: &Ty, v: i64) -> String {
    if v == NONE {
        return "empty".to_string();
    }
    match base(ty) {
        Some(Ty::Qty(unit)) if unit.is_empty() => v.to_string(),
        Some(Ty::Qty(unit)) => format!("{v}[{}]", unit_text(unit)),
        Some(Ty::Bool) => (if v != 0 { "true" } else { "false" }).to_string(),
        Some(Ty::Time) => format!("@{v}"),
        Some(Ty::Actor) => im.actors.get(v as usize).cloned().unwrap_or_else(|| v.to_string()),
        Some(Ty::Enum(name)) => im
            .enums
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, values)| values.get(v as usize))
            .cloned()
            .unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}

fn kind_of(q: &QueryI) -> &'static str {
    match q {
        QueryI::Exists(_) => "exists",
        QueryI::Forall(_) => "forall",
        QueryI::Can(..) => "can",
        QueryI::Num { .. } => "num",
        QueryI::Region(..) => "region",
        QueryI::Prefer { .. } => "prefer",
    }
}

// ---- the analysis ------------------------------------------------------------------------------

/// A compiled model, run for every assignment, with everything the views need.
pub struct Analysis {
    pub image: Arc<Image>,
    pub map: SourceMap,
    /// The readable listing of the image (`fean dump`).
    pub listing: String,
    /// The image as file text.
    pub image_text: String,
    names: Names,
    pub slots: Vec<Slot>,
    pub selected: usize,
    answers: Vec<QueryAnswer>,
    findings: Vec<Finding>,
    solves: Vec<SolveAnswer>,
    session: Session,
    assignments: Vec<Vec<i64>>,
}

/// The outcome of compiling and analysing a text: the IDE's whole result state.
pub struct Report {
    pub error: Option<FeanError>,
    pub analysis: Option<Analysis>,
    /// Compile error included, so the list is never missing a reason.
    pub findings: Vec<Finding>,
}

/// Compile and analyse a model text. Nothing is thrown: what goes wrong becomes a finding.
pub fn analyze(src: &str, prelude: Option<&str>, budget: usize) -> Report {
    let failed = |e: FeanError, what: &str| {
        let finding = Finding {
            severity: Severity::Error,
            kind: "compile",
            code: e.code.clone(),
            title: format!("[{}] {what}", e.code),
            detail: e.message.clone(),
            loc: e.line.map(|line| Loc { origin: crate::syntax::Origin::Model, line }),
            assignment: None,
            run: None,
            step: None,
            query: None,
            solve: None,
            count: 1,
        };
        Report { error: Some(e), analysis: None, findings: vec![finding] }
    };
    let compiled = match compile(src, prelude) {
        Ok(c) => c,
        Err(e) => return failed(e, "컴파일 오류"),
    };
    match Analysis::new(&compiled, budget) {
        Ok(analysis) => Report { error: None, findings: analysis.findings.clone(), analysis: Some(analysis) },
        Err(e) => failed(e, "분석 오류"),
    }
}

fn run_infos(vm: &Vm, world_labels: &[String]) -> Vec<RunInfo> {
    let im = &vm.im;
    let mut runs = Vec::with_capacity(vm.leaves.len());
    for &leaf in &vm.leaves {
        let path = vm.path(leaf);
        let node = &vm.nodes[leaf as usize];
        let world = node.world as usize;
        let mut picks = Vec::new();
        let mut key = format!("w{world}");
        let mut parts: Vec<String> = Vec::new();
        for &n in &path {
            let at = &vm.nodes[n as usize];
            match &at.step {
                Step::Pick(who, menu, action) => {
                    picks.push((*who, *menu, *action));
                    key += &format!("/p:{}:{}:{}", im.actors[*who], im.menus[*menu], im.actions[*action]);
                    parts.push(im.actions[*action].clone());
                }
                Step::Branch(j) => {
                    key += &format!("/b:{j}");
                    parts.push(format!("alt {}", j + 1));
                }
                Step::Commit(_) => {
                    if let Some(p) = at.parent {
                        let siblings = &vm.nodes[p as usize].children;
                        if siblings.len() > 1 {
                            let k = siblings.iter().position(|c| *c == n).unwrap_or(0);
                            key += &format!("/c:{k}");
                            parts.push(format!("#{}", k + 1));
                        }
                    }
                }
                _ => {}
            }
        }
        let (outcome, reason) = match &node.step {
            Step::Fail(unit, status, why) => (Some(*status), format!("{}: {why}", im.units[*unit].name)),
            _ => (None, String::new()),
        };
        let mut label = world_labels.get(world).cloned().unwrap_or_else(|| format!("w{world}"));
        if !parts.is_empty() {
            label += &format!(" · {}", parts.join(" → "));
        }
        runs.push(RunInfo { leaf, world, outcome, reason, steps: path.len(), picks, label, key });
    }
    runs
}

impl Analysis {
    pub fn new(compiled: &Compiled, budget: usize) -> R<Analysis> {
        Analysis::with_limits(compiled, budget, solve::RUN_LIMIT)
    }

    /// The same with a limit on the runs one solve may take.
    pub fn with_limits(compiled: &Compiled, budget: usize, run_limit: usize) -> R<Analysis> {
        let image = Arc::new(Image::decode(&compiled.image));
        let names = Names::new(&compiled.spec);
        let mut session = Session::from_image(image.clone(), budget);
        session.run_limit = run_limit;
        let assignments = session.assignments();
        let every: Vec<usize> = (0..image.params.len()).collect();
        let mut analysis = Analysis {
            listing: image::listing(&compiled.image),
            image_text: image::dumps(&compiled.image),
            image: image.clone(),
            map: compiled.map.clone(),
            names,
            slots: Vec::new(),
            selected: 0,
            answers: Vec::new(),
            findings: Vec::new(),
            solves: Vec::new(),
            session,
            assignments: assignments.clone(),
        };
        if image.unknowns.is_empty() {
            for (a, pv) in assignments.iter().enumerate() {
                let label = session::head(&image, &every, pv);
                let made = Vm::new(image.clone(), pv.clone(), budget);
                analysis.push_slot(made, pv, label, Origin::Assignment, a, None);
            }
        }
        // the solves, and the cases they give: the solution, and the first candidate that is none
        for (s, sv) in image.solves.iter().enumerate() {
            let mut rows = Vec::new();
            for (a, pv) in assignments.iter().enumerate() {
                let solved = analysis.session.solved(s, pv);
                let label = session::head(&image, &every, pv);
                let solution = solved.best.clone().map(|p| analysis.case(s, a, &p, Origin::Solution(s)));
                let counter = solved.counter.clone().map(|p| analysis.case(s, a, &p, Origin::Counter(s)));
                rows.push(SolveRow { assignment: a, label, solved, solution, counter });
            }
            let text = analysis.session.solve_text(s);
            analysis.solves.push(SolveAnswer {
                name: sv.name.clone(),
                form: sv.form.clone(),
                text,
                loc: analysis.map.solves.get(s).copied().flatten(),
                rows,
            });
        }
        if analysis.slots.is_empty() {
            // unknowns, and no solution to give them values: nothing can be run
            let why = FeanError::new(Kind::Limit, "open", "미지수의 값을 정해 줄 해가 없어 실행할 사례가 없습니다", None);
            analysis.slots.push(Slot {
                pvals: Vec::new(),
                label: "값이 정해지지 않음".into(),
                vm: None,
                error: Some(why),
                runs: Vec::new(),
                origin: Origin::Open,
                assignment: 0,
                point: None,
            });
        }
        analysis.answers = analysis.answer_all();
        analysis.findings = analysis.find_all();
        Ok(analysis)
    }

    fn push_slot(&mut self, made: R<Vm>, pvals: &[i64], label: String, origin: Origin, assignment: usize, point: Option<Vec<i64>>) -> usize {
        let made = made.and_then(|mut vm| vm.explore().map(|()| vm));
        self.slots.push(match made {
            Ok(vm) => {
                let labels = world_labels(&self.image, &self.names, &vm);
                let runs = run_infos(&vm, &labels);
                Slot { pvals: pvals.to_vec(), label, vm: Some(vm), error: None, runs, origin, assignment, point }
            }
            Err(e) => Slot { pvals: pvals.to_vec(), label, vm: None, error: Some(e), runs: Vec::new(), origin, assignment, point },
        });
        self.slots.len() - 1
    }

    /// Run the model at a candidate of a solve and make it a case; its number.
    fn case(&mut self, s: usize, assignment: usize, point: &[i64], origin: Origin) -> usize {
        self.case_of(s, assignment, point, origin, None)
    }

    /// The same with the instance already made (or its failure).
    fn case_of(&mut self, s: usize, assignment: usize, point: &[i64], origin: Origin, made: Option<R<Vm>>) -> usize {
        let pv = self.assignments[assignment].clone();
        let solved = self.session.solved(s, &pv);
        let every: Vec<usize> = (0..self.image.params.len()).collect();
        let what = match origin {
            Origin::Solution(_) => "해",
            Origin::Counter(_) => "해가 아닌 값",
            _ => "고른 값",
        };
        let mut shown = solved.point_text(point);
        if shown.chars().count() > 72 {
            shown = shown.chars().take(70).collect::<String>() + " …";
        }
        let mut label = format!("{}의 {what} · {shown}", self.image.solves[s].name);
        if !every.is_empty() {
            label += &format!(" · {}", session::head(&self.image, &every, &pv));
        }
        let made = match made {
            Some(made) => made,
            None => self.session.instance(s, &pv, point),
        };
        self.push_slot(made, &pv, label, origin, assignment, Some(point.to_vec()))
    }

    /// Look at a candidate of a solve: the model is run there and becomes a case (the last one;
    /// it takes the place of a candidate picked before). Its number.
    pub fn open_case(&mut self, s: usize, row: usize, point: &[i64]) -> R<usize> {
        let Some(assignment) = self.solves.get(s).and_then(|sv| sv.rows.get(row)).map(|r| r.assignment) else {
            return Err(FeanError::image("I-Ref", format!("there is no solve {s} with a row {row}")));
        };
        let wanted = self.solves[s].rows[row].solved.vars.len();
        if point.len() != wanted {
            return Err(FeanError::image("I-Ref", format!("a candidate of this solve has {wanted} coordinates")));
        }
        // a state outside the declared ranges is no candidate: nothing is opened, and nothing changes
        let pv = self.assignments[assignment].clone();
        let made = self.session.instance(s, &pv, point);
        if let Err(e) = &made {
            if e.code == "I-Ref" {
                return Err(e.clone());
            }
        }
        while matches!(self.slots.last().map(|slot| &slot.origin), Some(Origin::Picked(_))) {
            self.slots.pop();
            for answer in &mut self.answers {
                answer.rows.pop();
            }
        }
        let at = self.case_of(s, assignment, point, Origin::Picked(s), Some(made));
        let image = self.image.clone();
        for (qi, entry) in image.queries.iter().enumerate() {
            let target = match &entry.q {
                QueryI::Region(_, inner) => inner.as_ref(),
                other => other,
            };
            let row = self.explain(at, target, &entry.unit);
            self.answers[qi].rows.push(row);
        }
        if self.selected >= self.slots.len() {
            self.selected = 0;
        }
        Ok(at)
    }

    pub fn solves(&self) -> &[SolveAnswer] {
        &self.solves
    }

    /// Choose the assignment the other views talk about. Fails with the assignment's own error
    /// when it could not be run (the selection still moves, so its findings can be shown).
    pub fn select(&mut self, assignment: usize) -> R<()> {
        if assignment >= self.slots.len() {
            return Err(FeanError::image("I-Ref", format!("there is no assignment {assignment}")));
        }
        self.selected = assignment;
        match &self.slots[assignment].error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    pub fn slot(&self) -> &Slot {
        &self.slots[self.selected]
    }

    pub fn vm(&self) -> Option<&Vm> {
        self.slot().vm.as_ref()
    }

    pub fn runs(&self) -> &[RunInfo] {
        &self.slot().runs
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    pub fn answers(&self) -> &[QueryAnswer] {
        &self.answers
    }

    /// The line of an element in the model text (none for what the standard contracts wrote).
    pub fn line(&self, loc: Option<Loc>) -> Option<u32> {
        SourceMap::line(loc)
    }

    pub fn cell_text(&self, cell: usize, v: i64) -> String {
        value_text(&self.image, &self.names.cell_ty[cell], v)
    }

    pub fn slot_text(&self, slot: usize, v: i64) -> String {
        value_text(&self.image, &self.names.slot_ty[slot], v)
    }

    pub fn amount_text(&self, asset: usize, amount: i64) -> String {
        format!("{amount}[{}]", self.image.assets[asset].name)
    }

    /// An event with its arguments as written: `elect(A)`, `collect`.
    pub fn event_text(&self, event: usize, args: &[i64]) -> String {
        let name = &self.image.events[event].name;
        if args.is_empty() {
            return name.clone();
        }
        let types = &self.names.event_params[event];
        let texts: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, v)| match types.get(i) {
                Some(ty) => value_text(&self.image, ty, *v),
                None => v.to_string(),
            })
            .collect();
        format!("{name}({})", texts.join(", "))
    }

    // ---- steps of a history ------------------------------------------------------------------
    pub fn steps(&self, run: usize) -> Vec<StepInfo> {
        let Some(vm) = self.vm() else { return Vec::new() };
        let Some(info) = self.runs().get(run) else { return Vec::new() };
        let im = &self.image;
        let mut out = Vec::new();
        for n in vm.path(info.leaf) {
            let node = &vm.nodes[n as usize];
            let mut step = StepInfo {
                node: n,
                kind: node.step.kind(),
                time: node.state.t,
                title: String::new(),
                actors: Vec::new(),
                unit: None,
                events: Vec::new(),
                moves: Vec::new(),
                mints: Vec::new(),
                changes: Vec::new(),
                tells: Vec::new(),
                forgets: Vec::new(),
                reason: String::new(),
                status: None,
                loc: node.src.and_then(|(b, i)| self.map.stmt(b as usize, i as usize)),
            };
            let join = |step: &mut StepInfo, who: usize| {
                if !step.actors.contains(&who) {
                    step.actors.push(who);
                }
            };
            match &node.step {
                Step::Init(w) => {
                    step.title = match vm.begun {
                        Some(mark) => format!("{} 에서 시작", im.marks[mark]),
                        None => format!("world {w}"),
                    }
                }
                Step::End => step.title = "end".to_string(),
                Step::Tick(t) => step.title = format!("@{t}"),
                Step::Branch(j) => step.title = format!("alt {}", j + 1),
                Step::Mark(m) => step.title = im.marks[*m].clone(),
                Step::Show(who, menu) => {
                    step.title = im.menus[*menu].clone();
                    join(&mut step, *who);
                }
                Step::Pick(who, _, action) => {
                    step.title = im.actions[*action].clone();
                    join(&mut step, *who);
                }
                Step::Reject(unit, why) => {
                    step.title = im.units[*unit].name.clone();
                    step.unit = Some(*unit);
                    step.reason = why.clone();
                    step.status = Some(Status::Blocked);
                }
                Step::Fail(unit, status, why) => {
                    step.title = im.units[*unit].name.clone();
                    step.unit = Some(*unit);
                    step.reason = why.clone();
                    step.status = Some(*status);
                }
                Step::Commit(unit) => {
                    step.title = im.units[*unit].name.clone();
                    step.unit = Some(*unit);
                    if let Some(record) = &node.record {
                        for (event, args) in &record.events {
                            step.events.push(self.event_text(*event, args));
                            for by in &self.names.event_by[*event] {
                                match by {
                                    By::Actor(a) => join(&mut step, *a),
                                    By::Param(p) => {
                                        if let Some(&v) = args.get(*p) {
                                            if v >= 0 && (v as usize) < im.actors.len() {
                                                join(&mut step, v as usize);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        for m in &record.moves {
                            join(&mut step, m.from);
                            join(&mut step, m.to);
                        }
                        for m in &record.mints {
                            join(&mut step, m.actor);
                        }
                        for &(_, slot, value, asof) in &record.tells {
                            let who = im.slots[slot].actor;
                            join(&mut step, who);
                            step.tells.push(TellInfo {
                                actor: who,
                                slot,
                                key: im.slots[slot].key.clone(),
                                value,
                                text: self.slot_text(slot, value),
                                asof,
                            });
                        }
                        for &(_, slot) in &record.forgets {
                            let who = im.slots[slot].actor;
                            join(&mut step, who);
                            step.forgets.push((who, im.slots[slot].key.clone()));
                        }
                        step.moves = record.moves.clone();
                        step.mints = record.mints.clone();
                    }
                    if let Some(p) = node.parent {
                        let before = &vm.nodes[p as usize].state.cells;
                        if !Arc::ptr_eq(before, &node.state.cells) {
                            for (c, (&old, &new)) in before.iter().zip(node.state.cells.iter()).enumerate() {
                                if old != new {
                                    step.changes.push(Change {
                                        cell: c,
                                        name: im.cell_name(c),
                                        before: old,
                                        after: new,
                                        before_text: self.cell_text(c, old),
                                        after_text: self.cell_text(c, new),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            out.push(step);
        }
        out
    }

    // ---- who paid whom -----------------------------------------------------------------------
    /// The transfers of a history up to and including step `upto`, added up per (from, to, asset).
    pub fn graph(&self, run: usize, upto: usize) -> Vec<Edge> {
        let Some(vm) = self.vm() else { return Vec::new() };
        let Some(info) = self.runs().get(run) else { return Vec::new() };
        let mut edges: Vec<Edge> = Vec::new();
        let mut add = |from: Option<usize>, to: Option<usize>, asset: usize, amount: i64| match edges
            .iter_mut()
            .find(|e| e.from == from && e.to == to && e.asset == asset)
        {
            Some(e) => {
                e.amount += amount;
                e.count += 1;
            }
            None => edges.push(Edge { from, to, asset, amount, count: 1 }),
        };
        for (i, n) in vm.path(info.leaf).into_iter().enumerate() {
            if i > upto {
                break;
            }
            if let Some(record) = &vm.nodes[n as usize].record {
                for m in &record.moves {
                    add(Some(m.from), Some(m.to), m.asset, m.amount);
                }
                for m in &record.mints {
                    if m.amount > 0 {
                        add(None, Some(m.actor), m.asset, m.amount);
                    } else {
                        add(Some(m.actor), None, m.asset, -m.amount);
                    }
                }
            }
        }
        edges
    }

    /// Every transfer an actor took part in, in order, with the running balance per asset.
    pub fn ledger(&self, run: usize, actor: usize) -> Vec<LedgerRow> {
        let Some(vm) = self.vm() else { return Vec::new() };
        let Some(info) = self.runs().get(run) else { return Vec::new() };
        let im = &self.image;
        let path = vm.path(info.leaf);
        let start = &vm.nodes[path[0] as usize].state;
        let mut balance: Vec<i64> = (0..im.assets.len()).map(|x| start.cells[im.holding(x, actor)]).collect();
        let mut rows = Vec::new();
        for (i, &n) in path.iter().enumerate() {
            let node = &vm.nodes[n as usize];
            let (Some(record), Step::Commit(unit)) = (&node.record, &node.step) else { continue };
            let title = im.units[*unit].name.clone();
            for m in &record.moves {
                if m.from != actor && m.to != actor {
                    continue;
                }
                let inflow = if m.to == actor { m.amount } else { 0 };
                let outflow = if m.from == actor { m.amount } else { 0 };
                balance[m.asset] += inflow - outflow;
                let other = if m.from == actor { m.to } else { m.from };
                rows.push(LedgerRow {
                    step: i,
                    time: node.state.t,
                    title: title.clone(),
                    counterparty: Some(other),
                    asset: m.asset,
                    inflow,
                    outflow,
                    balance: balance[m.asset],
                });
            }
            for m in &record.mints {
                if m.actor != actor {
                    continue;
                }
                balance[m.asset] += m.amount;
                rows.push(LedgerRow {
                    step: i,
                    time: node.state.t,
                    title: title.clone(),
                    counterparty: None,
                    asset: m.asset,
                    inflow: m.amount.max(0),
                    outflow: (-m.amount).max(0),
                    balance: balance[m.asset],
                });
            }
        }
        rows
    }

    // ---- what an actor knows -----------------------------------------------------------------
    pub fn actor_view(&self, run: usize, step: usize, actor: usize) -> Option<ActorView> {
        let vm = self.vm()?;
        let info = self.runs().get(run)?;
        let path = vm.path(info.leaf);
        let n = *path.get(step)?;
        Some(self.view_at(vm, n, actor))
    }

    fn view_at(&self, vm: &Vm, n: u32, actor: usize) -> ActorView {
        let im = &self.image;
        let node = &vm.nodes[n as usize];
        let classes = vm.classes(actor);
        let members = &classes.members[classes.of[n as usize] as usize];
        let mut worlds: Vec<usize> = members.iter().map(|&m| vm.nodes[m as usize].world as usize).collect();
        worlds.sort_unstable();
        worlds.dedup();
        let mut cells = Vec::with_capacity(im.cell_count());
        for c in 0..im.cell_count() {
            let value = node.state.cells[c];
            let mut seen: Vec<i64> = Vec::new();
            for &m in members {
                let v = vm.nodes[m as usize].state.cells[c];
                if !seen.contains(&v) {
                    seen.push(v);
                    if seen.len() > 16 {
                        break;
                    }
                }
            }
            seen.sort_unstable();
            let known = seen.len() == 1;
            cells.push(CellRow {
                cell: c,
                name: im.cell_name(c),
                value,
                text: self.cell_text(c, value),
                observed: im.obs[actor].contains(&c),
                known,
                possible: if known { Vec::new() } else { seen.iter().map(|&v| self.cell_text(c, v)).collect() },
            });
        }
        let memory = im.mems[actor]
            .iter()
            .map(|&k| {
                let content = node.state.mem[k];
                let text = match content {
                    Some((v, asof, recv)) => format!("{} (asof @{asof}, recv @{recv})", self.slot_text(k, v)),
                    None => "empty".to_string(),
                };
                MemRow { slot: k, key: im.slots[k].key.clone(), content, text }
            })
            .collect();
        let menu = node.display.and_then(|d| {
            let (who, menu, actions) = &vm.displays[d as usize];
            (*who == actor).then(|| (*menu, actions.clone()))
        });
        ActorView {
            actor,
            node: n,
            time: node.state.t,
            holdings: (0..im.assets.len()).map(|x| (x, node.state.cells[im.holding(x, actor)])).collect(),
            menu,
            memory,
            worlds,
            points: members.len(),
            cells,
        }
    }

    // ---- the history tree --------------------------------------------------------------------
    /// The points where histories part (worlds, alternatives, choices, several settlements) and
    /// the ends, in history order; `depth` counts the kept points above.
    pub fn tree(&self) -> Vec<TreeRow> {
        let Some(vm) = self.vm() else { return Vec::new() };
        let im = &self.image;
        let labels = world_labels(im, &self.names, vm);
        let n = vm.nodes.len();
        let mut under = vec![0usize; n];
        for i in (0..n).rev() {
            let node = &vm.nodes[i];
            under[i] = node.is_leaf() as usize + node.children.iter().map(|&c| under[c as usize]).sum::<usize>();
        }
        let mut depth = vec![0usize; n];
        let mut kept = vec![false; n];
        let mut rows = Vec::new();
        for i in 0..n {
            let node = &vm.nodes[i];
            let above = match node.parent {
                Some(p) => depth[p as usize] + kept[p as usize] as usize,
                None => 0,
            };
            depth[i] = above;
            let label = match &node.step {
                Step::Init(w) => Some(labels.get(*w).cloned().unwrap_or_else(|| format!("world {w}"))),
                Step::Branch(j) => Some(format!("alt {}", j + 1)),
                Step::Pick(who, menu, action) => Some(format!("{}: {} = {}", im.actors[*who], im.menus[*menu], im.actions[*action])),
                Step::Fail(unit, status, why) => Some(format!("{} {} — {why}", status.name(), im.units[*unit].name)),
                Step::End => Some("end".to_string()),
                Step::Commit(unit) => match node.parent {
                    Some(p) if vm.nodes[p as usize].children.len() > 1 => {
                        let k = vm.nodes[p as usize].children.iter().position(|c| *c as usize == i).unwrap_or(0);
                        Some(format!("{} #{}", im.units[*unit].name, k + 1))
                    }
                    _ => None,
                },
                _ => None,
            };
            if let Some(label) = label {
                kept[i] = true;
                let run = if node.is_leaf() { vm.leaves.binary_search(&(i as u32)).ok() } else { None };
                rows.push(TreeRow { node: i as u32, depth: above, kind: node.step.kind(), label, run, leaves: under[i] });
            }
        }
        rows
    }

    // ---- queries and their grounds -----------------------------------------------------------
    fn answer_all(&mut self) -> Vec<QueryAnswer> {
        let image = self.image.clone();
        let assignments = self.assignments.clone();
        let assignments = &assignments;
        let mut answers = Vec::new();
        for (qi, entry) in image.queries.iter().enumerate() {
            let target = match &entry.q {
                QueryI::Region(_, inner) => inner.as_ref(),
                other => other,
            };
            let mut rows = Vec::new();
            for a in 0..self.slots.len() {
                rows.push(self.explain(a, target, &entry.unit));
            }
            let mut eval = |i: usize, _: &QueryI| -> R<Verdict> {
                match (&rows[i].error, &rows[i].verdict) {
                    (Some(e), _) => Err(e.clone()),
                    (None, Some(v)) => Ok(v.clone()),
                    (None, None) => Ok(Verdict::Unknown),
                }
            };
            let text = if image.unknowns.is_empty() {
                match session::tabulate(&image, assignments, &entry.q, &mut eval) {
                    Ok(res) => session::show(&image, &res, &entry.unit),
                    Err(e) => format!("[{}] {}", e.code, e.message),
                }
            } else {
                // unknowns: the answer in the case of each solve's solution
                let mut cases = Vec::new();
                let mut failed = None;
                for sv in &self.solves {
                    let found: Vec<(Vec<i64>, usize)> =
                        sv.rows.iter().filter_map(|r| r.solution.map(|slot| (assignments[r.assignment].clone(), slot))).collect();
                    if image.params.is_empty() && found.is_empty() {
                        cases.push((sv.name.clone(), None));
                        continue;
                    }
                    let pvals: Vec<Vec<i64>> = found.iter().map(|f| f.0.clone()).collect();
                    match session::tabulate(&image, &pvals, &entry.q, &mut |i, q| eval(found[i].1, q)) {
                        Ok(res) => cases.push((sv.name.clone(), Some(res))),
                        Err(e) => failed = Some(format!("[{}] {}", e.code, e.message)),
                    }
                }
                failed.unwrap_or_else(|| session::show(&image, &session::Res::Cases(cases), &entry.unit))
            };
            answers.push(QueryAnswer {
                name: entry.name.clone(),
                kind: kind_of(&entry.q),
                inner_kind: kind_of(target),
                text,
                unit: entry.unit.clone(),
                loc: self.map.queries.get(qi).copied().flatten(),
                rows,
            });
        }
        answers
    }

    fn explain(&mut self, assignment: usize, q: &QueryI, unit: &str) -> AnswerRow {
        let mut row =
            AnswerRow { assignment, verdict: None, text: String::new(), witness: None, counter: None, policy: Vec::new(), error: None };
        let slot = &mut self.slots[assignment];
        let done = match (&mut slot.vm, &slot.error) {
            (Some(vm), _) => grounds(vm, q, &self.image, &mut row),
            (None, Some(e)) => Err(e.clone()),
            (None, None) => Err(FeanError::image("I-Mode", "the assignment was not run")),
        };
        match done {
            Ok(verdict) => row.verdict = Some(verdict),
            Err(e) if e.kind == Kind::Limit => row.verdict = Some(Verdict::Unknown),
            Err(e) => row.error = Some(e),
        }
        row.text = match (&row.error, &row.verdict) {
            (Some(e), _) => format!("[{}]", e.code),
            (None, Some(v)) => session::verdict_text(v, unit),
            (None, None) => session::UNKNOWN.to_string(),
        };
        row
    }

    // ---- contradictions ----------------------------------------------------------------------
    fn find_all(&self) -> Vec<Finding> {
        let im = &self.image;
        let mut out = Vec::new();
        let named = |label: &str| if label.is_empty() { String::new() } else { format!(" ({label})") };
        for (a, slot) in self.slots.iter().enumerate() {
            if slot.origin != Origin::Assignment {
                continue; // a case of a solve is a candidate, not a claim of the model
            }
            match &slot.error {
                Some(e) if e.kind == Kind::Limit => out.push(Finding {
                    severity: Severity::Info,
                    kind: "unknown",
                    code: "limit".to_string(),
                    title: format!("탐색 한도를 넘어 판정하지 못함{}", named(&slot.label)),
                    detail: format!("{}. 한도를 올리거나 모델의 가지 수를 줄이면 판정할 수 있습니다.", e.message),
                    loc: None,
                    assignment: Some(a),
                    run: None,
                    step: None,
                    query: None,
                    solve: None,
                    count: 1,
                }),
                Some(e) => out.push(Finding {
                    severity: Severity::Error,
                    kind: "model",
                    code: e.code.clone(),
                    title: format!("[{}] 실행 오류{}", e.code, named(&slot.label)),
                    detail: e.message.clone(),
                    loc: None,
                    assignment: Some(a),
                    run: None,
                    step: None,
                    query: None,
                    solve: None,
                    count: 1,
                }),
                None => {}
            }
            // failed mandatory steps: one finding per (unit, status, reason)
            let Some(vm) = &slot.vm else { continue };
            let mut groups: Vec<(usize, Status, String, usize, usize)> = Vec::new();
            for (r, run) in slot.runs.iter().enumerate() {
                if let Step::Fail(unit, status, why) = &vm.nodes[run.leaf as usize].step {
                    match groups.iter_mut().find(|g| g.0 == *unit && g.1 == *status && g.2 == *why) {
                        Some(g) => g.4 += 1,
                        None => groups.push((*unit, *status, why.clone(), r, 1)),
                    }
                }
            }
            for (unit, status, why, r, count) in groups {
                let run = &slot.runs[r];
                let node = &vm.nodes[run.leaf as usize];
                let what = match status {
                    Status::Blocked => "필수 사건이 막힘",
                    Status::NoSettlement => "결제 불능 (가능한 사후 상태가 없음)",
                };
                out.push(Finding {
                    severity: Severity::Error,
                    kind: "failure",
                    code: status.name().to_string(),
                    title: format!("{what}: {}{}", im.units[unit].name, named(&slot.label)),
                    detail: format!("{why}. 이 실패로 끝나는 이력 {count}개 중 첫째: {}", run.label),
                    loc: node.src.and_then(|(b, i)| self.map.stmt(b as usize, i as usize)),
                    assignment: Some(a),
                    run: Some(r),
                    step: Some(run.steps - 1),
                    query: None,
                    solve: None,
                    count,
                });
            }
        }
        for (qi, answer) in self.answers.iter().enumerate() {
            for row in &answer.rows {
                if self.slots[row.assignment].origin != Origin::Assignment {
                    continue;
                }
                let label = &self.slots[row.assignment].label;
                if let Some(e) = &row.error {
                    if self.slots[row.assignment].error.is_none() {
                        out.push(Finding {
                            severity: Severity::Error,
                            kind: "model",
                            code: e.code.clone(),
                            title: format!("[{}] 질의 {} 의 실행 오류{}", e.code, answer.name, named(label)),
                            detail: e.message.clone(),
                            loc: answer.loc,
                            assignment: Some(row.assignment),
                            run: None,
                            step: None,
                            query: Some(qi),
                            solve: None,
                            count: 1,
                        });
                    }
                    continue;
                }
                let guarantee = answer.kind == "forall" || answer.kind == "can";
                match &row.verdict {
                    Some(Verdict::Bool(false)) if guarantee => {
                        let detail = match (answer.kind, row.counter) {
                            ("forall", Some(r)) => format!(
                                "모든 이력에서 성립해야 하지만 반례가 있습니다. 반례 이력: {}",
                                self.slots[row.assignment].runs[r].label
                            ),
                            (_, Some(r)) => format!(
                                "어떤 전략을 써도 보장할 수 없습니다. 식이 깨지는 이력의 예: {}",
                                self.slots[row.assignment].runs[r].label
                            ),
                            _ => "성립하지 않습니다.".to_string(),
                        };
                        out.push(Finding {
                            severity: Severity::Warning,
                            kind: "guarantee",
                            code: answer.kind.to_string(),
                            title: format!("보장이 깨짐: {}{}", answer.name, named(label)),
                            detail,
                            loc: answer.loc,
                            assignment: Some(row.assignment),
                            run: row.counter,
                            step: None,
                            query: Some(qi),
                            solve: None,
                            count: 1,
                        });
                    }
                    Some(Verdict::Unknown) if self.slots[row.assignment].error.is_none() => out.push(Finding {
                        severity: Severity::Info,
                        kind: "unknown",
                        code: "limit".to_string(),
                        title: format!("질의 {} 를 판정하지 못함{}", answer.name, named(label)),
                        detail: "전략을 찾는 걸음이 한도를 넘었습니다.".to_string(),
                        loc: answer.loc,
                        assignment: Some(row.assignment),
                        run: None,
                        step: None,
                        query: Some(qi),
                        solve: None,
                        count: 1,
                    }),
                    _ => {}
                }
            }
        }
        for (s, sv) in self.solves.iter().enumerate() {
            for row in &sv.rows {
                let solved = &row.solved;
                let solution = solved.leaves.iter().any(|l| l.verdict == solve::Verdict::Yes);
                let base = Finding {
                    severity: Severity::Info,
                    kind: "unknown",
                    code: sv.form.clone(),
                    title: String::new(),
                    detail: String::new(),
                    loc: sv.loc,
                    assignment: row.counter,
                    run: None,
                    step: None,
                    query: None,
                    solve: Some(s),
                    count: 1,
                };
                if let Some(why) = &solved.error {
                    out.push(Finding {
                        severity: Severity::Error,
                        kind: "model",
                        code: "solver".into(),
                        title: format!("역산 질의 {} 를 풀지 못함{}", sv.name, named(&row.label)),
                        detail: why.clone(),
                        ..base
                    });
                } else if !solved.complete || solved.unknown {
                    out.push(Finding {
                        title: format!("역산 질의 {} 를 끝까지 판정하지 못함{}", sv.name, named(&row.label)),
                        detail: format!(
                            "모델을 {}번 실행했지만 판정하지 못한 영역이 남았습니다. 탐색 한도를 올리거나 범위를 줄이면 판정할 수 있습니다.",
                            solved.runs
                        ),
                        ..base
                    });
                } else if !solution {
                    let example = match &solved.counter {
                        Some(p) => format!(" 예를 들어 {} 에서는 목표가 성립하지 않습니다.", solved.point_text(p)),
                        None => " 제약을 통과하는 후보가 하나도 없습니다.".to_string(),
                    };
                    out.push(Finding {
                        severity: Severity::Warning,
                        kind: "unsolved",
                        title: format!("해가 없음: {}{}", sv.name, named(&row.label)),
                        detail: format!("제약을 지키면서 목표가 성립하는 값이 없습니다.{example}"),
                        ..base
                    });
                }
            }
        }
        out
    }

    // ---- everything as JSON (for `fean inspect` and the tests) --------------------------------
    pub fn to_json(&mut self, views: bool) -> Value {
        let im = self.image.clone();
        let line = |loc: Option<Loc>| SourceMap::line(loc);
        let lines = |locs: &[Option<Loc>]| -> Vec<Option<u32>> { locs.iter().map(|l| line(*l)).collect() };
        let keep = self.selected;
        let mut assignments = Vec::new();
        for a in 0..self.slots.len() {
            self.selected = a;
            let slot = &self.slots[a];
            let mut runs = Vec::new();
            for (r, info) in slot.runs.iter().enumerate() {
                let vm = slot.vm.as_ref().unwrap();
                let steps = self.steps(r);
                let mut step_rows = Vec::new();
                for (at, step) in steps.iter().enumerate() {
                    let node = &vm.nodes[step.node as usize];
                    let cells: Vec<Value> = node.state.cells.iter().map(|&v| plain(v)).collect();
                    let mem: Vec<Value> = node
                        .state
                        .mem
                        .iter()
                        .map(|slot| match slot {
                            Some((v, asof, recv)) => json!([plain(*v), plain(*asof), recv]),
                            None => Value::Null,
                        })
                        .collect();
                    let mut row = json!({
                        "node": step.node, "kind": step.kind, "time": step.time, "title": step.title,
                        "actors": step.actors, "unit": step.unit, "events": step.events,
                        "moves": step.moves.iter().map(|m| json!([m.event, m.from, m.to, m.asset, m.amount])).collect::<Vec<_>>(),
                        "mints": step.mints.iter().map(|m| json!([m.event, m.actor, m.asset, m.amount])).collect::<Vec<_>>(),
                        "changes": step.changes.iter().map(|c| json!([c.cell, plain(c.before), plain(c.after), c.before_text, c.after_text])).collect::<Vec<_>>(),
                        "tells": step.tells.iter().map(|t| json!([t.actor, t.slot, plain(t.value), plain(t.asof), t.text])).collect::<Vec<_>>(),
                        "forgets": step.forgets, "reason": step.reason,
                        "status": step.status.map(|s| s.name()), "line": line(step.loc),
                        "cells": cells, "mem": mem,
                    });
                    if views {
                        let rows: Vec<Value> = (0..im.actors.len())
                            .map(|actor| {
                                let v = self.view_at(vm, step.node, actor);
                                json!({"actor": actor, "worlds": v.worlds, "points": v.points,
                                       "known": v.known().iter().map(|(c, _)| *c).collect::<Vec<_>>(),
                                       "unknown": v.unknown(),
                                       "possible": v.cells.iter().filter(|c| !c.known).map(|c| json!([c.cell, c.possible])).collect::<Vec<_>>(),
                                       "holdings": v.holdings.iter().map(|(x, n)| json!([x, n])).collect::<Vec<_>>(),
                                       "memory": v.memory.iter().map(|m| json!([m.key, m.text])).collect::<Vec<_>>(),
                                       "menu": v.menu.as_ref().map(|(m, acts)| json!([m, acts]))})
                            })
                            .collect();
                        row["views"] = json!(rows);
                        // the transfers so far, as the graph view shows them at this replay position
                        row["graph"] = json!(self.graph(r, at).iter().map(|e| json!([e.from, e.to, e.asset, e.amount, e.count])).collect::<Vec<_>>());
                    }
                    step_rows.push(row);
                }
                let graph: Vec<Value> =
                    self.graph(r, steps.len()).iter().map(|e| json!([e.from, e.to, e.asset, e.amount, e.count])).collect();
                let ledgers: Vec<Value> = (0..im.actors.len())
                    .map(|actor| {
                        self.ledger(r, actor)
                            .iter()
                            .map(|l| json!([l.step, l.time, l.title, l.counterparty, l.asset, l.inflow, l.outflow, l.balance]))
                            .collect()
                    })
                    .collect();
                runs.push(json!({
                    "leaf": info.leaf, "world": info.world, "outcome": info.outcome.map(|s| s.name()).unwrap_or("end"),
                    "reason": info.reason, "key": info.key, "label": info.label, "picks": info.picks,
                    "steps": step_rows, "graph": graph, "ledgers": ledgers,
                }));
            }
            let tree: Vec<Value> = self.tree().iter().map(|t| json!([t.node, t.depth, t.kind, t.label, t.run, t.leaves])).collect();
            let slot = &self.slots[a];
            assignments.push(json!({
                "label": slot.label, "pvals": slot.pvals.iter().map(|&v| plain(v)).collect::<Vec<_>>(),
                "origin": slot.origin.name(), "solve": slot.origin.solve(), "point": slot.point,
                "begun": slot.vm.as_ref().and_then(|vm| vm.begun).map(|m| im.marks[m].clone()),
                "unknowns": slot.vm.as_ref().map(|vm| vm.uv.iter().map(|x| x.v).collect::<Vec<_>>()),
                "status": slot.status(), "error": slot.error.as_ref().map(error_json),
                "worlds": slot.vm.as_ref().map(|vm| vm.worlds.len()), "points": slot.vm.as_ref().map(|vm| vm.nodes.len()),
                "runs": runs, "tree": tree,
            }));
        }
        self.selected = keep;
        let answers: Vec<Value> = self
            .answers
            .iter()
            .map(|a| {
                let rows: Vec<Value> = a
                    .rows
                    .iter()
                    .map(|r| {
                        let verdict = match &r.verdict {
                            Some(Verdict::Bool(b)) => json!(b),
                            Some(Verdict::Num(n)) => json!(n.text()),
                            Some(Verdict::Unknown) => json!(session::UNKNOWN),
                            None => Value::Null,
                        };
                        json!({"assignment": r.assignment, "verdict": verdict, "text": r.text, "witness": r.witness,
                               "counter": r.counter, "error": r.error.as_ref().map(error_json),
                               "policy": r.policy.iter().map(|p| json!({"actor": p.actor, "menu": p.menu, "node": p.node,
                                                                        "action": p.action, "label": p.label})).collect::<Vec<_>>()})
                    })
                    .collect();
                json!({"name": a.name, "kind": a.kind, "inner_kind": a.inner_kind, "text": a.text, "unit": a.unit,
                       "line": line(a.loc), "rows": rows})
            })
            .collect();
        let solves: Vec<Value> = self
            .solves
            .iter()
            .map(|sv| {
                let rows: Vec<Value> = sv
                    .rows
                    .iter()
                    .map(|row| {
                        let solved = &row.solved;
                        let regions: Vec<Value> = solved
                            .leaves
                            .iter()
                            .map(|l| {
                                let sample = &l.sample[..solved.vars.len().min(l.sample.len())];
                                let reason = match &l.verdict {
                                    solve::Verdict::Invalid(code) => json!(code),
                                    solve::Verdict::Undecided(why) => json!(why),
                                    _ => Value::Null,
                                };
                                json!({"verdict": l.verdict.word(), "reason": reason, "text": solved.region_text(&l.region),
                                       "sample": sample, "sample_text": solved.point_text(sample)})
                            })
                            .collect();
                        json!({
                            "assignment": row.assignment, "label": row.label, "text": solved.text, "runs": solved.runs,
                            "complete": solved.complete, "unknown": solved.unknown, "error": solved.error,
                            "vars": solved.vars.iter().map(|v| v.name.clone()).collect::<Vec<_>>(),
                            "best": solved.best, "best_text": solved.best.as_ref().map(|p| solved.point_text(p)),
                            "value": solved.value.map(|v| v.to_string()),
                            "counter": solved.counter, "counter_text": solved.counter.as_ref().map(|p| solved.point_text(p)),
                            "solution_case": row.solution, "counter_case": row.counter, "regions": regions,
                        })
                    })
                    .collect();
                json!({"name": sv.name, "form": sv.form, "text": sv.text, "line": line(sv.loc), "rows": rows})
            })
            .collect();
        json!({
            "ok": true, "model": im.model, "actors": im.actors,
            "unknowns": im.unknowns.iter().map(|u| u.name.clone()).collect::<Vec<_>>(), "solves": solves,
            "assets": im.assets.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
            "cells": (0..im.cell_count()).map(|c| im.cell_name(c)).collect::<Vec<_>>(),
            "slots": im.slots.iter().map(|s| format!("{}.{}", im.actors[s.actor], s.key)).collect::<Vec<_>>(),
            "events": im.events.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            "units": im.units.iter().map(|u| u.name.clone()).collect::<Vec<_>>(),
            "menus": im.menus, "marks": im.marks, "actions": im.actions,
            "lines": {
                "actors": lines(&self.map.actors), "assets": lines(&self.map.assets), "params": lines(&self.map.params),
                "cells": lines(&self.map.cells), "slots": lines(&self.map.slots), "events": lines(&self.map.events),
                "queries": lines(&self.map.queries), "menus": lines(&self.map.menus), "marks": lines(&self.map.marks),
                "units": lines(&self.map.units), "unknowns": lines(&self.map.unknowns), "solves": lines(&self.map.solves),
            },
            "findings": self.findings.iter().map(finding_json).collect::<Vec<_>>(),
            "answers": answers, "assignments": assignments,
        })
    }
}

/// Decide a query on a machine and note the grounds in `row`.
fn grounds(vm: &mut Vm, q: &QueryI, im: &Image, row: &mut AnswerRow) -> R<Verdict> {
    vm.explore()?;
    let vm = &*vm;
    let follows = |game: &Game, chosen: &[Option<usize>], leaf: usize| {
        game.picks[leaf].iter().all(|&(set, action)| chosen[set as usize].map_or(true, |c| c == action))
    };
    let policy_rows = |game: &Game, chosen: &[Option<usize>]| -> Vec<PolicyRow> {
        let mut rows = Vec::new();
        for (s, set) in game.sets.iter().enumerate() {
            if let Some(action) = chosen[s] {
                let nth = game.sets[..s].iter().filter(|o| o.actor == set.actor && o.menu == set.menu).count();
                let total = game.sets.iter().filter(|o| o.actor == set.actor && o.menu == set.menu).count();
                let which = if total > 1 { format!(" #{}", nth + 1) } else { String::new() };
                let run = vm.under(set.node).first().and_then(|leaf| vm.leaves.binary_search(leaf).ok());
                let mut step = 0;
                let mut at = vm.nodes[set.node as usize].parent;
                while let Some(p) = at {
                    step += 1;
                    at = vm.nodes[p as usize].parent;
                }
                rows.push(PolicyRow {
                    actor: set.actor,
                    menu: set.menu,
                    node: set.node,
                    action,
                    label: format!("{} · {}{which} → {}", im.actors[set.actor], im.menus[set.menu], im.actions[action]),
                    run,
                    step,
                });
            }
        }
        rows
    };
    match q {
        QueryI::Exists(code) => {
            let truth = vm.values(*code)?;
            row.witness = truth.iter().position(|&v| truthy(v));
            Ok(Verdict::Bool(row.witness.is_some()))
        }
        QueryI::Forall(code) => {
            let truth = vm.values(*code)?;
            row.counter = truth.iter().position(|&v| !truthy(v));
            Ok(Verdict::Bool(row.counter.is_none()))
        }
        QueryI::Can(who, code) => {
            let truth: Vec<bool> = vm.values(*code)?.into_iter().map(truthy).collect();
            let game = vm.game(who);
            match vm.winning_policy(&game, &truth)? {
                Some(chosen) => {
                    row.policy = policy_rows(&game, &chosen);
                    row.witness = (0..truth.len()).find(|&l| follows(&game, &chosen, l));
                    Ok(Verdict::Bool(true))
                }
                None => {
                    row.counter = truth.iter().position(|&t| !t);
                    Ok(Verdict::Bool(false))
                }
            }
        }
        QueryI::Num { policy, weight, max, code } => {
            let vals = vm.values(*code)?;
            let (value, chosen, game) = match policy {
                None => (vm.value_of(&vals, *weight, *max)?, None, None),
                Some(who) => {
                    let game = vm.game(who);
                    let (value, chosen) = vm.best_policy(&game, &vals, *weight, *max)?;
                    row.policy = policy_rows(&game, &chosen);
                    (value, Some(chosen), Some(game))
                }
            };
            if weight.is_none() {
                // the history that reaches the value (among those that follow the strategy)
                row.witness = (0..vals.len()).find(|&l| {
                    Ratio::int(vals[l]) == value
                        && match (&game, &chosen) {
                            (Some(game), Some(chosen)) => follows(game, chosen, l),
                            _ => true,
                        }
                });
            }
            Ok(Verdict::Num(value))
        }
        QueryI::Prefer { who, menu, a, b, code, weight } => Ok(Verdict::Bool(vm.prefer(*who, *menu, *a, *b, *code, *weight)?)),
        QueryI::Region(..) => Err(FeanError::image("I-Mode", "a region inside a region")),
    }
}

/// A name for every initial world: the initial values that differ between worlds.
fn world_labels(im: &Image, names: &Names, vm: &Vm) -> Vec<String> {
    let differing: Vec<usize> =
        (0..im.cell_count()).filter(|&c| vm.worlds.iter().any(|w| w.cells[c] != vm.worlds[0].cells[c])).collect();
    vm.worlds
        .iter()
        .enumerate()
        .map(|(w, st)| {
            if let Some(mark) = vm.begun {
                return format!("{} 에서 시작", im.marks[mark]);
            }
            if differing.is_empty() {
                return format!("world {w}");
            }
            let parts: Vec<String> = differing
                .iter()
                .take(4)
                .map(|&c| format!("{}={}", im.cell_name(c), value_text(im, &names.cell_ty[c], st.cells[c])))
                .collect();
            let more = if differing.len() > 4 { ", …" } else { "" };
            format!("world {w} ({}{more})", parts.join(", "))
        })
        .collect()
}

fn plain(v: i64) -> Value {
    if v == NONE {
        Value::Null
    } else {
        json!(v)
    }
}

fn error_json(e: &FeanError) -> Value {
    json!({"kind": e.kind.name(), "code": e.code, "line": e.line, "message": e.message})
}

fn finding_json(f: &Finding) -> Value {
    json!({"severity": f.severity.name(), "kind": f.kind, "code": f.code, "title": f.title, "detail": f.detail,
           "line": SourceMap::line(f.loc), "assignment": f.assignment, "run": f.run, "step": f.step,
           "query": f.query, "solve": f.solve, "count": f.count})
}

/// Everything the IDE shows about a model text, as JSON (`fean inspect`).
pub fn inspect(src: &str, prelude: Option<&str>, budget: usize, views: bool) -> Value {
    let report = analyze(src, prelude, budget);
    match report.analysis {
        Some(mut analysis) => analysis.to_json(views),
        None => json!({
            "ok": false,
            "error": report.error.as_ref().map(error_json),
            "findings": report.findings.iter().map(finding_json).collect::<Vec<_>>(),
        }),
    }
}
