"""Checks for the Rust engine, the solver, the history views, the IDE and the manual.

The Rust engine is judged against the Python engine (DR-30): the same verdict on every text, the
same image byte for byte, the same results and the same history tree. The solver is judged against
the definition of a solve, which the Python engine computes by trying every candidate (DR-54).
The history views are judged against the Python machine, the IDE through its automatic mode
(DR-33), the manual against the IDE's own feature list. Every test name here is an acceptance
criterion of the project's concept map, a design record kept outside this repository (C-09 to C-12,
C-14 to C-17).

The executables come from `bash rust/build.sh`; the tests refuse to run against stale ones.
"""
import copy
import json
import random
import re
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import fean  # noqa: E402
import test_fean_engine as E  # noqa: E402
import test_fean_v02 as V  # noqa: E402
from fean import Session, compile_source, dumps, listing, verify  # noqa: E402
from fean.static import count_labels  # noqa: E402
from fean.syntax import FeanError, SearchLimit  # noqa: E402
from fean.vm import INVALID, UNKNOWN  # noqa: E402
from fean_ref import expectations  # noqa: E402

DIST = ROOT / 'dist'
FEAN = DIST / 'fean'
IDE = DIST / 'fean-ide'
EXAMPLE = ROOT / 'examples' / 'repay80.fean'


def fresh(exe, *crates):
    """Fail early when an executable is missing or older than what it is built from."""
    sources = [ROOT / 'fean' / 'prelude.fean', ROOT / 'rust' / 'Cargo.toml']
    for crate in crates:
        sources += [p for p in (ROOT / 'rust' / crate).rglob('*') if p.suffix in ('.rs', '.toml')]
    newest = max(p.stat().st_mtime for p in sources if not p.name.startswith('._'))
    if not exe.exists() or exe.stat().st_mtime < newest:
        raise RuntimeError(f'{exe.relative_to(ROOT)} 이 없거나 소스보다 오래됐습니다. 먼저 `bash rust/build.sh` 를 실행하십시오.')


def batch(jobs):
    """Run jobs in one process of the Rust engine: [{'op': ..., 'src': ...}] -> [result]."""
    done = subprocess.run([str(FEAN), 'batch'], input=json.dumps({'jobs': jobs}), capture_output=True, text=True)
    assert done.returncode == 0, done.stderr
    out = json.loads(done.stdout)
    assert len(out) == len(jobs)
    return out


def rust(*args, stdin=None):
    return subprocess.run([str(FEAN), *args], cwd=ROOT, input=stdin, capture_output=True, text=True)


def python_verdict(src, prelude=None):
    """(error class, rule, line) of the Python front end, or None when the text is accepted."""
    try:
        fean.frontend(src, prelude)
    except FeanError as exc:
        return type(exc).__name__, exc.code, exc.line
    return None


def rust_verdict(result):
    return None if result['ok'] else (result['kind'], result['code'], result['line'])


def plain(v):
    return None if v is None else int(v)


def failure(exc):
    if isinstance(exc, SearchLimit):
        return {'kind': 'SearchLimit', 'code': 'limit'}
    return {'kind': type(exc).__name__, 'code': exc.code}


def python_histories(src, budget=200_000):
    """What `fean batch` reports for the op `histories`, computed by the Python engine."""
    try:
        session = Session(compile_source(src), budget)
    except FeanError as exc:
        return failure(exc)
    rows = []
    for pv in session.assignments():
        row = {'pvals': [plain(v) for v in pv]}
        try:
            vm = session.vm(pv).explore()
        except (FeanError, SearchLimit) as exc:
            row['error'] = failure(exc)
        else:
            row['worlds'] = len(vm.worlds)
            row['steps'] = [list(n.step) for n in vm.nodes]
            row['leaves'] = [[leaf.world, leaf.fail, leaf.state.t, [plain(v) for v in leaf.state.cells],
                              [None if m is None else [plain(m[0]), plain(m[1]), m[2]] for m in leaf.state.mem]]
                             for leaf in vm.leaves]
        rows.append(row)
    try:
        results = [[name, text] for name, text in session.results().items()]
    except FeanError as exc:
        results = failure(exc)
    return {'assignments': rows, 'results': results}


def rust_histories(result):
    def short(err):
        return {'kind': err['kind'], 'code': err['code']}

    if not result['ok']:
        return short(result)
    rows = []
    for row in result['assignments']:
        if 'error' in row:
            row = {'pvals': row['pvals'], 'error': short(row['error'])}
        rows.append(row)
    results = result['results']
    return {'assignments': rows, 'results': short(results) if isinstance(results, dict) else results}


ARITHMETIC = '''model Arithmetic {
  actor A
  time 0..0
  param n : Num in 0..3
  query down  = max run: floor(-7 / 2)
  query up    = max run: ceil(-7 / 2)
  query plain = max run: floor(7 / 2)
  query other = max run: floor(7 / (n - 2)) ?? 99
  query above = max run: ceil(-7 / (n - 2)) ?? 99
  query whole = max run: exact(-8 / (n - 2)) ?? 99
  query part  = max run: exact(-7 / (n - 2)) ?? 99
  query mixed = max run: min(-3, n) + max(n, 2) * -2
}'''


# ---- the solver ---------------------------------------------------------------------------------

# What a candidate is, in the words of the solver's report.
WORD = {True: 'yes', False: 'no', INVALID: 'invalid', UNKNOWN: 'undecided', None: 'outside'}

# Wide ranges: no engine can try every candidate here. What is right is known by arithmetic.
BIG = {
    'Capital': '''model BigCapital {
  actor I, D, L
  unknown cap : Num in 0..1000000000
  asset KRW in 0..1000000000 init I = cap
  time 0..1
  cell remain : Qty[KRW] in 0..80000000 init 80000000 write collect observe D, L
  event award by I { move I -> D : bal(I, KRW) }
  event collect by D {
    let b = min(bal(D, KRW), remain)
    move D -> L : b
    remain -= b
  }
  program { run award  at @1 { run collect } }
  query repaid = forall run: at(end, remain == 0[KRW])
  query left   = max run: at(end, bal(D, KRW)) ?? 0[KRW]
  solve need   = min cap: repaid
  solve enough = find: repaid
  solve spare  = any: repaid and left >= 20000000[KRW]
  solve short  = max cap where cap <= 150000000: not repaid
  solve band   = find: repaid and left <= 999[KRW]
}''',
    'Fees': '''model BigFees {
  actor I, D, L, J
  unknown cap : Num in 0..1000000000
  unknown fee : Num in 0..1000000000
  asset KRW in 0..1000000000 init I = cap
  time 0..1
  cell remain : Qty[KRW] in 0..20000 init 20000 write collect observe D, L
  event award by I { move I -> D : bal(I, KRW) }
  event charge by D { move D -> J : fee * 1[KRW] }
  event collect by D {
    let b = min(bal(D, KRW), remain)
    move D -> L : b
    remain -= b
  }
  program { run award  attempt charge { } else { }  at @1 { run collect } }
  query repaid = forall run: at(end, remain == 0[KRW])
  query kept   = min run: at(end, bal(D, KRW)) ?? 0[KRW]
  solve ok    = find: repaid
  solve first = any: repaid and fee >= 3000000
  solve lean  = min cap where fee >= 5000000: repaid
  solve rich  = max kept where cap <= 30000000: repaid
  solve dear  = max fee where cap == 25000000: repaid
  solve never = any where cap <= 19999: repaid and fee <= cap
}''',
    'Interest': '''model Interest {
  actor D, L
  unknown principal : Num in 0..1000000000
  unknown funds     : Num in 0..2000000000
  asset KRW in 0..2000000000 init D = funds
  time 0..1
  cell owed : Qty[KRW] in 0..2000000000 init 0 write accrue, repay
  cell done : Bool init false write repay
  event accrue by L { owed := floor(principal * 10500 / 10000) * 1[KRW] }
  event repay by D {
    require bal(D, KRW) >= owed
    move D -> L : owed
    done := true
  }
  program { run accrue  at @1 { attempt repay { } } }
  query repaid = forall run: at(end, done)
  solve cash    = min funds where principal == 123456789: repaid
  solve largest = max principal where funds == 1000000: repaid
  solve every   = find: repaid
  solve sharp   = any where principal >= 500000000: repaid and funds * 10000 == principal * 10500
}''',
}


# The end of 64 bits. Quantities of the Rust engine are 64-bit: where `a * 4` does not fit, the model is not
# valid there (D-Overflow) and the candidate is no solution. (Python's numbers do not end, so only arithmetic
# can say what is right here.)
MOST = (2 ** 63 - 1) // 4
EDGE = f'''model Edge {{
  actor A
  unknown a : Num in {MOST - 11}..{MOST + 9}
  time 0..0
  solve fits = find: a * 4 >= 0
  solve last = max a: a * 4 >= 0
  solve past = any: a * 4 < 0
}}'''


def as_solver_model(src, points):
    """A model of the engine checks as a solver problem: its parameters become unknowns, its regions plain
    queries, and every query gets solves (with `points`: one at every label that is written once)."""
    text = re.sub(r'^(\s*)param(\s+\w+\s*:)', r'\1unknown\2', src, flags=re.M)
    text = re.sub(r'=\s*region\s+[\w, ]+:\s*', '= ', text)
    spec = fean.frontend(text)
    bools = [q for q, info in spec.qinfo.items() if info[0] == 'bool']
    numbers = [q for q, info in spec.qinfo.items() if info[0] == 'num' and spec.queries[q][2] is None]
    extra = []
    if spec.unknowns and not points:
        for q in bools:
            extra += [f'  solve s_{q} = find: {q}', f'  solve a_{q} = any: not {q}']
        for q in numbers:
            extra += [f'  solve hi_{q} = max {q}: {q} >= {q}', f'  solve lo_{q} = min {q}: {q} >= {q}']
    if points and bools:
        extra += [f'  solve p_{label.replace(".", "_")} = find at({label}): {bools[0]}'
                  for label in sorted(spec.labels) if count_labels(spec.program, label) == 1]
    return text.rstrip()[:-1] + '\n'.join(extra) + '\n}\n' if extra else None


def rust_solves(src, points=None, budget=200_000, runs=None):
    """Every solve of a text as the Rust solver answers it: [{name, form, rows: [one per assignment]}].
    `points` = {solve name: [[pvals, [point, ...]], ...]} asks for the solver's verdict at those points."""
    job = {'op': 'solve', 'src': src, 'budget': budget, 'points': points or {}}
    if runs is not None:
        job['runs'] = runs
    result = batch([job])[0]
    assert result['ok'], result
    return result['solves']


def coordinates(session, name, pvals):
    """The coordinates of a solve's candidates, with their declared bounds (the Python engine's Var list)."""
    entry = session._entry(name)
    if entry['point'] is None:
        return session._unknowns()
    frame = fean.Vm(session.im, pvals, session.budget, [u['lo'] for u in session.im['unknowns']], start=(entry['point'], None))
    return session._vars(frame)


def agree(test, got, want, points, what):
    """Two lists of verdicts over the same candidates are the same. If not, say where they first differ: the
    difference of thousands of verdicts takes minutes to print and tells less."""
    if got != want:
        wrong = [i for i, (g, w) in enumerate(zip(got, want)) if g != w]
        at = wrong[0] if wrong else min(len(got), len(want))
        test.fail(f'{what}: {len(wrong)} of {len(want)} candidates differ (the solver answered {len(got)}); first at '
                  f'{list(points[at]) if at < len(points) else "the end"}: solver {got[at] if at < len(got) else None}, '
                  f'definition {want[at] if at < len(want) else None}')


class TestSolver(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fresh(FEAN, 'fean-core', 'fean-cli')

    def enumerated(self, name, src, budget=200_000):
        """The solver against the definition: every candidate, the text, the case, the first non-solution."""
        session = Session(compile_source(src), budget)
        asked = {sv['name']: [[[plain(v) for v in pv], [list(p) for p in session.solve(sv['name'], pv).verdicts]]
                              for pv in session.assignments()] for sv in session.im['solves']}
        seen = 0
        for sv, got in zip(session.im['solves'], rust_solves(src, asked, budget)):
            self.assertEqual((got['name'], got['form'], len(got['rows'])), (sv['name'], sv['form'], len(session.assignments())))
            for pv, row in zip(session.assignments(), got['rows']):
                with self.subTest(model=name, solve=sv['name'], pvals=pv):
                    solved = session.solve(sv['name'], pv)
                    self.assertFalse(solved.limit)
                    self.assertIsNone(row['error'])
                    agree(self, [v[0] for v in row['verdicts']], [WORD[v] for v in solved.verdicts.values()],
                          list(solved.verdicts), f'{name}.{sv["name"]}')
                    self.assertEqual(row['best'], None if solved.best is None else list(solved.best))
                    self.assertEqual(row['value'], None if solved.value is None else str(solved.value))
                    self.assertEqual(row['unknown'], solved.unknown)
                    self.assertEqual(row['complete'], not solved.points(UNKNOWN))
                    if not session.wide(sv['name']):
                        self.assertEqual(row['text'], session._solved_text(solved))
                    if row['complete']:                                         # the first candidate that is no solution
                        self.assertEqual(row['counter'], next((list(p) for p, v in solved.verdicts.items() if v is not True), None))
                    self.assertEqual(row['vars'], [v.name for v in coordinates(session, sv['name'], pv)])
                    seen += len(solved.verdicts)
        return seen

    def sampled(self, name, src, count, seed=7):
        """Where there are too many candidates to try: the solver against single candidates tried by the
        Python engine, at random and around the points the solver itself ran the model at."""
        rng = random.Random(seed)
        session = Session(compile_source(src))
        first = rust_solves(src)
        asked, picked = {}, {}
        for sv, got in zip(session.im['solves'], first):
            rows = []
            for pv, row in zip(session.assignments(), got['rows']):
                self.assertIsNone(row['error'], (name, sv['name']))
                self.assertTrue(row['complete'], (name, sv['name']))
                vars_ = coordinates(session, sv['name'], pv)
                points = [[rng.randint(v.lo, v.hi) for v in vars_] for _ in range(count)]
                for region in row['regions']:                                   # the borders are near the samples
                    for k in range(len(vars_)):
                        for step in (-1, 1):
                            point = list(region['sample'])
                            point[k] = min(vars_[k].hi, max(vars_[k].lo, point[k] + step))
                            points.append(point)
                    points.append(list(region['sample']))
                rows.append([[plain(v) for v in pv], points])
                picked[(sv['name'], tuple(pv))] = points
            asked[sv['name']] = rows
        seen = {}
        for sv, got in zip(session.im['solves'], rust_solves(src, asked)):
            for pv, row in zip(session.assignments(), got['rows']):
                with self.subTest(model=name, solve=sv['name'], pvals=pv):
                    points = picked[(sv['name'], tuple(pv))]
                    want = [WORD[session.judge(sv['name'], pv, tuple(p))] for p in points]
                    agree(self, [v[0] for v in row['verdicts']], want, points, f'{name}.{sv["name"]}')
                    for word in want:
                        seen[word] = seen.get(word, 0) + 1
                    for region in row['regions']:                               # a region has the verdict of its sample
                        self.assertEqual(region['verdict'], WORD[session.judge(sv['name'], pv, tuple(region['sample']))])
        return seen

    def test_solver_matches_enumeration(self):
        texts = dict(E.SOLVERS)
        for name, src in {**E.CORPUS, 'Arithmetic': ARITHMETIC}.items():       # the engine's own models, turned round
            derived = None if name in E.SOLVERS else as_solver_model(src, points=False)
            if derived is not None:
                texts[name + '?'] = derived
        self.assertEqual(len(texts), 23)
        # the two front ends make the same image of every one of them
        for (name, src), result in zip(texts.items(), batch([{'op': 'compile', 'src': src} for src in texts.values()])):
            with self.subTest(image=name):
                image = compile_source(src)
                self.assertEqual((result['image'], result['listing']), (dumps(image), listing(image)))
        candidates = sum(self.enumerated(name, src) for name, src in texts.items())
        self.assertGreater(candidates, 14_000)
        solves = sum(len(compile_source(src)['solves']) for src in texts.values())
        self.assertGreater(solves, 140)
        # what the solver writes for the documented examples is what the document says
        for name in ('Capital', 'Escrow'):
            got = {sv['name']: sv['rows'][0]['text'] for sv in rust_solves(V.MODELS[name])}
            want = expectations(V.MODELS[name])
            self.assertEqual(got, {k: want[k] for k in got})
        escrow = {sv['name']: sv['rows'][0] for sv in rust_solves(V.MODELS['Escrow'])}
        self.assertEqual(escrow['pre']['text'],
                         '{[due <= bal(B, KRW), due + bal(S, KRW) <= 3], [paid=true, due >= bal(B, KRW) + 1]}')
        self.assertEqual(sorted(r['verdict'] for r in escrow['pre']['regions']), ['no', 'no', 'yes', 'yes'])
        # far fewer runs than candidates: the model is run once per way it can go
        capital = {sv['name']: sv['rows'][0] for sv in rust_solves(V.MODELS['Capital'])}
        self.assertTrue(all(row['runs'] <= 6 for row in capital.values()), capital)
        self.assertEqual([(r['verdict'], r['text']) for r in capital['need']['regions']],
                         [('no', '[cap <= 79]'), ('yes', '[cap >= 80]')])
        # the regions are written with names and conditions, quotients by how they are computed
        texts = {f'{name}.{sv["name"]}': sv['rows'][0]['text'] for name in ('Fees', 'Kinds', 'Split', 'Limits', 'Gate', 'Area')
                 for sv in rust_solves(V.SOLVE_MODELS[name])}
        self.assertEqual(texts['Fees.ok'], '{[cap >= fee + 20]}')
        self.assertEqual(texts['Kinds.every'], '{[mode=fast, loose=false, when <= @2], [loose=true, when <= @2]}')
        self.assertEqual(texts['Split.band'], '{[total <= 13, parts=0, total >= 3*floor(total / 3) + 1]}')
        self.assertEqual(texts['Limits.ok'], '{[n=4..6, n <= lim], [n=7, lim=7], [n=8, lim=8]}')
        self.assertEqual(texts['Gate.idle'], '{[hold!=empty]}')
        self.assertEqual(texts['Area.ok'], '{[w >= 3, h >= 4], [w >= 4, h=3], [w=6, h=2]}')    # a product: one side at a time
        for name, want in V.SOLVE_EXPECTED.items():                             # the aimed models: what arithmetic says
            got = {sv['name']: sv['rows'][0]['text'] for sv in rust_solves(V.SOLVE_MODELS[name])}
            self.assertEqual({k: got[k] for k in got if k != 'ok'}, {k: want[k] for k in got if k != 'ok'}, name)
        # a search limit: the candidates left open are the same ones, and the answers the same
        for name in ('Pickup', 'Gate', 'Signal'):
            for budget in (2, 3, 6):
                self.assertGreater(self.enumerated(f'{name} @{budget}', V.SOLVE_MODELS[name], budget), 0)
        tiny = {sv['name']: sv['rows'][0] for sv in rust_solves(V.SOLVE_MODELS['Pickup'], budget=3)}
        self.assertEqual((tiny['dull']['text'], tiny['flat']['text'], tiny['sharp']['text']),
                         ('now=@0, step=3, bal(T, KRW)=2, bal(U, KRW)=0', 'UNKNOWN', 'UNKNOWN'))
        self.assertIn(' unknown {[', tiny['reach3']['text'])
        # a damaged table of unknowns or solves is refused by the same rule
        good = compile_source(V.SOLVE_MODELS['Tiers'])
        cases = E.solve_breakages(good)
        images = []
        for _, breakage in cases:
            images.append(copy.deepcopy(good))
            breakage(images[-1])
        for (code, _), result in zip(cases, batch([{'op': 'verify', 'image': image} for image in images])):
            self.assertEqual((result['ok'], result['kind'], result['code']), (False, 'ImageErr', code), result)
        old = copy.deepcopy(compile_source(V.MODELS['Order']))                   # an image of format 1
        old['version'] = 1
        del old['unknowns'], old['solves']
        self.assertEqual(batch([{'op': 'verify', 'image': old}])[0]['code'], 'I-Format')

    def test_solver_states_match_enumeration(self):
        # small states: every state of the point, also those that are no candidates
        seen, words = 0, set()
        for name in ('Escrow', 'Gate', 'Pickup'):
            src = E.SOLVERS[name]
            session = Session(compile_source(src))
            points, asked = {}, {}
            for sv in session.im['solves']:
                if sv['point'] is not None:
                    vars_ = coordinates(session, sv['name'], [])
                    grid = [[]]
                    for v in vars_:
                        grid = [p + [x] for p in grid for x in range(v.lo, v.hi + 1)]
                    points[sv['name']] = grid
                    asked[sv['name']] = [[[], grid]]
            self.assertGreaterEqual(len(points), 4, name)
            for got in rust_solves(src, asked):
                if got['name'] in points:
                    with self.subTest(model=name, solve=got['name']):
                        want = [WORD[session.judge(got['name'], [], tuple(p))] for p in points[got['name']]]
                        agree(self, [v[0] for v in got['rows'][0]['verdicts']], want, points[got['name']], f'{name}.{got["name"]}')
                        words |= set(want)
                        seen += len(want)
        self.assertGreater(seen, 14_000)
        self.assertEqual(words, {'yes', 'no', 'invalid', 'outside'})
        # large states: the engine's own models, solved at each of their labels
        kinds, models = {}, 0
        for name, src in E.CORPUS.items():
            derived = None if name in E.SOLVERS else as_solver_model(src, points=True)
            if derived is None:
                continue
            for word, n in self.sampled(name, derived, 300 if name == 'Repay80' else 60).items():
                kinds[word] = kinds.get(word, 0) + n
            models += 1
        self.assertEqual(models, 9)
        self.assertEqual(set(kinds), {'yes', 'no', 'invalid', 'outside'})
        self.assertGreater(sum(kinds.values()), 3_000)
        self.assertGreater(min(kinds.values()), 50)
        # the state a history is really in at the label is a candidate, and the solver says what the rest of
        # that history says
        src = as_solver_model(V.MODELS['Repay80'].replace('param early : Bool', 'cell early : Bool init in { true, false }'), points=True)
        session = Session(compile_source(src))
        vm = session.vm([]).explore()
        reached = [n for n in vm.nodes if n.step == ('mark', 0)]
        self.assertEqual(len(reached), 8)
        states = []
        for node in reached:
            point = [node.state.t] + [(-1 if v is None else int(v)) for v in node.state.cells]
            for slot in node.state.mem:
                point += [-1, 0, 0] if slot is None else [int(slot[0]), slot[1], slot[2]]
            states.append(point)
        (solve,) = [sv for sv in rust_solves(src, {'p_funded': [[[], states]]}) if sv['name'] == 'p_funded']
        query = session.im['queries'][0]['q']
        for node, point, verdict in zip(reached, states, solve['rows'][0]['verdicts']):
            below = fean.Vm(session.im, [], start=(0, node.state))
            self.assertEqual(verdict[0], WORD[below.query(query)])
            self.assertEqual(verdict[0], WORD[session.judge('p_funded', [], tuple(point))])

    def test_solver_scales_and_stays_exact(self):
        started = time.perf_counter()
        answers = {name: {sv['name']: sv['rows'][0] for sv in rust_solves(src)} for name, src in BIG.items()}
        self.assertLess(time.perf_counter() - started, 5)
        for rows in answers.values():
            for row in rows.values():
                self.assertIsNone(row['error'])
                self.assertTrue(row['complete'])
                self.assertLessEqual(row['runs'], 100)                           # ranges of a billion, a handful of runs
        text = lambda model: {name: row['text'] for name, row in answers[model].items()}   # noqa: E731
        self.assertEqual(text('Capital'), {
            'need': '80000000 at cap=80000000', 'enough': '{80000000..1000000000}', 'spare': 'cap=100000000',
            'short': '79999999 at cap=79999999', 'band': '{80000000..80000999}'})
        self.assertEqual(text('Fees'), {
            'ok': '{[cap >= 20000, cap <= fee - 1], [cap >= fee + 20000]}', 'first': 'cap=20000, fee=3000000',
            'lean': '20000 at cap=20000, fee=5000000', 'rich': '29980000[KRW] at cap=30000000, fee=0',
            'dear': '1000000000 at cap=25000000, fee=1000000000', 'never': 'none'})
        owed = lambda principal: principal * 10500 // 10000                      # noqa: E731
        self.assertEqual(text('Interest'), {
            'cash': f'{owed(123456789)} at principal=123456789, funds={owed(123456789)}',
            'largest': '952381 at principal=952381, funds=1000000',
            'every': '{[funds >= floor(10500*principal / 10000)]}',
            'sharp': 'principal=500000000, funds=525000000'})
        self.assertTrue(owed(952381) <= 1000000 < owed(952382))                  # the largest is the largest
        # the end of 64 bits: where a form does not fit, the candidate is not valid, and the border is where arithmetic puts it
        edge = {sv['name']: sv['rows'][0] for sv in rust_solves(EDGE)}
        self.assertEqual({name: row['text'] for name, row in edge.items()},
                         {'fits': f'{{{MOST - 11}..{MOST}}}', 'last': f'{MOST} at a={MOST}', 'past': 'none'})
        self.assertEqual([(r['verdict'], r['text']) for r in edge['fits']['regions']],
                         [('yes', f'[a <= {MOST}]'), ('invalid', f'[a >= {MOST + 1}]')])
        self.assertTrue(all(row['complete'] and row['error'] is None for row in edge.values()))
        self.assertEqual(MOST * 4 + 3, 2 ** 63 - 1)
        # and at single candidates the Python engine says the same
        kinds = {}
        for name, src in BIG.items():
            for word, n in self.sampled(name, src, 40).items():
                kinds[word] = kinds.get(word, 0) + n
        self.assertEqual(set(kinds), {'yes', 'no', 'outside'})
        self.assertGreater(sum(kinds.values()), 800)
        # a limit on the runs: what is not settled is said to be open, and nothing false is claimed
        for runs in (0, 1, 2, 3):
            with self.subTest(runs=runs):
                session = Session(compile_source(V.MODELS['Capital']))
                asked = {sv['name']: [[[], [[cap] for cap in range(201)]]] for sv in session.im['solves']}
                for sv in rust_solves(V.MODELS['Capital'], asked, runs=runs):
                    row, truth = sv['rows'][0], session.solve(sv['name']).verdicts
                    self.assertLessEqual(row['runs'], runs)
                    for cap, verdict in enumerate(row['verdicts']):
                        self.assertIn(verdict[0], ('undecided', WORD[truth.get((cap,))]))
                    if not row['complete'] and sv['form'] != 'find':
                        self.assertIn(row['text'], ('UNKNOWN', session.solve_text(sv['name'])))
                    if runs == 0:
                        self.assertEqual((row['complete'], row['text']),
                                         (False, '{} unknown {0..200}' if sv['form'] == 'find' else 'UNKNOWN'))
        limited = {sv['name']: sv['rows'][0]['text'] for sv in rust_solves(V.MODELS['Capital'], runs=1)}
        self.assertEqual(limited['enough'], '{81..200} unknown {0..80}')


class TestRustEngine(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fresh(FEAN, 'fean-core', 'fean-cli')

    def test_rust_frontend_matches_python(self):
        texts = [(name, src, None) for name, src in {**E.CORPUS, **V.SOLVE_MODELS}.items()]
        texts += [(f'syntax {n}', src, None) for n, (_, _, src) in enumerate(V.SYNTAX_CASES)]
        texts += [(f'type {n}', V.model(V.HEAD + body), None) for n, (_, body) in enumerate(V.TYPE_CASES)]
        texts += [(f'structure {n}', V.model(V.HEAD + body), None) for n, (_, body) in enumerate(V.STRUCTURE_CASES)]
        texts += [(f'solve {n}', V.model(V.HEAD + body), None) for n, (_, body) in enumerate(V.SOLVE_CASES)]
        texts += [(f'solve ok {n}', V.model(V.HEAD + body), None) for n, body in enumerate(V.SOLVE_OK)]
        accepted = [
            'event e(n : Num) by D { x := floor(x * 2 / n) ?? 0[KRW] }',
            'event e by D { x := floor(x * 3 / 2) }',
            'event e by D { given g = mem(D, note)  c := g }',
            'event e by D { auth true  move L -> D : 1[KRW]  bal(D, KRW) += 1[KRW] }',
            'event e by D, L { move L -> D : 1[KRW] }',
            V.IDLE + ' program { alt { choose D menu m { a => { } } } or { choose D menu m { b => { } } } }',
            V.IDLE + ' program { attempt e { label done } else { label done } }',
        ]
        texts += [(f'accepted {n}', V.model(V.HEAD + body), None) for n, body in enumerate(accepted)]
        texts += [(f'late {rule}', V.model(V.HEAD + body), None) for rule, body in E.ERROR_CASES.items()]
        texts += [('no prelude', 'model M { actor A }', ''), ('prelude needed', V.MODELS['LoanLifecycle'], '')]
        jobs = [{'op': 'check', 'src': src} | ({} if prelude is None else {'prelude': prelude}) for _, src, prelude in texts]
        seen = {'SyntaxErr': 0, 'StaticErr': 0, None: 0}
        for (name, src, prelude), result in zip(texts, batch(jobs)):
            with self.subTest(text=name):
                want = python_verdict(src, prelude)
                self.assertEqual(rust_verdict(result), want)
                seen[want[0] if want else None] += 1
        self.assertEqual(len(texts), 19 + 14 + 14 + 34 + 25 + 28 + 11 + 7 + 7 + 2)
        self.assertEqual(seen, {'SyntaxErr': 14, 'StaticErr': 34 + 25 + 28 + 1, None: 19 + 14 + 11 + 7 + 7 + 1})
        # the rules are named on the same lines as in the specification's own cases
        for (code, line, src), result in zip(V.SYNTAX_CASES, batch([{'op': 'check', 'src': s} for _, _, s in V.SYNTAX_CASES])):
            self.assertEqual((result['code'], result['line']), (code, line), src)

    def test_rust_images_are_identical(self):
        texts = {**E.CORPUS, **V.SOLVE_MODELS}
        names = list(texts)
        results = batch([{'op': 'compile', 'src': texts[name]} for name in names])
        size = 0
        for name, result in zip(names, results):
            with self.subTest(model=name):
                image = compile_source(texts[name])
                self.assertTrue(result['ok'], result)
                self.assertEqual(result['image'], dumps(image))                 # byte for byte
                self.assertEqual(result['listing'], listing(image))
                self.assertEqual(verify(json.loads(result['image'])), image)    # and Python accepts Rust's image
                size += len(result['image'].encode('utf-8'))
        self.assertEqual(len(names), 33)
        self.assertGreater(size, 90_000)                                        # bytes compared
        extra = [V.model(V.HEAD + body) for body in E.UNIT_NAMES]               # unit names, argument by argument
        for src, result in zip(extra, batch([{'op': 'compile', 'src': src} for src in extra])):
            self.assertEqual(result['image'], dumps(compile_source(src)))
        bare = batch([{'op': 'compile', 'src': 'model M { }', 'prelude': ''}])[0]
        self.assertEqual(bare['image'], dumps(compile_source('model M { }', '')))
        with tempfile.TemporaryDirectory() as tmp:                             # and as files, through both command lines
            ours, theirs = Path(tmp) / 'rust.feanc', Path(tmp) / 'python.feanc'
            self.assertEqual(rust('compile', str(EXAMPLE), '-o', str(ours)).returncode, 0)
            self.assertEqual(E.cli('compile', str(EXAMPLE), '-o', str(theirs)).returncode, 0)
            self.assertEqual(ours.read_bytes(), theirs.read_bytes())
            self.assertEqual(rust('dump', str(theirs)).stdout, E.cli('dump', str(ours)).stdout)

    def test_rust_results_and_histories_match(self):
        names = [name for name in E.CORPUS if name not in E.OPEN]              # an unknown has no value of its own
        points = queries = 0
        for name, result in zip(names, batch([{'op': 'histories', 'src': E.CORPUS[name]} for name in names])):
            with self.subTest(model=name):
                want = python_histories(E.CORPUS[name])
                got = rust_histories(result)
                session = Session(compile_source(E.CORPUS[name]))
                wide = [sv['name'] for sv in session.im['solves'] if session.wide(sv['name'])]   # written as regions
                self.assertEqual([r for r in got['results'] if r[0] not in wide],
                                 [r for r in want['results'] if r[0] not in wide])
                self.assertEqual([r[0] for r in got['results']], [r[0] for r in want['results']])
                self.assertEqual(len(got['assignments']), len(want['assignments']))
                for ours, theirs in zip(got['assignments'], want['assignments']):
                    self.assertEqual(ours['pvals'], theirs['pvals'])
                    self.assertEqual(ours['worlds'], theirs['worlds'])
                    self.assertEqual(ours['steps'], theirs['steps'])            # every step of every history
                    self.assertEqual(ours['leaves'], theirs['leaves'])          # (world, failure, time, cells, slots)
                    points += len(theirs['steps'])
                queries += len(want['results'])
        self.assertGreater(points, 4_000)
        self.assertEqual(queries, 32 + 5 + 59)
        for name in V.DOC_MODELS:                                               # the specification's expected results
            with self.subTest(document=name):
                result = batch([{'op': 'run', 'src': V.MODELS[name]}])[0]
                self.assertEqual(dict(result['results']), expectations(V.MODELS[name]))
        # division rounds the same way on both sides of zero
        arith = dict(batch([{'op': 'run', 'src': ARITHMETIC}])[0]['results'])
        self.assertEqual(arith, fean.run_source(ARITHMETIC))
        every = lambda v: '{' + ', '.join(f'n={n}: {v}' for n in range(4)) + '}'       # noqa: E731
        self.assertEqual((arith['down'], arith['up'], arith['plain']), (every(-4), every(-3), every(3)))
        self.assertEqual(arith['other'], '{n=0: -4, n=1: -7, n=2: 99, n=3: 7}')
        self.assertEqual(arith['whole'], '{n=0: 4, n=1: 8, n=2: 99, n=3: -8}')
        self.assertEqual(arith['part'], '{n=0: 99, n=1: 7, n=2: 99, n=3: -7}')    # a remainder: no value
        # models that fail when instantiated or run fail the same way, assignment by assignment
        for rule, body in E.ERROR_CASES.items():
            with self.subTest(rule=rule):
                src = V.model(V.HEAD + body)
                got = rust_histories(batch([{'op': 'histories', 'src': src}])[0])
                self.assertEqual(got, python_histories(src))
                errors = [row['error']['code'] for row in got['assignments'] if 'error' in row]
                if isinstance(got['results'], dict):
                    errors.append(got['results']['code'])
                self.assertIn(rule.strip(), errors)
        for budget in (5, 40, 400):                                             # an exhausted budget is UNKNOWN
            with self.subTest(budget=budget):
                src = V.MODELS['Repay80']
                got = rust_histories(batch([{'op': 'histories', 'src': src, 'budget': budget}])[0])
                self.assertEqual(got, python_histories(src, budget))
        tight = dict(batch([{'op': 'run', 'src': V.MODELS['Repay80'], 'budget': 5}])[0]['results'])
        self.assertEqual(tight['canD'], '{} unknown {false, true}')
        self.assertEqual(tight['worst'], '{early=false: UNKNOWN, early=true: UNKNOWN}')

    def test_rust_rejects_broken_images(self):
        good = compile_source(V.MODELS['Order'])
        cases = E.breakages(good)
        images = []
        for _, breakage in cases:
            image = copy.deepcopy(good)
            breakage(image)
            images.append(image)
        results = batch([{'op': 'verify', 'image': image} for image in images])
        for n, ((code, _), image, result) in enumerate(zip(cases, images, results)):
            with self.subTest(case=n, rule=code):
                with self.assertRaises(fean.ImageErr) as caught:
                    verify(image)
                self.assertEqual(caught.exception.code, code)
                self.assertEqual((result['ok'], result['kind'], result['code']), (False, 'ImageErr', code), result)
        self.assertEqual(len(cases), 24)
        for result in batch([{'op': 'loads', 'text': text} for text in E.BROKEN_TEXTS]):
            self.assertEqual((result['ok'], result['code']), (False, 'I-Format'))
        self.assertEqual(batch([{'op': 'verify', 'image': good}, {'op': 'loads', 'text': dumps(good)}]),
                         [{'ok': True}, {'ok': True}])
        with tempfile.TemporaryDirectory() as tmp:                             # the machine refuses a damaged file too
            broken = Path(tmp) / 'broken.feanc'
            broken.write_text(dumps(images[7]), encoding='utf-8')
            done = rust('run', str(broken))
            self.assertEqual(done.returncode, 1)
            self.assertIn('[I-Ref]', done.stderr)

    def test_rust_has_no_recursion_limit(self):
        head = 'model Long {\n actor A\n asset KRW in 0..10\n time 0..3\n'
        count = 50_000                                                          # statements on one path
        long = (head + ' cell n : Num in 0..100000 init 0 write tick\n event tick by A { n += 1 }\n program {\n'
                + '  run tick\n' * count + ' }\n query total = max run: at(end, n) ?? 0\n}')
        nested = head + ' program { ' + 'at @0 { ' * 3_000 + '} ' * 3_000 + '}\n}'
        wide = head + ' query q = max run: ' + ' + '.join(['1'] * 5_000) + '\n}'
        fits = head + ' query q = max run: ' + ' + '.join(['1'] * 1_500) + '\n}'
        deep = head + ' query q = max run: ' + '(' * 3_000 + '1' + ')' * 3_000 + '\n}'
        ran, too_nested, too_wide, summed, too_deep = batch([
            {'op': 'run', 'src': long, 'budget': 200_000}, {'op': 'check', 'src': nested},
            {'op': 'check', 'src': wide}, {'op': 'run', 'src': fits}, {'op': 'check', 'src': deep}])
        self.assertEqual(ran, {'ok': True, 'results': [['total', str(count)]]})
        self.assertEqual(summed, {'ok': True, 'results': [['q', '1500']]})
        for refused in (too_nested, too_wide, too_deep):                        # refused by rule, not by a crash
            self.assertEqual((refused['ok'], refused['kind'], refused['code']), (False, 'SyntaxErr', 'S-Parse'), refused)
        with self.assertRaises(RecursionError):                                 # the limit the Rust engine removes
            Session(compile_source(long.replace('  run tick\n' * count, '  run tick\n' * 2_000))).results()
        self.assertEqual(batch([{'op': 'run', 'src': long.replace('  run tick\n' * count, '  run tick\n' * 2_000)}])[0],
                         {'ok': True, 'results': [['total', '2000']]})
        over = head + ' query q = max run: 9223372036854775807 + 1\n}'          # 64-bit quantities (DR-31)
        big = batch([{'op': 'run', 'src': over}])[0]
        self.assertEqual((big['ok'], big['kind'], big['code']), (False, 'ModelErr', 'D-Overflow'))
        huge = batch([{'op': 'check', 'src': head + ' query q = max run: 9223372036854775808\n}'}])[0]
        self.assertEqual((huge['ok'], huge['code']), (False, 'S-Lex'))

    def test_rust_cli_matches_python_cli(self):
        moving = V.model(V.HEAD + E.ERROR_CASES['D-Move'])
        typed = V.model(V.HEAD + 'event e by D { require c == 1[KRW] }')
        example = EXAMPLE.read_text(encoding='utf-8')

        def play(run, tmp):
            """One script of commands; returns everything a user could observe."""
            for old in sorted(tmp.rglob('*'), reverse=True):
                old.rmdir() if old.is_dir() else old.unlink()
            files = {'m.fean': example, 'lexical.fean': 'model M {\n actor A $\n}', 'typed.fean': typed,
                     'moving.fean': moving, 'broken.feanc': '{"format": "feanc", "version": 99}',
                     'crlf.fean': 'model M {\r\n actor A\r\n actor A\r\n}', 'latin.fean': None}
            for name, text in files.items():
                if text is None:
                    (tmp / name).write_bytes(b'model M { actor \xff }')
                else:
                    (tmp / name).write_bytes(text.encode('utf-8'))
            (tmp / 'out').mkdir()
            at = lambda name: str(tmp / name)                                   # noqa: E731
            script = [
                (['compile', at('m.fean')], None),
                (['run', at('m.fean')], None),
                (['run', at('m.feanc')], None),
                (['run', at('m.feanc'), '--json'], None),
                (['run', at('m.fean'), '--budget', '5'], None),
                (['run', '--budget=40', at('m.fean')], None),
                (['run', '-'], example),
                (['compile', at('m.fean'), '-o', at('out/named.feanc')], None),
                (['compile', '-', '-o', at('piped.feanc')], example),
                (['compile', '-'], 'model M { }'),
                (['compile', '-', '-o', at('bare.feanc'), '--no-prelude'], 'model M { }'),
                (['run', at('bare.feanc')], None),
                (['run', at('bare.feanc'), '--json'], None),
                (['check', at('m.fean')], None),
                (['check', '-'], example),
                (['dump', at('m.feanc')], None),
                (['compile', at('lexical.fean')], None),
                (['check', at('lexical.fean')], None),
                (['compile', at('typed.fean')], None),
                (['run', at('typed.fean')], None),
                (['compile', at('moving.fean')], None),
                (['run', at('moving.fean')], None),
                (['run', at('moving.feanc')], None),
                (['check', at('crlf.fean')], None),
                (['check', at('latin.fean')], None),
                (['run', at('absent.fean')], None),
                (['compile', at('m.fean'), '-o', at('nowhere/m.feanc')], None),
                (['run', at('broken.feanc')], None),
                (['dump', at('broken.feanc')], None),
                (['dump', at('m.fean')], None),
                (['run', at('m.fean'), '--budget', 'many'], None),
                (['run'], None),
                (['run', at('m.fean'), at('m.fean')], None),
                (['fly', at('m.fean')], None),
                ([], None),
                (['--help'], None),
            ]
            seen = []
            for args, stdin in script:
                done = run(*args, stdin=stdin)
                code = done.returncode
                # what went wrong is named the same way: place and rule, or the system's words
                if code in (1, 2):
                    said = done.stderr.split('] ')[0]
                elif code == 3 and args and args[0] in ('run', 'compile', 'check', 'dump') and len(args) == 2:
                    said = done.stderr.strip() if 'absent' in args[1] or 'latin' in args[1] else ''
                else:
                    said = ''
                out = done.stdout if code == 0 and args[:1] != ['--help'] else ''
                seen.append((' '.join(a.replace(str(tmp), '.') for a in args), code, out, said))
            images = {p.relative_to(tmp).as_posix(): p.read_bytes() for p in sorted(tmp.rglob('*.feanc'))}
            left = sorted(p.name for p in tmp.rglob('*.tmp'))
            return seen, images, left

        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            theirs = play(E.cli, tmp)
            ours = play(rust, tmp)
        for mine, yours in zip(ours[0], theirs[0]):
            with self.subTest(command=yours[0]):
                self.assertEqual(mine, yours)
        self.assertEqual(ours[1], theirs[1])                                    # the same image files
        self.assertEqual((ours[2], theirs[2]), ([], []))                        # and nothing half written
        codes = [code for _, code, _, _ in theirs[0]]
        self.assertEqual({0, 1, 2, 3}, set(codes))                              # every exit code was exercised
        self.assertEqual(sorted(theirs[1]), ['bare.feanc', 'broken.feanc', 'm.feanc', 'moving.feanc',
                                             'out/named.feanc', 'piped.feanc'])

    def test_rust_is_faster_than_python(self):
        sources = list(E.CORPUS.values()) * 5

        def best(action, times=5):
            spent = []
            for _ in range(times):
                start = time.perf_counter()
                action()
                spent.append(time.perf_counter() - start)
            return min(spent)

        batch([{'op': 'check', 'src': sources[0]}])                             # the first start of a new file is slow
        python_compile = best(lambda: [compile_source(src) for src in sources])
        rust_compile = best(lambda: batch([{'op': 'compile', 'src': src} for src in sources]))
        python_run = best(lambda: [Session(compile_source(src)).results() for src in sources])
        rust_run = best(lambda: batch([{'op': 'run', 'src': src} for src in sources]))
        self.assertLess(rust_compile, python_compile)                           # each includes starting the process
        self.assertLess(rust_run, python_run)
        self.assertLess(rust_run * 3, python_run)                               # and not by a hair


def inspect(src, views=False, budget=200_000, prelude=None):
    """The history views of a text as the Rust engine reports them (`fean inspect`)."""
    job = {'op': 'inspect', 'src': src, 'views': views, 'budget': budget}
    if prelude is not None:
        job['prelude'] = prelude
    return batch([job])[0]['report']


def machines(src, budget=200_000):
    """The Python machine of every assignment, explored."""
    image = compile_source(src)
    session = Session(image, budget)
    return image, [session.vm(pv).explore() for pv in session.assignments()]


def state_of(node):
    return ([plain(v) for v in node.state.cells],
            [None if m is None else [plain(m[0]), plain(m[1]), m[2]] for m in node.state.mem])


def target(image, number):
    """The query that is judged per assignment: the query itself, or the one inside a region."""
    q = image['queries'][number]['q']
    return q['inner'] if q['kind'] == 'region' else q


HISTORY_MODELS = ('Repay80', 'LoanLifecycle', 'Market')


class TestHistoryViews(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fresh(FEAN, 'fean-core', 'fean-cli')

    def test_findings_list_contradictions(self):
        # a mandatory step with no settlement, and a guarantee that does not hold
        src = V.MODELS['Support']
        report = inspect(src)
        image, (vm,) = machines(src)
        kinds = [(f['kind'], f['code']) for f in report['findings']]
        self.assertEqual(sorted(kinds), [('failure', 'NO_SETTLEMENT'), ('guarantee', 'forall')])
        failure = next(f for f in report['findings'] if f['kind'] == 'failure')
        run = report['assignments'][0]['runs'][failure['run']]
        self.assertEqual((run['outcome'], run['steps'][failure['step']]['kind']), ('NO_SETTLEMENT', 'fail'))
        self.assertEqual(vm.leaves[failure['run']].fail, 'NO_SETTLEMENT')           # the Python machine agrees
        self.assertEqual(failure['count'], sum(1 for leaf in vm.leaves if leaf.fail))
        self.assertIn('aidAfter', failure['title'])
        self.assertIn('attempt aidAfter', src.split('\n')[failure['line'] - 1])
        self.assertEqual(failure['severity'], 'error')
        broken = next(f for f in report['findings'] if f['kind'] == 'guarantee')
        self.assertIn('allEnd', broken['title'])
        code = target(image, broken['query'])['code']
        self.assertFalse(vm.rv(code, vm.leaves[broken['run']]))                    # its counterexample is one
        self.assertIn('query allEnd', src.split('\n')[broken['line'] - 1])
        # every false guarantee of the documented example, assignment by assignment, with a counterexample
        src = V.MODELS['Repay80']
        report = inspect(src)
        image, vms = machines(src)
        found = {(f['query'], f['assignment']) for f in report['findings']}
        self.assertEqual({f['kind'] for f in report['findings']}, {'guarantee'})
        expected = set()
        for number, entry in enumerate(image['queries']):
            if entry['q']['kind'] in ('forall', 'can'):
                expected |= {(number, a) for a, machine in enumerate(vms) if machine.query(entry['q']) is False}
        self.assertEqual(found, expected)
        self.assertEqual({image['queries'][q]['name'] for q, _ in found}, {'always'})
        for f in report['findings']:
            self.assertFalse(vms[f['assignment']].rv(target(image, f['query'])['code'], vms[f['assignment']].leaves[f['run']]))
        # a text that does not compile: the rule and the line
        for src in ('model M {\n actor A $\n}', V.model(V.HEAD + 'event e by D { require c == 1[KRW] }')):
            report = inspect(src)
            want = python_verdict(src)
            self.assertEqual(report['ok'], False)
            (only,) = report['findings']
            self.assertEqual((only['kind'], only['code'], only['line'], only['severity']), ('compile', want[1], want[2], 'error'))
            self.assertEqual((report['error']['kind'], report['error']['code']), want[:2])
        # texts that compile and fail when run: the rule, for the assignments that fail
        for rule, body in E.ERROR_CASES.items():
            with self.subTest(rule=rule):
                src = V.model(V.HEAD + body)
                report = inspect(src)
                want = python_histories(src)
                failed = [a for a, row in enumerate(want['assignments']) if 'error' in row]
                models = [f for f in report['findings'] if f['kind'] == 'model']
                self.assertTrue(models)
                self.assertEqual({f['code'] for f in models}, {rule.strip()})
                if failed:
                    self.assertEqual(sorted(f['assignment'] for f in models if f['query'] is None), failed)
                    self.assertEqual([s['status'] for s in report['assignments']],
                                     ['error' if a in failed else 'ok' for a in range(len(want['assignments']))])
        # a budget too small to decide is reported as undecided, not as a contradiction
        report = inspect(V.MODELS['Repay80'], budget=5)
        self.assertEqual({(f['kind'], f['severity']) for f in report['findings']}, {('unknown', 'info')})
        self.assertEqual([s['status'] for s in report['assignments']], ['unknown', 'unknown'])
        self.assertEqual({a['name']: a['text'] for a in report['answers']},
                         fean.run_source(V.MODELS['Repay80'], budget=5))

    def test_graph_and_ledger_agree_with_moves(self):
        moves = runs = 0
        for name in HISTORY_MODELS:
            src = E.CORPUS[name]
            report = inspect(src, views=True)
            image, vms = machines(src)
            actors, base = len(image['actors']), len(image['cells'])
            holding = lambda x, who: base + x * actors + who                        # noqa: E731
            for slot, vm in zip(report['assignments'], vms):
                for number, run in enumerate(slot['runs']):
                    with self.subTest(model=name, assignment=slot['label'], run=number):
                        steps = run['steps']
                        start, finish = vm.nodes[steps[0]['node']].state.cells, vm.leaves[number].state.cells
                        total = {}
                        for i, step in enumerate(steps):
                            net = {}
                            for _, src_actor, dst, asset, amount in step['moves']:
                                self.assertGreaterEqual(amount, 0)
                                edge = total.setdefault((src_actor, dst, asset), [0, 0])
                                edge[0] += amount
                                edge[1] += 1
                                net[holding(asset, src_actor)] = net.get(holding(asset, src_actor), 0) - amount
                                net[holding(asset, dst)] = net.get(holding(asset, dst), 0) + amount
                                moves += 1
                            for _, who, asset, amount in step['mints']:
                                key = (None, who, asset) if amount > 0 else (who, None, asset)
                                edge = total.setdefault(key, [0, 0])
                                edge[0] += abs(amount)
                                edge[1] += 1
                                net[holding(asset, who)] = net.get(holding(asset, who), 0) + amount
                            # the graph at this replay position is the sum of the moves so far
                            self.assertEqual({(f, t, x): [amount, count] for f, t, x, amount, count in step['graph']}, total)
                            if i:                                                    # the moves explain the state
                                before = vm.nodes[steps[i - 1]['node']].state.cells
                                after = vm.nodes[step['node']].state.cells
                                for cell in range(base, len(after)):
                                    self.assertEqual(after[cell] - before[cell], net.get(cell, 0), (step['title'], cell))
                        self.assertEqual({(f, t, x): [amount, count] for f, t, x, amount, count in run['graph']}, total)
                        for who, ledger in enumerate(run['ledgers']):
                            balance = {x: start[holding(x, who)] for x in range(len(image['assets']))}
                            for step, when, title, other, asset, inflow, outflow, after in ledger:
                                balance[asset] += inflow - outflow
                                self.assertEqual(after, balance[asset])              # a running balance
                                self.assertEqual(when, steps[step]['time'])
                                self.assertEqual(title, steps[step]['title'])
                                self.assertNotEqual(other, who if inflow != outflow else -1)
                            for x in balance:                                        # that ends at what the actor holds
                                self.assertEqual(balance[x], finish[holding(x, who)])
                            rows = sum(1 for s in steps for m in s['moves'] if who in (m[1], m[2]))
                            rows += sum(1 for s in steps for m in s['mints'] if m[1] == who)
                            self.assertEqual(len(ledger), rows)
                        runs += 1
        self.assertGreater(moves, 300)
        self.assertGreater(runs, 100)
        # the graph up to a step is the graph of the moves so far
        report = inspect(V.MODELS['Repay80'])
        run = report['assignments'][1]['runs'][0]
        self.assertEqual(run['graph'], [[2, 0, 0, 100, 1], [0, 1, 0, 80, 1]])        # I pays D 100, D repays L 80
        self.assertEqual(run['ledgers'][0], [[12, 3, 'award', 2, 0, 100, 0, 100], [15, 4, 'collect', 1, 0, 0, 80, 20]])

    def test_replay_states_match_machine(self):
        points = changed = 0
        for name in HISTORY_MODELS:
            src = E.CORPUS[name]
            report = inspect(src)
            image, vms = machines(src)
            for slot, vm in zip(report['assignments'], vms):
                self.assertEqual((slot['worlds'], slot['points'], len(slot['runs'])),
                                 (len(vm.worlds), len(vm.nodes), len(vm.leaves)))
                index = {id(n): i for i, n in enumerate(vm.nodes)}
                for number, run in enumerate(slot['runs']):
                    with self.subTest(model=name, assignment=slot['label'], run=number):
                        path = vm.path(vm.leaves[number])
                        self.assertEqual([s['node'] for s in run['steps']], [index[id(n)] for n in path])
                        self.assertEqual(run['leaf'], index[id(vm.leaves[number])])
                        self.assertEqual(run['world'], vm.leaves[number].world)
                        self.assertEqual(run['outcome'], vm.leaves[number].fail or 'end')
                        before = None
                        for step, node in zip(run['steps'], path):
                            cells, mem = state_of(node)
                            self.assertEqual((step['kind'], step['time']), (node.step[0], node.state.t))
                            self.assertEqual((step['cells'], step['mem']), (cells, mem))
                            diff = [] if before is None else [[c, old, new] for c, (old, new) in
                                                              enumerate(zip(before, cells)) if old != new]
                            self.assertEqual([c[:3] for c in step['changes']], diff)
                            for cell, _, new, _, shown in step['changes']:           # and written as the model writes it
                                dom = image['cells'][cell]['dom'] if cell < len(image['cells']) else {'kind': 'qty'}
                                written = E.decode(image, dom, new)
                                if dom['kind'] in ('enum', 'actor'):
                                    self.assertEqual(shown, 'empty' if written is None else written)
                                elif dom['kind'] == 'qty' and new is not None:
                                    self.assertEqual(shown.split('[')[0], str(new))
                            changed += len(diff)
                            before = cells
                            points += 1
        self.assertGreater(points, 2_000)
        self.assertGreater(changed, 500)
        last = inspect(V.MODELS['Repay80'])['assignments'][1]['runs'][0]['steps'][15]
        self.assertEqual([[c[0], c[3], c[4]] for c in last['changes']],
                         [[3, '80[KRW]', '0[KRW]'], [4, 'active', 'settled'], [5, '100[KRW]', '20[KRW]'],
                          [6, '0[KRW]', '80[KRW]']])
        self.assertEqual(last['events'], ['collect'])
        elect = inspect(V.MODELS['Repay80'])['assignments'][1]['runs'][0]['steps'][8]
        self.assertEqual((elect['events'], elect['actors'], elect['tells'][0][4]), (['elect(A)'], [0], 'A'))

    def test_knowledge_panel_matches_k(self):
        src = V.MODELS['Repay80']
        report = inspect(src, views=True)
        image, vms = machines(src)
        d, regime = image['actors'].index('D'), [c['name'] for c in image['cells']].index('regime')
        seen = 0
        for slot, vm in zip(report['assignments'], vms):
            groups = [vm.classes(who) for who in range(len(image['actors']))]
            for run in slot['runs']:
                for step in run['steps']:
                    node = vm.nodes[step['node']]
                    for view in step['views']:
                        who = view['actor']
                        members = groups[who][vm.view(node, who)]
                        self.assertEqual(view['worlds'], sorted({m.world for m in members}))
                        self.assertEqual(view['points'], len(members))
                        fixed = [c for c in range(len(node.state.cells))
                                 if all(m.state.cells[c] == node.state.cells[c] for m in members)]
                        self.assertEqual(view['known'], fixed)                       # knowing = the same wherever it could be
                        self.assertEqual(sorted(view['known'] + view['unknown']), list(range(len(node.state.cells))))
                        seen += 1
                    if step['kind'] == 'show':                                       # just before D chooses
                        mine = step['views'][d]
                        self.assertEqual(mine['menu'], [image['menus'].index('claimMenu'),
                                                        [image['actions'].index('A'), image['actions'].index('B')]])
                        if slot['label'] == 'early=true':                            # told at @1: one world left
                            self.assertEqual(len(mine['worlds']), 1)
                            self.assertIn(regime, mine['known'])
                        else:                                                        # not yet told: both worlds possible
                            self.assertEqual(mine['worlds'], [0, 1])
                            self.assertIn(regime, mine['unknown'])
                            self.assertIn([regime, ['boom', 'slump']], mine['possible'])
        self.assertGreater(seen, 400)
        # the panel says what K says: knowsD is the region where D knows the regime at the menu
        knows = next(a for a in report['answers'] if a['name'] == 'knowsD')
        self.assertEqual([row['verdict'] for row in knows['rows']], [False, True])

    def test_explanations_are_witnesses(self):
        witnesses = counters = strategies = values = 0
        for name, src in E.CORPUS.items():
            if name in E.OPEN:                                                   # its cases are checked with the solver
                continue
            with self.subTest(model=name):
                report = inspect(src)
                image, vms = machines(src)
                results = fean.run_image(image)
                self.assertEqual({a['name']: a['text'] for a in report['answers']},
                                 {entry['name']: results[entry['name']] for entry in image['queries']})
                for number, answer in enumerate(report['answers']):
                    q = target(image, number)
                    self.assertEqual((answer['kind'], answer['inner_kind']), (image['queries'][number]['q']['kind'], q['kind']))
                    for row in answer['rows'][:len(vms)]:                        # the assignments; cases of solves follow
                        vm = vms[row['assignment']]
                        verdict = vm.query(q)
                        if isinstance(verdict, bool):
                            self.assertIs(row['verdict'], verdict)
                        else:
                            self.assertEqual(row['verdict'], str(verdict))
                        if row['witness'] is not None and q['kind'] in ('exists', 'forall', 'can'):
                            self.assertTrue(vm.rv(q['code'], vm.leaves[row['witness']]))
                            witnesses += 1
                        if row['counter'] is not None:
                            self.assertFalse(vm.rv(q['code'], vm.leaves[row['counter']]))
                            counters += 1
                        if q['kind'] == 'exists':
                            self.assertEqual(row['witness'] is not None, verdict)
                        if q['kind'] == 'forall':
                            self.assertEqual(row['counter'] is None, verdict)
                        if q['kind'] == 'can':
                            self.assertEqual((row['witness'] is not None, row['counter'] is None), (verdict, verdict))
                        chosen = {(p['actor'], vm.view(vm.nodes[p['node']], p['actor'])): p['action'] for p in row['policy']}
                        who = set(q['who'] if q['kind'] == 'can' else (q.get('policy') or []) if q['kind'] == 'num' else [])
                        follow = [leaf for leaf in vm.leaves
                                  if all(chosen.get((i, view), act) == act for i, view, act in vm.choices(leaf) if i in who)]
                        if q['kind'] == 'can' and verdict:                           # the strategy really guarantees it
                            self.assertTrue(all(vm.rv(q['code'], leaf) for leaf in follow))
                            self.assertIn(vm.leaves[row['witness']], follow)
                            strategies += 1
                        if q['kind'] == 'num' and q['weight'] is None:               # the history that reaches the value
                            leaf = vm.leaves[row['witness']]
                            self.assertEqual(vm.rv(q['code'], leaf), verdict)
                            self.assertIn(leaf, follow)
                            pick = max if q['opt'] == 'max' else min
                            if q['policy'] is not None:                              # and the strategy that secures it
                                self.assertEqual(pick(vm.rv(q['code'], x) for x in follow), verdict)
                                strategies += 1
                            values += 1
        self.assertGreater(witnesses, 80)
        self.assertGreater(counters, 30)
        self.assertGreater(strategies, 60)
        self.assertGreater(values, 8)

    def test_solve_cases_are_inspectable(self):
        cases = 0
        for name in ('Capital', 'Escrow', 'Tiers', 'Gate'):
            src = E.SOLVERS[name]
            report = inspect(src)
            image = compile_source(src)
            session = Session(image)
            assignments = session.assignments()
            self.assertEqual(report['unknowns'], [u['name'] for u in image['unknowns']])
            self.assertEqual([(sv['name'], sv['form']) for sv in report['solves']], [(sv['name'], sv['form']) for sv in image['solves']])
            if image['unknowns']:                                               # no value without a solve: only cases
                self.assertTrue(all(slot['origin'] in ('solution', 'counter') for slot in report['assignments']))
            else:                                                               # the assignments, then the cases
                self.assertEqual([slot['origin'] for slot in report['assignments']][:len(assignments)], ['assignment'] * len(assignments))
            for s, sv in enumerate(report['solves']):
                self.assertEqual(sv['line'], next(i for i, line in enumerate(src.split('\n'), 1)
                                                  if re.match(rf'\s*solve\s+{sv["name"]}\b', line)))
                for row in sv['rows']:
                    pv = assignments[row['assignment']]
                    solved = session.solve(sv['name'], pv)
                    best = None if solved.best is None else list(solved.best)
                    counter = next((list(p) for p, v in solved.verdicts.items() if v is not True), None)
                    self.assertEqual((row['best'], row['counter']), (best, counter))
                    self.assertEqual((row['solution_case'] is None, row['counter_case'] is None), (best is None, counter is None))
                    if best is not None:
                        self.assertEqual(row['best_text'], session._point(solved, tuple(best)))
                    for kind, case, point in (('solution', row['solution_case'], best), ('counter', row['counter_case'], counter)):
                        if case is None:
                            continue
                        with self.subTest(model=name, solve=sv['name'], pvals=pv, case=kind):
                            slot = report['assignments'][case]
                            self.assertEqual((slot['origin'], slot['solve'], slot['point']), (kind, s, point))
                            self.assertIn(sv['name'], slot['label'])
                            self.assertEqual(slot['unknowns'], point[:len(image['unknowns'])])
                            mark = image['solves'][s]['point']
                            self.assertEqual(slot['begun'], None if mark is None else image['marks'][mark])
                            judged = session.judge(sv['name'], pv, tuple(point))
                            self.assertIs(judged is True, kind == 'solution')    # the case is what it is called
                            if judged is INVALID:                                # not valid there: nothing to replay
                                self.assertEqual(slot['status'], 'error')
                                continue
                            vm = session.instance(sv['name'], pv, tuple(point)).explore()   # the Python machine, there
                            self.assertEqual((slot['worlds'], slot['points'], len(slot['runs'])),
                                             (len(vm.worlds), len(vm.nodes), len(vm.leaves)))
                            for run, leaf in zip(slot['runs'], vm.leaves):
                                path = vm.path(leaf)
                                self.assertEqual([(st['kind'], st['time']) for st in run['steps']],
                                                 [(n.step[0], n.state.t) for n in path])
                                self.assertEqual([(st['cells'], st['mem']) for st in run['steps']], [state_of(n) for n in path])
                                self.assertEqual(run['outcome'], leaf.fail or 'end')
                            for number, answer in enumerate(report['answers']):  # plain queries, answered in the case
                                verdict = vm.query(target(image, number))
                                got = answer['rows'][case]['verdict']
                                self.assertEqual(got, verdict if isinstance(verdict, bool) else str(verdict))
                            cases += 1
            # a plain query's own text: one answer per solve, where that solve's solution holds
            results = session.results()
            self.assertEqual({a['name']: a['text'] for a in report['answers']}, {q['name']: results[q['name']] for q in image['queries']})
            wide = [sv['name'] for sv in image['solves'] if session.wide(sv['name'])]
            self.assertEqual({sv['name']: sv['text'] for sv in report['solves'] if sv['name'] not in wide},
                             {name: text for name, text in results.items() if name in [sv['name'] for sv in image['solves']] and name not in wide})
        self.assertGreater(cases, 40)
        # a solve without a solution is listed with the contradictions; a model with unknowns claims nothing else
        escrow = inspect(V.MODELS['Escrow'])
        self.assertEqual([(f['kind'], f['severity'], f['solve']) for f in escrow['findings']], [('unsolved', 'warning', 3)])
        self.assertIn('full', escrow['findings'][0]['title'])
        self.assertIn('bal(S, KRW)=3', escrow['findings'][0]['detail'])
        self.assertEqual(escrow['findings'][0]['assignment'], escrow['solves'][3]['rows'][0]['counter_case'])
        self.assertEqual(inspect(V.MODELS['Capital'])['findings'], [])
        tiers = inspect(V.SOLVE_MODELS['Tiers'])
        self.assertEqual(sorted((f['kind'], tiers['solves'][f['solve']]['name']) for f in tiers['findings']),
                         [('unsolved', 'band')] + [('unsolved', 'nope')] * 4 + [('unsolved', 'slim')] * 3)
        limited = inspect(V.SOLVE_MODELS['Pickup'], budget=3)                    # left open is not the same as none
        self.assertIn(('unknown', 'info'), {(f['kind'], f['severity']) for f in limited['findings'] if f['solve'] is not None})
        self.assertNotIn('unsolved', {f['kind'] for f in limited['findings']})
        # unknowns and no solve: nothing can be run, and that is said
        bare = inspect(V.model('  actor A\n  unknown u : Num in 0..3\n  time 0..0\n  query q = exists run: reach(end)'))
        self.assertEqual([(slot['origin'], slot['status']) for slot in bare['assignments']], [('open', 'unknown')])
        self.assertEqual(bare['answers'][0]['text'], '{}')
        # any candidate can be opened as a case: the model there, with the same views
        src = V.MODELS['Capital']
        picked = batch([{'op': 'case', 'src': src, 'solve': 0, 'row': 0, 'point': [123]}])[0]
        slot = picked['report']['assignments'][picked['case']]
        self.assertEqual((slot['origin'], slot['point'], slot['unknowns'], picked['case']), ('picked', [123], [123], 8))
        vm = Session(compile_source(src)).instance('need', [], (123,)).explore()
        self.assertEqual([st['cells'] for st in slot['runs'][0]['steps']], [state_of(n)[0] for n in vm.path(vm.leaves[0])])
        self.assertEqual([a['rows'][picked['case']]['text'] for a in picked['report']['answers']], ['true', '43[KRW]'])
        self.assertEqual(batch([{'op': 'case', 'src': src, 'solve': 0, 'row': 0, 'point': [1, 2]}])[0]['code'], 'I-Ref')
        # a state outside the declared ranges, or "no slot" spelled another way, is no candidate: refused
        gate = V.SOLVE_MODELS['Gate']
        open_at = lambda point: batch([{'op': 'case', 'src': gate, 'solve': 0, 'row': 0, 'point': point}])[0]    # noqa: E731
        fine = open_at([0, 0, -1, 2, 0, -1, 0, 0])
        self.assertEqual((fine['ok'], fine['report']['assignments'][fine['case']]['status']), (True, 'ok'))
        for point in ([0, 0, -1, 2, 0, -1, 1, 0], [0, 0, -1, 2, 0, -1, 0, 1], [0, 0, -1, 9, 0, -1, 0, 0], [0, 0, -1, 2, 0]):
            refused = open_at(point)
            self.assertEqual((refused['ok'], refused['code']), (False, 'I-Ref'), point)
        # a candidate at which the model is not valid is a case that was not run, and no contradiction of the model
        limits = inspect(V.SOLVE_MODELS['Limits'])
        self.assertIn(('counter', 'error'), {(slot['origin'], slot['status']) for slot in limits['assignments']})
        self.assertEqual([(f['kind'], limits['solves'][f['solve']]['name']) for f in limits['findings']], [('unsolved', 'low')])
        # the command line prints the solves after the queries, as the Python command line does
        with tempfile.TemporaryDirectory() as tmp:
            for name in ('Capital', 'Tiers', 'Escrow'):
                path = Path(tmp) / f'{name}.fean'
                path.write_text(E.SOLVERS[name], encoding='utf-8')
                ours, theirs = rust('run', str(path)), E.cli('run', str(path))
                self.assertEqual((ours.returncode, theirs.returncode), (0, 0))
                wide = 'pre     = '                                             # the one line written as regions
                self.assertEqual([line for line in ours.stdout.splitlines() if not line.startswith(wide)],
                                 [line for line in theirs.stdout.splitlines() if not line.startswith(wide)])
                self.assertEqual(json.loads(rust('run', str(path), '--json').stdout).keys(),
                                 json.loads(E.cli('run', str(path), '--json').stdout).keys())
            self.assertIn('pre     = {[due <= bal(B, KRW), due + bal(S, KRW) <= 3], [paid=true, due >= bal(B, KRW) + 1]}',
                          rust('run', str(Path(tmp) / 'Escrow.fean')).stdout)

    def test_elements_have_source_lines(self):
        def declared(src, word, name):
            import re
            return next(i for i, line in enumerate(src.split('\n'), 1)
                        if re.search(rf'\b{word}\s+(?:\w+\s*,\s*)*{re.escape(name)}\b', line))

        src = V.MODELS['Repay80']
        report = inspect(src)
        lines, text = report['lines'], src.split('\n')
        self.assertEqual(lines['events'], [declared(src, 'event', name) for name in report['events']])
        self.assertEqual(lines['cells'], [declared(src, 'cell', name) for name in report['cells'][:len(lines['cells'])]])
        self.assertEqual(lines['queries'], [declared(src, 'query', a['name']) for a in report['answers']])
        self.assertEqual(lines['actors'], [declared(src, 'actor', name) for name in report['actors']])
        self.assertEqual(lines['assets'], [declared(src, 'asset', name) for name in report['assets']])
        self.assertEqual(lines['params'], [declared(src, 'param', 'early')])
        self.assertEqual(lines['slots'], [declared(src, 'slot', name.split('.')[1]) for name in report['slots']])
        self.assertEqual(lines['menus'], [declared(src, 'menu', 'claimMenu')])
        self.assertEqual(lines['marks'], [declared(src, 'label', 'funded')])
        self.assertEqual([a['line'] for a in report['answers']], lines['queries'])
        checked = 0
        for slot in report['assignments']:
            for run in slot['runs']:
                for step in run['steps']:
                    if step['kind'] in ('init', 'end'):
                        self.assertIsNone(step['line'])
                        continue
                    line = text[step['line'] - 1]
                    word = {'tick': f'at {step["title"]}', 'show': 'choose', 'pick': 'choose', 'mark': f'label {step["title"]}'}
                    self.assertIn(word.get(step['kind'], step['title'].split('(')[0]), line, step)
                    checked += 1
        self.assertGreater(checked, 100)
        # what a contract instance brings is shown at the line that brought it into the model
        for name, instance, called in (('LoanLifecycle', 'instance c =', None), ('CallExample', 'instance call =', 'do call.decide')):
            src = V.MODELS[name]
            report = inspect(src)
            text = src.split('\n')
            home = next(i for i, line in enumerate(text, 1) if instance in line)
            self.assertEqual(set(report['lines']['events']), {home})
            self.assertEqual(set(report['lines']['cells']), {home})
            for slot in report['assignments']:
                for run in slot['runs']:
                    for step in run['steps']:
                        if step['line'] is not None:
                            self.assertLessEqual(step['line'], len(text))
                            self.assertIn('at @', ' '.join(text[9:step['line']]))    # inside the program block
            if called:
                shows = [s for slot in report['assignments'] for run in slot['runs'] for s in run['steps'] if s['kind'] == 'show']
                self.assertTrue(shows)
                self.assertTrue(all(called in text[s['line'] - 1] for s in shows))   # the menu comes from the proc
                self.assertEqual(report['lines']['menus'], [shows[0]['line']])


class TestIde(unittest.TestCase):
    """The IDE is checked through its automatic mode (DR-33): one run of `fean-ide --shots DIR` opens the
    real window, plays the scenario (open the example, run, every view, jump to code, edit and rerun,
    make errors) and leaves the pictures and a report of what it observed. A screen is needed."""

    @classmethod
    def setUpClass(cls):
        fresh(IDE, 'fean-core', 'fean-ide')
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = Path(cls.tmp.name) / 'shots'
        done = subprocess.run([str(IDE), '--shots', str(cls.dir)], capture_output=True, text=True, timeout=300)
        if done.returncode != 0:
            raise RuntimeError(f'fean-ide --shots 가 실패했습니다 (종료 코드 {done.returncode}): {done.stderr}')
        cls.report = json.loads((cls.dir / 'report.json').read_text(encoding='utf-8'))
        cls.example = EXAMPLE.read_text(encoding='utf-8')

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def line(self, text):
        """The line of the example on which a piece of text is written."""
        return next(i for i, line in enumerate(self.example.split('\n'), 1) if text in line)

    def test_ide_editor_reports_errors_in_place(self):
        report = self.report
        self.assertTrue(report['ok'], report['error'])
        self.assertEqual(report['editor'], {'clean': {'text': self.example, 'found': [], 'checked': True},
                                            'lines': len(self.example.splitlines()), 'dirty': False})
        seen = {}
        for kind, rule in (('syntax', 'S-Lex'), ('type', 'T-Eq')):
            noted = report['diagnostics'][kind]
            self.assertNotEqual(noted['text'], self.example)
            self.assertTrue(noted['checked'])
            want = python_verdict(noted['text'])                                   # the Python front end on the same text
            (found,) = noted['found']
            self.assertEqual((found['kind'], found['code'], found['line']), want)
            self.assertEqual(found['code'], rule)
            seen[kind] = found['line']
        self.assertEqual(seen, {'syntax': self.line('actor D, L, I, J'), 'type': self.line('event elect(')})
        for name in ('14-syntax-error.png', '15-type-error.png'):
            self.assertIn(name, report['shots'])
            self.assertGreater((self.dir / name).stat().st_size, 50_000)
        after = report['diagnostics']['after_run']['found']                        # running does not list it twice
        self.assertEqual([(d['code'], d['line']) for d in after], [('T-Eq', seen['type'])])
        self.assertEqual(batch([{'op': 'check', 'src': '', 'prelude': ''}])[0]['ok'], False)

    def test_ide_run_loop_refreshes_results(self):
        report = self.report
        first, second, failed, third = (report[k] for k in ('first_run', 'second_run', 'failed_run', 'third_run'))
        self.assertEqual(first['answers'], expectations(self.example))             # what the specification expects
        self.assertEqual((first['stale'], first['error'], first['runs_done']), (False, None, 1))
        edited = self.example.replace('init I = 100', 'init I = 60')
        self.assertEqual(report['edited_text'], edited)
        self.assertNotEqual(edited, self.example)
        self.assertTrue(report['selection']['stale_after_edit'])                   # edited, not yet run: marked stale
        self.assertEqual(second['answers'], fean.run_source(edited))               # run again: the Python engine agrees
        self.assertEqual((second['stale'], second['runs_done']), (False, 2))
        self.assertEqual((first['answers']['canD'], second['answers']['canD']), ('{true}', '{}'))
        self.assertEqual({f['kind'] for f in second['findings']}, {'failure', 'guarantee'})
        self.assertEqual({f['kind'] for f in first['findings']}, {'guarantee'})
        # a text that does not compile: the problem is shown and the older results stay, marked stale
        broken = report['diagnostics']['type']['text']
        want = python_verdict(broken)
        self.assertEqual((failed['error']['kind'], failed['error']['code'], failed['error']['line']), ('compile', want[1], want[2]))
        self.assertEqual((failed['stale'], failed['has_results'], failed['runs_done']), (True, True, 3))
        self.assertEqual(failed['answers'], second['answers'])
        # and the loop goes on: the original text again, the original results again
        self.assertEqual(third['answers'], first['answers'])
        self.assertEqual((third['stale'], third['error'], third['runs_done']), (False, None, 4))

    def test_ide_renders_every_view(self):
        from PIL import Image
        report = self.report
        self.assertEqual(report['views'], ['개요와 질의', '모순', '역산', '거래 그래프', '시점별 구조와 재생', '주체', '이력', '컴파일 결과'])
        self.assertEqual(report['layouts'], ['편집', '나란히', '결과'])
        self.assertTrue(report['korean_font'])
        names = ['01-editor.png', '19-solver.png'] + [f'{n:02d}-{view}.png' for n, view in enumerate(
            ['overview', 'findings', 'graph', 'timeline', 'actor', 'histories', 'compile'], 3)]
        prints = set()
        for name in names:
            with self.subTest(picture=name):
                self.assertIn(name, report['shots'])
                with Image.open(self.dir / name) as image:
                    self.assertEqual(list(image.size), report['size'])            # all of one size
                    small = image.convert('RGB').resize((360, 225))
                    self.assertGreater(len(small.getcolors(maxcolors=360 * 225)), 100)   # not a blank screen
                    prints.add(small.tobytes())
        self.assertEqual(len(prints), len(names))                                  # and no two alike
        self.assertGreaterEqual(report['size'][0], 1440)
        self.assertEqual(len(report['shots']), 30)
        self.assertEqual(sorted(p.name for p in self.dir.glob('*.png')), sorted(report['shots']))
        # every built-in example goes through every view, and shows what the engine computes for it
        self.assertEqual(sorted(report['examples']), sorted(V.DOC_MODELS))
        for name, seen in report['examples'].items():
            with self.subTest(example=name):
                self.assertEqual(seen['views'], report['views'])
                self.assertTrue(seen['text_is_example'])
                want = expectations(V.MODELS[name])                                 # queries, then solves
                self.assertEqual({**seen['answers'], **dict(seen['solves'])}, want)
                self.assertEqual([name for name, _ in seen['solves']], [k for k in want if k not in seen['answers']])
        self.assertEqual({f['code'] for f in report['examples']['Support']['findings']}, {'NO_SETTLEMENT', 'forall'})
        self.assertEqual(report['actor_view'], {'actor': 'D', 'assignment': 'early=false', 'worlds': [0, 1],
                                                'unknown': ['regime'], 'menu': 2})
        self.assertEqual(report['find'], {'query': 'collect', 'count': self.example.count('collect'),
                                          'line': self.line('event collect')})

    def test_ide_shows_solver_view(self):
        solver = self.report['solver']
        capital, escrow = V.MODELS['Capital'], V.MODELS['Escrow']
        line = lambda src, text: next(i for i, row in enumerate(src.split('\n'), 1) if text in row)   # noqa: E731
        # what the view shows is what the command line prints, and what the specification expects
        want = expectations(capital)
        printed = dict(batch([{'op': 'run', 'src': capital}])[0]['results'])
        self.assertEqual(solver['example'], 'Capital')
        self.assertEqual({sv['name']: sv['text'] for sv in solver['solves']}, {k: printed[k] for k in ('need', 'enough', 'spare', 'short')})
        self.assertEqual({**solver['run']['answers'], **{sv['name']: sv['text'] for sv in solver['solves']}}, want)
        self.assertEqual((solver['run']['findings'], solver['run']['stale'], solver['run']['error']), ([], False, None))
        session = Session(compile_source(capital))
        for sv in solver['solves']:
            with self.subTest(solve=sv['name']):
                (row,) = sv['rows']
                solved = session.solve(sv['name'])
                self.assertEqual((row['best'], row['complete']), (list(solved.best), True))
                self.assertEqual(row['counter'], next(list(p) for p, v in solved.verdicts.items() if v is not True))
                self.assertLessEqual(row['runs'], 6)                                 # 201 candidates, a handful of runs
                self.assertEqual(sv['line'], line(capital, f'solve {sv["name"]}'))
                self.assertEqual({verdict for verdict, _ in row['regions']} - {'outside'}, {'yes', 'no'})
        self.assertEqual(solver['solves'][0]['rows'][0]['regions'], [['no', '[cap <= 79]'], ['yes', '[cap >= 80]']])
        # a model with unknowns is run where its solves have solutions, and where they have none
        self.assertEqual(solver['cases'], [f'{name}의 {what} · cap={cap}' for name, pair in
                                           (('need', (80, 0)), ('enough', (80, 0)), ('spare', (100, 0)), ('short', (79, 80)))
                                           for what, cap in zip(('해', '해가 아닌 값'), pair)])
        # choosing a solution makes it the case every other view shows
        chosen = solver['picked_solution']
        self.assertEqual((chosen['label'], chosen['origin'], chosen['solve'], chosen['point'], chosen['unknowns']),
                         ('short의 해 · cap=79', 'solution', 3, [79], [79]))
        self.assertEqual(chosen['case'], solver['solves'][3]['rows'][0]['solution_case'])
        self.assertEqual(solver['solution_steps'], ['world 0', 'award', '@1', 'collect', 'end'])
        # "이 값으로 보기" opens the value the solver ran in that region: the model is run there
        value = solver['picked_value']
        (sample,) = [r['sample'] for r in inspect(capital)['solves'][0]['rows'][0]['regions'] if r['verdict'] == 'yes']
        self.assertEqual(sample, [100])
        self.assertEqual((value['label'], value['origin'], value['point'], value['unknowns'], value['cases']),
                         ('need의 고른 값 · cap=100', 'picked', sample, sample, 9))
        vm = session.instance('need', [], tuple(sample))
        answers = [vm.query(q['q']) for q in session.im['queries']]
        self.assertEqual(answers, [True, 20])
        self.assertEqual(solver['answers_in_case'], [['repaid', 'true'], ['left', '20[KRW]']])
        # from a solve and from an unknown back into the code
        self.assertEqual(solver['jump_solve'], {'name': 'spare', 'line': line(capital, 'solve spare')})
        self.assertEqual(solver['jump_unknown'], {'name': 'cap', 'line': line(capital, 'unknown cap')})
        # the state at a point, and a solve without a solution among the contradictions
        point = solver['point']
        self.assertEqual(point['example'], 'Escrow')
        self.assertEqual({sv['name']: sv['text'] for sv in point['solves']},
                         {k: v for k, v in expectations(escrow).items() if k != 'settled'})
        (missing,) = point['findings']
        self.assertEqual((missing['kind'], missing['solve'], missing['line']), ('unsolved', 3, line(escrow, 'solve full')))
        self.assertIn('full', missing['title'])
        counter = solver['unsolved']['case']
        self.assertEqual((solver['unsolved']['solve'], counter['origin'], counter['solve'], counter['point'], counter['case']),
                         (3, 'counter', 3, [0, 1, 0, 0, 3], missing['assignment']))
        self.assertEqual(solver['unsolved_steps'], [['init', 'ready 에서 시작'], ['tick', '@1'], ['reject', 'pay'], ['end', 'end']])
        self.assertIs(Session(compile_source(escrow)).judge('full', [], (0, 1, 0, 0, 3)), False)   # and it is none
        # the pictures of these scenes
        from PIL import Image
        prints = set()
        for name in ('19-solver.png', '20-solver-case.png', '21-solver-overview.png', '22-solver-point.png',
                     '23-solver-findings.png', '24-solver-counter.png'):
            self.assertIn(name, self.report['shots'])
            with Image.open(self.dir / name) as image:
                self.assertEqual(list(image.size), self.report['size'])
                prints.add(image.convert('RGB').resize((360, 225)).tobytes())
        self.assertEqual(len(prints), 6)

    def test_ide_jumps_to_code(self):
        jumps = self.report['jumps']
        self.assertEqual(jumps['event'], {'name': 'collect', 'line': self.line('event collect'), 'layout': '나란히'})
        self.assertEqual(jumps['query'], {'name': 'canD', 'line': self.line('query canD')})
        self.assertEqual(jumps['finding']['line'], self.line('query always'))      # a broken guarantee: its query
        self.assertIn('always', jumps['finding']['title'])
        self.assertEqual(jumps['statement'], {'title': 'collect', 'line': self.line('run collect')})
        self.assertEqual(jumps['from_results'], {'line': 2, 'layout': '나란히'})    # the editor comes back into view
        lines = {jumps[k]['line'] for k in ('event', 'query', 'finding', 'statement')}
        self.assertEqual(len(lines), 4)
        self.assertIn('10-jump.png', self.report['shots'])

    def test_ide_keeps_selection_across_runs(self):
        selection = self.report['selection']
        chosen = {'assignment': 'early=true', 'actor': 'L', 'run_key': 'w1/p:D:claimMenu:B', 'step': 12}
        self.assertEqual(selection['before'], chosen)
        self.assertEqual(selection['after'], chosen)                               # the same after another text was run
        self.assertNotEqual(self.report['second_run']['answers'], self.report['first_run']['answers'])
        # the history found again is the same one for the Python engine too: world 1, D picks B
        image = compile_source(self.report['edited_text'])
        vm = Session(image).vm([True]).explore()
        picks = [[(image['actors'][n.step[1]], image['actions'][n.step[3]]) for n in vm.path(leaf) if n.step[0] == 'pick']
                 for leaf in vm.leaves]
        index = next(i for i, leaf in enumerate(vm.leaves) if leaf.world == 1 and picks[i] == [('D', 'B')])
        self.assertEqual(index, 3)                                                 # the history that was selected


MANUAL = ROOT / 'docs' / 'manual'
CHAPTERS = ['1. 시작하기', '2. 화면의 구성', '3. 코드 편집기', '4. 실행과 순환', '5. 결과 보기 여덟 가지',
            '6. 역산: 조건에서 값으로', '7. 코드로 이동', '8. 명령줄', '9. 문제 해결', '10. 기능 목록과 단축키',
            '부록. 규칙 이름 읽는 법']


def squeeze(text):
    """Text without any spacing: how words are compared with what a PDF gives back."""
    import re
    return re.sub(r'\s+', '', text)


def pdf_text(reader):
    return ''.join(page.extract_text() or '' for page in reader.pages)


class TestManual(unittest.TestCase):
    """The manual is built by docs/manual/build.py from manual.html, the IDE's screens and the IDE's feature list."""

    def test_manual_covers_every_feature(self):
        import html
        import re
        from pypdf import PdfReader
        fresh(IDE, 'fean-core', 'fean-ide')
        listed = json.loads(subprocess.run([str(IDE), '--features'], capture_output=True, text=True, check=True).stdout)
        names = [f['name'] for f in listed]
        self.assertEqual((len(names), len(set(names))), (40, 40))
        self.assertEqual({f['group'] for f in listed}, {'편집기', '실행', '화면', '결과', '명령'})
        source = (MANUAL / 'manual.html').read_text(encoding='utf-8')
        plain = html.unescape(re.sub(r'<[^>]+>', '', source))
        printed = squeeze(pdf_text(PdfReader(str(MANUAL / 'FEAN-IDE-설명서.pdf'))))
        table = source[source.index('<!-- features:begin -->'):source.index('<!-- features:end -->')]
        outside = squeeze(html.unescape(re.sub(r'<[^>]+>', '', source.replace(table, ''))))
        for feature in listed:
            with self.subTest(feature=feature['name']):
                self.assertIn(feature['name'], plain)                               # in the source...
                self.assertIn(squeeze(feature['name']), printed)                    # ...in the PDF...
                self.assertIn(squeeze(feature['how']), printed)                     # ...with how to reach it...
                self.assertIn(squeeze(feature['name']), outside)                    # ...and explained outside the table too
        self.assertEqual(table.count('<tr><td>'), len(listed))                      # the table is the IDE's own list
        for word in ['개요와 질의', '모순', '역산', '거래 그래프', '시점별 구조와 재생', '주체', '이력', '컴파일 결과',
                     '편집', '나란히', '결과', '결과 낡음', '결과 최신', '탐색 한도', 'fean-ide --shots', 'fean inspect']:
            self.assertIn(squeeze(word), printed, word)
        for word in ['unknown cap : Num in 0..200', 'solve need = min cap: repaid', 'solve enough = find: repaid',
                     'solve spare = any: repaid', 'solve short = max cap where cap <= 150: not repaid',
                     'find at(ready)', '해 보기', '해가 아닌 값 보기', '해 없음', '미판정']:
            self.assertIn(squeeze(word), printed, word)                             # solving backwards, in its words
        for keys in ('⌘R', '⌘S', '⇧⌘S', '⌘O', '⌘N', '⌘F', '⌘1', '⌘2', '⌘3', 'F5', 'Space'):
            self.assertIn(keys, printed)                                            # every shortcut of the IDE
        for name in V.DOC_MODELS:                                                   # and every built-in example
            self.assertIn(name, printed)

    def test_manual_pdf_is_complete(self):
        import re
        from pypdf import PdfReader
        source_path, pdf_path = MANUAL / 'manual.html', MANUAL / 'FEAN-IDE-설명서.pdf'
        self.assertTrue(pdf_path.exists())
        self.assertGreaterEqual(pdf_path.stat().st_mtime, source_path.stat().st_mtime)     # printed from this source
        source = source_path.read_text(encoding='utf-8')
        figures = re.findall(r'<img src="(img/[^"]+)"', source)
        self.assertGreaterEqual(len(figures), 20)
        self.assertEqual(len(figures), len(set(figures)))
        for figure in figures:
            self.assertTrue((MANUAL / figure).exists(), figure)
        captions = re.findall(r'<figcaption>그림 (\d+)\.', source)
        self.assertEqual([int(n) for n in captions], list(range(1, len(figures) + 1)))      # numbered in order
        reader = PdfReader(str(pdf_path))
        self.assertGreaterEqual(len(reader.pages), 8)
        self.assertEqual(sum(len(page.images) for page in reader.pages), len(figures))      # every figure is in the PDF
        text = pdf_text(reader)
        self.assertEqual([c for c in text if ord(c) < 32 and c not in '\n\r\t'], [])        # real spaces, real text
        printed = squeeze(text)
        self.assertGreater(len(printed), 12_000)
        for title in ['FEAN IDE 설명서'] + CHAPTERS:
            self.assertIn(squeeze(title), printed, title)
        starts = [n for n, page in enumerate(reader.pages)                                  # each chapter opens a page
                  if any(squeeze(page.extract_text() or '')[len(str(n + 1)):].startswith(squeeze(c)) for c in CHAPTERS)]
        self.assertEqual(len(starts), len(CHAPTERS))
        self.assertEqual(starts, sorted(starts))
        box = reader.pages[0].mediabox
        self.assertEqual((round(float(box.width)), round(float(box.height))), (595, 842))   # A4


if __name__ == '__main__':
    unittest.main()
