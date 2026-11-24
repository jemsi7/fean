"""FEAN v0.2 static semantics: expansion of templates and parameters, then type and structure checks.

Rules: docs/financial-language.md section 2. Map unit: I-05-c.
Types are tuples: ('Bool',) ('Time',) ('Actor',) ('Asset',) ('Enum', name) ('Qty', unit) ('Opt', type).
A unit is a sorted tuple of (asset, exponent); Num is the empty unit.
"""
from dataclasses import dataclass, field

from .syntax import StaticErr

BOOL, TIME, ACTOR, ASSET = ('Bool',), ('Time',), ('Actor',), ('Asset',)
NUM = ('Qty', ())
NULL = ('Opt', None)            # the type of `empty`
LAYERS = {'C': 'constant', 'S': 'state', 'P': 'post-state', 'Q': 'point', 'R': 'run', 'V': 'goal',
          'X': 'point-goal'}


def opt(t):
    return t if t[0] == 'Opt' else ('Opt', t)


def base(t):
    return t[1] if t[0] == 'Opt' else t


def is_opt(t):
    return t[0] == 'Opt'


def fits(t, want):
    """A value of type t may be used where `want` is expected (tau <: tau?)."""
    if t == NULL:
        return is_opt(want)
    if is_opt(t) and not is_opt(want):
        return False
    return base(t) == base(want)


def unit_of(pairs):
    acc = {}
    for name, exp in pairs:
        acc[name] = acc.get(name, 0) + exp
    return tuple(sorted((n, e) for n, e in acc.items() if e))


def unit_text(unit):
    up = '*'.join(n for n, e in unit if e > 0 for _ in range(e))
    down = ''.join('/' + n for n, e in unit if e < 0 for _ in range(-e))
    return (up or '1') + down


def type_text(t):
    if t == NULL:
        return 'empty'
    if t[0] == 'Opt':
        return type_text(t[1]) + '?'
    if t[0] == 'Qty':
        return f'Qty[{unit_text(t[1])}]' if t[1] else 'Num'
    return t[1] if t[0] == 'Enum' else t[0]


def const_text(node):
    tag = node[0]
    if tag == 'int':
        return str(node[1])
    if tag == 'neg':
        return '-' + const_text(node[1])
    if tag == 'qty':
        return f'{node[1]}[{unit_text(unit_of(node[2]))}]'
    if tag == 'time':
        return f'@{node[1]}'
    if tag == 'bool':
        return 'true' if node[1] else 'false'
    return node[1]


@dataclass
class Cell:
    name: str
    type: tuple
    lo: object
    hi: object
    init: list
    writers: set
    owner: str
    observers: object
    line: int = None


@dataclass
class Event:
    name: str
    params: list
    by: list
    net: bool
    clauses: list
    owner: str
    line: int = None
    writes: tuple = None        # (cells assigned or freed, cells changed by delta, slots written)


@dataclass
class Spec:
    name: str
    actors: list = field(default_factory=list)
    assets: dict = field(default_factory=dict)
    enums: dict = field(default_factory=dict)
    enum_of: dict = field(default_factory=dict)
    time: tuple = (0, 0)
    params: dict = field(default_factory=dict)       # name -> (type, [values])
    unknowns: dict = field(default_factory=dict)     # name -> (type, lo, hi): values are found by solve
    cells: dict = field(default_factory=dict)
    slots: dict = field(default_factory=dict)        # (actor, key) -> type
    assumes: list = field(default_factory=list)
    invariants: list = field(default_factory=list)
    defs: dict = field(default_factory=dict)         # name -> (params, result type, body)
    events: dict = field(default_factory=dict)
    batches: dict = field(default_factory=dict)      # name -> (net, [(event, args)])
    program: list = field(default_factory=list)
    weights: dict = field(default_factory=dict)
    values: dict = field(default_factory=dict)
    queries: dict = field(default_factory=dict)
    qinfo: dict = field(default_factory=dict)        # query name -> ('bool',) | ('num', unit) | ('region', names)
    solves: dict = field(default_factory=dict)       # name -> ('solve', form, objective, label, where, goal)
    sinfo: dict = field(default_factory=dict)        # solve name -> unit of its objective | None
    menus: dict = field(default_factory=dict)        # menu -> [(actor, labels)]
    labels: set = field(default_factory=set)
    notes: dict = field(default_factory=dict)        # id(node) -> fact fixed by the checker


class Scope:
    """Renaming and substitution while a template body is copied."""

    def __init__(self, prefix='', cells=(), units=(), procs=(), insts=(), sub=None, err=None):
        self.prefix, self.cells, self.units = prefix, set(cells), set(units)
        self.procs, self.insts, self.sub, self.err = set(procs), set(insts), dict(sub or {}), err

    def _q(self, q, members):
        head = q.split('.')[0]
        return self.prefix + q if head in members or head in self.insts else q

    def cell(self, q):
        return self._q(q, self.cells)

    def unit(self, q):
        return self._q(q, self.units)

    def proc(self, q):
        return self._q(q, self.procs)

    def local(self, n):
        return self.prefix + n

    def name(self, n):
        """A position that needs a plain name (actor, asset, slot owner)."""
        if n in self.sub:
            node = self.sub[n]
            if node[0] != 'name':
                self.err('W-Static', f'parameter {n} must be given a name here, not {const_text(node)}')
            return node[1]
        return n


def sx(e, sc):
    tag = e[0]
    if tag == 'name':
        return sc.sub[e[1]] if e[1] in sc.sub else ('name', sc.cell(e[1]))
    if tag in ('int', 'time', 'bool', 'empty', 'now', 'at', 'reach'):
        return e
    if tag == 'qty':
        return ('qty', e[1], [(sc.name(n), x) for n, x in e[2]])
    if tag in ('mem', 'asof', 'recv'):
        return (tag, sc.name(e[1]), e[2])
    if tag == 'bal':
        return ('bal', sx(e[1], sc), sc.name(e[2]))
    if tag == 'post':
        return ('post', starget(e[1], sc))
    if tag == 'K':
        return ('K', sc.name(e[1]), sx(e[2], sc))
    if tag == 'call':
        return ('call', e[1], [sx(a, sc) for a in e[2]])
    return (tag,) + tuple(sx(c, sc) if isinstance(c, tuple) else c for c in e[1:])


def starget(t, sc):
    if t[0] == 'balc':
        return ('balc', sc.name(t[1]), sc.name(t[2]))
    return ('cell', sc.cell(t[1]))


def sclause(c, sc):
    k = c[0]
    if k in ('auth', 'require', 'ensure'):
        return (k, sx(c[1], sc))
    if k in ('given', 'let'):
        return (k, c[1], sx(c[2], sc))
    if k == 'set':
        return ('set', starget(c[1], sc), sx(c[2], sc))
    if k == 'delta':
        return ('delta', starget(c[1], sc), c[2], sx(c[3], sc))
    if k == 'free':
        return ('free', starget(c[1], sc))
    if k == 'move':
        return ('move', sx(c[1], sc), sx(c[2], sc), sx(c[3], sc))
    if k == 'tell':
        return ('tell', sc.name(c[1]), c[2], sx(c[3], sc), sx(c[4], sc))
    return ('forget', sc.name(c[1]), c[2])


def sunit(u, sc):
    if u[0] == 'batch':
        return ('batch', sc.unit(u[1]))
    return ('ev', sc.unit(u[1]), [sx(a, sc) for a in u[2]])


def sblock(block, sc):
    return [sstmt(s, sc) for s in block]


def sstmt(s, sc):
    k = s[0]
    if k == 'run':
        return ('run', sunit(s[1], sc))
    if k == 'attempt':
        return ('attempt', sunit(s[1], sc), sblock(s[2], sc), sblock(s[3], sc))
    if k == 'at':
        return ('at', sx(s[1], sc), sblock(s[2], sc))
    if k == 'alt':
        return ('alt', [sblock(b, sc) for b in s[1]])
    if k == 'choose':
        actions = []
        for label, param, blk in s[3]:
            if param is not None:
                x, vals = param
                if isinstance(vals, tuple):
                    vals = ('range', sx(vals[1], sc), sx(vals[2], sc))
                else:
                    vals = [sx(v, sc) for v in vals]
                param = (x, vals)
            actions.append((label, param, sblock(blk, sc)))
        return ('choose', sc.name(s[1]), sc.local(s[2]), actions)
    if k == 'do':
        return ('do', sc.proc(s[1]), [sx(a, sc) for a in s[2]])
    return ('label', sc.local(s[1]))


def stype(t, sc):
    if t[0] == 'Opt':
        return ('Opt', stype(t[1], sc))
    if t[0] == 'Qty':
        return ('Qty', [(sc.name(n), x) for n, x in t[1]])
    return t


def action_params(block):
    """Names bound by parameterised actions anywhere inside a block."""
    out = []
    for st in block:
        k = st[0]
        if k == 'attempt':
            out += action_params(st[2]) + action_params(st[3])
        elif k == 'at':
            out += action_params(st[2])
        elif k == 'alt':
            for b in st[1]:
                out += action_params(b)
        elif k == 'choose':
            for _, param, blk in st[3]:
                if param is not None:
                    out.append(param[0])
                out += action_params(blk)
    return out


def calls_in(e, out):
    if isinstance(e, tuple):
        if e and e[0] == 'call':
            out.add(e[1])
        for c in e:
            calls_in(c, out)
    elif isinstance(e, list):
        for c in e:
            calls_in(c, out)


class Builder:
    def __init__(self, name, decls):
        self.s = Spec(name)
        self.decls = decls
        self.line = None
        self.exprnames = {}         # names usable in expressions -> what they are
        self.contracts, self.procs = {}, {}
        self.raw_program = None
        self.value_types, self.value_busy = {}, set()
        self.const_defs = {}

    def err(self, code, msg):
        raise StaticErr(code, msg, self.line)

    def top(self):
        return Scope(err=self.err)

    def declare(self, name, kind):
        if name in self.exprnames:
            self.err('W-Name', f'{name} is already declared as {self.exprnames[name]}')
        self.exprnames[name] = kind

    def unique(self, table, name, kind):
        if name in table:
            self.err('W-Name', f'{kind} {name} is declared twice')

    # ---- pass 1 and 2: collect and expand -----------------------------------
    def build(self):
        s = self.s
        later = []
        for d in self.decls:
            self.line = d['line']
            k = d['kind']
            if k == 'actor':
                for n in d['names']:
                    self.declare(n, 'an actor')
                    s.actors.append(n)
            elif k == 'asset':
                self.unique(s.assets, d['name'], 'asset')
                s.assets[d['name']] = {'lo': d['lo'], 'hi': d['hi'], 'init': dict(d['init']),
                                       'public': d['public'], 'line': d['line']}
            elif k == 'enum':
                self.unique(s.enums, d['name'], 'enum')
                s.enums[d['name']] = d['values']
                for v in d['values']:
                    self.declare(v, f'a value of {d["name"]}')
                    s.enum_of[v] = d['name']
            elif k == 'time':
                s.time = (d['lo'], d['hi'])
                if d['lo'] > d['hi']:
                    self.err('W-Clock', 'the time range is empty')
            elif k == 'contract':
                self.unique(self.contracts, d['name'], 'contract')
                self.contracts[d['name']] = d
            else:
                later.append(d)
        for d in later:                               # parameters and functions before their users
            self.line = d['line']
            if d['kind'] == 'param':
                self.declare(d['name'], 'a parameter')
                s.params[d['name']] = (self.rtype(d['type']), self.param_domain(d))
            elif d['kind'] == 'unknown':
                self.declare(d['name'], 'an unknown')
                s.unknowns[d['name']] = (self.rtype(d['type']), *self.unknown_range(d))
            elif d['kind'] == 'def':
                self.unique(s.defs, d['name'], 'def')
                params = [(p, self.rtype(t)) for p, t in d['params']]
                s.defs[d['name']] = (params, self.rtype(d['type']), d['body'])
        for d in later:
            self.line = d['line']
            k = d['kind']
            if k in ('cell', 'event', 'batch', 'proc', 'instance'):
                self.add_member(d, self.top(), '', [])
            elif k == 'slot':
                ty = self.rtype(d['type'])
                if is_opt(ty) or ty == ASSET:
                    self.err('E-Tell', f'slot {d["name"]} needs a plain value type')
                for who in d['for']:
                    if who not in s.actors:
                        self.err('T-Name', f'unknown actor {who}')
                    self.unique(s.slots, (who, d['name']), 'slot')
                    s.slots[(who, d['name'])] = ty
            elif k == 'assume':
                s.assumes.append(d['expr'])
            elif k == 'invariant':
                s.invariants.append(d['expr'])
            elif k == 'program':
                if self.raw_program is not None:
                    self.err('W-Name', 'program is declared twice')
                self.raw_program = d
            elif k == 'weight':
                self.unique(s.weights, d['name'], 'weight')
                s.weights[d['name']] = d['expr']
            elif k == 'value':
                self.unique(s.values, d['name'], 'value')
                self.declare(d['name'], 'a value')
                s.values[d['name']] = d['expr']
            elif k == 'query':
                self.unique(s.queries, d['name'], 'query')
                self.unique(s.solves, d['name'], 'solve')
                s.queries[d['name']] = (d['query'], d['line'])
            elif k == 'solve':
                self.unique(s.solves, d['name'], 'solve')
                self.unique(s.queries, d['name'], 'query')
                s.solves[d['name']] = (d['solve'], d['line'])
        self.check_all()
        return s

    def rtype(self, t):
        if t[0] == 'Opt':
            return opt(self.rtype(t[1]))
        if t[0] == 'Qty':
            for n, _ in t[1]:
                if n not in self.s.assets:
                    self.err('T-Lit', f'unknown asset {n} in a unit')
            return ('Qty', unit_of(t[1]))
        if t[0] == 'Enum' and t[1] not in self.s.enums:
            self.err('T-Name', f'unknown type {t[1]}')
        return t

    def const_int(self, node, what):
        if node is not None and node[0] == 'int':
            return node[1]
        if node is not None and node[0] == 'neg' and node[1][0] == 'int':
            return -node[1][1]
        self.err('W-Range', f'{what} needs integer literal bounds')

    def param_domain(self, d):
        ty = self.rtype(d['type'])
        if ty == BOOL:
            return [False, True]
        if ty[0] == 'Enum':
            return list(self.s.enums[ty[1]])
        if ty == TIME:
            return list(range(self.s.time[0], self.s.time[1] + 1))
        if ty == ACTOR:
            return list(self.s.actors)
        if ty[0] == 'Qty':
            if d['lo'] is None:
                self.err('W-Range', f'parameter {d["name"]} needs a range')
            lo, hi = self.const_int(d['lo'], 'a parameter'), self.const_int(d['hi'], 'a parameter')
            if lo > hi:
                self.err('W-Range', f'parameter {d["name"]} has an empty range')
            return list(range(lo, hi + 1))
        self.err('W-Range', f'parameter {d["name"]} cannot have type {type_text(ty)}')

    def unknown_range(self, d):
        """The values an unknown ranges over, as numbers: (lo, hi)."""
        ty, s = self.rtype(d['type']), self.s
        if ty == BOOL:
            return 0, 1
        if ty[0] == 'Enum':
            return 0, len(s.enums[ty[1]]) - 1
        if ty == TIME:
            return s.time
        if ty == ACTOR:
            if not s.actors:
                self.err('W-Range', f'unknown {d["name"]} has no actor to range over')
            return 0, len(s.actors) - 1
        if ty[0] == 'Qty':
            if d['lo'] is None:
                self.err('W-Range', f'unknown {d["name"]} needs a range')
            lo, hi = self.const_int(d['lo'], 'an unknown'), self.const_int(d['hi'], 'an unknown')
            if lo > hi:
                self.err('W-Range', f'unknown {d["name"]} has an empty range')
            return lo, hi
        self.err('W-Range', f'unknown {d["name"]} cannot have type {type_text(ty)}')

    def check_arg(self, arg, want, what):
        if want == ASSET:
            if arg[0] != 'name' or arg[1] not in self.s.assets:
                self.err('P-Args', f'{what} needs an asset name')
            return
        got = self.typeof(arg, 'C', {})
        if not fits(got, want):
            self.err('P-Args', f'{what} needs {type_text(want)}, got {type_text(got)}')

    def binders_ok(self, names, sc):
        """A name bound inside a template body must not hide what the copy substitutes or renames."""
        for n in names:
            if n in sc.sub or n in sc.cells or n in sc.insts:
                self.err('W-Name', f'{n} hides a parameter or member of its template')

    def add_member(self, m, sc, owner, stack):
        s, k = self.s, m['kind']
        name = sc.prefix + m['name']
        if k == 'cell':
            self.declare(name, 'a cell')
            observers = 'all' if m['observe'] == 'all' else [sc.name(a) for a in m['observe']]
            s.cells[name] = Cell(name, self.rtype(stype(m['type'], sc)),
                                 None if m['lo'] is None else sx(m['lo'], sc),
                                 None if m['hi'] is None else sx(m['hi'], sc),
                                 [sx(v, sc) for v in m['init']], {sc.unit(w) for w in m['write']},
                                 owner, observers, self.line)
        elif k == 'event':
            self.unique(s.events, name, 'event')
            self.binders_ok([p for p, _ in m['params']]
                            + [c[1] for c in m['clauses'] if c[0] in ('let', 'given')], sc)
            params = [(p, self.rtype(stype(t, sc))) for p, t in m['params']]
            s.events[name] = Event(name, params, [sx(e, sc) for e in m['by']], m['net'],
                                   [sclause(c, sc) for c in m['clauses']], owner, self.line)
        elif k == 'batch':
            self.unique(s.batches, name, 'batch')
            s.batches[name] = (m['net'], [(sc.unit(q), [sx(a, sc) for a in args]) for q, args in m['members']],
                               self.line)
        elif k == 'proc':
            self.unique(self.procs, name, 'proc')
            self.binders_ok([p for p, _ in m['params']] + action_params(m['body']), sc)
            self.procs[name] = ([(p, stype(t, sc)) for p, t in m['params']], sblock(m['body'], sc))
        else:
            self.instantiate(sc.prefix, m['name'], m['contract'], [sx(a, sc) for a in m['args']], stack)

    def instantiate(self, prefix, name, cname, args, stack):
        if cname not in self.contracts:
            self.err('P-Unit', f'unknown contract {cname}')
        if cname in stack:
            self.err('W-Rec', f'contract {cname} contains itself')
        tmpl = self.contracts[cname]
        if len(args) != len(tmpl['params']):
            self.err('P-Args', f'{cname} takes {len(tmpl["params"])} arguments, got {len(args)}')
        inst = prefix + name
        self.declare(inst, 'an instance')
        kinds = {}
        for m in tmpl['members']:
            kinds.setdefault(m['kind'], set()).add(m['name'])
        sc = Scope(inst + '.', kinds.get('cell', ()), kinds.get('event', set()) | kinds.get('batch', set()),
                   kinds.get('proc', ()), kinds.get('instance', ()), err=self.err)
        for (pname, ptype), arg in zip(tmpl['params'], args):
            self.check_arg(arg, self.rtype(stype(ptype, sc)), f'{cname} parameter {pname}')
            sc.sub[pname] = arg
        for m in tmpl['members']:
            self.add_member(m, sc, inst, stack + [cname])

    def inline(self, block, stack):
        out = []
        for st in block:
            k = st[0]
            if k == 'do':
                q, args = st[1], st[2]
                if q not in self.procs:
                    self.err('P-Unit', f'unknown proc {q}')
                if q in stack:
                    self.err('W-Rec', f'proc {q} calls itself')
                params, body = self.procs[q]
                if len(params) != len(args):
                    self.err('P-Args', f'{q} takes {len(params)} arguments, got {len(args)}')
                sc = self.top()
                for (p, ptype), arg in zip(params, args):
                    self.check_arg(arg, self.rtype(stype(ptype, sc)), f'{q} parameter {p}')
                    sc.sub[p] = arg
                out.extend(self.inline(sblock(body, sc), stack + [q]))
            elif k == 'attempt':
                out.append(('attempt', st[1], self.inline(st[2], stack), self.inline(st[3], stack)))
            elif k == 'at':
                out.append(('at', st[1], self.inline(st[2], stack)))
            elif k == 'alt':
                out.append(('alt', [self.inline(b, stack) for b in st[1]]))
            elif k == 'choose':
                actions = []
                for label, param, blk in st[3]:
                    if param is None:
                        actions.append((label, self.inline(blk, stack)))
                        continue
                    x, vals = param
                    self.fresh(x, {})
                    for v in self.action_values(vals):
                        sc = self.top()
                        sc.sub[x] = v
                        actions.append((f'{label}({const_text(v)})', self.inline(sblock(blk, sc), stack)))
                out.append(('choose', st[1], st[2], actions))
            else:
                out.append(st)
        return out

    def action_values(self, vals):
        if isinstance(vals, tuple):
            lo, hi = self.const_int(vals[1], 'an action range'), self.const_int(vals[2], 'an action range')
            return [('int', n) if n >= 0 else ('neg', ('int', -n)) for n in range(lo, hi + 1)]
        for v in vals:
            literal = v[0] in ('int', 'qty', 'time', 'bool') or (v[0] == 'neg' and v[1][0] == 'int')
            named = v[0] == 'name' and (v[1] in self.s.enum_of or v[1] in self.s.actors)
            if not (literal or named):
                self.err('P-Choose', 'action parameter values must be literals')
        return vals

    # ---- pass 3: types ------------------------------------------------------
    def layer(self, mode, allowed, what):
        if mode not in allowed:
            self.err('T-Layer', f'{what} cannot be used in a {LAYERS[mode]} expression')

    def fresh(self, name, env):
        if name in env or name in self.exprnames:
            self.err('W-Name', f'{name} hides an existing name')

    def want_bool(self, e, mode, env, what, code='T-Bool'):
        t = self.typeof(e, mode, env)
        if t != BOOL:
            self.err(code, f'{what} needs Bool, got {type_text(t)}')

    def value_type(self, name):
        if name not in self.value_types:
            if name in self.value_busy:
                self.err('W-Rec', f'value {name} refers to itself')
            self.value_busy.add(name)
            self.value_types[name] = self.typeof(self.s.values[name], 'R', {})
            self.value_busy.discard(name)
        return self.value_types[name]

    def point(self, p):
        if p[0] == 'label' and p[1] not in self.s.labels:
            self.err('Q-Point', f'no label {p[1]} in the program')
        if p[0] == 'menu' and p[1] not in self.s.menus:
            self.err('Q-Point', f'no menu {p[1]} in the program')

    def typeof(self, e, mode, env):
        s, tag = self.s, e[0]
        if tag == 'int':
            return NUM
        if tag == 'qty':
            for n, _ in e[2]:
                if n not in s.assets:
                    self.err('T-Lit', f'unknown asset {n} in a unit')
            return ('Qty', unit_of(e[2]))
        if tag == 'time':
            if not s.time[0] <= e[1] <= s.time[1]:
                self.err('T-Lit', f'@{e[1]} is outside the declared time range')
            return TIME
        if tag == 'bool':
            return BOOL
        if tag == 'empty':
            return NULL
        if tag == 'now':
            self.layer(mode, 'SPQX', 'now')
            return TIME
        if tag == 'name':
            q = e[1]
            if q in env:
                return env[q]
            if mode in 'VX' and q in s.queries:             # in a goal, a name is first a query
                info = s.qinfo[q]
                if info[0] == 'bool':
                    return BOOL
                if info[0] == 'num':
                    if s.queries[q][2] is not None:
                        self.err('Q-Solve', f'query {q} is an expected value; a goal cannot use it')
                    return ('Qty', info[1])
                self.err('Q-Solve', f'query {q} is a region; it has no single value')
            if q in s.params:
                return s.params[q][0]
            if q in s.unknowns:
                return s.unknowns[q][0]
            if q in s.cells:
                self.layer(mode, 'SPQX', f'cell {q}')
                return s.cells[q].type
            if q in s.enum_of:
                return ('Enum', s.enum_of[q])
            if q in s.actors:
                return ACTOR
            if q in s.values:
                self.layer(mode, 'R', f'value {q}')
                return self.value_type(q)
            self.err('T-Name', f'unknown name {q}')
        if tag == 'bin':
            op = e[1]
            if op in ('and', 'or'):
                self.want_bool(e[2], mode, env, op)
                self.want_bool(e[3], mode, env, op)
                return BOOL
            a, b = self.typeof(e[2], mode, env), self.typeof(e[3], mode, env)
            if op in ('+', '-'):
                if a[0] != 'Qty' or a != b:
                    self.err('T-Arith', f'{type_text(a)} {op} {type_text(b)}')
                return a
            if op == '*':
                if a[0] != 'Qty' or b[0] != 'Qty':
                    self.err('T-Arith', f'{type_text(a)} * {type_text(b)}')
                return ('Qty', unit_of(list(a[1]) + list(b[1])))
            if op in ('==', '!='):
                if base(a) is not None and base(b) is not None and base(a) != base(b):
                    self.err('T-Eq', f'{type_text(a)} {op} {type_text(b)}')
                return BOOL
            if a != b or a[0] not in ('Qty', 'Time'):
                self.err('T-Cmp', f'{type_text(a)} {op} {type_text(b)}')
            return BOOL
        if tag == 'not':
            self.want_bool(e[1], mode, env, 'not')
            return BOOL
        if tag == 'neg':
            a = self.typeof(e[1], mode, env)
            if a[0] != 'Qty':
                self.err('T-Arith', f'-{type_text(a)}')
            return a
        if tag == 'if':
            self.want_bool(e[1], mode, env, 'if')
            a, b = self.typeof(e[2], mode, env), self.typeof(e[3], mode, env)
            if a == NULL or b == NULL:
                return opt(b if a == NULL else a)
            if base(a) != base(b):
                self.err('T-If', f'branches have types {type_text(a)} and {type_text(b)}')
            return opt(a) if is_opt(a) or is_opt(b) else a
        if tag == 'div':
            a, b = self.typeof(e[2], mode, env), self.typeof(e[3], mode, env)
            if a[0] != 'Qty' or b[0] != 'Qty':
                self.err('T-Arith', f'{type_text(a)} / {type_text(b)}')
            res = ('Qty', unit_of(list(a[1]) + [(n, -x) for n, x in b[1]]))
            literal = e[3][0] in ('int', 'qty') and e[3][1] != 0
            return res if literal and e[1] != 'exact' else opt(res)
        if tag == 'minmax':
            a, b = self.typeof(e[2], mode, env), self.typeof(e[3], mode, env)
            if a != b or a[0] not in ('Qty', 'Time'):
                self.err('T-Arith', f'{e[1]}({type_text(a)}, {type_text(b)})')
            return a
        if tag == 'known':
            self.typeof(e[1], mode, env)
            return BOOL
        if tag == 'default':
            a, d = self.typeof(e[1], mode, env), self.typeof(e[2], mode, env)
            if d == NULL or (a != NULL and base(a) != base(d)):
                self.err('T-Opt', f'{type_text(a)} ?? {type_text(d)}')
            return d
        if tag in ('mem', 'asof', 'recv'):
            self.layer(mode, 'SPQX', tag)
            if e[1] not in s.actors:
                self.err('W-Static', f'{tag} needs an actor name, got {e[1]}')
            if (e[1], e[2]) not in s.slots:
                self.err('T-Mem', f'no slot {e[2]} for {e[1]}')
            return opt(s.slots[(e[1], e[2])] if tag == 'mem' else TIME)
        if tag == 'bal':
            self.layer(mode, 'SPQX', 'bal')
            if self.typeof(e[1], mode, env) != ACTOR or e[2] not in s.assets:
                self.err('T-Bal', 'bal needs an actor and a declared asset')
            return ('Qty', ((e[2], 1),))
        if tag == 'post':
            self.layer(mode, 'P', 'post')
            return self.target_type(e[1])
        if tag == 'K':
            self.layer(mode, 'Q', 'K')
            if e[1] not in s.actors:
                self.err('W-Static', f'K needs an actor name, got {e[1]}')
            self.want_bool(e[2], 'Q', env, 'K')
            return BOOL
        if tag == 'at':
            self.layer(mode, 'R', 'at')
            self.point(e[1])
            inner = self.typeof(e[2], 'Q', {})
            s.notes[id(e)] = inner == BOOL
            return BOOL if inner == BOOL else opt(inner)
        if tag == 'reach':
            self.layer(mode, 'R', 'reach')
            self.point(e[1])
            return BOOL
        if tag == 'call':
            if e[1] not in s.defs:
                self.err('T-Call', f'unknown function {e[1]}')
            params, result, body = s.defs[e[1]]
            if len(params) != len(e[2]):
                self.err('T-Call', f'{e[1]} takes {len(params)} arguments, got {len(e[2])}')
            for (p, want), arg in zip(params, e[2]):
                got = self.typeof(arg, mode, env)
                if not fits(got, want):
                    self.err('T-Call', f'{e[1]} parameter {p} needs {type_text(want)}, got {type_text(got)}')
            if mode in 'CRV' and e[1] not in self.const_defs:
                self.const_defs[e[1]] = True
                self.typeof(body, 'C', dict(params))
            return result
        self.err('T-Name', f'unexpected expression {tag}')

    def target_type(self, tgt):
        s = self.s
        if tgt[0] == 'balc':
            if tgt[1] not in s.actors:
                self.err('W-Static', f'bal needs an actor name, got {tgt[1]}')
            if tgt[2] not in s.assets:
                self.err('T-Bal', f'unknown asset {tgt[2]}')
            return ('Qty', ((tgt[2], 1),))
        if tgt[1] not in s.cells:
            code = 'W-Static' if tgt[1] in self.exprnames else 'T-Name'
            self.err(code, f'{tgt[1]} is not a cell')
        return s.cells[tgt[1]].type

    # ---- pass 3: declarations ----------------------------------------------
    def check_all(self):
        s = self.s
        for x, a in s.assets.items():
            self.line = a['line']
            for bound in (a['lo'], a['hi']):
                if self.typeof(bound, 'C', {}) != NUM:
                    self.err('W-Range', f'asset {x} needs integer bounds')
            for who, vals in a['init'].items():
                if who not in s.actors:
                    self.err('T-Name', f'unknown actor {who}')
                for v in vals:
                    if self.typeof(v, 'C', {}) != NUM:
                        self.err('W-Range', f'initial holding of {who} must be an integer count of {x}')
        for c in s.cells.values():
            self.line = c.line
            bt = base(c.type)
            if bt == ASSET:
                self.err('E-Set', f'cell {c.name} cannot hold an asset')
            if bt[0] == 'Qty':
                if c.lo is None:
                    self.err('W-Range', f'quantity cell {c.name} needs a range')
                for bound in (c.lo, c.hi):
                    if self.typeof(bound, 'C', {}) not in (NUM, bt):
                        self.err('W-Range', f'range of {c.name} must be in its own unit')
            elif c.lo is not None:
                self.err('W-Range', f'only quantity cells have a range ({c.name})')
            for v in c.init:
                got = self.typeof(v, 'C', {})
                if not (fits(got, c.type) or (bt[0] == 'Qty' and got == NUM)):
                    self.err('E-Set', f'initial value of {c.name} needs {type_text(c.type)}, got {type_text(got)}')
            if c.observers != 'all':
                for who in c.observers:
                    if who not in s.actors:
                        self.err('T-Name', f'unknown observer {who} of {c.name}')
            for w in c.writers:
                if w not in s.events:
                    self.err('P-Unit', f'unknown writer {w} of {c.name}')
        graph = {}
        for f, (params, result, body) in s.defs.items():
            env = {}
            for p, t in params:
                if t == ASSET:
                    self.err('P-Args', f'def parameter {p} cannot be an asset')
                self.fresh(p, env)
                env[p] = t
            got = self.typeof(body, 'S', env)
            if not fits(got, result):
                self.err('T-Call', f'{f} returns {type_text(got)}, declared {type_text(result)}')
            graph[f] = set()
            calls_in(body, graph[f])
        for f in graph:
            seen, todo = set(), list(graph[f])
            while todo:
                g = todo.pop()
                if g == f:
                    self.err('W-Rec', f'def {f} calls itself')
                if g not in seen:
                    seen.add(g)
                    todo.extend(graph.get(g, ()))
        for e in s.assumes:
            self.want_bool(e, 'S', {}, 'assume')
        for e in s.invariants:
            self.want_bool(e, 'S', {}, 'invariant')
        for ev in s.events.values():
            self.check_event(ev)
        for name, (net, members, line) in list(s.batches.items()):
            self.line = line
            self.check_batch(name, members)
            s.batches[name] = (net, members)
        if self.raw_program is not None:
            self.line = self.raw_program['line']
            s.program = self.inline(sblock(self.raw_program['body'], self.top()), [])
            self.check_block(s.program, set())
            self.check_clock(s.program, s.time[0])
        for name, e in s.weights.items():
            if self.typeof(e, 'S', {}) != NUM:
                self.err('Q-Num', f'weight {name} needs Num')
        for name in s.values:
            self.value_type(name)
        for name, (q, line) in list(s.queries.items()):
            self.line = line
            s.qinfo[name] = self.check_query(q)
            s.queries[name] = q
        for name, (sv, line) in list(s.solves.items()):
            self.line = line
            s.sinfo[name] = self.check_solve(sv)
            s.solves[name] = sv

    def write_target(self, tgt, ev, code, allow_bal):
        s = self.s
        ty = self.target_type(tgt)
        if tgt[0] == 'balc':
            if not allow_bal:
                self.err(code, 'a holding changes only by move, += or -=')
            return ('bal', tgt[1], tgt[2]), ty
        cell = s.cells[tgt[1]]
        allowed = ev.owner == cell.owner if cell.owner else ev.name in cell.writers
        if not allowed:
            self.err('W-Writer', f'{ev.name} may not write {cell.name}')
        return cell.name, ty

    def check_event(self, ev):
        s = self.s
        self.line = ev.line
        env = {}
        for p, t in ev.params:
            if t == ASSET:
                self.err('P-Args', f'event parameter {p} cannot be an asset')
            self.fresh(p, env)
            env[p] = t
        for b in ev.by:
            if self.typeof(b, 'S', env) != ACTOR:
                self.err('T-Name', f'by needs an actor in {ev.name}')
        has_auth = any(c[0] == 'auth' for c in ev.clauses)
        sets, deltas, slots = set(), set(), set()
        for c in ev.clauses:
            k = c[0]
            if k in ('require', 'auth'):
                self.want_bool(c[1], 'S', env, k)
            elif k == 'ensure':
                self.want_bool(c[1], 'P', env, k)
            elif k == 'let':
                self.fresh(c[1], env)
                env[c[1]] = self.typeof(c[2], 'S', env)
            elif k == 'given':
                self.fresh(c[1], env)
                t = self.typeof(c[2], 'S', env)
                if not is_opt(t) or t == NULL:
                    self.err('T-Given', f'given {c[1]} needs an optional value, got {type_text(t)}')
                env[c[1]] = base(t)
            elif k == 'set':
                key, ct = self.write_target(c[1], ev, 'E-Set', False)
                vt = self.typeof(c[2], 'S', env)
                if not fits(vt, ct):
                    self.err('E-Set', f'{key} needs {type_text(ct)}, got {type_text(vt)}')
                if key in sets or key in deltas:
                    self.err('W-Write', f'{key} is written twice in {ev.name}')
                sets.add(key)
            elif k == 'delta':
                key, ct = self.write_target(c[1], ev, 'E-Delta', True)
                vt = self.typeof(c[3], 'S', env)
                if ct[0] != 'Qty' or vt != ct:
                    self.err('E-Delta', f'{key} is {type_text(ct)}, change is {type_text(vt)}')
                if key in sets:
                    self.err('W-Write', f'{key} is written twice in {ev.name}')
                if c[1][0] == 'balc' and not has_auth:
                    self.err('W-Debit', f'{ev.name} creates or destroys holdings without auth')
                deltas.add(key)
            elif k == 'free':
                key, _ = self.write_target(c[1], ev, 'E-Free', False)
                if key in sets or key in deltas:
                    self.err('W-Write', f'{key} is written twice in {ev.name}')
                sets.add(key)
            elif k == 'move':
                if self.typeof(c[1], 'S', env) != ACTOR or self.typeof(c[2], 'S', env) != ACTOR:
                    self.err('E-Move', 'move needs two actors')
                vt = self.typeof(c[3], 'S', env)
                if vt[0] != 'Qty' or len(vt[1]) != 1 or vt[1][0][1] != 1:
                    self.err('E-Move', f'move needs a quantity of one asset, got {type_text(vt)}')
                if not has_auth and c[1] not in ev.by:
                    self.err('W-Debit', f'{ev.name} debits an actor who is not its principal, without auth')
                s.notes[id(c)] = vt[1][0][0]
            else:
                who, slot = c[1], c[2]
                if who not in s.actors:
                    self.err('W-Static', f'{k} needs an actor name, got {who}')
                if (who, slot) not in s.slots:
                    self.err('E-Tell', f'no slot {slot} for {who}')
                if k == 'tell':
                    vt = self.typeof(c[3], 'S', env)
                    if is_opt(vt) or not fits(vt, s.slots[(who, slot)]):
                        self.err('E-Tell', f'{who}.{slot} needs {type_text(s.slots[(who, slot)])}, got {type_text(vt)}')
                    if self.typeof(c[4], 'S', env) != TIME:
                        self.err('E-Tell', 'asof needs a Time')
                if (who, slot) in slots:
                    self.err('W-Write', f'{who}.{slot} is written twice in {ev.name}')
                slots.add((who, slot))
        ev.writes = (sets, deltas, slots)

    def check_args(self, ev, args):
        if len(args) != len(ev.params):
            self.err('P-Args', f'{ev.name} takes {len(ev.params)} arguments, got {len(args)}')
        for (p, want), arg in zip(ev.params, args):
            self.check_arg(arg, want, f'{ev.name} parameter {p}')

    def check_batch(self, name, members):
        s = self.s
        seen, sets, deltas, slots = set(), set(), set(), set()
        for q, args in members:
            if q not in s.events:
                self.err('P-Unit', f'unknown event {q} in batch {name}')
            if q in seen:
                self.err('W-Batch', f'{q} appears twice in batch {name}')
            seen.add(q)
            self.check_args(s.events[q], args)
            es, ed, esl = s.events[q].writes
            if (es & (sets | deltas)) or (ed & sets) or (esl & slots):
                self.err('W-Write', f'batch {name} writes the same target twice')
            sets |= es
            deltas |= ed
            slots |= esl

    def check_unit(self, u):
        s = self.s
        if u[0] == 'batch':
            if u[1] not in s.batches:
                self.err('W-Batch', f'batch {u[1]} is not declared')
        else:
            if u[1] not in s.events:
                self.err('P-Unit', f'unknown event {u[1]}')
            self.check_args(s.events[u[1]], u[2])

    def branches(self, blocks, seen):
        acc = set()
        for b in blocks:
            mine = set(seen)
            self.check_block(b, mine)
            acc |= mine
        seen |= acc

    def check_block(self, block, seen):
        """`seen` holds menu and label names already used on every path into this block."""
        s = self.s
        for st in block:
            k = st[0]
            if k == 'run':
                self.check_unit(st[1])
            elif k == 'attempt':
                self.check_unit(st[1])
                self.branches([st[2], st[3]], seen)
            elif k == 'at':
                if self.typeof(st[1], 'C', {}) != TIME:
                    self.err('W-Clock', 'at needs a Time constant')
                self.check_block(st[2], seen)
            elif k == 'alt':
                self.branches(st[1], seen)
            elif k == 'choose':
                who, menu, actions = st[1], st[2], st[3]
                if who not in s.actors:
                    self.err('W-Static', f'choose needs an actor name, got {who}')
                labels = tuple(label for label, _ in actions)
                if len(set(labels)) != len(labels):
                    self.err('P-Choose', f'menu {menu} repeats an action name')
                if ('menu', menu) in seen:
                    self.err('W-Menu', f'menu {menu} appears twice on one path')
                seen.add(('menu', menu))
                s.menus.setdefault(menu, []).append((who, labels))
                self.branches([blk for _, blk in actions], seen)
            elif k == 'label':
                if ('label', st[1]) in seen:
                    self.err('W-Menu', f'label {st[1]} appears twice on one path')
                seen.add(('label', st[1]))
                s.labels.add(st[1])

    def check_clock(self, block, clock):
        """Latest clock any path can reach; literal `at` times must not go back."""
        for st in block:
            k = st[0]
            if k == 'at':
                if st[1][0] == 'time':
                    if st[1][1] < clock:
                        self.err('W-Clock', f'at @{st[1][1]} goes back from @{clock}')
                    clock = st[1][1]
                clock = self.check_clock(st[2], clock)
            elif k == 'attempt':
                clock = max(self.check_clock(st[2], clock), self.check_clock(st[3], clock))
            elif k == 'alt':
                clock = max(self.check_clock(b, clock) for b in st[1])
            elif k == 'choose':
                clock = max(self.check_clock(blk, clock) for _, blk in st[3])
        return clock

    def check_query(self, q):
        s, k = self.s, q[0]
        if k in ('exists', 'forall'):
            self.want_bool(q[1], 'R', {}, k, 'Q-Bool')
            return ('bool',)
        if k == 'can':
            for who in q[1]:
                if who not in s.actors:
                    self.err('T-Name', f'unknown actor {who}')
            self.want_bool(q[2], 'R', {}, 'can', 'Q-Bool')
            return ('bool',)
        if k == 'num':
            for who in q[1] or ():
                if who not in s.actors:
                    self.err('T-Name', f'unknown actor {who}')
            if q[2] is not None and q[2] not in s.weights:
                self.err('Q-Num', f'unknown weight {q[2]}')
            t = self.typeof(q[4], 'R', {})
            if t[0] != 'Qty':
                self.err('Q-Num', f'{q[3]} run needs a quantity, got {type_text(t)}')
            return ('num', t[1])
        if k == 'region':
            for p in q[1]:
                if p not in s.params:
                    self.err('Q-Region', f'{p} is not a parameter')
            if self.check_query(q[2]) != ('bool',):
                self.err('Q-Region', 'region needs a true/false query inside')
            return ('region', q[1])
        who, point, a, b, value, weight = q[1:]
        if who not in s.actors:
            self.err('T-Name', f'unknown actor {who}')
        if point[0] != 'menu' or point[1] not in s.menus:
            self.err('Q-Prefer', 'prefer needs a menu(m) point that exists in the program')
        offered = {label for _, labels in s.menus[point[1]] for label in labels}
        if a not in offered or b not in offered:
            self.err('Q-Prefer', f'{a} and {b} must be actions of menu {point[1]}')
        if value not in s.values or self.value_type(value)[0] != 'Qty':
            self.err('Q-Prefer', f'{value} must be a declared numeric value')
        if weight is not None and weight not in s.weights:
            self.err('Q-Prefer', f'unknown weight {weight}')
        return ('bool',)

    def check_solve(self, sv):
        """Returns the unit of the objective (None when there is none)."""
        s = self.s
        _, form, objective, label, where, goal = sv
        if label is None:
            if not s.unknowns:
                self.err('Q-Solve', 'nothing to solve for: declare an unknown, or name a point with at(...)')
            mode, before = 'V', 'C'
        else:
            self.point(('label', label))
            times = count_labels(s.program, label)
            if times != 1:
                self.err('Q-Solve', f'label {label} is written {times} times; a point must be one place')
            mode, before = 'X', 'S'
        if where is not None:
            self.want_bool(where, before, {}, 'where', 'Q-Solve')
        unit = None
        if objective is not None:
            t = self.typeof(objective, mode, {})
            if t[0] != 'Qty':
                self.err('Q-Solve', f'{form} needs a quantity, got {type_text(t)}')
            unit = t[1]
        self.want_bool(goal, mode, {}, 'the goal', 'Q-Solve')
        return unit


def count_labels(block, label):
    """How many `label` statements with this name the program has."""
    n = 0
    for st in block:
        k = st[0]
        if k == 'label':
            n += st[1] == label
        elif k == 'attempt':
            n += count_labels(st[2], label) + count_labels(st[3], label)
        elif k == 'at':
            n += count_labels(st[2], label)
        elif k == 'alt':
            n += sum(count_labels(b, label) for b in st[1])
        elif k == 'choose':
            n += sum(count_labels(blk, label) for _, blk in st[3])
    return n


def build(name, decls):
    """Expand and check a parsed model; return its Spec or raise StaticErr."""
    return Builder(name, decls).build()
