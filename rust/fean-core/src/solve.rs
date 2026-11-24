//! The solver: answers a solve without trying every candidate. Map unit I-16-d.
//!
//! The definition of a solve is to try every candidate (docs/financial-language.md section 5.4);
//! the Python engine does exactly that. Here the model is run at one point of a region while the
//! machine records every decision that depended on a variable (`vm.rs`, `sym.rs`). Inside those
//! conditions the run is the same, so the verdict found at the point holds for the whole region.
//! Each condition is then turned around, one at a time, to give the regions still to be looked at.
//! The answer is the same as the definition's (DR-48); what cannot be settled within the limits is
//! reported as undecided, never guessed.

use std::sync::Arc;

use crate::image::{Dom, Image, Ins, SolveInfo};
use crate::sym::{find, optimum, tighten, Best, Cond, Found, Region, Space, SymCtx, Val, INF};
use crate::syntax::{FeanError, Kind};
use crate::vm::Vm;

/// Most runs of the model one solve may take; the regions left over are undecided.
pub const RUN_LIMIT: usize = 10_000;
/// Most intervals written for one set of a single variable; the rest is reported as undecided.
pub const SPAN_LIMIT: usize = 2_000;
/// Where a quantity is looked for when its range depends on the unknowns.
const OPEN_RANGE: (i64, i64) = (-(1 << 62), 1 << 62);
const MERGE_LIMIT: usize = 1_500;

pub const UNKNOWN: &str = "UNKNOWN";
pub const NONE_TEXT: &str = "none";

#[derive(Clone, Debug, PartialEq)]
pub enum VarKind {
    Bool,
    Enum(usize),
    Time,
    Actor,
    Num,
}

/// One coordinate of a candidate: an unknown, or one part of the state at a point.
#[derive(Clone, Debug)]
pub struct VarInfo {
    pub name: String,
    pub kind: VarKind,
    pub lo: i64,
    pub hi: i64,
    /// Can be empty (an optional cell, or a slot that is not there): then `lo` stands for that.
    pub opt: bool,
    /// The cell whose range reads the unknowns: its bounds are settled only in a run.
    pub open: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// A solution.
    Yes,
    /// A candidate where the goal is false.
    No,
    /// Not a candidate: the filter, an invariant or a range says no.
    Outside,
    /// A candidate where the model is not valid (the rule that says so).
    Invalid(String),
    /// A candidate the limits left open (why).
    Undecided(String),
}

impl Verdict {
    pub fn word(&self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
            Verdict::Outside => "outside",
            Verdict::Invalid(_) => "invalid",
            Verdict::Undecided(_) => "undecided",
        }
    }
}

/// A region where every point has the same verdict, with the point it was found at.
#[derive(Clone, Debug)]
pub struct Leaf {
    pub region: Region,
    pub verdict: Verdict,
    pub sample: Vec<i64>,
    /// For a solution of `min` / `max`: the value of the objective, as a form over the region.
    pub objective: Option<Val>,
}

/// The answer to one solve under one assignment of the parameters.
#[derive(Clone, Debug)]
pub struct Solved {
    pub form: String,
    pub unit: String,
    /// The mark the candidates' states are taken at, if any.
    pub point: Option<usize>,
    pub unknowns: usize,
    pub vars: Vec<VarInfo>,
    pub space: Space,
    pub leaves: Vec<Leaf>,
    pub runs: usize,
    /// No region was left undecided.
    pub complete: bool,
    /// The answer is UNKNOWN.
    pub unknown: bool,
    /// The representative solution: the case plain queries are answered in.
    pub best: Option<Vec<i64>>,
    /// The optimum of `min` / `max`.
    pub value: Option<i128>,
    /// The first candidate that is not a solution.
    pub counter: Option<Vec<i64>>,
    pub text: String,
    /// Set when the solver itself failed; the answer is then UNKNOWN.
    pub error: Option<String>,
    /// Display names of enum values and actors, for writing points.
    enums: Vec<Vec<String>>,
    actors: Vec<String>,
    /// At a point: where the coordinates of the slots begin, and the earliest time. A slot that
    /// is not there is written with the earliest times only (DR-56).
    slots_at: usize,
    earliest: i64,
}

fn reads_unknown(image: &Image, code: Option<usize>) -> bool {
    code.map_or(false, |c| image.codes[c].ins.iter().any(|i| matches!(i, Ins::Unk(_))))
}

fn kind_of(dom: Dom) -> VarKind {
    match dom {
        Dom::Qty => VarKind::Num,
        Dom::Bool => VarKind::Bool,
        Dom::Enum(e) => VarKind::Enum(e),
        Dom::Time => VarKind::Time,
        Dom::Actor => VarKind::Actor,
    }
}

/// The coordinates of the candidates of a solve: the unknowns, then (at a point) the state.
pub fn variables(image: &Arc<Image>, sv: &SolveInfo, pvals: &[i64], budget: usize) -> Result<Vec<VarInfo>, FeanError> {
    let mut vars: Vec<VarInfo> = image
        .unknowns
        .iter()
        .map(|u| VarInfo {
            name: u.name.clone(),
            kind: match (u.kind.as_str(), u.enum_) {
                ("bool", _) => VarKind::Bool,
                ("enum", Some(e)) => VarKind::Enum(e),
                ("time", _) => VarKind::Time,
                ("actor", _) => VarKind::Actor,
                _ => VarKind::Num,
            },
            lo: u.lo,
            hi: u.hi,
            opt: false,
            open: None,
        })
        .collect();
    let Some(mark) = sv.point else { return Ok(vars) };
    // the ranges of the state, worked out once where they do not read the unknowns
    let uvals: Vec<Val> = image.unknowns.iter().map(|u| Val::plain(u.lo)).collect();
    let mut frame = Vm::bare(image.clone(), pvals.to_vec(), uvals, Some(mark), budget, None)?;
    frame.instantiate()?;
    let plain = |name: String, kind: VarKind, lo: i64, hi: i64, opt: bool| VarInfo { name, kind, lo, hi, opt, open: None };
    vars.push(plain("now".into(), VarKind::Time, image.time.0, image.time.1, false));
    for (c, cell) in image.cells.iter().enumerate() {
        let (lo, hi, opt) = frame.cell_range(c);
        let mut var = plain(cell.name.clone(), kind_of(cell.dom), lo - opt as i64, hi, opt);
        if reads_unknown(image, cell.lo) || reads_unknown(image, cell.hi) {
            (var.lo, var.hi, var.open) = (OPEN_RANGE.0, OPEN_RANGE.1, Some(c));
        }
        vars.push(var);
    }
    for (x, asset) in image.assets.iter().enumerate() {
        for (i, who) in image.actors.iter().enumerate() {
            let c = image.holding(x, i);
            let (lo, hi, _) = frame.cell_range(c);
            let mut var = plain(format!("bal({who}, {})", asset.name), VarKind::Num, lo, hi, false);
            if reads_unknown(image, Some(asset.lo)) || reads_unknown(image, Some(asset.hi)) {
                (var.lo, var.hi, var.open) = (OPEN_RANGE.0, OPEN_RANGE.1, Some(c));
            }
            vars.push(var);
        }
    }
    for slot in &image.slots {
        let (who, (lo, hi)) = (&image.actors[slot.actor], Vm::slot_range(image, slot.dom));
        vars.push(plain(format!("mem({who}, {})", slot.key), kind_of(slot.dom), lo - 1, hi, true));
        vars.push(plain(format!("asof({who}, {})", slot.key), VarKind::Time, image.time.0, image.time.1, false));
        vars.push(plain(format!("recv({who}, {})", slot.key), VarKind::Time, image.time.0, image.time.1, false));
    }
    Ok(vars)
}

struct Job<'a> {
    image: &'a Arc<Image>,
    sv: &'a SolveInfo,
    pvals: &'a [i64],
    budget: usize,
    unknowns: usize,
}

/// What one run found: the verdict at the point, the objective (for a solution), and the
/// decisions it depended on.
struct Run {
    verdict: Verdict,
    objective: Option<Val>,
    conds: Vec<Cond>,
}

impl Job<'_> {
    fn phases(&self, vm: &mut Vm, coords: &[Val]) -> Result<(Verdict, Option<Val>), FeanError> {
        if self.sv.point.is_some() {
            vm.instantiate()?;
            if !vm.seat(coords)? || !vm.admits(self.sv.filter)? {
                return Ok((Verdict::Outside, None));
            }
        } else {
            if !vm.admits(self.sv.filter)? {
                return Ok((Verdict::Outside, None));
            }
            vm.instantiate()?;
        }
        vm.explore()?;
        let goal = vm.goal_val(self.sv.goal)?;
        if !vm.decide(goal) {
            return Ok((Verdict::No, None));
        }
        let objective = match self.sv.objective {
            Some(code) => Some(vm.goal_val(code)?),
            None => None,
        };
        Ok((Verdict::Yes, objective))
    }

    /// Run the model at a point, recording. The space comes back with what the run added to it.
    fn run(&self, space: Space, point: &[i64]) -> Result<(Space, Run), String> {
        let mut ctx = SymCtx::begin(space, point.to_vec());
        let forms: Vec<Val> = (0..ctx.space.real).map(|v| Val { v: point[v], f: ctx.space.forms.var(v as u32) }).collect();
        let (uvals, coords) = (forms[..self.unknowns].to_vec(), &forms[self.unknowns..]);
        let mut vm = Vm::bare(self.image.clone(), self.pvals.to_vec(), uvals, self.sv.point, self.budget, Some(ctx))
            .map_err(|e| e.to_string())?;
        let outcome = self.phases(&mut vm, coords);
        let ctx = vm.take_sym().ok_or("the run lost its record")?;
        if let Some(trouble) = ctx.trouble {
            return Err(trouble);
        }
        let (verdict, objective) = match outcome {
            Ok(found) => found,
            Err(e) => match e.kind {
                Kind::Limit => (Verdict::Undecided(e.message.clone()), None),
                Kind::Static | Kind::Model => (Verdict::Invalid(e.code.clone()), None),
                _ => return Err(e.to_string()),
            },
        };
        Ok((ctx.space, Run { verdict, objective, conds: ctx.conds }))
    }
}

/// Solve number `index` of the image under one assignment of the parameters.
pub fn solve(image: &Arc<Image>, index: usize, pvals: &[i64], budget: usize, run_limit: usize) -> Solved {
    let sv = &image.solves[index];
    let mut out = Solved {
        form: sv.form.clone(),
        unit: sv.unit.clone(),
        point: sv.point,
        unknowns: image.unknowns.len(),
        vars: Vec::new(),
        space: Space::default(),
        leaves: Vec::new(),
        runs: 0,
        complete: true,
        unknown: false,
        best: None,
        value: None,
        counter: None,
        text: String::new(),
        error: None,
        enums: image.enums.iter().map(|e| e.1.clone()).collect(),
        actors: image.actors.clone(),
        slots_at: image.unknowns.len() + 1 + image.cell_count(),
        earliest: image.time.0,
    };
    match variables(image, sv, pvals, budget) {
        Ok(vars) => out.vars = vars,
        Err(e) => return out.failed(e.to_string()),
    }
    let bounds: Vec<(i64, i64)> = out.vars.iter().map(|v| (v.lo, v.hi)).collect();
    let mut space = Space::new(&bounds);
    let job = Job { image, sv, pvals, budget, unknowns: out.unknowns };
    let mut todo = vec![Region::whole(&space)];
    while let Some(region) = todo.pop() {
        if out.runs >= run_limit {
            let reason = format!("more than {run_limit} runs");
            out.leaves.push(Leaf { region, verdict: Verdict::Undecided(reason), sample: Vec::new(), objective: None });
            continue;
        }
        let point = match find(&space, &region, false) {
            Found::Empty => continue,
            Found::Unknown => {
                let reason = "no point found or ruled out".to_string();
                out.leaves.push(Leaf { region, verdict: Verdict::Undecided(reason), sample: Vec::new(), objective: None });
                continue;
            }
            Found::Point(p) => p,
        };
        out.runs += 1;
        let run = match job.run(space, &point) {
            Ok((back, run)) => {
                space = back;
                run
            }
            Err(trouble) => {
                out.space = Space::new(&bounds);
                return out.failed(trouble);
            }
        };
        // the region where this run is the run: every decision kept; and, decision by decision,
        // the regions where it was the first to go the other way
        let mut leaf = region;
        for c in run.conds {
            if leaf.implies(&space, &c) {
                continue;
            }
            for (lo, hi) in [(-INF, c.lo - 1), (c.hi + 1, INF)] {
                if lo <= hi && (lo > -INF || hi < INF) {
                    let mut other = leaf.clone();
                    if other.add(&space, Cond { form: c.form, lo, hi }) {
                        todo.push(other);
                    }
                }
            }
            if !leaf.add(&space, c) {
                out.space = space;
                return out.failed("a recorded condition contradicts its own region".into());
            }
        }
        let mut sample = point;
        space.complete(&mut sample);
        out.leaves.push(Leaf { region: leaf, verdict: run.verdict, sample, objective: run.objective });
    }
    out.space = space;
    out.merge();
    if out.leaves.len() <= MERGE_LIMIT {
        for k in 0..out.leaves.len() {
            let tidy = out.tidy(&out.leaves[k].region);
            out.leaves[k].region = tidy;
        }
        out.merge();
    }
    out.conclude();
    out
}

fn before(a: &[i64], b: &[i64]) -> bool {
    a < b
}

impl Solved {
    fn failed(mut self, why: String) -> Solved {
        self.error = Some(why);
        self.unknown = true;
        self.complete = false;
        self.text = UNKNOWN.to_string();
        self
    }

    fn real(&self) -> usize {
        self.vars.len()
    }

    /// Join leaves with the same verdict that differ in the interval of one variable or of one
    /// form only, where those intervals touch.
    fn merge(&mut self) {
        if self.leaves.len() > MERGE_LIMIT {
            return;
        }
        let n = self.space.vars();
        let bounds = |leaf: &Leaf, space: &Space| -> Vec<(i128, i128)> { (0..n).map(|v| (leaf.region.lo(space, v), leaf.region.hi(space, v))).collect() };
        let conds = |leaf: &Leaf| -> Vec<Cond> {
            let mut list = leaf.region.conds.clone();
            list.sort_by_key(|c| c.form);
            list
        };
        let mut items: Vec<(Leaf, Vec<(i128, i128)>, Vec<Cond>)> =
            std::mem::take(&mut self.leaves).into_iter().map(|leaf| (bounds(&leaf, &self.space), conds(&leaf), leaf)).map(|(b, c, l)| (l, b, c)).collect();
        let touch = |a: (i128, i128), b: (i128, i128)| a.0 <= b.1.saturating_add(1) && b.0 <= a.1.saturating_add(1);
        loop {
            let mut joined = false;
            let mut i = 0;
            while i < items.len() {
                let mut j = i + 1;
                while j < items.len() {
                    let same_goal = items[i].0.verdict == items[j].0.verdict
                        && match (items[i].0.objective, items[j].0.objective) {
                            (None, None) => true,
                            (Some(a), Some(b)) => a.f == b.f && (a.f != 0 || a.v == b.v),
                            _ => false,
                        };
                    let mut merged: Option<(Vec<(i128, i128)>, Vec<Cond>)> = None;
                    if same_goal && items[i].2.len() == items[j].2.len() && items[i].2.iter().zip(&items[j].2).all(|(a, b)| a.form == b.form) {
                        let differ_b: Vec<usize> = (0..n).filter(|&v| items[i].1[v] != items[j].1[v]).collect();
                        let differ_c: Vec<usize> = (0..items[i].2.len()).filter(|&k| items[i].2[k] != items[j].2[k]).collect();
                        if differ_b.len() + differ_c.len() == 1 {
                            let (mut b, mut c) = (items[i].1.clone(), items[i].2.clone());
                            if let Some(&v) = differ_b.first() {
                                if touch(items[i].1[v], items[j].1[v]) {
                                    b[v] = (items[i].1[v].0.min(items[j].1[v].0), items[i].1[v].1.max(items[j].1[v].1));
                                    merged = Some((b, c));
                                }
                            } else {
                                let k = differ_c[0];
                                let (x, y) = (items[i].2[k], items[j].2[k]);
                                if touch((x.lo, x.hi), (y.lo, y.hi)) {
                                    c[k] = Cond { form: x.form, lo: x.lo.min(y.lo), hi: x.hi.max(y.hi) };
                                    merged = Some((b, c));
                                }
                            }
                        }
                    }
                    match merged {
                        Some((b, c)) => {
                            items[i].1 = b;
                            items[i].2 = c;
                            items.swap_remove(j);
                            joined = true;
                        }
                        None => j += 1,
                    }
                }
                i += 1;
            }
            if !joined {
                break;
            }
        }
        self.leaves = items
            .into_iter()
            .map(|(mut leaf, b, c)| {
                leaf.region.lo = b.iter().map(|x| x.0).collect();
                leaf.region.hi = b.iter().map(|x| x.1).collect();
                leaf.region.conds = c;
                leaf
            })
            .collect();
        // a fixed order: by the sample points, so that the same model always reads the same
        self.leaves.sort_by(|a, b| a.sample.cmp(&b.sample));
    }

    /// The same region written with fewer conditions: a bound or a condition that the others
    /// already imply is left out. (The points of the region do not change.)
    fn tidy(&self, region: &Region) -> Region {
        let space = &self.space;
        let n = space.vars();
        // first the bounds as narrow as they are, then: conditions the bounds imply go, and
        // after them bounds the remaining conditions imply
        let mut r = tighten(space, region).unwrap_or_else(|| region.clone());
        for v in r.lo.len()..n {
            r.lo.push(space.lo[v]);
            r.hi.push(space.hi[v]);
        }
        let empty = |test: &Region| matches!(find(space, test, false), Found::Empty);
        let mut k = 0;
        while k < r.conds.len() {
            let c = r.conds[k];
            if c.lo > -INF {
                let mut test = r.clone();
                test.conds[k] = Cond { form: c.form, lo: -INF, hi: c.hi.min(c.lo - 1) };
                if empty(&test) {
                    r.conds[k].lo = -INF;
                }
            }
            let c = r.conds[k];
            if c.hi < INF {
                let mut test = r.clone();
                test.conds[k] = Cond { form: c.form, lo: c.lo.max(c.hi + 1), hi: INF };
                if empty(&test) {
                    r.conds[k].hi = INF;
                }
            }
            if r.conds[k].lo <= -INF && r.conds[k].hi >= INF {
                r.conds.remove(k);
            } else {
                k += 1;
            }
        }
        // quotients first, so that what remains is said about the model's own names where it can be
        for v in (self.real()..n).chain(0..self.real()) {
            if r.lo[v] > space.lo[v] {
                // without this bound: is there a point below it?
                let mut test = r.clone();
                test.lo[v] = space.lo[v];
                test.hi[v] = test.hi[v].min(r.lo[v] - 1);
                if empty(&test) {
                    r.lo[v] = space.lo[v];
                }
            }
            if r.hi[v] < space.hi[v] {
                let mut test = r.clone();
                test.hi[v] = space.hi[v];
                test.lo[v] = test.lo[v].max(r.hi[v] + 1);
                if empty(&test) {
                    r.hi[v] = space.hi[v];
                }
            }
        }
        r
    }

    /// The first point of a leaf, as its real coordinates.
    fn first(&self, leaf: &Leaf) -> Found {
        match find(&self.space, &leaf.region, true) {
            Found::Point(mut p) => {
                p.truncate(self.real());
                Found::Point(p)
            }
            other => other,
        }
    }

    /// The first point over the leaves with a verdict: (point, was every leaf searched to the end).
    fn first_of(&self, wanted: impl Fn(&Verdict) -> bool) -> (Option<Vec<i64>>, bool) {
        let (mut best, mut sure): (Option<Vec<i64>>, bool) = (None, true);
        for leaf in self.leaves.iter().filter(|l| wanted(&l.verdict)) {
            match self.first(leaf) {
                Found::Point(p) => {
                    if best.as_ref().map_or(true, |b| before(&p, b)) {
                        best = Some(p);
                    }
                }
                Found::Empty => {}
                Found::Unknown => sure = false,
            }
        }
        (best, sure)
    }

    fn conclude(&mut self) {
        let open = |v: &Verdict| matches!(v, Verdict::Undecided(_));
        self.complete = !self.leaves.iter().any(|l| open(&l.verdict));
        let (first, sure) = self.first_of(|v| *v == Verdict::Yes);
        let (first_open, open_sure) = self.first_of(open);
        self.counter = self.first_of(|v| matches!(v, Verdict::No | Verdict::Invalid(_))).0;
        match self.form.as_str() {
            "find" => {
                self.best = if sure { first } else { None };
                self.text = self.set_text();
            }
            "any" => {
                // UNKNOWN when an open candidate could come before the first solution
                let overtaken = match (&first, &first_open) {
                    (_, None) => !open_sure,
                    (None, Some(_)) => true,
                    (Some(found), Some(other)) => before(other, found) || !open_sure,
                };
                self.unknown = !sure || overtaken;
                self.best = if self.unknown { None } else { first };
                self.text = match (&self.best, self.unknown) {
                    (_, true) => UNKNOWN.to_string(),
                    (Some(p), _) => self.point_text(p),
                    (None, _) => NONE_TEXT.to_string(),
                };
            }
            _ => {
                let max = self.form == "max";
                let mut found: Option<(i128, Vec<i64>)> = None;
                self.unknown = !self.complete;
                let leaves: Vec<(Region, Val)> = self
                    .leaves
                    .iter()
                    .filter(|l| l.verdict == Verdict::Yes)
                    .filter_map(|l| l.objective.map(|o| (l.region.clone(), o)))
                    .collect();
                for (region, objective) in leaves {
                    if self.unknown {
                        break;
                    }
                    match optimum(&mut self.space, &region, objective.f, objective.v, max) {
                        Best::Value(v, mut p) => {
                            p.truncate(self.real());
                            let better = match &found {
                                None => true,
                                Some((best, at)) => (if max { v > *best } else { v < *best }) || (v == *best && before(&p, at)),
                            };
                            if better {
                                found = Some((v, p));
                            }
                        }
                        Best::Empty => {}
                        Best::Unknown => self.unknown = true,
                    }
                }
                if self.unknown {
                    self.text = UNKNOWN.to_string();
                } else if let Some((v, p)) = found {
                    let unit = if self.unit.is_empty() { String::new() } else { format!("[{}]", self.unit) };
                    self.text = format!("{v}{unit} at {}", self.point_text(&p));
                    (self.value, self.best) = (Some(v), Some(p));
                } else {
                    self.text = NONE_TEXT.to_string();
                }
            }
        }
    }

    /// The verdict at a point of the real coordinates (the same as trying that candidate).
    pub fn member(&self, point: &[i64]) -> Verdict {
        if point.len() != self.real() || (0..point.len()).any(|v| (point[v] as i128) < self.space.lo[v] || (point[v] as i128) > self.space.hi[v]) {
            return Verdict::Outside;
        }
        if self.point.is_some() {
            // "no slot" has one spelling: the two times at their earliest
            for k in (self.slots_at..self.real()).step_by(3) {
                if point[k] == self.vars[k].lo && (point[k + 1], point[k + 2]) != (self.earliest, self.earliest) {
                    return Verdict::Outside;
                }
            }
        }
        let mut full = point.to_vec();
        if !self.space.complete(&mut full) {
            return Verdict::Undecided("a quotient does not fit".into());
        }
        match self.leaves.iter().find(|l| l.region.contains(&self.space, &full)) {
            Some(leaf) => leaf.verdict.clone(),
            None => Verdict::Undecided("no region holds this point".into()),
        }
    }

    // ---- writing --------------------------------------------------------------------------------
    /// A value of a coordinate as the specification writes it.
    pub fn value_text(&self, var: usize, n: i64) -> String {
        let info = &self.vars[var];
        if info.opt && n == info.lo && info.open.is_none() {
            return "empty".to_string();
        }
        match &info.kind {
            VarKind::Bool => (if n != 0 { "true" } else { "false" }).to_string(),
            VarKind::Enum(e) => self.enums[*e].get(n as usize).cloned().unwrap_or_else(|| n.to_string()),
            VarKind::Actor => self.actors.get(n as usize).cloned().unwrap_or_else(|| n.to_string()),
            VarKind::Time => format!("@{n}"),
            VarKind::Num => n.to_string(),
        }
    }

    /// `cap=80, fee=0`.
    pub fn point_text(&self, point: &[i64]) -> String {
        (0..self.real().min(point.len())).map(|v| format!("{}={}", self.vars[v].name, self.value_text(v, point[v]))).collect::<Vec<_>>().join(", ")
    }

    /// Is this auxiliary variable `floor(-x / d)`, which reads better as `-ceil(x / d)`?
    fn turned(&self, v: usize) -> bool {
        v >= self.real() && {
            let aux = &self.space.aux[v - self.real()];
            let form = self.space.forms.get(aux.num);
            aux.div > 0 && form.terms.first().map_or(false, |t| t.1 < 0) && form.terms.iter().all(|t| t.1 < 0) && form.c <= 0
        }
    }

    /// The name of a variable; for a quotient, how it is computed. With `turned`, the name stands
    /// for the variable with its sign changed.
    fn var_name(&self, v: usize) -> String {
        if v < self.real() {
            return self.vars[v].name.clone();
        }
        let aux = &self.space.aux[v - self.real()];
        if self.turned(v) {
            let form = self.space.forms.get(aux.num);
            let terms: Vec<(u32, i128)> = form.terms.iter().map(|&(var, k)| (var, -k)).collect();
            return format!("ceil({} / {})", self.terms_text(&terms, -form.c), aux.div);
        }
        if aux.div < 0 {
            return format!("floor({} / ({}))", self.form_text(aux.num), aux.div);
        }
        format!("floor({} / {})", self.form_text(aux.num), aux.div)
    }

    /// Terms with every turned variable written under its other name (and the sign changed).
    fn straight(&self, terms: &[(u32, i128)]) -> Vec<(u32, i128)> {
        terms.iter().map(|&(v, k)| if self.turned(v as usize) { (v, -k) } else { (v, k) }).collect()
    }

    fn terms_text(&self, terms: &[(u32, i128)], konst: i128) -> String {
        let mut out = String::new();
        for &(v, k) in terms {
            let name = self.var_name(v as usize);
            let (sign, size) = (if k < 0 { "-" } else { "+" }, k.abs());
            let term = if size == 1 { name } else { format!("{size}*{name}") };
            if out.is_empty() {
                out = if k < 0 { format!("-{term}") } else { term };
            } else {
                out += &format!(" {sign} {term}");
            }
        }
        if out.is_empty() {
            return konst.to_string();
        }
        if konst > 0 {
            out += &format!(" + {konst}");
        } else if konst < 0 {
            out += &format!(" - {}", -konst);
        }
        out
    }

    fn form_text(&self, f: u32) -> String {
        let form = self.space.forms.get(f);
        self.terms_text(&self.straight(&form.terms), form.c)
    }

    /// One condition on a form of several variables: what is added on the left, what is taken
    /// away on the right.
    fn cond_text(&self, c: &Cond) -> String {
        let mut terms = self.straight(&self.space.forms.get(c.form).terms);
        let (mut lo, mut hi) = (c.lo, c.hi);
        if terms.iter().all(|t| t.1 < 0) {
            // nothing would stand on the left: read the condition the other way round
            terms = terms.iter().map(|&(v, k)| (v, -k)).collect();
            (lo, hi) = (if hi >= INF { -INF } else { -hi }, if lo <= -INF { INF } else { -lo });
        }
        let left: Vec<(u32, i128)> = terms.iter().copied().filter(|t| t.1 > 0).collect();
        let right: Vec<(u32, i128)> = terms.iter().filter(|t| t.1 < 0).map(|&(v, k)| (v, -k)).collect();
        let lhs = self.terms_text(&left, 0);
        let rhs = |bound: i128| self.terms_text(&right, bound);
        if lo == hi {
            format!("{lhs} = {}", rhs(lo))
        } else if hi >= INF {
            format!("{lhs} >= {}", rhs(lo))
        } else if lo <= -INF {
            format!("{lhs} <= {}", rhs(hi))
        } else {
            format!("{lo} <= {} <= {hi}", self.terms_text(&terms, 0))
        }
    }

    /// The bounds of one coordinate in a region, when they say more than its declaration.
    fn bound_text(&self, v: usize, lo: i128, hi: i128) -> Option<String> {
        let (least, greatest) = (self.space.lo[v], self.space.hi[v]);
        if lo <= least && hi >= greatest {
            return None;
        }
        let name = self.var_name(v);
        // a turned quotient is bounded the other way round
        let (lo, hi, least, greatest) = if self.turned(v) {
            let flip = |x: i128| if x.abs() >= INF { -x.signum() * INF } else { -x };
            (flip(hi), flip(lo), flip(greatest), flip(least))
        } else {
            (lo, hi, least, greatest)
        };
        let show = |n: i128| if v < self.real() { self.value_text(v, n as i64) } else { n.to_string() };
        let info = (v < self.real()).then(|| &self.vars[v]);
        let listed = info.map_or(false, |i| matches!(i.kind, VarKind::Bool | VarKind::Enum(_) | VarKind::Actor));
        if listed && hi - lo < 64 {
            return Some(format!("{name}={}", (lo..=hi).map(show).collect::<Vec<_>>().join("|")));
        }
        if let Some(i) = info.filter(|i| i.opt && i.open.is_none()) {
            // the least value stands for empty
            let none = i.lo as i128;
            if lo == none && hi == none {
                return Some(format!("{name}=empty"));
            }
            if lo == none {
                return Some(format!("{name}=empty|{}", if hi == none + 1 { show(hi) } else { format!("{}..{}", show(none + 1), show(hi)) }));
            }
            if lo == none + 1 && hi >= greatest {
                return Some(format!("{name}!=empty"));
            }
        }
        Some(if lo == hi {
            format!("{name}={}", show(lo))
        } else if lo > least && hi < greatest {
            format!("{name}={}..{}", show(lo), show(hi))
        } else if lo > least {
            format!("{name} >= {}", show(lo))
        } else {
            format!("{name} <= {}", show(hi))
        })
    }

    /// `[cap >= 80, cap >= fee + 80]`: every candidate that keeps all the conditions.
    pub fn region_text(&self, region: &Region) -> String {
        let mut parts: Vec<String> =
            (0..self.space.vars()).filter_map(|v| self.bound_text(v, region.lo(&self.space, v), region.hi(&self.space, v))).collect();
        let mut conds = region.conds.clone();
        conds.sort_by_key(|c| self.space.forms.get(c.form).terms.clone());
        parts.extend(conds.iter().filter(|c| c.lo > -INF || c.hi < INF).map(|c| self.cond_text(c)));
        format!("[{}]", parts.join(", "))
    }

    /// The sets of a `find`: the solutions, and the candidates left undecided.
    fn set_text(&self) -> String {
        let open = |v: &Verdict| matches!(v, Verdict::Undecided(_));
        if self.real() == 1 {
            let (hits, undecided) = self.sweep();
            let show = |spans: &[(i64, i64)]| -> String {
                let mut parts: Vec<String> = Vec::new();
                for &(a, b) in spans {
                    if self.vars[0].kind == VarKind::Num && !(self.vars[0].opt && a == self.vars[0].lo) {
                        parts.push(if a == b { a.to_string() } else { format!("{a}..{b}") });
                    } else if self.vars[0].kind == VarKind::Num {
                        parts.push("empty".into());
                        if b > a {
                            parts.push(if a + 1 == b { b.to_string() } else { format!("{}..{b}", a + 1) });
                        }
                    } else {
                        parts.extend((a..=b).map(|n| self.value_text(0, n)));
                    }
                }
                format!("{{{}}}", parts.join(", "))
            };
            return show(&hits) + &(if undecided.is_empty() { String::new() } else { format!(" unknown {}", show(&undecided)) });
        }
        let list = |wanted: &dyn Fn(&Verdict) -> bool| -> String {
            let parts: Vec<String> = self.leaves.iter().filter(|l| wanted(&l.verdict)).map(|l| self.region_text(&l.region)).collect();
            format!("{{{}}}", parts.join(", "))
        };
        let hits = list(&|v| *v == Verdict::Yes);
        if self.leaves.iter().any(|l| open(&l.verdict)) {
            format!("{hits} unknown {}", list(&open))
        } else {
            hits
        }
    }

    /// For a single coordinate: the intervals of solutions and of undecided candidates, in order.
    fn sweep(&self) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
        let (lo, hi) = (self.vars[0].lo, self.vars[0].hi);
        let class = |v: &Verdict| match v {
            Verdict::Yes => 0u8,
            Verdict::Undecided(_) => 1,
            _ => 2,
        };
        let (mut hits, mut undecided): (Vec<(i64, i64)>, Vec<(i64, i64)>) = (Vec::new(), Vec::new());
        let mut at = lo;
        let var = self.space.forms.find_var(0);
        while at <= hi {
            let here = class(&self.member(&[at]));
            // where does a leaf of another class begin, after `at`?
            let (mut next, mut sure) = (hi as i128 + 1, true);
            for leaf in self.leaves.iter().filter(|l| class(&l.verdict) != here) {
                let Some(var) = var else {
                    sure = false;
                    break;
                };
                let mut part = leaf.region.clone();
                if !part.add(&self.space, Cond { form: var, lo: at as i128 + 1, hi: INF }) {
                    continue;
                }
                match find(&self.space, &part, true) {
                    Found::Point(p) => next = next.min(p[0] as i128),
                    Found::Empty => {}
                    Found::Unknown => sure = false,
                }
            }
            let spans = hits.len() + undecided.len();
            if !sure || spans >= SPAN_LIMIT {
                undecided.push((at, hi)); // the rest could not be told apart: left open
                break;
            }
            let end = (next - 1) as i64;
            match here {
                0 => hits.push((at, end)),
                1 => undecided.push((at, end)),
                _ => {}
            }
            at = match end.checked_add(1) {
                Some(n) => n,
                None => break,
            };
        }
        (hits, undecided)
    }

    /// The values of the unknowns and the coordinates of the state (if the solve has a point) of
    /// a candidate: what an instance of the model at that candidate is made from.
    pub fn parts(&self, point: &[i64]) -> (Vec<i64>, Option<Vec<i64>>) {
        let uvals = point[..self.unknowns].to_vec();
        (uvals, self.point.map(|_| point[self.unknowns..self.real()].to_vec()))
    }
}
