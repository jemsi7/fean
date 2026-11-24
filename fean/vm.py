"""FEAN machine: runs an image.

Meaning: docs/financial-language.md sections 3-5. How the machine realises it: docs/engine.md
section 6. Map units: I-07-a, I-15-a. The machine reads no names and checks no types; all of that
was settled when the image was compiled. A solve is answered here by trying every candidate, which
is its definition; the solver that finds the boundaries instead is the Rust engine's.
"""
import copy
from fractions import Fraction
from itertools import product

from .ir import OPS, verify
from .syntax import ModelErr, SearchLimit, StaticErr

UNKNOWN = 'UNKNOWN'
NONE = 'none'
INVALID = 'invalid'
ENUM_LIMIT = 200_000                # more candidates than this are not tried: the answer is UNKNOWN
SLOT_RANGE = (-10 ** 9, 10 ** 9)    # where the value of a quantity slot is looked for
_N = {name: i for i, name in enumerate(OPS)}
(PUSH, PARAM, LOCAL, STORE, DUP, POP, LOAD, BAL, NOW, MEM, ASOF, RECV, POST, ADD, SUB, MUL, MIN, MAX, DIVF, DIVC,
 DIVX, EQ, NE, LT, LE, GT, GE, NEG, NOT, KNOWN, JMP, JF, JT, K, AT, REACH, REQUIRE, GIVEN, FXSET, FXADD, FXFREE,
 FXMOVE, FXTELL, FXFORGET, UNK, QUERY) = (_N[k] for k in (
    'PUSH', 'PARAM', 'LOCAL', 'STORE', 'DUP', 'POP', 'LOAD', 'BAL', 'NOW', 'MEM', 'ASOF', 'RECV', 'POST', 'ADD',
    'SUB', 'MUL', 'MIN', 'MAX', 'DIVF', 'DIVC', 'DIVX', 'EQ', 'NE', 'LT', 'LE', 'GT', 'GE', 'NEG', 'NOT', 'KNOWN',
    'JMP', 'JF', 'JT', 'K', 'AT', 'REACH', 'REQUIRE', 'GIVEN', 'FXSET', 'FXADD', 'FXFREE', 'FXMOVE', 'FXTELL',
    'FXFORGET', 'UNK', 'QUERY'))


class State:
    __slots__ = ('t', 'cells', 'mem')

    def __init__(self, t, cells, mem):
        self.t, self.cells, self.mem = t, cells, mem


class Node:
    """An observation point: the history prefix that ends with `step`."""
    __slots__ = ('parent', 'step', 'state', 'display', 'children', 'fail', 'end', 'world')

    def __init__(self, parent, step, state, display, fail, end, world):
        self.parent, self.step, self.state, self.display = parent, step, state, display
        self.children, self.fail, self.end, self.world = [], fail, end, world


class _Blocked(Exception):
    def __init__(self, reason):
        self.reason = reason


class _Effects:
    """What one commit wants to change, gathered before anything is applied."""
    __slots__ = ('sets', 'deltas', 'frees', 'tells', 'forgets', 'inflow', 'outflow', 'event')

    def __init__(self):
        self.sets, self.deltas, self.frees, self.tells, self.forgets = {}, {}, [], {}, []
        self.inflow, self.outflow, self.event = {}, {}, ''


class Vm:
    """One instance of an image: every public parameter and every unknown has a value.

    With `start = (mark number, state)` the instance begins at that point of the program: the state is
    its one initial world and what runs is the program that remains after the mark.
    """

    def __init__(self, image, pvals=(), budget=200_000, uvals=(), start=None):
        im = self.im = image
        self.pv, self.budget, self.start = list(pvals), budget, start
        self.uv = [bool(v) if u['kind'] == 'bool' else v for u, v in zip(im['unknowns'], uvals)]
        self.nA, self.H0 = len(im['actors']), len(im['cells'])
        self.ins = [[(_N[i[0]],) + tuple(i[1:]) + (None,) * (5 - len(i)) for i in c['ins']] for c in im['codes']]
        self.nloc = [c['locals'] for c in im['codes']]
        self.roots = None
        self.count = 0
        self._forget()
        self._instantiate()

    def _forget(self):
        self.roots, self.count, self._verdicts = None, 0, {}
        self._views, self._classes, self._points, self._paths, self._choices, self._probs = {}, {}, {}, {}, {}, {}

    def at(self, state):
        """The same instance begun at another state of its starting point; the code and the ranges are shared."""
        vm = copy.copy(self)
        vm.start, vm.worlds = (self.start[0], state), [state]
        vm._forget()
        return vm

    # ---- instantiation: ranges, domains, initial worlds ---------------------
    def _instantiate(self):
        im, nA = self.im, self.nA
        t0, t1 = im['time']
        begun = self.start is not None                  # at a point the declared initial values play no part
        self.ranges, self.domains, choices = {}, [], []
        for i, c in enumerate(im['cells']):
            kind = c['dom']['kind']
            if kind == 'qty':
                lo, hi = self.run(c['lo'], None), self.run(c['hi'], None)
                if lo > hi and not begun:
                    raise StaticErr('W-Range', f'cell {c["name"]} has an empty range')
                self.ranges[i] = (lo, hi)
                dom = range(lo, hi + 1)
            elif kind == 'bool':
                dom = [False, True]
            elif kind == 'enum':
                dom = list(range(len(im['enums'][c['dom']['enum']]['values'])))
            elif kind == 'time':
                dom = list(range(t0, t1 + 1))
            else:
                dom = list(range(nA))
            if c['opt']:
                dom = [None, *dom]
            self.domains.append(dom)
            if begun:
                continue
            vals = [self.run(v, None) for v in c['init']]
            for v in vals:
                if v not in dom:
                    raise StaticErr('W-Range', f'initial value {v} of {c["name"]} is outside its range')
            choices.append(vals)
        self.floor = []
        for x, a in enumerate(im['assets']):
            lo, hi = self.run(a['lo'], None), self.run(a['hi'], None)
            if lo > hi and not begun:
                raise StaticErr('W-Range', f'asset {a["name"]} has an empty range')
            self.floor.append(lo)
            for i in range(nA):
                self.ranges[self.H0 + x * nA + i] = (lo, hi)
                if begun:
                    continue
                vals = [self.run(v, None) for v in a['init'][i]]
                for v in vals:
                    if not lo <= v <= hi:
                        raise StaticErr('W-Range', f'initial holding {v} of {im["actors"][i]} is outside the '
                                                   f'range of {a["name"]}')
                choices.append(vals)
        self.worlds = []
        if begun:
            if self.start[1] is not None:
                self.worlds.append(self.start[1])
            return
        empty = [None] * len(im['slots'])
        for combo in product(*choices):
            st = State(t0, list(combo), empty)
            if all(self.run(a, st) for a in im['assumes']):
                if not all(self.run(inv, st) for inv in im['invariants']):
                    raise ModelErr('D-Init', 'an initial world breaks an invariant')
                self.worlds.append(st)
        if not self.worlds:
            raise ModelErr('D-Init', 'there is no initial world')

    # ---- code ---------------------------------------------------------------
    def run(self, cid, s, frame=None, post=None, node=None, leaf=None, fx=None):
        """Execute code block `cid`; return the value it leaves (None for guard and effect blocks)."""
        ins = self.ins[cid]
        if frame is None and self.nloc[cid]:
            frame = [None] * self.nloc[cid]
        stack = []
        push, pop = stack.append, stack.pop
        pc, end = 0, len(ins)
        while pc < end:
            op, a, b, c, d = ins[pc]
            pc += 1
            if op == PUSH:
                push(a)
            elif op == LOAD:
                push(s.cells[a])
            elif op == LOCAL:
                push(frame[a])
            elif op == STORE:
                frame[a] = pop()
            elif op == PARAM:
                push(self.pv[a])
            elif op == UNK:
                push(self.uv[a])
            elif op == QUERY:
                push(self.verdict(a))
            elif op == JF:
                if not pop():
                    pc = a
            elif op == JT:
                if pop():
                    pc = a
            elif op == JMP:
                pc = a
            elif op == EQ:
                y = pop()
                push(pop() == y)
            elif op == NE:
                y = pop()
                push(pop() != y)
            elif op == LT:
                y = pop()
                push(pop() < y)
            elif op == LE:
                y = pop()
                push(pop() <= y)
            elif op == GT:
                y = pop()
                push(pop() > y)
            elif op == GE:
                y = pop()
                push(pop() >= y)
            elif op == ADD:
                y = pop()
                push(pop() + y)
            elif op == SUB:
                y = pop()
                push(pop() - y)
            elif op == MUL:
                y = pop()
                push(pop() * y)
            elif op == MIN:
                y = pop()
                push(min(pop(), y))
            elif op == MAX:
                y = pop()
                push(max(pop(), y))
            elif op == DIVF:
                y = pop()
                x = pop()
                push(None if y == 0 else x // y)
            elif op == DIVC:
                y = pop()
                x = pop()
                push(None if y == 0 else -((-x) // y))
            elif op == DIVX:
                y = pop()
                x = pop()
                push(x // y if y != 0 and x % y == 0 else None)
            elif op == NOT:
                push(not pop())
            elif op == NEG:
                push(-pop())
            elif op == KNOWN:
                push(pop() is not None)
            elif op == DUP:
                push(stack[-1])
            elif op == POP:
                pop()
            elif op == BAL:
                push(s.cells[self.H0 + a * self.nA + pop()])
            elif op == NOW:
                push(s.t)
            elif op == MEM or op == ASOF or op == RECV:
                slot = s.mem[a]
                push(None if slot is None else slot[op - MEM])
            elif op == POST:
                push(post.cells[a])
            elif op == K:
                push(self.knows(node, a, b))
            elif op == AT:
                p = self.point(leaf, a, b)
                push((False if d else None) if p is None else self.run(c, p.state, None, None, p))
            elif op == REACH:
                push(self.point(leaf, a, b) is not None)
            elif op == REQUIRE:
                if not pop():
                    raise _Blocked(a)
            elif op == GIVEN:
                v = pop()
                if v is None:
                    raise _Blocked(b)
                frame[a] = v
            elif op == FXSET:
                fx.sets[a] = pop()
            elif op == FXADD:
                dlt = pop()
                fx.deltas[a] = fx.deltas.get(a, 0) + dlt
                if a >= self.H0:                       # a holding: creation or destruction
                    side = fx.inflow if dlt > 0 else fx.outflow
                    side[a] = side.get(a, 0) + abs(dlt)
            elif op == FXFREE:
                fx.frees.append(a)
            elif op == FXMOVE:
                n = pop()
                dst = self.H0 + a * self.nA + pop()
                src = self.H0 + a * self.nA + pop()
                if n < 0:
                    raise ModelErr('D-Move', f'{fx.event} moves a negative amount {n}')
                fx.deltas[src] = fx.deltas.get(src, 0) - n
                fx.deltas[dst] = fx.deltas.get(dst, 0) + n
                fx.outflow[src] = fx.outflow.get(src, 0) + n
                fx.inflow[dst] = fx.inflow.get(dst, 0) + n
            elif op == FXTELL:
                asof = pop()
                fx.tells[a] = (pop(), asof, s.t)
            else:                                       # FXFORGET
                fx.forgets.append(a)
        return stack[-1] if stack else None

    # ---- one commit ---------------------------------------------------------
    def step(self, unit, s):
        """Next_U(s) for a unit of the image: ('OK', [states]) | ('BLOCKED', why) | ('NO_SETTLEMENT', why)."""
        u = self.im['units'][unit]
        members = [(m['event'], [self.run(arg, None) for arg in m['args']]) for m in u['members']]
        return self.commit(members, u['net'], s, u['name'])

    def commit(self, members, net, s, name=''):
        """The same judgement for explicit members [(event number, [argument values])]."""
        im, events = self.im, self.im['events']
        frames = []
        try:
            for ei, args in members:                    # 1. bindings and conditions, in written order
                ev = events[ei]
                frame = list(args) + [None] * (ev['locals'] - len(args))
                self.run(ev['guard'], s, frame)
                frames.append((ev, frame))
        except _Blocked as blocked:
            return 'BLOCKED', blocked.reason
        fx = _Effects()
        for ev, frame in frames:                        # 2. flows, read from the pre-state
            fx.event = ev['name']
            self.run(ev['effects'], s, frame, fx=fx)
        for key, out in fx.outflow.items():             # 3. funds: gross unless the unit is net
            have = s.cells[key] + (fx.inflow.get(key, 0) if net else 0)
            x, who = divmod(key - self.H0, self.nA)
            if have - out < self.floor[x]:
                return 'BLOCKED', f'funds of {im["actors"][who]} in {im["assets"][x]["name"]}'
        written = list(fx.sets) + list(fx.deltas) + fx.frees
        cands = []
        for choice in product(*[self.domains[c] for c in fx.frees]):    # 4. candidates
            cells = list(s.cells)
            for c, v in fx.sets.items():
                cells[c] = v
            for c, dlt in fx.deltas.items():
                cells[c] += dlt
            for c, v in zip(fx.frees, choice):
                cells[c] = v
            mem = s.mem
            if fx.tells or fx.forgets:
                mem = list(mem)
                for k, v in fx.tells.items():
                    mem[k] = v
                for k in fx.forgets:
                    mem[k] = None
            s2 = State(s.t, cells, mem)
            if not all(self._in_range(c, cells[c]) for c in written):   # 5. range, invariant, ensure
                continue
            if not all(self.run(inv, s2) for inv in im['invariants']):
                continue
            if all(self.run(p, s, frame, s2) for ev, frame in frames for p in ev['ensure']):
                cands.append(s2)
        return ('OK', cands) if cands else ('NO_SETTLEMENT', f'no post-state for {name}')

    def _in_range(self, c, v):
        r = self.ranges.get(c)
        return r is None or v is None or r[0] <= v <= r[1]

    # ---- program steps ------------------------------------------------------
    def _node(self, parent, step, state, display=None, fail=None, end=False):
        self.count += 1
        if self.count > self.budget:
            raise SearchLimit(f'more than {self.budget} observation points')
        n = Node(parent, step, state, display, fail, end, step[1] if parent is None else parent.world)
        if parent is not None:
            parent.children.append(n)
        return n

    def explore(self):
        """Run the program from every initial world; H_M is self.nodes, complete histories are self.leaves."""
        if self.roots is not None:
            return self
        self.count, roots = 0, []
        for w, st in enumerate(self.worlds):
            root = self._node(None, ('init', w), st)
            b, i, k = (self.im['entry'], 0, None) if self.start is None else self._resume(self.start[0])
            self._block(b, i, st, root, k)
            roots.append(root)
        self.roots, self.nodes, self.leaves = roots, [], []
        stack = list(reversed(roots))
        while stack:
            n = stack.pop()
            self.nodes.append(n)
            if n.end or n.fail:
                self.leaves.append(n)
            stack.extend(reversed(n.children))
        return self

    def _resume(self, mark):
        """(block, index, k) just after `mark`: the rest of its block, then of the blocks around it."""
        blocks = self.im['blocks']

        def find(b, k):
            for i, st in enumerate(blocks[b]):
                kind, rest = st[0], (b, i + 1, k)
                if kind == 'mark' and st[1] == mark:
                    return rest
                inner = ([st[2]] if kind == 'at' else [st[2], st[3]] if kind == 'attempt' else st[1] if kind == 'alt'
                         else [blk for _, blk in st[3]] if kind == 'choose' else [])
                for blk in inner:
                    found = find(blk, rest)
                    if found is not None:
                        return found
            return None

        return find(self.im['entry'], None)

    def _block(self, b, i, s, node, k):
        """Run statement i of block b; k = (block, index, k') is what remains after this block."""
        stmts = self.im['blocks'][b]
        if i == len(stmts):
            if k is None:
                self._node(node, ('end',), s, end=True)
            else:
                self._block(k[0], k[1], s, node, k[2])
            return
        st = stmts[i]
        kind, rest = st[0], (b, i + 1, k)
        if kind == 'at':
            t = self.run(st[1], None)
            if t < s.t or not self.im['time'][0] <= t <= self.im['time'][1]:
                raise StaticErr('W-Clock', f'at @{t} is outside the clock at @{s.t}')
            s1 = State(t, s.cells, s.mem)
            self._block(st[2], 0, s1, self._node(node, ('tick', t), s1), rest)
        elif kind == 'run' or kind == 'attempt':
            name = self.im['units'][st[1]]['name']
            status, info = self.step(st[1], s)
            if status == 'OK':
                for s2 in info:
                    n = self._node(node, ('commit', name), s2)
                    if kind == 'attempt':
                        self._block(st[2], 0, s2, n, rest)
                    else:
                        self._block(b, i + 1, s2, n, k)
            elif status == 'BLOCKED' and kind == 'attempt':
                self._block(st[3], 0, s, self._node(node, ('reject', name, info), s), rest)
            else:
                self._node(node, ('fail', name, status, info), s, fail=status)
        elif kind == 'alt':
            for j, blk in enumerate(st[1]):
                self._block(blk, 0, s, self._node(node, ('branch', j), s), rest)
        elif kind == 'choose':
            who, menu, arms = st[1], st[2], st[3]
            shown = self._node(node, ('show', who, menu), s, display=(who, menu, tuple(a for a, _ in arms)))
            for action, blk in arms:
                self._block(blk, 0, s, self._node(shown, ('pick', who, menu, action), s), rest)
        else:                                           # mark
            self._block(b, i + 1, s, self._node(node, ('mark', st[1]), s), k)

    # ---- views and knowledge ------------------------------------------------
    def view(self, node, who):
        """view_i(p) = (public time, menu shown to i, i's slots, cells i observes)."""
        key = (id(node), who)
        v = self._views.get(key)
        if v is None:
            s, d = node.state, node.display
            shown = (d[1], d[2]) if d is not None and d[0] == who else None
            v = self._views[key] = (s.t, shown, tuple(s.mem[k] for k in self.im['mems'][who]),
                                    tuple(s.cells[c] for c in self.im['obs'][who]))
        return v

    def classes(self, who):
        if who not in self._classes:
            groups = {}
            for n in self.explore().nodes:
                groups.setdefault(self.view(n, who), []).append(n)
            self._classes[who] = groups
        return self._classes[who]

    def knows(self, node, who, code):
        """M, p |= K(i, phi): phi (a point-layer code block) holds wherever i cannot tell apart from p."""
        return all(self.run(code, q.state, None, None, q) for q in self.classes(who)[self.view(node, who)])

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

    def point(self, leaf, kind, ident):
        """The point of a history: kind 0 start, 1 end, 2 menu, 3 label."""
        table = self._points.get(id(leaf))
        if table is None:
            table = {}
            for n in self.path(leaf):
                step = n.step[0]
                if step == 'init':
                    table[(0, 0)] = n
                    if self.start is not None:          # an instance begun at a mark: its start is that point
                        table[(3, self.start[0])] = n
                elif step == 'end':
                    table[(1, 0)] = n
                elif step == 'show':
                    table[(2, n.step[2])] = n
                elif step == 'mark':
                    table[(3, n.step[1])] = n
            self._points[id(leaf)] = table
        return table.get((kind, ident))

    def under(self, node):
        out, stack = [], [node]
        while stack:
            n = stack.pop()
            if n.end or n.fail:
                out.append(n)
            stack.extend(n.children)
        return out

    # ---- queries ------------------------------------------------------------
    def rv(self, code, leaf):
        return self.run(code, None, None, None, None, leaf)

    def choices(self, leaf):
        c = self._choices.get(id(leaf))
        if c is None:
            c = [(n.step[1], self.view(n.parent, n.step[1]), n.step[3]) for n in self.path(leaf)
                 if n.step[0] == 'pick']
            self._choices[id(leaf)] = c
        return c

    def policies(self, coalition):
        """Every profile of view-based deterministic strategies for the coalition."""
        menus = {}
        for n in self.explore().nodes:
            if n.step[0] == 'show' and n.step[1] in coalition:
                menus[(n.step[1], self.view(n, n.step[1]))] = n.display[2]
        order = list(menus)
        for acts in product(*[menus[key] for key in order]):
            yield dict(zip(order, acts))

    def follows(self, leaf, policy, coalition):
        return all(policy[(who, v)] == a for who, v, a in self.choices(leaf) if who in coalition)

    def probs(self, weight):
        if weight not in self._probs:
            w = self.im['weights'][weight]
            raw = [self.run(w['code'], st) for st in self.worlds]
            if any(x < 0 for x in raw) or sum(raw) == 0:
                raise ModelErr('D-Weight', f'weight {w["name"]} is negative or sums to zero')
            self._probs[weight] = [Fraction(x, sum(raw)) for x in raw]
        return self._probs[weight]

    def verdict(self, i):
        """The verdict of query i in this instance: what its name stands for in a goal."""
        if i not in self._verdicts:
            self._verdicts[i] = self.query(self.im['queries'][i]['q'])
        return self._verdicts[i]

    def query(self, q):
        leaves, kind = self.explore().leaves, q['kind']
        if kind == 'exists':
            return any(self.rv(q['code'], leaf) for leaf in leaves)
        if kind == 'forall':
            return all(self.rv(q['code'], leaf) for leaf in leaves)
        if kind == 'can':
            who = set(q['who'])
            return any(all(self.rv(q['code'], leaf) for leaf in leaves if self.follows(leaf, pol, who))
                       for pol in self.policies(who))
        if kind == 'num':
            return self._number(q)
        return self._prefer(q)

    def _number(self, q):
        pick, code, weight = (max if q['opt'] == 'max' else min), q['code'], q['weight']

        def value(leaves):
            if weight is None:
                return pick(self.rv(code, leaf) for leaf in leaves)
            total = Fraction(0)
            for w, p in enumerate(self.probs(weight)):
                if p:
                    total += p * pick(self.rv(code, leaf) for leaf in leaves if leaf.world == w)
            return total

        if q['policy'] is None:
            return value(self.leaves)
        who = set(q['policy'])
        return max(value([leaf for leaf in self.leaves if self.follows(leaf, pol, who)])
                   for pol in self.policies(who))

    def _prefer(self, q):
        """True when, in every information set of `who` at the menu, a scores strictly above b."""
        who, menu, code = q['who'], q['menu'], q['code']
        groups = {}
        for n in self.nodes:
            if n.step[0] == 'show' and n.step[2] == menu:
                groups.setdefault(self.view(n, who), []).append(n)
        probs = self.probs(q['weight']) if q['weight'] is not None else None

        def score(nodes, action):
            worst = {}
            for n in nodes:
                for child in n.children:
                    if child.step[3] == action:
                        for leaf in self.under(child):
                            v = self.rv(code, leaf)
                            worst[leaf.world] = v if leaf.world not in worst else min(worst[leaf.world], v)
            if not worst:
                return None
            if probs is None:
                return min(worst.values())
            mass = sum(probs[w] for w in worst)
            if mass == 0:
                raise ModelErr('D-Weight', 'an information set has probability zero')
            return sum(probs[w] * v for w, v in worst.items()) / mass

        if not groups:
            return False
        for nodes in groups.values():
            sa, sb = score(nodes, q['a']), score(nodes, q['b'])
            if sa is None or sb is None or not sa > sb:
                return False
        return True


def spans(xs):
    """[0, 1, 2, 5] -> ['0..2', '5']."""
    out, i = [], 0
    while i < len(xs):
        j = i
        while j + 1 < len(xs) and xs[j + 1] == xs[j] + 1:
            j += 1
        out.append(str(xs[i]) if i == j else f'{xs[i]}..{xs[j]}')
        i = j + 1
    return out


class Var:
    """One coordinate of a candidate: an unknown, or one part of the state at a point.

    Values are whole numbers lo..hi. Enum values, actors and truth values are numbered; when `opt`,
    the number lo stands for the empty value, so that empty comes first in the order.
    """
    __slots__ = ('name', 'kind', 'lo', 'hi', 'opt', 'names')

    def __init__(self, name, kind, lo, hi, opt=False, names=None):
        self.name, self.kind, self.lo, self.hi, self.opt, self.names = name, kind, lo, hi, opt, names

    def value(self, n):
        """The machine value the number stands for."""
        if self.opt and n == self.lo:
            return None
        return bool(n) if self.kind == 'bool' else n

    def show(self, n):
        if self.opt and n == self.lo:
            return 'empty'
        if self.kind == 'bool':
            return 'true' if n else 'false'
        if self.names is not None:
            return self.names[n]
        return f'@{n}' if self.kind == 'time' else str(n)


class Solved:
    """The answer to one solve under one parameter assignment."""

    def __init__(self, form, unit):
        self.form, self.unit = form, unit
        self.limit = False      # too many candidates: nothing was tried
        self.verdicts = {}      # candidate -> True | False | INVALID | UNKNOWN, in order; filtered-out ones are absent
        self.vars = {}          # candidate -> [Var] (the ranges of a state can depend on the unknowns)
        self.best = None        # the representative solution: the case plain queries are answered in
        self.value = None       # the optimum of min / max
        self.unknown = False    # the answer is UNKNOWN

    def points(self, verdict=True):
        return [p for p, v in self.verdicts.items() if v is verdict]


class Session:
    """A verified image and its public parameters: one Vm per assignment, results written as text."""

    def __init__(self, image, budget=200_000, limit=ENUM_LIMIT):
        self.im, self.budget, self.limit = verify(image), budget, limit
        self._vms, self._solved = {}, {}

    def assignments(self):
        return [list(combo) for combo in product(*[p['values'] for p in self.im['params']])]

    def vm(self, pvals=()):
        key = tuple(pvals)
        if key not in self._vms:
            self._vms[key] = Vm(self.im, key, self.budget)
        return self._vms[key]

    def evaluate(self, pvals, q):
        return self._evaluate(self.vm, pvals, q)

    @staticmethod
    def _evaluate(vm, pvals, q):
        try:
            return vm(pvals).query(q)
        except SearchLimit:
            return UNKNOWN

    def result(self, name):
        """bool | number | ('region', params, true tuples, undecided tuples) | ('table', params, {tuple: result})
        | ('cases', [(solve name, one of the above | NONE)]) when the image has unknowns."""
        q = next(entry['q'] for entry in self.im['queries'] if entry['name'] == name)
        if self.im['unknowns']:
            return ('cases', [(sv['name'], self._in_case(q, sv['name'])) for sv in self.im['solves']])
        return self._over(q, self.assignments(), self.vm)

    def _in_case(self, q, solve):
        """A plain query, answered where the solve's representative solution holds."""
        found = [pv for pv in self.assignments() if self.case(solve, pv) is not None]
        if not self.im['params'] and not found:
            return NONE
        return self._over(q, found, lambda pv: self.instance(solve, pv, self.case(solve, pv)))

    def _over(self, q, assignments, vm):
        every = list(range(len(self.im['params'])))
        if q['kind'] == 'region':
            rest = [p for p in every if p not in q['params']]
            table = {} if rest else {(): ([], [])}
            for pv in assignments:
                hits, undecided = table.setdefault(tuple(pv[p] for p in rest), ([], []))
                verdict = self._evaluate(vm, pv, q['inner'])
                if verdict is True:
                    hits.append(tuple(pv[p] for p in q['params']))
                elif verdict is UNKNOWN:
                    undecided.append(tuple(pv[p] for p in q['params']))
            if not rest:
                return ('region', q['params'], *table[()])
            return ('table', rest, {key: ('region', q['params'], *found) for key, found in table.items()})
        if not every:
            return self._evaluate(vm, [], q)
        return ('table', every, {tuple(pv): self._evaluate(vm, pv, q) for pv in assignments})

    def text(self, name):
        unit = next(entry['unit'] for entry in self.im['queries'] if entry['name'] == name)
        return self._text(self.result(name), unit)

    def results(self):
        out = {entry['name']: self.text(entry['name']) for entry in self.im['queries']}
        out.update((sv['name'], self.solve_text(sv['name'])) for sv in self.im['solves'])
        return out

    # ---- solves: the definition, by trying every candidate -------------------
    def _entry(self, name):
        return next(sv for sv in self.im['solves'] if sv['name'] == name)

    def solve(self, name, pvals=()):
        """The Solved of one solve under one parameter assignment."""
        key = (name, tuple(pvals))
        if key not in self._solved:
            self._solved[key] = self._solve(self._entry(name), list(pvals))
        return self._solved[key]

    def case(self, name, pvals=()):
        """The representative solution of a solve (a candidate), or None."""
        return self.solve(name, pvals).best

    def points(self, name, pvals=()):
        """{candidate: True | False | INVALID | UNKNOWN} for every candidate that was tried."""
        return self.solve(name, pvals).verdicts

    def instance(self, name, pvals, point):
        """The Vm of the model where a candidate of the solve holds."""
        mark, n = self._entry(name)['point'], len(self.im['unknowns'])
        if mark is None:
            return Vm(self.im, pvals, self.budget, point[:n])
        frame = Vm(self.im, pvals, self.budget, point[:n], start=(mark, None))
        return frame.at(self._state(self._vars(frame), point))

    def _unknowns(self):
        im = self.im
        return [Var(u['name'], u['kind'], u['lo'], u['hi'],
                    names=im['enums'][u['enum']]['values'] if u['kind'] == 'enum'
                    else im['actors'] if u['kind'] == 'actor' else None) for u in im['unknowns']]

    def _interval(self, dom, declared):
        kind = dom['kind']
        if kind == 'qty':
            return declared
        if kind == 'bool':
            return 0, 1
        if kind == 'enum':
            return 0, len(self.im['enums'][dom['enum']]['values']) - 1
        return tuple(self.im['time']) if kind == 'time' else (0, len(self.im['actors']) - 1)

    def _var(self, name, dom, declared, opt=False):
        lo, hi = self._interval(dom, declared)
        names = (self.im['enums'][dom['enum']]['values'] if dom['kind'] == 'enum'
                 else self.im['actors'] if dom['kind'] == 'actor' else None)
        return Var(name, dom['kind'], lo - opt, hi, opt, names)

    def _vars(self, frame):
        """The coordinates of a candidate at a point: the unknowns, then the state (clock, cells, holdings, slots)."""
        im, clock = self.im, {'kind': 'time'}
        out = self._unknowns() + [self._var('now', clock, None)]
        for i, c in enumerate(im['cells']):
            out.append(self._var(c['name'], c['dom'], frame.ranges.get(i), c['opt']))
        for x, a in enumerate(im['assets']):
            for i, who in enumerate(im['actors']):
                out.append(self._var(f'bal({who}, {a["name"]})', {'kind': 'qty'}, frame.ranges[frame.H0 + x * frame.nA + i]))
        for sl in im['slots']:
            who = im['actors'][sl['actor']]
            out.append(self._var(f'mem({who}, {sl["key"]})', sl['dom'], SLOT_RANGE, True))
            out.append(self._var(f'asof({who}, {sl["key"]})', clock, None))
            out.append(self._var(f'recv({who}, {sl["key"]})', clock, None))
        return out

    def _state(self, vars_, point):
        """The state a candidate stands for; None when it is a second spelling of a state (a slot that is
        absent carries the earliest times)."""
        im = self.im
        vals = [v.value(n) for v, n in zip(vars_, point)][len(im['unknowns']):]
        cut = 1 + len(im['cells']) + len(im['assets']) * len(im['actors'])
        mem, t0 = [], im['time'][0]
        for k in range(cut, len(vals), 3):
            value, asof, recv = vals[k:k + 3]
            if value is None and (asof, recv) != (t0, t0):
                return None
            mem.append(None if value is None else (value, asof, recv))
        return State(vals[0], vals[1:cut], mem)

    @staticmethod
    def _size(vars_, n):
        """How many states the coordinates after the first n spell."""
        size, i = 1, n
        while i < len(vars_):
            v = vars_[i]
            width = max(0, v.hi - v.lo + 1)
            if v.name.startswith('mem('):               # value, asof, recv: absent is one state
                times = vars_[i + 1].hi - vars_[i + 1].lo + 1
                width, i = 1 + max(0, width - 1) * times * times, i + 2
            size, i = size * width, i + 1
        return size

    def _solve(self, sv, pvals):
        im, out = self.im, Solved(sv['form'], sv['unit'])
        mark, where, goal, objective = sv['point'], sv['where'], sv['goal'], sv['objective']
        uvars = self._unknowns()
        total = 1
        for v in uvars:
            total *= v.hi - v.lo + 1
        if total > self.limit:
            out.limit = out.unknown = True
            return out
        todo, total = [], 0                             # [(unknown numbers, [Var], frame)]
        for combo in product(*[range(v.lo, v.hi + 1) for v in uvars]):
            frame = Vm(im, pvals, self.budget, combo, start=(mark, None))   # only evaluates; never explored
            vars_ = uvars if mark is None else self._vars(frame)
            total += self._size(vars_, len(uvars))
            if total > self.limit:
                out.limit = out.unknown = True
                return out
            todo.append((combo, vars_, frame))
        best = None
        for combo, vars_, frame in todo:
            for rest in product(*[range(v.lo, v.hi + 1) for v in vars_[len(uvars):]]):
                point, state = combo + rest, None
                if mark is not None:
                    state = self._state(vars_, point)
                    if state is None or not all(frame.run(inv, state) for inv in im['invariants']):
                        continue
                if where is not None and not frame.run(where, state):
                    continue
                out.vars[point] = vars_
                verdict, score = self._try(pvals, combo, frame, mark, state, goal, objective)
                out.verdicts[point] = verdict
                if verdict is True and objective is not None:
                    if best is None or (score < best if sv['form'] == 'min' else score > best):
                        best, out.best = score, point
        hits, open_ = out.points(True), out.points(UNKNOWN)
        if sv['form'] == 'find':
            out.best = hits[0] if hits else None
        elif sv['form'] == 'any':
            first = hits[0] if hits else None
            out.unknown = any(first is None or p < first for p in open_)
            out.best = None if out.unknown else first
        else:
            out.unknown = bool(open_)
            out.best, out.value = (None, None) if out.unknown else (out.best, best)
        return out

    def judge(self, name, pvals, point):
        """One candidate, tried by itself: True | False | INVALID | UNKNOWN, or None when it is no candidate
        (outside a range, breaking an invariant, or filtered out by `where`)."""
        im, sv = self.im, self._entry(name)
        mark, n = sv['point'], len(im['unknowns'])
        uvars = self._unknowns()
        if len(point) < n or any(not v.lo <= x <= v.hi for v, x in zip(uvars, point)):
            return None
        frame, state = Vm(im, pvals, self.budget, point[:n], start=(mark, None)), None
        if mark is not None:
            vars_ = self._vars(frame)
            if len(point) != len(vars_) or any(not v.lo <= x <= v.hi for v, x in zip(vars_, point)):
                return None
            state = self._state(vars_, point)
            if state is None or not all(frame.run(inv, state) for inv in im['invariants']):
                return None
        if sv['where'] is not None and not frame.run(sv['where'], state):
            return None
        return self._try(pvals, point[:n], frame, mark, state, sv['goal'], None)[0]

    def _try(self, pvals, uvals, frame, mark, state, goal, objective):
        """(verdict, objective value) at one candidate: the model is set up there, run, and the goal read."""
        try:
            vm = Vm(self.im, pvals, self.budget, uvals) if mark is None else frame.at(state)
            vm.explore()
            if not vm.run(goal, state):
                return False, None
            return True, None if objective is None else vm.run(objective, state)
        except SearchLimit:
            return UNKNOWN, None
        except (StaticErr, ModelErr):
            return INVALID, None

    def wide(self, name):
        """A `find` over several coordinates: the solver writes it as regions, this engine as a list of solutions."""
        im, sv = self.im, self._entry(name)
        state = (0 if sv['point'] is None
                 else 1 + len(im['cells']) + len(im['assets']) * len(im['actors']) + 3 * len(im['slots']))
        return sv['form'] == 'find' and len(im['unknowns']) + state > 1

    def solve_result(self, name):
        """Solved | ('table', params, {tuple: Solved})."""
        every = list(range(len(self.im['params'])))
        if not every:
            return self.solve(name)
        return ('table', every, {tuple(pv): self.solve(name, pv) for pv in self.assignments()})

    def solve_text(self, name):
        return self._text(self.solve_result(name), '')

    @staticmethod
    def _point(solved, point, names=True):
        return ', '.join((f'{v.name}=' if names else '') + v.show(n) for v, n in zip(solved.vars[point], point))

    def _solved_text(self, r):
        """A Solved, written as in the `// =>` comments."""
        if r.unknown:
            return UNKNOWN
        if r.form == 'find':
            def show(points):
                if not points:
                    return '{}'
                if len(points[0]) == 1:
                    var = r.vars[points[0]][0]
                    xs = [p[0] for p in points]
                    if var.kind in ('num', 'qty'):
                        empty = ['empty'] if var.opt and xs[0] == var.lo else []
                        return '{' + ', '.join(empty + spans(xs[len(empty):])) + '}'
                    return '{' + ', '.join(var.show(x) for x in xs) + '}'
                return '{' + ', '.join('(' + self._point(r, p, names=False) + ')' for p in points) + '}'

            open_ = r.points(UNKNOWN)
            return show(r.points(True)) + (' unknown ' + show(open_) if open_ else '')
        if r.best is None:
            return NONE
        if r.form == 'any':
            return self._point(r, r.best)
        return f'{r.value}' + (f'[{r.unit}]' if r.unit else '') + ' at ' + self._point(r, r.best)

    def _shown(self, p, v):
        param = self.im['params'][p]
        return param['show'][param['values'].index(v)]

    def _text(self, r, unit):
        if isinstance(r, Solved):
            return self._solved_text(r)
        if r is NONE:
            return NONE
        if isinstance(r, tuple) and r[0] == 'cases':
            return '{' + ', '.join(f'{solve}: {self._text(val, unit)}' for solve, val in r[1]) + '}'
        if isinstance(r, tuple) and r[0] == 'table':
            rows = []
            for key, val in r[2].items():
                head = ', '.join(f'{self.im["params"][p]["name"]}={self._shown(p, v)}' for p, v in zip(r[1], key))
                rows.append(f'{head}: {self._text(val, unit)}')
            return '{' + ', '.join(rows) + '}'
        if isinstance(r, tuple) and r[0] == 'region':
            params = r[1]

            def show(hits):
                if len(params) == 1:
                    xs = [h[0] for h in hits]
                    if self.im['params'][params[0]]['kind'] == 'num':
                        return '{' + ', '.join(spans(xs)) + '}'
                    return '{' + ', '.join(self._shown(params[0], x) for x in xs) + '}'
                return '{' + ', '.join('(' + ', '.join(self._shown(p, x) for p, x in zip(params, h)) + ')'
                                       for h in hits) + '}'

            return show(r[2]) + (' unknown ' + show(r[3]) if r[3] else '')
        if r is UNKNOWN:
            return UNKNOWN
        if isinstance(r, bool):
            return 'true' if r else 'false'
        return str(r) + (f'[{unit}]' if unit else '')
