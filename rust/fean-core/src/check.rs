//! Static semantics: expansion of templates and parameters, then type and structure checks.
//!
//! Port of `fean/static.py` (rules: docs/financial-language.md section 2; Map unit I-09-b).
//! Declarations are registered and checked in the same order as the Python front end, because
//! both stop at the first error and must name the same rule on the same line.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use crate::syntax::*;

// ---- types -----------------------------------------------------------------------------------

/// Canonical unit: (asset, exponent) sorted by asset, no zero exponents. Empty means Num.
pub type Unit = Vec<(String, i32)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Time,
    Actor,
    Asset,
    Enum(String),
    Qty(Unit),
    Opt(Box<Ty>),
    /// The type of `empty`: fits every optional type.
    Null,
}

pub fn opt(t: Ty) -> Ty {
    match t {
        Ty::Opt(_) | Ty::Null => t,
        other => Ty::Opt(Box::new(other)),
    }
}

/// The type without its optionality; `None` for the type of `empty`.
pub fn base(t: &Ty) -> Option<&Ty> {
    match t {
        Ty::Null => None,
        Ty::Opt(inner) => Some(inner),
        other => Some(other),
    }
}

pub fn is_opt(t: &Ty) -> bool {
    matches!(t, Ty::Opt(_) | Ty::Null)
}

/// A value of type `t` may be used where `want` is expected (tau <: tau?).
pub fn fits(t: &Ty, want: &Ty) -> bool {
    if *t == Ty::Null {
        return is_opt(want);
    }
    if is_opt(t) && !is_opt(want) {
        return false;
    }
    base(t) == base(want)
}

pub fn unit_of(pairs: &[(String, i32)]) -> Unit {
    let mut acc: Vec<(String, i32)> = Vec::new();
    for (name, exp) in pairs {
        match acc.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 += exp,
            None => acc.push((name.clone(), *exp)),
        }
    }
    acc.retain(|(_, e)| *e != 0);
    acc.sort();
    acc
}

pub fn unit_text(unit: &Unit) -> String {
    let mut up: Vec<&str> = Vec::new();
    let mut down = String::new();
    for (n, e) in unit {
        for _ in 0..e.unsigned_abs() {
            if *e > 0 {
                up.push(n);
            } else {
                down.push('/');
                down.push_str(n);
            }
        }
    }
    let head = if up.is_empty() { "1".to_string() } else { up.join("*") };
    head + &down
}

pub fn type_text(t: &Ty) -> String {
    match t {
        Ty::Null => "empty".into(),
        Ty::Opt(inner) => type_text(inner) + "?",
        Ty::Qty(u) if u.is_empty() => "Num".into(),
        Ty::Qty(u) => format!("Qty[{}]", unit_text(u)),
        Ty::Enum(name) => name.clone(),
        Ty::Bool => "Bool".into(),
        Ty::Time => "Time".into(),
        Ty::Actor => "Actor".into(),
        Ty::Asset => "Asset".into(),
    }
}

/// A constant written the way it appears in action and unit names.
pub fn const_text(node: &Expr) -> String {
    match node {
        Expr::Int(n) => n.to_string(),
        Expr::Neg(inner) => format!("-{}", const_text(inner)),
        Expr::Qty(n, unit) => format!("{}[{}]", n, unit_text(&unit_of(unit))),
        Expr::Time(n) => format!("@{n}"),
        Expr::Bool(b) => if *b { "true".into() } else { "false".into() },
        Expr::Name(q) => q.clone(),
        _ => "…".into(),
    }
}

// ---- ordered map -----------------------------------------------------------------------------

/// A map that remembers insertion order (the tables of a model keep declaration order).
#[derive(Clone, Debug)]
pub struct OrdMap<V> {
    keys: Vec<String>,
    vals: Vec<V>,
    index: HashMap<String, usize>,
}

impl<V> Default for OrdMap<V> {
    fn default() -> Self {
        OrdMap { keys: Vec::new(), vals: Vec::new(), index: HashMap::new() }
    }
}

impl<V> OrdMap<V> {
    pub fn insert(&mut self, key: &str, val: V) {
        match self.index.get(key) {
            Some(&i) => self.vals[i] = val,
            None => {
                self.index.insert(key.to_string(), self.keys.len());
                self.keys.push(key.to_string());
                self.vals.push(val);
            }
        }
    }
    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.vals[i])
    }
    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
    pub fn values(&self) -> &[V] {
        &self.vals
    }
    pub fn values_mut(&mut self) -> &mut [V] {
        &mut self.vals
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys.iter().zip(self.vals.iter())
    }
}

// ---- the checked model -----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum PVal {
    Bool(bool),
    Int(i64),
    Name(String),
}

#[derive(Clone, Debug)]
pub struct AssetDecl {
    pub lo: Expr,
    pub hi: Expr,
    pub init: OrdMap<Vec<Expr>>,
    pub public: bool,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub enum Observers {
    All,
    Some(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct CellDecl {
    pub name: String,
    pub ty: Ty,
    pub lo: Option<Expr>,
    pub hi: Option<Expr>,
    pub init: Vec<Expr>,
    pub writers: HashSet<String>,
    pub owner: String,
    pub observers: Observers,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WKey {
    Cell(String),
    Bal(String, String),
}

impl WKey {
    fn text(&self) -> String {
        match self {
            WKey::Cell(c) => c.clone(),
            WKey::Bal(who, asset) => format!("bal({who},{asset})"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Writes {
    pub sets: HashSet<WKey>,
    pub deltas: HashSet<WKey>,
    pub slots: HashSet<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct EventDecl {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub by: Vec<Expr>,
    pub net: bool,
    pub clauses: Vec<Clause>,
    pub owner: String,
    pub loc: Loc,
    pub writes: Writes,
}

#[derive(Clone, Debug)]
pub struct BatchDecl {
    pub net: bool,
    pub members: Vec<(String, Vec<Expr>)>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub struct DefDecl {
    pub params: Vec<(String, Ty)>,
    pub result: Ty,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QInfo {
    Bool,
    Num(Unit),
    Region(Vec<String>),
}

/// A model after expansion and checking: only ground declarations remain.
#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub actors: Vec<String>,
    pub actor_loc: HashMap<String, Loc>,
    pub assets: OrdMap<AssetDecl>,
    pub enums: OrdMap<Vec<String>>,
    pub enum_of: HashMap<String, String>,
    pub time: (i64, i64),
    pub params: OrdMap<(Ty, Vec<PVal>)>,
    pub param_loc: HashMap<String, Loc>,
    /// Unknowns: (type, least value, greatest value). Enum values, actors and truth values are numbered.
    pub unknowns: OrdMap<(Ty, i64, i64)>,
    pub unknown_loc: HashMap<String, Loc>,
    pub cells: OrdMap<CellDecl>,
    pub slots: Vec<((String, String), Ty)>,
    pub slot_loc: HashMap<(String, String), Loc>,
    pub assumes: Vec<Expr>,
    pub invariants: Vec<Expr>,
    pub defs: OrdMap<DefDecl>,
    pub events: OrdMap<EventDecl>,
    pub batches: OrdMap<BatchDecl>,
    pub program: Vec<Stmt>,
    pub weights: OrdMap<Expr>,
    pub values: OrdMap<Expr>,
    pub queries: OrdMap<Query>,
    pub query_loc: HashMap<String, Loc>,
    pub qinfo: HashMap<String, QInfo>,
    pub solves: OrdMap<Solve>,
    pub solve_loc: HashMap<String, Loc>,
    /// The unit of what a solve minimises or maximises (None when it has no objective).
    pub sinfo: HashMap<String, Option<Unit>>,
    pub menus: OrdMap<Vec<(String, Vec<String>)>>,
    pub labels: HashSet<String>,
}

impl Spec {
    pub fn slot_type(&self, who: &str, key: &str) -> Option<&Ty> {
        self.slots.iter().find(|((w, k), _)| w == who && k == key).map(|(_, t)| t)
    }
}

// ---- substitution while a template body is copied --------------------------------------------

#[derive(Default)]
struct Scope {
    prefix: String,
    cells: HashSet<String>,
    units: HashSet<String>,
    procs: HashSet<String>,
    insts: HashSet<String>,
    sub: HashMap<String, Expr>,
    line: Option<u32>,
}

impl Scope {
    fn q(&self, q: &str, members: &HashSet<String>) -> String {
        let head = q.split('.').next().unwrap_or(q);
        if members.contains(head) || self.insts.contains(head) {
            format!("{}{}", self.prefix, q)
        } else {
            q.to_string()
        }
    }
    fn cell(&self, q: &str) -> String {
        self.q(q, &self.cells)
    }
    fn unit(&self, q: &str) -> String {
        self.q(q, &self.units)
    }
    fn proc(&self, q: &str) -> String {
        self.q(q, &self.procs)
    }
    fn local(&self, n: &str) -> String {
        format!("{}{}", self.prefix, n)
    }
    /// A position that needs a plain name (actor, asset, slot owner).
    fn name(&self, n: &str) -> R<String> {
        match self.sub.get(n) {
            Some(Expr::Name(q)) => Ok(q.clone()),
            Some(other) => Err(FeanError::new(
                Kind::Static,
                "W-Static",
                format!("parameter {n} must be given a name here, not {}", const_text(other)),
                self.line,
            )),
            None => Ok(n.to_string()),
        }
    }
    fn unit_ast(&self, unit: &UnitAst) -> R<UnitAst> {
        unit.iter().map(|(n, x)| Ok((self.name(n)?, *x))).collect()
    }
}

fn sx(e: &Expr, sc: &Scope) -> R<Expr> {
    let b = |x: &Expr| -> R<Box<Expr>> { Ok(Box::new(sx(x, sc)?)) };
    Ok(match e {
        Expr::Name(q) => match sc.sub.get(q) {
            Some(replacement) => replacement.clone(),
            None => Expr::Name(sc.cell(q)),
        },
        Expr::Int(_) | Expr::Time(_) | Expr::Bool(_) | Expr::Empty | Expr::Now | Expr::At(..) | Expr::Reach(_) => e.clone(),
        Expr::Qty(n, unit) => Expr::Qty(*n, sc.unit_ast(unit)?),
        Expr::Mem(field, who, slot) => Expr::Mem(*field, sc.name(who)?, slot.clone()),
        Expr::Bal(who, asset) => Expr::Bal(b(who)?, sc.name(asset)?),
        Expr::Post(tgt) => Expr::Post(starget(tgt, sc)?),
        Expr::K(who, inner) => Expr::K(sc.name(who)?, b(inner)?),
        Expr::Call(f, args) => Expr::Call(f.clone(), args.iter().map(|a| sx(a, sc)).collect::<R<_>>()?),
        Expr::Bin(op, x, y) => Expr::Bin(*op, b(x)?, b(y)?),
        Expr::Not(x) => Expr::Not(b(x)?),
        Expr::Neg(x) => Expr::Neg(b(x)?),
        Expr::If(c, x, y) => Expr::If(b(c)?, b(x)?, b(y)?),
        Expr::Div(mode, x, y) => Expr::Div(*mode, b(x)?, b(y)?),
        Expr::MinMax(is_min, x, y) => Expr::MinMax(*is_min, b(x)?, b(y)?),
        Expr::Known(x) => Expr::Known(b(x)?),
        Expr::Default(x, y) => Expr::Default(b(x)?, b(y)?),
    })
}

fn sxs(list: &[Expr], sc: &Scope) -> R<Vec<Expr>> {
    list.iter().map(|a| sx(a, sc)).collect()
}

fn starget(t: &Target, sc: &Scope) -> R<Target> {
    Ok(match t {
        Target::Bal(who, asset) => Target::Bal(sc.name(who)?, sc.name(asset)?),
        Target::Cell(q) => Target::Cell(sc.cell(q)),
    })
}

fn sclause(c: &Clause, sc: &Scope) -> R<Clause> {
    Ok(match c {
        Clause::Auth(e) => Clause::Auth(sx(e, sc)?),
        Clause::Require(e) => Clause::Require(sx(e, sc)?),
        Clause::Ensure(e) => Clause::Ensure(sx(e, sc)?),
        Clause::Given(x, e) => Clause::Given(x.clone(), sx(e, sc)?),
        Clause::Let(x, e) => Clause::Let(x.clone(), sx(e, sc)?),
        Clause::Set(t, e) => Clause::Set(starget(t, sc)?, sx(e, sc)?),
        Clause::Delta(t, sign, e) => Clause::Delta(starget(t, sc)?, *sign, sx(e, sc)?),
        Clause::Free(t) => Clause::Free(starget(t, sc)?),
        Clause::Move(a, b, n, _) => Clause::Move(sx(a, sc)?, sx(b, sc)?, sx(n, sc)?, RefCell::new(None)),
        Clause::Tell(who, slot, v, u) => Clause::Tell(sc.name(who)?, slot.clone(), sx(v, sc)?, sx(u, sc)?),
        Clause::Forget(who, slot) => Clause::Forget(sc.name(who)?, slot.clone()),
    })
}

fn sunit(u: &UnitRef, sc: &Scope) -> R<UnitRef> {
    Ok(match u {
        UnitRef::Batch(q) => UnitRef::Batch(sc.unit(q)),
        UnitRef::Ev(q, args) => UnitRef::Ev(sc.unit(q), sxs(args, sc)?),
    })
}

fn sblock(block: &[Stmt], sc: &Scope) -> R<Vec<Stmt>> {
    block.iter().map(|s| sstmt(s, sc)).collect()
}

fn sstmt(s: &Stmt, sc: &Scope) -> R<Stmt> {
    let kind = match &s.kind {
        StmtKind::Run(u) => StmtKind::Run(sunit(u, sc)?),
        StmtKind::Attempt(u, yes, no) => StmtKind::Attempt(sunit(u, sc)?, sblock(yes, sc)?, sblock(no, sc)?),
        StmtKind::At(t, blk) => StmtKind::At(sx(t, sc)?, sblock(blk, sc)?),
        StmtKind::Alt(blocks) => StmtKind::Alt(blocks.iter().map(|b| sblock(b, sc)).collect::<R<_>>()?),
        StmtKind::Choose(who, menu, actions) => {
            let mut out = Vec::new();
            for a in actions {
                let param = match &a.param {
                    Some((x, ActionVals::Range(lo, hi))) => Some((x.clone(), ActionVals::Range(sx(lo, sc)?, sx(hi, sc)?))),
                    Some((x, ActionVals::List(vals))) => Some((x.clone(), ActionVals::List(sxs(vals, sc)?))),
                    None => None,
                };
                out.push(Action { label: a.label.clone(), param, body: sblock(&a.body, sc)? });
            }
            StmtKind::Choose(sc.name(who)?, sc.local(menu), out)
        }
        StmtKind::Do(q, args) => StmtKind::Do(sc.proc(q), sxs(args, sc)?),
        StmtKind::Label(l) => StmtKind::Label(sc.local(l)),
    };
    Ok(Stmt { kind, loc: s.loc })
}

fn stype(t: &TyAst, sc: &Scope) -> R<TyAst> {
    Ok(match t {
        TyAst::Opt(inner) => TyAst::Opt(Box::new(stype(inner, sc)?)),
        TyAst::Qty(unit) => TyAst::Qty(sc.unit_ast(unit)?),
        other => other.clone(),
    })
}

/// Names bound by parameterised actions anywhere inside a block.
fn action_params(block: &[Stmt]) -> Vec<String> {
    let mut out = Vec::new();
    for st in block {
        match &st.kind {
            StmtKind::Attempt(_, yes, no) => {
                out.extend(action_params(yes));
                out.extend(action_params(no));
            }
            StmtKind::At(_, blk) => out.extend(action_params(blk)),
            StmtKind::Alt(blocks) => blocks.iter().for_each(|b| out.extend(action_params(b))),
            StmtKind::Choose(_, _, actions) => {
                for a in actions {
                    if let Some((x, _)) = &a.param {
                        out.push(x.clone());
                    }
                    out.extend(action_params(&a.body));
                }
            }
            _ => {}
        }
    }
    out
}

/// Statements copied from the standard contracts are shown at the line that called them:
/// the model text is what the reader can open (statement lines are not used for error lines).
fn relocate(block: &mut [Stmt], site: Loc) {
    for st in block {
        if st.loc.origin == Origin::Prelude {
            st.loc = site;
        }
        match &mut st.kind {
            StmtKind::Attempt(_, yes, no) => {
                relocate(yes, site);
                relocate(no, site);
            }
            StmtKind::At(_, blk) => relocate(blk, site),
            StmtKind::Alt(blocks) => blocks.iter_mut().for_each(|b| relocate(b, site)),
            StmtKind::Choose(_, _, actions) => actions.iter_mut().for_each(|a| relocate(&mut a.body, site)),
            _ => {}
        }
    }
}

fn calls_in(e: &Expr, out: &mut HashSet<String>) {
    let both = |a: &Expr, b: &Expr, out: &mut HashSet<String>| {
        calls_in(a, out);
        calls_in(b, out);
    };
    match e {
        Expr::Call(f, args) => {
            out.insert(f.clone());
            args.iter().for_each(|a| calls_in(a, out));
        }
        Expr::Bin(_, a, b) | Expr::Div(_, a, b) | Expr::MinMax(_, a, b) | Expr::Default(a, b) => both(a, b, out),
        Expr::If(c, a, b) => {
            calls_in(c, out);
            both(a, b, out);
        }
        Expr::Not(a) | Expr::Neg(a) | Expr::Known(a) | Expr::Bal(a, _) | Expr::K(_, a) | Expr::At(_, a, _) => calls_in(a, out),
        _ => {}
    }
}

// ---- the builder -----------------------------------------------------------------------------

type Env = HashMap<String, Ty>;

struct ProcTmpl {
    params: Params,
    body: Vec<Stmt>,
}

struct ContractTmpl {
    params: Params,
    members: Vec<Decl>,
}

struct Builder {
    name: String,
    line: Cell<Option<u32>>,
    loc: Cell<Loc>,
    exprnames: HashMap<String, String>,
    actors: Vec<String>,
    actor_loc: HashMap<String, Loc>,
    assets: OrdMap<AssetDecl>,
    enums: OrdMap<Vec<String>>,
    enum_of: HashMap<String, String>,
    time: (i64, i64),
    params: OrdMap<(Ty, Vec<PVal>)>,
    param_loc: HashMap<String, Loc>,
    unknowns: OrdMap<(Ty, i64, i64)>,
    unknown_loc: HashMap<String, Loc>,
    solves: OrdMap<Solve>,
    solve_loc: HashMap<String, Loc>,
    sinfo: HashMap<String, Option<Unit>>,
    cells: OrdMap<CellDecl>,
    slots: Vec<((String, String), Ty)>,
    slot_loc: HashMap<(String, String), Loc>,
    assumes: Vec<Expr>,
    invariants: Vec<Expr>,
    defs: OrdMap<DefDecl>,
    events: OrdMap<EventDecl>,
    batches: OrdMap<BatchDecl>,
    contracts: OrdMap<ContractTmpl>,
    procs: OrdMap<ProcTmpl>,
    raw_program: Option<(Vec<Stmt>, Loc)>,
    program: Vec<Stmt>,
    weights: OrdMap<Expr>,
    values: OrdMap<Expr>,
    queries: OrdMap<Query>,
    query_loc: HashMap<String, Loc>,
    qinfo: HashMap<String, QInfo>,
    menus: RefCell<OrdMap<Vec<(String, Vec<String>)>>>,
    labels: RefCell<HashSet<String>>,
    value_types: RefCell<HashMap<String, Ty>>,
    value_busy: RefCell<HashSet<String>>,
    const_defs: RefCell<HashSet<String>>,
}

const LAYERS: &[(char, &str)] = &[('C', "constant"), ('S', "state"), ('P', "post-state"), ('Q', "point"), ('R', "run"),
    ('V', "goal"), ('X', "point-goal")];

impl Builder {
    fn err<T>(&self, code: &str, msg: impl Into<String>) -> R<T> {
        Err(FeanError::new(Kind::Static, code, msg, self.line.get()))
    }

    fn at_decl(&self, loc: Loc) {
        self.line.set(Some(loc.line));
        self.loc.set(loc);
    }

    fn top(&self) -> Scope {
        Scope { line: self.line.get(), ..Scope::default() }
    }

    fn declare(&mut self, name: &str, kind: &str) -> R<()> {
        if let Some(prev) = self.exprnames.get(name) {
            return self.err("W-Name", format!("{name} is already declared as {prev}"));
        }
        self.exprnames.insert(name.to_string(), kind.to_string());
        Ok(())
    }

    fn unique(&self, taken: bool, name: &str, kind: &str) -> R<()> {
        if taken {
            return self.err("W-Name", format!("{kind} {name} is declared twice"));
        }
        Ok(())
    }

    // -- passes 1 and 2: collect and expand
    fn build(mut self, decls: Vec<Decl>) -> R<Spec> {
        let mut later = Vec::new();
        for d in decls {
            self.at_decl(d.loc);
            match d.kind {
                DeclKind::Actor(names) => {
                    for n in names {
                        self.declare(&n, "an actor")?;
                        self.actor_loc.insert(n.clone(), d.loc);
                        self.actors.push(n);
                    }
                }
                DeclKind::Asset { name, lo, hi, init, public } => {
                    self.unique(self.assets.contains(&name), &name, "asset")?;
                    let mut by_actor = OrdMap::default();
                    for (who, vals) in init {
                        by_actor.insert(&who, vals);
                    }
                    self.assets.insert(&name, AssetDecl { lo, hi, init: by_actor, public, loc: d.loc });
                }
                DeclKind::Enum { name, values } => {
                    self.unique(self.enums.contains(&name), &name, "enum")?;
                    self.enums.insert(&name, values.clone());
                    for v in values {
                        self.declare(&v, &format!("a value of {name}"))?;
                        self.enum_of.insert(v, name.clone());
                    }
                }
                DeclKind::Time(lo, hi) => {
                    self.time = (lo, hi);
                    if lo > hi {
                        return self.err("W-Clock", "the time range is empty");
                    }
                }
                DeclKind::Contract { name, params, members } => {
                    self.unique(self.contracts.contains(&name), &name, "contract")?;
                    self.contracts.insert(&name, ContractTmpl { params, members });
                }
                kind => later.push(Decl { kind, loc: d.loc }),
            }
        }
        for d in &later {
            // parameters and functions before their users
            self.at_decl(d.loc);
            match &d.kind {
                DeclKind::Param { name, ty, lo, hi } => {
                    self.declare(name, "a parameter")?;
                    let resolved = self.rtype(ty)?;
                    let domain = self.param_domain(name, ty, lo.as_ref(), hi.as_ref())?;
                    self.params.insert(name, (resolved, domain));
                    self.param_loc.insert(name.clone(), d.loc);
                }
                DeclKind::Unknown { name, ty, lo, hi } => {
                    self.declare(name, "an unknown")?;
                    let resolved = self.rtype(ty)?;
                    let (least, greatest) = self.unknown_range(name, &resolved, lo.as_ref(), hi.as_ref())?;
                    self.unknowns.insert(name, (resolved, least, greatest));
                    self.unknown_loc.insert(name.clone(), d.loc);
                }
                DeclKind::Def { name, params, ty, body } => {
                    self.unique(self.defs.contains(name), name, "def")?;
                    let mut typed = Vec::new();
                    for (p, t) in params {
                        typed.push((p.clone(), self.rtype(t)?));
                    }
                    let result = self.rtype(ty)?;
                    self.defs.insert(name, DefDecl { params: typed, result, body: body.clone() });
                }
                _ => {}
            }
        }
        for d in later {
            self.at_decl(d.loc);
            match d.kind {
                DeclKind::Cell { .. } | DeclKind::Event { .. } | DeclKind::Batch { .. } | DeclKind::Proc { .. }
                | DeclKind::Instance { .. } => {
                    let sc = self.top();
                    self.add_member(&d, &sc, "", &[])?;
                }
                DeclKind::Slot { name, ty, owners } => {
                    let resolved = self.rtype(&ty)?;
                    if is_opt(&resolved) || resolved == Ty::Asset {
                        return self.err("E-Tell", format!("slot {name} needs a plain value type"));
                    }
                    for who in owners {
                        if !self.actors.contains(&who) {
                            return self.err("T-Name", format!("unknown actor {who}"));
                        }
                        let key = (who, name.clone());
                        self.unique(self.slots.iter().any(|(k, _)| *k == key), &format!("{key:?}"), "slot")?;
                        self.slot_loc.insert(key.clone(), d.loc);
                        self.slots.push((key, resolved.clone()));
                    }
                }
                DeclKind::Assume(e) => self.assumes.push(e),
                DeclKind::Invariant(e) => self.invariants.push(e),
                DeclKind::Program(body) => {
                    if self.raw_program.is_some() {
                        return self.err("W-Name", "program is declared twice");
                    }
                    self.raw_program = Some((body, d.loc));
                }
                DeclKind::Weight { name, expr } => {
                    self.unique(self.weights.contains(&name), &name, "weight")?;
                    self.weights.insert(&name, expr);
                }
                DeclKind::Value { name, expr } => {
                    self.unique(self.values.contains(&name), &name, "value")?;
                    self.declare(&name, "a value")?;
                    self.values.insert(&name, expr);
                }
                DeclKind::Query { name, query } => {
                    self.unique(self.queries.contains(&name), &name, "query")?;
                    self.unique(self.solves.contains(&name), &name, "solve")?;
                    self.queries.insert(&name, query);
                    self.query_loc.insert(name, d.loc);
                }
                DeclKind::Solve { name, solve } => {
                    self.unique(self.solves.contains(&name), &name, "solve")?;
                    self.unique(self.queries.contains(&name), &name, "query")?;
                    self.solves.insert(&name, solve);
                    self.solve_loc.insert(name, d.loc);
                }
                _ => {}
            }
        }
        self.check_all()?;
        Ok(Spec {
            name: self.name,
            actors: self.actors,
            actor_loc: self.actor_loc,
            assets: self.assets,
            enums: self.enums,
            enum_of: self.enum_of,
            time: self.time,
            params: self.params,
            param_loc: self.param_loc,
            unknowns: self.unknowns,
            unknown_loc: self.unknown_loc,
            solves: self.solves,
            solve_loc: self.solve_loc,
            sinfo: self.sinfo,
            cells: self.cells,
            slots: self.slots,
            slot_loc: self.slot_loc,
            assumes: self.assumes,
            invariants: self.invariants,
            defs: self.defs,
            events: self.events,
            batches: self.batches,
            program: self.program,
            weights: self.weights,
            values: self.values,
            queries: self.queries,
            query_loc: self.query_loc,
            qinfo: self.qinfo,
            menus: self.menus.into_inner(),
            labels: self.labels.into_inner(),
        })
    }

    fn rtype(&self, t: &TyAst) -> R<Ty> {
        Ok(match t {
            TyAst::Opt(inner) => opt(self.rtype(inner)?),
            TyAst::Qty(unit) => {
                for (n, _) in unit {
                    if !self.assets.contains(n) {
                        return self.err("T-Lit", format!("unknown asset {n} in a unit"));
                    }
                }
                Ty::Qty(unit_of(unit))
            }
            TyAst::Enum(name) => {
                if !self.enums.contains(name) {
                    return self.err("T-Name", format!("unknown type {name}"));
                }
                Ty::Enum(name.clone())
            }
            TyAst::Bool => Ty::Bool,
            TyAst::Time => Ty::Time,
            TyAst::Actor => Ty::Actor,
            TyAst::Asset => Ty::Asset,
        })
    }

    fn const_int(&self, node: Option<&Expr>, what: &str) -> R<i64> {
        match node {
            Some(Expr::Int(n)) => Ok(*n),
            Some(Expr::Neg(inner)) => match inner.as_ref() {
                Expr::Int(n) => Ok(-n),
                _ => self.err("W-Range", format!("{what} needs integer literal bounds")),
            },
            _ => self.err("W-Range", format!("{what} needs integer literal bounds")),
        }
    }

    fn param_domain(&self, name: &str, ty: &TyAst, lo: Option<&Expr>, hi: Option<&Expr>) -> R<Vec<PVal>> {
        let ty = self.rtype(ty)?;
        match &ty {
            Ty::Bool => Ok(vec![PVal::Bool(false), PVal::Bool(true)]),
            Ty::Enum(e) => Ok(self.enums.get(e).unwrap().iter().map(|v| PVal::Name(v.clone())).collect()),
            Ty::Time => Ok((self.time.0..=self.time.1).map(PVal::Int).collect()),
            Ty::Actor => Ok(self.actors.iter().map(|a| PVal::Name(a.clone())).collect()),
            Ty::Qty(_) => {
                if lo.is_none() {
                    return self.err("W-Range", format!("parameter {name} needs a range"));
                }
                let (lo, hi) = (self.const_int(lo, "a parameter")?, self.const_int(hi, "a parameter")?);
                if lo > hi {
                    return self.err("W-Range", format!("parameter {name} has an empty range"));
                }
                Ok((lo..=hi).map(PVal::Int).collect())
            }
            other => self.err("W-Range", format!("parameter {name} cannot have type {}", type_text(other))),
        }
    }

    /// The values an unknown ranges over, as numbers.
    fn unknown_range(&self, name: &str, ty: &Ty, lo: Option<&Expr>, hi: Option<&Expr>) -> R<(i64, i64)> {
        match ty {
            Ty::Bool => Ok((0, 1)),
            Ty::Enum(e) => Ok((0, self.enums.get(e).unwrap().len() as i64 - 1)),
            Ty::Time => Ok(self.time),
            Ty::Actor => {
                if self.actors.is_empty() {
                    return self.err("W-Range", format!("unknown {name} has no actor to range over"));
                }
                Ok((0, self.actors.len() as i64 - 1))
            }
            Ty::Qty(_) => {
                if lo.is_none() {
                    return self.err("W-Range", format!("unknown {name} needs a range"));
                }
                let (lo, hi) = (self.const_int(lo, "an unknown")?, self.const_int(hi, "an unknown")?);
                if lo > hi {
                    return self.err("W-Range", format!("unknown {name} has an empty range"));
                }
                Ok((lo, hi))
            }
            other => self.err("W-Range", format!("unknown {name} cannot have type {}", type_text(other))),
        }
    }

    fn check_arg(&self, arg: &Expr, want: &Ty, what: &str) -> R<()> {
        if *want == Ty::Asset {
            return match arg {
                Expr::Name(a) if self.assets.contains(a) => Ok(()),
                _ => self.err("P-Args", format!("{what} needs an asset name")),
            };
        }
        let got = self.typeof_(arg, 'C', &Env::new())?;
        if !fits(&got, want) {
            return self.err("P-Args", format!("{what} needs {}, got {}", type_text(want), type_text(&got)));
        }
        Ok(())
    }

    /// A name bound inside a template body must not hide what the copy substitutes or renames.
    fn binders_ok(&self, names: &[String], sc: &Scope) -> R<()> {
        for n in names {
            if sc.sub.contains_key(n) || sc.cells.contains(n) || sc.insts.contains(n) {
                return self.err("W-Name", format!("{n} hides a parameter or member of its template"));
            }
        }
        Ok(())
    }

    fn add_member(&mut self, m: &Decl, sc: &Scope, owner: &str, stack: &[String]) -> R<()> {
        let loc = self.loc.get();
        match &m.kind {
            DeclKind::Cell { name, ty, lo, hi, init, write, observe } => {
                let name = format!("{}{}", sc.prefix, name);
                self.declare(&name, "a cell")?;
                let observers = match observe {
                    Observe::All => Observers::All,
                    Observe::Some(list) => Observers::Some(list.iter().map(|a| sc.name(a)).collect::<R<_>>()?),
                };
                let cell = CellDecl {
                    name: name.clone(),
                    ty: self.rtype(&stype(ty, sc)?)?,
                    lo: match lo {
                        Some(e) => Some(sx(e, sc)?),
                        None => None,
                    },
                    hi: match hi {
                        Some(e) => Some(sx(e, sc)?),
                        None => None,
                    },
                    init: sxs(init, sc)?,
                    writers: write.iter().map(|w| sc.unit(w)).collect(),
                    owner: owner.to_string(),
                    observers,
                    loc,
                };
                self.cells.insert(&name, cell);
            }
            DeclKind::Event { name, params, by, net, clauses } => {
                let name = format!("{}{}", sc.prefix, name);
                self.unique(self.events.contains(&name), &name, "event")?;
                let mut binders: Vec<String> = params.iter().map(|(p, _)| p.clone()).collect();
                for c in clauses {
                    if let Clause::Let(x, _) | Clause::Given(x, _) = c {
                        binders.push(x.clone());
                    }
                }
                self.binders_ok(&binders, sc)?;
                let mut typed = Vec::new();
                for (p, t) in params {
                    typed.push((p.clone(), self.rtype(&stype(t, sc)?)?));
                }
                let event = EventDecl {
                    name: name.clone(),
                    params: typed,
                    by: sxs(by, sc)?,
                    net: *net,
                    clauses: clauses.iter().map(|c| sclause(c, sc)).collect::<R<_>>()?,
                    owner: owner.to_string(),
                    loc,
                    writes: Writes::default(),
                };
                self.events.insert(&name, event);
            }
            DeclKind::Batch { name, net, members } => {
                let name = format!("{}{}", sc.prefix, name);
                self.unique(self.batches.contains(&name), &name, "batch")?;
                let mut list = Vec::new();
                for (q, args) in members {
                    list.push((sc.unit(q), sxs(args, sc)?));
                }
                self.batches.insert(&name, BatchDecl { net: *net, members: list, loc });
            }
            DeclKind::Proc { name, params, body } => {
                let name = format!("{}{}", sc.prefix, name);
                self.unique(self.procs.contains(&name), &name, "proc")?;
                let mut binders: Vec<String> = params.iter().map(|(p, _)| p.clone()).collect();
                binders.extend(action_params(body));
                self.binders_ok(&binders, sc)?;
                let mut typed = Vec::new();
                for (p, t) in params {
                    typed.push((p.clone(), stype(t, sc)?));
                }
                self.procs.insert(&name, ProcTmpl { params: typed, body: sblock(body, sc)? });
            }
            DeclKind::Instance { name, contract, args } => {
                let args = sxs(args, sc)?;
                self.instantiate(&sc.prefix, name, contract, args, stack)?;
            }
            _ => unreachable!("only members reach add_member"),
        }
        Ok(())
    }

    fn instantiate(&mut self, prefix: &str, name: &str, cname: &str, args: Vec<Expr>, stack: &[String]) -> R<()> {
        if !self.contracts.contains(cname) {
            return self.err("P-Unit", format!("unknown contract {cname}"));
        }
        if stack.iter().any(|c| c == cname) {
            return self.err("W-Rec", format!("contract {cname} contains itself"));
        }
        let (params, members) = {
            let tmpl = self.contracts.get(cname).unwrap();
            (tmpl.params.clone(), tmpl.members.clone())
        };
        if args.len() != params.len() {
            return self.err("P-Args", format!("{cname} takes {} arguments, got {}", params.len(), args.len()));
        }
        let inst = format!("{prefix}{name}");
        self.declare(&inst, "an instance")?;
        let mut sc = Scope { prefix: format!("{inst}."), line: self.line.get(), ..Scope::default() };
        for m in &members {
            match &m.kind {
                DeclKind::Cell { name, .. } => {
                    sc.cells.insert(name.clone());
                }
                DeclKind::Event { name, .. } | DeclKind::Batch { name, .. } => {
                    sc.units.insert(name.clone());
                }
                DeclKind::Proc { name, .. } => {
                    sc.procs.insert(name.clone());
                }
                DeclKind::Instance { name, .. } => {
                    sc.insts.insert(name.clone());
                }
                _ => {}
            }
        }
        for ((pname, ptype), arg) in params.iter().zip(args) {
            let want = self.rtype(&stype(ptype, &sc)?)?;
            self.check_arg(&arg, &want, &format!("{cname} parameter {pname}"))?;
            sc.sub.insert(pname.clone(), arg);
        }
        let mut deeper = stack.to_vec();
        deeper.push(cname.to_string());
        for m in &members {
            self.add_member(m, &sc, &inst, &deeper)?;
        }
        Ok(())
    }

    fn inline(&self, block: &[Stmt], stack: &[String]) -> R<Vec<Stmt>> {
        let mut out = Vec::new();
        for st in block {
            let kind = match &st.kind {
                StmtKind::Do(q, args) => {
                    let Some(proc) = self.procs.get(q) else {
                        return self.err("P-Unit", format!("unknown proc {q}"));
                    };
                    if stack.iter().any(|p| p == q) {
                        return self.err("W-Rec", format!("proc {q} calls itself"));
                    }
                    if proc.params.len() != args.len() {
                        return self.err("P-Args", format!("{q} takes {} arguments, got {}", proc.params.len(), args.len()));
                    }
                    let mut sc = self.top();
                    for ((p, ptype), arg) in proc.params.iter().zip(args) {
                        let want = self.rtype(&stype(ptype, &sc)?)?;
                        self.check_arg(arg, &want, &format!("{q} parameter {p}"))?;
                        sc.sub.insert(p.clone(), arg.clone());
                    }
                    let mut deeper = stack.to_vec();
                    deeper.push(q.clone());
                    let mut body = self.inline(&sblock(&proc.body, &sc)?, &deeper)?;
                    relocate(&mut body, st.loc);
                    out.extend(body);
                    continue;
                }
                StmtKind::Attempt(u, yes, no) => StmtKind::Attempt(u.clone(), self.inline(yes, stack)?, self.inline(no, stack)?),
                StmtKind::At(t, blk) => StmtKind::At(t.clone(), self.inline(blk, stack)?),
                StmtKind::Alt(blocks) => {
                    StmtKind::Alt(blocks.iter().map(|b| self.inline(b, stack)).collect::<R<_>>()?)
                }
                StmtKind::Choose(who, menu, actions) => {
                    let mut arms = Vec::new();
                    for a in actions {
                        match &a.param {
                            None => arms.push(Action { label: a.label.clone(), param: None, body: self.inline(&a.body, stack)? }),
                            Some((x, vals)) => {
                                self.fresh(x, &Env::new())?;
                                for v in self.action_values(vals)? {
                                    let mut sc = self.top();
                                    let label = format!("{}({})", a.label, const_text(&v));
                                    sc.sub.insert(x.clone(), v);
                                    arms.push(Action { label, param: None, body: self.inline(&sblock(&a.body, &sc)?, stack)? });
                                }
                            }
                        }
                    }
                    StmtKind::Choose(who.clone(), menu.clone(), arms)
                }
                other => other.clone(),
            };
            out.push(Stmt { kind, loc: st.loc });
        }
        Ok(out)
    }

    fn action_values(&self, vals: &ActionVals) -> R<Vec<Expr>> {
        match vals {
            ActionVals::Range(lo, hi) => {
                let (lo, hi) = (self.const_int(Some(lo), "an action range")?, self.const_int(Some(hi), "an action range")?);
                Ok((lo..=hi).map(|n| if n >= 0 { Expr::Int(n) } else { Expr::Neg(Box::new(Expr::Int(-n))) }).collect())
            }
            ActionVals::List(list) => {
                for v in list {
                    let literal = matches!(v, Expr::Int(_) | Expr::Qty(..) | Expr::Time(_) | Expr::Bool(_))
                        || matches!(v, Expr::Neg(inner) if matches!(inner.as_ref(), Expr::Int(_)));
                    let named = matches!(v, Expr::Name(q) if self.enum_of.contains_key(q) || self.actors.contains(q));
                    if !(literal || named) {
                        return self.err("P-Choose", "action parameter values must be literals");
                    }
                }
                Ok(list.clone())
            }
        }
    }

    // -- pass 3: types
    fn layer(&self, mode: char, allowed: &str, what: &str) -> R<()> {
        if !allowed.contains(mode) {
            let name = LAYERS.iter().find(|(c, _)| *c == mode).map(|(_, n)| *n).unwrap_or("?");
            return self.err("T-Layer", format!("{what} cannot be used in a {name} expression"));
        }
        Ok(())
    }

    fn fresh(&self, name: &str, env: &Env) -> R<()> {
        if env.contains_key(name) || self.exprnames.contains_key(name) {
            return self.err("W-Name", format!("{name} hides an existing name"));
        }
        Ok(())
    }

    fn want_bool(&self, e: &Expr, mode: char, env: &Env, what: &str, code: &str) -> R<()> {
        let t = self.typeof_(e, mode, env)?;
        if t != Ty::Bool {
            return self.err(code, format!("{what} needs Bool, got {}", type_text(&t)));
        }
        Ok(())
    }

    fn value_type(&self, name: &str) -> R<Ty> {
        if let Some(t) = self.value_types.borrow().get(name) {
            return Ok(t.clone());
        }
        if self.value_busy.borrow().contains(name) {
            return self.err("W-Rec", format!("value {name} refers to itself"));
        }
        self.value_busy.borrow_mut().insert(name.to_string());
        let t = self.typeof_(self.values.get(name).unwrap(), 'R', &Env::new())?;
        self.value_busy.borrow_mut().remove(name);
        self.value_types.borrow_mut().insert(name.to_string(), t.clone());
        Ok(t)
    }

    fn point(&self, p: &Point) -> R<()> {
        match p {
            Point::Label(l) if !self.labels.borrow().contains(l) => self.err("Q-Point", format!("no label {l} in the program")),
            Point::Menu(m) if !self.menus.borrow().contains(m) => self.err("Q-Point", format!("no menu {m} in the program")),
            _ => Ok(()),
        }
    }

    fn typeof_(&self, e: &Expr, mode: char, env: &Env) -> R<Ty> {
        let text = type_text;
        match e {
            Expr::Int(_) => Ok(Ty::Qty(Vec::new())),
            Expr::Qty(_, unit) => {
                for (n, _) in unit {
                    if !self.assets.contains(n) {
                        return self.err("T-Lit", format!("unknown asset {n} in a unit"));
                    }
                }
                Ok(Ty::Qty(unit_of(unit)))
            }
            Expr::Time(n) => {
                if !(self.time.0 <= *n && *n <= self.time.1) {
                    return self.err("T-Lit", format!("@{n} is outside the declared time range"));
                }
                Ok(Ty::Time)
            }
            Expr::Bool(_) => Ok(Ty::Bool),
            Expr::Empty => Ok(Ty::Null),
            Expr::Now => {
                self.layer(mode, "SPQX", "now")?;
                Ok(Ty::Time)
            }
            Expr::Name(q) => {
                if let Some(t) = env.get(q) {
                    return Ok(t.clone());
                }
                if (mode == 'V' || mode == 'X') && self.queries.contains(q) {
                    // in a goal, a name is first a query: it stands for that query's verdict
                    return match (self.qinfo.get(q), self.queries.get(q)) {
                        (Some(QInfo::Bool), _) => Ok(Ty::Bool),
                        (Some(QInfo::Num(_)), Some(Query::Num { weight: Some(_), .. })) => {
                            self.err("Q-Solve", format!("query {q} is an expected value; a goal cannot use it"))
                        }
                        (Some(QInfo::Num(unit)), _) => Ok(Ty::Qty(unit.clone())),
                        _ => self.err("Q-Solve", format!("query {q} is a region; it has no single value")),
                    };
                }
                if let Some((t, _)) = self.params.get(q) {
                    return Ok(t.clone());
                }
                if let Some((t, _, _)) = self.unknowns.get(q) {
                    return Ok(t.clone());
                }
                if let Some(cell) = self.cells.get(q) {
                    self.layer(mode, "SPQX", &format!("cell {q}"))?;
                    return Ok(cell.ty.clone());
                }
                if let Some(en) = self.enum_of.get(q) {
                    return Ok(Ty::Enum(en.clone()));
                }
                if self.actors.contains(q) {
                    return Ok(Ty::Actor);
                }
                if self.values.contains(q) {
                    self.layer(mode, "R", &format!("value {q}"))?;
                    return self.value_type(q);
                }
                self.err("T-Name", format!("unknown name {q}"))
            }
            Expr::Bin(op, x, y) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    self.want_bool(x, mode, env, op.text(), "T-Bool")?;
                    self.want_bool(y, mode, env, op.text(), "T-Bool")?;
                    return Ok(Ty::Bool);
                }
                let (a, b) = (self.typeof_(x, mode, env)?, self.typeof_(y, mode, env)?);
                let shown = || format!("{} {} {}", text(&a), op.text(), text(&b));
                match op {
                    BinOp::Add | BinOp::Sub => {
                        if !matches!(a, Ty::Qty(_)) || a != b {
                            return self.err("T-Arith", shown());
                        }
                        Ok(a)
                    }
                    BinOp::Mul => match (&a, &b) {
                        (Ty::Qty(u), Ty::Qty(v)) => {
                            let mut all = u.clone();
                            all.extend(v.iter().cloned());
                            Ok(Ty::Qty(unit_of(&all)))
                        }
                        _ => self.err("T-Arith", shown()),
                    },
                    BinOp::Eq | BinOp::Ne => {
                        if let (Some(p), Some(q)) = (base(&a), base(&b)) {
                            if p != q {
                                return self.err("T-Eq", shown());
                            }
                        }
                        Ok(Ty::Bool)
                    }
                    _ => {
                        if a != b || !matches!(a, Ty::Qty(_) | Ty::Time) {
                            return self.err("T-Cmp", shown());
                        }
                        Ok(Ty::Bool)
                    }
                }
            }
            Expr::Not(x) => {
                self.want_bool(x, mode, env, "not", "T-Bool")?;
                Ok(Ty::Bool)
            }
            Expr::Neg(x) => {
                let a = self.typeof_(x, mode, env)?;
                if !matches!(a, Ty::Qty(_)) {
                    return self.err("T-Arith", format!("-{}", text(&a)));
                }
                Ok(a)
            }
            Expr::If(c, x, y) => {
                self.want_bool(c, mode, env, "if", "T-Bool")?;
                let (a, b) = (self.typeof_(x, mode, env)?, self.typeof_(y, mode, env)?);
                if a == Ty::Null || b == Ty::Null {
                    return Ok(opt(if a == Ty::Null { b } else { a }));
                }
                if base(&a) != base(&b) {
                    return self.err("T-If", format!("branches have types {} and {}", text(&a), text(&b)));
                }
                Ok(if is_opt(&a) || is_opt(&b) { opt(a) } else { a })
            }
            Expr::Div(dmode, x, y) => {
                let (a, b) = (self.typeof_(x, mode, env)?, self.typeof_(y, mode, env)?);
                let (Ty::Qty(u), Ty::Qty(v)) = (&a, &b) else {
                    return self.err("T-Arith", format!("{} / {}", text(&a), text(&b)));
                };
                let mut all = u.clone();
                all.extend(v.iter().map(|(n, x)| (n.clone(), -x)));
                let res = Ty::Qty(unit_of(&all));
                let literal = matches!(y.as_ref(), Expr::Int(n) | Expr::Qty(n, _) if *n != 0);
                Ok(if literal && *dmode != DivMode::Exact { res } else { opt(res) })
            }
            Expr::MinMax(is_min, x, y) => {
                let (a, b) = (self.typeof_(x, mode, env)?, self.typeof_(y, mode, env)?);
                if a != b || !matches!(a, Ty::Qty(_) | Ty::Time) {
                    let f = if *is_min { "min" } else { "max" };
                    return self.err("T-Arith", format!("{f}({}, {})", text(&a), text(&b)));
                }
                Ok(a)
            }
            Expr::Known(x) => {
                self.typeof_(x, mode, env)?;
                Ok(Ty::Bool)
            }
            Expr::Default(x, y) => {
                let (a, d) = (self.typeof_(x, mode, env)?, self.typeof_(y, mode, env)?);
                if d == Ty::Null || (a != Ty::Null && base(&a) != base(&d)) {
                    return self.err("T-Opt", format!("{} ?? {}", text(&a), text(&d)));
                }
                Ok(d)
            }
            Expr::Mem(field, who, slot) => {
                self.layer(mode, "SPQX", field.text())?;
                if !self.actors.contains(who) {
                    return self.err("W-Static", format!("{} needs an actor name, got {who}", field.text()));
                }
                let Some((_, ty)) = self.slots.iter().find(|((w, k), _)| w == who && k == slot) else {
                    return self.err("T-Mem", format!("no slot {slot} for {who}"));
                };
                Ok(opt(if *field == Field::Mem { ty.clone() } else { Ty::Time }))
            }
            Expr::Bal(who, asset) => {
                self.layer(mode, "SPQX", "bal")?;
                if self.typeof_(who, mode, env)? != Ty::Actor || !self.assets.contains(asset) {
                    return self.err("T-Bal", "bal needs an actor and a declared asset");
                }
                Ok(Ty::Qty(vec![(asset.clone(), 1)]))
            }
            Expr::Post(tgt) => {
                self.layer(mode, "P", "post")?;
                self.target_type(tgt)
            }
            Expr::K(who, inner) => {
                self.layer(mode, "Q", "K")?;
                if !self.actors.contains(who) {
                    return self.err("W-Static", format!("K needs an actor name, got {who}"));
                }
                self.want_bool(inner, 'Q', env, "K", "T-Bool")?;
                Ok(Ty::Bool)
            }
            Expr::At(point, inner, flag) => {
                self.layer(mode, "R", "at")?;
                self.point(point)?;
                let t = self.typeof_(inner, 'Q', &Env::new())?;
                flag.set(t == Ty::Bool);
                Ok(if t == Ty::Bool { Ty::Bool } else { opt(t) })
            }
            Expr::Reach(point) => {
                self.layer(mode, "R", "reach")?;
                self.point(point)?;
                Ok(Ty::Bool)
            }
            Expr::Call(f, args) => {
                let Some(def) = self.defs.get(f) else {
                    return self.err("T-Call", format!("unknown function {f}"));
                };
                if def.params.len() != args.len() {
                    return self.err("T-Call", format!("{f} takes {} arguments, got {}", def.params.len(), args.len()));
                }
                for ((p, want), arg) in def.params.iter().zip(args) {
                    let got = self.typeof_(arg, mode, env)?;
                    if !fits(&got, want) {
                        return self.err("T-Call", format!("{f} parameter {p} needs {}, got {}", text(want), text(&got)));
                    }
                }
                if "CRV".contains(mode) && !self.const_defs.borrow().contains(f) {
                    self.const_defs.borrow_mut().insert(f.clone());
                    let inner: Env = def.params.iter().cloned().collect();
                    self.typeof_(&def.body, 'C', &inner)?;
                }
                Ok(def.result.clone())
            }
        }
    }

    fn target_type(&self, tgt: &Target) -> R<Ty> {
        match tgt {
            Target::Bal(who, asset) => {
                if !self.actors.contains(who) {
                    return self.err("W-Static", format!("bal needs an actor name, got {who}"));
                }
                if !self.assets.contains(asset) {
                    return self.err("T-Bal", format!("unknown asset {asset}"));
                }
                Ok(Ty::Qty(vec![(asset.clone(), 1)]))
            }
            Target::Cell(q) => match self.cells.get(q) {
                Some(cell) => Ok(cell.ty.clone()),
                None => {
                    let code = if self.exprnames.contains_key(q) { "W-Static" } else { "T-Name" };
                    self.err(code, format!("{q} is not a cell"))
                }
            },
        }
    }

    // -- pass 3: declarations
    fn check_all(&mut self) -> R<()> {
        let none = Env::new();
        let num = Ty::Qty(Vec::new());
        for (x, a) in self.assets.iter() {
            self.at_decl(a.loc);
            for bound in [&a.lo, &a.hi] {
                if self.typeof_(bound, 'C', &none)? != num {
                    return self.err("W-Range", format!("asset {x} needs integer bounds"));
                }
            }
            for (who, vals) in a.init.iter() {
                if !self.actors.contains(who) {
                    return self.err("T-Name", format!("unknown actor {who}"));
                }
                for v in vals {
                    if self.typeof_(v, 'C', &none)? != num {
                        return self.err("W-Range", format!("initial holding of {who} must be an integer count of {x}"));
                    }
                }
            }
        }
        for c in self.cells.values() {
            self.at_decl(c.loc);
            let bt = base(&c.ty).cloned();
            if bt == Some(Ty::Asset) {
                return self.err("E-Set", format!("cell {} cannot hold an asset", c.name));
            }
            let quantity = matches!(bt, Some(Ty::Qty(_)));
            if quantity {
                let (Some(lo), Some(hi)) = (&c.lo, &c.hi) else {
                    return self.err("W-Range", format!("quantity cell {} needs a range", c.name));
                };
                for bound in [lo, hi] {
                    let t = self.typeof_(bound, 'C', &none)?;
                    if t != num && Some(&t) != bt.as_ref() {
                        return self.err("W-Range", format!("range of {} must be in its own unit", c.name));
                    }
                }
            } else if c.lo.is_some() {
                return self.err("W-Range", format!("only quantity cells have a range ({})", c.name));
            }
            for v in &c.init {
                let got = self.typeof_(v, 'C', &none)?;
                if !(fits(&got, &c.ty) || (quantity && got == num)) {
                    return self.err(
                        "E-Set",
                        format!("initial value of {} needs {}, got {}", c.name, type_text(&c.ty), type_text(&got)),
                    );
                }
            }
            if let Observers::Some(list) = &c.observers {
                for who in list {
                    if !self.actors.contains(who) {
                        return self.err("T-Name", format!("unknown observer {who} of {}", c.name));
                    }
                }
            }
            for w in &c.writers {
                if !self.events.contains(w) {
                    return self.err("P-Unit", format!("unknown writer {w} of {}", c.name));
                }
            }
        }
        let mut graph: Vec<(String, HashSet<String>)> = Vec::new();
        for (f, def) in self.defs.iter() {
            let mut env = Env::new();
            for (p, t) in &def.params {
                if *t == Ty::Asset {
                    return self.err("P-Args", format!("def parameter {p} cannot be an asset"));
                }
                self.fresh(p, &env)?;
                env.insert(p.clone(), t.clone());
            }
            let got = self.typeof_(&def.body, 'S', &env)?;
            if !fits(&got, &def.result) {
                return self.err("T-Call", format!("{f} returns {}, declared {}", type_text(&got), type_text(&def.result)));
            }
            let mut called = HashSet::new();
            calls_in(&def.body, &mut called);
            graph.push((f.clone(), called));
        }
        for (f, direct) in &graph {
            let mut seen: HashSet<String> = HashSet::new();
            let mut todo: Vec<String> = direct.iter().cloned().collect();
            while let Some(g) = todo.pop() {
                if g == *f {
                    return self.err("W-Rec", format!("def {f} calls itself"));
                }
                if seen.insert(g.clone()) {
                    if let Some((_, more)) = graph.iter().find(|(name, _)| *name == g) {
                        todo.extend(more.iter().cloned());
                    }
                }
            }
        }
        for e in &self.assumes {
            self.want_bool(e, 'S', &none, "assume", "T-Bool")?;
        }
        for e in &self.invariants {
            self.want_bool(e, 'S', &none, "invariant", "T-Bool")?;
        }
        for i in 0..self.events.len() {
            let writes = self.check_event(&self.events.values()[i])?;
            self.events.values_mut()[i].writes = writes;
        }
        for (name, batch) in self.batches.iter() {
            self.at_decl(batch.loc);
            self.check_batch(name, &batch.members)?;
        }
        if let Some((body, loc)) = self.raw_program.take() {
            self.at_decl(loc);
            let program = self.inline(&sblock(&body, &self.top())?, &[])?;
            let mut seen = HashSet::new();
            self.check_block(&program, &mut seen)?;
            self.check_clock(&program, self.time.0)?;
            self.program = program;
        }
        for (name, e) in self.weights.iter() {
            if self.typeof_(e, 'S', &none)? != num {
                return self.err("Q-Num", format!("weight {name} needs Num"));
            }
        }
        for name in self.values.keys() {
            self.value_type(name)?;
        }
        let mut infos = Vec::new();
        for (name, q) in self.queries.iter() {
            self.at_decl(self.query_loc[name]);
            infos.push((name.clone(), self.check_query(q)?));
        }
        self.qinfo.extend(infos);
        let mut units = Vec::new();
        for (name, solve) in self.solves.iter() {
            self.at_decl(self.solve_loc[name]);
            units.push((name.clone(), self.check_solve(solve)?));
        }
        self.sinfo.extend(units);
        Ok(())
    }

    /// The unit of the objective (None when the solve has none).
    fn check_solve(&self, sv: &Solve) -> R<Option<Unit>> {
        let none = Env::new();
        let (mode, before) = match &sv.point {
            None => {
                if self.unknowns.is_empty() {
                    return self.err("Q-Solve", "nothing to solve for: declare an unknown, or name a point with at(...)");
                }
                ('V', 'C')
            }
            Some(label) => {
                self.point(&Point::Label(label.clone()))?;
                let times = count_labels(&self.program, label);
                if times != 1 {
                    return self.err("Q-Solve", format!("label {label} is written {times} times; a point must be one place"));
                }
                ('X', 'S')
            }
        };
        if let Some(filter) = &sv.filter {
            self.want_bool(filter, before, &none, "where", "Q-Solve")?;
        }
        let mut unit = None;
        if let Some(objective) = &sv.objective {
            match self.typeof_(objective, mode, &none)? {
                Ty::Qty(u) => unit = Some(u),
                other => {
                    return self.err("Q-Solve", format!("{} needs a quantity, got {}", sv.form.text(), type_text(&other)));
                }
            }
        }
        self.want_bool(&sv.goal, mode, &none, "the goal", "Q-Solve")?;
        Ok(unit)
    }

    fn write_target(&self, tgt: &Target, ev: &EventDecl, code: &str, allow_bal: bool) -> R<(WKey, Ty)> {
        let ty = self.target_type(tgt)?;
        match tgt {
            Target::Bal(who, asset) => {
                if !allow_bal {
                    return self.err(code, "a holding changes only by move, += or -=");
                }
                Ok((WKey::Bal(who.clone(), asset.clone()), ty))
            }
            Target::Cell(q) => {
                let cell = self.cells.get(q).unwrap();
                let allowed = if cell.owner.is_empty() { cell.writers.contains(&ev.name) } else { ev.owner == cell.owner };
                if !allowed {
                    return self.err("W-Writer", format!("{} may not write {}", ev.name, cell.name));
                }
                Ok((WKey::Cell(cell.name.clone()), ty))
            }
        }
    }

    fn check_event(&self, ev: &EventDecl) -> R<Writes> {
        self.at_decl(ev.loc);
        let mut env = Env::new();
        for (p, t) in &ev.params {
            if *t == Ty::Asset {
                return self.err("P-Args", format!("event parameter {p} cannot be an asset"));
            }
            self.fresh(p, &env)?;
            env.insert(p.clone(), t.clone());
        }
        for b in &ev.by {
            if self.typeof_(b, 'S', &env)? != Ty::Actor {
                return self.err("T-Name", format!("by needs an actor in {}", ev.name));
            }
        }
        let has_auth = ev.clauses.iter().any(|c| matches!(c, Clause::Auth(_)));
        let mut w = Writes::default();
        let twice = |key: &WKey| format!("{} is written twice in {}", key.text(), ev.name);
        for c in &ev.clauses {
            match c {
                Clause::Require(e) => self.want_bool(e, 'S', &env, "require", "T-Bool")?,
                Clause::Auth(e) => self.want_bool(e, 'S', &env, "auth", "T-Bool")?,
                Clause::Ensure(e) => self.want_bool(e, 'P', &env, "ensure", "T-Bool")?,
                Clause::Let(x, e) => {
                    self.fresh(x, &env)?;
                    let t = self.typeof_(e, 'S', &env)?;
                    env.insert(x.clone(), t);
                }
                Clause::Given(x, e) => {
                    self.fresh(x, &env)?;
                    let t = self.typeof_(e, 'S', &env)?;
                    match &t {
                        Ty::Opt(inner) => {
                            env.insert(x.clone(), inner.as_ref().clone());
                        }
                        _ => return self.err("T-Given", format!("given {x} needs an optional value, got {}", type_text(&t))),
                    }
                }
                Clause::Set(tgt, e) => {
                    let (key, ct) = self.write_target(tgt, ev, "E-Set", false)?;
                    let vt = self.typeof_(e, 'S', &env)?;
                    if !fits(&vt, &ct) {
                        return self.err("E-Set", format!("{} needs {}, got {}", key.text(), type_text(&ct), type_text(&vt)));
                    }
                    if w.sets.contains(&key) || w.deltas.contains(&key) {
                        return self.err("W-Write", twice(&key));
                    }
                    w.sets.insert(key);
                }
                Clause::Delta(tgt, _, e) => {
                    let (key, ct) = self.write_target(tgt, ev, "E-Delta", true)?;
                    let vt = self.typeof_(e, 'S', &env)?;
                    if !matches!(ct, Ty::Qty(_)) || vt != ct {
                        return self.err("E-Delta", format!("{} is {}, change is {}", key.text(), type_text(&ct), type_text(&vt)));
                    }
                    if w.sets.contains(&key) {
                        return self.err("W-Write", twice(&key));
                    }
                    if matches!(tgt, Target::Bal(..)) && !has_auth {
                        return self.err("W-Debit", format!("{} creates or destroys holdings without auth", ev.name));
                    }
                    w.deltas.insert(key);
                }
                Clause::Free(tgt) => {
                    let (key, _) = self.write_target(tgt, ev, "E-Free", false)?;
                    if w.sets.contains(&key) || w.deltas.contains(&key) {
                        return self.err("W-Write", twice(&key));
                    }
                    w.sets.insert(key);
                }
                Clause::Move(a, b, n, asset) => {
                    if self.typeof_(a, 'S', &env)? != Ty::Actor || self.typeof_(b, 'S', &env)? != Ty::Actor {
                        return self.err("E-Move", "move needs two actors");
                    }
                    let vt = self.typeof_(n, 'S', &env)?;
                    let single = match &vt {
                        Ty::Qty(u) if u.len() == 1 && u[0].1 == 1 => Some(u[0].0.clone()),
                        _ => None,
                    };
                    let Some(x) = single else {
                        return self.err("E-Move", format!("move needs a quantity of one asset, got {}", type_text(&vt)));
                    };
                    if !has_auth && !ev.by.contains(a) {
                        return self.err("W-Debit", format!("{} debits an actor who is not its principal, without auth", ev.name));
                    }
                    *asset.borrow_mut() = Some(x);
                }
                Clause::Tell(who, slot, ..) | Clause::Forget(who, slot) => {
                    let kind = if matches!(c, Clause::Tell(..)) { "tell" } else { "forget" };
                    if !self.actors.contains(who) {
                        return self.err("W-Static", format!("{kind} needs an actor name, got {who}"));
                    }
                    let Some((_, slot_ty)) = self.slots.iter().find(|((w2, k), _)| w2 == who && k == slot) else {
                        return self.err("E-Tell", format!("no slot {slot} for {who}"));
                    };
                    if let Clause::Tell(_, _, v, u) = c {
                        let vt = self.typeof_(v, 'S', &env)?;
                        if is_opt(&vt) || !fits(&vt, slot_ty) {
                            return self.err(
                                "E-Tell",
                                format!("{who}.{slot} needs {}, got {}", type_text(slot_ty), type_text(&vt)),
                            );
                        }
                        if self.typeof_(u, 'S', &env)? != Ty::Time {
                            return self.err("E-Tell", "asof needs a Time");
                        }
                    }
                    let key = (who.clone(), slot.clone());
                    if w.slots.contains(&key) {
                        return self.err("W-Write", format!("{who}.{slot} is written twice in {}", ev.name));
                    }
                    w.slots.insert(key);
                }
            }
        }
        Ok(w)
    }

    fn check_args(&self, ev: &EventDecl, args: &[Expr]) -> R<()> {
        if args.len() != ev.params.len() {
            return self.err("P-Args", format!("{} takes {} arguments, got {}", ev.name, ev.params.len(), args.len()));
        }
        for ((p, want), arg) in ev.params.iter().zip(args) {
            self.check_arg(arg, want, &format!("{} parameter {p}", ev.name))?;
        }
        Ok(())
    }

    fn check_batch(&self, name: &str, members: &[(String, Vec<Expr>)]) -> R<()> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut all = Writes::default();
        for (q, args) in members {
            let Some(ev) = self.events.get(q) else {
                return self.err("P-Unit", format!("unknown event {q} in batch {name}"));
            };
            if !seen.insert(q.as_str()) {
                return self.err("W-Batch", format!("{q} appears twice in batch {name}"));
            }
            self.check_args(ev, args)?;
            let w = &ev.writes;
            let clash = w.sets.iter().any(|k| all.sets.contains(k) || all.deltas.contains(k))
                || w.deltas.iter().any(|k| all.sets.contains(k))
                || w.slots.iter().any(|k| all.slots.contains(k));
            if clash {
                return self.err("W-Write", format!("batch {name} writes the same target twice"));
            }
            all.sets.extend(w.sets.iter().cloned());
            all.deltas.extend(w.deltas.iter().cloned());
            all.slots.extend(w.slots.iter().cloned());
        }
        Ok(())
    }

    fn check_unit(&self, u: &UnitRef) -> R<()> {
        match u {
            UnitRef::Batch(q) => {
                if !self.batches.contains(q) {
                    return self.err("W-Batch", format!("batch {q} is not declared"));
                }
                Ok(())
            }
            UnitRef::Ev(q, args) => match self.events.get(q) {
                Some(ev) => self.check_args(ev, args),
                None => self.err("P-Unit", format!("unknown event {q}")),
            },
        }
    }

    fn branches(&self, blocks: &[&[Stmt]], seen: &mut HashSet<(bool, String)>) -> R<()> {
        let mut acc = HashSet::new();
        for b in blocks {
            let mut mine = seen.clone();
            self.check_block(b, &mut mine)?;
            acc.extend(mine);
        }
        seen.extend(acc);
        Ok(())
    }

    /// `seen` holds the menu (true) and label (false) names already used on every path into this block.
    fn check_block(&self, block: &[Stmt], seen: &mut HashSet<(bool, String)>) -> R<()> {
        for st in block {
            match &st.kind {
                StmtKind::Run(u) => self.check_unit(u)?,
                StmtKind::Attempt(u, yes, no) => {
                    self.check_unit(u)?;
                    self.branches(&[yes.as_slice(), no.as_slice()], seen)?;
                }
                StmtKind::At(t, blk) => {
                    if self.typeof_(t, 'C', &Env::new())? != Ty::Time {
                        return self.err("W-Clock", "at needs a Time constant");
                    }
                    self.check_block(blk, seen)?;
                }
                StmtKind::Alt(blocks) => {
                    let list: Vec<&[Stmt]> = blocks.iter().map(|b| b.as_slice()).collect();
                    self.branches(&list, seen)?;
                }
                StmtKind::Choose(who, menu, actions) => {
                    if !self.actors.contains(who) {
                        return self.err("W-Static", format!("choose needs an actor name, got {who}"));
                    }
                    let labels: Vec<String> = actions.iter().map(|a| a.label.clone()).collect();
                    if labels.iter().collect::<HashSet<_>>().len() != labels.len() {
                        return self.err("P-Choose", format!("menu {menu} repeats an action name"));
                    }
                    if seen.contains(&(true, menu.clone())) {
                        return self.err("W-Menu", format!("menu {menu} appears twice on one path"));
                    }
                    seen.insert((true, menu.clone()));
                    {
                        let mut menus = self.menus.borrow_mut();
                        let mut list = menus.get(menu).cloned().unwrap_or_default();
                        list.push((who.clone(), labels));
                        menus.insert(menu, list);
                    }
                    let list: Vec<&[Stmt]> = actions.iter().map(|a| a.body.as_slice()).collect();
                    self.branches(&list, seen)?;
                }
                StmtKind::Label(l) => {
                    if seen.contains(&(false, l.clone())) {
                        return self.err("W-Menu", format!("label {l} appears twice on one path"));
                    }
                    seen.insert((false, l.clone()));
                    self.labels.borrow_mut().insert(l.clone());
                }
                StmtKind::Do(..) => {}
            }
        }
        Ok(())
    }

    /// Latest clock any path can reach; literal `at` times must not go back.
    fn check_clock(&self, block: &[Stmt], mut clock: i64) -> R<i64> {
        for st in block {
            match &st.kind {
                StmtKind::At(t, blk) => {
                    if let Expr::Time(n) = t {
                        if *n < clock {
                            return self.err("W-Clock", format!("at @{n} goes back from @{clock}"));
                        }
                        clock = *n;
                    }
                    clock = self.check_clock(blk, clock)?;
                }
                StmtKind::Attempt(_, yes, no) => {
                    clock = self.check_clock(yes, clock)?.max(self.check_clock(no, clock)?);
                }
                StmtKind::Alt(blocks) => {
                    let mut latest = i64::MIN;
                    for b in blocks {
                        latest = latest.max(self.check_clock(b, clock)?);
                    }
                    clock = latest;
                }
                StmtKind::Choose(_, _, actions) => {
                    let mut latest = i64::MIN;
                    for a in actions {
                        latest = latest.max(self.check_clock(&a.body, clock)?);
                    }
                    clock = latest;
                }
                _ => {}
            }
        }
        Ok(clock)
    }

    fn check_query(&self, q: &Query) -> R<QInfo> {
        let none = Env::new();
        let actors_ok = |list: &[String]| -> R<()> {
            for who in list {
                if !self.actors.contains(who) {
                    return self.err("T-Name", format!("unknown actor {who}"));
                }
            }
            Ok(())
        };
        match q {
            Query::Exists(e) => {
                self.want_bool(e, 'R', &none, "exists", "Q-Bool")?;
                Ok(QInfo::Bool)
            }
            Query::Forall(e) => {
                self.want_bool(e, 'R', &none, "forall", "Q-Bool")?;
                Ok(QInfo::Bool)
            }
            Query::Can(who, e) => {
                actors_ok(who)?;
                self.want_bool(e, 'R', &none, "can", "Q-Bool")?;
                Ok(QInfo::Bool)
            }
            Query::Num { policy, weight, max, expr } => {
                if let Some(who) = policy {
                    actors_ok(who)?;
                }
                if let Some(w) = weight {
                    if !self.weights.contains(w) {
                        return self.err("Q-Num", format!("unknown weight {w}"));
                    }
                }
                match self.typeof_(expr, 'R', &none)? {
                    Ty::Qty(unit) => Ok(QInfo::Num(unit)),
                    other => {
                        let f = if *max { "max" } else { "min" };
                        self.err("Q-Num", format!("{f} run needs a quantity, got {}", type_text(&other)))
                    }
                }
            }
            Query::Region(params, inner) => {
                for p in params {
                    if !self.params.contains(p) {
                        return self.err("Q-Region", format!("{p} is not a parameter"));
                    }
                }
                if self.check_query(inner)? != QInfo::Bool {
                    return self.err("Q-Region", "region needs a true/false query inside");
                }
                Ok(QInfo::Region(params.clone()))
            }
            Query::Prefer { who, point, a, b, value, weight } => {
                actors_ok(std::slice::from_ref(who))?;
                let menus = self.menus.borrow();
                let offered: HashSet<&String> = match point {
                    Point::Menu(m) if menus.contains(m) => menus.get(m).unwrap().iter().flat_map(|(_, l)| l.iter()).collect(),
                    _ => return self.err("Q-Prefer", "prefer needs a menu(m) point that exists in the program"),
                };
                if !offered.contains(a) || !offered.contains(b) {
                    let Point::Menu(m) = point else { unreachable!() };
                    return self.err("Q-Prefer", format!("{a} and {b} must be actions of menu {m}"));
                }
                drop(menus);
                if !self.values.contains(value) || !matches!(self.value_type(value)?, Ty::Qty(_)) {
                    return self.err("Q-Prefer", format!("{value} must be a declared numeric value"));
                }
                if let Some(w) = weight {
                    if !self.weights.contains(w) {
                        return self.err("Q-Prefer", format!("unknown weight {w}"));
                    }
                }
                Ok(QInfo::Bool)
            }
        }
    }
}

/// How many `label` statements with this name the program has.
fn count_labels(block: &[Stmt], label: &str) -> usize {
    let mut n = 0;
    let mut stack: Vec<&[Stmt]> = vec![block];
    while let Some(stmts) = stack.pop() {
        for st in stmts {
            match &st.kind {
                StmtKind::Label(l) => n += (l == label) as usize,
                StmtKind::Attempt(_, a, b) => {
                    stack.push(a);
                    stack.push(b);
                }
                StmtKind::At(_, body) => stack.push(body),
                StmtKind::Alt(blocks) => stack.extend(blocks.iter().map(|b| b.as_slice())),
                StmtKind::Choose(_, _, actions) => stack.extend(actions.iter().map(|a| a.body.as_slice())),
                _ => {}
            }
        }
    }
    n
}

/// Expand and check a parsed model.
pub fn build(name: &str, decls: Vec<Decl>) -> R<Spec> {
    let builder = Builder {
        name: name.to_string(),
        line: Cell::new(None),
        loc: Cell::new(Loc { origin: Origin::Model, line: 0 }),
        exprnames: HashMap::new(),
        actors: Vec::new(),
        actor_loc: HashMap::new(),
        assets: OrdMap::default(),
        enums: OrdMap::default(),
        enum_of: HashMap::new(),
        time: (0, 0),
        params: OrdMap::default(),
        param_loc: HashMap::new(),
        unknowns: OrdMap::default(),
        unknown_loc: HashMap::new(),
        solves: OrdMap::default(),
        solve_loc: HashMap::new(),
        sinfo: HashMap::new(),
        cells: OrdMap::default(),
        slots: Vec::new(),
        slot_loc: HashMap::new(),
        assumes: Vec::new(),
        invariants: Vec::new(),
        defs: OrdMap::default(),
        events: OrdMap::default(),
        batches: OrdMap::default(),
        contracts: OrdMap::default(),
        procs: OrdMap::default(),
        raw_program: None,
        program: Vec::new(),
        weights: OrdMap::default(),
        values: OrdMap::default(),
        queries: OrdMap::default(),
        query_loc: HashMap::new(),
        qinfo: HashMap::new(),
        menus: RefCell::new(OrdMap::default()),
        labels: RefCell::new(HashSet::new()),
        value_types: RefCell::new(HashMap::new()),
        value_busy: RefCell::new(HashSet::new()),
        const_defs: RefCell::new(HashSet::new()),
    };
    builder.build(decls)
}
