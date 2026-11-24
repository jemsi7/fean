//! What the solver reasons with: affine forms over its variables, range conditions on them,
//! regions, and whether a region holds a point of whole numbers. Map unit I-16-b.
//!
//! A run of the machine at one point records every decision that depended on a variable as a
//! condition `lo <= form <= hi`. Wherever all those conditions hold the run goes the same way, so
//! the answer found at the point is the answer of the whole region (DR-53). Nothing here guesses:
//! a region is called empty only when that is proved, and a point is returned only after it was
//! checked against every condition.

use std::collections::{HashMap, HashSet};

/// A bound at or beyond this is no bound.
pub const INF: i128 = i128::MAX / 8;

/// A machine value while decisions are recorded: the value at the sample point, and the form that
/// gives it everywhere else (0: the value is a constant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Val {
    pub v: i64,
    pub f: u32,
}

impl Val {
    #[inline]
    pub fn plain(v: i64) -> Val {
        Val { v, f: 0 }
    }
}

/// `c + sum(coefficient * variable)`; terms are sorted by variable and never zero.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Form {
    pub c: i128,
    pub terms: Vec<(u32, i128)>,
}

/// Every form met so far, each stored once. Number 0 stands for "no form": a constant.
#[derive(Clone, Debug)]
pub struct Forms {
    list: Vec<Form>,
    index: HashMap<Form, u32>,
}

impl Default for Forms {
    fn default() -> Self {
        Forms { list: vec![Form::default()], index: HashMap::new() }
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}

pub fn floor_div(a: i128, b: i128) -> i128 {
    let q = a / b;
    if a % b != 0 && ((a % b < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

pub fn ceil_div(a: i128, b: i128) -> i128 {
    -floor_div(-a, b)
}

fn clamp(x: i128) -> i128 {
    x.clamp(-INF, INF)
}

impl Forms {
    pub fn get(&self, id: u32) -> &Form {
        &self.list[id as usize]
    }

    fn intern(&mut self, form: Form) -> u32 {
        if form.terms.is_empty() {
            return 0;
        }
        if let Some(&id) = self.index.get(&form) {
            return id;
        }
        let id = self.list.len() as u32;
        self.index.insert(form.clone(), id);
        self.list.push(form);
        id
    }

    pub fn var(&mut self, v: u32) -> u32 {
        self.intern(Form { c: 0, terms: vec![(v, 1)] })
    }

    /// The form that is exactly this variable, if it was ever made.
    pub fn find_var(&self, v: u32) -> Option<u32> {
        self.index.get(&Form { c: 0, terms: vec![(v, 1)] }).copied()
    }

    /// The variable a form is, when it is exactly one variable.
    pub fn as_var(&self, id: u32) -> Option<u32> {
        let f = self.get(id);
        match f.terms.as_slice() {
            [(v, 1)] if f.c == 0 => Some(*v),
            _ => None,
        }
    }

    /// `ka * form(a) + kb * form(b) + c`. None when a number does not fit.
    pub fn combine(&mut self, a: u32, ka: i128, b: u32, kb: i128, c: i128) -> Option<u32> {
        let (fa, fb) = (&self.list[a as usize], &self.list[b as usize]);
        let mut konst = c.checked_add(fa.c.checked_mul(ka)?)?.checked_add(fb.c.checked_mul(kb)?)?;
        let mut terms: Vec<(u32, i128)> = Vec::with_capacity(fa.terms.len() + fb.terms.len());
        let (mut i, mut j) = (0, 0);
        while i < fa.terms.len() || j < fb.terms.len() {
            let take_a = j == fb.terms.len() || (i < fa.terms.len() && fa.terms[i].0 <= fb.terms[j].0);
            let take_b = i == fa.terms.len() || (j < fb.terms.len() && fb.terms[j].0 <= fa.terms[i].0);
            let (var, mut coef) = (if take_a { fa.terms[i].0 } else { fb.terms[j].0 }, 0i128);
            if take_a {
                coef = coef.checked_add(fa.terms[i].1.checked_mul(ka)?)?;
                i += 1;
            }
            if take_b {
                coef = coef.checked_add(fb.terms[j].1.checked_mul(kb)?)?;
                j += 1;
            }
            if coef != 0 {
                if coef.abs() >= INF {
                    return None;
                }
                terms.push((var, coef));
            }
        }
        if konst.abs() >= INF {
            return None;
        }
        if terms.is_empty() {
            konst = 0;
        }
        Some(self.intern(Form { c: konst, terms }))
    }

    pub fn eval(&self, id: u32, point: &[i64]) -> i128 {
        let f = self.get(id);
        f.terms.iter().fold(f.c, |sum, (v, k)| sum.saturating_add(k.saturating_mul(point[*v as usize] as i128)))
    }

    /// The least and greatest value of a form over a box.
    pub fn range(&self, id: u32, lo: &dyn Fn(u32) -> i128, hi: &dyn Fn(u32) -> i128) -> (i128, i128) {
        let f = self.get(id);
        let (mut least, mut greatest) = (f.c, f.c);
        for &(v, k) in &f.terms {
            let (a, b) = (lo(v), hi(v));
            let (x, y) = if k > 0 { (a, b) } else { (b, a) };
            least = if x.abs() >= INF || least.abs() >= INF { -INF } else { clamp(least.saturating_add(k.saturating_mul(x))) };
            greatest = if y.abs() >= INF || greatest.abs() >= INF { INF } else { clamp(greatest.saturating_add(k.saturating_mul(y))) };
        }
        (least, greatest)
    }
}

/// `lo <= form <= hi`. The form is normal: no constant, coefficients without a common factor,
/// the first one positive. So one condition is written in one way only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cond {
    pub form: u32,
    pub lo: i128,
    pub hi: i128,
}

/// An auxiliary variable: the quotient `floor(num / div)`. It is fixed by the other variables, so
/// it adds no freedom; it lets a division stay a form.
#[derive(Clone, Copy, Debug)]
pub struct Aux {
    pub num: u32,
    pub div: i128,
}

/// The variables of one solve and everything known about them that does not change from run to run.
#[derive(Clone, Debug, Default)]
pub struct Space {
    pub forms: Forms,
    /// The declared bounds of every variable: the real ones first, then the auxiliary ones.
    pub lo: Vec<i128>,
    pub hi: Vec<i128>,
    pub real: usize,
    pub aux: Vec<Aux>,
    aux_index: HashMap<(u32, i128), u32>,
    /// What defines the auxiliary variables: true at every point.
    pub defs: Vec<Cond>,
    /// Forms already known to stay inside 64 bits over the whole box.
    safe: HashSet<u32>,
}

impl Space {
    pub fn new(bounds: &[(i64, i64)]) -> Space {
        Space {
            lo: bounds.iter().map(|b| b.0 as i128).collect(),
            hi: bounds.iter().map(|b| b.1 as i128).collect(),
            real: bounds.len(),
            ..Space::default()
        }
    }

    pub fn vars(&self) -> usize {
        self.lo.len()
    }

    pub fn range(&self, form: u32) -> (i128, i128) {
        self.forms.range(form, &|v| self.lo[v as usize], &|v| self.hi[v as usize])
    }

    /// `lo <= form(f) <= hi` in normal form. None when a number does not fit.
    pub fn cond(&mut self, f: u32, lo: i128, hi: i128) -> Option<Cond> {
        let form = self.forms.get(f).clone();
        let first = form.terms.first()?.1;
        let g = form.terms.iter().fold(0, |g, t| gcd(g, t.1));
        let unit = if first < 0 { -g } else { g };
        let terms: Vec<(u32, i128)> = form.terms.iter().map(|&(v, k)| (v, k / unit)).collect();
        // lo - c <= unit * S <= hi - c, where S is the normal form
        let (a, b) = (if lo <= -INF { None } else { Some(lo.checked_sub(form.c)?) }, if hi >= INF { None } else { Some(hi.checked_sub(form.c)?) });
        let (least, greatest) = if unit > 0 {
            (a.map(|x| ceil_div(x, g)), b.map(|x| floor_div(x, g)))
        } else {
            (b.map(|x| ceil_div(-x, g)), a.map(|x| floor_div(-x, g)))
        };
        let normal = self.forms.intern(Form { c: 0, terms });
        Some(Cond { form: normal, lo: least.unwrap_or(-INF).max(-INF), hi: greatest.unwrap_or(INF).min(INF) })
    }

    /// Does the condition hold everywhere, by the declared bounds alone?
    pub fn always(&self, c: &Cond) -> bool {
        let (least, greatest) = self.range(c.form);
        least >= c.lo && greatest <= c.hi
    }

    pub fn holds(&self, c: &Cond, point: &[i64]) -> bool {
        let v = self.forms.eval(c.form, point);
        c.lo <= v && v <= c.hi
    }

    /// Must a form be watched so that it stays a 64-bit quantity? (Once per form.)
    pub fn may_overflow(&mut self, f: u32) -> bool {
        if self.safe.contains(&f) {
            return false;
        }
        let (least, greatest) = self.range(f);
        if least > i64::MIN as i128 && greatest <= i64::MAX as i128 {
            self.safe.insert(f);
            return false;
        }
        true
    }

    /// The variable that is `floor(form(num) / div)`, made on first use: (its number, was it new).
    pub fn quotient(&mut self, num: u32, div: i128) -> Option<(u32, bool)> {
        if let Some(&v) = self.aux_index.get(&(num, div)) {
            return Some((v, false));
        }
        let (least, greatest) = self.range(num);
        let ends = [least, greatest].map(|x| if x.abs() >= INF { x * div.signum() } else { floor_div(x, div) });
        let v = self.lo.len() as u32;
        self.lo.push(ends[0].min(ends[1]).max(-INF));
        self.hi.push(ends[0].max(ends[1]).min(INF));
        self.aux.push(Aux { num, div });
        self.aux_index.insert((num, div), v);
        // the remainder num - div * q lies in 0..div-1 (or div+1..0 when div is negative)
        let q = self.forms.var(v);
        let rest = self.forms.combine(num, 1, q, -div, 0)?;
        let def = if div > 0 { self.cond(rest, 0, div - 1)? } else { self.cond(rest, div + 1, 0)? };
        self.defs.push(def);
        Some((v, true))
    }

    /// Give every auxiliary variable its value at a point of the real variables.
    pub fn complete(&self, point: &mut Vec<i64>) -> bool {
        point.truncate(self.real);
        for aux in &self.aux {
            let q = floor_div(self.forms.eval(aux.num, point), aux.div);
            match i64::try_from(q) {
                Ok(q) => point.push(q),
                Err(_) => return false,
            }
        }
        true
    }
}

/// A box of variable bounds and conditions on forms of several variables.
#[derive(Clone, Debug)]
pub struct Region {
    pub lo: Vec<i128>,
    pub hi: Vec<i128>,
    pub conds: Vec<Cond>,
}

impl Region {
    pub fn whole(space: &Space) -> Region {
        Region { lo: space.lo.clone(), hi: space.hi.clone(), conds: Vec::new() }
    }

    pub fn lo(&self, space: &Space, v: usize) -> i128 {
        self.lo.get(v).copied().unwrap_or(space.lo[v])
    }

    pub fn hi(&self, space: &Space, v: usize) -> i128 {
        self.hi.get(v).copied().unwrap_or(space.hi[v])
    }

    fn grow(&mut self, space: &Space) {
        for v in self.lo.len()..space.vars() {
            self.lo.push(space.lo[v]);
            self.hi.push(space.hi[v]);
        }
    }

    /// Narrow the region by a condition. False when it is then plainly empty.
    pub fn add(&mut self, space: &Space, c: Cond) -> bool {
        self.grow(space);
        if let Some(v) = space.forms.as_var(c.form) {
            let v = v as usize;
            self.lo[v] = self.lo[v].max(c.lo);
            self.hi[v] = self.hi[v].min(c.hi);
            return self.lo[v] <= self.hi[v];
        }
        match self.conds.iter_mut().find(|own| own.form == c.form) {
            Some(own) => {
                own.lo = own.lo.max(c.lo);
                own.hi = own.hi.min(c.hi);
                own.lo <= own.hi
            }
            None => {
                self.conds.push(c);
                c.lo <= c.hi
            }
        }
    }

    /// The least and greatest value a form can have in the region, by its box and its own condition.
    pub fn range(&self, space: &Space, form: u32) -> (i128, i128) {
        let (mut least, mut greatest) = space.forms.range(form, &|v| self.lo(space, v as usize), &|v| self.hi(space, v as usize));
        if let Some(own) = self.conds.iter().find(|own| own.form == form) {
            least = least.max(own.lo);
            greatest = greatest.min(own.hi);
        }
        (least, greatest)
    }

    /// Does the condition hold everywhere in the region (as far as intervals show)?
    pub fn implies(&self, space: &Space, c: &Cond) -> bool {
        let (least, greatest) = self.range(space, c.form);
        least >= c.lo && greatest <= c.hi
    }

    pub fn contains(&self, space: &Space, point: &[i64]) -> bool {
        (0..space.vars().min(point.len())).all(|v| self.lo(space, v) <= point[v] as i128 && point[v] as i128 <= self.hi(space, v))
            && self.conds.iter().all(|c| space.holds(c, point))
    }
}

// ---- whole points in a region -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Point(Vec<i64>),
    /// Proved: the region holds no point.
    Empty,
    /// The search gave up.
    Unknown,
}

/// Most boxes one search may open.
pub const SEARCH_STEPS: usize = 20_000;
const ROUNDS: usize = 40;
const ELIMINATION_LIMIT: usize = 3_000;

type Row = (Vec<(u32, i128)>, i128, i128);

struct Search<'a> {
    space: &'a Space,
    rows: Vec<Row>,
}

impl Search<'_> {
    /// Tighten a box by every row until nothing changes. None: the box is empty. The flag tells
    /// whether a fixed point was reached.
    fn propagate(&self, lo: &mut [i128], hi: &mut [i128]) -> Option<bool> {
        for _ in 0..ROUNDS {
            let mut changed = false;
            for (terms, least, greatest) in &self.rows {
                // the range of the whole sum
                let (mut smin, mut smax, mut open_min, mut open_max) = (0i128, 0i128, 0usize, 0usize);
                for &(v, k) in terms {
                    let (a, b) = (lo[v as usize], hi[v as usize]);
                    let (x, y) = if k > 0 { (a, b) } else { (b, a) };
                    if x.abs() >= INF {
                        open_min += 1;
                    } else {
                        smin = smin.saturating_add(k.saturating_mul(x));
                    }
                    if y.abs() >= INF {
                        open_max += 1;
                    } else {
                        smax = smax.saturating_add(k.saturating_mul(y));
                    }
                }
                if (open_min == 0 && smin > *greatest) || (open_max == 0 && smax < *least) {
                    return None;
                }
                for &(v, k) in terms {
                    let v = v as usize;
                    let (a, b) = (lo[v], hi[v]);
                    let (x, y) = if k > 0 { (a, b) } else { (b, a) };
                    // k * var <= greatest - (smin without this term); k * var >= least - (smax without it)
                    let others_min =
                        if x.abs() >= INF { (open_min == 1).then_some(smin) } else { (open_min == 0).then(|| smin.saturating_sub(k.saturating_mul(x))) };
                    let others_max =
                        if y.abs() >= INF { (open_max == 1).then_some(smax) } else { (open_max == 0).then(|| smax.saturating_sub(k.saturating_mul(y))) };
                    let upper = match others_min {
                        Some(m) if *greatest < INF && m.abs() < INF => Some(*greatest - m),
                        _ => None,
                    };
                    let lower = match others_max {
                        Some(m) if *least > -INF && m.abs() < INF => Some(*least - m),
                        _ => None,
                    };
                    let (new_lo, new_hi) = if k > 0 {
                        (lower.map(|x| ceil_div(x, k)), upper.map(|x| floor_div(x, k)))
                    } else {
                        (upper.map(|x| ceil_div(x, k)), lower.map(|x| floor_div(x, k)))
                    };
                    if let Some(n) = new_lo {
                        if n > lo[v] {
                            lo[v] = n;
                            changed = true;
                        }
                    }
                    if let Some(n) = new_hi {
                        if n < hi[v] {
                            hi[v] = n;
                            changed = true;
                        }
                    }
                    if lo[v] > hi[v] {
                        return None;
                    }
                }
            }
            if !changed {
                return Some(true);
            }
        }
        Some(false)
    }

    /// A point of the box: the real variables as chosen, the auxiliary ones as they must be.
    fn candidate(&self, lo: &[i128], hi: &[i128], least: bool) -> Option<Vec<i64>> {
        let mut point: Vec<i64> = Vec::with_capacity(lo.len());
        for v in 0..self.space.real {
            let pick = if least || lo[v] <= -INF || hi[v] >= INF { lo[v] } else { lo[v] + (hi[v] - lo[v]) / 2 };
            point.push(i64::try_from(pick).ok()?);
        }
        if !self.space.complete(&mut point) {
            return None;
        }
        let inside = (0..point.len()).all(|v| lo[v] <= point[v] as i128 && point[v] as i128 <= hi[v]);
        let holds = self.rows.iter().all(|(terms, least, greatest)| {
            let sum = terms.iter().fold(0i128, |s, (v, k)| s.saturating_add(k.saturating_mul(point[*v as usize] as i128)));
            *least <= sum && sum <= *greatest
        });
        (inside && holds).then_some(point)
    }

    /// Fourier-Motzkin elimination over the rationals, with bounds rounded to whole numbers:
    /// true only when the rows cannot all hold.
    fn refuted(&self, lo: &[i128], hi: &[i128]) -> bool {
        // every row as `sum(k * var) <= bound`
        let mut rows: Vec<(Vec<(u32, i128)>, i128)> = Vec::new();
        let push = |terms: Vec<(u32, i128)>, bound: i128, rows: &mut Vec<(Vec<(u32, i128)>, i128)>| -> bool {
            let g = terms.iter().fold(0, |g, t| gcd(g, t.1));
            if g == 0 {
                return bound < 0; // 0 <= bound
            }
            rows.push((terms.iter().map(|&(v, k)| (v, k / g)).collect(), floor_div(bound, g)));
            false
        };
        let mut used: HashSet<u32> = HashSet::new();
        for (terms, least, greatest) in &self.rows {
            used.extend(terms.iter().map(|t| t.0));
            if *greatest < INF && push(terms.clone(), *greatest, &mut rows) {
                return true;
            }
            if *least > -INF && push(terms.iter().map(|&(v, k)| (v, -k)).collect(), -*least, &mut rows) {
                return true;
            }
        }
        for &v in &used {
            if hi[v as usize] < INF {
                rows.push((vec![(v, 1)], hi[v as usize]));
            }
            if lo[v as usize] > -INF {
                rows.push((vec![(v, -1)], -lo[v as usize]));
            }
        }
        let mut vars: Vec<u32> = used.into_iter().collect();
        vars.sort_unstable();
        while !vars.is_empty() {
            // the variable whose elimination makes the fewest new rows
            let cost = |v: u32| {
                let (mut up, mut down) = (0usize, 0usize);
                for (terms, _) in &rows {
                    match terms.iter().find(|t| t.0 == v) {
                        Some(t) if t.1 > 0 => up += 1,
                        Some(_) => down += 1,
                        None => {}
                    }
                }
                up * down
            };
            let at = (0..vars.len()).min_by_key(|&i| cost(vars[i])).unwrap();
            let v = vars.swap_remove(at);
            let (mut ups, mut downs, mut rest) = (Vec::new(), Vec::new(), Vec::new());
            for row in rows {
                match row.0.iter().find(|t| t.0 == v).map(|t| t.1) {
                    Some(k) if k > 0 => ups.push((row, k)),
                    Some(k) => downs.push((row, -k)),
                    None => rest.push(row),
                }
            }
            if ups.len() * downs.len() + rest.len() > ELIMINATION_LIMIT {
                return false;
            }
            for ((up, ku), (down, kd)) in ups.iter().flat_map(|u| downs.iter().map(move |d| (u, d))) {
                // kd * up + ku * down: the variable cancels
                let mut sum: HashMap<u32, i128> = HashMap::new();
                let mut fits = true;
                for (row, k) in [(up, *kd), (down, *ku)] {
                    for &(var, coef) in &row.0 {
                        match coef.checked_mul(k).and_then(|x| sum.get(&var).copied().unwrap_or(0).checked_add(x)) {
                            Some(x) => {
                                sum.insert(var, x);
                            }
                            None => fits = false,
                        }
                    }
                }
                let bound = up.1.checked_mul(*kd).and_then(|x| down.1.checked_mul(*ku).and_then(|y| x.checked_add(y)));
                let (true, Some(bound)) = (fits, bound) else { continue };
                let mut terms: Vec<(u32, i128)> = sum.into_iter().filter(|t| t.1 != 0 && t.0 != v).collect();
                terms.sort_unstable();
                if terms.iter().any(|t| t.1.abs() >= INF) || bound.abs() >= INF {
                    continue;
                }
                let g = terms.iter().fold(0, |g, t| gcd(g, t.1));
                if g == 0 {
                    if bound < 0 {
                        return true;
                    }
                    continue;
                }
                rest.push((terms.iter().map(|&(var, k)| (var, k / g)).collect(), floor_div(bound, g)));
            }
            rows = rest;
            rows.sort();
            rows.dedup();
        }
        false
    }
}

/// The same region with every variable's bounds as narrow as the conditions make them at once
/// (None: it holds no point). Its points do not change.
pub fn tighten(space: &Space, region: &Region) -> Option<Region> {
    let n = space.vars();
    let rows: Vec<Row> = region.conds.iter().chain(&space.defs).map(|c| (space.forms.get(c.form).terms.clone(), c.lo, c.hi)).collect();
    let search = Search { space, rows };
    let (mut lo, mut hi): (Vec<i128>, Vec<i128>) = ((0..n).map(|v| region.lo(space, v)).collect(), (0..n).map(|v| region.hi(space, v)).collect());
    if (0..n).any(|v| lo[v] > hi[v]) {
        return None;
    }
    search.propagate(&mut lo, &mut hi)?;
    Some(Region { lo, hi, conds: region.conds.clone() })
}

/// A point of a region. With `least`, the first point in the order of the variables.
pub fn find(space: &Space, region: &Region, least: bool) -> Found {
    let n = space.vars();
    let mut rows: Vec<Row> = Vec::new();
    for c in region.conds.iter().chain(&space.defs) {
        let terms = space.forms.get(c.form).terms.clone();
        match rows.iter_mut().find(|row| row.0 == terms) {
            Some(row) => {
                row.1 = row.1.max(c.lo);
                row.2 = row.2.min(c.hi);
            }
            None => rows.push((terms, c.lo, c.hi)),
        }
    }
    if rows.iter().any(|row| row.1 > row.2) {
        return Found::Empty;
    }
    let search = Search { space, rows };
    let (mut lo, mut hi): (Vec<i128>, Vec<i128>) = ((0..n).map(|v| region.lo(space, v)).collect(), (0..n).map(|v| region.hi(space, v)).collect());
    if (0..n).any(|v| lo[v] > hi[v]) {
        return Found::Empty;
    }
    match search.propagate(&mut lo, &mut hi) {
        None => return Found::Empty,
        Some(false) => {
            if search.refuted(&lo, &hi) {
                return Found::Empty;
            }
        }
        Some(true) => {}
    }
    let mut stack = vec![(lo, hi)];
    let (mut steps, mut refuted_tried) = (0usize, 0usize);
    while let Some((mut lo, mut hi)) = stack.pop() {
        steps += 1;
        if steps > SEARCH_STEPS {
            return Found::Unknown;
        }
        if search.propagate(&mut lo, &mut hi).is_none() {
            continue;
        }
        if let Some(point) = search.candidate(&lo, &hi, least) {
            return Found::Point(point);
        }
        // the first real variable still open: its lower half holds the earlier points
        let Some(v) = (0..space.real).find(|&v| lo[v] < hi[v]) else { continue };
        if steps.is_power_of_two() && steps >= 64 && refuted_tried < 6 {
            refuted_tried += 1;
            if search.refuted(&lo, &hi) {
                continue;
            }
        }
        if lo[v] <= -INF || hi[v] >= INF {
            return Found::Unknown;
        }
        let mid = lo[v] + (hi[v] - lo[v]) / 2;
        let (mut upper_lo, mut lower_hi) = (lo.clone(), hi.clone());
        upper_lo[v] = mid + 1;
        lower_hi[v] = mid;
        stack.push((upper_lo, hi));
        stack.push((lo, lower_hi));
    }
    Found::Empty
}

/// The least (or greatest) value of a form over a region, and the first point that has it.
#[derive(Clone, Debug, PartialEq)]
pub enum Best {
    Value(i128, Vec<i64>),
    Empty,
    Unknown,
}

pub fn optimum(space: &mut Space, region: &Region, form: u32, constant: i64, max: bool) -> Best {
    if form == 0 {
        return match find(space, region, true) {
            Found::Point(p) => Best::Value(constant as i128, p),
            Found::Empty => Best::Empty,
            Found::Unknown => Best::Unknown,
        };
    }
    match find(space, region, false) {
        Found::Empty => return Best::Empty,
        Found::Unknown => return Best::Unknown,
        Found::Point(p) => {
            // a value that is reached, and the bound the box gives: the optimum lies between them
            let reached = space.forms.eval(form, &p);
            let (least, greatest) = region.range(space, form);
            let (mut good, mut limit) = if max { (reached, greatest) } else { (reached, least) };
            if limit.abs() >= INF {
                return Best::Unknown;
            }
            while good != limit {
                // is there a point strictly better than `good`, as far as the middle?
                let mid = if max { good + (limit - good + 1) / 2 } else { good - (good - limit + 1) / 2 };
                let Some(c) = (if max { space.cond(form, mid, INF) } else { space.cond(form, -INF, mid) }) else { return Best::Unknown };
                let mut part = region.clone();
                if !part.add(space, c) {
                    limit = if max { mid - 1 } else { mid + 1 };
                    continue;
                }
                match find(space, &part, false) {
                    Found::Point(q) => good = space.forms.eval(form, &q),
                    Found::Empty => limit = if max { mid - 1 } else { mid + 1 },
                    Found::Unknown => return Best::Unknown,
                }
            }
            let Some(c) = space.cond(form, good, good) else { return Best::Unknown };
            let mut part = region.clone();
            if !part.add(space, c) {
                return Best::Unknown;
            }
            match find(space, &part, true) {
                Found::Point(q) => Best::Value(good, q),
                _ => Best::Unknown,
            }
        }
    }
}

// ---- one recorded run ----------------------------------------------------------------------------

/// What a run of the machine carries while it records its decisions.
#[derive(Clone, Debug)]
pub struct SymCtx {
    pub space: Space,
    /// The sample: a value for every variable.
    pub point: Vec<i64>,
    /// The decisions of this run, in the order they were taken (each once).
    pub conds: Vec<Cond>,
    seen: HashSet<Cond>,
    /// Set when a recorded condition did not hold at the sample: the run cannot be trusted.
    pub trouble: Option<String>,
}

impl SymCtx {
    pub fn begin(space: Space, point: Vec<i64>) -> SymCtx {
        SymCtx { space, point, conds: Vec::new(), seen: HashSet::new(), trouble: None }
    }

    /// The form of `kx * x + ky * y`.
    pub fn lin(&mut self, x: Val, kx: i128, y: Val, ky: i128) -> Option<u32> {
        let c = (if x.f == 0 { kx.checked_mul(x.v as i128)? } else { 0 }).checked_add(if y.f == 0 { ky.checked_mul(y.v as i128)? } else { 0 })?;
        self.space.forms.combine(x.f, kx, y.f, ky, c)
    }

    /// Record `lo <= form(f) <= hi`.
    pub fn bound(&mut self, f: u32, lo: i128, hi: i128) {
        if f == 0 {
            return;
        }
        let Some(c) = self.space.cond(f, lo, hi) else {
            self.pin_all();
            return;
        };
        if !self.space.holds(&c, &self.point) {
            self.trouble.get_or_insert_with(|| format!("a recorded condition is false at the sample (form {f}, {lo}..{hi})"));
            return;
        }
        if self.space.always(&c) || !self.seen.insert(c) {
            return;
        }
        self.conds.push(c);
    }

    /// The fallback that is always right: every variable keeps its sample value.
    pub fn pin_all(&mut self) {
        for v in 0..self.space.vars() {
            let (form, at) = (self.space.forms.var(v as u32), self.point[v] as i128);
            let c = Cond { form, lo: at, hi: at };
            if !self.space.always(&c) && self.seen.insert(c) {
                self.conds.push(c);
            }
        }
    }

    /// A value must be what its form gives at the sample.
    pub fn check(&mut self, x: Val) {
        if x.f != 0 && self.space.forms.eval(x.f, &self.point) != x.v as i128 {
            self.trouble.get_or_insert_with(|| format!("a value ({}) differs from its form at the sample", x.v));
        }
    }

    /// The form of `floor(x / div)` for a constant `div`, through an auxiliary variable.
    pub fn quotient(&mut self, x: Val, div: i64) -> Option<u32> {
        let (v, new) = self.space.quotient(x.f, div as i128)?;
        if new {
            let q = floor_div(self.space.forms.eval(x.f, &self.point), div as i128);
            self.point.push(i64::try_from(q).ok()?);
        }
        Some(self.space.forms.var(v))
    }
}
