"""FEAN queries: runs, policies, expectations, regions and preferences; and solves, by trying every candidate.

Meaning: docs/financial-language.md section 5 (solves: 5.4). Map units: I-04-b, I-15-a.
"""
from fractions import Fraction
from itertools import product

from .dynamic import Exploration, Machine, State
from .static import base, is_opt, unit_text
from .syntax import ModelErr, SearchLimit, StaticErr

UNKNOWN = 'UNKNOWN'
NONE = 'none'
INVALID = 'invalid'
ENUM_LIMIT = 200_000                # more candidates than this are not tried: the answer is UNKNOWN
SLOT_RANGE = (-10 ** 9, 10 ** 9)    # where the value of a quantity slot is looked for


class Analysis:
    """All queries over one fully instantiated model (every parameter fixed)."""

    def __init__(self, machine):
        self.m = machine
        self.ex = Exploration(machine)
        self.leaves = self.ex.leaves
        self._choices, self._probs = {}, {}

    def rv(self, e, leaf):
        return self.m.ev(e, None, None, None, None, leaf)

    def choices(self, leaf):
        """[(actor, view at the menu, action)] along the history."""
        c = self._choices.get(id(leaf))
        if c is None:
            c = [(n.step[1], self.ex.view(n.parent, n.step[1]), n.step[3])
                 for n in self.ex.path(leaf) if n.step[0] == 'pick']
            self._choices[id(leaf)] = c
        return c

    def policies(self, coalition):
        """Every profile of view-based deterministic strategies for the coalition."""
        menus = {}
        for n in self.ex.nodes:
            if n.step[0] == 'show' and n.step[1] in coalition:
                menus[(n.step[1], self.ex.view(n, n.step[1]))] = n.display[2]
        order = list(menus)
        for acts in product(*[menus[key] for key in order]):
            yield dict(zip(order, acts))

    def follows(self, leaf, policy, coalition):
        return all(policy[(i, v)] == a for i, v, a in self.choices(leaf) if i in coalition)

    def probs(self, weight):
        if weight not in self._probs:
            raw = [self.m.ev(self.m.spec.weights[weight], w) for w in self.m.worlds]
            if any(x < 0 for x in raw) or sum(raw) == 0:
                raise ModelErr('D-Weight', f'weight {weight} is negative or sums to zero')
            self._probs[weight] = [Fraction(x, sum(raw)) for x in raw]
        return self._probs[weight]

    def query(self, q):
        kind = q[0]
        if kind == 'exists':
            return any(self.rv(q[1], leaf) for leaf in self.leaves)
        if kind == 'forall':
            return all(self.rv(q[1], leaf) for leaf in self.leaves)
        if kind == 'can':
            who = set(q[1])
            return any(all(self.rv(q[2], leaf) for leaf in self.leaves if self.follows(leaf, pol, who))
                       for pol in self.policies(who))
        if kind == 'num':
            return self.number(*q[1:])
        return self.prefer(*q[1:])

    def number(self, policy, weight, opt, e):
        pick = max if opt == 'max' else min

        def value(leaves):
            if weight is None:
                return pick(self.rv(e, leaf) for leaf in leaves)
            total = Fraction(0)
            for w, p in enumerate(self.probs(weight)):
                if p:
                    total += p * pick(self.rv(e, leaf) for leaf in leaves if leaf.world == w)
            return total

        if policy is None:
            return value(self.leaves)
        who = set(policy)
        return max(value([leaf for leaf in self.leaves if self.follows(leaf, pol, who)])
                   for pol in self.policies(who))

    def prefer(self, who, point, a, b, value, weight):
        """True when, in every information set of `who` at the menu, a scores strictly above b."""
        groups = {}
        for n in self.ex.nodes:
            if n.step[0] == 'show' and n.step[2] == point[1]:
                groups.setdefault(self.ex.view(n, who), []).append(n)
        probs = self.probs(weight) if weight is not None else None
        target = ('name', value)

        def score(nodes, action):
            worst = {}
            for n in nodes:
                for child in n.children:
                    if child.step[3] == action:
                        for leaf in self.ex.under(child):
                            v = self.rv(target, leaf)
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
            sa, sb = score(nodes, a), score(nodes, b)
            if sa is None or sb is None or not sa > sb:
                return False
        return True


def spans(xs):
    """[0,1,2,5] -> ['0..2', '5']."""
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
    __slots__ = ('name', 'type', 'lo', 'hi', 'opt')

    def __init__(self, name, type_, lo, hi, opt=False):
        self.name, self.type, self.lo, self.hi, self.opt = name, type_, lo, hi, opt

    def value(self, spec, n):
        """The runtime value the number stands for."""
        if self.opt and n == self.lo:
            return None
        if self.type == ('Bool',):
            return bool(n)
        if self.type[0] == 'Enum':
            return spec.enums[self.type[1]][n]
        if self.type == ('Actor',):
            return spec.actors[n]
        return n

    def show(self, spec, n):
        v = self.value(spec, n)
        if v is None:
            return 'empty'
        if isinstance(v, bool):
            return 'true' if v else 'false'
        return f'@{v}' if self.type == ('Time',) else str(v)


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


class _Verdicts:
    """The query names of a goal: each stands for the verdict of that query in this instance."""

    def __init__(self, spec, analysis):
        self.spec, self.analysis, self.memo = spec, analysis, {}

    def __contains__(self, name):
        return name in self.spec.queries

    def __getitem__(self, name):
        if name not in self.memo:
            self.memo[name] = self.analysis.query(self.spec.queries[name])
        return self.memo[name]


class Family:
    """A checked model together with its public parameters: one Analysis per assignment."""

    def __init__(self, spec, budget=200_000, limit=ENUM_LIMIT):
        self.spec, self.budget, self.limit = spec, budget, limit
        self._cache, self._solved = {}, {}

    def assignments(self):
        names = list(self.spec.params)
        return [dict(zip(names, combo)) for combo in product(*[self.spec.params[n][1] for n in names])]

    def analysis(self, pvals=None):
        pvals = pvals or {}
        key = tuple(sorted(pvals.items()))
        if key not in self._cache:
            self._cache[key] = Analysis(Machine(self.spec, pvals, self.budget))
        return self._cache[key]

    def evaluate(self, pvals, q):
        return self._evaluate(self.analysis, pvals, q)

    @staticmethod
    def _evaluate(analysis, pvals, q):
        try:
            return analysis(pvals).query(q)
        except SearchLimit:
            return UNKNOWN

    def result(self, name):
        """bool | number | ('region', names, true tuples, undecided tuples) | ('table', names, {tuple: result})
        | ('cases', [(solve name, one of the above | NONE)]) when the model has unknowns."""
        q = self.spec.queries[name]
        if self.spec.unknowns:
            return ('cases', [(solve, self._in_case(q, solve)) for solve in self.spec.solves])
        return self._over(q, self.assignments(), self.analysis)

    def _in_case(self, q, solve):
        """A plain query, answered where the solve's representative solution holds."""
        found = [pv for pv in self.assignments() if self.case(solve, pv) is not None]
        if not self.spec.params and not found:
            return NONE
        return self._over(q, found, lambda pv: self.instance(solve, pv, self.case(solve, pv)))

    def _over(self, q, assignments, analysis):
        names = list(self.spec.params)
        if q[0] == 'region':
            rest = [n for n in names if n not in q[1]]
            table = {} if rest else {(): ([], [])}
            for pv in assignments:
                hits, undecided = table.setdefault(tuple(pv[n] for n in rest), ([], []))
                verdict = self._evaluate(analysis, pv, q[2])
                if verdict is True:
                    hits.append(tuple(pv[n] for n in q[1]))
                elif verdict is UNKNOWN:
                    undecided.append(tuple(pv[n] for n in q[1]))
            if not rest:
                return ('region', q[1], *table[()])
            return ('table', rest, {key: ('region', q[1], *found) for key, found in table.items()})
        if not names:
            return self._evaluate(analysis, {}, q)
        return ('table', names, {tuple(pv[n] for n in names): self._evaluate(analysis, pv, q)
                                 for pv in assignments})

    def text(self, name):
        info = self.spec.qinfo[name]
        return self._text(self.result(name), info[1] if info[0] == 'num' else ())

    def results(self):
        out = {name: self.text(name) for name in self.spec.queries}
        out.update((name, self.solve_text(name)) for name in self.spec.solves)
        return out

    # ---- solves: the definition, by trying every candidate -------------------
    def solve(self, name, pvals=None):
        """The Solved of one solve under one parameter assignment."""
        pvals = dict(pvals or {})
        key = (name, tuple(sorted(pvals.items())))
        if key not in self._solved:
            self._solved[key] = self._solve(name, pvals)
        return self._solved[key]

    def case(self, name, pvals=None):
        """The representative solution of a solve (a candidate), or None."""
        return self.solve(name, pvals).best

    def instance(self, name, pvals, point):
        """The Analysis of the model where a candidate of the solve holds."""
        label = self.spec.solves[name][3]
        uvars, values = self._unknowns(point)
        vals = {**dict(pvals or {}), **values}
        if label is None:
            return Analysis(Machine(self.spec, vals, self.budget))
        frame = Machine(self.spec, vals, self.budget, start=(label, None))
        state = self._state(self._vars(uvars, frame), point)
        return Analysis(Machine(self.spec, vals, self.budget, start=(label, state)))

    def _unknowns(self, point):
        spec = self.spec
        uvars = [Var(n, base(t), lo, hi) for n, (t, lo, hi) in spec.unknowns.items()]
        return uvars, {v.name: v.value(spec, n) for v, n in zip(uvars, point)}

    def _interval(self, bt, declared):
        spec = self.spec
        if bt[0] == 'Qty':
            return declared
        if bt == ('Bool',):
            return 0, 1
        if bt[0] == 'Enum':
            return 0, len(spec.enums[bt[1]]) - 1
        return spec.time if bt == ('Time',) else (0, len(spec.actors) - 1)

    def _vars(self, uvars, frame):
        """The coordinates of a candidate at a point: the unknowns, then the state (clock, cells, holdings, slots)."""
        spec = self.spec
        out = list(uvars) + [Var('now', ('Time',), *spec.time)]
        for name, c in spec.cells.items():
            lo, hi = self._interval(base(c.type), frame.ranges.get(name))
            out.append(Var(name, base(c.type), lo - is_opt(c.type), hi, is_opt(c.type)))
        for x in spec.assets:
            for i in spec.actors:
                out.append(Var(f'bal({i}, {x})', ('Qty', ((x, 1),)), *frame.ranges[('bal', i, x)]))
        for (who, key), ty in spec.slots.items():
            lo, hi = self._interval(base(ty), SLOT_RANGE)
            out.append(Var(f'mem({who}, {key})', base(ty), lo - 1, hi, True))
            out.append(Var(f'asof({who}, {key})', ('Time',), *spec.time))
            out.append(Var(f'recv({who}, {key})', ('Time',), *spec.time))
        return out

    def _state(self, vars_, point):
        """The state a candidate stands for; None when it is a second spelling of a state (a slot that is
        absent carries the earliest times)."""
        spec = self.spec
        vals = iter([v.value(spec, n) for v, n in zip(vars_, point)][len(spec.unknowns):])
        t, sig, mu = next(vals), {}, {}
        for name in spec.cells:
            sig[name] = next(vals)
        for x in spec.assets:
            for i in spec.actors:
                sig[('bal', i, x)] = next(vals)
        for slot in spec.slots:
            value, asof, recv = next(vals), next(vals), next(vals)
            if value is not None:
                mu[slot] = (value, asof, recv)
            elif (asof, recv) != (spec.time[0], spec.time[0]):
                return None
        return State(t, sig, mu)

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

    def _solve(self, name, pvals):
        _, form, objective, label, where, goal = self.spec.solves[name]
        spec, out = self.spec, Solved(form, self.spec.sinfo[name])
        uvars, _ = self._unknowns(())
        total = 1
        for v in uvars:
            total *= v.hi - v.lo + 1
        if total > self.limit:
            out.limit = out.unknown = True
            return out
        todo, total = [], 0                             # [(unknown numbers, values in force, [Var], frame)]
        for combo in product(*[range(v.lo, v.hi + 1) for v in uvars]):
            vals = {**pvals, **self._unknowns(combo)[1]}
            frame = Machine(spec, vals, self.budget, start=(label, None))   # only evaluates; never explored
            vars_ = uvars if label is None else self._vars(uvars, frame)
            total += self._size(vars_, len(uvars))
            if total > self.limit:
                out.limit = out.unknown = True
                return out
            todo.append((combo, vals, vars_, frame))
        best = None
        for combo, vals, vars_, frame in todo:
            for rest in product(*[range(v.lo, v.hi + 1) for v in vars_[len(uvars):]]):
                point, state = combo + rest, None
                if label is not None:
                    state = self._state(vars_, point)
                    if state is None or not all(frame.ev(inv, state) for inv in spec.invariants):
                        continue
                if where is not None and not frame.ev(where, state):
                    continue
                out.vars[point] = vars_
                verdict, score = self._try(vals, label, state, goal, objective)
                out.verdicts[point] = verdict
                if verdict is True and objective is not None:
                    better = best is None or (score < best if form == 'min' else score > best)
                    if better:
                        best, out.best = score, point
        hits, open_ = out.points(True), out.points(UNKNOWN)
        if form == 'find':
            out.best = hits[0] if hits else None
        elif form == 'any':
            first = hits[0] if hits else None
            out.unknown = any(first is None or p < first for p in open_)
            out.best = None if out.unknown else first
        else:
            out.unknown = bool(open_)
            out.best, out.value = (None, None) if out.unknown else (out.best, best)
        return out

    def _try(self, vals, label, state, goal, objective):
        """(verdict, objective value) at one candidate: the model is set up there, run, and the goal read."""
        try:
            machine = Machine(self.spec, vals, self.budget, start=None if label is None else (label, state))
            names = _Verdicts(self.spec, Analysis(machine))
            if not machine.ev(goal, state, names):
                return False, None
            return True, None if objective is None else machine.ev(objective, state, names)
        except SearchLimit:
            return UNKNOWN, None
        except (StaticErr, ModelErr):
            return INVALID, None

    def wide(self, name):
        """A `find` over several coordinates: the solver writes it as regions, this engine as a list of solutions."""
        spec = self.spec
        _, form, _, label, _, _ = spec.solves[name]
        state = 0 if label is None else 1 + len(spec.cells) + len(spec.assets) * len(spec.actors) + 3 * len(spec.slots)
        return form == 'find' and len(spec.unknowns) + state > 1

    def solve_result(self, name):
        """Solved | ('table', names, {tuple: Solved})."""
        names = list(self.spec.params)
        if not names:
            return self.solve(name)
        return ('table', names, {tuple(pv[n] for n in names): self.solve(name, pv) for pv in self.assignments()})

    def solve_text(self, name):
        return self._text(self.solve_result(name), ())

    def _point(self, solved, point, sep=', ', names=True):
        return sep.join((f'{v.name}=' if names else '') + v.show(self.spec, n)
                        for v, n in zip(solved.vars[point], point))

    def _solved_text(self, r):
        """A Solved, written as in the `// =>` comments."""
        spec = self.spec
        if r.unknown:
            return UNKNOWN
        if r.form == 'find':
            def show(points):
                if not points:
                    return '{}'
                if len(points[0]) == 1:
                    var = r.vars[points[0]][0]
                    xs = [p[0] for p in points]
                    if var.type[0] == 'Qty':
                        empty = ['empty'] if var.opt and xs[0] == var.lo else []
                        return '{' + ', '.join(empty + spans(xs[len(empty):])) + '}'
                    return '{' + ', '.join(var.show(spec, x) for x in xs) + '}'
                return '{' + ', '.join('(' + self._point(r, p, names=False) + ')' for p in points) + '}'

            open_ = r.points(UNKNOWN)
            return show(r.points(True)) + (' unknown ' + show(open_) if open_ else '')
        if r.best is None:
            return NONE
        if r.form == 'any':
            return self._point(r, r.best)
        return f'{r.value}' + (f'[{unit_text(r.unit)}]' if r.unit else '') + ' at ' + self._point(r, r.best)

    def _pv(self, name, v):
        if isinstance(v, bool):
            return 'true' if v else 'false'
        return f'@{v}' if self.spec.params[name][0] == ('Time',) else str(v)

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
                head = ', '.join(f'{n}={self._pv(n, v)}' for n, v in zip(r[1], key))
                rows.append(f'{head}: {self._text(val, unit)}')
            return '{' + ', '.join(rows) + '}'
        if isinstance(r, tuple) and r[0] == 'region':
            names = r[1]

            def show(hits):
                if len(names) == 1:
                    xs = [h[0] for h in hits]
                    if base(self.spec.params[names[0]][0])[0] == 'Qty':
                        return '{' + ', '.join(spans(xs)) + '}'
                    return '{' + ', '.join(self._pv(names[0], x) for x in xs) + '}'
                return '{' + ', '.join('(' + ', '.join(self._pv(n, x) for n, x in zip(names, h)) + ')'
                                       for h in hits) + '}'

            return show(r[2]) + (' unknown ' + show(r[3]) if r[3] else '')
        if r is UNKNOWN:
            return UNKNOWN
        if isinstance(r, bool):
            return 'true' if r else 'false'
        return str(r) + (f'[{unit_text(unit)}]' if unit else '')
