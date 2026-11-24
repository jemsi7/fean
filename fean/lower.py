"""Lowering: a checked model (Spec) becomes an image. This is the ahead-of-time compilation step.

Everything that can be decided before running is decided here: names become numbers, `def`s and
values are expanded in place, expressions become stack code, events become guard/effects/ensure
blocks that share one frame of locals. Correspondence table: docs/engine.md section 3. Map unit: I-06-c.
"""
import json

from .ir import FORMAT, LANGUAGE, VERSION
from .static import ACTOR, BOOL, TIME, base, const_text, is_opt, unit_text

BINARY = {'+': 'ADD', '-': 'SUB', '*': 'MUL', '==': 'EQ', '!=': 'NE', '<': 'LT', '<=': 'LE', '>': 'GT', '>=': 'GE'}
DIVIDE = {'floor': 'DIVF', 'ceil': 'DIVC', 'exact': 'DIVX'}


def shown(arg):
    """A unit argument as it appears in the unit's name: literals and names as written, the rest as `…`."""
    if arg[0] == 'neg':
        inner = shown(arg[1])
        return inner if inner == '…' else '-' + inner
    return const_text(arg) if arg[0] in ('int', 'qty', 'time', 'bool', 'name') else '…'


class Frame:
    """Local slots: one frame per event (shared by its blocks), or one per stand-alone block."""

    def __init__(self):
        self.size = 0

    def new(self):
        self.size += 1
        return self.size - 1


class Code:
    def __init__(self, mode, frame=None):
        self.mode, self.frame, self.ins = mode, frame or Frame(), []

    def emit(self, *instr):
        self.ins.append(list(instr))
        return len(self.ins) - 1

    def land(self, at):
        """Make the jump emitted at `at` land on the next instruction."""
        self.ins[at][1] = len(self.ins)


class Lowering:
    def __init__(self, spec):
        s = self.s = spec
        self.actor = {a: i for i, a in enumerate(s.actors)}
        self.asset = {x: i for i, x in enumerate(s.assets)}
        self.enum = {name: i for i, name in enumerate(s.enums)}
        self.enumval = {v: s.enums[e].index(v) for v, e in s.enum_of.items()}
        self.param = {p: i for i, p in enumerate(s.params)}
        self.unknown = {u: i for i, u in enumerate(s.unknowns)}
        self.inside = 0                                  # how many expanded defs we are within
        self.query_no = {q: i for i, q in enumerate(s.queries)}
        self.cell = {c: i for i, c in enumerate(s.cells)}
        self.slot = {k: i for i, k in enumerate(s.slots)}
        self.event = {e: i for i, e in enumerate(s.events)}
        self.weight = {w: i for i, w in enumerate(s.weights)}
        self.menus, self.marks, self.actions = [], [], []
        self.codes, self.units, self.blocks = [], [], []
        self.known = {}

    # ---- tables -------------------------------------------------------------
    def holding(self, who, asset):
        return len(self.cell) + self.asset[asset] * len(self.actor) + self.actor[who]

    @staticmethod
    def intern(table, name):
        if name not in table:
            table.append(name)
        return table.index(name)

    def share(self, table, entry):
        """Equal entries are stored once."""
        key = (id(table), json.dumps(entry, sort_keys=True))
        if key not in self.known:
            self.known[key] = len(table)
            table.append(entry)
        return self.known[key]

    def add(self, code):
        return self.share(self.codes, {'mode': code.mode, 'locals': code.frame.size, 'ins': code.ins})

    def value(self, e, mode):
        code = Code(mode)
        self.expr(e, code, {})
        return self.add(code)

    def target(self, tgt):
        return self.holding(tgt[1], tgt[2]) if tgt[0] == 'balc' else self.cell[tgt[1]]

    def point(self, p):
        if p[0] == 'menu':
            return 2, self.menus.index(p[1])
        if p[0] == 'label':
            return 3, self.marks.index(p[1])
        return (0, 0) if p[0] == 'start' else (1, 0)

    def domain(self, ty):
        bt = base(ty)
        if bt[0] == 'Qty':
            return {'kind': 'qty'}
        if bt == BOOL:
            return {'kind': 'bool'}
        if bt == TIME:
            return {'kind': 'time'}
        if bt == ACTOR:
            return {'kind': 'actor'}
        return {'kind': 'enum', 'enum': self.enum[bt[1]]}

    # ---- expressions --------------------------------------------------------
    def expr(self, e, code, scope):
        tag, emit = e[0], code.emit
        if tag in ('int', 'qty', 'time', 'bool'):
            emit('PUSH', e[1])
        elif tag == 'empty':
            emit('PUSH', None)
        elif tag == 'now':
            emit('NOW')
        elif tag == 'name':
            self.name(e[1], code, scope)
        elif tag == 'bin':
            op = e[1]
            if op in ('and', 'or'):                     # short circuit: the right side runs only if needed
                self.expr(e[2], code, scope)
                skip = emit('JF' if op == 'and' else 'JT', None)
                self.expr(e[3], code, scope)
                done = emit('JMP', None)
                code.land(skip)
                emit('PUSH', op == 'or')
                code.land(done)
            else:
                self.expr(e[2], code, scope)
                self.expr(e[3], code, scope)
                emit(BINARY[op])
        elif tag in ('not', 'neg', 'known'):
            self.expr(e[1], code, scope)
            emit(tag.upper())
        elif tag == 'if':
            self.expr(e[1], code, scope)
            other = emit('JF', None)
            self.expr(e[2], code, scope)
            done = emit('JMP', None)
            code.land(other)
            self.expr(e[3], code, scope)
            code.land(done)
        elif tag == 'div':
            self.expr(e[2], code, scope)
            self.expr(e[3], code, scope)
            emit(DIVIDE[e[1]])
        elif tag == 'minmax':
            self.expr(e[2], code, scope)
            self.expr(e[3], code, scope)
            emit(e[1].upper())
        elif tag == 'default':                           # e ?? d: keep e unless it is empty
            self.expr(e[1], code, scope)
            emit('DUP')
            emit('KNOWN')
            done = emit('JT', None)
            emit('POP')
            self.expr(e[2], code, scope)
            code.land(done)
        elif tag in ('mem', 'asof', 'recv'):
            emit(tag.upper(), self.slot[(e[1], e[2])])
        elif tag == 'bal':
            who = e[1]
            if who[0] == 'name' and who[1] not in scope and who[1] in self.actor:
                emit('LOAD', self.holding(who[1], e[2]))
            else:
                self.expr(who, code, scope)
                emit('BAL', self.asset[e[2]])
        elif tag == 'post':
            emit('POST', self.target(e[1]))
        elif tag == 'call':                              # a def is expanded where it is called
            params, _, body = self.s.defs[e[1]]
            inner = {}
            for (p, _), arg in zip(params, e[2]):
                self.expr(arg, code, scope)
                inner[p] = code.frame.new()
                emit('STORE', inner[p])
            self.inside += 1                             # the body's names are its own, whatever layer calls it
            self.expr(body, code, inner)
            self.inside -= 1
        elif tag == 'K':
            emit('K', self.actor[e[1]], self.value(e[2], 'Q'))
        elif tag == 'at':
            emit('AT', *self.point(e[1]), self.value(e[2], 'Q'), bool(self.s.notes.get(id(e))))
        elif tag == 'reach':
            emit('REACH', *self.point(e[1]))
        else:
            raise AssertionError(f'unchecked expression {tag}')

    def name(self, q, code, scope):
        s = self.s
        if q in scope:
            code.emit('LOCAL', scope[q])
        elif code.mode in 'VX' and not self.inside and q in s.queries:      # the verdict of a query, in a goal
            code.emit('QUERY', self.query_no[q])
        elif q in s.params:
            code.emit('PARAM', self.param[q])
        elif q in s.unknowns:
            code.emit('UNK', self.unknown[q])
        elif q in s.cells:
            code.emit('LOAD', self.cell[q])
        elif q in s.enum_of:
            code.emit('PUSH', self.enumval[q])
        elif q in s.actors:
            code.emit('PUSH', self.actor[q])
        else:                                            # a declared value: its expression, in place
            self.expr(s.values[q], code, {})

    # ---- events, units, program --------------------------------------------
    def lower_event(self, ev):
        frame = Frame()
        scope = {p: frame.new() for p, _ in ev.params}
        guard, effects, ensures = Code('G', frame), Code('E', frame), []
        for c in ev.clauses:
            k = c[0]
            if k == 'let':
                self.expr(c[2], guard, scope)
                scope[c[1]] = frame.new()
                guard.emit('STORE', scope[c[1]])
            elif k == 'given':
                self.expr(c[2], guard, scope)
                scope[c[1]] = frame.new()
                guard.emit('GIVEN', scope[c[1]], f'given {c[1]} in {ev.name}')
            elif k in ('require', 'auth'):
                self.expr(c[1], guard, scope)
                guard.emit('REQUIRE', f'{k} in {ev.name}')
            elif k == 'ensure':
                post = Code('P', frame)
                self.expr(c[1], post, scope)
                ensures.append(post)
            elif k == 'set':
                self.expr(c[2], effects, scope)
                effects.emit('FXSET', self.target(c[1]))
            elif k == 'delta':
                self.expr(c[3], effects, scope)
                if c[2] < 0:
                    effects.emit('NEG')
                effects.emit('FXADD', self.target(c[1]))
            elif k == 'free':
                effects.emit('FXFREE', self.target(c[1]))
            elif k == 'move':
                for part in c[1:4]:
                    self.expr(part, effects, scope)
                effects.emit('FXMOVE', self.asset[self.s.notes[id(c)]])
            elif k == 'tell':
                self.expr(c[3], effects, scope)
                self.expr(c[4], effects, scope)
                effects.emit('FXTELL', self.slot[(c[1], c[2])])
            else:
                effects.emit('FXFORGET', self.slot[(c[1], c[2])])
        return {'name': ev.name, 'params': len(ev.params), 'locals': frame.size, 'net': ev.net,
                'guard': self.add(guard), 'effects': self.add(effects), 'ensure': [self.add(p) for p in ensures]}

    def unit(self, u):
        s = self.s
        if u[0] == 'batch':
            net, members = s.batches[u[1]]
            name = 'batch ' + u[1]
        else:
            net, members = s.events[u[1]].net, [(u[1], u[2])]
            texts = [shown(a) for a in u[2]]
            name = u[1] + (f'({", ".join(texts)})' if texts else '')
        return self.share(self.units, {
            'name': name, 'net': net,
            'members': [{'event': self.event[q], 'args': [self.value(a, 'C') for a in args]} for q, args in members]})

    def block(self, stmts):
        ix = len(self.blocks)
        self.blocks.append(None)
        out = []
        for st in stmts:
            k = st[0]
            if k == 'run':
                out.append(['run', self.unit(st[1])])
            elif k == 'attempt':
                out.append(['attempt', self.unit(st[1]), self.block(st[2]), self.block(st[3])])
            elif k == 'at':
                out.append(['at', self.value(st[1], 'C'), self.block(st[2])])
            elif k == 'alt':
                out.append(['alt', [self.block(b) for b in st[1]]])
            elif k == 'choose':
                out.append(['choose', self.actor[st[1]], self.intern(self.menus, st[2]),
                            [[self.intern(self.actions, label), self.block(blk)] for label, blk in st[3]]])
            else:
                out.append(['mark', self.intern(self.marks, st[1])])
        self.blocks[ix] = out
        return ix

    def query(self, q):
        k = q[0]
        if k in ('exists', 'forall'):
            return {'kind': k, 'code': self.value(q[1], 'R')}
        if k == 'can':
            return {'kind': 'can', 'who': [self.actor[a] for a in q[1]], 'code': self.value(q[2], 'R')}
        if k == 'num':
            return {'kind': 'num', 'policy': None if q[1] is None else [self.actor[a] for a in q[1]],
                    'weight': None if q[2] is None else self.weight[q[2]], 'opt': q[3],
                    'code': self.value(q[4], 'R')}
        if k == 'region':
            return {'kind': 'region', 'params': [self.param[p] for p in q[1]], 'inner': self.query(q[2])}
        who, point, a, b, value, weight = q[1:]
        return {'kind': 'prefer', 'who': self.actor[who], 'menu': self.menus.index(point[1]),
                'a': self.actions.index(a), 'b': self.actions.index(b), 'code': self.value(('name', value), 'R'),
                'weight': None if weight is None else self.weight[weight]}

    def unknown_entry(self, name, ty, lo, hi):
        kind = ('bool' if ty == BOOL else 'time' if ty == TIME else 'actor' if ty == ACTOR
                else 'enum' if ty[0] == 'Enum' else 'num')
        entry = {'name': name, 'kind': kind, 'lo': lo, 'hi': hi}
        if kind == 'enum':
            entry['enum'] = self.enum[ty[1]]
        return entry

    def solve_entry(self, name, sv):
        _, form, objective, label, where, goal = sv
        mode, before = ('V', 'C') if label is None else ('X', 'S')
        unit = self.s.sinfo[name]
        return {'name': name, 'form': form, 'point': None if label is None else self.marks.index(label),
                'where': None if where is None else self.value(where, before),
                'objective': None if objective is None else self.value(objective, mode),
                'goal': self.value(goal, mode), 'unit': unit_text(unit) if unit else ''}

    def parameter(self, name, ty, values):
        if ty == BOOL:
            return {'name': name, 'kind': 'bool', 'values': values, 'show': ['true' if v else 'false' for v in values]}
        if ty == TIME:
            return {'name': name, 'kind': 'time', 'values': values, 'show': [f'@{v}' for v in values]}
        if ty == ACTOR:
            return {'name': name, 'kind': 'actor', 'values': [self.actor[v] for v in values], 'show': list(values)}
        if ty[0] == 'Enum':
            return {'name': name, 'kind': 'enum', 'values': [self.enumval[v] for v in values], 'show': list(values)}
        return {'name': name, 'kind': 'num', 'values': values, 'show': [str(v) for v in values]}

    # ---- the image ----------------------------------------------------------
    def image(self):
        s = self.s
        zero = ('int', 0)
        cells = [{'name': name, 'dom': self.domain(c.type), 'opt': is_opt(c.type),
                  'lo': None if c.lo is None else self.value(c.lo, 'C'),
                  'hi': None if c.hi is None else self.value(c.hi, 'C'),
                  'init': [self.value(v, 'C') for v in c.init]} for name, c in s.cells.items()]
        assets = [{'name': x, 'lo': self.value(a['lo'], 'C'), 'hi': self.value(a['hi'], 'C'),
                   'init': [[self.value(v, 'C') for v in a['init'].get(who, [zero])] for who in s.actors]}
                  for x, a in s.assets.items()]
        obs, mems = [], []
        for who in s.actors:
            seen = [self.cell[n] for n, c in s.cells.items() if c.observers == 'all' or who in c.observers]
            seen += [self.holding(j, x) for x, a in s.assets.items() for j in s.actors if j == who or a['public']]
            obs.append(seen)
            mems.append([self.slot[key] for key in sorted(s.slots) if key[0] == who])
        events = [self.lower_event(ev) for ev in s.events.values()]
        entry = self.block(s.program)
        queries = []
        for name, q in s.queries.items():
            info = s.qinfo[name]
            queries.append({'name': name, 'unit': unit_text(info[1]) if info[0] == 'num' and info[1] else '',
                            'q': self.query(q)})
        solves = [self.solve_entry(name, sv) for name, sv in s.solves.items()]
        return {
            'format': FORMAT, 'version': VERSION, 'language': LANGUAGE, 'model': s.name,
            'actors': list(s.actors), 'assets': assets,
            'enums': [{'name': name, 'values': list(values)} for name, values in s.enums.items()],
            'time': list(s.time),
            'params': [self.parameter(name, ty, values) for name, (ty, values) in s.params.items()],
            'unknowns': [self.unknown_entry(name, *u) for name, u in s.unknowns.items()],
            'cells': cells,
            'slots': [{'actor': self.actor[who], 'key': key, 'dom': self.domain(ty)}
                      for (who, key), ty in s.slots.items()],
            'obs': obs, 'mems': mems,
            'assumes': [self.value(e, 'S') for e in s.assumes],
            'invariants': [self.value(e, 'S') for e in s.invariants],
            'events': events, 'units': self.units, 'blocks': self.blocks, 'entry': entry,
            'menus': self.menus, 'marks': self.marks, 'actions': self.actions,
            'weights': [{'name': name, 'code': self.value(e, 'S')} for name, e in s.weights.items()],
            'queries': queries, 'solves': solves, 'codes': self.codes,
        }


def lower(spec):
    """Compile a checked model into an image (not yet verified)."""
    return Lowering(spec).image()
