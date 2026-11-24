//! Surface syntax: tokens, syntax tree and a recursive-descent parser.
//!
//! Port of `fean/syntax.py` (grammar: docs/financial-language.md section 1.1; Map unit I-09-b).
//! The order of checks matches the Python parser so both report the same rule and line.

use std::cell::{Cell, RefCell};
use std::fmt;

/// Deepest nesting of expressions and blocks the front end accepts (DR-31).
pub const DEPTH_LIMIT: usize = 2000;
const NODE_LIMIT: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Syntax,
    Static,
    Model,
    Image,
    /// The search budget was exceeded: the verdict is UNKNOWN (not an error of the model).
    Limit,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Syntax => "SyntaxErr",
            Kind::Static => "StaticErr",
            Kind::Model => "ModelErr",
            Kind::Image => "ImageErr",
            Kind::Limit => "SearchLimit",
        }
    }
}

/// An error named after a rule of the specification.
#[derive(Clone, Debug)]
pub struct FeanError {
    pub kind: Kind,
    pub code: String,
    pub message: String,
    pub line: Option<u32>,
}

impl FeanError {
    pub fn new(kind: Kind, code: &str, message: impl Into<String>, line: Option<u32>) -> Self {
        FeanError { kind, code: code.to_string(), message: message.into(), line }
    }
    pub fn syntax(code: &str, message: impl Into<String>, line: u32) -> Self {
        Self::new(Kind::Syntax, code, message, Some(line))
    }
    pub fn model(code: &str, message: impl Into<String>) -> Self {
        Self::new(Kind::Model, code, message, None)
    }
    pub fn image(code: &str, message: impl Into<String>) -> Self {
        Self::new(Kind::Image, code, message, None)
    }
}

impl fmt::Display for FeanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if let Some(line) = self.line {
            write!(f, " (line {line})")?;
        }
        Ok(())
    }
}

impl std::error::Error for FeanError {}

pub type R<T> = Result<T, FeanError>;

pub const KEYWORDS: &[&str] = &[
    "model", "actor", "asset", "enum", "time", "param", "cell", "slot", "assume", "invariant", "def", "event",
    "contract", "instance", "batch", "proc", "program", "weight", "value", "query", "in", "init", "write",
    "observe", "all", "for", "by", "net", "auth", "require", "given", "let", "ensure", "move", "tell", "asof",
    "forget", "free", "run", "attempt", "else", "at", "alt", "or", "choose", "menu", "do", "label", "if", "then",
    "and", "not", "true", "false", "empty", "now", "known", "floor", "ceil", "exact", "min", "max", "mem", "recv",
    "bal", "post", "K", "reach", "start", "end", "exists", "forall", "can", "policy", "expect", "region",
    "prefer", "over", "under", "POSSIBLE", "ALL", "CAN", "Bool", "Time", "Actor", "Asset", "Num", "Qty",
    "unknown", "solve", "find", "any", "where",
];

const SYMBOLS2: &[&str] = &["..", ":=", "+=", "-=", "->", "=>", "==", "!=", "<=", ">=", "??"];
const SYMBOLS1: &str = "{}()[],:.<>+-*/=?";
const COMPARE: &[&str] = &["==", "!=", "<", "<=", ">", ">="];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tk {
    Id,
    Kw,
    Int,
    Time,
    Sym,
    Comment,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: Tk,
    pub text: String,
    pub line: u32,
    pub start: usize,
    pub end: usize,
}

fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Split a text into tokens. With `trivia`, comments are kept (for the editor); the parser drops them.
pub fn lex(src: &str, trivia: bool) -> R<Vec<Token>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let (mut pos, mut line) = (0usize, 1u32);
    while pos < bytes.len() {
        let c = src[pos..].chars().next().unwrap();
        if is_space(c) {
            if c == '\n' {
                line += 1;
            }
            pos += c.len_utf8();
            continue;
        }
        let rest = &src[pos..];
        if rest.starts_with("//") {
            let len = rest.find('\n').unwrap_or(rest.len());
            if trivia {
                out.push(Token { kind: Tk::Comment, text: rest[..len].to_string(), line, start: pos, end: pos + len });
            }
            pos += len;
            continue;
        }
        let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
        if c == '@' && digits(pos + 1) > 0 {
            let len = 1 + digits(pos + 1);
            out.push(Token { kind: Tk::Time, text: rest[..len].to_string(), line, start: pos, end: pos + len });
            pos += len;
            continue;
        }
        if c.is_ascii_digit() {
            let len = digits(pos);
            out.push(Token { kind: Tk::Int, text: rest[..len].to_string(), line, start: pos, end: pos + len });
            pos += len;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let len = bytes[pos..].iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'_').count();
            let text = &rest[..len];
            let kind = if KEYWORDS.contains(&text) { Tk::Kw } else { Tk::Id };
            out.push(Token { kind, text: text.to_string(), line, start: pos, end: pos + len });
            pos += len;
            continue;
        }
        if let Some(sym) = SYMBOLS2.iter().find(|s| rest.starts_with(**s)) {
            out.push(Token { kind: Tk::Sym, text: sym.to_string(), line, start: pos, end: pos + 2 });
            pos += 2;
            continue;
        }
        if SYMBOLS1.contains(c) {
            out.push(Token { kind: Tk::Sym, text: c.to_string(), line, start: pos, end: pos + 1 });
            pos += 1;
            continue;
        }
        return Err(FeanError::syntax("S-Lex", format!("unexpected character {c:?}"), line));
    }
    out.push(Token { kind: Tk::Eof, text: String::new(), line, start: pos, end: pos });
    Ok(out)
}

pub fn tokenize(src: &str) -> R<Vec<Token>> {
    lex(src, false)
}

// ---- syntax tree -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    Prelude,
    Model,
}

/// Where a declaration or statement was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Loc {
    pub origin: Origin,
    pub line: u32,
}

/// A unit as written: (asset, exponent) in source order.
pub type UnitAst = Vec<(String, i32)>;

#[derive(Clone, Debug, PartialEq)]
pub enum TyAst {
    Bool,
    Time,
    Actor,
    Asset,
    Qty(UnitAst),
    Enum(String),
    Opt(Box<TyAst>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Add,
    Sub,
    Mul,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Or => "or",
            BinOp::And => "and",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DivMode {
    Floor,
    Ceil,
    Exact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Mem,
    Asof,
    Recv,
}

impl Field {
    pub fn text(self) -> &'static str {
        match self {
            Field::Mem => "mem",
            Field::Asof => "asof",
            Field::Recv => "recv",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Point {
    Start,
    End,
    Menu(String),
    Label(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Cell(String),
    Bal(String, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Qty(i64, UnitAst),
    Time(i64),
    Bool(bool),
    Empty,
    Now,
    Name(String),
    Call(String, Vec<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Div(DivMode, Box<Expr>, Box<Expr>),
    MinMax(bool, Box<Expr>, Box<Expr>),
    Known(Box<Expr>),
    Default(Box<Expr>, Box<Expr>),
    Mem(Field, String, String),
    Bal(Box<Expr>, String),
    Post(Target),
    K(String, Box<Expr>),
    /// `at(point, e)`; the flag is set by the checker when `e` is a Bool.
    At(Point, Box<Expr>, Cell<bool>),
    Reach(Point),
}

#[derive(Clone, Debug)]
pub enum Clause {
    Auth(Expr),
    Require(Expr),
    Ensure(Expr),
    Given(String, Expr),
    Let(String, Expr),
    Set(Target, Expr),
    Delta(Target, i64, Expr),
    Free(Target),
    /// `move a -> b : n`; the asset is filled in by the checker.
    Move(Expr, Expr, Expr, RefCell<Option<String>>),
    Tell(String, String, Expr, Expr),
    Forget(String, String),
}

#[derive(Clone, Debug)]
pub enum UnitRef {
    Batch(String),
    Ev(String, Vec<Expr>),
}

#[derive(Clone, Debug)]
pub enum ActionVals {
    List(Vec<Expr>),
    Range(Expr, Expr),
}

#[derive(Clone, Debug)]
pub struct Action {
    pub label: String,
    pub param: Option<(String, ActionVals)>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Run(UnitRef),
    Attempt(UnitRef, Vec<Stmt>, Vec<Stmt>),
    At(Expr, Vec<Stmt>),
    Alt(Vec<Vec<Stmt>>),
    Choose(String, String, Vec<Action>),
    Do(String, Vec<Expr>),
    Label(String),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub enum Query {
    Exists(Expr),
    Forall(Expr),
    Can(Vec<String>, Expr),
    Num { policy: Option<Vec<String>>, weight: Option<String>, max: bool, expr: Expr },
    Region(Vec<String>, Box<Query>),
    Prefer { who: String, point: Point, a: String, b: String, value: String, weight: Option<String> },
}

/// The four forms of a solve: every solution, one solution, or the solution that makes a quantity least or greatest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolveForm {
    Find,
    Any,
    Min,
    Max,
}

impl SolveForm {
    pub fn text(self) -> &'static str {
        match self {
            SolveForm::Find => "find",
            SolveForm::Any => "any",
            SolveForm::Min => "min",
            SolveForm::Max => "max",
        }
    }
}

/// `solve NAME = form [at(label)] [where filter] : goal`.
#[derive(Clone, Debug)]
pub struct Solve {
    pub form: SolveForm,
    pub objective: Option<Expr>,
    pub point: Option<String>,
    pub filter: Option<Expr>,
    pub goal: Expr,
}

#[derive(Clone, Debug)]
pub enum Observe {
    All,
    Some(Vec<String>),
}

pub type Params = Vec<(String, TyAst)>;

#[derive(Clone, Debug)]
pub enum DeclKind {
    Actor(Vec<String>),
    Asset { name: String, lo: Expr, hi: Expr, init: Vec<(String, Vec<Expr>)>, public: bool },
    Enum { name: String, values: Vec<String> },
    Time(i64, i64),
    Param { name: String, ty: TyAst, lo: Option<Expr>, hi: Option<Expr> },
    Unknown { name: String, ty: TyAst, lo: Option<Expr>, hi: Option<Expr> },
    Solve { name: String, solve: Solve },
    Slot { name: String, ty: TyAst, owners: Vec<String> },
    Assume(Expr),
    Invariant(Expr),
    Def { name: String, params: Params, ty: TyAst, body: Expr },
    Contract { name: String, params: Params, members: Vec<Decl> },
    Program(Vec<Stmt>),
    Weight { name: String, expr: Expr },
    Value { name: String, expr: Expr },
    Query { name: String, query: Query },
    Cell { name: String, ty: TyAst, lo: Option<Expr>, hi: Option<Expr>, init: Vec<Expr>, write: Vec<String>, observe: Observe },
    Event { name: String, params: Params, by: Vec<Expr>, net: bool, clauses: Vec<Clause> },
    Batch { name: String, net: bool, members: Vec<(String, Vec<Expr>)> },
    Proc { name: String, params: Params, body: Vec<Stmt> },
    Instance { name: String, contract: String, args: Vec<Expr> },
}

#[derive(Clone, Debug)]
pub struct Decl {
    pub kind: DeclKind,
    pub loc: Loc,
}

/// Depth of an expression tree, measured without recursion.
fn tree_depth(root: &Expr) -> usize {
    let mut deepest = 0;
    let mut stack: Vec<(&Expr, usize)> = vec![(root, 1)];
    while let Some((e, d)) = stack.pop() {
        deepest = deepest.max(d);
        match e {
            Expr::Call(_, args) => stack.extend(args.iter().map(|x| (x, d + 1))),
            Expr::Bin(_, a, b) | Expr::Div(_, a, b) | Expr::MinMax(_, a, b) | Expr::Default(a, b) => {
                stack.push((a.as_ref(), d + 1));
                stack.push((b.as_ref(), d + 1));
            }
            Expr::If(c, a, b) => {
                stack.push((c.as_ref(), d + 1));
                stack.push((a.as_ref(), d + 1));
                stack.push((b.as_ref(), d + 1));
            }
            Expr::Not(a) | Expr::Neg(a) | Expr::Known(a) | Expr::Bal(a, _) | Expr::K(_, a) | Expr::At(_, a, _) => {
                stack.push((a.as_ref(), d + 1))
            }
            _ => {}
        }
    }
    deepest
}

// ---- parser ----------------------------------------------------------------------------------

pub struct Parser {
    toks: Vec<Token>,
    i: usize,
    origin: Origin,
    depth: usize,
    nodes: usize,
    open: usize,
}

impl Parser {
    pub fn new(src: &str, origin: Origin) -> R<Self> {
        Ok(Parser { toks: tokenize(src)?, i: 0, origin, depth: 0, nodes: 0, open: 0 })
    }

    // -- tokens
    fn peek(&self, k: usize) -> &Token {
        &self.toks[(self.i + k).min(self.toks.len() - 1)]
    }
    fn at_k(&self, text: &str, k: usize) -> bool {
        let t = self.peek(k);
        t.text == text && matches!(t.kind, Tk::Kw | Tk::Sym)
    }
    fn at(&self, text: &str) -> bool {
        self.at_k(text, 0)
    }
    fn accept(&mut self, text: &str) -> bool {
        if self.at(text) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn fail<T>(&self, msg: &str) -> R<T> {
        let t = self.peek(0);
        let found = if t.kind == Tk::Eof { "end of input".to_string() } else { format!("{:?}", t.text) };
        Err(FeanError::syntax("S-Parse", format!("{msg}, found {found}"), t.line))
    }
    fn expect(&mut self, text: &str) -> R<()> {
        if self.accept(text) {
            Ok(())
        } else {
            self.fail(&format!("expected {text:?}"))
        }
    }
    fn name(&mut self) -> R<String> {
        if self.peek(0).kind != Tk::Id {
            return self.fail("expected a name");
        }
        self.i += 1;
        Ok(self.toks[self.i - 1].text.clone())
    }
    fn names(&mut self) -> R<Vec<String>> {
        let mut out = vec![self.name()?];
        while self.accept(",") {
            out.push(self.name()?);
        }
        Ok(out)
    }
    fn qname(&mut self) -> R<String> {
        let mut out = self.name()?;
        while self.accept(".") {
            out.push('.');
            out.push_str(&self.name()?);
        }
        Ok(out)
    }
    fn int_text(&self, text: &str, line: u32) -> R<i64> {
        text.parse::<i64>().map_err(|_| FeanError::syntax("S-Lex", format!("integer {text} is too large"), line))
    }
    fn integer(&mut self) -> R<i64> {
        if self.peek(0).kind != Tk::Int {
            return self.fail("expected an integer");
        }
        self.i += 1;
        let t = &self.toks[self.i - 1];
        self.int_text(&t.text, t.line)
    }
    fn args_until(&mut self, close: &str) -> R<Vec<Expr>> {
        let mut out = Vec::new();
        if !self.accept(close) {
            out.push(self.expr()?);
            while self.accept(",") {
                out.push(self.expr()?);
            }
            self.expect(close)?;
        }
        Ok(out)
    }
    fn loc(&self) -> Loc {
        Loc { origin: self.origin, line: self.peek(0).line }
    }
    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > DEPTH_LIMIT {
            return self.fail(&format!("nesting is deeper than {DEPTH_LIMIT}"));
        }
        Ok(())
    }
    fn node(&mut self) -> R<()> {
        self.nodes += 1;
        if self.nodes > NODE_LIMIT {
            return self.fail("this expression is too large");
        }
        Ok(())
    }

    // -- declarations
    pub fn model(&mut self) -> R<(String, Vec<Decl>)> {
        self.expect("model")?;
        let name = self.name()?;
        self.expect("{")?;
        let mut decls = Vec::new();
        while !self.at("}") {
            decls.push(self.decl(false)?);
        }
        self.expect("}")?;
        if self.peek(0).kind != Tk::Eof {
            return self.fail("expected end of input");
        }
        Ok((name, decls))
    }

    pub fn decls_only(&mut self) -> R<Vec<Decl>> {
        let mut out = Vec::new();
        while self.peek(0).kind != Tk::Eof {
            out.push(self.decl(false)?);
        }
        Ok(out)
    }

    fn decl(&mut self, members_only: bool) -> R<Decl> {
        const DECLS: &[&str] = &["actor", "asset", "enum", "time", "param", "slot", "assume", "invariant", "def",
            "contract", "program", "weight", "value", "query", "cell", "event", "batch", "proc", "instance",
            "unknown", "solve"];
        const MEMBERS: &[&str] = &["cell", "event", "batch", "proc", "instance"];
        let loc = self.loc();
        let t = self.peek(0).clone();
        let known = t.kind == Tk::Kw && DECLS.contains(&t.text.as_str());
        if !known || (members_only && !MEMBERS.contains(&t.text.as_str())) {
            return self.fail(if members_only { "expected a contract member" } else { "expected a declaration" });
        }
        self.i += 1;
        let kind = match t.text.as_str() {
            "actor" => DeclKind::Actor(self.names()?),
            "asset" => self.d_asset()?,
            "enum" => {
                let name = self.name()?;
                self.expect("{")?;
                let values = self.names()?;
                self.expect("}")?;
                DeclKind::Enum { name, values }
            }
            "time" => {
                let lo = self.integer()?;
                self.expect("..")?;
                DeclKind::Time(lo, self.integer()?)
            }
            "param" | "unknown" => {
                let name = self.name()?;
                self.expect(":")?;
                let ty = self.type_()?;
                let (mut lo, mut hi) = (None, None);
                if self.accept("in") {
                    lo = Some(self.sum()?);
                    self.expect("..")?;
                    hi = Some(self.sum()?);
                }
                if t.text == "param" {
                    DeclKind::Param { name, ty, lo, hi }
                } else {
                    DeclKind::Unknown { name, ty, lo, hi }
                }
            }
            "solve" => {
                let name = self.name()?;
                self.expect("=")?;
                DeclKind::Solve { name, solve: self.solve()? }
            }
            "slot" => {
                let name = self.name()?;
                self.expect(":")?;
                let ty = self.type_()?;
                self.expect("for")?;
                DeclKind::Slot { name, ty, owners: self.names()? }
            }
            "assume" => DeclKind::Assume(self.expr()?),
            "invariant" => DeclKind::Invariant(self.expr()?),
            "def" => {
                let name = self.name()?;
                self.expect("(")?;
                let params = self.params(")")?;
                self.expect(":")?;
                let ty = self.type_()?;
                self.expect("=")?;
                DeclKind::Def { name, params, ty, body: self.expr()? }
            }
            "contract" => {
                let name = self.name()?;
                self.expect("(")?;
                let params = self.params(")")?;
                self.expect("{")?;
                let mut members = Vec::new();
                while !self.at("}") {
                    members.push(self.decl(true)?);
                }
                self.expect("}")?;
                DeclKind::Contract { name, params, members }
            }
            "program" => DeclKind::Program(self.block()?),
            "weight" => {
                let name = self.name()?;
                self.expect("=")?;
                DeclKind::Weight { name, expr: self.expr()? }
            }
            "value" => {
                let name = self.name()?;
                self.expect("=")?;
                DeclKind::Value { name, expr: self.expr()? }
            }
            "query" => {
                let name = self.name()?;
                self.expect("=")?;
                DeclKind::Query { name, query: self.query()? }
            }
            "cell" => self.d_cell()?,
            "event" => {
                let name = self.name()?;
                let params = if self.accept("(") { self.params(")")? } else { Vec::new() };
                self.expect("by")?;
                let mut by = vec![self.expr()?];
                while self.accept(",") {
                    by.push(self.expr()?);
                }
                let net = self.accept("net");
                self.expect("{")?;
                let mut clauses = Vec::new();
                while !self.at("}") {
                    clauses.push(self.clause()?);
                }
                self.expect("}")?;
                DeclKind::Event { name, params, by, net, clauses }
            }
            "batch" => {
                let name = self.name()?;
                self.expect("=")?;
                let net = self.accept("net");
                self.expect("{")?;
                let mut members = vec![self.call()?];
                while self.accept(",") {
                    members.push(self.call()?);
                }
                self.expect("}")?;
                DeclKind::Batch { name, net, members }
            }
            "proc" => {
                let name = self.name()?;
                self.expect("(")?;
                let params = self.params(")")?;
                DeclKind::Proc { name, params, body: self.block()? }
            }
            _ => {
                let name = self.name()?;
                self.expect("=")?;
                let contract = self.name()?;
                self.expect("(")?;
                DeclKind::Instance { name, contract, args: self.args_until(")")? }
            }
        };
        Ok(Decl { kind, loc })
    }

    fn d_asset(&mut self) -> R<DeclKind> {
        let name = self.name()?;
        self.expect("in")?;
        let lo = self.sum()?;
        self.expect("..")?;
        let hi = self.sum()?;
        let mut init = Vec::new();
        if self.accept("init") {
            loop {
                let who = self.name()?;
                if self.accept("=") {
                    init.push((who, vec![self.expr()?]));
                } else {
                    self.expect("in")?;
                    self.expect("{")?;
                    init.push((who, self.args_until("}")?));
                }
                if !self.accept(",") {
                    break;
                }
            }
        }
        let mut public = false;
        if self.accept("observe") {
            self.expect("all")?;
            public = true;
        }
        Ok(DeclKind::Asset { name, lo, hi, init, public })
    }

    fn d_cell(&mut self) -> R<DeclKind> {
        let name = self.name()?;
        self.expect(":")?;
        let ty = self.type_()?;
        let (mut lo, mut hi) = (None, None);
        if self.accept("in") {
            lo = Some(self.sum()?);
            self.expect("..")?;
            hi = Some(self.sum()?);
        }
        self.expect("init")?;
        let init = if self.accept("in") {
            self.expect("{")?;
            self.args_until("}")?
        } else {
            vec![self.expr()?]
        };
        let mut write = Vec::new();
        if self.accept("write") {
            write.push(self.qname()?);
            while self.accept(",") {
                write.push(self.qname()?);
            }
        }
        let mut observe = Observe::Some(Vec::new());
        if self.accept("observe") {
            observe = if self.accept("all") { Observe::All } else { Observe::Some(self.names()?) };
        }
        Ok(DeclKind::Cell { name, ty, lo, hi, init, write, observe })
    }

    fn params(&mut self, close: &str) -> R<Params> {
        let mut out = Vec::new();
        if !self.accept(close) {
            loop {
                let name = self.name()?;
                self.expect(":")?;
                out.push((name, self.type_()?));
                if !self.accept(",") {
                    break;
                }
            }
            self.expect(close)?;
        }
        Ok(out)
    }

    fn type_(&mut self) -> R<TyAst> {
        let t = self.peek(0).clone();
        let ty = if t.kind == Tk::Kw && matches!(t.text.as_str(), "Bool" | "Time" | "Actor" | "Asset") {
            self.i += 1;
            match t.text.as_str() {
                "Bool" => TyAst::Bool,
                "Time" => TyAst::Time,
                "Actor" => TyAst::Actor,
                _ => TyAst::Asset,
            }
        } else if t.kind == Tk::Kw && t.text == "Num" {
            self.i += 1;
            TyAst::Qty(Vec::new())
        } else if t.kind == Tk::Kw && t.text == "Qty" {
            self.i += 1;
            self.expect("[")?;
            let unit = self.unit()?;
            self.expect("]")?;
            TyAst::Qty(unit)
        } else {
            TyAst::Enum(self.name()?)
        };
        Ok(if self.accept("?") { TyAst::Opt(Box::new(ty)) } else { ty })
    }

    fn unit(&mut self) -> R<UnitAst> {
        let mut out = vec![(self.name()?, 1)];
        while self.at("*") || self.at("/") {
            let sign = if self.peek(0).text == "*" { 1 } else { -1 };
            self.i += 1;
            out.push((self.name()?, sign));
        }
        Ok(out)
    }

    // -- events
    fn clause(&mut self) -> R<Clause> {
        let t = self.peek(0).clone();
        if t.kind == Tk::Kw && t.text != "bal" {
            self.i += 1;
            return match t.text.as_str() {
                "auth" => Ok(Clause::Auth(self.expr()?)),
                "require" => Ok(Clause::Require(self.expr()?)),
                "ensure" => Ok(Clause::Ensure(self.expr()?)),
                "given" | "let" => {
                    let x = self.name()?;
                    self.expect("=")?;
                    let e = self.expr()?;
                    Ok(if t.text == "given" { Clause::Given(x, e) } else { Clause::Let(x, e) })
                }
                "free" => Ok(Clause::Free(self.target()?)),
                "move" => {
                    let a = self.expr()?;
                    self.expect("->")?;
                    let b = self.expr()?;
                    self.expect(":")?;
                    Ok(Clause::Move(a, b, self.expr()?, RefCell::new(None)))
                }
                "tell" => {
                    let who = self.name()?;
                    self.expect(".")?;
                    let slot = self.name()?;
                    self.expect(":=")?;
                    let v = self.expr()?;
                    self.expect("asof")?;
                    Ok(Clause::Tell(who, slot, v, self.expr()?))
                }
                "forget" => {
                    let who = self.name()?;
                    self.expect(".")?;
                    Ok(Clause::Forget(who, self.name()?))
                }
                _ => {
                    self.i -= 1;
                    self.fail("expected a clause")
                }
            };
        }
        let tgt = self.target()?;
        if self.accept(":=") {
            return Ok(Clause::Set(tgt, self.expr()?));
        }
        if self.accept("+=") {
            return Ok(Clause::Delta(tgt, 1, self.expr()?));
        }
        if self.accept("-=") {
            return Ok(Clause::Delta(tgt, -1, self.expr()?));
        }
        self.fail("expected ':=', '+=' or '-='")
    }

    fn target(&mut self) -> R<Target> {
        if self.accept("bal") {
            self.expect("(")?;
            let who = self.name()?;
            self.expect(",")?;
            let asset = self.name()?;
            self.expect(")")?;
            return Ok(Target::Bal(who, asset));
        }
        Ok(Target::Cell(self.qname()?))
    }

    // -- programs
    fn block(&mut self) -> R<Vec<Stmt>> {
        self.enter()?;
        self.expect("{")?;
        let mut out = Vec::new();
        while !self.at("}") {
            out.push(self.stmt()?);
        }
        self.expect("}")?;
        self.depth -= 1;
        Ok(out)
    }

    fn call(&mut self) -> R<(String, Vec<Expr>)> {
        let q = self.qname()?;
        let args = if self.accept("(") { self.args_until(")")? } else { Vec::new() };
        Ok((q, args))
    }

    fn unitref(&mut self) -> R<UnitRef> {
        if self.accept("batch") {
            return Ok(UnitRef::Batch(self.qname()?));
        }
        let (q, args) = self.call()?;
        Ok(UnitRef::Ev(q, args))
    }

    fn stmt(&mut self) -> R<Stmt> {
        let loc = self.loc();
        let kind = if self.accept("run") {
            StmtKind::Run(self.unitref()?)
        } else if self.accept("attempt") {
            let unit = self.unitref()?;
            let yes = self.block()?;
            let no = if self.accept("else") { self.block()? } else { Vec::new() };
            StmtKind::Attempt(unit, yes, no)
        } else if self.accept("at") {
            let t = self.expr()?;
            StmtKind::At(t, self.block()?)
        } else if self.accept("alt") {
            let mut blocks = vec![self.block()?];
            self.expect("or")?;
            blocks.push(self.block()?);
            while self.accept("or") {
                blocks.push(self.block()?);
            }
            StmtKind::Alt(blocks)
        } else if self.accept("choose") {
            let who = self.name()?;
            self.expect("menu")?;
            let menu = self.name()?;
            self.expect("{")?;
            let mut actions = vec![self.action()?];
            while !self.at("}") {
                actions.push(self.action()?);
            }
            self.expect("}")?;
            StmtKind::Choose(who, menu, actions)
        } else if self.accept("do") {
            let q = self.qname()?;
            self.expect("(")?;
            StmtKind::Do(q, self.args_until(")")?)
        } else if self.accept("label") {
            StmtKind::Label(self.name()?)
        } else {
            return self.fail("expected a statement");
        };
        Ok(Stmt { kind, loc })
    }

    fn action(&mut self) -> R<Action> {
        let label = self.name()?;
        let mut param = None;
        if self.accept("(") {
            let x = self.name()?;
            self.expect("in")?;
            let values = if self.accept("{") {
                ActionVals::List(self.args_until("}")?)
            } else {
                let lo = self.sum()?;
                self.expect("..")?;
                ActionVals::Range(lo, self.sum()?)
            };
            self.expect(")")?;
            param = Some((x, values));
        }
        self.expect("=>")?;
        Ok(Action { label, param, body: self.block()? })
    }

    // -- expressions
    pub fn expr(&mut self) -> R<Expr> {
        let top = self.open == 0;
        if top {
            self.nodes = 0;
        }
        self.open += 1;
        self.enter()?;
        let mut e = self.conj()?;
        while self.accept("or") {
            self.node()?;
            e = Expr::Bin(BinOp::Or, Box::new(e), Box::new(self.conj()?));
        }
        self.depth -= 1;
        self.open -= 1;
        if top && tree_depth(&e) > DEPTH_LIMIT {
            return self.fail(&format!("this expression is deeper than {DEPTH_LIMIT}"));
        }
        Ok(e)
    }

    fn conj(&mut self) -> R<Expr> {
        let mut e = self.neg()?;
        while self.accept("and") {
            self.node()?;
            e = Expr::Bin(BinOp::And, Box::new(e), Box::new(self.neg()?));
        }
        Ok(e)
    }

    fn neg(&mut self) -> R<Expr> {
        if self.accept("not") {
            self.enter()?;
            self.node()?;
            let inner = self.neg()?;
            self.depth -= 1;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> R<Expr> {
        let a = self.sum()?;
        let t = self.peek(0).clone();
        if t.kind == Tk::Sym && COMPARE.contains(&t.text.as_str()) {
            self.i += 1;
            let b = self.sum()?;
            let next = self.peek(0);
            if next.kind == Tk::Sym && COMPARE.contains(&next.text.as_str()) {
                return self.fail("comparisons do not chain");
            }
            let op = match t.text.as_str() {
                "==" => BinOp::Eq,
                "!=" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                _ => BinOp::Ge,
            };
            self.node()?;
            return Ok(Expr::Bin(op, Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    fn sum(&mut self) -> R<Expr> {
        let top = self.open == 0;                       // a bound: `sum` can be an entry point too
        if top {
            self.nodes = 0;
        }
        self.open += 1;
        let mut e = self.prod()?;
        while self.at("+") || self.at("-") {
            let op = if self.peek(0).text == "+" { BinOp::Add } else { BinOp::Sub };
            self.i += 1;
            self.node()?;
            e = Expr::Bin(op, Box::new(e), Box::new(self.prod()?));
        }
        self.open -= 1;
        if top && tree_depth(&e) > DEPTH_LIMIT {
            return self.fail(&format!("this expression is deeper than {DEPTH_LIMIT}"));
        }
        Ok(e)
    }

    fn prod(&mut self) -> R<Expr> {
        let mut e = self.unary()?;
        while self.accept("*") {
            self.node()?;
            e = Expr::Bin(BinOp::Mul, Box::new(e), Box::new(self.unary()?));
        }
        Ok(e)
    }

    fn unary(&mut self) -> R<Expr> {
        if self.accept("-") {
            self.enter()?;
            self.node()?;
            let inner = self.unary()?;
            self.depth -= 1;
            return Ok(Expr::Neg(Box::new(inner)));
        }
        let mut e = self.atom()?;
        while self.accept("??") {
            self.node()?;
            e = Expr::Default(Box::new(e), Box::new(self.atom()?));
        }
        Ok(e)
    }

    fn atom(&mut self) -> R<Expr> {
        self.node()?;
        let t = self.peek(0).clone();
        match t.kind {
            Tk::Int => {
                self.i += 1;
                let n = self.int_text(&t.text, t.line)?;
                if self.accept("[") {
                    let unit = self.unit()?;
                    self.expect("]")?;
                    return Ok(Expr::Qty(n, unit));
                }
                Ok(Expr::Int(n))
            }
            Tk::Time => {
                self.i += 1;
                Ok(Expr::Time(self.int_text(&t.text[1..], t.line)?))
            }
            Tk::Id => {
                let q = self.qname()?;
                if self.accept("(") {
                    return Ok(Expr::Call(q, self.args_until(")")?));
                }
                Ok(Expr::Name(q))
            }
            Tk::Sym if t.text == "(" => {
                self.i += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tk::Kw => {
                self.i += 1;
                match t.text.as_str() {
                    "true" => Ok(Expr::Bool(true)),
                    "false" => Ok(Expr::Bool(false)),
                    "empty" => Ok(Expr::Empty),
                    "now" => Ok(Expr::Now),
                    "if" => {
                        let c = self.expr()?;
                        self.expect("then")?;
                        let a = self.expr()?;
                        self.expect("else")?;
                        Ok(Expr::If(Box::new(c), Box::new(a), Box::new(self.expr()?)))
                    }
                    "floor" | "ceil" | "exact" => {
                        self.expect("(")?;
                        let a = self.sum()?;
                        self.expect("/")?;
                        let b = self.sum()?;
                        self.expect(")")?;
                        let mode = match t.text.as_str() {
                            "floor" => DivMode::Floor,
                            "ceil" => DivMode::Ceil,
                            _ => DivMode::Exact,
                        };
                        Ok(Expr::Div(mode, Box::new(a), Box::new(b)))
                    }
                    "min" | "max" => {
                        self.expect("(")?;
                        let a = self.expr()?;
                        self.expect(",")?;
                        let b = self.expr()?;
                        self.expect(")")?;
                        Ok(Expr::MinMax(t.text == "min", Box::new(a), Box::new(b)))
                    }
                    "known" => {
                        self.expect("(")?;
                        let e = self.expr()?;
                        self.expect(")")?;
                        Ok(Expr::Known(Box::new(e)))
                    }
                    "mem" | "asof" | "recv" => {
                        self.expect("(")?;
                        let who = self.name()?;
                        self.expect(",")?;
                        let slot = self.name()?;
                        self.expect(")")?;
                        let field = match t.text.as_str() {
                            "mem" => Field::Mem,
                            "asof" => Field::Asof,
                            _ => Field::Recv,
                        };
                        Ok(Expr::Mem(field, who, slot))
                    }
                    "bal" => {
                        self.expect("(")?;
                        let who = self.expr()?;
                        self.expect(",")?;
                        let asset = self.name()?;
                        self.expect(")")?;
                        Ok(Expr::Bal(Box::new(who), asset))
                    }
                    "post" => {
                        self.expect("(")?;
                        let tgt = self.target()?;
                        self.expect(")")?;
                        Ok(Expr::Post(tgt))
                    }
                    "K" => {
                        self.expect("(")?;
                        let who = self.name()?;
                        self.expect(",")?;
                        let e = self.expr()?;
                        self.expect(")")?;
                        Ok(Expr::K(who, Box::new(e)))
                    }
                    "at" => {
                        self.expect("(")?;
                        let p = self.point()?;
                        self.expect(",")?;
                        let e = self.expr()?;
                        self.expect(")")?;
                        Ok(Expr::At(p, Box::new(e), Cell::new(false)))
                    }
                    "reach" => {
                        self.expect("(")?;
                        let p = self.point()?;
                        self.expect(")")?;
                        Ok(Expr::Reach(p))
                    }
                    _ => {
                        self.i -= 1;
                        self.fail("expected an expression")
                    }
                }
            }
            _ => self.fail("expected an expression"),
        }
    }

    fn point(&mut self) -> R<Point> {
        if self.accept("start") {
            return Ok(Point::Start);
        }
        if self.accept("end") {
            return Ok(Point::End);
        }
        if self.accept("menu") {
            self.expect("(")?;
            let q = self.qname()?;
            self.expect(")")?;
            return Ok(Point::Menu(q));
        }
        Ok(Point::Label(self.qname()?))
    }

    // -- queries
    fn coalition(&mut self) -> R<Vec<String>> {
        self.expect("{")?;
        let out = if self.at("}") { Vec::new() } else { self.names()? };
        self.expect("}")?;
        Ok(out)
    }

    fn solve(&mut self) -> R<Solve> {
        let form = match self.peek(0).text.as_str() {
            "find" => SolveForm::Find,
            "any" => SolveForm::Any,
            "min" => SolveForm::Min,
            "max" => SolveForm::Max,
            _ => return self.fail("expected find, any, min or max"),
        };
        if self.peek(0).kind != Tk::Kw {
            return self.fail("expected find, any, min or max");
        }
        self.i += 1;
        let objective = if matches!(form, SolveForm::Min | SolveForm::Max) { Some(self.expr()?) } else { None };
        let mut point = None;
        if self.accept("at") {
            self.expect("(")?;
            point = Some(self.qname()?);
            self.expect(")")?;
        }
        let filter = if self.accept("where") { Some(self.expr()?) } else { None };
        self.expect(":")?;
        Ok(Solve { form, objective, point, filter, goal: self.expr()? })
    }

    fn query(&mut self) -> R<Query> {
        if self.accept("exists") || self.at("forall") {
            let forall = self.accept("forall");
            self.expect("run")?;
            self.expect(":")?;
            let e = self.expr()?;
            return Ok(if forall { Query::Forall(e) } else { Query::Exists(e) });
        }
        if self.accept("can") {
            let who = self.coalition()?;
            self.expect(":")?;
            return Ok(Query::Can(who, self.expr()?));
        }
        if self.accept("region") {
            let params = self.names()?;
            self.expect(":")?;
            return Ok(Query::Region(params, Box::new(self.query()?)));
        }
        if self.accept("prefer") {
            let who = self.name()?;
            self.expect("at")?;
            let point = self.point()?;
            self.expect(":")?;
            let a = self.name()?;
            self.expect("over")?;
            let b = self.name()?;
            self.expect("by")?;
            let value = self.name()?;
            let weight = if self.accept("under") { Some(self.name()?) } else { None };
            return Ok(Query::Prefer { who, point, a, b, value, weight });
        }
        if self.at("POSSIBLE") || self.at("ALL") {
            let possible = self.peek(0).text == "POSSIBLE";
            self.i += 1;
            self.expect("(")?;
            let goal = self.expr()?;
            self.expect(")")?;
            let at = Expr::At(Point::End, Box::new(goal), Cell::new(false));
            return Ok(if possible { Query::Exists(at) } else { Query::Forall(at) });
        }
        if self.accept("CAN") {
            self.expect("(")?;
            let who = self.coalition()?;
            self.expect(",")?;
            let goal = self.expr()?;
            self.expect(")")?;
            return Ok(Query::Can(who, Expr::At(Point::End, Box::new(goal), Cell::new(false))));
        }
        let (mut policy, mut weight) = (None, None);
        if self.at("max") && self.at_k("policy", 1) {
            self.i += 2;
            policy = Some(self.coalition()?);
            self.expect(":")?;
        }
        if self.accept("expect") {
            weight = Some(self.name()?);
            self.expect(":")?;
        }
        if self.at("max") || self.at("min") {
            let max = self.peek(0).text == "max";
            self.i += 1;
            self.expect("run")?;
            self.expect(":")?;
            return Ok(Query::Num { policy, weight, max, expr: self.expr()? });
        }
        self.fail("expected a query")
    }
}

/// Parse a complete model: (name, declarations).
pub fn parse_model(src: &str, origin: Origin) -> R<(String, Vec<Decl>)> {
    Parser::new(src, origin)?.model()
}

/// Parse bare declarations (the standard prelude).
pub fn parse_decls(src: &str, origin: Origin) -> R<Vec<Decl>> {
    Parser::new(src, origin)?.decls_only()
}
