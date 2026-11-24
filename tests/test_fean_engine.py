"""Checks for the FEAN engine: ahead-of-time compilation to an image, and the machine that runs it.

The engine is judged against two things (DR-25): the expected results written in the specification,
and the reference implementation in tests/fean_ref, down to every leaf of the history tree.
Every test name here is an acceptance criterion of the project's concept map, a design record kept
outside this repository (C-06 to C-08, C-14, C-15).
"""
import copy
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import fean  # noqa: E402
import fean_ref  # noqa: E402
import test_fean_v02 as V  # noqa: E402
from fean import (ImageErr, ModelErr, Session, StaticErr, compile_source, dumps, listing, loads,  # noqa: E402
                  run_image, run_source, verify)
from fean.ir import OPS, TABLES  # noqa: E402
from fean_ref import Machine, Unit, expectations  # noqa: E402
from fean_ref.dynamic import Exploration  # noqa: E402

ENGINE_DOC = ROOT / 'docs' / 'engine.md'
EXAMPLE = ROOT / 'examples' / 'repay80.fean'

MARKET = '''model Market {
  actor A, B, M
  asset KRW  in 0..50 init A = 30, B = 20
  asset unit in 0..3  init M = 3
  enum Mood { calm, wild }
  time 0..3
  param fee : Num in 0..2
  cell mood  : Mood init in { calm, wild }
  cell price : Qty[KRW/unit] in 5..15 init in { 5, 10 }
  cell sold  : Num in 0..3 init 0 write buy observe all
  cell flag  : Bool? init empty write mark1
  slot hint : Mood for A
  slot seen : Num for B

  def cost(n : Num) : Qty[KRW] = price * (n * 1[unit]) + fee * 1[KRW]

  event whisper by M { tell A.hint := mood asof now }
  event forgetIt by M { forget A.hint }
  event buy(who : Actor, n : Num) by who, M {
    require n > 0
    let c = cost(n)
    move who -> M : c
    move M -> who : n * 1[unit]
    sold += n
  }
  event mark1 by M { flag := mood == wild }
  event peek by M { tell B.seen := sold asof now }

  program {
    at @0 { alt { run whisper } or { } }
    at @1 {
      choose A menu first {
        take(n in 1..2) => { attempt buy(A, n) { label gotA } else { label noA } }
        skip => { }
      }
      run peek
    }
    at @2 {
      attempt forgetIt { }
      choose B menu second { one => { attempt buy(B, 1) { } }  none_ => { } }
      run mark1
    }
  }

  weight w = if mood == calm then 3 else 1
  value spentA = 30[KRW] - (at(end, bal(A, KRW)) ?? 30[KRW])
  value keptB  = at(end, bal(B, KRW)) ?? 0[KRW]

  query anySale   = exists run: at(end, sold > 0)
  query hintWorks = exists run: at(menu(first), K(A, mood == wild))
  query forgotten = forall run: at(menu(second), not known(mem(A, hint)))
  query nested    = exists run: at(menu(second), K(B, sold == 2) and not K(A, K(B, mood == wild)))
  query canBuy2   = region fee: can {A}: at(end, bal(A, unit) == 2[unit])
  query bestSpend = max policy {A}: expect w: min run: spentA
  query stock     = min run: at(end, bal(M, unit)) ?? 0[unit]
  query flagged   = exists run: at(end, flag == true) and reach(gotA)
  query bHolds    = region fee: prefer B at menu(second): none_ over one by keptB under w
}
'''

CLEARING = '''model Clearing {
  actor P, Q2, R
  asset KRW in 0..90 init P = 10, R in { 0, 40 }
  time 0..1
  cell x : Qty[KRW] in 0..3 init 0 write settle
  cell y : Qty[KRW] in 0..3 init 0 write settle
  cell total : Qty[KRW] in 0..90 init 0 write tally
  assume bal(P, KRW) + bal(R, KRW) >= 10[KRW]
  invariant x + y <= 4[KRW]

  event p1 by P  { move P -> Q2 : 30[KRW] }
  event p2 by Q2 { move Q2 -> R : 30[KRW] }
  event p3 by R  { move R -> P : 25[KRW] }
  batch ring      = net { p1, p2, p3 }
  batch ringGross = { p1, p2, p3 }
  event settle by P {
    free x  free y
    ensure post(x) + post(y) == 3[KRW]
    ensure post(x) >= post(y)
  }
  event tally by P { total := bal(P, KRW) + bal(Q2, KRW) + bal(R, KRW) }

  program {
    attempt batch ringGross { label g } else { label ng }
    attempt batch ring { label n } else { label nn }
    at @1 { run settle  run tally }
  }

  query ringOk     = forall run: reach(n)
  query grossNever = forall run: reach(ng)
  query conserve   = forall run: at(end, total == bal(P, KRW) + bal(Q2, KRW) + bal(R, KRW))
  query widest     = max run: at(end, x - y) ?? 0[KRW]
  query narrowest  = min run: at(end, x - y) ?? 9[KRW]
}
'''

AUX = {
    'Market': MARKET,
    'Clearing': CLEARING,
    'Flow': V.TestTransitions.FLOW,
    'Overdraft': V.TestTransitions.OVERDRAFT,
    'Fail': V.TestTransitions.FAIL,
    'Quiet': V.model(V.TestEpistemics.QUIET.replace('PUBLIC', '')),
    'QuietPublic': V.model(V.TestEpistemics.QUIET.replace('PUBLIC', 'observe all')),
    'Repay80+': V.with_queries(V.MODELS['Repay80'], '''
      query atMenu = forall run: at(menu(claimMenu), K(D, regime == boom) or K(D, regime == slump))
      query nested = exists run: at(funded, K(L, K(D, regime == boom)) or not K(J, K(D, selected == A)))
      query nobody = can {}: at(end, remain == 0[KRW])
      query both   = can {D, J}: at(end, remain == 0[KRW])
      query best   = expect prior: max run: paid
      query spread = max run: cash - (at(end, bal(L, KRW)) ?? 0[KRW])
    '''),
    'Workout+': V.with_queries(V.MODELS['Workout'], '''
      query maximin = region pn: prefer Senior at menu(workout): liquidate over defer by toSenior
      query average = expect chance: min run: toJunior
      query firmCan = region pn: can {Firm}: at(end, bal(Junior, KRW) >= 25[KRW])
    '''),
}
CORPUS = {**V.MODELS, **AUX}
OPEN = [name for name, src in CORPUS.items() if re.search(r'^\s*unknown\s', src, re.M)]     # run only case by case
SOLVERS = {'Capital': V.MODELS['Capital'], 'Escrow': V.MODELS['Escrow'], **V.SOLVE_MODELS}


def reference(src, budget=200_000):
    return fean_ref.load(src, V.PRELUDE, budget)


def decode(image, dom, v):
    """A machine value written the way the reference implementation holds it."""
    if v is None or isinstance(v, bool):
        return v
    if dom['kind'] == 'enum':
        return image['enums'][dom['enum']]['values'][v]
    return image['actors'][v] if dom['kind'] == 'actor' else v


def vm_leaf(image, leaf):
    st, cells, mem = leaf.state, {}, {}
    for i, c in enumerate(image['cells']):
        cells[c['name']] = decode(image, c['dom'], st.cells[i])
    base, actors = len(image['cells']), image['actors']
    for x, a in enumerate(image['assets']):
        for i, who in enumerate(actors):
            cells[f'bal({who},{a["name"]})'] = st.cells[base + x * len(actors) + i]
    for k, slot in enumerate(image['slots']):
        if st.mem[k] is not None:
            content, asof, recv = st.mem[k]
            mem[(actors[slot['actor']], slot['key'])] = (decode(image, slot['dom'], content), asof, recv)
    return repr((leaf.world, leaf.fail or 'end', st.t, sorted(cells.items()), sorted(mem.items())))


def ref_leaf(leaf):
    st = leaf.state
    cells = {(k if isinstance(k, str) else f'bal({k[1]},{k[2]})'): v for k, v in st.sig.items()}
    return repr((leaf.world, leaf.fail or 'end', st.t, sorted(cells.items()), sorted(st.mu.items())))


def provoke(make):
    """The rule name raised when a model is instantiated for every assignment and its queries are run."""
    try:
        make()
    except fean.FeanError as exc:
        return type(exc).__name__, exc.code
    return None


def engine_all(src, budget=200_000):
    session = Session(compile_source(src), budget)
    for pv in session.assignments():
        session.vm(pv)
    return session.results()


def reference_all(src, budget=200_000):
    family = reference(src, budget)
    for pv in family.assignments():
        family.analysis(pv)
    return family.results()


def breakages(good):
    """Ways to damage a verified image, each with the rule that must refuse it (shared with the Rust checks)."""
    def first(mode, op=None):
        return next(i for i, c in enumerate(good['codes'])
                    if c['mode'] == mode and (op is None or any(x[0] == op for x in c['ins'])))

    def at(im, mode, op):
        code = im['codes'][first(mode, op)]
        return next(x for x in code['ins'] if x[0] == op)

    def swap(im, a, b):
        im[a] = b

    return [
        ('I-Format', lambda im: swap(im, 'format', 'other')),
        ('I-Format', lambda im: swap(im, 'version', 99)),
        ('I-Format', lambda im: im.pop('codes')),
        ('I-Format', lambda im: swap(im, 'time', [3, 1])),
        ('I-Format', lambda im: im['blocks'][0].append(['fly', 1])),
        ('I-Format', lambda im: at(im, 'C', 'PUSH').append(7)),
        ('I-Format', lambda im: at(im, 'C', 'PUSH').__setitem__(1, 'two')),
        ('I-Ref', lambda im: at(im, 'Q', 'LOAD').__setitem__(1, 999)),
        ('I-Ref', lambda im: at(im, 'Q', 'LOAD').__setitem__(1, True)),
        ('I-Ref', lambda im: im['queries'][0]['q'].__setitem__('code', 999)),
        ('I-Ref', lambda im: swap(im, 'entry', 99)),
        ('I-Ref', lambda im: im['blocks'][0][0][3][1].__setitem__(1, im['blocks'][0][0][3][0][1])),
        ('I-Ref', lambda im: im['codes'][first('E')]['ins'].insert(0, ['LOCAL', 5])),
        ('I-Ref', lambda im: im['units'][0]['members'][0]['args'].clear()),
        ('I-Mode', lambda im: im['codes'][first('C')]['ins'].insert(0, ['LOAD', 0])),
        ('I-Mode', lambda im: im['codes'][first('C')]['ins'].insert(0, ['FLY'])),
        ('I-Mode', lambda im: im['events'][0].__setitem__('guard', im['events'][0]['effects'])),
        ('I-Mode', lambda im: im['codes'][first('Q')].__setitem__('mode', 'Z')),
        ('I-Stack', lambda im: im['codes'][first('C')]['ins'].insert(0, ['ADD'])),
        ('I-Stack', lambda im: im['codes'][first('C')]['ins'].append(['PUSH', 1])),
        ('I-Stack', lambda im: im['codes'][first('C')]['ins'].insert(0, ['JMP', 99])),
        ('I-Stack', lambda im: im['codes'][first('C')]['ins'].append(['JMP', 0])),      # a loop
        ('I-Stack', lambda im: im['codes'][first('E')]['ins'].append(['PUSH', 1])),
        ('I-Stack', lambda im: im['codes'][first('Q', 'JF')]['ins'].insert(0, ['PUSH', 0])),
    ]


def solve_breakages(good):
    """The same for the tables of unknowns and solves; `good` is the image of the model Tiers."""
    def first(mode, op):
        return next(i for i, c in enumerate(good['codes']) if c['mode'] == mode and any(x[0] == op for x in c['ins']))

    def at(im, mode, op):
        return next(x for x in im['codes'][first(mode, op)]['ins'] if x[0] == op)

    need, band = 0, 1                                   # solve need = min cap: ...; solve band = find where ...
    region = next(i for i, q in enumerate(good['queries']) if q['q']['kind'] == 'region')
    return [
        ('I-Format', lambda im: im.pop('solves')),
        ('I-Format', lambda im: im['solves'][need].__setitem__('form', 'guess')),
        ('I-Format', lambda im: im['solves'][band].__setitem__('objective', im['solves'][band]['goal'])),
        ('I-Format', lambda im: im['solves'][need].__setitem__('objective', None)),
        ('I-Format', lambda im: im['unknowns'][0].__setitem__('lo', 99)),
        ('I-Format', lambda im: im['unknowns'][0].__setitem__('kind', 'text')),
        ('I-Format', lambda im: im['unknowns'][0].__setitem__('hi', '40')),
        ('I-Ref', lambda im: at(im, 'V', 'QUERY').__setitem__(1, 99)),
        ('I-Ref', lambda im: at(im, 'V', 'QUERY').__setitem__(1, region)),
        ('I-Ref', lambda im: at(im, 'C', 'UNK').__setitem__(1, 7)),
        ('I-Ref', lambda im: im['solves'][need].__setitem__('point', 0)),
        ('I-Ref', lambda im: im['solves'][need].__setitem__('goal', 999)),
        ('I-Mode', lambda im: im['solves'][need].__setitem__('goal', im['solves'][band]['where'])),
        ('I-Mode', lambda im: im['solves'][band].__setitem__('where', im['solves'][band]['goal'])),
        ('I-Mode', lambda im: im['codes'][first('R', 'AT')]['ins'].insert(0, ['QUERY', 0])),
    ]


BROKEN_TEXTS = ('not json', '[1, 2]', '{"format": "feanc"}')

# Models that compile and then fail when instantiated or run, by the rule that stops them
# (shared with the Rust checks; a trailing space only keeps the keys apart).
ERROR_CASES = {
    'D-Init': V.IDLE + ' assume false query q = exists run: reach(end)',
    'D-Init ': V.IDLE + ' invariant x > 0[KRW] query q = exists run: reach(end)',
    'D-Move': 'event e by D { move D -> L : 0[KRW] - 5[KRW] } program { run e } query q = exists run: reach(end)',
    'D-Weight': V.IDLE + ' weight w = 0 query q = expect w: min run: 1',
    'W-Range': V.IDLE + ' cell y : Qty[KRW] in 0..5 init 9',
    'W-Range ': V.IDLE + ' cell y : Qty[KRW] in 5..0 init 0',
    'W-Clock': V.IDLE + ' param t : Time program { at @2 { } at t { } } query q = exists run: reach(end)',
}


# How a unit is named after its arguments: literals and names as written, anything computed as `…`.
UNIT_NAMES = {
    'event e(n : Qty[KRW]) by D { x := n } program { run e(-3[KRW]) }': ['e(-3[KRW])'],
    'event e(n : Qty[KRW]) by D { x := n } program { run e(1[KRW] + 2[KRW]) }': ['e(…)'],
    'event e(n : Qty[KRW]) by D { x := n } program { run e(-(1[KRW] + 2[KRW])) }': ['e(…)'],
    'event e(n : Qty[KRW]) by D { x := n } program { run e(-(if true then 1[KRW] else 2[KRW])) }': ['e(…)'],
    'param p : Num in 0..2 event e(n : Num, k : Color, t : Time) by D { }'
    ' program { run e(-p, blue, @1) run e(p, red, @2) }': ['e(-p, blue, @1)', 'e(p, red, @2)'],
}


def cli(*args, stdin=None):
    return subprocess.run([sys.executable, '-B', '-m', 'fean', *args], cwd=ROOT, input=stdin,
                          capture_output=True, text=True)


class TestImage(unittest.TestCase):
    def test_document_models_compile(self):
        for name in V.DOC_MODELS:
            with self.subTest(model=name):
                src = V.MODELS[name]
                image = compile_source(src)
                self.assertIs(verify(image), image)
                self.assertEqual((image['format'], image['version'], image['language'], image['model']),
                                 ('feanc', 2, '0.3', name))
                self.assertEqual([entry['name'] for entry in image['queries'] + image['solves']],
                                 list(expectations(src)))
                spec = fean.frontend(src)
                self.assertEqual([c['name'] for c in image['cells']], list(spec.cells))
                self.assertEqual([e['name'] for e in image['events']], list(spec.events))
                self.assertEqual(image['actors'], spec.actors)
        self.assertEqual(set(compile_source(V.MODELS['Order'])), set(TABLES) | {'format', 'version'})

    def test_image_is_deterministic_and_round_trips(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name, src in CORPUS.items():
                with self.subTest(model=name):
                    image = compile_source(src)
                    text = dumps(image)
                    self.assertEqual(dumps(compile_source(src)), text)          # same text, same bytes
                    self.assertEqual(loads(text), image)                        # and back
                    self.assertEqual(dumps(loads(text)), text)
                    self.assertTrue(text.endswith('}\n') and text.count('\n') == 1)
                    path = Path(tmp) / f'{len(text)}.feanc'
                    path.write_text(text, encoding='utf-8')
                    self.assertEqual(loads(path.read_text(encoding='utf-8')), image)
                    self.assertEqual(json.dumps(json.loads(text), ensure_ascii=False, sort_keys=True,
                                                separators=(',', ':')) + '\n', text)

    def test_image_is_closed(self):
        for name in ('LoanLifecycle', 'CallExample', 'Repay80', 'Workout'):
            with self.subTest(model=name):
                image = compile_source(V.MODELS[name])
                for code in image['codes']:
                    for instr in code['ins']:
                        for arg in instr[1:]:                                   # every operand is a number...
                            if isinstance(arg, str):                            # ...except the reason of a refusal
                                self.assertIn(instr[0], ('REQUIRE', 'GIVEN'))
                for stmts in image['blocks']:
                    for st in stmts:
                        self.assertTrue(all(not isinstance(x, str) for x in st[1:]), st)
        loan = compile_source(V.MODELS['LoanLifecycle'])
        text = dumps(loan)
        for gone in ('withInterest', '"Loan"', '"Sale"', 'lender', 'principal'):  # def, templates, their parameters
            self.assertNotIn(gone, text)
        remain = next(c for c in loan['cells'] if c['name'] == 'c.remain')
        bound = loan['codes'][remain['hi']]                                      # 0..face, face = withInterest(100, 500)
        self.assertEqual(bound['mode'], 'C')
        self.assertEqual([i[0] for i in bound['ins']],
                         ['PUSH', 'STORE', 'PUSH', 'STORE', 'LOCAL', 'PUSH', 'LOCAL', 'ADD', 'MUL', 'PUSH', 'DIVF'])
        self.assertEqual(bound['locals'], 2)
        call = dumps(compile_source(V.MODELS['CallExample']))
        self.assertNotIn('decide', call)                                         # the proc was expanded
        self.assertIn('call.callMenu', call)
        repay = compile_source(V.MODELS['Repay80'])
        self.assertNotIn('"cash"', dumps(repay))                                 # values are expanded into queries
        self.assertNotIn('"paid"', dumps(repay))
        self.assertEqual(repay['params'], [{'name': 'early', 'kind': 'bool', 'values': [False, True],
                                            'show': ['false', 'true']}])        # parameters stay for the machine
        for body, names in UNIT_NAMES.items():                                   # units are named after their arguments
            self.assertEqual([u['name'] for u in compile_source(V.model(V.HEAD + body))['units']], names)
        collect = next(e for e in repay['events'] if e['name'] == 'collect')
        self.assertEqual([repay['codes'][collect[k]]['mode'] for k in ('guard', 'effects')], ['G', 'E'])
        self.assertEqual([i[0] for i in repay['codes'][collect['effects']]['ins']],
                         ['PUSH', 'PUSH', 'LOCAL', 'FXMOVE', 'LOCAL', 'NEG', 'FXADD',
                          'LOCAL', 'LOAD', 'EQ', 'JF', 'PUSH', 'JMP', 'PUSH', 'FXSET'])

    def test_broken_images_rejected(self):
        good = compile_source(V.MODELS['Order'])
        self.assertIs(verify(good), good)
        cases = breakages(good)
        self.assertEqual(len(cases), 24)
        for n, (code, breakage) in enumerate(cases):
            with self.subTest(case=n, rule=code):
                image = copy.deepcopy(good)
                breakage(image)
                with self.assertRaises(ImageErr) as caught:
                    verify(image)
                self.assertEqual(caught.exception.code, code, str(caught.exception))
                with self.assertRaises(ImageErr):                               # and the machine refuses it too
                    Session(image)
        for text in BROKEN_TEXTS:
            with self.assertRaises(ImageErr) as caught:
                loads(text)
            self.assertEqual(caught.exception.code, 'I-Format')
        self.assertEqual(verify(copy.deepcopy(good)), good)
        # the instruction table and the rule names in the engine document are the ones implemented
        doc = ENGINE_DOC.read_text(encoding='utf-8')
        self.assertEqual(set(re.findall(r'^\| `([A-Z]+)`', doc, re.M)), set(OPS))
        rules = set(re.findall(r'\b(I-[A-Z][a-z]+)\b', doc))
        self.assertEqual(rules, {'I-Format', 'I-Ref', 'I-Mode', 'I-Stack'})
        mine = Path(__file__).read_text(encoding='utf-8')
        self.assertTrue(all(f"'{rule}'" in mine for rule in rules))


class TestMachine(unittest.TestCase):
    def test_vm_matches_document(self):
        total = 0
        for name in V.DOC_MODELS:
            with self.subTest(model=name):
                image = compile_source(V.MODELS[name])
                wide = [sv['name'] for sv in image['solves'] if Session(image).wide(sv['name'])]   # regions, in the document
                expected = {k: v for k, v in expectations(V.MODELS[name]).items() if k not in wide}
                self.assertEqual({k: v for k, v in run_image(image).items() if k not in wide}, expected)
                self.assertEqual({k: v for k, v in run_image(loads(dumps(image))).items() if k not in wide},
                                 expected)                                      # from the file form as well
                total += len(expected)
        self.assertEqual(total, 42)

    def test_vm_matches_reference_queries(self):
        seen = 0
        for name, src in AUX.items():
            with self.subTest(model=name):
                want = reference(src).results()
                self.assertEqual(run_source(src), want)
                seen += len(want)
        self.assertEqual(seen, 59)                                               # queries compared
        market = run_source(MARKET)                                              # and the values are not vacuous
        every = '{fee=0: true, fee=1: true, fee=2: true}'
        self.assertEqual((market['anySale'], market['hintWorks'], market['forgotten']), (every, every, every))
        self.assertEqual(market['stock'], '{fee=0: 0[unit], fee=1: 0[unit], fee=2: 0[unit]}')
        self.assertEqual(market['canBuy2'], '{0..2}')
        self.assertEqual(run_source(CLEARING), {'ringOk': 'true', 'grossNever': 'true', 'conserve': 'true',
                                                 'widest': '3[KRW]', 'narrowest': '1[KRW]'})

    def test_vm_matches_reference_histories(self):
        points = 0
        for name, src in CORPUS.items():
            if name in OPEN:                            # an unknown has no value until a solve gives it one
                continue
            with self.subTest(model=name):
                image = compile_source(src)
                session, family = Session(image), reference(src)
                engine_runs, reference_runs = session.assignments(), family.assignments()
                self.assertEqual(len(engine_runs), len(reference_runs))
                for pv, named in zip(engine_runs, reference_runs):
                    vm, analysis = session.vm(pv).explore(), family.analysis(named)
                    self.assertEqual(len(vm.worlds), len(analysis.m.worlds))
                    self.assertEqual(len(vm.nodes), len(analysis.ex.nodes))
                    self.assertEqual(sorted(vm_leaf(image, leaf) for leaf in vm.leaves),
                                     sorted(ref_leaf(leaf) for leaf in analysis.leaves))
                    self.assertEqual([n.step[0] for n in vm.nodes], [n.step[0] for n in analysis.ex.nodes])
                    points += len(vm.nodes)
        self.assertGreater(points, 4_000)                                        # observation points compared
        for name in ('Flow', 'Overdraft', 'Fail', 'Clearing', 'MutualDebt', 'Support'):   # one commit at a time
            with self.subTest(commits=name):
                image = compile_source(CORPUS[name])
                vm, machine = Session(image).vm(), Machine(reference(CORPUS[name]).spec)
                for number, event in enumerate(image['events']):
                    if event['params']:
                        continue
                    ev = machine.spec.events[event['name']]

                    def judge(step):
                        try:
                            status, info = step()
                        except ModelErr as exc:
                            return 'ERROR', exc.code
                        return status, len(info) if status == 'OK' else info

                    ours = judge(lambda: vm.commit([(number, [])], event['net'], vm.worlds[0], event['name']))
                    theirs = judge(lambda: machine.step(Unit(ev.name, [(ev, [])], ev.net), machine.worlds[0]))
                    self.assertEqual(ours, theirs, event['name'])

    def test_engine_solves_match_reference(self):
        self.assertEqual(OPEN, ['Capital'])
        candidates = cases = 0
        for name, src in SOLVERS.items():
            with self.subTest(model=name):
                image = compile_source(src)
                text = dumps(image)
                self.assertEqual(dumps(compile_source(src)), text)              # the new tables are deterministic too
                self.assertEqual(dumps(loads(text)), text)
                shown = listing(image)
                for u in image['unknowns']:
                    self.assertIn(f'{u["name"]} : {u["kind"]} {u["lo"]}..{u["hi"]}', shown)
                self.assertEqual([line.split(' = ')[0].strip() for line in shown.split('solves\n')[1].split('codes\n')[0]
                                  .splitlines()], [sv['name'] for sv in image['solves']])
                session, family = Session(image), reference(src)
                self.assertEqual(session.results(), family.results())
                for sv in image['solves']:
                    self.assertEqual(session.wide(sv['name']), family.wide(sv['name']))
                    for pv, named in zip(session.assignments(), family.assignments()):
                        ours, theirs = session.solve(sv['name'], pv), family.solve(sv['name'], named)
                        self.assertEqual(ours.verdicts, theirs.verdicts, sv['name'])    # candidate by candidate
                        self.assertEqual((ours.best, ours.value, ours.unknown), (theirs.best, theirs.value, theirs.unknown))
                        candidates += len(ours.verdicts)
                        if ours.best is not None:                               # the case, down to every leaf
                            vm = session.instance(sv['name'], pv, ours.best).explore()
                            analysis = family.instance(sv['name'], named, theirs.best)
                            self.assertEqual(sorted(vm_leaf(image, leaf) for leaf in vm.leaves),
                                             sorted(ref_leaf(leaf) for leaf in analysis.leaves))
                            self.assertEqual([n.step[0] for n in vm.nodes], [n.step[0] for n in analysis.ex.nodes])
                            cases += 1
        self.assertEqual((candidates, cases), (13_253, 65))
        # limits are the same on both sides: too many candidates, and candidates the search limit leaves open
        capital, pickup = V.MODELS['Capital'], V.SOLVE_MODELS['Pickup']
        self.assertEqual(Session(compile_source(capital), limit=200).results(),
                         fean_ref.load(capital, V.PRELUDE, limit=200).results())
        self.assertEqual(Session(compile_source(capital), limit=200).solve_text('need'), 'UNKNOWN')
        for budget in (2, 3, 6, 9):
            with self.subTest(budget=budget):
                ours, theirs = Session(compile_source(pickup), budget), fean_ref.load(pickup, V.PRELUDE, budget)
                self.assertEqual(ours.results(), theirs.results())
                self.assertEqual(ours.points('reach3'), theirs.solve('reach3').verdicts)
        self.assertEqual(Session(compile_source(pickup), 3).solve_text('dull'),
                         'now=@0, step=3, bal(T, KRW)=2, bal(U, KRW)=0')

        # a case begun at a point is exactly the rest of the histories that pass that point (both implementations)
        def below(node, state):
            out, stack = [], list(reversed(node.children))
            while stack:
                n = stack.pop()
                out.append((n.step, bool(n.fail), bool(n.end), state(n.state)))
                stack.extend(reversed(n.children))
            return out

        resumed = 0
        for name, src, unknowns in (('Repay80', V.MODELS['Repay80'], [()]), ('Escrow', V.MODELS['Escrow'], [()]),
                                    ('Clearing', CLEARING, [()]), ('Pickup', pickup, [()]),
                                    ('Gate', V.SOLVE_MODELS['Gate'], [(0,), (1,), (2,)])):
            with self.subTest(resume=name):
                image = compile_source(src)
                session, family = Session(image), reference(src)
                for pv, named in zip(session.assignments(), family.assignments()):
                    for uv in unknowns:
                        whole = fean.Vm(image, pv, uvals=uv).explore()
                        for node in whole.nodes:
                            if node.step[0] == 'mark':
                                part = fean.Vm(image, pv, uvals=uv, start=(node.step[1], node.state)).explore()
                                state = lambda s: (s.t, tuple(s.cells), tuple(s.mem))     # noqa: E731
                                self.assertEqual(below(part.roots[0], state), below(node, state))
                                self.assertIs(part.point(part.leaves[0], 3, node.step[1]), part.roots[0])
                                resumed += 1
                        vals = {**named, **dict(zip(family.spec.unknowns, uv))}
                        whole = Exploration(Machine(family.spec, vals))
                        for node in whole.nodes:
                            if node.step[0] == 'mark':
                                part = Exploration(Machine(family.spec, vals, start=(node.step[1], node.state)))
                                state = lambda s: (s.t, sorted(map(repr, s.sig.items())), sorted(s.mu.items()))  # noqa: E731
                                self.assertEqual(below(part.roots[0], state), below(node, state))
                                resumed += 1
        self.assertEqual(resumed, 40)

        # a damaged table of unknowns or solves is refused by rule
        good = compile_source(V.SOLVE_MODELS['Tiers'])
        broken = solve_breakages(good)
        self.assertEqual(len(broken), 15)
        for n, (code, breakage) in enumerate(broken):
            with self.subTest(case=n, rule=code):
                image = copy.deepcopy(good)
                breakage(image)
                with self.assertRaises(ImageErr) as caught:
                    verify(image)
                self.assertEqual(caught.exception.code, code, str(caught.exception))
        old = copy.deepcopy(compile_source(V.MODELS['Order']))                   # format 1 had neither table
        old['version'] = 1
        del old['unknowns'], old['solves']
        with self.assertRaises(ImageErr) as caught:
            verify(old)
        self.assertEqual(caught.exception.code, 'I-Format')
        # the command line prints solves after queries, from the text and from the image
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'capital.fean'
            path.write_text(capital, encoding='utf-8')
            done = cli('run', str(path))
            self.assertEqual((done.returncode, done.stderr), (0, ''))
            lines = [line.split(' = ', 1) for line in done.stdout.splitlines()]
            self.assertEqual({name.strip(): value for name, value in lines}, expectations(capital))
            self.assertEqual(cli('compile', str(path)).returncode, 0)
            self.assertEqual(cli('run', str(path.with_suffix('.feanc'))).stdout, done.stdout)
            dumped = cli('dump', str(path.with_suffix('.feanc'))).stdout
            for expected in ('format feanc 2   language 0.3', 'unknown  0 cap : num 0..200', 'solves',
                             'need = min c', 'short = max c', 'UNK 0', '; cap', 'QUERY 0', '; repaid'):
                self.assertIn(expected, dumped)

    def test_vm_reports_errors_and_limits(self):
        cases = ERROR_CASES
        for rule, body in cases.items():
            with self.subTest(rule=rule):
                src = V.model(V.HEAD + body)
                ours = provoke(lambda: engine_all(src))
                self.assertEqual(ours, provoke(lambda: reference_all(src)))
                self.assertEqual(ours[1], rule.strip())
        self.assertEqual(provoke(lambda: engine_all(V.model(V.HEAD + cases['D-Move']))), ('ModelErr', 'D-Move'))
        self.assertEqual(provoke(lambda: engine_all(V.model(V.HEAD + cases['W-Range']))), ('StaticErr', 'W-Range'))
        for budget in (5, 40, 400):                                              # an exhausted budget is UNKNOWN
            with self.subTest(budget=budget):
                ours = run_source(V.MODELS['Repay80'], budget=budget)
                self.assertEqual(ours, reference(V.MODELS['Repay80'], budget).results())
        self.assertEqual(run_source(V.MODELS['Repay80'], budget=5)['canD'], '{} unknown {false, true}')
        self.assertEqual(run_source(V.MODELS['Repay80'], budget=5)['worst'], '{early=false: UNKNOWN, early=true: UNKNOWN}')
        with self.assertRaises(StaticErr):                                       # compile errors stop before any image
            compile_source(V.model(V.HEAD + 'event e by D { require c == 1[KRW] }'))


class TestEngine(unittest.TestCase):
    def test_cli_compile(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / 'm.fean'
            src.write_text(EXAMPLE.read_text(encoding='utf-8'), encoding='utf-8')
            done = cli('compile', str(src))
            self.assertEqual((done.returncode, done.stderr), (0, ''))
            image = loads((Path(tmp) / 'm.feanc').read_text(encoding='utf-8'))
            self.assertEqual(image, compile_source(EXAMPLE.read_text(encoding='utf-8')))
            self.assertIn('m.feanc', done.stdout)
            self.assertIn('질의 10', done.stdout)
            other = Path(tmp) / 'out' / 'named.feanc'
            other.parent.mkdir()
            self.assertEqual(cli('compile', str(src), '-o', str(other)).returncode, 0)
            self.assertEqual(loads(other.read_text(encoding='utf-8')), image)
            piped = Path(tmp) / 'piped.feanc'
            done = cli('compile', '-', '-o', str(piped), stdin=src.read_text(encoding='utf-8'))
            self.assertEqual(done.returncode, 0)
            self.assertEqual(piped.read_text(encoding='utf-8'), (Path(tmp) / 'm.feanc').read_text(encoding='utf-8'))
            self.assertEqual(cli('compile', '-', stdin='model M { }').returncode, 3)      # nowhere to write
            bare = cli('compile', '-', '-o', str(Path(tmp) / 'bare.feanc'), '--no-prelude', stdin='model M { }')
            self.assertEqual(bare.returncode, 0)
            self.assertEqual(loads((Path(tmp) / 'bare.feanc').read_text(encoding='utf-8'))['enums'], [])
            self.assertEqual(sorted(p.name for p in Path(tmp).rglob('*.tmp')), [])

    def test_cli_run_source_and_image_agree(self):
        expected = expectations(V.MODELS['Repay80'])
        with tempfile.TemporaryDirectory() as tmp:
            image = Path(tmp) / 'repay80.feanc'
            self.assertEqual(cli('compile', str(EXAMPLE), '-o', str(image)).returncode, 0)
            from_text, from_image = cli('run', str(EXAMPLE)), cli('run', str(image))
            self.assertEqual((from_text.returncode, from_image.returncode), (0, 0))
            self.assertEqual(from_text.stdout, from_image.stdout)
            lines = [line.split(' = ', 1) for line in from_image.stdout.splitlines()]
            self.assertEqual({name.strip(): value for name, value in lines}, expected)
            as_json = cli('run', str(image), '--json')
            self.assertEqual(json.loads(as_json.stdout), expected)
            piped = cli('run', '-', stdin=EXAMPLE.read_text(encoding='utf-8'))
            self.assertEqual(piped.stdout, from_text.stdout)
            limited = cli('run', str(image), '--budget', '5')
            self.assertEqual(limited.returncode, 0)
            self.assertIn('UNKNOWN', limited.stdout)
            checked = cli('check', str(EXAMPLE))
            self.assertEqual(checked.returncode, 0)
            self.assertIn('검사 통과', checked.stdout)

    def test_cli_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            def source(name, text):
                path = Path(tmp) / name
                path.write_text(text, encoding='utf-8')
                return path

            lexical = source('lexical.fean', 'model M {\n actor A $\n}')
            done = cli('compile', str(lexical))
            self.assertEqual(done.returncode, 1)
            self.assertTrue(done.stderr.startswith(f'{lexical}:2: [S-Lex] '), done.stderr)
            typed = source('typed.fean', V.model(V.HEAD + 'event e by D { require c == 1[KRW] }'))
            done = cli('compile', str(typed))
            self.assertEqual(done.returncode, 1)
            self.assertRegex(done.stderr, rf'^{re.escape(str(typed))}:\d+: \[T-Eq\] ')
            self.assertEqual(cli('check', str(typed)).returncode, 1)
            self.assertEqual(sorted(p.name for p in Path(tmp).glob('*.feanc')), [])          # no image left behind
            moving = source('moving.fean', V.model(
                V.HEAD + 'event e by D { move D -> L : 0[KRW] - 5[KRW] } program { run e }'
                         ' query q = exists run: reach(end)'))
            self.assertEqual(cli('compile', str(moving)).returncode, 0)                      # it compiles...
            done = cli('run', str(moving))
            self.assertEqual(done.returncode, 2)                                             # ...and fails when run
            self.assertEqual(done.stderr.split(': ', 1)[1].split(' ', 1)[0], '[D-Move]')
            self.assertEqual(cli('run', str(Path(tmp) / 'moving.feanc')).returncode, 2)
            self.assertEqual(cli('run', str(Path(tmp) / 'absent.fean')).returncode, 3)
            broken = source('broken.feanc', '{"format": "feanc", "version": 99}')
            done = cli('run', str(broken))
            self.assertEqual(done.returncode, 1)
            self.assertIn('[I-Format]', done.stderr)
            self.assertEqual(cli('dump', str(broken)).returncode, 1)
            good = source('good.fean', 'model M { actor A }')                               # an error keeps the old image
            self.assertEqual(cli('compile', str(good)).returncode, 0)
            before = (Path(tmp) / 'good.feanc').read_text(encoding='utf-8')
            good.write_text('model M { actor A actor A }', encoding='utf-8')
            done = cli('compile', str(good))
            self.assertEqual(done.returncode, 1)
            self.assertIn('[W-Name]', done.stderr)
            self.assertEqual((Path(tmp) / 'good.feanc').read_text(encoding='utf-8'), before)
            self.assertEqual(cli('fly', str(good)).returncode, 3)
            self.assertEqual(cli().returncode, 3)
            self.assertEqual(cli('--help').returncode, 0)

    def test_cli_dump(self):
        with tempfile.TemporaryDirectory() as tmp:
            image = Path(tmp) / 'repay80.feanc'
            cli('compile', str(EXAMPLE), '-o', str(image))
            done = cli('dump', str(image))
            self.assertEqual((done.returncode, done.stderr), (0, ''))
            self.assertEqual(done.stdout, listing(loads(image.read_text(encoding='utf-8'))))
            for expected in ('image Repay80   format feanc 2   language 0.3', 'param    0 early : bool {false, true}',
                             'cells', 'remain', 'holdings of KRW', 'slot     0 D.report', 'events', 'collect',
                             'units', 'elect(A)', 'program  entry b0', 'choose D menu claimMenu', 'queries',
                             'canD = region early: can {D}', 'worst = max policy {D} min run', '[KRW]', 'codes',
                             'FXMOVE 0', '; bal(D,KRW)', 'REQUIRE "require in collect"', '; menu claimMenu'):
                self.assertIn(expected, done.stdout)

    def test_engine_sources_match_document(self):
        self.assertEqual(fean.STD_PRELUDE, V.PRELUDE)
        self.assertEqual(EXAMPLE.read_text(encoding='utf-8'), V.MODELS['Repay80'])
        examples = {p.name: p.read_text(encoding='utf-8') for p in (ROOT / 'examples').glob('*.fean')
                    if not p.name.startswith('.')}
        self.assertEqual(examples, {f'{name.lower()}.fean': V.MODELS[name] for name in V.DOC_MODELS})
        self.assertIs(fean_ref.parse_model, fean.parse_model)                    # one front end (DR-23)
        self.assertIs(fean_ref.build, fean.build)
        self.assertIs(fean_ref.StaticErr, fean.StaticErr)
        for path in (ROOT / 'fean').glob('*.py'):
            if not path.name.startswith('.'):
                text = path.read_text(encoding='utf-8')
                self.assertNotIn('fean_ref', text, path.name)                    # the engine never reaches into tests/
                self.assertNotRegex(text, r'^\s*(from|import)\s+tests\b', path.name)


if __name__ == '__main__':
    unittest.main()
