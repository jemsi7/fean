//! Lowering: a checked model (Spec) becomes an image. This is the ahead-of-time compilation step.
//!
//! Port of `fean/lower.py` (correspondence table: docs/engine.md section 3; Map unit I-09-c).
//! Code blocks, units and blocks are created in the same order as the Python lowering, because the
//! two engines must write the same image byte for byte (DR-30).

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::check::{base, is_opt, unit_text, EventDecl, Observers, PVal, QInfo, Spec, Ty};
use crate::image::{canonical_text, FORMAT, LANGUAGE, VERSION};
use crate::syntax::*;

/// Where the parts of an image were written in the model text. Kept beside the image, not in it,
/// so that images stay equal to the Python engine's (DR-30).
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    pub actors: Vec<Option<Loc>>,
    pub assets: Vec<Option<Loc>>,
    pub params: Vec<Option<Loc>>,
    /// Declared cells, by cell number (holdings have the line of their asset).
    pub cells: Vec<Option<Loc>>,
    pub slots: Vec<Option<Loc>>,
    pub events: Vec<Option<Loc>>,
    pub queries: Vec<Option<Loc>>,
    pub unknowns: Vec<Option<Loc>>,
    pub solves: Vec<Option<Loc>>,
    /// The `choose` statement that shows a menu.
    pub menus: Vec<Option<Loc>>,
    /// The `label` statement of a mark.
    pub marks: Vec<Option<Loc>>,
    /// The statement at (block, position).
    pub stmts: Vec<Vec<Loc>>,
    /// The first statement that uses a unit.
    pub units: Vec<Option<Loc>>,
    /// The `choose` statement of each arm block: block number -> (menu, action).
    pub arms: HashMap<usize, (usize, usize)>,
}

impl SourceMap {
    /// The line in the model text, if the element was written there (not in the standard contracts).
    pub fn line(loc: Option<Loc>) -> Option<u32> {
        match loc {
            Some(Loc { origin: Origin::Model, line }) => Some(line),
            _ => None,
        }
    }
    pub fn stmt(&self, block: usize, at: usize) -> Option<Loc> {
        self.stmts.get(block).and_then(|b| b.get(at)).copied()
    }
}

type Scope = HashMap<String, usize>;

fn emit(code: &mut Vec<Value>, instr: Value) -> usize {
    code.push(instr);
    code.len() - 1
}

/// Make the jump emitted at `at` land on the next instruction.
fn land(code: &mut [Value], at: usize) {
    let next = code.len();
    code[at][1] = json!(next);
}

/// A unit argument as it appears in the unit's name: literals and names as written, the rest as `…`.
fn shown(arg: &Expr) -> String {
    match arg {
        Expr::Int(n) => n.to_string(),
        Expr::Qty(n, unit) => format!("{}[{}]", n, unit_text(&crate::check::unit_of(unit))),
        Expr::Time(n) => format!("@{n}"),
        Expr::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
        Expr::Name(q) => q.clone(),
        Expr::Neg(inner) => match shown(inner) {
            text if text == "…" => text,
            text => format!("-{text}"),
        },
        _ => "…".to_string(),
    }
}

struct Lowering<'a> {
    s: &'a Spec,
    actor: HashMap<&'a str, usize>,
    enumval: HashMap<&'a str, usize>,
    slot: HashMap<(&'a str, &'a str), usize>,
    menus: Vec<String>,
    marks: Vec<String>,
    actions: Vec<String>,
    codes: Vec<Value>,
    units: Vec<Value>,
    blocks: Vec<Value>,
    known_codes: HashMap<String, usize>,
    known_units: HashMap<String, usize>,
    map: SourceMap,
    /// Lowering a goal (layer V or X): there a name is first a query, standing for its verdict.
    goal: bool,
    /// How many expanded defs we are within: the names of a def's body are its own.
    inside: usize,
}

fn intern(table: &mut Vec<String>, name: &str) -> usize {
    match table.iter().position(|n| n == name) {
        Some(i) => i,
        None => {
            table.push(name.to_string());
            table.len() - 1
        }
    }
}

impl<'a> Lowering<'a> {
    fn new(s: &'a Spec) -> Self {
        let actor = s.actors.iter().enumerate().map(|(i, a)| (a.as_str(), i)).collect();
        let mut enumval = HashMap::new();
        for (_, values) in s.enums.iter() {
            for (i, v) in values.iter().enumerate() {
                enumval.insert(v.as_str(), i);
            }
        }
        let slot = s.slots.iter().enumerate().map(|(i, ((who, key), _))| ((who.as_str(), key.as_str()), i)).collect();
        Lowering {
            s,
            actor,
            enumval,
            slot,
            menus: Vec::new(),
            marks: Vec::new(),
            actions: Vec::new(),
            codes: Vec::new(),
            units: Vec::new(),
            blocks: Vec::new(),
            known_codes: HashMap::new(),
            known_units: HashMap::new(),
            map: SourceMap::default(),
            goal: false,
            inside: 0,
        }
    }

    // ---- tables ------------------------------------------------------------------------------
    fn holding(&self, who: &str, asset: &str) -> usize {
        self.s.cells.len() + self.s.assets.index_of(asset).unwrap() * self.s.actors.len() + self.actor[who]
    }

    /// Equal code blocks are stored once.
    fn add(&mut self, mode: char, locals: usize, ins: Vec<Value>) -> usize {
        let entry = json!({"mode": mode.to_string(), "locals": locals, "ins": ins});
        let key = canonical_text(&entry);
        if let Some(&i) = self.known_codes.get(&key) {
            return i;
        }
        self.known_codes.insert(key, self.codes.len());
        self.codes.push(entry);
        self.codes.len() - 1
    }

    fn value(&mut self, e: &Expr, mode: char) -> usize {
        let (mut ins, mut frame) = (Vec::new(), 0usize);
        let outer = (self.goal, self.inside);
        (self.goal, self.inside) = (mode == 'V' || mode == 'X', 0);
        self.expr(e, &mut ins, &mut frame, &Scope::new());
        (self.goal, self.inside) = outer;
        self.add(mode, frame, ins)
    }

    fn target(&self, tgt: &Target) -> usize {
        match tgt {
            Target::Bal(who, asset) => self.holding(who, asset),
            Target::Cell(q) => self.s.cells.index_of(q).unwrap(),
        }
    }

    fn point(&self, p: &Point) -> (usize, usize) {
        match p {
            Point::Start => (0, 0),
            Point::End => (1, 0),
            Point::Menu(m) => (2, self.menus.iter().position(|x| x == m).expect("a checked menu")),
            Point::Label(l) => (3, self.marks.iter().position(|x| x == l).expect("a checked label")),
        }
    }

    fn domain(&self, ty: &Ty) -> Value {
        match base(ty) {
            Some(Ty::Qty(_)) => json!({"kind": "qty"}),
            Some(Ty::Bool) => json!({"kind": "bool"}),
            Some(Ty::Time) => json!({"kind": "time"}),
            Some(Ty::Actor) => json!({"kind": "actor"}),
            Some(Ty::Enum(name)) => json!({"kind": "enum", "enum": self.s.enums.index_of(name).unwrap()}),
            other => unreachable!("a stored value of type {other:?}"),
        }
    }

    // ---- expressions -------------------------------------------------------------------------
    fn expr(&mut self, e: &Expr, code: &mut Vec<Value>, frame: &mut usize, scope: &Scope) {
        match e {
            Expr::Int(n) | Expr::Qty(n, _) | Expr::Time(n) => {
                emit(code, json!(["PUSH", n]));
            }
            Expr::Bool(b) => {
                emit(code, json!(["PUSH", b]));
            }
            Expr::Empty => {
                emit(code, json!(["PUSH", null]));
            }
            Expr::Now => {
                emit(code, json!(["NOW"]));
            }
            Expr::Name(q) => self.name(q, code, frame, scope),
            Expr::Bin(op @ (BinOp::And | BinOp::Or), a, b) => {
                // short circuit: the right side runs only if needed
                let is_or = *op == BinOp::Or;
                self.expr(a, code, frame, scope);
                let skip = emit(code, json!([if is_or { "JT" } else { "JF" }, null]));
                self.expr(b, code, frame, scope);
                let done = emit(code, json!(["JMP", null]));
                land(code, skip);
                emit(code, json!(["PUSH", is_or]));
                land(code, done);
            }
            Expr::Bin(op, a, b) => {
                self.expr(a, code, frame, scope);
                self.expr(b, code, frame, scope);
                let name = match op {
                    BinOp::Add => "ADD",
                    BinOp::Sub => "SUB",
                    BinOp::Mul => "MUL",
                    BinOp::Eq => "EQ",
                    BinOp::Ne => "NE",
                    BinOp::Lt => "LT",
                    BinOp::Le => "LE",
                    BinOp::Gt => "GT",
                    BinOp::Ge => "GE",
                    BinOp::And | BinOp::Or => unreachable!(),
                };
                emit(code, json!([name]));
            }
            Expr::Not(a) => {
                self.expr(a, code, frame, scope);
                emit(code, json!(["NOT"]));
            }
            Expr::Neg(a) => {
                self.expr(a, code, frame, scope);
                emit(code, json!(["NEG"]));
            }
            Expr::Known(a) => {
                self.expr(a, code, frame, scope);
                emit(code, json!(["KNOWN"]));
            }
            Expr::If(c, a, b) => {
                self.expr(c, code, frame, scope);
                let other = emit(code, json!(["JF", null]));
                self.expr(a, code, frame, scope);
                let done = emit(code, json!(["JMP", null]));
                land(code, other);
                self.expr(b, code, frame, scope);
                land(code, done);
            }
            Expr::Div(mode, a, b) => {
                self.expr(a, code, frame, scope);
                self.expr(b, code, frame, scope);
                let name = match mode {
                    DivMode::Floor => "DIVF",
                    DivMode::Ceil => "DIVC",
                    DivMode::Exact => "DIVX",
                };
                emit(code, json!([name]));
            }
            Expr::MinMax(is_min, a, b) => {
                self.expr(a, code, frame, scope);
                self.expr(b, code, frame, scope);
                emit(code, json!([if *is_min { "MIN" } else { "MAX" }]));
            }
            Expr::Default(a, b) => {
                // e ?? d: keep e unless it is empty
                self.expr(a, code, frame, scope);
                emit(code, json!(["DUP"]));
                emit(code, json!(["KNOWN"]));
                let done = emit(code, json!(["JT", null]));
                emit(code, json!(["POP"]));
                self.expr(b, code, frame, scope);
                land(code, done);
            }
            Expr::Mem(field, who, key) => {
                let name = match field {
                    Field::Mem => "MEM",
                    Field::Asof => "ASOF",
                    Field::Recv => "RECV",
                };
                emit(code, json!([name, self.slot[&(who.as_str(), key.as_str())]]));
            }
            Expr::Bal(who, asset) => match who.as_ref() {
                Expr::Name(q) if !scope.contains_key(q) && self.actor.contains_key(q.as_str()) => {
                    emit(code, json!(["LOAD", self.holding(q, asset)]));
                }
                other => {
                    self.expr(other, code, frame, scope);
                    emit(code, json!(["BAL", self.s.assets.index_of(asset).unwrap()]));
                }
            },
            Expr::Post(tgt) => {
                emit(code, json!(["POST", self.target(tgt)]));
            }
            Expr::Call(f, args) => {
                // a def is expanded where it is called
                let def = self.s.defs.get(f).expect("a checked def");
                let mut inner = Scope::new();
                for ((p, _), arg) in def.params.iter().zip(args) {
                    self.expr(arg, code, frame, scope);
                    inner.insert(p.clone(), *frame);
                    emit(code, json!(["STORE", *frame]));
                    *frame += 1;
                }
                self.inside += 1;
                self.expr(&def.body, code, frame, &inner);
                self.inside -= 1;
            }
            Expr::K(who, inner) => {
                let sub = self.value(inner, 'Q');
                emit(code, json!(["K", self.actor[who.as_str()], sub]));
            }
            Expr::At(point, inner, flag) => {
                let (kind, ident) = self.point(point);
                let sub = self.value(inner, 'Q');
                emit(code, json!(["AT", kind, ident, sub, flag.get()]));
            }
            Expr::Reach(point) => {
                let (kind, ident) = self.point(point);
                emit(code, json!(["REACH", kind, ident]));
            }
        }
    }

    fn name(&mut self, q: &str, code: &mut Vec<Value>, frame: &mut usize, scope: &Scope) {
        let s = self.s;
        if let Some(local) = scope.get(q) {
            emit(code, json!(["LOCAL", local]));
        } else if let (true, 0, Some(i)) = (self.goal, self.inside, s.queries.index_of(q)) {
            emit(code, json!(["QUERY", i]));
        } else if let Some(i) = s.params.index_of(q) {
            emit(code, json!(["PARAM", i]));
        } else if let Some(i) = s.unknowns.index_of(q) {
            emit(code, json!(["UNK", i]));
        } else if let Some(i) = s.cells.index_of(q) {
            emit(code, json!(["LOAD", i]));
        } else if s.enum_of.contains_key(q) {
            emit(code, json!(["PUSH", self.enumval[q]]));
        } else if let Some(&i) = self.actor.get(q) {
            emit(code, json!(["PUSH", i]));
        } else {
            // a declared value: its expression, in place
            let body = s.values.get(q).expect("a checked name");
            self.expr(body, code, frame, &Scope::new());
        }
    }

    // ---- events, units, program --------------------------------------------------------------
    fn lower_event(&mut self, ev: &EventDecl) -> Value {
        let mut frame = 0usize;
        let mut scope = Scope::new();
        for (p, _) in &ev.params {
            scope.insert(p.clone(), frame);
            frame += 1;
        }
        let (mut guard, mut effects, mut ensures): (Vec<Value>, Vec<Value>, Vec<Vec<Value>>) = (Vec::new(), Vec::new(), Vec::new());
        for c in &ev.clauses {
            match c {
                Clause::Let(x, e) => {
                    self.expr(e, &mut guard, &mut frame, &scope);
                    scope.insert(x.clone(), frame);
                    emit(&mut guard, json!(["STORE", frame]));
                    frame += 1;
                }
                Clause::Given(x, e) => {
                    self.expr(e, &mut guard, &mut frame, &scope);
                    scope.insert(x.clone(), frame);
                    emit(&mut guard, json!(["GIVEN", frame, format!("given {x} in {}", ev.name)]));
                    frame += 1;
                }
                Clause::Require(e) => {
                    self.expr(e, &mut guard, &mut frame, &scope);
                    emit(&mut guard, json!(["REQUIRE", format!("require in {}", ev.name)]));
                }
                Clause::Auth(e) => {
                    self.expr(e, &mut guard, &mut frame, &scope);
                    emit(&mut guard, json!(["REQUIRE", format!("auth in {}", ev.name)]));
                }
                Clause::Ensure(e) => {
                    let mut post = Vec::new();
                    self.expr(e, &mut post, &mut frame, &scope);
                    ensures.push(post);
                }
                Clause::Set(tgt, e) => {
                    self.expr(e, &mut effects, &mut frame, &scope);
                    emit(&mut effects, json!(["FXSET", self.target(tgt)]));
                }
                Clause::Delta(tgt, sign, e) => {
                    self.expr(e, &mut effects, &mut frame, &scope);
                    if *sign < 0 {
                        emit(&mut effects, json!(["NEG"]));
                    }
                    emit(&mut effects, json!(["FXADD", self.target(tgt)]));
                }
                Clause::Free(tgt) => {
                    emit(&mut effects, json!(["FXFREE", self.target(tgt)]));
                }
                Clause::Move(a, b, n, asset) => {
                    for part in [a, b, n] {
                        self.expr(part, &mut effects, &mut frame, &scope);
                    }
                    let asset = asset.borrow().clone().expect("the checker names the asset of a move");
                    emit(&mut effects, json!(["FXMOVE", self.s.assets.index_of(&asset).unwrap()]));
                }
                Clause::Tell(who, key, v, asof) => {
                    self.expr(v, &mut effects, &mut frame, &scope);
                    self.expr(asof, &mut effects, &mut frame, &scope);
                    emit(&mut effects, json!(["FXTELL", self.slot[&(who.as_str(), key.as_str())]]));
                }
                Clause::Forget(who, key) => {
                    emit(&mut effects, json!(["FXFORGET", self.slot[&(who.as_str(), key.as_str())]]));
                }
            }
        }
        let guard = self.add('G', frame, guard);
        let effects = self.add('E', frame, effects);
        let ensure: Vec<usize> = ensures.into_iter().map(|p| self.add('P', frame, p)).collect();
        json!({"name": ev.name, "params": ev.params.len(), "locals": frame, "net": ev.net,
               "guard": guard, "effects": effects, "ensure": ensure})
    }

    fn unit(&mut self, u: &UnitRef, loc: Loc) -> usize {
        let s = self.s;
        let (name, net, members): (String, bool, Vec<(&String, &Vec<Expr>)>) = match u {
            UnitRef::Batch(q) => {
                let batch = s.batches.get(q).expect("a checked batch");
                (format!("batch {q}"), batch.net, batch.members.iter().map(|(e, args)| (e, args)).collect())
            }
            UnitRef::Ev(q, args) => {
                let texts: Vec<String> = args.iter().map(shown).collect();
                let name = if texts.is_empty() { q.clone() } else { format!("{q}({})", texts.join(", ")) };
                (name, s.events.get(q).expect("a checked event").net, vec![(q, args)])
            }
        };
        let mut rows = Vec::new();
        for (q, args) in members {
            let codes: Vec<usize> = args.iter().map(|a| self.value(a, 'C')).collect();
            rows.push(json!({"event": s.events.index_of(q).unwrap(), "args": codes}));
        }
        let entry = json!({"name": name, "net": net, "members": rows});
        let key = canonical_text(&entry);
        let ix = match self.known_units.get(&key) {
            Some(&i) => i,
            None => {
                self.known_units.insert(key, self.units.len());
                self.units.push(entry);
                self.map.units.push(None);
                self.units.len() - 1
            }
        };
        if self.map.units[ix].is_none() {
            self.map.units[ix] = Some(loc);
        }
        ix
    }

    fn block(&mut self, stmts: &[Stmt]) -> usize {
        let ix = self.blocks.len();
        self.blocks.push(Value::Null);
        self.map.stmts.push(stmts.iter().map(|st| st.loc).collect());
        let mut out = Vec::new();
        for st in stmts {
            out.push(match &st.kind {
                StmtKind::Run(u) => json!(["run", self.unit(u, st.loc)]),
                StmtKind::Attempt(u, yes, no) => {
                    let unit = self.unit(u, st.loc);
                    let (yes, no) = (self.block(yes), self.block(no));
                    json!(["attempt", unit, yes, no])
                }
                StmtKind::At(t, blk) => {
                    let time = self.value(t, 'C');
                    json!(["at", time, self.block(blk)])
                }
                StmtKind::Alt(blocks) => {
                    let branches: Vec<usize> = blocks.iter().map(|b| self.block(b)).collect();
                    json!(["alt", branches])
                }
                StmtKind::Choose(who, menu, actions) => {
                    let menu = intern(&mut self.menus, menu);
                    if self.map.menus.len() < self.menus.len() {
                        self.map.menus.push(Some(st.loc));
                    }
                    let mut arms = Vec::new();
                    for a in actions {
                        let action = intern(&mut self.actions, &a.label);
                        let blk = self.block(&a.body);
                        self.map.arms.insert(blk, (menu, action));
                        arms.push(json!([action, blk]));
                    }
                    json!(["choose", self.actor[who.as_str()], menu, arms])
                }
                StmtKind::Label(l) => {
                    let mark = intern(&mut self.marks, l);
                    if self.map.marks.len() < self.marks.len() {
                        self.map.marks.push(Some(st.loc));
                    }
                    json!(["mark", mark])
                }
                StmtKind::Do(q, _) => unreachable!("proc {q} is expanded by the checker"),
            });
        }
        self.blocks[ix] = Value::Array(out);
        ix
    }

    fn query(&mut self, q: &Query) -> Value {
        let actors = |list: &[String]| -> Vec<usize> { list.iter().map(|a| self.actor[a.as_str()]).collect() };
        match q {
            Query::Exists(e) => json!({"kind": "exists", "code": self.value(e, 'R')}),
            Query::Forall(e) => json!({"kind": "forall", "code": self.value(e, 'R')}),
            Query::Can(who, e) => {
                let who = actors(who);
                json!({"kind": "can", "who": who, "code": self.value(e, 'R')})
            }
            Query::Num { policy, weight, max, expr } => {
                let policy = policy.as_ref().map(|p| actors(p));
                let weight = weight.as_ref().map(|w| self.s.weights.index_of(w).unwrap());
                json!({"kind": "num", "policy": policy, "weight": weight,
                       "opt": if *max { "max" } else { "min" }, "code": self.value(expr, 'R')})
            }
            Query::Region(params, inner) => {
                let params: Vec<usize> = params.iter().map(|p| self.s.params.index_of(p).unwrap()).collect();
                json!({"kind": "region", "params": params, "inner": self.query(inner)})
            }
            Query::Prefer { who, point, a, b, value, weight } => {
                let Point::Menu(m) = point else { unreachable!("prefer is checked to name a menu") };
                let menu = self.menus.iter().position(|x| x == m).expect("a checked menu");
                let act = |label: &String| self.actions.iter().position(|x| x == label).expect("a checked action");
                let (a, b) = (act(a), act(b));
                let weight = weight.as_ref().map(|w| self.s.weights.index_of(w).unwrap());
                let code = self.value(&Expr::Name(value.clone()), 'R');
                json!({"kind": "prefer", "who": self.actor[who.as_str()], "menu": menu, "a": a, "b": b,
                       "code": code, "weight": weight})
            }
        }
    }

    fn parameter(&self, name: &str, ty: &Ty, values: &[PVal]) -> Value {
        let ints = || -> Vec<i64> {
            values.iter().map(|v| if let PVal::Int(n) = v { *n } else { unreachable!("a number") }).collect()
        };
        let names = || -> Vec<&str> {
            values.iter().map(|v| if let PVal::Name(n) = v { n.as_str() } else { unreachable!("a name") }).collect()
        };
        match ty {
            Ty::Bool => {
                let vals: Vec<bool> = values.iter().map(|v| matches!(v, PVal::Bool(true))).collect();
                let show: Vec<&str> = vals.iter().map(|b| if *b { "true" } else { "false" }).collect();
                json!({"name": name, "kind": "bool", "values": vals, "show": show})
            }
            Ty::Time => {
                let show: Vec<String> = ints().iter().map(|v| format!("@{v}")).collect();
                json!({"name": name, "kind": "time", "values": ints(), "show": show})
            }
            Ty::Actor => {
                let vals: Vec<usize> = names().iter().map(|v| self.actor[v]).collect();
                json!({"name": name, "kind": "actor", "values": vals, "show": names()})
            }
            Ty::Enum(_) => {
                let vals: Vec<usize> = names().iter().map(|v| self.enumval[v]).collect();
                json!({"name": name, "kind": "enum", "values": vals, "show": names()})
            }
            _ => {
                let show: Vec<String> = ints().iter().map(|v| v.to_string()).collect();
                json!({"name": name, "kind": "num", "values": ints(), "show": show})
            }
        }
    }

    // ---- the image ---------------------------------------------------------------------------
    fn image(mut self) -> (Value, SourceMap) {
        let s = self.s;
        let zero = [Expr::Int(0)];
        let mut cells = Vec::new();
        for (name, c) in s.cells.iter() {
            let lo = c.lo.as_ref().map(|e| self.value(e, 'C'));
            let hi = c.hi.as_ref().map(|e| self.value(e, 'C'));
            let init: Vec<usize> = c.init.iter().map(|v| self.value(v, 'C')).collect();
            cells.push(json!({"name": name, "dom": self.domain(&c.ty), "opt": is_opt(&c.ty),
                              "lo": lo, "hi": hi, "init": init}));
            self.map.cells.push(Some(c.loc));
        }
        let mut assets = Vec::new();
        for (name, a) in s.assets.iter() {
            let lo = self.value(&a.lo, 'C');
            let hi = self.value(&a.hi, 'C');
            let mut init = Vec::new();
            for who in &s.actors {
                let values: &[Expr] = a.init.get(who).map(|v| v.as_slice()).unwrap_or(&zero);
                init.push(values.iter().map(|v| self.value(v, 'C')).collect::<Vec<usize>>());
            }
            assets.push(json!({"name": name, "lo": lo, "hi": hi, "init": init}));
            self.map.assets.push(Some(a.loc));
        }
        let (mut obs, mut mems) = (Vec::new(), Vec::new());
        for who in &s.actors {
            let mut seen: Vec<usize> = Vec::new();
            for (i, c) in s.cells.values().iter().enumerate() {
                let sees = match &c.observers {
                    Observers::All => true,
                    Observers::Some(list) => list.contains(who),
                };
                if sees {
                    seen.push(i);
                }
            }
            for (x, a) in s.assets.iter() {
                for j in &s.actors {
                    if j == who || a.public {
                        seen.push(self.holding(j, x));
                    }
                }
            }
            obs.push(seen);
            let mut keys: Vec<&(String, String)> = s.slots.iter().map(|(k, _)| k).filter(|k| k.0 == *who).collect();
            keys.sort();
            mems.push(keys.iter().map(|k| self.slot[&(k.0.as_str(), k.1.as_str())]).collect::<Vec<usize>>());
        }
        let mut events = Vec::new();
        for ev in s.events.values() {
            events.push(self.lower_event(ev));
            self.map.events.push(Some(ev.loc));
        }
        let entry = self.block(&s.program);
        let mut queries = Vec::new();
        for (name, q) in s.queries.iter() {
            let unit = match s.qinfo.get(name) {
                Some(QInfo::Num(unit)) if !unit.is_empty() => unit_text(unit),
                _ => String::new(),
            };
            queries.push(json!({"name": name, "unit": unit, "q": self.query(q)}));
            self.map.queries.push(s.query_loc.get(name).copied());
        }
        let mut solves = Vec::new();
        for (name, sv) in s.solves.iter() {
            let (mode, before) = if sv.point.is_none() { ('V', 'C') } else { ('X', 'S') };
            let point = sv.point.as_ref().map(|l| self.marks.iter().position(|x| x == l).expect("a checked label"));
            let filter = sv.filter.as_ref().map(|e| self.value(e, before));
            let objective = sv.objective.as_ref().map(|e| self.value(e, mode));
            let goal = self.value(&sv.goal, mode);
            let unit = match s.sinfo.get(name) {
                Some(Some(unit)) if !unit.is_empty() => unit_text(unit),
                _ => String::new(),
            };
            solves.push(json!({"name": name, "form": sv.form.text(), "point": point, "where": filter,
                               "objective": objective, "goal": goal, "unit": unit}));
            self.map.solves.push(s.solve_loc.get(name).copied());
        }
        let unknowns: Vec<Value> = s
            .unknowns
            .iter()
            .map(|(name, (ty, lo, hi))| match ty {
                Ty::Bool => json!({"name": name, "kind": "bool", "lo": lo, "hi": hi}),
                Ty::Time => json!({"name": name, "kind": "time", "lo": lo, "hi": hi}),
                Ty::Actor => json!({"name": name, "kind": "actor", "lo": lo, "hi": hi}),
                Ty::Enum(e) => json!({"name": name, "kind": "enum", "lo": lo, "hi": hi, "enum": s.enums.index_of(e).unwrap()}),
                _ => json!({"name": name, "kind": "num", "lo": lo, "hi": hi}),
            })
            .collect();
        self.map.unknowns = s.unknowns.keys().iter().map(|u| s.unknown_loc.get(u).copied()).collect();
        let enums: Vec<Value> = s.enums.iter().map(|(name, values)| json!({"name": name, "values": values})).collect();
        let params: Vec<Value> = s.params.iter().map(|(name, (ty, values))| self.parameter(name, ty, values)).collect();
        let slots: Vec<Value> = s
            .slots
            .iter()
            .map(|((who, key), ty)| json!({"actor": self.actor[who.as_str()], "key": key, "dom": self.domain(ty)}))
            .collect();
        let assumes: Vec<usize> = s.assumes.iter().map(|e| self.value(e, 'S')).collect();
        let invariants: Vec<usize> = s.invariants.iter().map(|e| self.value(e, 'S')).collect();
        let weights: Vec<Value> = s.weights.iter().map(|(name, e)| json!({"name": name, "code": self.value(e, 'S')})).collect();
        self.map.actors = s.actors.iter().map(|a| s.actor_loc.get(a).copied()).collect();
        self.map.params = s.params.keys().iter().map(|p| s.param_loc.get(p).copied()).collect();
        self.map.slots = s.slots.iter().map(|(k, _)| s.slot_loc.get(k).copied()).collect();
        let image = json!({
            "format": FORMAT, "version": VERSION, "language": LANGUAGE, "model": s.name,
            "actors": s.actors, "assets": assets, "enums": enums, "time": [s.time.0, s.time.1],
            "params": params, "unknowns": unknowns, "cells": cells, "slots": slots, "obs": obs, "mems": mems,
            "assumes": assumes, "invariants": invariants, "events": events, "units": self.units,
            "blocks": self.blocks, "entry": entry, "menus": self.menus, "marks": self.marks,
            "actions": self.actions, "weights": weights, "queries": queries, "solves": solves, "codes": self.codes,
        });
        (image, self.map)
    }
}

/// Compile a checked model into an image (not yet verified) and its source map.
pub fn lower(spec: &Spec) -> (Value, SourceMap) {
    Lowering::new(spec).image()
}
