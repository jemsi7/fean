//! A verified image and its public parameters: one machine per assignment, results written as text.
//!
//! Port of `Session` in `fean/vm.py` (Map units I-09-d, I-17-a). The text of a result is the
//! notation of docs/financial-language.md sections 5.3 and 5.4. Solves are answered by the solver
//! (`solve.rs`); where the model has unknowns, a plain query is answered in the case of each solve.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::image::{verify, Image, QueryI};
use crate::solve::{solve, Solved, NONE_TEXT, RUN_LIMIT};
use crate::syntax::{FeanError, Kind, R};
use crate::vm::{Answer, Ratio, Start, Vm};

pub const UNKNOWN: &str = "UNKNOWN";

/// The verdict of a query for one assignment of the public parameters.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Bool(bool),
    Num(Ratio),
    /// The search budget was exceeded.
    Unknown,
}

/// The result of a query over every assignment.
#[derive(Clone, Debug, PartialEq)]
pub enum Res {
    Plain(Verdict),
    /// The parameter values (for `params`) where the inner query holds, and those left undecided.
    Region { params: Vec<usize>, hits: Vec<Vec<i64>>, undecided: Vec<Vec<i64>> },
    /// One result per value of the parameters `params`.
    Table { params: Vec<usize>, rows: Vec<(Vec<i64>, Res)> },
    /// A model with unknowns: the result in the case of each solve (None: the solve has no case).
    Cases(Vec<(String, Option<Res>)>),
}

/// [0, 1, 2, 5] -> ["0..2", "5"].
pub fn spans(xs: &[i64]) -> Vec<String> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < xs.len() {
        let mut j = i;
        while j + 1 < xs.len() && xs[j].checked_add(1) == Some(xs[j + 1]) {
            j += 1;
        }
        out.push(if i == j { xs[i].to_string() } else { format!("{}..{}", xs[i], xs[j]) });
        i = j + 1;
    }
    out
}

pub struct Session {
    pub image: Arc<Image>,
    pub budget: usize,
    /// Most runs of the model one solve may take.
    pub run_limit: usize,
    vms: HashMap<Vec<i64>, Vm>,
    solved: HashMap<(usize, Vec<i64>), Arc<Solved>>,
    cases: HashMap<(usize, Vec<i64>), Vm>,
}

impl Session {
    /// Verify and load an image.
    pub fn new(image: &Value, budget: usize) -> R<Session> {
        verify(image)?;
        Ok(Session::from_image(Arc::new(Image::decode(image)), budget))
    }

    pub fn from_image(image: Arc<Image>, budget: usize) -> Session {
        Session { image, budget, run_limit: RUN_LIMIT, vms: HashMap::new(), solved: HashMap::new(), cases: HashMap::new() }
    }

    /// Every assignment of the public parameters, the last parameter turning fastest.
    pub fn assignments(&self) -> Vec<Vec<i64>> {
        let mut out: Vec<Vec<i64>> = vec![Vec::new()];
        for p in &self.image.params {
            let mut next = Vec::with_capacity(out.len() * p.values.len());
            for row in &out {
                for &v in &p.values {
                    let mut longer = row.clone();
                    longer.push(v);
                    next.push(longer);
                }
            }
            out = next;
        }
        out
    }

    /// The machine of an assignment (built once).
    pub fn vm(&mut self, pvals: &[i64]) -> R<&mut Vm> {
        if !self.vms.contains_key(pvals) {
            let vm = Vm::new(self.image.clone(), pvals.to_vec(), self.budget)?;
            self.vms.insert(pvals.to_vec(), vm);
        }
        Ok(self.vms.get_mut(pvals).unwrap())
    }

    /// Take the machine of an assignment out of the session (for the history views).
    pub fn take_vm(&mut self, pvals: &[i64]) -> R<Vm> {
        self.vm(pvals)?;
        Ok(self.vms.remove(pvals).unwrap())
    }

    pub fn evaluate(&mut self, pvals: &[i64], q: &QueryI) -> R<Verdict> {
        match self.vm(pvals)?.query(q) {
            Ok(Answer::Bool(b)) => Ok(Verdict::Bool(b)),
            Ok(Answer::Num(n)) => Ok(Verdict::Num(n)),
            Err(FeanError { kind: Kind::Limit, .. }) => Ok(Verdict::Unknown),
            Err(e) => Err(e),
        }
    }

    pub fn result(&mut self, query: usize) -> R<Res> {
        let q = self.image.queries[query].q.clone();
        let image = self.image.clone();
        let assignments = self.assignments();
        if image.unknowns.is_empty() {
            return tabulate(&image, &assignments, &q, &mut |i, q| self.evaluate(&assignments[i], q));
        }
        // unknowns have no value by themselves: the query is answered where each solve's
        // representative solution holds
        let mut rows = Vec::new();
        for (s, sv) in image.solves.iter().enumerate() {
            let found: Vec<Vec<i64>> = assignments.iter().filter(|pv| self.solved(s, pv).best.is_some()).cloned().collect();
            if image.params.is_empty() && found.is_empty() {
                rows.push((sv.name.clone(), None));
                continue;
            }
            let res = tabulate(&image, &found, &q, &mut |i, q| self.evaluate_case(s, &found[i], q))?;
            rows.push((sv.name.clone(), Some(res)));
        }
        Ok(Res::Cases(rows))
    }

    // ---- solves ----------------------------------------------------------------------------------
    /// The answer to a solve under one assignment (worked out once).
    pub fn solved(&mut self, index: usize, pvals: &[i64]) -> Arc<Solved> {
        let key = (index, pvals.to_vec());
        if !self.solved.contains_key(&key) {
            let answer = solve(&self.image, index, pvals, self.budget, self.run_limit);
            self.solved.insert(key.clone(), Arc::new(answer));
        }
        self.solved[&key].clone()
    }

    /// The machine of the model at one candidate of a solve.
    pub fn instance(&mut self, index: usize, pvals: &[i64], point: &[i64]) -> R<Vm> {
        let solved = self.solved(index, pvals);
        if point.len() != solved.vars.len() {
            return Err(FeanError::image("I-Ref", "a candidate needs a value for every coordinate"));
        }
        let (uvals, coords) = solved.parts(point);
        let start = match (solved.point, coords) {
            (Some(mark), Some(coords)) => Some(Start { mark, coords }),
            _ => None,
        };
        Vm::open(self.image.clone(), pvals.to_vec(), uvals, start, self.budget)
    }

    /// A query in the case of a solve: where its representative solution holds.
    fn evaluate_case(&mut self, index: usize, pvals: &[i64], q: &QueryI) -> R<Verdict> {
        let key = (index, pvals.to_vec());
        if !self.cases.contains_key(&key) {
            let best = self.solved(index, pvals).best.clone().ok_or_else(|| FeanError::image("I-Ref", "this solve has no case"))?;
            let vm = self.instance(index, pvals, &best)?;
            self.cases.insert(key.clone(), vm);
        }
        match self.cases.get_mut(&key).unwrap().query(q) {
            Ok(Answer::Bool(b)) => Ok(Verdict::Bool(b)),
            Ok(Answer::Num(n)) => Ok(Verdict::Num(n)),
            Err(FeanError { kind: Kind::Limit, .. }) => Ok(Verdict::Unknown),
            Err(e) => Err(e),
        }
    }

    /// The result of a solve as text: its answer, or one answer per assignment.
    pub fn solve_text(&mut self, index: usize) -> String {
        let image = self.image.clone();
        if image.params.is_empty() {
            return self.solved(index, &[]).text.clone();
        }
        let every: Vec<usize> = (0..image.params.len()).collect();
        let rows: Vec<String> =
            self.assignments().iter().map(|pv| format!("{}: {}", head(&image, &every, pv), self.solved(index, pv).text)).collect();
        format!("{{{}}}", rows.join(", "))
    }

    /// The result of a query as text.
    pub fn text(&mut self, query: usize) -> R<String> {
        let res = self.result(query)?;
        Ok(self.show(&res, &self.image.queries[query].unit))
    }

    /// Every query with its result text, in declaration order; then every solve.
    pub fn results(&mut self) -> R<Vec<(String, String)>> {
        let mut out = Vec::new();
        for i in 0..self.image.queries.len() {
            out.push((self.image.queries[i].name.clone(), self.text(i)?));
        }
        for i in 0..self.image.solves.len() {
            out.push((self.image.solves[i].name.clone(), self.solve_text(i)));
        }
        Ok(out)
    }

    /// A parameter value as it was written in the model.
    pub fn shown(&self, param: usize, v: i64) -> String {
        shown(&self.image, param, v)
    }

    pub fn show(&self, r: &Res, unit: &str) -> String {
        show(&self.image, r, unit)
    }
}

/// The result of a query over every assignment, given the verdict of each assignment
/// (`eval` receives the number of the assignment and the query to judge there).
pub fn tabulate(
    image: &Image,
    assignments: &[Vec<i64>],
    q: &QueryI,
    eval: &mut dyn FnMut(usize, &QueryI) -> R<Verdict>,
) -> R<Res> {
    let every: Vec<usize> = (0..image.params.len()).collect();
    if let QueryI::Region(params, inner) = q {
        let rest: Vec<usize> = every.iter().copied().filter(|p| !params.contains(p)).collect();
        let mut table: Vec<(Vec<i64>, Vec<Vec<i64>>, Vec<Vec<i64>>)> = Vec::new();
        for (i, pv) in assignments.iter().enumerate() {
            let key: Vec<i64> = rest.iter().map(|&p| pv[p]).collect();
            let row = match table.iter().position(|(k, _, _)| *k == key) {
                Some(row) => row,
                None => {
                    table.push((key, Vec::new(), Vec::new()));
                    table.len() - 1
                }
            };
            let point: Vec<i64> = params.iter().map(|&p| pv[p]).collect();
            match eval(i, inner)? {
                Verdict::Bool(true) => table[row].1.push(point),
                Verdict::Unknown => table[row].2.push(point),
                _ => {}
            }
        }
        let region = |hits, undecided| Res::Region { params: params.clone(), hits, undecided };
        if rest.is_empty() {
            let (_, hits, undecided) = table.into_iter().next().unwrap_or_default();
            return Ok(region(hits, undecided));
        }
        let rows = table.into_iter().map(|(key, hits, undecided)| (key, region(hits, undecided))).collect();
        return Ok(Res::Table { params: rest, rows });
    }
    if every.is_empty() {
        return Ok(Res::Plain(eval(0, q)?));
    }
    let mut rows = Vec::new();
    for (i, pv) in assignments.iter().enumerate() {
        rows.push((pv.clone(), Res::Plain(eval(i, q)?)));
    }
    Ok(Res::Table { params: every, rows })
}

/// A parameter value as it was written in the model.
pub fn shown(image: &Image, param: usize, v: i64) -> String {
    let p = &image.params[param];
    match p.values.iter().position(|x| *x == v) {
        Some(i) => p.show[i].clone(),
        None => v.to_string(),
    }
}

/// "name=value, name=value" for an assignment of the parameters `params`.
pub fn head(image: &Image, params: &[usize], key: &[i64]) -> String {
    let parts: Vec<String> = params.iter().zip(key).map(|(&p, &v)| format!("{}={}", image.params[p].name, shown(image, p, v))).collect();
    parts.join(", ")
}

/// A verdict as text (docs/financial-language.md section 5.3).
pub fn verdict_text(v: &Verdict, unit: &str) -> String {
    match v {
        Verdict::Unknown => UNKNOWN.to_string(),
        Verdict::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
        Verdict::Num(n) => {
            if unit.is_empty() {
                n.text()
            } else {
                format!("{}[{unit}]", n.text())
            }
        }
    }
}

/// A result as text.
pub fn show(image: &Image, r: &Res, unit: &str) -> String {
    match r {
        Res::Table { params, rows } => {
            let rows: Vec<String> =
                rows.iter().map(|(key, val)| format!("{}: {}", head(image, params, key), show(image, val, unit))).collect();
            format!("{{{}}}", rows.join(", "))
        }
        Res::Region { params, hits, undecided } => {
            let list = |hits: &[Vec<i64>]| -> String {
                if params.len() == 1 {
                    let xs: Vec<i64> = hits.iter().map(|h| h[0]).collect();
                    if image.params[params[0]].kind == "num" {
                        return format!("{{{}}}", spans(&xs).join(", "));
                    }
                    let names: Vec<String> = xs.iter().map(|&x| shown(image, params[0], x)).collect();
                    return format!("{{{}}}", names.join(", "));
                }
                let tuples: Vec<String> = hits
                    .iter()
                    .map(|h| {
                        let parts: Vec<String> = params.iter().zip(h).map(|(&p, &x)| shown(image, p, x)).collect();
                        format!("({})", parts.join(", "))
                    })
                    .collect();
                format!("{{{}}}", tuples.join(", "))
            };
            if undecided.is_empty() {
                list(hits)
            } else {
                format!("{} unknown {}", list(hits), list(undecided))
            }
        }
        Res::Plain(v) => verdict_text(v, unit),
        Res::Cases(rows) => {
            let rows: Vec<String> = rows
                .iter()
                .map(|(solve, res)| format!("{solve}: {}", res.as_ref().map_or(NONE_TEXT.to_string(), |r| show(image, r, unit))))
                .collect();
            format!("{{{}}}", rows.join(", "))
        }
    }
}
