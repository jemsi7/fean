"""FEAN v0.2 dynamic semantics: evaluation, commits, program steps, histories, views and knowledge.

Meaning: docs/financial-language.md sections 3 and 4. Map units: I-03-b, I-02-b.
Runtime values are plain Python: int for quantities, numbers and times, bool, str for enum
values and actors, None for the empty value. Units are guaranteed by the static checks.
"""
from dataclasses import dataclass
from itertools import product

from .static import base, is_opt
from .syntax import ModelErr, SearchLimit, StaticErr


class State:
    __slots__ = ('t', 'sig', 'mu')

    def __init__(self, t, sig, mu):
        self.t, self.sig, self.mu = t, sig, mu


@dataclass
class Unit:
    """One commit: a single event or a declared batch."""
    name: str
    events: list        # [(Event, [argument values])]
    net: bool


class Node:
    """An observation point: the history prefix that ends with `step`."""
    __slots__ = ('parent', 'step', 'state', 'display', 'children', 'fail', 'end', 'world')

    def __init__(self, parent, step, state, display, fail, end, world):
        self.parent, self.step, self.state, self.display = parent, step, state, display
        self.children, self.fail, self.end, self.world = [], fail, end, world


class Machine:
    """One instance of a model: every parameter and every unknown has a value (both are in `pvals`).

    With `start = (label, state)` the instance begins at that point of the program: the state is its
    one initial world and what runs is the program that remains after the label.
    """

    def __init__(self, spec, pvals=None, budget=200_000, start=None):
        self.spec, self.pvals, self.budget, self.start = spec, dict(pvals or {}), budget, start
        self.exploration = None
        self.count = 0
        self._prepare()

    # ---- instantiation: ranges, domains, initial worlds ---------------------
    def const(self, e):
        return self.ev(e, None)

    def _prepare(self):
        s = self.spec
        self.ranges, self.domains = {}, {}
        keys, choices = [], []
        for name, c in s.cells.items():
            bt = base(c.type)
            if bt[0] == 'Qty':
                lo, hi = self.const(c.lo), self.const(c.hi)
                if lo > hi and self.start is None:
                    raise StaticErr('W-Range', f'cell {name} has an empty range', c.line)
                self.ranges[name] = (lo, hi)
                dom = range(lo, hi + 1)
            elif bt == ('Bool',):
                dom = [False, True]
            elif bt[0] == 'Enum':
                dom = list(s.enums[bt[1]])
            elif bt == ('Time',):
                dom = list(range(s.time[0], s.time[1] + 1))
            else:
                dom = list(s.actors)
            if is_opt(c.type):
                dom = [None, *dom]
            self.domains[name] = dom
            if self.start is not None:                  # at a point the declared initial values play no part
                continue
            vals = [self.const(v) for v in c.init]
            for v in vals:
                if v not in dom:
                    raise StaticErr('W-Range', f'initial value {v} of {name} is outside its range', c.line)
            keys.append(name)
            choices.append(vals)
        self.floor = {}
        for x, a in s.assets.items():
            lo, hi = self.const(a['lo']), self.const(a['hi'])
            if lo > hi and self.start is None:
                raise StaticErr('W-Range', f'asset {x} has an empty range', a['line'])
            self.floor[x] = lo
            for i in s.actors:
                self.ranges[('bal', i, x)] = (lo, hi)
                if self.start is not None:
                    continue
                vals = [self.const(v) for v in a['init'].get(i, [('int', 0)])]
                for v in vals:
                    if not lo <= v <= hi:
                        raise StaticErr('W-Range', f'initial holding {v} of {i} is outside the range of {x}',
                                        a['line'])
                keys.append(('bal', i, x))
                choices.append(vals)
        self.worlds = []
        if self.start is not None and self.start[1] is not None:
            self.worlds.append(self.start[1])
        for combo in product(*choices) if self.start is None else ():
            st = State(s.time[0], dict(zip(keys, combo)), {})
            if all(self.ev(a, st) for a in s.assumes):
                if not all(self.ev(inv, st) for inv in s.invariants):
                    raise ModelErr('D-Init', 'an initial world breaks an invariant')
                self.worlds.append(st)
        if not self.worlds and self.start is None:
            raise ModelErr('D-Init', 'there is no initial world')
        self.obs = {}
        for i in s.actors:
            seen = [n for n, c in s.cells.items() if c.observers == 'all' or i in c.observers]
            seen += [('bal', j, x) for x, a in s.assets.items() for j in s.actors if j == i or a['public']]
            self.obs[i] = seen

    # ---- expressions --------------------------------------------------------
    def ev(self, e, s, env=None, post=None, node=None, leaf=None):
        tag = e[0]
        if tag in ('int', 'qty', 'time', 'bool'):
            return e[1]
        if tag == 'name':
            q = e[1]
            if env is not None and q in env:
                return env[q]
            if q in self.pvals:
                return self.pvals[q]
            if leaf is not None and q in self.spec.values:
                return self.ev(self.spec.values[q], None, None, None, None, leaf)
            if s is not None and q in s.sig:
                return s.sig[q]
            return q                                    # an enum value or an actor
        if tag == 'empty':
            return None
        if tag == 'now':
            return s.t

        def go(x):
            return self.ev(x, s, env, post, node, leaf)

        if tag == 'bin':
            op = e[1]
            if op == 'and':
                return bool(go(e[2])) and bool(go(e[3]))
            if op == 'or':
                return bool(go(e[2])) or bool(go(e[3]))
            a, b = go(e[2]), go(e[3])
            if op == '+':
                return a + b
            if op == '-':
                return a - b
            if op == '*':
                return a * b
            if op == '==':
                return a == b
            if op == '!=':
                return a != b
            if op == '<':
                return a < b
            if op == '<=':
                return a <= b
            if op == '>':
                return a > b
            return a >= b
        if tag == 'not':
            return not go(e[1])
        if tag == 'neg':
            return -go(e[1])
        if tag == 'if':
            return go(e[2]) if go(e[1]) else go(e[3])
        if tag == 'div':
            a, b = go(e[2]), go(e[3])
            if b == 0:
                return None
            if e[1] == 'floor':
                return a // b
            if e[1] == 'ceil':
                return -((-a) // b)
            return a // b if a % b == 0 else None
        if tag == 'minmax':
            a, b = go(e[2]), go(e[3])
            return min(a, b) if e[1] == 'min' else max(a, b)
        if tag == 'known':
            return go(e[1]) is not None
        if tag == 'default':
            v = go(e[1])
            return go(e[2]) if v is None else v
        if tag in ('mem', 'asof', 'recv'):
            slot = s.mu.get((e[1], e[2]))
            return None if slot is None else slot[('mem', 'asof', 'recv').index(tag)]
        if tag == 'bal':
            return s.sig[('bal', go(e[1]), e[2])]
        if tag == 'post':
            return post.sig[self.key(e[1])]
        if tag == 'call':
            params, _, body = self.spec.defs[e[1]]
            return self.ev(body, s, {p: go(a) for (p, _), a in zip(params, e[2])})
        if tag == 'K':
            return self.exploration.knows(node, e[1], e[2])
        if tag == 'at':
            p = self.exploration.point(leaf, e[1])
            if p is None:
                return False if self.spec.notes.get(id(e)) else None
            return self.ev(e[2], p.state, None, None, p)
        if tag == 'reach':
            return self.exploration.point(leaf, e[1]) is not None
        raise AssertionError(f'unchecked expression {tag}')

    @staticmethod
    def key(tgt):
        return ('bal', tgt[1], tgt[2]) if tgt[0] == 'balc' else tgt[1]

    # ---- one commit ---------------------------------------------------------
    def unit(self, u):
        s = self.spec
        if u[0] == 'batch':
            net, members = s.batches[u[1]]
            return Unit(u[1], [(s.events[q], [self.const(a) for a in args]) for q, args in members], net)
        ev = s.events[u[1]]
        args = [self.const(a) for a in u[2]]
        shown = u[1] + ('(' + ', '.join(str(a) for a in args) + ')' if args else '')
        return Unit(shown, [(ev, args)], ev.net)

    def step(self, unit, s):
        """Next_U(s): ('OK', [states]) | ('BLOCKED', reason) | ('NO_SETTLEMENT', reason)."""
        envs = []
        for ev, args in unit.events:                    # 1. bindings and conditions, in written order
            env = {p: a for (p, _), a in zip(ev.params, args)}
            for c in ev.clauses:
                k = c[0]
                if k == 'let':
                    env[c[1]] = self.ev(c[2], s, env)
                elif k == 'given':
                    v = self.ev(c[2], s, env)
                    if v is None:
                        return 'BLOCKED', f'given {c[1]} in {ev.name}'
                    env[c[1]] = v
                elif k in ('require', 'auth') and not self.ev(c[1], s, env):
                    return 'BLOCKED', f'{k} in {ev.name}'
            envs.append(env)
        sets, deltas, frees, tells, forgets = {}, {}, [], {}, []
        inflow, outflow = {}, {}
        for (ev, _), env in zip(unit.events, envs):     # 2. flows, read from the pre-state
            for c in ev.clauses:
                k = c[0]
                if k == 'set':
                    sets[self.key(c[1])] = self.ev(c[2], s, env)
                elif k == 'delta':
                    key, d = self.key(c[1]), c[2] * self.ev(c[3], s, env)
                    deltas[key] = deltas.get(key, 0) + d
                    if c[1][0] == 'balc':
                        side = inflow if d > 0 else outflow
                        side[key] = side.get(key, 0) + abs(d)
                elif k == 'free':
                    frees.append(self.key(c[1]))
                elif k == 'move':
                    a, b, n = self.ev(c[1], s, env), self.ev(c[2], s, env), self.ev(c[3], s, env)
                    if n < 0:
                        raise ModelErr('D-Move', f'{ev.name} moves a negative amount {n}')
                    x = self.spec.notes[id(c)]
                    src, dst = ('bal', a, x), ('bal', b, x)
                    deltas[src] = deltas.get(src, 0) - n
                    deltas[dst] = deltas.get(dst, 0) + n
                    outflow[src] = outflow.get(src, 0) + n
                    inflow[dst] = inflow.get(dst, 0) + n
                elif k == 'tell':
                    tells[(c[1], c[2])] = (self.ev(c[3], s, env), self.ev(c[4], s, env), s.t)
                elif k == 'forget':
                    forgets.append((c[1], c[2]))
        for key, out in outflow.items():                # 3. funds: gross unless the unit is net
            have = s.sig[key] + (inflow.get(key, 0) if unit.net else 0)
            if have - out < self.floor[key[2]]:
                return 'BLOCKED', f'funds of {key[1]} in {key[2]}'
        written = list(sets) + list(deltas) + frees
        cands = []
        for choice in product(*[self.domains[c] for c in frees]):   # 4. candidates
            sig = dict(s.sig)
            sig.update(sets)
            for key, d in deltas.items():
                sig[key] += d
            sig.update(zip(frees, choice))
            mu = dict(s.mu)
            mu.update(tells)
            for key in forgets:
                mu.pop(key, None)
            s2 = State(s.t, sig, mu)
            if not all(self.in_range(key, sig[key]) for key in written):   # 5. range, invariant, ensure
                continue
            if not all(self.ev(inv, s2) for inv in self.spec.invariants):
                continue
            if all(self.ev(c[1], s, env, s2) for (ev, _), env in zip(unit.events, envs)
                   for c in ev.clauses if c[0] == 'ensure'):
                cands.append(s2)
        return ('OK', cands) if cands else ('NO_SETTLEMENT', f'no post-state for {unit.name}')

    def in_range(self, key, v):
        r = self.ranges.get(key)
        return r is None or v is None or r[0] <= v <= r[1]

    # ---- program steps ------------------------------------------------------
    def node(self, parent, step, state, display=None, fail=None, end=False):
        self.count += 1
        if self.count > self.budget:
            raise SearchLimit(f'more than {self.budget} observation points')
        n = Node(parent, step, state, display, fail, end, step[1] if parent is None else parent.world)
        if parent is not None:
            parent.children.append(n)
        return n

    def explore(self):
        """Run the program from every initial world; return the root of each history tree."""
        self.count = 0
        roots = []
        if self.start is not None:
            st = self.worlds[0]
            root = self.node(None, ('init', 0), st)
            self.resume(self.start[0])(st, root)
            return [root]
        for w, st in enumerate(self.worlds):
            root = self.node(None, ('init', w), st)
            self.run_block(self.spec.program, 0, st, root, lambda s, n: self.node(n, ('end',), s, end=True))
            roots.append(root)
        return roots

    def resume(self, label):
        """What remains of the program after `label`: the rest of its block, then of the blocks around it."""
        def find(block, k):
            for i, st in enumerate(block):
                def rest(s, n, i=i):
                    return self.run_block(block, i + 1, s, n, k)

                tag = st[0]
                if tag == 'label' and st[1] == label:
                    return rest
                inner = ([st[2]] if tag == 'at' else [st[2], st[3]] if tag == 'attempt' else st[1] if tag == 'alt'
                         else [blk for _, blk in st[3]] if tag == 'choose' else [])
                for blk in inner:
                    found = find(blk, rest)
                    if found is not None:
                        return found
            return None

        return find(self.spec.program, lambda s, n: self.node(n, ('end',), s, end=True))

    def run_block(self, block, i, s, node, k):
        if i == len(block):
            return k(s, node)
        st = block[i]
        tag = st[0]

        def rest(s2, n2):
            return self.run_block(block, i + 1, s2, n2, k)

        if tag == 'at':
            t = self.const(st[1])
            if t < s.t or not self.spec.time[0] <= t <= self.spec.time[1]:
                raise StaticErr('W-Clock', f'at @{t} is outside the clock at @{s.t}')
            s1 = State(t, s.sig, s.mu)
            return self.run_block(st[2], 0, s1, self.node(node, ('tick', t), s1), rest)
        if tag in ('run', 'attempt'):
            unit = self.unit(st[1])
            status, info = self.step(unit, s)
            if status == 'OK':
                for s2 in info:
                    n = self.node(node, ('commit', unit.name), s2)
                    if tag == 'attempt':
                        self.run_block(st[2], 0, s2, n, rest)
                    else:
                        rest(s2, n)
            elif status == 'BLOCKED' and tag == 'attempt':
                self.run_block(st[3], 0, s, self.node(node, ('reject', unit.name, info), s), rest)
            else:
                self.node(node, ('fail', unit.name, status, info), s, fail=status)
            return None
        if tag == 'alt':
            for j, blk in enumerate(st[1]):
                self.run_block(blk, 0, s, self.node(node, ('branch', j), s), rest)
            return None
        if tag == 'choose':
            who, menu, actions = st[1], st[2], st[3]
            shown = self.node(node, ('show', who, menu), s, display=(who, menu, tuple(a for a, _ in actions)))
            for label, blk in actions:
                self.run_block(blk, 0, s, self.node(shown, ('pick', who, menu, label), s), rest)
            return None
        return rest(s, self.node(node, ('mark', st[1]), s))        # label


class Exploration:
    """H_M: every observation point of every history, with views and knowledge."""

    def __init__(self, machine):
        self.m = machine
        machine.exploration = self
        self.roots = machine.explore()
        self.nodes, self.leaves = [], []
        stack = list(reversed(self.roots))
        while stack:
            n = stack.pop()
            self.nodes.append(n)
            if n.end or n.fail:
                self.leaves.append(n)
            stack.extend(reversed(n.children))
        self._views, self._classes, self._points, self._paths = {}, {}, {}, {}

    def view(self, node, i):
        """view_i(p) = (public time, menu shown to i, i's slots, cells i observes)."""
        key = (id(node), i)
        v = self._views.get(key)
        if v is None:
            s, d = node.state, node.display
            shown = (d[1], d[2]) if d is not None and d[0] == i else None
            memory = tuple(sorted((k, val) for (a, k), val in s.mu.items() if a == i))
            v = self._views[key] = (s.t, shown, memory, tuple(s.sig[c] for c in self.m.obs[i]))
        return v

    def classes(self, i):
        if i not in self._classes:
            groups = {}
            for n in self.nodes:
                groups.setdefault(self.view(n, i), []).append(n)
            self._classes[i] = groups
        return self._classes[i]

    def knows(self, node, i, phi):
        """M, p |= K(i, phi): phi holds at every point i cannot tell from p."""
        return all(self.m.ev(phi, q.state, None, None, q) for q in self.classes(i)[self.view(node, i)])

    def path(self, leaf):
        p = self._paths.get(id(leaf))
        if p is None:
            p, n = [], leaf
            while n is not None:
                p.append(n)
                n = n.parent
            p.reverse()
            self._paths[id(leaf)] = p
        return p

    def point(self, leaf, psel):
        table = self._points.get(id(leaf))
        if table is None:
            table = {}
            for n in self.path(leaf):
                kind = n.step[0]
                if kind == 'init':
                    table[('start',)] = n
                    if self.m.start is not None:        # an instance begun at a label: its start is that point
                        table[('label', self.m.start[0])] = n
                elif kind == 'mark':
                    table[('label', n.step[1])] = n
                elif kind == 'show':
                    table[('menu', n.step[2])] = n
                elif kind == 'end':
                    table[('end',)] = n
            self._points[id(leaf)] = table
        return table.get(psel)

    def under(self, node):
        out, stack = [], [node]
        while stack:
            n = stack.pop()
            if n.end or n.fail:
                out.append(n)
            stack.extend(n.children)
        return out
