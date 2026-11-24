//! The machine: runs an image.
//!
//! Port of `fean/vm.py` (meaning: docs/financial-language.md sections 3-5; how the machine realises
//! it: docs/engine.md section 6; Map units I-09-d, I-16-c). The machine reads no names and checks no
//! types: all of that was settled when the image was compiled.
//!
//! Differences from the Python machine, none of which change a result (DR-31):
//! the program runs on a work stack instead of recursion, so a path may be as long as memory allows;
//! values are 64-bit (`D-Overflow` when a quantity does not fit); `K` is computed once per
//! information set; strategy profiles are searched with pruning and a limit (`POLICY_LIMIT`).
//!
//! For the solver the machine can also record a run (DR-53): every value then carries, beside what
//! it is at the sample point, the affine form that gives it at every other point (`Val`), and
//! every decision that depends on a form is written down as a condition on that form. Where a
//! form stops being affine (a product of two unknowns, an unknown used as a divisor or as an
//! index) the unknown is pinned to its sample value, which is always safe. When nothing is
//! recorded every form is 0 and the machine works on plain numbers as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::image::{Dom, Image, Ins, QueryI, StmtI, NONE};
use crate::sym::{SymCtx, Val, INF};
use crate::syntax::{FeanError, Kind, R};

pub const DEFAULT_BUDGET: usize = 200_000;
/// Most steps one search over strategy profiles may take before the verdict is UNKNOWN.
pub const POLICY_LIMIT: usize = 2_000_000;
/// Where the value of a quantity slot is looked for when the state at a point is solved for (DR-52).
pub const SLOT_RANGE: (i64, i64) = (-1_000_000_000, 1_000_000_000);
/// In the forms of a state: the solver put this value there, and whether it is empty has not been
/// looked at yet in this run.
pub const PENDING: u32 = 1 << 31;

const NO_CONT: u32 = u32::MAX;

/// What an actor was told: (value, as-of time, time received).
pub type Tell = (i64, i64, i64);

#[derive(Clone, Debug)]
pub struct State {
    pub t: i64,
    pub cells: Arc<Vec<i64>>,
    pub mem: Arc<Vec<Option<Tell>>>,
    /// While a run is recorded: the forms of the clock, of the cells and of the slots.
    pub ft: u32,
    pub fcells: Option<Arc<Vec<u32>>>,
    pub fmem: Option<Arc<Vec<[u32; 3]>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Blocked,
    NoSettlement,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Blocked => "BLOCKED",
            Status::NoSettlement => "NO_SETTLEMENT",
        }
    }
}

/// One step of a history.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The initial world with this number.
    Init(usize),
    End,
    Tick(i64),
    /// A unit committed.
    Commit(usize),
    /// An attempted unit was blocked: (unit, reason).
    Reject(usize, String),
    /// A mandatory step failed: (unit, status, reason).
    Fail(usize, Status, String),
    Branch(usize),
    /// A menu is shown: (actor, menu).
    Show(usize, usize),
    /// An action is picked: (actor, menu, action).
    Pick(usize, usize, usize),
    Mark(usize),
}

impl Step {
    pub fn kind(&self) -> &'static str {
        match self {
            Step::Init(_) => "init",
            Step::End => "end",
            Step::Tick(_) => "tick",
            Step::Commit(_) => "commit",
            Step::Reject(..) => "reject",
            Step::Fail(..) => "fail",
            Step::Branch(_) => "branch",
            Step::Show(..) => "show",
            Step::Pick(..) => "pick",
            Step::Mark(_) => "mark",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub event: usize,
    pub from: usize,
    pub to: usize,
    pub asset: usize,
    pub amount: i64,
}

/// A holding created (amount > 0) or destroyed (amount < 0) by `+=` / `-=`.
#[derive(Clone, Debug, PartialEq)]
pub struct Mint {
    pub event: usize,
    pub actor: usize,
    pub asset: usize,
    pub amount: i64,
}

/// What one commit did, kept for the history views (the judgement itself does not need it).
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// The events of the unit with their argument values.
    pub events: Vec<(usize, Vec<i64>)>,
    pub moves: Vec<Move>,
    pub mints: Vec<Mint>,
    /// (event, slot, value, as-of)
    pub tells: Vec<(usize, usize, i64, i64)>,
    /// (event, slot)
    pub forgets: Vec<(usize, usize)>,
}

/// An observation point: the history prefix that ends with `step`.
#[derive(Clone, Debug)]
pub struct Node {
    pub parent: Option<u32>,
    pub step: Step,
    pub state: State,
    pub children: Vec<u32>,
    pub world: u32,
    /// For a `show` step: the number of what is displayed, in `Vm::displays`.
    pub display: Option<u32>,
    /// The statement that made this step: (block, position).
    pub src: Option<(u32, u32)>,
    pub record: Option<Arc<Record>>,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        matches!(self.step, Step::End | Step::Fail(..))
    }
    pub fn failed(&self) -> Option<Status> {
        match self.step {
            Step::Fail(_, status, _) => Some(status),
            _ => None,
        }
    }
}

/// The judgement of one commit.
#[derive(Clone, Debug)]
pub enum Outcome {
    Ok(Vec<State>, Arc<Record>),
    Blocked(String),
    NoSettlement(String),
}

// ---- exact fractions ---------------------------------------------------------------------------

fn overflow() -> FeanError {
    FeanError::model("D-Overflow", "a quantity does not fit in 64 bits")
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}

/// An exact fraction (expected values). Always in lowest terms with a positive denominator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    pub n: i128,
    pub d: i128,
}

impl Ratio {
    pub fn int(n: i64) -> Ratio {
        Ratio { n: n as i128, d: 1 }
    }
    pub fn new(n: i128, d: i128) -> R<Ratio> {
        if d == 0 {
            return Err(FeanError::model("D-Weight", "division by a zero weight"));
        }
        let g = gcd(n, d).max(1);
        let sign = if d < 0 { -1 } else { 1 };
        Ok(Ratio { n: sign * n / g, d: sign * d / g })
    }
    pub fn add(self, o: Ratio) -> R<Ratio> {
        let n = self.n.checked_mul(o.d).and_then(|x| o.n.checked_mul(self.d).and_then(|y| x.checked_add(y)));
        let d = self.d.checked_mul(o.d);
        match (n, d) {
            (Some(n), Some(d)) => Ratio::new(n, d),
            _ => Err(overflow()),
        }
    }
    pub fn mul(self, o: Ratio) -> R<Ratio> {
        match (self.n.checked_mul(o.n), self.d.checked_mul(o.d)) {
            (Some(n), Some(d)) => Ratio::new(n, d),
            _ => Err(overflow()),
        }
    }
    pub fn div(self, o: Ratio) -> R<Ratio> {
        match (self.n.checked_mul(o.d), self.d.checked_mul(o.n)) {
            (Some(n), Some(d)) => Ratio::new(n, d),
            _ => Err(overflow()),
        }
    }
    pub fn gt(self, o: Ratio) -> R<bool> {
        match (self.n.checked_mul(o.d), o.n.checked_mul(self.d)) {
            (Some(a), Some(b)) => Ok(a > b),
            _ => Err(overflow()),
        }
    }
    pub fn is_zero(self) -> bool {
        self.n == 0
    }
    /// Written like Python's `str(Fraction)`: `3` or `1/2`.
    pub fn text(self) -> String {
        if self.d == 1 {
            self.n.to_string()
        } else {
            format!("{}/{}", self.n, self.d)
        }
    }
    pub fn to_f64(self) -> f64 {
        self.n as f64 / self.d as f64
    }
}

/// The value of a query for one assignment of the public parameters.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Bool(bool),
    Num(Ratio),
}

// ---- running code ------------------------------------------------------------------------------

enum Halt {
    Blocked(usize),
    Error(FeanError),
}

impl From<FeanError> for Halt {
    fn from(e: FeanError) -> Halt {
        Halt::Error(e)
    }
}

fn internal(what: &str) -> FeanError {
    FeanError::image("I-Mode", format!("the machine met an image it cannot run ({what})"))
}

fn limit(what: String) -> FeanError {
    FeanError::new(Kind::Limit, "limit", what, None)
}

fn need(s: Option<&State>) -> Result<&State, Halt> {
    s.ok_or_else(|| Halt::Error(internal("a state is needed")))
}

/// Is a machine value true? (The empty value is not.)
#[inline]
pub fn truthy(v: i64) -> bool {
    v != 0 && v != NONE
}

#[inline]
fn num(x: Val) -> Result<i64, Halt> {
    if x.v == NONE {
        Err(Halt::Error(internal("an empty value in arithmetic")))
    } else {
        Ok(x.v)
    }
}

fn floor_div(x: i64, y: i64) -> Option<i64> {
    let q = x.checked_div(y)?;
    Some(if x % y != 0 && ((x % y < 0) != (y < 0)) { q - 1 } else { q })
}

const ZERO: Val = Val { v: 0, f: 0 };
const EMPTY: Val = Val { v: NONE, f: 0 };

/// What one commit wants to change, gathered before anything is applied.
#[derive(Default)]
struct Effects {
    sets: Vec<(usize, Val)>,
    deltas: Vec<(usize, Val)>,
    frees: Vec<usize>,
    tells: Vec<(usize, (Val, Val, Val))>,
    forgets: Vec<usize>,
    inflow: Vec<(usize, Val)>,
    outflow: Vec<(usize, Val)>,
    event: usize,
    record: Record,
}

fn lookup(table: &[(usize, Val)], key: usize) -> Val {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(ZERO)
}

/// A score to compare: a fraction, and (for a plain best or worst of whole quantities) its form.
#[derive(Clone, Copy, Debug)]
struct Scored {
    r: Ratio,
    f: u32,
}

impl Scored {
    fn val(self) -> Val {
        Val { v: self.r.n as i64, f: self.f }
    }
}

/// The values a cell may take: `lo..=hi`, preceded by the empty value when optional.
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    pub lo: i64,
    pub hi: i64,
    pub opt: bool,
}

impl Domain {
    pub fn contains(&self, v: i64) -> bool {
        if v == NONE {
            self.opt
        } else {
            self.lo <= v && v <= self.hi
        }
    }
    pub fn len(&self) -> u128 {
        let span = if self.hi >= self.lo { (self.hi as i128 - self.lo as i128 + 1) as u128 } else { 0 };
        span + self.opt as u128
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn get(&self, i: u128) -> i64 {
        if self.opt {
            if i == 0 {
                NONE
            } else {
                (self.lo as i128 + i as i128 - 1) as i64
            }
        } else {
            (self.lo as i128 + i as i128) as i64
        }
    }
}

/// The nodes an actor cannot tell apart, for every node.
#[derive(Debug)]
pub struct Classes {
    /// node -> class number
    pub of: Vec<u32>,
    /// class number -> its nodes, in history order
    pub members: Vec<Vec<u32>>,
}

/// One information set of an actor: the points where a menu is shown that the actor cannot tell apart.
#[derive(Clone, Debug)]
pub struct InfoSet {
    pub actor: usize,
    /// The class of the actor's view at the menu.
    pub class: u32,
    pub menu: usize,
    pub actions: Vec<usize>,
    /// The first point of the set, in history order.
    pub node: u32,
}

/// The information sets of a coalition and what each complete history picked in them.
pub struct Game {
    pub sets: Vec<InfoSet>,
    /// per leaf (in the order of `Vm::leaves`): (information set, action picked)
    pub picks: Vec<Vec<(u32, usize)>>,
}

enum Work {
    Block { b: u32, i: u32, state: State, node: u32, k: u32 },
    Commits { cands: Vec<State>, record: Arc<Record>, next: usize, unit: u32, then: Option<u32>, b: u32, i: u32, node: u32, k: u32, rest: u32 },
    Alt { b: u32, i: u32, next: usize, state: State, node: u32, rest: u32 },
    Picks { b: u32, i: u32, next: usize, state: State, shown: u32, rest: u32 },
}

/// Where an instance begins when it does not begin at the start of the program: at a mark, in the
/// state the coordinates give (clock, cells, holdings, slots: see `Vm::seat`). That state is then
/// the one initial world, and what runs is the program that remains after the mark (the rest of
/// its block, then of the blocks around it).
#[derive(Clone, Debug)]
pub struct Start {
    pub mark: usize,
    pub coords: Vec<i64>,
}

/// One instance of an image: every public parameter and every unknown has a value.
pub struct Vm {
    pub im: Arc<Image>,
    pub pv: Vec<i64>,
    /// The values of the unknowns (numbers; see `UnknownInfo`).
    pub uv: Vec<Val>,
    /// The mark this instance begins at, when it does not begin at the start of the program.
    pub begun: Option<usize>,
    pub budget: usize,
    n_actors: usize,
    h0: usize,
    ranges: Vec<Option<(Val, Val)>>,
    pub domains: Vec<Domain>,
    dom_forms: Vec<(u32, u32)>,
    floor: Vec<Val>,
    pub worlds: Vec<State>,
    /// Every observation point, in history order (depth first); complete histories are `leaves`.
    pub nodes: Vec<Node>,
    pub leaves: Vec<u32>,
    pub roots: Vec<u32>,
    /// What a `show` step displays: (actor, menu, actions).
    pub displays: Vec<(usize, usize, Vec<usize>)>,
    display_ix: HashMap<(usize, usize, Vec<usize>), u32>,
    conts: Vec<(u32, u32, u32)>,
    explored: Option<Result<(), FeanError>>,
    subtree_end: Vec<u32>,
    classes: RefCell<Vec<Option<Arc<Classes>>>>,
    kmemo: RefCell<HashMap<(u32, u32, u32), bool>>,
    points: RefCell<HashMap<u32, HashMap<(u8, u32), u32>>>,
    probs: RefCell<HashMap<usize, Arc<Vec<Ratio>>>>,
    /// The verdicts of queries read by a goal, by query number (each is worked out once).
    verdicts: RefCell<HashMap<usize, (Answer, u32)>>,
    /// Is this run recorded?
    rec: bool,
    sym: RefCell<Option<Box<SymCtx>>>,
}

macro_rules! pop {
    ($stack:expr) => {
        match $stack.pop() {
            Some(v) => v,
            None => return Err(Halt::Error(internal("an empty stack"))),
        }
    };
}

impl Vm {
    pub fn new(image: Arc<Image>, pvals: Vec<i64>, budget: usize) -> R<Vm> {
        Vm::open(image, pvals, Vec::new(), None, budget)
    }

    /// An instance with values for the unknowns, optionally begun at a mark.
    pub fn open(image: Arc<Image>, pvals: Vec<i64>, uvals: Vec<i64>, start: Option<Start>, budget: usize) -> R<Vm> {
        let uvals = uvals.into_iter().map(Val::plain).collect();
        let mut vm = Vm::bare(image, pvals, uvals, start.as_ref().map(|s| s.mark), budget, None)?;
        vm.instantiate()?;
        if let Some(start) = start {
            let coords: Vec<Val> = start.coords.into_iter().map(Val::plain).collect();
            if !vm.seat(&coords)? {
                return Err(FeanError::image("I-Ref", "the state to begin in lies outside the declared ranges"));
            }
        }
        Ok(vm)
    }

    /// The same without ranges and initial worlds: only constant code can run until `instantiate`.
    /// With `sym` the run is recorded.
    pub fn bare(image: Arc<Image>, pvals: Vec<i64>, uvals: Vec<Val>, begun: Option<usize>, budget: usize, sym: Option<SymCtx>) -> R<Vm> {
        let (n_actors, h0) = (image.actors.len(), image.cells.len());
        let vm = Vm {
            pv: pvals,
            uv: uvals,
            begun,
            budget,
            n_actors,
            h0,
            ranges: vec![None; image.cell_count()],
            domains: Vec::new(),
            dom_forms: Vec::new(),
            floor: Vec::new(),
            worlds: Vec::new(),
            nodes: Vec::new(),
            leaves: Vec::new(),
            roots: Vec::new(),
            displays: Vec::new(),
            display_ix: HashMap::new(),
            conts: Vec::new(),
            explored: None,
            subtree_end: Vec::new(),
            classes: RefCell::new(vec![None; n_actors]),
            kmemo: RefCell::new(HashMap::new()),
            points: RefCell::new(HashMap::new()),
            probs: RefCell::new(HashMap::new()),
            verdicts: RefCell::new(HashMap::new()),
            rec: sym.is_some(),
            sym: RefCell::new(sym.map(Box::new)),
            im: image,
        };
        if vm.pv.len() != vm.im.params.len() {
            return Err(internal("the number of parameter values"));
        }
        if vm.uv.len() != vm.im.unknowns.len() {
            return Err(internal("the number of values for the unknowns"));
        }
        if matches!(vm.begun, Some(mark) if mark >= vm.im.marks.len()) {
            return Err(internal("the mark to begin at"));
        }
        Ok(vm)
    }

    /// What the run recorded (the run must not be used afterwards).
    pub fn take_sym(&mut self) -> Option<SymCtx> {
        self.sym.borrow_mut().take().map(|b| *b)
    }

    // ---- recording: decisions that depend on a form ---------------------------------------------
    #[inline]
    fn with<T>(&self, f: impl FnOnce(&mut SymCtx) -> T) -> T {
        let mut guard = self.sym.borrow_mut();
        f(guard.as_mut().expect("a recorded run"))
    }

    /// x <= y.
    fn le(&self, x: Val, y: Val) -> bool {
        let holds = x.v <= y.v;
        if (x.f | y.f) != 0 {
            self.with(|s| match s.lin(x, 1, y, -1) {
                Some(d) if holds => s.bound(d, -INF, 0),
                Some(d) => s.bound(d, 1, INF),
                None => s.pin_all(),
            });
        }
        holds
    }

    /// x < y.
    fn lt(&self, x: Val, y: Val) -> bool {
        let holds = x.v < y.v;
        if (x.f | y.f) != 0 {
            self.with(|s| match s.lin(x, 1, y, -1) {
                Some(d) if holds => s.bound(d, -INF, -1),
                Some(d) => s.bound(d, 0, INF),
                None => s.pin_all(),
            });
        }
        holds
    }

    /// x == y, for two numbers. When they differ, which one is less is part of the decision.
    fn same(&self, x: Val, y: Val) -> bool {
        if (x.f | y.f) != 0 {
            self.with(|s| match s.lin(x, 1, y, -1) {
                Some(d) if x.v == y.v => s.bound(d, 0, 0),
                Some(d) if x.v < y.v => s.bound(d, -INF, -1),
                Some(d) => s.bound(d, 1, INF),
                None => s.pin_all(),
            });
        }
        x.v == y.v
    }

    /// Is the value true?
    fn truth(&self, x: Val) -> bool {
        if x.f != 0 {
            self.with(|s| {
                if x.v == 0 {
                    s.bound(x.f, 0, 0)
                } else if x.v > 0 {
                    s.bound(x.f, 1, INF)
                } else {
                    s.bound(x.f, -INF, -1)
                }
            });
        }
        truthy(x.v)
    }

    /// The value is used where no form can follow: it keeps its sample value.
    fn pin(&self, x: Val) -> i64 {
        if x.f != 0 {
            self.with(|s| s.bound(x.f, x.v as i128, x.v as i128));
        }
        x.v
    }

    /// The truth of a goal's value, as a decision of the run (for the solver).
    pub fn decide(&self, x: Val) -> bool {
        if self.rec {
            self.with(|s| s.check(x));
        }
        self.truth(x)
    }

    /// The value `v` with the form of `kx * x + ky * y`, watched so that it stays a 64-bit quantity.
    fn formed(&self, v: i64, x: Val, kx: i128, y: Val, ky: i128) -> Val {
        if (x.f | y.f) == 0 {
            return Val::plain(v);
        }
        self.with(|s| match s.lin(x, kx, y, ky) {
            Some(f) => {
                if f != 0 && s.space.may_overflow(f) {
                    s.bound(f, i64::MIN as i128 + 1, i64::MAX as i128);
                }
                Val { v, f }
            }
            None => {
                s.pin_all();
                Val::plain(v)
            }
        })
    }

    /// An overflow at the sample is the overflow of these very values.
    fn spill(&self, x: Val, y: Val) -> Halt {
        self.pin(x);
        self.pin(y);
        Halt::Error(overflow())
    }

    fn add(&self, x: Val, y: Val) -> Result<Val, Halt> {
        match num(x)?.checked_add(num(y)?) {
            Some(v) if v != NONE => Ok(self.formed(v, x, 1, y, 1)),
            _ => Err(self.spill(x, y)),
        }
    }

    fn sub(&self, x: Val, y: Val) -> Result<Val, Halt> {
        match num(x)?.checked_sub(num(y)?) {
            Some(v) if v != NONE => Ok(self.formed(v, x, 1, y, -1)),
            _ => Err(self.spill(x, y)),
        }
    }

    fn neg(&self, x: Val) -> Result<Val, Halt> {
        match num(x)?.checked_neg() {
            Some(v) if v != NONE => Ok(self.formed(v, x, -1, ZERO, 0)),
            _ => Err(self.spill(x, ZERO)),
        }
    }

    fn mul(&self, x: Val, y: Val) -> Result<Val, Halt> {
        let (a, b) = (num(x)?, num(y)?);
        let (mut x, mut y) = (x, y);
        if x.f != 0 && y.f != 0 {
            // a product of two forms is no form: the one with fewer values keeps its sample value
            let narrow_x = self.with(|s| {
                let (xr, yr) = (s.space.range(x.f), s.space.range(y.f));
                xr.1.saturating_sub(xr.0) < yr.1.saturating_sub(yr.0)
            });
            if narrow_x {
                x = Val::plain(self.pin(x));
            } else {
                y = Val::plain(self.pin(y));
            }
        }
        match a.checked_mul(b) {
            Some(v) if v != NONE => Ok(if x.f != 0 { self.formed(v, x, b as i128, ZERO, 0) } else { self.formed(v, y, a as i128, ZERO, 0) }),
            _ => Err(self.spill(x, y)),
        }
    }

    /// `x / y`, rounded down (0), rounded up (1) or only when it is whole (2). Empty when y is 0.
    fn div(&self, x: Val, y: Val, kind: u8) -> Result<Val, Halt> {
        let (a, b) = (num(x)?, num(y)?);
        self.pin(y); // a divisor is no part of a form
        if b == 0 {
            return Ok(EMPTY);
        }
        match kind {
            0 => {
                let Some(q) = floor_div(a, b).filter(|q| *q != NONE) else { return Err(self.spill(x, ZERO)) };
                Ok(self.quotient(x, b, q))
            }
            1 => {
                let minus = self.neg(x)?;
                let Some(q) = floor_div(minus.v, b).filter(|q| *q != NONE) else { return Err(self.spill(x, ZERO)) };
                self.neg(self.quotient(minus, b, q))
            }
            _ => {
                let whole = a % b == 0;
                let Some(q) = floor_div(a, b).filter(|q| *q != NONE) else { return Err(self.spill(x, ZERO)) };
                let quot = self.quotient(x, b, q);
                if x.f != 0 {
                    // the remainder x - b * q decides: 0, or strictly between 0 and b
                    self.with(|s| match s.lin(x, 1, quot, -(b as i128)) {
                        Some(rest) if whole => s.bound(rest, 0, 0),
                        Some(rest) if b > 0 => s.bound(rest, 1, b as i128 - 1),
                        Some(rest) => s.bound(rest, b as i128 + 1, -1),
                        None => s.pin_all(),
                    });
                }
                Ok(if whole { quot } else { EMPTY })
            }
        }
    }

    /// `floor(x / b)` as a value: `q` at the sample, and an auxiliary variable as its form.
    fn quotient(&self, x: Val, b: i64, q: i64) -> Val {
        if x.f == 0 {
            return Val::plain(q);
        }
        self.with(|s| match s.quotient(x, b) {
            Some(f) => Val { v: q, f },
            None => {
                s.pin_all();
                Val::plain(q)
            }
        })
    }

    // ---- reading a state --------------------------------------------------------------------------
    #[inline]
    fn now(s: &State) -> Val {
        Val { v: s.t, f: s.ft }
    }

    #[inline]
    fn cell(&self, s: &State, c: usize) -> Val {
        let v = s.cells[c];
        match &s.fcells {
            None => Val::plain(v),
            Some(forms) => {
                let f = forms[c];
                if f & PENDING == 0 {
                    Val { v, f }
                } else {
                    self.arrive(c, v, f & !PENDING)
                }
            }
        }
    }

    /// The least value of a cell (the bound below it stands for "empty" in a solved state).
    fn least(&self, c: usize) -> Val {
        match self.ranges[c] {
            Some((lo, _)) => lo,
            None => Val::plain(self.domains[c].lo),
        }
    }

    /// A cell the solver put in the starting state is looked at: whether it is empty is a decision.
    fn arrive(&self, c: usize, v: i64, f: u32) -> Val {
        let lo = self.least(c);
        let coordinate = Val { v: if v == NONE { lo.v.wrapping_sub(1) } else { v }, f };
        self.with(|s| match s.lin(coordinate, 1, lo, -1) {
            Some(d) if v == NONE => s.bound(d, -INF, -1),
            Some(d) => s.bound(d, 0, INF),
            None => s.pin_all(),
        });
        if v == NONE {
            EMPTY
        } else {
            Val { v, f }
        }
    }

    /// A slot: (value, as-of time, time received), or nothing.
    fn slot(&self, s: &State, k: usize) -> Option<(Val, Val, Val)> {
        let Some(forms) = &s.fmem else {
            return s.mem[k].map(|(v, a, r)| (Val::plain(v), Val::plain(a), Val::plain(r)));
        };
        let [fv, fa, fr] = forms[k];
        if fv & PENDING == 0 {
            return s.mem[k].map(|(v, a, r)| (Val { v, f: fv }, Val { v: a, f: fa }, Val { v: r, f: fr }));
        }
        // put there by the solver: whether there is a slot at all is a decision
        let (f, present) = (fv & !PENDING, s.mem[k].is_some());
        self.with(|c| match c.space.forms.as_var(f).map(|var| c.space.lo[var as usize]) {
            Some(none) if present => c.bound(f, none + 1, INF),
            Some(none) => c.bound(f, -INF, none),
            None => c.pin_all(),
        });
        s.mem[k].map(|(v, a, r)| (Val { v, f }, Val { v: a, f: fa }, Val { v: r, f: fr }))
    }

    // ---- instantiation: ranges, domains, initial worlds ----------------------------------------
    /// Work out the ranges and the initial worlds. Begun at a mark, the declared initial values
    /// play no part, and an empty range is not an error (it only has no value).
    pub fn instantiate(&mut self) -> R<()> {
        let im = self.im.clone();
        let (t0, t1) = im.time;
        let begun = self.begun.is_some();
        let range_err = |msg: String| FeanError::new(Kind::Static, "W-Range", msg, None);
        let shown = |v: i64| if v == NONE { "empty".to_string() } else { v.to_string() };
        let mut choices: Vec<Vec<Val>> = Vec::new();
        for (i, c) in im.cells.iter().enumerate() {
            let (lo, hi) = match c.dom {
                Dom::Qty => {
                    let (lo, hi) = (self.constant_val(c.lo.unwrap_or(0))?, self.constant_val(c.hi.unwrap_or(0))?);
                    if !begun && self.lt(hi, lo) {
                        return Err(range_err(format!("cell {} has an empty range", c.name)));
                    }
                    self.ranges[i] = Some((lo, hi));
                    (lo, hi)
                }
                Dom::Bool => (ZERO, Val::plain(1)),
                Dom::Enum(e) => (ZERO, Val::plain(im.enums[e].1.len() as i64 - 1)),
                Dom::Time => (Val::plain(t0), Val::plain(t1)),
                Dom::Actor => (ZERO, Val::plain(self.n_actors as i64 - 1)),
            };
            self.domains.push(Domain { lo: lo.v, hi: hi.v, opt: c.opt });
            self.dom_forms.push((lo.f, hi.f));
            if begun {
                continue;
            }
            let mut vals = Vec::new();
            for &v in &c.init {
                vals.push(self.constant_val(v)?);
            }
            for &v in &vals {
                let inside = if v.v == NONE { c.opt } else { self.le(lo, v) && self.le(v, hi) };
                if !inside {
                    return Err(range_err(format!("initial value {} of {} is outside its range", shown(v.v), c.name)));
                }
            }
            choices.push(vals);
        }
        for (x, a) in im.assets.iter().enumerate() {
            let (lo, hi) = (self.constant_val(a.lo)?, self.constant_val(a.hi)?);
            if !begun && self.lt(hi, lo) {
                return Err(range_err(format!("asset {} has an empty range", a.name)));
            }
            self.floor.push(lo);
            for i in 0..self.n_actors {
                self.ranges[self.h0 + x * self.n_actors + i] = Some((lo, hi));
                if begun {
                    continue;
                }
                let mut vals = Vec::new();
                for &v in &a.init[i] {
                    vals.push(self.constant_val(v)?);
                }
                for &v in &vals {
                    if v.v == NONE || !(self.le(lo, v) && self.le(v, hi)) {
                        return Err(range_err(format!(
                            "initial holding {} of {} is outside the range of {}",
                            shown(v.v),
                            im.actors[i],
                            a.name
                        )));
                    }
                }
                choices.push(vals);
            }
        }
        if begun {
            return Ok(());
        }
        let empty: Arc<Vec<Option<Tell>>> = Arc::new(vec![None; im.slots.len()]);
        let no_forms: Option<Arc<Vec<[u32; 3]>>> = self.rec.then(|| Arc::new(vec![[0; 3]; im.slots.len()]));
        let mut pick = vec![0usize; choices.len()];
        if choices.iter().all(|c| !c.is_empty()) {
            'worlds: loop {
                let cells: Vec<i64> = pick.iter().zip(&choices).map(|(i, vals)| vals[*i].v).collect();
                let forms: Option<Arc<Vec<u32>>> =
                    self.rec.then(|| Arc::new(pick.iter().zip(&choices).map(|(i, vals)| vals[*i].f).collect()));
                let st = State { t: t0, cells: Arc::new(cells), mem: empty.clone(), ft: 0, fcells: forms, fmem: no_forms.clone() };
                let mut assumed = true;
                for &a in &im.assumes {
                    if !self.truth(self.in_state_val(a, &st)?) {
                        assumed = false;
                        break;
                    }
                }
                if assumed {
                    for &inv in &im.invariants {
                        if !self.truth(self.in_state_val(inv, &st)?) {
                            return Err(FeanError::model("D-Init", "an initial world breaks an invariant"));
                        }
                    }
                    self.worlds.push(st);
                }
                // the next combination: the last choice turns fastest
                let mut at = choices.len();
                loop {
                    if at == 0 {
                        break 'worlds;
                    }
                    at -= 1;
                    pick[at] += 1;
                    if pick[at] < choices[at].len() {
                        break;
                    }
                    pick[at] = 0;
                }
            }
        }
        if self.worlds.is_empty() {
            return Err(FeanError::model("D-Init", "there is no initial world"));
        }
        Ok(())
    }

    /// The whole numbers a slot's value can be (below the least one: no slot).
    pub fn slot_range(im: &Image, dom: Dom) -> (i64, i64) {
        match dom {
            Dom::Qty => SLOT_RANGE,
            Dom::Bool => (0, 1),
            Dom::Enum(e) => (0, im.enums[e].1.len() as i64 - 1),
            Dom::Time => im.time,
            Dom::Actor => (0, im.actors.len() as i64 - 1),
        }
    }

    /// The range of a cell as worked out by `instantiate`: (least, greatest, may be empty).
    pub fn cell_range(&self, c: usize) -> (i64, i64, bool) {
        let opt = c < self.h0 && self.im.cells[c].opt;
        match self.ranges[c] {
            Some((lo, hi)) => (lo.v, hi.v, opt),
            None => (self.domains[c].lo, self.domains[c].hi, opt),
        }
    }

    /// Put an instance begun at a mark into the state the coordinates give: the clock, every cell,
    /// every holding (asset by asset, actor by actor), and for every slot its value, as-of time and
    /// time received. One below the least value of an optional cell or of a slot's value means
    /// empty, or no slot (DR-56). False when the coordinates are no state: a value outside its
    /// range, or a second spelling of "no slot".
    pub fn seat(&mut self, coords: &[Val]) -> R<bool> {
        let im = self.im.clone();
        let n = im.cell_count();
        if self.begun.is_none() || self.domains.len() != im.cells.len() || coords.len() != 1 + n + 3 * im.slots.len() {
            return Err(internal("the coordinates of a state"));
        }
        let (t0, t1) = im.time;
        let now = coords[0];
        if now.v < t0 || now.v > t1 {
            return Ok(false);
        }
        let (mut cells, mut forms) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for c in 0..n {
            let x = coords[1 + c];
            let opt = c < self.h0 && im.cells[c].opt;
            let (lo, hi) = match self.ranges[c] {
                Some(range) => range,
                None => (Val::plain(self.domains[c].lo), Val::plain(self.domains[c].hi)),
            };
            let least = if opt { self.plain_val(self.sub(lo, Val::plain(1)))? } else { lo };
            if !(self.le(least, x) && self.le(x, hi)) {
                return Ok(false);
            }
            if opt {
                cells.push(if x.v == lo.v.wrapping_sub(1) { NONE } else { x.v });
                forms.push(if x.f != 0 { x.f | PENDING } else { 0 });
            } else {
                cells.push(x.v);
                forms.push(x.f);
            }
        }
        let (mut mem, mut fmem) = (Vec::with_capacity(im.slots.len()), Vec::with_capacity(im.slots.len()));
        for (k, slot) in im.slots.iter().enumerate() {
            let (x, asof, recv) = (coords[1 + n + 3 * k], coords[2 + n + 3 * k], coords[3 + n + 3 * k]);
            let (lo, hi) = Vm::slot_range(&im, slot.dom);
            if x.v < lo - 1 || x.v > hi || asof.v < t0 || asof.v > t1 || recv.v < t0 || recv.v > t1 {
                return Ok(false);
            }
            let pending = if x.f != 0 { x.f | PENDING } else { 0 };
            if x.v == lo - 1 {
                if x.f == 0 && (asof.v, recv.v) != (t0, t0) {
                    return Ok(false);
                }
                mem.push(None);
                fmem.push([pending, 0, 0]);
            } else {
                mem.push(Some((x.v, asof.v, recv.v)));
                fmem.push([pending, asof.f, recv.f]);
            }
        }
        self.worlds = vec![State {
            t: now.v,
            cells: Arc::new(cells),
            mem: Arc::new(mem),
            ft: now.f,
            fcells: self.rec.then(|| Arc::new(forms)),
            fmem: self.rec.then(|| Arc::new(fmem)),
        }];
        Ok(true)
    }

    /// Is this a candidate at all? Begun at a mark: the state keeps the invariants and passes the
    /// filter. Otherwise: the unknowns pass the filter (the model is not set up yet).
    pub fn admits(&self, filter: Option<usize>) -> R<bool> {
        if self.begun.is_none() {
            return match filter {
                Some(code) => Ok(self.truth(self.constant_val(code)?)),
                None => Ok(true),
            };
        }
        let state = self.worlds.first().cloned().ok_or_else(|| internal("no state to begin in"))?;
        for &inv in &self.im.invariants {
            if !self.truth(self.in_state_val(inv, &state)?) {
                return Ok(false);
            }
        }
        match filter {
            Some(code) => Ok(self.truth(self.in_state_val(code, &state)?)),
            None => Ok(true),
        }
    }

    // ---- code ----------------------------------------------------------------------------------
    /// Execute code block `cid`; return the value it leaves (0 for guard and effect blocks).
    #[allow(clippy::too_many_arguments)]
    fn exec(
        &self,
        cid: usize,
        s: Option<&State>,
        frame: Option<&mut [Val]>,
        post: Option<&State>,
        node: Option<u32>,
        leaf: Option<u32>,
        mut fx: Option<&mut Effects>,
    ) -> Result<Val, Halt> {
        let code = &self.im.codes[cid];
        let mut own: Vec<Val>;
        let frame: &mut [Val] = match frame {
            Some(f) => f,
            None => {
                own = vec![EMPTY; code.locals];
                &mut own
            }
        };
        let ins = &code.ins;
        let mut stack: Vec<Val> = Vec::with_capacity(8);
        let (mut pc, end) = (0usize, ins.len());
        while pc < end {
            let instr = &ins[pc];
            pc += 1;
            match instr {
                Ins::Push(v) => stack.push(Val::plain(*v)),
                Ins::Load(c) => stack.push(self.cell(need(s)?, *c)),
                Ins::Local(i) => stack.push(frame[*i]),
                Ins::Store(i) => frame[*i] = pop!(stack),
                Ins::Param(i) => stack.push(Val::plain(self.pv[*i])),
                Ins::Unk(i) => stack.push(self.uv[*i]),
                Ins::Query(i) => stack.push(self.verdict(*i)?),
                Ins::Jf(t) => {
                    if !self.truth(pop!(stack)) {
                        pc = *t;
                    }
                }
                Ins::Jt(t) => {
                    if self.truth(pop!(stack)) {
                        pc = *t;
                    }
                }
                Ins::Jmp(t) => pc = *t,
                Ins::Eq | Ins::Ne => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    // an empty value was settled when it was read: only two numbers need a decision
                    let same = if (x.f | y.f) == 0 || x.v == NONE || y.v == NONE { x.v == y.v } else { self.same(x, y) };
                    stack.push(Val::plain((same == matches!(instr, Ins::Eq)) as i64));
                }
                Ins::Lt | Ins::Le | Ins::Gt | Ins::Ge => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    num(x)?;
                    num(y)?;
                    let r = match instr {
                        Ins::Lt => self.lt(x, y),
                        Ins::Le => self.le(x, y),
                        Ins::Gt => self.lt(y, x),
                        _ => self.le(y, x),
                    };
                    stack.push(Val::plain(r as i64));
                }
                Ins::Add => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.add(x, y)?);
                }
                Ins::Sub => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.sub(x, y)?);
                }
                Ins::Mul => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.mul(x, y)?);
                }
                Ins::Min | Ins::Max => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    num(x)?;
                    num(y)?;
                    let x_first = self.le(x, y);
                    stack.push(if x_first == matches!(instr, Ins::Min) { x } else { y });
                }
                Ins::DivF => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.div(x, y, 0)?);
                }
                Ins::DivC => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.div(x, y, 1)?);
                }
                Ins::DivX => {
                    let y = pop!(stack);
                    let x = pop!(stack);
                    stack.push(self.div(x, y, 2)?);
                }
                Ins::Not => {
                    let x = pop!(stack);
                    stack.push(Val::plain(!self.truth(x) as i64));
                }
                Ins::Neg => {
                    let x = pop!(stack);
                    stack.push(self.neg(x)?);
                }
                Ins::Known => {
                    let x = pop!(stack);
                    stack.push(Val::plain((x.v != NONE) as i64));
                }
                Ins::Dup => match stack.last() {
                    Some(&v) => stack.push(v),
                    None => return Err(Halt::Error(internal("an empty stack"))),
                },
                Ins::Pop => {
                    pop!(stack);
                }
                Ins::Bal(x) => {
                    let who = self.actor(pop!(stack))?;
                    stack.push(self.cell(need(s)?, self.h0 + x * self.n_actors + who));
                }
                Ins::Now => stack.push(Vm::now(need(s)?)),
                Ins::Mem(k) => stack.push(self.slot(need(s)?, *k).map(|slot| slot.0).unwrap_or(EMPTY)),
                Ins::Asof(k) => stack.push(self.slot(need(s)?, *k).map(|slot| slot.1).unwrap_or(EMPTY)),
                Ins::Recv(k) => stack.push(self.slot(need(s)?, *k).map(|slot| slot.2).unwrap_or(EMPTY)),
                Ins::Post(c) => stack.push(self.cell(need(post)?, *c)),
                Ins::K(who, sub) => {
                    let at = node.ok_or_else(|| Halt::Error(internal("K needs a point")))?;
                    stack.push(Val::plain(self.knows(at, *who, *sub)? as i64));
                }
                Ins::At(kind, ident, sub, flag) => {
                    let of = leaf.ok_or_else(|| Halt::Error(internal("at needs a history")))?;
                    stack.push(match self.point(of, *kind, *ident) {
                        None => {
                            if *flag {
                                ZERO
                            } else {
                                EMPTY
                            }
                        }
                        Some(p) => self.at_point_val(*sub, p)?,
                    });
                }
                Ins::Reach(kind, ident) => {
                    let of = leaf.ok_or_else(|| Halt::Error(internal("reach needs a history")))?;
                    stack.push(Val::plain(self.point(of, *kind, *ident).is_some() as i64));
                }
                Ins::Require(reason) => {
                    if !self.truth(pop!(stack)) {
                        return Err(Halt::Blocked(*reason));
                    }
                }
                Ins::Given(local, reason) => {
                    let v = pop!(stack);
                    if v.v == NONE {
                        return Err(Halt::Blocked(*reason));
                    }
                    frame[*local] = v;
                }
                Ins::FxSet(c) => {
                    let v = pop!(stack);
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    match fx.sets.iter_mut().find(|(k, _)| k == c) {
                        Some(entry) => entry.1 = v,
                        None => fx.sets.push((*c, v)),
                    }
                }
                Ins::FxAdd(c) => {
                    let dlt = pop!(stack);
                    num(dlt)?;
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    self.bump(&mut fx.deltas, *c, dlt)?;
                    if *c >= self.h0 {
                        // a holding: creation or destruction
                        let grows = self.lt(ZERO, dlt);
                        let amount = if grows { dlt } else { self.neg(dlt)? };
                        self.bump(if grows { &mut fx.inflow } else { &mut fx.outflow }, *c, amount)?;
                        let (x, who) = ((*c - self.h0) / self.n_actors, (*c - self.h0) % self.n_actors);
                        fx.record.mints.push(Mint { event: fx.event, actor: who, asset: x, amount: dlt.v });
                    }
                }
                Ins::FxFree(c) => {
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    fx.frees.push(*c);
                }
                Ins::FxMove(x) => {
                    let n = pop!(stack);
                    num(n)?;
                    let to = self.actor(pop!(stack))?;
                    let from = self.actor(pop!(stack))?;
                    let (dst, src) = (self.h0 + x * self.n_actors + to, self.h0 + x * self.n_actors + from);
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    if self.lt(n, ZERO) {
                        return Err(Halt::Error(FeanError::model(
                            "D-Move",
                            format!("{} moves a negative amount {}", self.im.events[fx.event].name, n.v),
                        )));
                    }
                    let back = self.neg(n)?;
                    self.bump(&mut fx.deltas, src, back)?;
                    self.bump(&mut fx.deltas, dst, n)?;
                    self.bump(&mut fx.outflow, src, n)?;
                    self.bump(&mut fx.inflow, dst, n)?;
                    fx.record.moves.push(Move { event: fx.event, from, to, asset: *x, amount: n.v });
                }
                Ins::FxTell(k) => {
                    let asof = pop!(stack);
                    let v = pop!(stack);
                    let now = Vm::now(need(s)?);
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    match fx.tells.iter_mut().find(|(slot, _)| slot == k) {
                        Some(entry) => entry.1 = (v, asof, now),
                        None => fx.tells.push((*k, (v, asof, now))),
                    }
                    fx.record.tells.push((fx.event, *k, v.v, asof.v));
                }
                Ins::FxForget(k) => {
                    let fx = fx.as_deref_mut().ok_or_else(|| Halt::Error(internal("an effect outside a commit")))?;
                    fx.forgets.push(*k);
                    fx.record.forgets.push((fx.event, *k));
                }
            }
        }
        Ok(stack.last().copied().unwrap_or(ZERO))
    }

    fn bump(&self, table: &mut Vec<(usize, Val)>, key: usize, by: Val) -> Result<(), Halt> {
        match table.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = self.add(entry.1, by)?,
            None => table.push((key, by)),
        }
        Ok(())
    }

    /// An actor used as an index: no form can follow it.
    fn actor(&self, x: Val) -> Result<usize, Halt> {
        let v = self.pin(x);
        if v >= 0 && (v as usize) < self.n_actors {
            Ok(v as usize)
        } else {
            Err(Halt::Error(internal("a value that is not an actor")))
        }
    }

    fn plain_val(&self, r: Result<Val, Halt>) -> R<Val> {
        match r {
            Ok(v) => Ok(v),
            Err(Halt::Error(e)) => Err(e),
            Err(Halt::Blocked(_)) => Err(internal("a condition outside a guard")),
        }
    }

    fn constant_val(&self, cid: usize) -> R<Val> {
        self.plain_val(self.exec(cid, None, None, None, None, None, None))
    }

    fn in_state_val(&self, cid: usize, s: &State) -> R<Val> {
        self.plain_val(self.exec(cid, Some(s), None, None, None, None, None))
    }

    fn at_point_val(&self, cid: usize, node: u32) -> R<Val> {
        self.plain_val(self.exec(cid, Some(&self.nodes[node as usize].state), None, None, Some(node), None, None))
    }

    fn rv_val(&self, cid: usize, leaf: u32) -> R<Val> {
        self.plain_val(self.exec(cid, None, None, None, None, Some(leaf), None))
    }

    /// A constant block (layer C).
    pub fn constant(&self, cid: usize) -> R<i64> {
        Ok(self.constant_val(cid)?.v)
    }

    /// A state block (layer S) in a state.
    pub fn in_state(&self, cid: usize, s: &State) -> R<i64> {
        Ok(self.in_state_val(cid, s)?.v)
    }

    /// A point block (layer Q) at an observation point.
    pub fn at_point(&self, cid: usize, node: u32) -> R<i64> {
        Ok(self.at_point_val(cid, node)?.v)
    }

    /// A run block (layer R) on a complete history.
    pub fn rv(&self, cid: usize, leaf: u32) -> R<i64> {
        Ok(self.rv_val(cid, leaf)?.v)
    }

    // ---- one commit ----------------------------------------------------------------------------
    fn members_val(&self, unit: usize) -> R<Vec<(usize, Vec<Val>)>> {
        let mut out = Vec::new();
        for (event, args) in &self.im.units[unit].members {
            let mut vals = Vec::with_capacity(args.len());
            for &a in args {
                vals.push(self.constant_val(a)?);
            }
            out.push((*event, vals));
        }
        Ok(out)
    }

    /// The members of a unit with their argument values.
    pub fn members(&self, unit: usize) -> R<Vec<(usize, Vec<i64>)>> {
        Ok(self.members_val(unit)?.into_iter().map(|(e, args)| (e, args.into_iter().map(|a| a.v).collect())).collect())
    }

    /// Next_U(s) for a unit of the image.
    pub fn step(&self, unit: usize, s: &State) -> R<Outcome> {
        let u = &self.im.units[unit];
        self.commit_val(&self.members_val(unit)?, u.net, s, &u.name)
    }

    /// The same judgement for explicit members [(event number, [argument values])].
    pub fn commit(&self, members: &[(usize, Vec<i64>)], net: bool, s: &State, name: &str) -> R<Outcome> {
        let members: Vec<(usize, Vec<Val>)> =
            members.iter().map(|(e, args)| (*e, args.iter().copied().map(Val::plain).collect())).collect();
        self.commit_val(&members, net, s, name)
    }

    fn commit_val(&self, members: &[(usize, Vec<Val>)], net: bool, s: &State, name: &str) -> R<Outcome> {
        let im = &self.im;
        let mut frames: Vec<(usize, Vec<Val>)> = Vec::with_capacity(members.len());
        for (ei, args) in members {
            // 1. bindings and conditions, in written order
            let ev = &im.events[*ei];
            let mut frame = args.clone();
            frame.resize(ev.locals.max(args.len()), EMPTY);
            match self.exec(ev.guard, Some(s), Some(frame.as_mut_slice()), None, None, None, None) {
                Ok(_) => {}
                Err(Halt::Blocked(reason)) => return Ok(Outcome::Blocked(im.reasons[reason].clone())),
                Err(Halt::Error(e)) => return Err(e),
            }
            frames.push((*ei, frame));
        }
        let mut fx = Effects::default();
        for (ei, frame) in frames.iter_mut() {
            // 2. flows, read from the pre-state
            fx.event = *ei;
            let r = self.exec(im.events[*ei].effects, Some(s), Some(frame.as_mut_slice()), None, None, None, Some(&mut fx));
            self.plain_val(r)?;
        }
        for &(key, out) in &fx.outflow {
            // 3. funds: gross unless the unit is net
            let extra = if net { lookup(&fx.inflow, key) } else { ZERO };
            let have = self.plain_val(self.add(self.cell(s, key), extra))?;
            let left = self.plain_val(self.sub(have, out))?;
            let (x, who) = ((key - self.h0) / self.n_actors, (key - self.h0) % self.n_actors);
            if self.lt(left, self.floor[x]) {
                return Ok(Outcome::Blocked(format!("funds of {} in {}", im.actors[who], im.assets[x].name)));
            }
        }
        let mut vals: Vec<i64> = (*s.cells).clone();
        let mut forms: Option<Vec<u32>> =
            self.rec.then(|| s.fcells.as_ref().map(|f| (**f).clone()).unwrap_or_else(|| vec![0; vals.len()]));
        for &(c, v) in &fx.sets {
            vals[c] = v.v;
            if let Some(f) = &mut forms {
                f[c] = v.f;
            }
        }
        for &(c, dlt) in &fx.deltas {
            let before = match &forms {
                Some(f) if f[c] & PENDING != 0 => self.arrive(c, vals[c], f[c] & !PENDING),
                Some(f) => Val { v: vals[c], f: f[c] },
                None => Val::plain(vals[c]),
            };
            if before.v == NONE {
                return Err(internal("a change to an empty cell"));
            }
            let after = self.plain_val(self.add(before, dlt))?;
            vals[c] = after.v;
            if let Some(f) = &mut forms {
                f[c] = after.f;
            }
        }
        let (mem, fmem) = if fx.tells.is_empty() && fx.forgets.is_empty() {
            (s.mem.clone(), s.fmem.clone())
        } else {
            let mut mem = (*s.mem).clone();
            let mut fmem: Option<Vec<[u32; 3]>> =
                self.rec.then(|| s.fmem.as_ref().map(|f| (**f).clone()).unwrap_or_else(|| vec![[0; 3]; mem.len()]));
            for &(k, (v, asof, recv)) in &fx.tells {
                mem[k] = Some((v.v, asof.v, recv.v));
                if let Some(f) = &mut fmem {
                    f[k] = [v.f, asof.f, recv.f];
                }
            }
            for &k in &fx.forgets {
                mem[k] = None;
                if let Some(f) = &mut fmem {
                    f[k] = [0; 3];
                }
            }
            (Arc::new(mem), fmem.map(Arc::new))
        };
        let written: Vec<usize> =
            fx.sets.iter().map(|(c, _)| *c).chain(fx.deltas.iter().map(|(c, _)| *c)).chain(fx.frees.iter().copied()).collect();
        let doms: Vec<Domain> = fx.frees.iter().map(|c| self.domains[*c]).collect();
        if self.rec {
            // how many values a freed cell can take is no form: its bounds keep their sample values
            for &c in &fx.frees {
                let (lo, hi) = self.dom_forms[c];
                self.pin(Val { v: self.domains[c].lo, f: lo });
                self.pin(Val { v: self.domains[c].hi, f: hi });
            }
        }
        let mut cands = Vec::new();
        if doms.iter().all(|d| !d.is_empty()) {
            // 4. candidates: every way to fill the freed cells, the last one turning fastest
            let mut pick = vec![0u128; doms.len()];
            'choices: loop {
                let (mut cells, mut cforms) = (vals.clone(), forms.clone());
                for ((c, d), i) in fx.frees.iter().zip(&doms).zip(&pick) {
                    cells[*c] = d.get(*i);
                    if let Some(f) = &mut cforms {
                        f[*c] = 0;
                    }
                }
                let s2 = State {
                    t: s.t,
                    cells: Arc::new(cells),
                    mem: mem.clone(),
                    ft: s.ft,
                    fcells: cforms.map(Arc::new),
                    fmem: fmem.clone(),
                };
                // 5. range, invariant, ensure
                let mut ok = true;
                for &c in &written {
                    if !self.in_range(c, self.cell(&s2, c)) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    for &inv in &im.invariants {
                        if !self.truth(self.in_state_val(inv, &s2)?) {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    'ensure: for (ei, frame) in frames.iter_mut() {
                        for &p in &im.events[*ei].ensure {
                            let r = self.exec(p, Some(s), Some(frame.as_mut_slice()), Some(&s2), None, None, None);
                            if !self.truth(self.plain_val(r)?) {
                                ok = false;
                                break 'ensure;
                            }
                        }
                    }
                }
                if ok {
                    cands.push(s2);
                }
                let mut at = doms.len();
                loop {
                    if at == 0 {
                        break 'choices;
                    }
                    at -= 1;
                    pick[at] += 1;
                    if pick[at] < doms[at].len() {
                        break;
                    }
                    pick[at] = 0;
                }
            }
        }
        if cands.is_empty() {
            return Ok(Outcome::NoSettlement(format!("no post-state for {name}")));
        }
        let mut record = fx.record;
        record.events = members.iter().map(|(e, args)| (*e, args.iter().map(|a| a.v).collect())).collect();
        Ok(Outcome::Ok(cands, Arc::new(record)))
    }

    fn in_range(&self, c: usize, v: Val) -> bool {
        match self.ranges[c] {
            Some((lo, hi)) => v.v == NONE || (self.le(lo, v) && self.le(v, hi)),
            None => true,
        }
    }

    // ---- program steps -------------------------------------------------------------------------
    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        parent: Option<u32>,
        step: Step,
        state: State,
        display: Option<u32>,
        src: Option<(u32, u32)>,
        record: Option<Arc<Record>>,
    ) -> R<u32> {
        if self.nodes.len() + 1 > self.budget {
            return Err(limit(format!("more than {} observation points", self.budget)));
        }
        let id = self.nodes.len() as u32;
        let world = match (&step, parent) {
            (Step::Init(w), _) => *w as u32,
            (_, Some(p)) => self.nodes[p as usize].world,
            _ => 0,
        };
        if let Some(p) = parent {
            self.nodes[p as usize].children.push(id);
        }
        self.nodes.push(Node { parent, step, state, children: Vec::new(), world, display, src, record });
        Ok(id)
    }

    fn cont(&mut self, b: u32, i: u32, k: u32) -> u32 {
        self.conts.push((b, i, k));
        (self.conts.len() - 1) as u32
    }

    /// Run the program from every initial world. Afterwards `nodes` holds every observation point
    /// and `leaves` the complete histories. The outcome is remembered: a second call is free.
    pub fn explore(&mut self) -> R<()> {
        if let Some(done) = &self.explored {
            return done.clone();
        }
        let outcome = self.run_program();
        if outcome.is_err() {
            self.nodes.clear();
            self.leaves.clear();
            self.roots.clear();
        }
        self.conts = Vec::new();
        self.explored = Some(outcome.clone());
        outcome
    }

    /// (block, position, continuation) just after a mark: where an instance begun there starts.
    fn resume(&mut self, mark: usize) -> R<(u32, u32, u32)> {
        let im = self.im.clone();
        // every block but the entry is referred to once: by one statement of its parent
        let mut parent: Vec<Option<(u32, u32)>> = vec![None; im.blocks.len()];
        let mut found = None;
        for (b, stmts) in im.blocks.iter().enumerate() {
            for (i, st) in stmts.iter().enumerate() {
                let at = Some((b as u32, i as u32));
                match st {
                    StmtI::At(_, blk) => parent[*blk] = at,
                    StmtI::Attempt(_, yes, no) => {
                        parent[*yes] = at;
                        parent[*no] = at;
                    }
                    StmtI::Alt(blocks) => blocks.iter().for_each(|blk| parent[*blk] = at),
                    StmtI::Choose(_, _, arms) => arms.iter().for_each(|(_, blk)| parent[*blk] = at),
                    StmtI::Mark(m) if *m == mark => found = at,
                    _ => {}
                }
            }
        }
        let Some((b, i)) = found else { return Err(internal("a mark that is not in the program")) };
        let mut chain = vec![b];
        while let Some((up, _)) = parent[*chain.last().unwrap() as usize] {
            chain.push(up);
            if chain.len() > im.blocks.len() {
                return Err(internal("blocks that contain each other"));
            }
        }
        let mut k = NO_CONT;
        for pair in chain.windows(2).rev() {
            let (child, up) = (pair[0], pair[1]);
            let (_, at) = parent[child as usize].unwrap();
            k = self.cont(up, at + 1, k);
        }
        Ok((b, i + 1, k))
    }

    fn run_program(&mut self) -> R<()> {
        let im = self.im.clone();
        let mut stack: Vec<Work> = Vec::new();
        let begin = match self.begun {
            Some(mark) => self.resume(mark)?,
            None => (im.entry as u32, 0, NO_CONT),
        };
        for w in 0..self.worlds.len() {
            let st = self.worlds[w].clone();
            let root = self.node(None, Step::Init(w), st.clone(), None, None, None)?;
            self.roots.push(root);
            stack.push(Work::Block { b: begin.0, i: begin.1, state: st, node: root, k: begin.2 });
            while let Some(work) = stack.pop() {
                self.work(&im, work, &mut stack)?;
            }
        }
        self.leaves = (0..self.nodes.len() as u32).filter(|&n| self.nodes[n as usize].is_leaf()).collect();
        // nodes are stored depth first, so a subtree is a run of consecutive numbers
        let mut end = vec![0u32; self.nodes.len()];
        for n in (0..self.nodes.len()).rev() {
            end[n] = match self.nodes[n].children.last() {
                Some(&last) => end[last as usize],
                None => n as u32 + 1,
            };
        }
        self.subtree_end = end;
        Ok(())
    }

    fn work(&mut self, im: &Arc<Image>, work: Work, stack: &mut Vec<Work>) -> R<()> {
        match work {
            Work::Block { mut b, mut i, state, node, mut k } => {
                // the end of a block: what remains after it, or the end of the history
                while i as usize == im.blocks[b as usize].len() {
                    if k == NO_CONT {
                        self.node(Some(node), Step::End, state, None, None, None)?;
                        return Ok(());
                    }
                    (b, i, k) = self.conts[k as usize];
                }
                let src = Some((b, i));
                match &im.blocks[b as usize][i as usize] {
                    StmtI::At(time, blk) => {
                        let t = self.constant_val(*time)?;
                        if self.lt(t, Vm::now(&state)) || !(self.le(Val::plain(im.time.0), t) && self.le(t, Val::plain(im.time.1))) {
                            return Err(FeanError::new(
                                Kind::Static,
                                "W-Clock",
                                format!("at @{} is outside the clock at @{}", t.v, state.t),
                                None,
                            ));
                        }
                        let s1 = State { t: t.v, ft: t.f, ..state.clone() };
                        let n = self.node(Some(node), Step::Tick(t.v), s1.clone(), None, src, None)?;
                        let rest = self.cont(b, i + 1, k);
                        stack.push(Work::Block { b: *blk as u32, i: 0, state: s1, node: n, k: rest });
                    }
                    stmt @ (StmtI::Run(unit) | StmtI::Attempt(unit, _, _)) => {
                        let attempt = match stmt {
                            StmtI::Attempt(_, yes, no) => Some((*yes as u32, *no as u32)),
                            _ => None,
                        };
                        match self.step(*unit, &state)? {
                            Outcome::Ok(cands, record) => {
                                let rest = if attempt.is_some() { self.cont(b, i + 1, k) } else { NO_CONT };
                                stack.push(Work::Commits {
                                    cands,
                                    record,
                                    next: 0,
                                    unit: *unit as u32,
                                    then: attempt.map(|(yes, _)| yes),
                                    b,
                                    i,
                                    node,
                                    k,
                                    rest,
                                });
                            }
                            Outcome::Blocked(reason) if attempt.is_some() => {
                                let n = self.node(Some(node), Step::Reject(*unit, reason), state.clone(), None, src, None)?;
                                let rest = self.cont(b, i + 1, k);
                                stack.push(Work::Block { b: attempt.unwrap().1, i: 0, state, node: n, k: rest });
                            }
                            Outcome::Blocked(reason) => {
                                self.node(Some(node), Step::Fail(*unit, Status::Blocked, reason), state, None, src, None)?;
                            }
                            Outcome::NoSettlement(reason) => {
                                self.node(Some(node), Step::Fail(*unit, Status::NoSettlement, reason), state, None, src, None)?;
                            }
                        }
                    }
                    StmtI::Alt(_) => {
                        let rest = self.cont(b, i + 1, k);
                        stack.push(Work::Alt { b, i, next: 0, state, node, rest });
                    }
                    StmtI::Choose(who, menu, arms) => {
                        let key = (*who, *menu, arms.iter().map(|(a, _)| *a).collect::<Vec<usize>>());
                        let display = match self.display_ix.get(&key) {
                            Some(&d) => d,
                            None => {
                                let d = self.displays.len() as u32;
                                self.displays.push(key.clone());
                                self.display_ix.insert(key, d);
                                d
                            }
                        };
                        let shown = self.node(Some(node), Step::Show(*who, *menu), state.clone(), Some(display), src, None)?;
                        let rest = self.cont(b, i + 1, k);
                        stack.push(Work::Picks { b, i, next: 0, state, shown, rest });
                    }
                    StmtI::Mark(mark) => {
                        let n = self.node(Some(node), Step::Mark(*mark), state.clone(), None, src, None)?;
                        stack.push(Work::Block { b, i: i + 1, state, node: n, k });
                    }
                }
            }
            Work::Commits { cands, record, next, unit, then, b, i, node, k, rest } => {
                let s2 = cands[next].clone();
                let n = self.node(Some(node), Step::Commit(unit as usize), s2.clone(), None, Some((b, i)), Some(record.clone()))?;
                if next + 1 < cands.len() {
                    stack.push(Work::Commits { cands, record, next: next + 1, unit, then, b, i, node, k, rest });
                }
                stack.push(match then {
                    Some(yes) => Work::Block { b: yes, i: 0, state: s2, node: n, k: rest },
                    None => Work::Block { b, i: i + 1, state: s2, node: n, k },
                });
            }
            Work::Alt { b, i, next, state, node, rest } => {
                let StmtI::Alt(blocks) = &im.blocks[b as usize][i as usize] else { return Err(internal("alt")) };
                let n = self.node(Some(node), Step::Branch(next), state.clone(), None, Some((b, i)), None)?;
                let blk = blocks[next] as u32;
                if next + 1 < blocks.len() {
                    stack.push(Work::Alt { b, i, next: next + 1, state: state.clone(), node, rest });
                }
                stack.push(Work::Block { b: blk, i: 0, state, node: n, k: rest });
            }
            Work::Picks { b, i, next, state, shown, rest } => {
                let StmtI::Choose(who, menu, arms) = &im.blocks[b as usize][i as usize] else {
                    return Err(internal("choose"));
                };
                let (action, blk) = arms[next];
                let n = self.node(Some(shown), Step::Pick(*who, *menu, action), state.clone(), None, Some((b, i)), None)?;
                if next + 1 < arms.len() {
                    stack.push(Work::Picks { b, i, next: next + 1, state: state.clone(), shown, rest });
                }
                stack.push(Work::Block { b: blk as u32, i: 0, state, node: n, k: rest });
            }
        }
        Ok(())
    }

    // ---- views and knowledge -------------------------------------------------------------------
    /// What `who` sees at a state: view_i(p) = (public time, menu shown to i, i's slots, cells i
    /// observes), and beside it the forms of those values while a run is recorded.
    fn view(&self, who: usize, state: &State, shown: u32) -> (Vec<i64>, Vec<u32>) {
        let im = &self.im;
        let size = 2 + 4 * im.mems[who].len() + im.obs[who].len();
        let (mut key, mut forms) = (Vec::with_capacity(size), Vec::with_capacity(if self.rec { size } else { 0 }));
        let put = |x: Val, key: &mut Vec<i64>, forms: &mut Vec<u32>| {
            key.push(x.v);
            if self.rec {
                forms.push(x.f);
            }
        };
        put(Vm::now(state), &mut key, &mut forms);
        put(Val::plain(shown as i64), &mut key, &mut forms);
        for &k in &im.mems[who] {
            match self.slot(state, k) {
                Some((v, asof, recv)) => [Val::plain(1), v, asof, recv].into_iter().for_each(|x| put(x, &mut key, &mut forms)),
                None => (0..4).for_each(|_| put(ZERO, &mut key, &mut forms)),
            }
        }
        for &c in &im.obs[who] {
            put(self.cell(state, c), &mut key, &mut forms);
        }
        (key, forms)
    }

    /// The classes of observation points `who` cannot tell apart.
    pub fn classes(&self, who: usize) -> Arc<Classes> {
        if let Some(c) = &self.classes.borrow()[who] {
            return c.clone();
        }
        let mut ids: HashMap<Vec<i64>, u32> = HashMap::new();
        let mut quick: HashMap<(usize, usize, i64, u32, u32), u32> = HashMap::new();
        let mut of = Vec::with_capacity(self.nodes.len());
        let mut members: Vec<Vec<u32>> = Vec::new();
        // while recording: for every class, the forms of each different way its view comes about
        let mut ways: Vec<Vec<Vec<u32>>> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            let shown = match n.display {
                Some(d) if self.displays[d as usize].0 == who => d + 1,
                _ => 0,
            };
            // points that share their state share their view: skip building the key
            let fast = (Arc::as_ptr(&n.state.cells) as usize, Arc::as_ptr(&n.state.mem) as usize, n.state.t, shown, n.state.ft);
            let id = match quick.get(&fast) {
                Some(&id) => id,
                None => {
                    let (key, forms) = self.view(who, &n.state, shown);
                    let fresh = members.len() as u32;
                    let id = *ids.entry(key).or_insert(fresh);
                    if id == fresh {
                        members.push(Vec::new());
                        ways.push(Vec::new());
                    }
                    if self.rec {
                        ways[id as usize].push(forms);
                    }
                    quick.insert(fast, id);
                    id
                }
            };
            of.push(id);
            members[id as usize].push(i as u32);
        }
        if self.rec {
            self.certify(&ids, &ways);
        }
        let classes = Arc::new(Classes { of, members });
        self.classes.borrow_mut()[who] = Some(classes.clone());
        classes
    }

    /// While recording: what makes two points one class, or two, is a decision. Record that the
    /// ways one class comes about stay equal, and that the classes, put in order, stay in that
    /// order (so that any two of them stay different).
    fn certify(&self, ids: &HashMap<Vec<i64>, u32>, ways: &[Vec<Vec<u32>>]) {
        for (key, &id) in ids {
            let (first, rest) = ways[id as usize].split_first().expect("a class has a way");
            for other in rest {
                for j in 0..key.len() {
                    if other[j] != first[j] {
                        self.same(Val { v: key[j], f: first[j] }, Val { v: key[j], f: other[j] });
                    }
                }
            }
        }
        let mut order: Vec<(&Vec<i64>, u32)> = ids.iter().map(|(key, &id)| (key, id)).collect();
        order.sort();
        for pair in order.windows(2) {
            let ((a, ia), (b, ib)) = (pair[0], pair[1]);
            let (fa, fb) = (&ways[ia as usize][0], &ways[ib as usize][0]);
            for j in 0..a.len().min(b.len()) {
                let (x, y) = (Val { v: a[j], f: fa[j] }, Val { v: b[j], f: fb[j] });
                if a[j] == b[j] {
                    if fa[j] != fb[j] {
                        self.same(x, y);
                    }
                    continue;
                }
                if a[j] != NONE && b[j] != NONE {
                    self.lt(x, y);
                }
                break;
            }
        }
    }

    /// M, p |= K(i, phi): phi (a point-layer code block) holds wherever i cannot tell apart from p.
    pub fn knows(&self, node: u32, who: usize, code: usize) -> R<bool> {
        let classes = self.classes(who);
        let class = classes.of[node as usize];
        let key = (who as u32, class, code as u32);
        if let Some(&known) = self.kmemo.borrow().get(&key) {
            return Ok(known);
        }
        let mut all = true;
        for &q in &classes.members[class as usize] {
            if !self.truth(self.at_point_val(code, q)?) {
                all = false;
                break;
            }
        }
        self.kmemo.borrow_mut().insert(key, all);
        Ok(all)
    }

    /// The observation points of a complete history, from its initial world to its last step.
    pub fn path(&self, leaf: u32) -> Vec<u32> {
        let mut p = Vec::new();
        let mut n = Some(leaf);
        while let Some(i) = n {
            p.push(i);
            n = self.nodes[i as usize].parent;
        }
        p.reverse();
        p
    }

    /// The point of a history: kind 0 start, 1 end, 2 menu, 3 label.
    pub fn point(&self, leaf: u32, kind: u8, ident: usize) -> Option<u32> {
        let mut cache = self.points.borrow_mut();
        let table = cache.entry(leaf).or_insert_with(|| {
            let mut table = HashMap::new();
            let mut n = Some(leaf);
            while let Some(i) = n {
                let node = &self.nodes[i as usize];
                let key = match node.step {
                    Step::Init(_) => Some((0u8, 0u32)),
                    Step::End => Some((1, 0)),
                    Step::Show(_, menu) => Some((2, menu as u32)),
                    Step::Mark(mark) => Some((3, mark as u32)),
                    _ => None,
                };
                if let Some(key) = key {
                    table.entry(key).or_insert(i);
                }
                if let (Step::Init(_), Some(mark)) = (&node.step, self.begun) {
                    // an instance begun at a mark: its start is that point
                    table.entry((3, mark as u32)).or_insert(i);
                }
                n = node.parent;
            }
            table
        });
        table.get(&(kind, ident as u32)).copied()
    }

    /// The complete histories that pass through a point.
    pub fn under(&self, node: u32) -> Vec<u32> {
        (node..self.subtree_end[node as usize]).filter(|&n| self.nodes[n as usize].is_leaf()).collect()
    }

    // ---- queries -------------------------------------------------------------------------------
    /// The information sets of a coalition (in the order their menus first appear) and the picks
    /// each complete history made in them. A strategy profile chooses one action per set.
    pub fn game(&self, who: &[usize]) -> Game {
        let mut index: HashMap<(usize, u32), u32> = HashMap::new();
        let mut sets: Vec<InfoSet> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Step::Show(w, menu) = n.step {
                if who.contains(&w) {
                    let class = self.classes(w).of[i];
                    index.entry((w, class)).or_insert_with(|| {
                        let actions = n.display.map(|d| self.displays[d as usize].2.clone()).unwrap_or_default();
                        sets.push(InfoSet { actor: w, class, menu, actions, node: i as u32 });
                        (sets.len() - 1) as u32
                    });
                }
            }
        }
        let mut picks = Vec::with_capacity(self.leaves.len());
        for &leaf in &self.leaves {
            let mut mine = Vec::new();
            let mut n = Some(leaf);
            while let Some(i) = n {
                let node = &self.nodes[i as usize];
                if let (Step::Pick(w, _, action), Some(parent)) = (&node.step, node.parent) {
                    if who.contains(w) {
                        let class = self.classes(*w).of[parent as usize];
                        mine.push((index[&(*w, class)], *action));
                    }
                }
                n = node.parent;
            }
            mine.reverse();
            picks.push(mine);
        }
        Game { sets, picks }
    }

    /// The probabilities of the initial worlds under a weight.
    pub fn probs(&self, weight: usize) -> R<Arc<Vec<Ratio>>> {
        if let Some(p) = self.probs.borrow().get(&weight) {
            return Ok(p.clone());
        }
        let (name, code) = &self.im.weights[weight];
        let mut raw = Vec::with_capacity(self.worlds.len());
        for st in &self.worlds {
            // a probability is a quotient of weights: no form follows it
            raw.push(self.pin(self.in_state_val(*code, st)?));
        }
        let mut total: i128 = 0;
        for &x in &raw {
            total += x as i128;
        }
        if raw.iter().any(|&x| x < 0 || x == NONE) || total == 0 {
            return Err(FeanError::model("D-Weight", format!("weight {name} is negative or sums to zero")));
        }
        let mut probs = Vec::with_capacity(raw.len());
        for &x in &raw {
            probs.push(Ratio::new(x as i128, total)?);
        }
        let probs = Arc::new(probs);
        self.probs.borrow_mut().insert(weight, probs.clone());
        Ok(probs)
    }

    fn values_val(&self, code: usize) -> R<Vec<Val>> {
        self.leaves.iter().map(|&leaf| self.rv_val(code, leaf)).collect()
    }

    /// The value of a run block on every complete history, in the order of `leaves`.
    pub fn values(&self, code: usize) -> R<Vec<i64>> {
        self.leaves.iter().map(|&leaf| self.rv(code, leaf)).collect()
    }

    /// A strategy profile of the coalition under which the block holds on every history that
    /// follows it; `None` when there is none. The profile gives an action for the information sets
    /// it had to decide (any action will do for the others).
    pub fn winning_policy(&self, game: &Game, truth: &[bool]) -> R<Option<Vec<Option<usize>>>> {
        let bad: Vec<&[(u32, usize)]> = (0..truth.len()).filter(|&l| !truth[l]).map(|l| game.picks[l].as_slice()).collect();
        let mut chosen: Vec<Option<usize>> = vec![None; game.sets.len()];
        let mut steps = 0usize;
        Ok(if search_can(game, &bad, &mut chosen, &mut steps)? { Some(chosen) } else { None })
    }

    /// Is score `a` strictly above score `b`?
    fn beats(&self, a: Scored, b: Scored) -> R<bool> {
        if (a.f | b.f) == 0 {
            a.r.gt(b.r)
        } else {
            Ok(self.lt(b.val(), a.val()))
        }
    }

    /// The value of a set of complete histories: the best (or worst) of them, or, under a weight,
    /// the expectation over the initial worlds of the best (or worst) in each world.
    fn aggregate(&self, vals: &[Val], worlds: &[u32], probs: Option<&[Ratio]>, n_worlds: usize, max: bool, alive: &[u32]) -> R<Scored> {
        let better = |a: Val, b: Val| {
            if max {
                if self.le(b, a) {
                    a
                } else {
                    b
                }
            } else if self.le(a, b) {
                a
            } else {
                b
            }
        };
        match probs {
            None => {
                let mut best: Option<Val> = None;
                for &l in alive {
                    best = Some(best.map_or(vals[l as usize], |old| better(old, vals[l as usize])));
                }
                best.map(|v| Scored { r: Ratio::int(v.v), f: v.f }).ok_or_else(|| internal("no history to take a value from"))
            }
            Some(p) => {
                let mut per: Vec<Option<Val>> = vec![None; n_worlds];
                for &l in alive {
                    let w = worlds[l as usize] as usize;
                    per[w] = Some(per[w].map_or(vals[l as usize], |old| better(old, vals[l as usize])));
                }
                let mut total = Ratio::int(0);
                for (w, pw) in p.iter().enumerate() {
                    if !pw.is_zero() {
                        let v = per[w].ok_or_else(|| internal("no history in a world"))?;
                        total = total.add(pw.mul(Ratio::int(self.pin(v)))?)?;
                    }
                }
                Ok(Scored { r: total, f: 0 })
            }
        }
    }

    fn best_scored(&self, game: &Game, vals: &[Val], weight: Option<usize>, max: bool) -> R<(Scored, Vec<Option<usize>>)> {
        let probs = match weight {
            Some(w) => Some(self.probs(w)?),
            None => None,
        };
        let worlds: Vec<u32> = self.leaves.iter().map(|&l| self.nodes[l as usize].world).collect();
        let mut search = NumSearch {
            vm: self,
            game,
            vals,
            worlds: &worlds,
            probs: probs.as_deref().map(|p| p.as_slice()),
            n_worlds: self.worlds.len(),
            max,
            chosen: vec![None; game.sets.len()],
            best: None,
            steps: 0,
        };
        let all: Vec<u32> = (0..vals.len() as u32).collect();
        search.go(0, &all)?;
        search.best.ok_or_else(|| internal("no strategy profile"))
    }

    fn value_scored(&self, vals: &[Val], weight: Option<usize>, max: bool) -> R<Scored> {
        let probs = match weight {
            Some(w) => Some(self.probs(w)?),
            None => None,
        };
        let worlds: Vec<u32> = self.leaves.iter().map(|&l| self.nodes[l as usize].world).collect();
        let all: Vec<u32> = (0..vals.len() as u32).collect();
        self.aggregate(vals, &worlds, probs.as_deref().map(|p| p.as_slice()), self.worlds.len(), max, &all)
    }

    /// max over strategy profiles of the coalition of the value of the histories that follow one;
    /// also the profile that reaches it.
    pub fn best_policy(&self, game: &Game, vals: &[i64], weight: Option<usize>, max: bool) -> R<(Ratio, Vec<Option<usize>>)> {
        let vals: Vec<Val> = vals.iter().copied().map(Val::plain).collect();
        let (score, chosen) = self.best_scored(game, &vals, weight, max)?;
        Ok((score.r, chosen))
    }

    /// The value of all complete histories: the best (or worst), or its expectation under a weight.
    pub fn value_of(&self, vals: &[i64], weight: Option<usize>, max: bool) -> R<Ratio> {
        let vals: Vec<Val> = vals.iter().copied().map(Val::plain).collect();
        Ok(self.value_scored(&vals, weight, max)?.r)
    }

    /// The verdict of query `i` in this instance, as a machine value: what its name stands for
    /// in a goal. The histories must have been explored.
    fn verdict(&self, i: usize) -> Result<Val, Halt> {
        let known = self.verdicts.borrow().get(&i).cloned();
        let (answer, form) = match known {
            Some(answer) => answer,
            None => {
                let answer = self.evaluate(&self.im.queries[i].q)?;
                self.verdicts.borrow_mut().insert(i, answer.clone());
                answer
            }
        };
        match answer {
            Answer::Bool(b) => Ok(Val::plain(b as i64)),
            Answer::Num(r) if r.d == 1 => match i64::try_from(r.n) {
                Ok(n) if n != NONE => Ok(Val { v: n, f: form }),
                _ => Err(Halt::Error(overflow())),
            },
            Answer::Num(_) => Err(Halt::Error(internal("a fraction where a goal needs a number"))),
        }
    }

    /// A goal or an objective (layer V, or layer X in the state the instance began in).
    pub fn goal_val(&mut self, code: usize) -> R<Val> {
        self.explore()?;
        let state = if self.begun.is_some() { self.worlds.first().cloned() } else { None };
        self.plain_val(self.exec(code, state.as_ref(), None, None, None, None, None))
    }

    pub fn goal(&mut self, code: usize) -> R<i64> {
        Ok(self.goal_val(code)?.v)
    }

    /// The value of a query (not a region) for this assignment.
    pub fn query(&mut self, q: &QueryI) -> R<Answer> {
        self.explore()?;
        Ok(self.evaluate(q)?.0)
    }

    /// The same, once the histories have been explored; with the form of a numeric answer.
    fn evaluate(&self, q: &QueryI) -> R<(Answer, u32)> {
        if self.explored.is_none() {
            return Err(internal("a query before the histories"));
        }
        match q {
            QueryI::Exists(code) => {
                for &leaf in &self.leaves {
                    if self.truth(self.rv_val(*code, leaf)?) {
                        return Ok((Answer::Bool(true), 0));
                    }
                }
                Ok((Answer::Bool(false), 0))
            }
            QueryI::Forall(code) => {
                for &leaf in &self.leaves {
                    if !self.truth(self.rv_val(*code, leaf)?) {
                        return Ok((Answer::Bool(false), 0));
                    }
                }
                Ok((Answer::Bool(true), 0))
            }
            QueryI::Can(who, code) => {
                let truth: Vec<bool> = self.values_val(*code)?.into_iter().map(|v| self.truth(v)).collect();
                let game = self.game(who);
                Ok((Answer::Bool(self.winning_policy(&game, &truth)?.is_some()), 0))
            }
            QueryI::Num { policy, weight, max, code } => {
                let vals = self.values_val(*code)?;
                let score = match policy {
                    None => self.value_scored(&vals, *weight, *max)?,
                    Some(who) => {
                        let game = self.game(who);
                        self.best_scored(&game, &vals, *weight, *max)?.0
                    }
                };
                Ok((Answer::Num(score.r), score.f))
            }
            QueryI::Prefer { who, menu, a, b, code, weight } => Ok((Answer::Bool(self.prefer(*who, *menu, *a, *b, *code, *weight)?), 0)),
            QueryI::Region(..) => Err(internal("a region inside a region")),
        }
    }

    /// True when, in every information set of `who` at the menu, a scores strictly above b.
    pub fn prefer(&self, who: usize, menu: usize, a: usize, b: usize, code: usize, weight: Option<usize>) -> R<bool> {
        let classes = self.classes(who);
        let mut order: Vec<u32> = Vec::new();
        let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if matches!(n.step, Step::Show(_, m) if m == menu) {
                let class = classes.of[i];
                groups.entry(class).or_insert_with(|| {
                    order.push(class);
                    Vec::new()
                });
                groups.get_mut(&class).unwrap().push(i as u32);
            }
        }
        let probs = match weight {
            Some(w) => Some(self.probs(w)?),
            None => None,
        };
        let score = |nodes: &[u32], action: usize| -> R<Option<Scored>> {
            let mut worst: Vec<Option<Val>> = vec![None; self.worlds.len()];
            let mut any = false;
            for &n in nodes {
                for &child in &self.nodes[n as usize].children {
                    if matches!(self.nodes[child as usize].step, Step::Pick(_, _, act) if act == action) {
                        for leaf in self.under(child) {
                            let v = self.rv_val(code, leaf)?;
                            let w = self.nodes[leaf as usize].world as usize;
                            worst[w] = Some(worst[w].map_or(v, |old| if self.le(old, v) { old } else { v }));
                            any = true;
                        }
                    }
                }
            }
            if !any {
                return Ok(None);
            }
            match &probs {
                None => {
                    let mut least: Option<Val> = None;
                    for v in worst.iter().flatten() {
                        least = Some(least.map_or(*v, |old| if self.le(old, *v) { old } else { *v }));
                    }
                    Ok(least.map(|v| Scored { r: Ratio::int(v.v), f: v.f }))
                }
                Some(p) => {
                    let (mut mass, mut total) = (Ratio::int(0), Ratio::int(0));
                    for (w, v) in worst.iter().enumerate() {
                        if let Some(v) = v {
                            mass = mass.add(p[w])?;
                            total = total.add(p[w].mul(Ratio::int(self.pin(*v)))?)?;
                        }
                    }
                    if mass.is_zero() {
                        return Err(FeanError::model("D-Weight", "an information set has probability zero"));
                    }
                    Ok(Some(Scored { r: total.div(mass)?, f: 0 }))
                }
            }
        };
        if order.is_empty() {
            return Ok(false);
        }
        for class in order {
            let nodes = &groups[&class];
            let (sa, sb) = (score(nodes, a)?, score(nodes, b)?);
            match (sa, sb) {
                (Some(sa), Some(sb)) if self.beats(sa, sb)? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}

/// Is there a strategy profile that no bad history follows? `bad` holds the picks of the bad
/// histories still alive (those that agree with every choice made so far).
fn search_can(game: &Game, bad: &[&[(u32, usize)]], chosen: &mut Vec<Option<usize>>, steps: &mut usize) -> R<bool> {
    if bad.is_empty() {
        return Ok(true);
    }
    *steps += 1;
    if *steps > POLICY_LIMIT {
        return Err(limit(format!("more than {POLICY_LIMIT} steps in the search over strategies")));
    }
    let mut branch: Option<u32> = None;
    for picks in bad {
        let mut open = false;
        for &(set, _) in picks.iter() {
            if chosen[set as usize].is_none() {
                open = true;
                branch = Some(branch.map_or(set, |b| b.min(set)));
            }
        }
        if !open {
            return Ok(false); // this bad history follows whatever is chosen from here on
        }
    }
    let Some(set) = branch else { return Ok(false) };
    for &action in &game.sets[set as usize].actions {
        chosen[set as usize] = Some(action);
        let alive: Vec<&[(u32, usize)]> =
            bad.iter().filter(|picks| picks.iter().all(|&(s, a)| s != set || a == action)).copied().collect();
        if search_can(game, &alive, chosen, steps)? {
            return Ok(true);
        }
    }
    chosen[set as usize] = None;
    Ok(false)
}

struct NumSearch<'a> {
    vm: &'a Vm,
    game: &'a Game,
    vals: &'a [Val],
    worlds: &'a [u32],
    probs: Option<&'a [Ratio]>,
    n_worlds: usize,
    max: bool,
    chosen: Vec<Option<usize>>,
    best: Option<(Scored, Vec<Option<usize>>)>,
    steps: usize,
}

impl NumSearch<'_> {
    /// Decide the information sets from `set` on; `alive` are the histories that agree so far.
    fn go(&mut self, set: usize, alive: &[u32]) -> R<()> {
        self.steps += 1;
        if self.steps > POLICY_LIMIT {
            return Err(limit(format!("more than {POLICY_LIMIT} steps in the search over strategies")));
        }
        if set == self.game.sets.len() {
            let value = self.vm.aggregate(self.vals, self.worlds, self.probs, self.n_worlds, self.max, alive)?;
            let better = match &self.best {
                None => true,
                Some((best, _)) => self.vm.beats(value, *best)?,
            };
            if better {
                self.best = Some((value, self.chosen.clone()));
            }
            return Ok(());
        }
        let touched = alive.iter().any(|&l| self.game.picks[l as usize].iter().any(|&(s, _)| s as usize == set));
        if !touched {
            // no living history meets this set: every action gives the same histories
            return self.go(set + 1, alive);
        }
        let actions = self.game.sets[set].actions.clone();
        for action in actions {
            self.chosen[set] = Some(action);
            let next: Vec<u32> = alive
                .iter()
                .copied()
                .filter(|&l| self.game.picks[l as usize].iter().all(|&(s, a)| s as usize != set || a == action))
                .collect();
            self.go(set + 1, &next)?;
        }
        self.chosen[set] = None;
        Ok(())
    }
}
