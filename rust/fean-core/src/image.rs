//! The image: the intermediate representation produced ahead of time and run by the machine.
//!
//! Port of `fean/ir.py` (format and rules: docs/engine.md sections 3-5; Map unit I-09-c).
//! On disk an image is JSON with sorted keys; `verify` checks it, `Image::decode` turns a verified
//! image into the typed form the machine runs.

use serde_json::Value;

use crate::syntax::{FeanError, R};

pub const FORMAT: &str = "feanc";
pub const VERSION: i64 = 2;
pub const LANGUAGE: &str = "0.3";

/// The empty value inside the machine.
pub const NONE: i64 = i64::MIN;

const ALL: &str = "CSPQRGEVX";
const STATE: &str = "SPQGEX";

/// instruction: (name, inline arguments, values popped, values pushed, layers that may use it)
pub const OPS: &[(&str, usize, usize, usize, &str)] = &[
    ("PUSH", 1, 0, 1, ALL), ("PARAM", 1, 0, 1, ALL), ("LOCAL", 1, 0, 1, ALL), ("STORE", 1, 1, 0, ALL),
    ("DUP", 0, 1, 2, ALL), ("POP", 0, 1, 0, ALL),
    ("LOAD", 1, 0, 1, STATE), ("BAL", 1, 1, 1, STATE), ("NOW", 0, 0, 1, STATE),
    ("MEM", 1, 0, 1, STATE), ("ASOF", 1, 0, 1, STATE), ("RECV", 1, 0, 1, STATE),
    ("POST", 1, 0, 1, "P"),
    ("ADD", 0, 2, 1, ALL), ("SUB", 0, 2, 1, ALL), ("MUL", 0, 2, 1, ALL), ("MIN", 0, 2, 1, ALL),
    ("MAX", 0, 2, 1, ALL), ("DIVF", 0, 2, 1, ALL), ("DIVC", 0, 2, 1, ALL), ("DIVX", 0, 2, 1, ALL),
    ("EQ", 0, 2, 1, ALL), ("NE", 0, 2, 1, ALL), ("LT", 0, 2, 1, ALL), ("LE", 0, 2, 1, ALL),
    ("GT", 0, 2, 1, ALL), ("GE", 0, 2, 1, ALL),
    ("NEG", 0, 1, 1, ALL), ("NOT", 0, 1, 1, ALL), ("KNOWN", 0, 1, 1, ALL),
    ("JMP", 1, 0, 0, ALL), ("JF", 1, 1, 0, ALL), ("JT", 1, 1, 0, ALL),
    ("K", 2, 0, 1, "Q"), ("AT", 4, 0, 1, "R"), ("REACH", 2, 0, 1, "R"),
    ("REQUIRE", 1, 1, 0, "G"), ("GIVEN", 2, 1, 0, "G"),
    ("FXSET", 1, 1, 0, "E"), ("FXADD", 1, 1, 0, "E"), ("FXFREE", 1, 0, 0, "E"),
    ("FXMOVE", 1, 3, 0, "E"), ("FXTELL", 1, 2, 0, "E"), ("FXFORGET", 1, 0, 0, "E"),
    ("UNK", 1, 0, 1, ALL), ("QUERY", 1, 0, 1, "VX"),
];

pub const TABLES: &[&str] = &[
    "model", "language", "actors", "assets", "enums", "time", "params", "cells", "slots", "obs", "mems",
    "assumes", "invariants", "events", "units", "blocks", "entry", "menus", "marks", "actions", "weights",
    "queries", "codes", "unknowns", "solves",
];
const CELL_OPS: &[&str] = &["LOAD", "POST", "FXSET", "FXADD", "FXFREE"];
const ASSET_OPS: &[&str] = &["BAL", "FXMOVE"];
const SLOT_OPS: &[&str] = &["MEM", "ASOF", "RECV", "FXTELL", "FXFORGET"];
const DOMAINS: &[&str] = &["qty", "bool", "enum", "time", "actor"];
const PARAM_KINDS: &[&str] = &["bool", "enum", "time", "actor", "num"];
const SOLVE_FORMS: &[&str] = &["find", "any", "min", "max"];
const POINTS: &[&str] = &["start", "end", "menu", "label"];

fn op_info(name: &str) -> Option<&'static (&'static str, usize, usize, usize, &'static str)> {
    OPS.iter().find(|o| o.0 == name)
}

/// Values a finished code block leaves on the stack, per layer.
fn result_of(mode: &str) -> Option<usize> {
    match mode {
        "C" | "S" | "P" | "Q" | "R" | "V" | "X" => Some(1),
        "G" | "E" => Some(0),
        _ => None,
    }
}

fn bad(code: &str, msg: impl Into<String>) -> FeanError {
    FeanError::image(code, msg)
}

fn shape(what: &str) -> FeanError {
    bad("I-Format", format!("a table has the wrong shape ({what})"))
}

// -- shape-checked access: a wrong shape is I-Format, as in the Python verifier
fn arr<'a>(v: &'a Value, what: &str) -> R<&'a Vec<Value>> {
    v.as_array().ok_or_else(|| shape(what))
}
fn key<'a>(v: &'a Value, k: &str) -> R<&'a Value> {
    v.as_object().and_then(|o| o.get(k)).ok_or_else(|| shape(k))
}
fn text<'a>(v: &'a Value, what: &str) -> R<&'a str> {
    v.as_str().ok_or_else(|| shape(what))
}
fn int(v: &Value, what: &str) -> R<i64> {
    if v.is_boolean() {
        return Err(shape(what));
    }
    v.as_i64().ok_or_else(|| shape(what))
}

/// Check that an image is well formed.
pub fn verify(image: &Value) -> R<()> {
    let ok = image.as_object().map(|o| o.get("format").and_then(Value::as_str) == Some(FORMAT)).unwrap_or(false);
    if !ok {
        return Err(bad("I-Format", "this is not a FEAN image"));
    }
    if image.get("version").and_then(Value::as_i64) != Some(VERSION) || image["version"].is_boolean() {
        return Err(bad("I-Format", format!("image version {} is not {VERSION}", image.get("version").unwrap_or(&Value::Null))));
    }
    for t in TABLES {
        if image.get(*t).is_none() {
            return Err(bad("I-Format", format!("table {t} is missing")));
        }
    }
    Verifier::new(image)?.run()
}

struct Verifier<'a> {
    im: &'a Value,
    n_actors: usize,
    n_assets: usize,
    n_enums: usize,
    n_params: usize,
    n_slots: usize,
    n_events: usize,
    n_units: usize,
    n_blocks: usize,
    n_menus: usize,
    n_marks: usize,
    n_actions: usize,
    n_weights: usize,
    n_codes: usize,
    n_cells: usize,
    n_unknowns: usize,
    n_queries: usize,
}

impl<'a> Verifier<'a> {
    fn new(im: &'a Value) -> R<Self> {
        let len = |k: &str| -> R<usize> { Ok(arr(&im[k], k)?.len()) };
        let (n_actors, n_assets) = (len("actors")?, len("assets")?);
        Ok(Verifier {
            im,
            n_actors,
            n_assets,
            n_enums: len("enums")?,
            n_params: len("params")?,
            n_slots: len("slots")?,
            n_events: len("events")?,
            n_units: len("units")?,
            n_blocks: len("blocks")?,
            n_menus: len("menus")?,
            n_marks: len("marks")?,
            n_actions: len("actions")?,
            n_weights: len("weights")?,
            n_codes: len("codes")?,
            n_cells: len("cells")? + n_assets * n_actors,
            n_unknowns: len("unknowns")?,
            n_queries: len("queries")?,
        })
    }

    fn size(&self, table: &str) -> usize {
        match table {
            "actors" => self.n_actors,
            "assets" => self.n_assets,
            "enums" => self.n_enums,
            "params" => self.n_params,
            "slots" => self.n_slots,
            "events" => self.n_events,
            "units" => self.n_units,
            "blocks" => self.n_blocks,
            "menus" => self.n_menus,
            "marks" => self.n_marks,
            "actions" => self.n_actions,
            "weights" => self.n_weights,
            "codes" => self.n_codes,
            "unknowns" => self.n_unknowns,
            "queries" => self.n_queries,
            _ => self.n_cells,
        }
    }

    fn rf(&self, v: &Value, table: &str, what: &str) -> R<usize> {
        match v.as_u64() {
            Some(i) if !v.is_boolean() && (i as usize) < self.size(table) => Ok(i as usize),
            _ => Err(bad("I-Ref", format!("{what}: {table} number {v} is out of range"))),
        }
    }

    fn code(&self, v: &Value, layers: &str, what: &str, frame: Option<i64>) -> R<usize> {
        let i = self.rf(v, "codes", what)?;
        let c = &self.im["codes"][i];
        let mode = text(key(c, "mode")?, "mode")?;
        if !layers.contains(mode) || mode.len() != 1 {
            return Err(bad("I-Mode", format!("{what}: code {i} is in layer {mode}, expected one of {layers}")));
        }
        if let Some(frame) = frame {
            if key(c, "locals")?.as_i64() != Some(frame) {
                return Err(bad("I-Ref", format!("{what}: code {i} has {} locals, its event has {frame}", c["locals"])));
            }
        }
        Ok(i)
    }

    fn run(&self) -> R<()> {
        let im = self.im;
        let time = arr(&im["time"], "time")?;
        let ordered = time.len() == 2
            && time.iter().all(|t| t.is_i64() || t.is_u64())
            && time[0].as_i64() <= time[1].as_i64();
        if !ordered {
            return Err(bad("I-Format", "the time range is not two ordered integers"));
        }
        for a in arr(&im["assets"], "assets")? {
            let what = format!("asset {}", key(a, "name")?);
            self.code(key(a, "lo")?, "C", &what, None)?;
            self.code(key(a, "hi")?, "C", &what, None)?;
            let init = arr(key(a, "init")?, "init")?;
            let mut full = init.len() == self.n_actors;
            for values in init {
                full &= !arr(values, "init")?.is_empty();
            }
            if !full {
                return Err(bad("I-Format", format!("{what} needs initial holdings for every actor")));
            }
            for values in init {
                for v in arr(values, "init")? {
                    self.code(v, "C", &what, None)?;
                }
            }
        }
        for p in arr(&im["params"], "params")? {
            let kind = text(key(p, "kind")?, "kind")?;
            let (values, show) = (arr(key(p, "values")?, "values")?, arr(key(p, "show")?, "show")?);
            if !PARAM_KINDS.contains(&kind) || values.len() != show.len() || values.is_empty() {
                return Err(bad("I-Format", format!("parameter {} is malformed", key(p, "name")?)));
            }
        }
        for c in arr(&im["cells"], "cells")? {
            let what = format!("cell {}", key(c, "name")?);
            let dom = key(c, "dom")?;
            self.domain(dom, &what)?;
            let (lo, hi) = (key(c, "lo")?, key(c, "hi")?);
            let qty = key(dom, "kind")?.as_str() == Some("qty");
            if lo.is_null() != hi.is_null() || qty != !lo.is_null() {
                return Err(bad("I-Format", format!("{what}: only quantity cells have a range, and they need one")));
            }
            for bound in [lo, hi] {
                if !bound.is_null() {
                    self.code(bound, "C", &what, None)?;
                }
            }
            let init = arr(key(c, "init")?, "init")?;
            if init.is_empty() {
                return Err(bad("I-Format", format!("{what} has no initial value")));
            }
            for v in init {
                self.code(v, "C", &what, None)?;
            }
        }
        for s in arr(&im["slots"], "slots")? {
            let what = format!("slot {}", key(s, "key")?);
            self.rf(key(s, "actor")?, "actors", &what)?;
            self.domain(key(s, "dom")?, &what)?;
        }
        for (table, target) in [("obs", "cells"), ("mems", "slots")] {
            let rows = arr(&im[table], table)?;
            if rows.len() != self.n_actors {
                return Err(bad("I-Format", format!("table {table} needs one list per actor")));
            }
            for row in rows {
                for i in arr(row, table)? {
                    self.rf(i, target, table)?;
                }
            }
        }
        for i in arr(&im["assumes"], "assumes")?.iter().chain(arr(&im["invariants"], "invariants")?) {
            self.code(i, "S", "assume/invariant", None)?;
        }
        for ev in arr(&im["events"], "events")? {
            let what = format!("event {}", key(ev, "name")?);
            let (params, locals) = (int(key(ev, "params")?, "params")?, int(key(ev, "locals")?, "locals")?);
            if !(0 <= params && params <= locals) {
                return Err(bad("I-Ref", format!("{what}: {params} parameters do not fit in {locals} locals")));
            }
            self.code(key(ev, "guard")?, "G", &what, Some(locals))?;
            self.code(key(ev, "effects")?, "E", &what, Some(locals))?;
            for i in arr(key(ev, "ensure")?, "ensure")? {
                self.code(i, "P", &what, Some(locals))?;
            }
        }
        for u in arr(&im["units"], "units")? {
            let what = format!("unit {}", key(u, "name")?);
            let members = arr(key(u, "members")?, "members")?;
            if members.is_empty() || !key(u, "net")?.is_boolean() {
                return Err(bad("I-Format", format!("{what} is malformed")));
            }
            for m in members {
                let ev = &im["events"][self.rf(key(m, "event")?, "events", &what)?];
                let args = arr(key(m, "args")?, "args")?;
                if Some(args.len() as i64) != ev["params"].as_i64() {
                    return Err(bad("I-Ref", format!("{what}: {} takes {} arguments", ev["name"], ev["params"])));
                }
                for i in args {
                    self.code(i, "C", &what, None)?;
                }
            }
        }
        self.blocks()?;
        for w in arr(&im["weights"], "weights")? {
            self.code(key(w, "code")?, "S", &format!("weight {}", key(w, "name")?), None)?;
        }
        for q in arr(&im["queries"], "queries")? {
            if !key(q, "unit")?.is_string() {
                return Err(bad("I-Format", format!("query {} has no unit text", key(q, "name")?)));
            }
            self.query(key(q, "q")?, &format!("query {}", key(q, "name")?))?;
        }
        for un in arr(&im["unknowns"], "unknowns")? {
            let what = format!("unknown {}", key(un, "name")?);
            let kind = text(key(un, "kind")?, "kind")?;
            let whole = |v: &Value| if v.is_boolean() { None } else { v.as_i64() };
            let (Some(lo), Some(hi)) = (whole(key(un, "lo")?), whole(key(un, "hi")?)) else {
                return Err(bad("I-Format", format!("{what} is malformed")));
            };
            if !PARAM_KINDS.contains(&kind) || lo > hi {
                return Err(bad("I-Format", format!("{what} is malformed")));
            }
            let limits = match kind {
                "bool" => Some((0, 1)),
                "actor" => Some((0, self.n_actors as i64 - 1)),
                "time" => Some((time[0].as_i64().unwrap_or(0), time[1].as_i64().unwrap_or(0))),
                "enum" => {
                    let e = self.rf(key(un, "enum")?, "enums", &what)?;
                    Some((0, arr(key(&im["enums"][e], "values")?, "values")?.len() as i64 - 1))
                }
                _ => None,
            };
            if let Some((least, greatest)) = limits {
                if lo < least || hi > greatest {
                    return Err(bad("I-Format", format!("{what} ranges outside its kind")));
                }
            }
        }
        for sv in arr(&im["solves"], "solves")? {
            let what = format!("solve {}", key(sv, "name")?);
            let form = key(sv, "form")?.as_str().unwrap_or("?");
            if !SOLVE_FORMS.contains(&form) || !key(sv, "unit")?.is_string() {
                return Err(bad("I-Format", format!("{what} is malformed")));
            }
            let (point, filter, objective) = (key(sv, "point")?, key(sv, "where")?, key(sv, "objective")?);
            if objective.is_null() == (form == "min" || form == "max") {
                return Err(bad("I-Format", format!("{what}: only min and max have an objective, and they need one")));
            }
            if !point.is_null() {
                self.rf(point, "marks", &what)?;
            }
            let (mode, before) = if point.is_null() { ("V", "C") } else { ("X", "S") };
            if !filter.is_null() {
                self.code(filter, before, &what, None)?;
            }
            if !objective.is_null() {
                self.code(objective, mode, &what, None)?;
            }
            self.code(key(sv, "goal")?, mode, &what, None)?;
        }
        for (i, c) in arr(&im["codes"], "codes")?.iter().enumerate() {
            self.block_of_code(i, c)?;
        }
        Ok(())
    }

    fn domain(&self, dom: &Value, what: &str) -> R<()> {
        let kind = text(key(dom, "kind")?, "kind")?;
        if !DOMAINS.contains(&kind) {
            return Err(bad("I-Format", format!("{what}: unknown domain {kind:?}")));
        }
        if kind == "enum" {
            self.rf(key(dom, "enum")?, "enums", what)?;
        }
        Ok(())
    }

    fn blocks(&self) -> R<()> {
        let im = self.im;
        let mut uses = vec![0usize; self.n_blocks];
        let entry = self.rf(&im["entry"], "blocks", "entry")?;
        for (bi, stmts) in arr(&im["blocks"], "blocks")?.iter().enumerate() {
            for st in arr(stmts, "block")? {
                let what = format!("block {bi}");
                let st_arr = arr(st, "statement")?;
                let kind = st_arr.first().and_then(Value::as_str).ok_or_else(|| shape("statement"))?;
                let malformed = || bad("I-Format", format!("{what}: malformed statement {st}"));
                let used = |v: &Value, uses: &mut Vec<usize>| -> R<()> {
                    uses[self.rf(v, "blocks", &what)?] += 1;
                    Ok(())
                };
                match (kind, st_arr.len()) {
                    ("at", 3) => {
                        self.code(&st_arr[1], "C", &what, None)?;
                        used(&st_arr[2], &mut uses)?;
                    }
                    ("run", 2) => {
                        self.rf(&st_arr[1], "units", &what)?;
                    }
                    ("attempt", 4) => {
                        self.rf(&st_arr[1], "units", &what)?;
                        used(&st_arr[2], &mut uses)?;
                        used(&st_arr[3], &mut uses)?;
                    }
                    ("alt", 2) => {
                        let branches = arr(&st_arr[1], "alt")?;
                        if branches.len() < 2 {
                            return Err(malformed());
                        }
                        for b in branches {
                            used(b, &mut uses)?;
                        }
                    }
                    ("choose", 4) => {
                        let arms = arr(&st_arr[3], "choose")?;
                        if arms.is_empty() {
                            return Err(malformed());
                        }
                        self.rf(&st_arr[1], "actors", &what)?;
                        self.rf(&st_arr[2], "menus", &what)?;
                        for arm in arms {
                            let pair = arr(arm, "arm")?;
                            if pair.len() != 2 {
                                return Err(shape("arm"));
                            }
                            self.rf(&pair[0], "actions", &what)?;
                            used(&pair[1], &mut uses)?;
                        }
                    }
                    ("mark", 2) => {
                        self.rf(&st_arr[1], "marks", &what)?;
                    }
                    _ => return Err(malformed()),
                }
            }
        }
        for (b, count) in uses.iter().enumerate() {
            if *count != if b == entry { 0 } else { 1 } {
                return Err(bad("I-Ref", format!("block {b} is referenced {count} times; the program must be a tree")));
            }
        }
        Ok(())
    }

    fn query(&self, q: &Value, what: &str) -> R<()> {
        let kind = text(key(q, "kind")?, "kind")?;
        match kind {
            "exists" | "forall" => {
                self.code(key(q, "code")?, "R", what, None)?;
            }
            "can" => {
                for a in arr(key(q, "who")?, "who")? {
                    self.rf(a, "actors", what)?;
                }
                self.code(key(q, "code")?, "R", what, None)?;
            }
            "num" => {
                let policy = key(q, "policy")?;
                if !policy.is_null() {
                    for a in arr(policy, "policy")? {
                        self.rf(a, "actors", what)?;
                    }
                }
                if !key(q, "weight")?.is_null() {
                    self.rf(&q["weight"], "weights", what)?;
                }
                let opt = key(q, "opt")?;
                if opt.as_str() != Some("max") && opt.as_str() != Some("min") {
                    return Err(bad("I-Format", format!("{what}: unknown aggregate {opt}")));
                }
                self.code(key(q, "code")?, "R", what, None)?;
            }
            "region" => {
                for p in arr(key(q, "params")?, "params")? {
                    self.rf(p, "params", what)?;
                }
                self.query(key(q, "inner")?, what)?;
            }
            "prefer" => {
                self.rf(key(q, "who")?, "actors", what)?;
                self.rf(key(q, "menu")?, "menus", what)?;
                self.rf(key(q, "a")?, "actions", what)?;
                self.rf(key(q, "b")?, "actions", what)?;
                if !key(q, "weight")?.is_null() {
                    self.rf(&q["weight"], "weights", what)?;
                }
                self.code(key(q, "code")?, "R", what, None)?;
            }
            other => return Err(bad("I-Format", format!("{what}: unknown kind {other:?}"))),
        }
        Ok(())
    }

    fn point(&self, kind: &Value, ident: &Value, what: &str) -> R<()> {
        let k = match kind.as_u64() {
            Some(k) if !kind.is_boolean() && k <= 3 => k,
            _ => return Err(bad("I-Ref", format!("{what}: unknown point kind {kind}"))),
        };
        if k >= 2 {
            self.rf(ident, if k == 2 { "menus" } else { "marks" }, what)?;
        } else if ident.as_i64() != Some(0) {
            return Err(bad("I-Ref", format!("{what}: start and end take no number")));
        }
        Ok(())
    }

    fn inner(&self, sub: &Value, ci: usize, what: &str) -> R<()> {
        let i = self.code(sub, "Q", what, None)?;
        if i >= ci {
            return Err(bad("I-Ref", format!("{what}: inner code {i} must come before it")));
        }
        Ok(())
    }

    fn block_of_code(&self, ci: usize, c: &Value) -> R<()> {
        let what = format!("code {ci}");
        let mode = text(key(c, "mode")?, "mode")?;
        let Some(leaves) = result_of(mode) else {
            return Err(bad("I-Mode", format!("{what}: unknown layer {mode:?}")));
        };
        let locals = match key(c, "locals")? {
            v if !v.is_boolean() && v.as_u64().is_some() => v.as_u64().unwrap() as usize,
            _ => return Err(bad("I-Format", format!("{what}: bad local count"))),
        };
        let ins = arr(key(c, "ins")?, "ins")?;
        let size = ins.len();
        let mut ops: Vec<(&str, usize, usize, Option<usize>)> = Vec::with_capacity(size);
        for (pc, instr) in ins.iter().enumerate() {
            let parts = arr(instr, "instruction")?;
            let op = parts.first().and_then(Value::as_str).unwrap_or("?");
            let Some(&(_, argc, pops, pushes, layers)) = op_info(op) else {
                return Err(bad("I-Mode", format!("{what}: unknown instruction {:?}", parts.first().unwrap_or(&Value::Null))));
            };
            if !layers.contains(mode) {
                return Err(bad("I-Mode", format!("{what}: {op} is not allowed in layer {mode}")));
            }
            if parts.len() != argc + 1 {
                return Err(bad("I-Format", format!("{what}: {op} takes {argc} arguments")));
            }
            let a = &parts[1..];
            let mut target = None;
            match op {
                "PUSH" => {
                    if !(a[0].is_null() || a[0].is_boolean() || a[0].is_i64() || a[0].is_u64()) {
                        return Err(bad("I-Format", format!("{what}: PUSH takes an integer, true, false or null")));
                    }
                }
                "PARAM" => {
                    self.rf(&a[0], "params", &what)?;
                }
                "UNK" => {
                    self.rf(&a[0], "unknowns", &what)?;
                }
                "QUERY" => {
                    let q = key(&self.im["queries"][self.rf(&a[0], "queries", &what)?], "q")?;
                    let kind = key(q, "kind")?.as_str().unwrap_or("?");
                    let weighted = kind == "num" && !q.get("weight").map(Value::is_null).unwrap_or(true);
                    if kind == "region" || weighted {
                        return Err(bad("I-Ref", format!("{what}: query {} has no single value to use", a[0])));
                    }
                }
                "LOCAL" | "STORE" | "GIVEN" => {
                    let fits = !a[0].is_boolean() && a[0].as_u64().map(|k| (k as usize) < locals).unwrap_or(false);
                    if !fits {
                        return Err(bad("I-Ref", format!("{what}: local {} is out of range", a[0])));
                    }
                    if op == "GIVEN" && !a[1].is_string() {
                        return Err(bad("I-Format", format!("{what}: GIVEN needs a reason text")));
                    }
                }
                "JMP" | "JF" | "JT" => {
                    let forward = !a[0].is_boolean() && a[0].as_u64().map(|t| pc < t as usize && t as usize <= size).unwrap_or(false);
                    if !forward {
                        return Err(bad("I-Stack", format!("{what}: jump at {pc} to {} must go forward inside the block", a[0])));
                    }
                    target = Some(a[0].as_u64().unwrap() as usize);
                }
                "K" => {
                    self.rf(&a[0], "actors", &what)?;
                    self.inner(&a[1], ci, &what)?;
                }
                "AT" => {
                    self.point(&a[0], &a[1], &what)?;
                    self.inner(&a[2], ci, &what)?;
                    if !a[3].is_boolean() {
                        return Err(bad("I-Format", format!("{what}: AT needs a true/false flag")));
                    }
                }
                "REACH" => self.point(&a[0], &a[1], &what)?,
                "REQUIRE" => {
                    if !a[0].is_string() {
                        return Err(bad("I-Format", format!("{what}: REQUIRE needs a reason text")));
                    }
                }
                _ if CELL_OPS.contains(&op) => {
                    self.rf(&a[0], "cells", &what)?;
                }
                _ if ASSET_OPS.contains(&op) => {
                    self.rf(&a[0], "assets", &what)?;
                }
                _ if SLOT_OPS.contains(&op) => {
                    self.rf(&a[0], "slots", &what)?;
                }
                _ => {}
            }
            ops.push((op, pops, pushes, target));
        }
        let mut depth: Vec<Option<usize>> = vec![None; size + 1];
        depth[0] = Some(0);
        let mut todo = vec![0usize];
        while let Some(pc) = todo.pop() {
            if pc == size {
                continue;
            }
            let (op, pops, pushes, target) = ops[pc];
            let have = depth[pc].unwrap();
            if have < pops {
                return Err(bad("I-Stack", format!("{what}: {op} at {pc} needs {pops} values, the stack has {have}")));
            }
            let after = have - pops + pushes;
            let nexts: Vec<usize> = match op {
                "JMP" => vec![target.unwrap()],
                "JF" | "JT" => vec![pc + 1, target.unwrap()],
                _ => vec![pc + 1],
            };
            for nxt in nexts {
                match depth[nxt] {
                    None => {
                        depth[nxt] = Some(after);
                        todo.push(nxt);
                    }
                    Some(d) if d != after => {
                        return Err(bad("I-Stack", format!("{what}: paths meet at {nxt} with different stack depths")));
                    }
                    _ => {}
                }
            }
        }
        if depth[size] != Some(leaves) {
            let found = depth[size].map(|d| d.to_string()).unwrap_or_else(|| "None".into());
            return Err(bad("I-Stack", format!("{what}: leaves {found} values, layer {mode} needs {leaves}")));
        }
        Ok(())
    }
}

// ---- file text -------------------------------------------------------------------------------

fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*k).clone()).to_string());
                out.push(':');
                canonical(&map[*k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// JSON text with sorted keys and no spaces (what `fean.ir.dumps` writes, without the newline).
pub fn canonical_text(v: &Value) -> String {
    let mut out = String::new();
    canonical(v, &mut out);
    out
}

/// The image as file text: equal images are equal bytes.
pub fn dumps(image: &Value) -> String {
    canonical_text(image) + "\n"
}

/// Read file text into a verified image.
pub fn loads(text: &str) -> R<Value> {
    let image: Value = serde_json::from_str(text).map_err(|e| bad("I-Format", format!("the file is not JSON ({e})")))?;
    verify(&image)?;
    Ok(image)
}

// ---- listing ---------------------------------------------------------------------------------

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn u(v: &Value) -> usize {
    v.as_u64().unwrap_or(0) as usize
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}
fn joined(v: &Value, sep: &str) -> String {
    items(v).iter().map(s).collect::<Vec<_>>().join(sep)
}
fn numbers(v: &Value) -> String {
    format!("[{}]", items(v).iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))
}

fn instr_text(im: &Value, instr: &Value) -> String {
    let parts = items(instr);
    let (op, a) = (s(&parts[0]), &parts[1..]);
    let mut note = String::new();
    if CELL_OPS.contains(&op) {
        let declared = items(&im["cells"]);
        let i = u(&a[0]);
        note = if i < declared.len() {
            s(&declared[i]["name"]).to_string()
        } else {
            let n = items(&im["actors"]).len();
            let (x, who) = ((i - declared.len()) / n, (i - declared.len()) % n);
            format!("bal({},{})", s(&im["actors"][who]), s(&im["assets"][x]["name"]))
        };
    } else if ASSET_OPS.contains(&op) {
        note = s(&im["assets"][u(&a[0])]["name"]).to_string();
    } else if SLOT_OPS.contains(&op) {
        let slot = &im["slots"][u(&a[0])];
        note = format!("{}.{}", s(&im["actors"][u(&slot["actor"])]), s(&slot["key"]));
    } else if op == "PARAM" {
        note = s(&im["params"][u(&a[0])]["name"]).to_string();
    } else if op == "UNK" {
        note = s(&im["unknowns"][u(&a[0])]["name"]).to_string();
    } else if op == "QUERY" {
        note = s(&im["queries"][u(&a[0])]["name"]).to_string();
    } else if op == "K" {
        note = s(&im["actors"][u(&a[0])]).to_string();
    } else if op == "AT" || op == "REACH" {
        let kind = u(&a[0]);
        note = POINTS[kind].to_string();
        if kind >= 2 {
            note += &format!(" {}", s(&im[if kind == 2 { "menus" } else { "marks" }][u(&a[1])]));
        }
    }
    let mut words = vec![op.to_string()];
    words.extend(a.iter().map(|x| x.to_string()));
    let text = words.join(" ");
    if note.is_empty() {
        text
    } else {
        format!("{text:24}; {note}")
    }
}

fn query_text(im: &Value, q: &Value) -> String {
    let names = |xs: &Value| format!("{{{}}}", items(xs).iter().map(|a| s(&im["actors"][u(a)])).collect::<Vec<_>>().join(", "));
    match s(&q["kind"]) {
        kind @ ("exists" | "forall") => format!("{kind} run c{}", q["code"]),
        "can" => format!("can {} c{}", names(&q["who"]), q["code"]),
        "num" => {
            let mut head = String::new();
            if !q["policy"].is_null() {
                head += &format!("max policy {} ", names(&q["policy"]));
            }
            if !q["weight"].is_null() {
                head += &format!("expect {} ", s(&im["weights"][u(&q["weight"])]["name"]));
            }
            format!("{head}{} run c{}", s(&q["opt"]), q["code"])
        }
        "region" => {
            let params: Vec<&str> = items(&q["params"]).iter().map(|p| s(&im["params"][u(p)]["name"])).collect();
            format!("region {}: {}", params.join(", "), query_text(im, &q["inner"]))
        }
        _ => {
            let tail = if q["weight"].is_null() {
                String::new()
            } else {
                format!(" under {}", s(&im["weights"][u(&q["weight"])]["name"]))
            };
            format!(
                "prefer {} at menu {}: {} over {} by c{}{tail}",
                s(&im["actors"][u(&q["who"])]),
                s(&im["menus"][u(&q["menu"])]),
                s(&im["actions"][u(&q["a"])]),
                s(&im["actions"][u(&q["b"])]),
                q["code"]
            )
        }
    }
}

/// A readable listing of a verified image (the same text as `fean.ir.listing`).
pub fn listing(im: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    let actors = items(&im["actors"]);
    out.push(format!("image {}   format {} {}   language {}", s(&im["model"]), s(&im["format"]), im["version"], s(&im["language"])));
    out.push(format!(
        "actors   {}",
        actors.iter().enumerate().map(|(i, a)| format!("{i} {}", s(a))).collect::<Vec<_>>().join("  ")
    ));
    out.push(format!("time     {}..{}", im["time"][0], im["time"][1]));
    for (i, p) in items(&im["params"]).iter().enumerate() {
        out.push(format!("param    {i} {} : {} {{{}}}", s(&p["name"]), s(&p["kind"]), joined(&p["show"], ", ")));
    }
    for (i, un) in items(&im["unknowns"]).iter().enumerate() {
        out.push(format!("unknown  {i} {} : {} {}..{}", s(&un["name"]), s(&un["kind"]), un["lo"], un["hi"]));
    }
    for (i, e) in items(&im["enums"]).iter().enumerate() {
        out.push(format!("enum     {i} {} {{{}}}", s(&e["name"]), joined(&e["values"], ", ")));
    }
    out.push("cells".into());
    let codes_of = |v: &Value| items(v).iter().map(|c| format!("c{c}")).collect::<Vec<_>>().join(" ");
    for (i, c) in items(&im["cells"]).iter().enumerate() {
        let mut dom = s(&c["dom"]["kind"]).to_string();
        if dom == "enum" {
            dom += &format!(" {}", s(&im["enums"][u(&c["dom"]["enum"])]["name"]));
        }
        let rng = if c["lo"].is_null() { String::new() } else { format!("  range c{}..c{}", c["lo"], c["hi"]) };
        let opt = if c["opt"].as_bool() == Some(true) { "?" } else { "" };
        out.push(format!("  {i:4} {:18} {dom}{opt}{rng}  init {}", s(&c["name"]), codes_of(&c["init"])));
    }
    let base = items(&im["cells"]).len();
    for (x, a) in items(&im["assets"]).iter().enumerate() {
        let first = base + x * actors.len();
        out.push(format!(
            "  {first:4}..{} holdings of {}  range c{}..c{}  one cell per actor",
            first + actors.len() - 1,
            s(&a["name"]),
            a["lo"],
            a["hi"]
        ));
    }
    for (i, slot) in items(&im["slots"]).iter().enumerate() {
        out.push(format!("slot     {i} {}.{}", s(&actors[u(&slot["actor"])]), s(&slot["key"])));
    }
    for (i, who) in actors.iter().enumerate() {
        out.push(format!("view     {}: cells {}  slots {}", s(who), numbers(&im["obs"][i]), numbers(&im["mems"][i])));
    }
    out.push("events".into());
    for (i, ev) in items(&im["events"]).iter().enumerate() {
        let ensure: String = items(&ev["ensure"]).iter().map(|c| format!(" c{c}")).collect();
        let net = if ev["net"].as_bool() == Some(true) { " net" } else { "" };
        let tail = if ensure.is_empty() { String::new() } else { format!("  ensure{ensure}") };
        out.push(format!(
            "  {i:4} {:18} params {} locals {}{net}  guard c{}  effects c{}{tail}",
            s(&ev["name"]),
            ev["params"],
            ev["locals"],
            ev["guard"],
            ev["effects"]
        ));
    }
    out.push("units".into());
    for (i, unit) in items(&im["units"]).iter().enumerate() {
        let members: Vec<String> = items(&unit["members"])
            .iter()
            .map(|m| format!("{}({})", s(&im["events"][u(&m["event"])]["name"]), codes_of(&m["args"])))
            .collect();
        let net = if unit["net"].as_bool() == Some(true) { " net" } else { "" };
        out.push(format!("  {i:4} {}{net}: {}", s(&unit["name"]), members.join(", ")));
    }
    out.push(format!("program  entry b{}", im["entry"]));
    for (b, stmts) in items(&im["blocks"]).iter().enumerate() {
        let mut parts = Vec::new();
        for st in items(stmts) {
            let st = items(st);
            parts.push(match s(&st[0]) {
                "at" => format!("at c{} b{}", st[1], st[2]),
                "run" => format!("run u{}", st[1]),
                "attempt" => format!("attempt u{} b{} else b{}", st[1], st[2], st[3]),
                "alt" => format!("alt {}", items(&st[1]).iter().map(|x| format!("b{x}")).collect::<Vec<_>>().join(" or ")),
                "choose" => {
                    let arms: Vec<String> =
                        items(&st[3]).iter().map(|arm| format!("{}=>b{}", s(&im["actions"][u(&arm[0])]), arm[1])).collect();
                    format!("choose {} menu {} {{{}}}", s(&actors[u(&st[1])]), s(&im["menus"][u(&st[2])]), arms.join(" "))
                }
                _ => format!("mark {}", s(&im["marks"][u(&st[1])])),
            });
        }
        out.push(format!("  b{b}: {}", if parts.is_empty() { "(empty)".to_string() } else { parts.join("; ") }));
    }
    for (i, w) in items(&im["weights"]).iter().enumerate() {
        out.push(format!("weight   {i} {} c{}", s(&w["name"]), w["code"]));
    }
    out.push("queries".into());
    for q in items(&im["queries"]) {
        let unit = if s(&q["unit"]).is_empty() { String::new() } else { format!("  [{}]", s(&q["unit"])) };
        out.push(format!("  {} = {}{unit}", s(&q["name"]), query_text(im, &q["q"])));
    }
    if !items(&im["solves"]).is_empty() {
        out.push("solves".into());
    }
    for sv in items(&im["solves"]) {
        let mut head = s(&sv["form"]).to_string();
        if !sv["objective"].is_null() {
            head += &format!(" c{}", sv["objective"]);
        }
        if !sv["point"].is_null() {
            head += &format!(" at {}", s(&im["marks"][u(&sv["point"])]));
        }
        if !sv["where"].is_null() {
            head += &format!(" where c{}", sv["where"]);
        }
        let unit = if s(&sv["unit"]).is_empty() { String::new() } else { format!("  [{}]", s(&sv["unit"])) };
        out.push(format!("  {} = {head}: c{}{unit}", s(&sv["name"]), sv["goal"]));
    }
    out.push("codes".into());
    for (i, c) in items(&im["codes"]).iter().enumerate() {
        let locals = if u(&c["locals"]) > 0 { format!(", locals {}", c["locals"]) } else { String::new() };
        out.push(format!("  c{i} [{}{locals}]", s(&c["mode"])));
        for (pc, instr) in items(&c["ins"]).iter().enumerate() {
            out.push(format!("      {pc:3} {}", instr_text(im, instr)));
        }
    }
    out.join("\n") + "\n"
}

// ---- the typed form the machine runs ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dom {
    Qty,
    Bool,
    Enum(usize),
    Time,
    Actor,
}

#[derive(Clone, Debug)]
pub enum Ins {
    Push(i64),
    Param(usize),
    Local(usize),
    Store(usize),
    Dup,
    Pop,
    Load(usize),
    Bal(usize),
    Now,
    Mem(usize),
    Asof(usize),
    Recv(usize),
    Post(usize),
    Add,
    Sub,
    Mul,
    Min,
    Max,
    DivF,
    DivC,
    DivX,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Neg,
    Not,
    Known,
    Jmp(usize),
    Jf(usize),
    Jt(usize),
    K(usize, usize),
    At(u8, usize, usize, bool),
    Reach(u8, usize),
    Require(usize),
    Given(usize, usize),
    FxSet(usize),
    FxAdd(usize),
    FxFree(usize),
    FxMove(usize),
    FxTell(usize),
    FxForget(usize),
    Unk(usize),
    Query(usize),
}

#[derive(Clone, Debug)]
pub struct CodeBlock {
    pub mode: char,
    pub locals: usize,
    pub ins: Vec<Ins>,
}

#[derive(Clone, Debug)]
pub struct AssetInfo {
    pub name: String,
    pub lo: usize,
    pub hi: usize,
    pub init: Vec<Vec<usize>>,
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub kind: String,
    pub values: Vec<i64>,
    pub show: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CellInfo {
    pub name: String,
    pub dom: Dom,
    pub opt: bool,
    pub lo: Option<usize>,
    pub hi: Option<usize>,
    pub init: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct SlotInfo {
    pub actor: usize,
    pub key: String,
    pub dom: Dom,
}

#[derive(Clone, Debug)]
pub struct EventInfo {
    pub name: String,
    pub params: usize,
    pub locals: usize,
    pub net: bool,
    pub guard: usize,
    pub effects: usize,
    pub ensure: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct UnitInfo {
    pub name: String,
    pub net: bool,
    pub members: Vec<(usize, Vec<usize>)>,
}

#[derive(Clone, Debug)]
pub enum StmtI {
    At(usize, usize),
    Run(usize),
    Attempt(usize, usize, usize),
    Alt(Vec<usize>),
    Choose(usize, usize, Vec<(usize, usize)>),
    Mark(usize),
}

#[derive(Clone, Debug)]
pub enum QueryI {
    Exists(usize),
    Forall(usize),
    Can(Vec<usize>, usize),
    Num { policy: Option<Vec<usize>>, weight: Option<usize>, max: bool, code: usize },
    Region(Vec<usize>, Box<QueryI>),
    Prefer { who: usize, menu: usize, a: usize, b: usize, code: usize, weight: Option<usize> },
}

#[derive(Clone, Debug)]
pub struct QueryInfo {
    pub name: String,
    pub unit: String,
    pub q: QueryI,
}

/// An unknown: its values are the whole numbers lo..=hi (enum values, actors and truth values by number).
#[derive(Clone, Debug)]
pub struct UnknownInfo {
    pub name: String,
    pub kind: String,
    pub lo: i64,
    pub hi: i64,
    pub enum_: Option<usize>,
}

/// A solve: what to find, where, under which filter, for which goal.
#[derive(Clone, Debug)]
pub struct SolveInfo {
    pub name: String,
    /// "find", "any", "min" or "max".
    pub form: String,
    /// The mark the candidates' states are taken at, if any.
    pub point: Option<usize>,
    pub filter: Option<usize>,
    pub objective: Option<usize>,
    pub goal: usize,
    pub unit: String,
}

/// A verified image, decoded for the machine. Every reference is an index.
#[derive(Clone, Debug)]
pub struct Image {
    pub model: String,
    pub actors: Vec<String>,
    pub assets: Vec<AssetInfo>,
    pub enums: Vec<(String, Vec<String>)>,
    pub time: (i64, i64),
    pub params: Vec<ParamInfo>,
    pub cells: Vec<CellInfo>,
    pub slots: Vec<SlotInfo>,
    pub obs: Vec<Vec<usize>>,
    pub mems: Vec<Vec<usize>>,
    pub assumes: Vec<usize>,
    pub invariants: Vec<usize>,
    pub events: Vec<EventInfo>,
    pub units: Vec<UnitInfo>,
    pub blocks: Vec<Vec<StmtI>>,
    pub entry: usize,
    pub menus: Vec<String>,
    pub marks: Vec<String>,
    pub actions: Vec<String>,
    pub weights: Vec<(String, usize)>,
    pub queries: Vec<QueryInfo>,
    pub unknowns: Vec<UnknownInfo>,
    pub solves: Vec<SolveInfo>,
    pub codes: Vec<CodeBlock>,
    /// Reason texts of REQUIRE and GIVEN, referenced by number.
    pub reasons: Vec<String>,
}

fn list(v: &Value) -> Vec<usize> {
    items(v).iter().map(u).collect()
}

fn strings(v: &Value) -> Vec<String> {
    items(v).iter().map(|x| s(x).to_string()).collect()
}

fn dom_of(v: &Value) -> Dom {
    match s(&v["kind"]) {
        "qty" => Dom::Qty,
        "bool" => Dom::Bool,
        "enum" => Dom::Enum(u(&v["enum"])),
        "time" => Dom::Time,
        _ => Dom::Actor,
    }
}

fn constant(v: &Value) -> i64 {
    match v {
        Value::Null => NONE,
        Value::Bool(b) => *b as i64,
        other => other.as_i64().unwrap_or(0),
    }
}

fn query_of(q: &Value) -> QueryI {
    let opt_ref = |v: &Value| if v.is_null() { None } else { Some(u(v)) };
    match s(&q["kind"]) {
        "exists" => QueryI::Exists(u(&q["code"])),
        "forall" => QueryI::Forall(u(&q["code"])),
        "can" => QueryI::Can(list(&q["who"]), u(&q["code"])),
        "num" => QueryI::Num {
            policy: if q["policy"].is_null() { None } else { Some(list(&q["policy"])) },
            weight: opt_ref(&q["weight"]),
            max: s(&q["opt"]) == "max",
            code: u(&q["code"]),
        },
        "region" => QueryI::Region(list(&q["params"]), Box::new(query_of(&q["inner"]))),
        _ => QueryI::Prefer {
            who: u(&q["who"]),
            menu: u(&q["menu"]),
            a: u(&q["a"]),
            b: u(&q["b"]),
            code: u(&q["code"]),
            weight: opt_ref(&q["weight"]),
        },
    }
}

impl Image {
    /// Decode a verified image. Call `verify` (or `loads`) first.
    pub fn decode(im: &Value) -> Image {
        let mut reasons: Vec<String> = Vec::new();
        let mut reason = |text: &str| -> usize {
            match reasons.iter().position(|r| r == text) {
                Some(i) => i,
                None => {
                    reasons.push(text.to_string());
                    reasons.len() - 1
                }
            }
        };
        let codes = items(&im["codes"])
            .iter()
            .map(|c| CodeBlock {
                mode: s(&c["mode"]).chars().next().unwrap_or('C'),
                locals: u(&c["locals"]),
                ins: items(&c["ins"])
                    .iter()
                    .map(|instr| {
                        let p = items(instr);
                        let a = |k: usize| u(&p[k]);
                        match s(&p[0]) {
                            "PUSH" => Ins::Push(constant(&p[1])),
                            "PARAM" => Ins::Param(a(1)),
                            "LOCAL" => Ins::Local(a(1)),
                            "STORE" => Ins::Store(a(1)),
                            "DUP" => Ins::Dup,
                            "POP" => Ins::Pop,
                            "LOAD" => Ins::Load(a(1)),
                            "BAL" => Ins::Bal(a(1)),
                            "NOW" => Ins::Now,
                            "MEM" => Ins::Mem(a(1)),
                            "ASOF" => Ins::Asof(a(1)),
                            "RECV" => Ins::Recv(a(1)),
                            "POST" => Ins::Post(a(1)),
                            "ADD" => Ins::Add,
                            "SUB" => Ins::Sub,
                            "MUL" => Ins::Mul,
                            "MIN" => Ins::Min,
                            "MAX" => Ins::Max,
                            "DIVF" => Ins::DivF,
                            "DIVC" => Ins::DivC,
                            "DIVX" => Ins::DivX,
                            "EQ" => Ins::Eq,
                            "NE" => Ins::Ne,
                            "LT" => Ins::Lt,
                            "LE" => Ins::Le,
                            "GT" => Ins::Gt,
                            "GE" => Ins::Ge,
                            "NEG" => Ins::Neg,
                            "NOT" => Ins::Not,
                            "KNOWN" => Ins::Known,
                            "JMP" => Ins::Jmp(a(1)),
                            "JF" => Ins::Jf(a(1)),
                            "JT" => Ins::Jt(a(1)),
                            "K" => Ins::K(a(1), a(2)),
                            "AT" => Ins::At(a(1) as u8, a(2), a(3), p[4].as_bool().unwrap_or(false)),
                            "REACH" => Ins::Reach(a(1) as u8, a(2)),
                            "REQUIRE" => Ins::Require(reason(s(&p[1]))),
                            "GIVEN" => Ins::Given(a(1), reason(s(&p[2]))),
                            "FXSET" => Ins::FxSet(a(1)),
                            "FXADD" => Ins::FxAdd(a(1)),
                            "FXFREE" => Ins::FxFree(a(1)),
                            "FXMOVE" => Ins::FxMove(a(1)),
                            "FXTELL" => Ins::FxTell(a(1)),
                            "UNK" => Ins::Unk(a(1)),
                            "QUERY" => Ins::Query(a(1)),
                            _ => Ins::FxForget(a(1)),
                        }
                    })
                    .collect(),
            })
            .collect();
        let blocks = items(&im["blocks"])
            .iter()
            .map(|stmts| {
                items(stmts)
                    .iter()
                    .map(|st| {
                        let p = items(st);
                        match s(&p[0]) {
                            "at" => StmtI::At(u(&p[1]), u(&p[2])),
                            "run" => StmtI::Run(u(&p[1])),
                            "attempt" => StmtI::Attempt(u(&p[1]), u(&p[2]), u(&p[3])),
                            "alt" => StmtI::Alt(list(&p[1])),
                            "choose" => {
                                StmtI::Choose(u(&p[1]), u(&p[2]), items(&p[3]).iter().map(|arm| (u(&arm[0]), u(&arm[1]))).collect())
                            }
                            _ => StmtI::Mark(u(&p[1])),
                        }
                    })
                    .collect()
            })
            .collect();
        Image {
            model: s(&im["model"]).to_string(),
            actors: strings(&im["actors"]),
            assets: items(&im["assets"])
                .iter()
                .map(|a| AssetInfo {
                    name: s(&a["name"]).to_string(),
                    lo: u(&a["lo"]),
                    hi: u(&a["hi"]),
                    init: items(&a["init"]).iter().map(list).collect(),
                })
                .collect(),
            enums: items(&im["enums"]).iter().map(|e| (s(&e["name"]).to_string(), strings(&e["values"]))).collect(),
            time: (im["time"][0].as_i64().unwrap_or(0), im["time"][1].as_i64().unwrap_or(0)),
            params: items(&im["params"])
                .iter()
                .map(|p| ParamInfo {
                    name: s(&p["name"]).to_string(),
                    kind: s(&p["kind"]).to_string(),
                    values: items(&p["values"]).iter().map(constant).collect(),
                    show: strings(&p["show"]),
                })
                .collect(),
            cells: items(&im["cells"])
                .iter()
                .map(|c| CellInfo {
                    name: s(&c["name"]).to_string(),
                    dom: dom_of(&c["dom"]),
                    opt: c["opt"].as_bool().unwrap_or(false),
                    lo: if c["lo"].is_null() { None } else { Some(u(&c["lo"])) },
                    hi: if c["hi"].is_null() { None } else { Some(u(&c["hi"])) },
                    init: list(&c["init"]),
                })
                .collect(),
            slots: items(&im["slots"])
                .iter()
                .map(|x| SlotInfo { actor: u(&x["actor"]), key: s(&x["key"]).to_string(), dom: dom_of(&x["dom"]) })
                .collect(),
            obs: items(&im["obs"]).iter().map(list).collect(),
            mems: items(&im["mems"]).iter().map(list).collect(),
            assumes: list(&im["assumes"]),
            invariants: list(&im["invariants"]),
            events: items(&im["events"])
                .iter()
                .map(|e| EventInfo {
                    name: s(&e["name"]).to_string(),
                    params: u(&e["params"]),
                    locals: u(&e["locals"]),
                    net: e["net"].as_bool().unwrap_or(false),
                    guard: u(&e["guard"]),
                    effects: u(&e["effects"]),
                    ensure: list(&e["ensure"]),
                })
                .collect(),
            units: items(&im["units"])
                .iter()
                .map(|x| UnitInfo {
                    name: s(&x["name"]).to_string(),
                    net: x["net"].as_bool().unwrap_or(false),
                    members: items(&x["members"]).iter().map(|m| (u(&m["event"]), list(&m["args"]))).collect(),
                })
                .collect(),
            blocks,
            entry: u(&im["entry"]),
            menus: strings(&im["menus"]),
            marks: strings(&im["marks"]),
            actions: strings(&im["actions"]),
            weights: items(&im["weights"]).iter().map(|w| (s(&w["name"]).to_string(), u(&w["code"]))).collect(),
            queries: items(&im["queries"])
                .iter()
                .map(|q| QueryInfo { name: s(&q["name"]).to_string(), unit: s(&q["unit"]).to_string(), q: query_of(&q["q"]) })
                .collect(),
            unknowns: items(&im["unknowns"])
                .iter()
                .map(|x| UnknownInfo {
                    name: s(&x["name"]).to_string(),
                    kind: s(&x["kind"]).to_string(),
                    lo: x["lo"].as_i64().unwrap_or(0),
                    hi: x["hi"].as_i64().unwrap_or(0),
                    enum_: x.get("enum").and_then(Value::as_u64).map(|e| e as usize),
                })
                .collect(),
            solves: items(&im["solves"])
                .iter()
                .map(|x| {
                    let opt = |v: &Value| if v.is_null() { None } else { Some(u(v)) };
                    SolveInfo {
                        name: s(&x["name"]).to_string(),
                        form: s(&x["form"]).to_string(),
                        point: opt(&x["point"]),
                        filter: opt(&x["where"]),
                        objective: opt(&x["objective"]),
                        goal: u(&x["goal"]),
                        unit: s(&x["unit"]).to_string(),
                    }
                })
                .collect(),
            codes,
            reasons,
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len() + self.assets.len() * self.actors.len()
    }

    pub fn holding(&self, asset: usize, actor: usize) -> usize {
        self.cells.len() + asset * self.actors.len() + actor
    }

    /// The name of a cell by number: a declared cell, or `bal(actor,asset)`.
    pub fn cell_name(&self, i: usize) -> String {
        if i < self.cells.len() {
            return self.cells[i].name.clone();
        }
        let (x, who) = ((i - self.cells.len()) / self.actors.len(), (i - self.cells.len()) % self.actors.len());
        format!("bal({},{})", self.actors[who], self.assets[x].name)
    }
}
