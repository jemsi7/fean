"""Checks for FEAN: the specification's own code blocks, run through the reference implementation.

Every test name here is an acceptance criterion of the project's concept map, a design record kept
outside this repository.
"""
import re
import unittest
from fractions import Fraction
from itertools import product
from pathlib import Path

from fean_ref import (INVALID, UNKNOWN, Analysis, FeanError, Machine, ModelErr, StaticErr, SyntaxErr, Unit,
                      doc_blocks, expectations, load, parse_model, tokenize)
from fean_ref.dynamic import State
from fean_ref.syntax import KEYWORDS, Parser

ROOT = Path(__file__).resolve().parents[1]
DOC = ROOT / 'docs' / 'financial-language.md'
PRELUDE = doc_blocks(DOC, 'fean-prelude')[0]
MODELS = {re.match(r'\s*model\s+(\w+)', b).group(1): b for b in doc_blocks(DOC, 'fean')}
DOC_MODELS = ['Order', 'MutualDebt', 'Support', 'LoanLifecycle', 'CallExample', 'Repay80', 'Straddle', 'Workout',
              'Capital', 'Escrow']

HEAD = '''
  actor D, L
  asset KRW in 0..100 init D = 50
  asset share in 0..10
  enum Color { red, blue }
  time 0..3
  cell x : Qty[KRW] in 0..100 init 0 write e
  cell c : Color init red write e
  slot note : Color for D
'''
IDLE = 'event e by D { }'

# Rejection cases, shared with the Rust engine checks (tests/test_fean_rust.py).
SYNTAX_CASES = [
        ('S-Lex', 1, 'model M { actor A $ }'),
        ('S-Parse', 3, 'model M {\n actor A\n'),
        ('S-Parse', 2, 'model M {\n actor time\n}'),
        ('S-Parse', 3, 'model M {\n actor A\n invariant 1 < 2 < 3\n}'),
        ('S-Parse', 3, 'model M {\n actor A\n event e { }\n}'),
        ('S-Parse', 2, 'model M {\n program { run }\n}'),
        ('S-Parse', 4, 'model M {\n actor A\n}\nactor B'),
        ('S-Parse', 3, 'model M {\n query q = exists run: reach(end)\n solve s = : q\n}'),
        ('S-Parse', 3, 'model M {\n query q = exists run: reach(end)\n solve s = find q\n}'),
        ('S-Parse', 2, 'model M {\n solve s = find at here: q\n}'),
        ('S-Parse', 2, 'model M {\n solve s = find at(end): q\n}'),
        ('S-Parse', 2, 'model M {\n unknown where : Num in 0..1\n}'),
        ('S-Parse', 3, 'model M {\n actor A\n solve s = min: q\n}'),
        ('S-Parse', 2, 'model M {\n solve s = find where: q\n}'),
]

TYPE_CASES = [
        ('T-Arith', 'event e by D { require bal(D, KRW) + 1[share] > 0[KRW] }'),
        ('T-Cmp', 'event e by D { require recv(D, note) <= @2 }'),
        ('T-Layer', 'event e by D { require K(D, x == 0[KRW]) }'),
        ('T-Layer', 'event e by D { require post(x) == 0[KRW] }'),
        ('T-Layer', IDLE + ' value v = x'),
        ('T-Layer', IDLE + ' cell y : Time init now'),
        ('T-Eq', 'event e by D { require c == 1[KRW] }'),
        ('E-Set', 'event e by D { c := mem(D, note) }'),
        ('E-Set', 'event e(n : Num) by D { x := floor(x * 2 / n) }'),
        ('E-Move', 'event e by D { move D -> L : 3 }'),
        ('E-Move', 'event e by D { move D -> L : 1[KRW] * 1[share] }'),
        ('T-Name', 'event e by D { require zzz == 0[KRW] }'),
        ('P-Args', 'event e(n : Qty[KRW]) by D { x := n } program { run e(red) }'),
        ('P-Args', 'event e(n : Qty[KRW]) by D { x := n } program { run e }'),
        ('Q-Bool', IDLE + ' query q = exists run: at(end, x)'),
        ('T-Bool', IDLE + ' invariant 1'),
        ('T-If', 'event e by D { x := if true then 1[KRW] else red }'),
        ('T-Opt', 'event e by D { x := mem(D, note) ?? 0[KRW] }'),
        ('T-Mem', 'event e by D { require known(mem(L, note)) }'),
        ('T-Given', 'event e by D { given g = x }'),
        ('T-Call', 'def f(a : Num) : Num = a + 1 event e by D { require f(red) == 1 }'),
        ('T-Bal', 'event e by D { require bal(D, gold) > 0[KRW] }'),
        ('T-Lit', 'event e by D { require x == 1[USD] }'),
        ('T-Lit', 'event e by D { require now == @9 }'),
        ('E-Delta', 'event e by D { c += 1[KRW] }'),
        ('E-Free', 'event e by D { free bal(D, KRW) }'),
        ('E-Tell', 'event e by D { tell D.note := 1[KRW] asof @0 }'),
        ('Q-Num', IDLE + ' query q = max run: at(end, c) ?? red'),
        ('Q-Num', IDLE + ' query q = max run: floor(7 / (1 - 1))'),
        ('Q-Point', IDLE + ' query q = exists run: reach(nowhere)'),
        ('Q-Region', IDLE + ' query q = region zzz: exists run: reach(end)'),
        ('Q-Prefer', IDLE + ' value v = 1 query q = prefer D at end: a over b by v'),
        ('P-Unit', IDLE + ' program { run nothing }'),
        ('P-Choose', IDLE + ' program { choose D menu m { a => { } a => { } } }'),
]

STRUCTURE_CASES = [
        ('W-Writer', IDLE + ' event f by D { x := 1[KRW] }'),
        ('W-Write', 'event e by D { x := 1[KRW]  x := 2[KRW] }'),
        ('W-Write', 'event e by D { x := 1[KRW]  x += 2[KRW] }'),
        ('W-Write', IDLE + ''' cell z : Qty[KRW] in 0..9 init 0 write g, h
             event g by D { z := 1[KRW] } event h by D { z := 2[KRW] } batch b = { g, h }'''),
        ('W-Write', 'event e by D { tell D.note := red asof @0  forget D.note }'),
        ('W-Debit', 'event e by D { move L -> D : 1[KRW] }'),
        ('W-Debit', 'event e by D { bal(D, KRW) += 1[KRW] }'),
        ('W-Batch', IDLE + ' program { run batch nope }'),
        ('W-Batch', IDLE + ' batch b = { e, e }'),
        ('W-Clock', IDLE + ' program { at @2 { } at @1 { } }'),
        ('W-Clock', IDLE + ' program { alt { at @3 { } } or { } at @2 { } }'),
        ('W-Clock', IDLE + ' program { at 3 { } }'),
        ('W-Menu', IDLE + ' program { choose D menu m { a => { } } choose D menu m { a => { } } }'),
        ('W-Menu', IDLE + ' program { label here  alt { label here } or { } }'),
        ('W-Name', IDLE + ' actor D'),
        ('W-Name', 'event e by D { let x = 1 }'),
        ('W-Name', IDLE + ' enum Other { red }'),
        ('W-Name', IDLE + ' contract T(n : Num) { event g by D { let n = 1 } } instance t = T(2)'),
        ('W-Rec', IDLE + ' proc p() { do p() } program { do p() }'),
        ('W-Rec', IDLE + ' def f(a : Num) : Num = f(a)'),
        ('W-Rec', IDLE + ' contract T() { instance inner = T() } instance t = T()'),
        ('W-Range', IDLE + ' cell y : Qty[KRW] init 0'),
        ('W-Range', IDLE + ' cell y : Color in 0..1 init red'),
        ('W-Static', 'event e(to : Actor) by D { tell to.note := red asof @0 }'),
        ('W-Static', IDLE + ' contract T(who : Actor) { event g by who { tell who.note := red asof @0 } }'
                            ' instance t = T(if true then D else L)'),
]


# Unknowns and solves (section 5.4). Each body follows HEAD; shared with the Rust engine checks.
SOLVE_HEAD = '''
  unknown u : Num in 0..3
  unknown m : Color
  param p : Num in 0..1
  event e by D { x := u * 1[KRW] }
  program { run e  label here  alt { label twice } or { label twice } }
  weight w = 1
  query ok   = forall run: at(end, x >= 0[KRW])
  query size = max run: at(end, x) ?? 0[KRW]
  query mean = expect w: min run: 1
  query zone = region p: exists run: reach(end)
'''

SOLVE_CASES = [
        ('Q-Solve', SOLVE_HEAD + 'solve s = find: size'),                           # the goal is not Bool
        ('Q-Solve', SOLVE_HEAD + 'solve s = find where u: ok'),                     # nor is the filter
        ('Q-Solve', SOLVE_HEAD + 'solve s = min ok: ok'),                           # what is minimised is a quantity
        ('Q-Solve', SOLVE_HEAD + 'solve s = max m: ok'),
        ('Q-Solve', SOLVE_HEAD + 'solve s = find: zone'),                           # a region has no single value
        ('Q-Solve', SOLVE_HEAD + 'solve s = find: mean >= 1'),                      # nor has an expected value
        ('Q-Solve', SOLVE_HEAD + 'solve s = find at(twice): ok'),                   # a point is one place
        ('Q-Solve', IDLE + ' query q = exists run: reach(end)  solve s = find: q'),  # nothing to solve for
        ('Q-Point', SOLVE_HEAD + 'solve s = find at(nowhere): ok'),
        ('T-Layer', SOLVE_HEAD + 'solve s = find: x == 0[KRW]'),                    # no state without a point
        ('T-Layer', SOLVE_HEAD + 'solve s = find where now == @0: ok'),
        ('T-Layer', SOLVE_HEAD + 'solve s = find at(here): at(end, x == 0[KRW])'),  # a goal reads verdicts, not histories
        ('T-Layer', SOLVE_HEAD + 'solve s = find at(here): K(D, x == 0[KRW])'),
        ('T-Name', SOLVE_HEAD + 'solve s = any: missing'),
        ('T-Name', SOLVE_HEAD + 'solve s = any where ok: ok'),                      # a filter runs before any verdict
        ('T-Name', SOLVE_HEAD + 'solve s = any at(here) where ok: ok'),
        ('T-Bool', SOLVE_HEAD + 'solve s = find: ok and u'),
        ('T-Arith', SOLVE_HEAD + 'solve s = max u + 1[KRW]: ok'),
        ('W-Name', SOLVE_HEAD + 'solve ok = find: ok'),                             # queries and solves share their names
        ('W-Name', SOLVE_HEAD + 'solve s = find: ok  solve s = any: ok'),
        ('W-Name', SOLVE_HEAD + 'query s2 = exists run: reach(end)  solve s2 = find: ok'),
        ('W-Name', SOLVE_HEAD + 'unknown x : Num in 0..1'),
        ('W-Name', SOLVE_HEAD + 'unknown p : Num in 0..1'),
        ('W-Name', SOLVE_HEAD + 'unknown u : Num in 0..1'),
        ('W-Range', SOLVE_HEAD + 'unknown z : Num'),
        ('W-Range', SOLVE_HEAD + 'unknown z : Num in 3..1'),
        ('W-Range', SOLVE_HEAD + 'unknown z : Qty[KRW]? in 0..1'),
        ('W-Range', SOLVE_HEAD + 'unknown z : Num in 0..u'),                        # the range is written in numbers
]

SOLVE_OK = [
        SOLVE_HEAD + 'solve a = find: ok',
        SOLVE_HEAD + 'solve a = any where u >= 1 and m == blue: ok and size >= 1[KRW]',
        SOLVE_HEAD + 'solve a = min size: ok',
        SOLVE_HEAD + 'solve a = max u where p == 0: not ok or size <= 2[KRW]',
        SOLVE_HEAD + 'solve a = find at(here): ok',
        SOLVE_HEAD + 'solve a = any at(here) where x >= 1[KRW] and known(mem(D, note)) and now == @0:'
                     ' ok and bal(D, KRW) >= x',
        SOLVE_HEAD + 'solve a = min bal(D, KRW) + x at(here): ok',
        SOLVE_HEAD + 'solve a = max size at(here) where c == m: ok',
        SOLVE_HEAD + 'query x = exists run: reach(here)  solve a = find: x',        # in a goal a name is a query first
        SOLVE_HEAD + 'unknown t : Time  unknown who : Actor  unknown b : Bool  unknown q : Qty[KRW] in 1..2'
                     '  cell y : Qty[KRW] in 0..q init q  solve a = find where b and who == L and t >= @1: ok',
        IDLE + ' program { label here } query q = exists run: reach(end)  solve s = find at(here): q',
]

# Models for the solver: small ranges, so that every candidate can be tried. Shared with the engine checks.
SOLVE_MODELS = {
    'Fees': '''model Fees {
  actor I, D, L, J
  unknown cap : Num in 0..40
  unknown fee : Num in 0..12
  asset KRW in 0..40 init I = cap
  time 0..1
  cell remain : Qty[KRW] in 0..20 init 20 write collect observe D, L
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
  solve first = any: repaid and fee >= 3
  solve lean  = min cap where fee >= 5: repaid
  solve rich  = max kept where cap <= 30: repaid
  solve dear  = max fee where cap == 25: repaid
  solve never = any where cap <= 19: repaid
}''',
    'Kinds': '''model Kinds {
  actor A, B, C
  enum Mode { slow, fast, off }
  unknown mode : Mode
  unknown loose : Bool
  unknown when : Time
  unknown who  : Actor
  asset KRW in 0..5 init A = 5
  time 0..3
  cell done : Bool init false write pay
  event pay(to : Actor) by A {
    require loose or mode == fast
    move A -> to : 2[KRW]
    done := true
  }
  program { at when { attempt pay(who) { } }  at @2 { } }
  query paid = exists run: at(end, done)
  query toB  = forall run: at(end, bal(B, KRW) == 2[KRW])
  solve every  = find: paid
  solve closed = any: toB and not loose
  solve late   = find where mode == off and who == B: paid
  solve self   = any where who == A: paid and toB
}''',
    'Signal': '''model Signal {
  actor D, J
  enum Regime { boom, slump }
  enum Pick { none, A, B }
  unknown since : Time
  unknown bonus : Num in 0..6
  asset KRW in 0..10 init J = 10
  time 0..3
  cell regime   : Regime init in { boom, slump }
  cell selected : Pick init none write elect
  slot report : Regime for D
  event notice by J { require now >= since  tell D.report := regime asof now }
  event elect(a : Pick) by D { selected := a }
  event award by J {
    let hit = (selected == A and regime == boom) or (selected == B and regime == slump)
    move J -> D : if hit then 4[KRW] + bonus * 1[KRW] else 0[KRW]
  }
  program {
    at @1 { attempt notice { } }
    at @2 { choose D menu m { A => { run elect(A) }  B => { run elect(B) } } }
    at @3 { attempt notice { }  run award }
  }
  weight prior = if regime == boom then 1 + bonus else 1
  value got = at(end, bal(D, KRW)) ?? 0[KRW]
  query knows  = forall run: at(menu(m), K(D, regime == boom) or K(D, regime == slump))
  query canWin = can {D}: at(end, bal(D, KRW) >= 4[KRW])
  query worst  = max policy {D}: min run: got
  query likesA = prefer D at menu(m): A over B by got under prior
  solve informed = find: knows
  solve best     = max worst: canWin
  solve enough   = min bonus where since == @1: canWin and worst >= 7[KRW]
  solve leansA   = find: likesA
  solve blind    = any: likesA and canWin
}''',
    'Split': '''model Split {
  actor P, Q
  unknown total : Num in 0..30
  unknown parts : Num in 0..4
  asset KRW in 0..30 init P = total
  time 0..0
  cell share : Qty[KRW] in 0..30 init 0 write cut
  cell top   : Qty[KRW] in 0..30 init 0 write cut
  cell whole : Bool init false write cut
  event cut by P {
    let half = floor(bal(P, KRW) / 2)
    share := floor(bal(P, KRW) / parts) ?? 0[KRW]
    top := ceil(bal(P, KRW) * 3 / 4)
    whole := known(exact(bal(P, KRW) / 3))
    move P -> Q : half
  }
  program { run cut }
  query halves = forall run: at(end, bal(Q, KRW) >= 7[KRW])
  query thirds = forall run: at(end, whole)
  query fair   = forall run: at(end, share >= 5[KRW])
  query capped = forall run: at(end, top <= 10[KRW])
  solve both  = find where parts == 2: halves and thirds
  solve least = min total: fair
  solve most  = max parts where total <= 20: fair
  solve odd   = any: thirds and fair and not halves
  solve band  = find where parts == 0: capped and not thirds
}''',
    'Limits': '''model Limits {
  actor A, B
  unknown n   : Num in 0..12
  unknown lim : Num in 0..8
  asset KRW in 0..lim init A = n
  time 0..0
  cell x : Qty[KRW] in 0..10 init 5 write shift
  assume not (n == 7 and lim == 8)
  event shift by A { move A -> B : (n - 3) * 1[KRW] }
  program { attempt shift { } }
  query moved = exists run: at(end, bal(B, KRW) >= 1[KRW])
  solve ok      = find: moved
  solve low     = any where n <= 2: moved
  solve biggest = max n: moved
  solve still   = find where lim == 8: not moved
}''',
    'Gate': '''model Gate {
  actor D, J
  enum Tone { calm, loud }
  unknown fee : Num in 0..2
  asset KRW in 0..2 init D = 2
  time 0..1
  cell hold : Qty[KRW]? in 0..2 init empty write stash
  slot heard : Tone for D
  event stash by D {
    given t = mem(D, heard)
    require t == loud
    hold := bal(D, KRW)
    move D -> J : fee * 1[KRW]
  }
  program {
    alt { label quiet } or { label busy  attempt stash { } else { } }
    at @1 { }
  }
  query kept = forall run: at(end, known(hold))
  query sees = forall run: at(busy, K(J, known(hold)))
  solve stashed = find at(busy): kept
  solve dear    = any at(busy) where fee == 2: kept and not known(hold)
  solve thin    = min bal(D, KRW) at(busy): kept and bal(J, KRW) >= 1[KRW]
  solve idle    = find at(quiet): kept
  solve aware   = any at(busy) where known(hold) and recv(D, heard) == @1: sees
}''',
    'Tiers': '''model Tiers {
  actor D, L
  param strict : Bool
  param extra  : Num in 0..1
  unknown cap : Num in 0..40
  asset KRW in 0..40 init D = cap
  time 0..0
  cell owed : Qty[KRW] in 0..40 init 20 write settle
  event settle by D {
    require bal(D, KRW) >= (if strict then 30[KRW] else 20[KRW]) + extra * 5[KRW]
    move D -> L : owed
    owed := 0[KRW]
  }
  program { attempt settle { } }
  query cleared = forall run: at(end, owed == 0[KRW])
  query zone    = region strict: forall run: at(end, owed == 0[KRW])
  solve need = min cap: cleared
  solve band = find where cap <= 33: cleared
  solve slim = any where cap <= 22 and not strict: cleared
  solve nope = any where cap <= 22 and strict: cleared
}''',
    'Pickup': '''model Pickup {
  actor T, U
  asset KRW in 0..4 init T = 4
  time 0..2
  cell step : Num in 0..3 init 0 write bump
  event bump by T { step += 1 }
  event give(n : Qty[KRW]) by T { move T -> U : n }
  program {
    at @0 { run bump }
    choose T menu first {
      go   => { run bump  label mid  attempt give(2[KRW]) { run bump } else { } }
      stay => { }
    }
    at @1 { choose U menu second { take => { run bump }  leave => { } } }
    at @2 { }
  }
  query high = exists run: at(end, step == 3)
  query sure = can {U}: at(end, step >= 2)
  query all3 = forall run: at(end, step == 3)
  solve reach3 = find at(mid): high
  solve forced = find at(mid): sure
  solve most   = max step at(mid) where now == @0: sure
  solve sharp  = any at(mid): all3
  solve dull   = any at(mid) where (step == 3 and bal(T, KRW) >= 2[KRW]) or (now == @1 and step == 0): not high
  solve flat   = min bal(U, KRW) at(mid) where (step == 3 and bal(T, KRW) >= 2[KRW]) or (now == @1 and step == 0): not high
}''',
    # each of these is aimed at one thing the solver must not get wrong: a product of two unknowns (Area), knowledge
    # that depends on an unknown (Sight), the range of a freed cell (Draw), quotients where the dividend is a
    # multiple of the divisor (Steps), a value weighed by a prior (Lean), a def that reads an unknown which has the
    # name of a query (Names)
    'Area': '''model Area {
  actor A, B
  unknown w : Num in 0..6
  unknown h : Num in 0..5
  asset KRW in 0..30 init A = w * h
  time 0..0
  event pay by A { move A -> B : 12[KRW] }
  program { attempt pay { } }
  query paid = exists run: at(end, bal(B, KRW) == 12[KRW])
  solve ok   = find: paid
  solve thin = min w: paid
  solve tall = max h where w <= 2: paid
  solve wide = any where h == 2: paid
  solve area = min w * h: paid and w >= h + 1
}''',
    'Sight': '''model Sight {
  actor D, J
  unknown mark : Num in 0..3
  time 0..0
  cell kind : Num in 0..1 init in { 0, 1 }
  cell sign : Num in 0..3 init 0 write reveal observe D
  event reveal by J { sign := if kind == 1 then 2 else mark }
  program { run reveal }
  query tells = forall run: at(end, K(D, kind == 1) or K(D, kind == 0))
  solve clear = find: tells
  solve fog   = any: not tells
  solve top   = max mark: tells
}''',
    'Draw': '''model Draw {
  actor A
  unknown top : Num in 0..5
  time 0..0
  cell pick : Num in 0..top init 0 write draw
  event draw by A { free pick }
  program { run draw }
  query three = exists run: at(end, pick == 3)
  query small = forall run: at(end, pick <= 2)
  solve hits  = find: three
  solve tight = max top: small
  solve both  = any: three and small
}''',
    'Steps': '''model Steps {
  actor A
  unknown total : Num in 0..12
  time 0..0
  solve top   = max total: floor(total / 3) <= 1
  solve low   = min total: ceil(total / 3) >= 3
  solve whole = max total: known(exact(total / 4)) and total <= 11
  solve gap   = find: floor(total / 3) <= 1 or total >= 7
  solve minus = max total: floor((0 - total) / 3) >= -2
  solve odd   = find: not known(exact(total / 3))
  solve quad  = find: total * 4 <= 13
}''',
    'Lean': '''model Lean {
  actor D, J
  enum Regime { boom, slump }
  enum Pick { none, A, B }
  unknown bonus : Num in 0..6
  asset KRW in 0..10 init J = 10
  time 0..1
  cell regime   : Regime init in { boom, slump }
  cell selected : Pick init none write elect
  event elect(a : Pick) by D { selected := a }
  event award by J {
    move J -> D : if selected == A and regime == boom then 4[KRW] + bonus * 1[KRW] else (if selected == B and regime == slump then 10[KRW] else 0[KRW])
  }
  program {
    choose D menu m { A => { run elect(A) }  B => { run elect(B) } }
    at @1 { run award }
  }
  weight prior = if regime == boom then 2 else 1
  value got = at(end, bal(D, KRW)) ?? 0[KRW]
  query likesA = prefer D at menu(m): A over B by got under prior
  solve leansA = find: likesA
  solve least  = min bonus: likesA
  solve never  = any where bonus <= 0: likesA
}''',
    'Names': '''model Names {
  actor A
  unknown cap : Num in 0..5
  time 0..0
  def spare() : Num = cap - 3
  query cap = exists run: reach(end)
  solve own  = find: cap and spare() >= 0
  solve most = max spare(): cap
}''',
}

# what arithmetic says about the aimed models (worked out by hand; see the comments in the models' queries)
SOLVE_EXPECTED = {
    'Area': {'ok': '{(3, 4), (3, 5), (4, 3), (4, 4), (4, 5), (5, 3), (5, 4), (5, 5), (6, 2), (6, 3), (6, 4), (6, 5)}',   # w * h >= 12
             'thin': '3 at w=3, h=4', 'tall': 'none', 'wide': 'w=6, h=2', 'area': '12 at w=4, h=3'},
    'Sight': {'clear': '{1, 3}', 'fog': 'mark=0', 'top': '3 at mark=3'},     # 0 looks like "nothing happened", 2 like the other world
    'Draw': {'hits': '{3..5}', 'tight': '2 at top=2', 'both': 'none'},
    'Steps': {'top': '5 at total=5', 'low': '7 at total=7', 'whole': '8 at total=8', 'gap': '{0..5, 7..12}',
              'minus': '6 at total=6', 'odd': '{1..2, 4..5, 7..8, 10..11}', 'quad': '{0..3}'},
    'Lean': {'leansA': '{2..6}', 'least': '2 at bonus=2', 'never': 'none'},  # 2 * (4 + bonus) > 10
    'Names': {'cap': '{own: true, most: true}', 'own': '{3..5}', 'most': '2 at cap=5'},   # in the def, cap is the unknown
}


def model(body, name='M'):
    return f'model {name} {{\n{body}\n}}'


def with_queries(block, extra):
    """The documented model with more queries added before its closing brace."""
    block = block.rstrip()
    assert block.endswith('}')
    return block[:-1] + extra + '\n}'


def unit(machine, name, *args):
    ev = machine.spec.events[name]
    return Unit(name, [(ev, list(args))], ev.net)


def bal(state, who, asset='KRW'):
    return state.sig[('bal', who, asset)]


class Case(unittest.TestCase):
    def reject(self, code, body, run=False, kind=FeanError):
        with self.assertRaises(kind) as caught:
            family = load(model(HEAD + body), PRELUDE)
            if run:
                for pvals in family.assignments():
                    family.analysis(pvals)
                family.results()
        self.assertEqual(caught.exception.code, code, str(caught.exception))
        return caught.exception

    def accept(self, body):
        return load(model(HEAD + body), PRELUDE)


class TestDocument(Case):
    def test_document_integrity(self):
        anchors, links = {}, []
        files = [p for p in ROOT.rglob('*.md') if not p.name.startswith('.')]
        for path in files:
            text = path.read_text(encoding='utf-8')
            anchors[path.resolve()] = set(re.findall(r'<a id="([^"]+)"', text))
            links += [(path, href) for href in re.findall(r'\]\(([^)\s]+)\)', text) if not href.startswith('http')]
        self.assertGreater(len(links), 60)
        for path, href in links:
            target, _, frag = href.partition('#')
            dest = (path.parent / target).resolve() if target else path.resolve()
            self.assertTrue(dest.exists(), f'{path.name}: {href}')
            if frag and dest.suffix == '.md':
                self.assertIn(frag, anchors[dest], f'{path.name}: {href}')
        spec = DOC.read_text(encoding='utf-8')
        self.assertEqual(list(MODELS), DOC_MODELS)
        self.assertEqual(len(doc_blocks(DOC, 'fean-prelude')), 1)
        self.assertEqual(len(doc_blocks(DOC, 'ebnf')), 1)
        for name, block in MODELS.items():
            declared = re.findall(r'^\s*(?:query|solve)\s+(\w+)\s*=', block, re.M)
            self.assertEqual(list(expectations(block)), declared, name)
        # the grammar in the document is closed, and agrees with the parser on every reserved word and symbol
        (ebnf,) = doc_blocks(DOC, 'ebnf')
        ebnf = re.sub(r'\(\*.*?\*\)', '', ebnf)
        literals = set(re.findall(r'"([^"]+)"', ebnf))
        self.assertEqual({w for w in literals if w[0].isalpha()}, set(KEYWORDS))
        self.assertEqual(len(KEYWORDS), 93)
        for symbol in literals:
            if not symbol[0].isalpha() and symbol != '@':
                self.assertEqual([kind for kind, _, _ in tokenize(symbol)], ['sym', 'eof'], symbol)
        defined = re.findall(r'^(\w+)\s*=', ebnf, re.M)
        used = set(re.findall(r'\b[A-Za-z]\w*\b', re.sub(r'"[^"]*"', ' ', re.sub(r'^\w+\s*=', '', ebnf, flags=re.M))))
        self.assertEqual(len(defined), len(set(defined)))
        self.assertEqual(used - set(defined), {'Name', 'Int'})
        self.assertEqual(set(defined) - used, {'model'})
        # every rule named in the specification is exercised by name in this file
        rules = set(re.findall(r'\b([STEWPQD]-[A-Z][a-z]+)\b', spec))
        self.assertEqual(len(rules), 42)
        mine = Path(__file__).read_text(encoding='utf-8')
        self.assertEqual({r for r in rules if f"'{r}'" not in mine}, set())
        # formulas are written the way GitHub renders them: in fenced math blocks (inside `$$` it drops the
        # backslash of a brace) and without \operatorname (which it does not allow)
        for path in (DOC, DOC.with_name('financial-language-v0.1.md'), ROOT / 'README.md', ROOT / 'README.en.md'):
            text = path.read_text(encoding='utf-8')
            self.assertNotIn('\\operatorname', text, path.name)
            self.assertEqual([line for line in text.splitlines() if line.strip() == '$$'], [], path.name)
        self.assertEqual(len(doc_blocks(DOC, 'math')), 4)
        # the design records are kept outside the repository; where they are present, they and the test
        # files name the same tests
        defined = set()
        for path in (ROOT / 'tests').glob('test_*.py'):
            defined |= set(re.findall(r'def (test_\w+)', path.read_text(encoding='utf-8')))
        records = ROOT / 'docs' / 'maps'
        if records.is_dir():
            concept = set(re.findall(r'`(test_\w+)`', (records / '01_concept_map.md').read_text(encoding='utf-8')))
            impl = set(re.findall(r'`(test_\w+)`', (records / '02_impl_map.md').read_text(encoding='utf-8')))
            self.assertEqual(concept, defined)
            self.assertEqual(impl, defined)
        self.assertEqual(len(defined), 80)


class TestSyntax(Case):
    def test_syntax_errors(self):
        cases = SYNTAX_CASES
        for code, line, src in cases:
            with self.subTest(src=src):
                with self.assertRaises(SyntaxErr) as caught:
                    parse_model(src)
                self.assertEqual((caught.exception.code, caught.exception.line), (code, line))
        self.assertEqual(parse_model('model M { program { } }')[1][0]['body'], [])
        self.assertEqual(parse_model('model M { time 0..4 }')[1][0], {'lo': 0, 'hi': 4, 'kind': 'time', 'line': 1})
        tree = Parser('1 + 2 * 3 == 7 and not false or true').expr()
        self.assertEqual(tree, ('bin', 'or',
                                ('bin', 'and',
                                 ('bin', '==', ('bin', '+', ('int', 1), ('bin', '*', ('int', 2), ('int', 3))),
                                  ('int', 7)),
                                 ('not', ('bool', False))),
                                ('bool', True)))
        self.assertEqual(Parser('- a ?? 3[KRW/share]').expr(),
                         ('neg', ('default', ('name', 'a'), ('qty', 3, [('KRW', 1), ('share', -1)]))))
        sugar = parse_model('model M { query q = CAN({D}, x) }')[1][0]['query']
        self.assertEqual(sugar, ('can', ['D'], ('at', ('end',), ('name', 'x'))))


class TestStatic(Case):
    def test_document_models_check(self):
        for name in DOC_MODELS:
            with self.subTest(model=name):
                family = load(MODELS[name], PRELUDE)
                self.assertEqual(family.spec.name, name)
                self.assertGreater(len(family.spec.queries), 0)
        spec = load(MODELS['Repay80'], PRELUDE).spec
        self.assertEqual(spec.cells['remain'].type, ('Qty', (('KRW', 1),)))
        self.assertEqual(spec.cells['proof'].type, ('Opt', ('Enum', 'Regime')))
        self.assertEqual(spec.params['early'], (('Bool',), [False, True]))
        self.assertEqual(spec.qinfo['worst'], ('num', (('KRW', 1),)))
        self.assertEqual(spec.qinfo['chance'], ('num', ()))

    def test_type_errors_rejected(self):
        cases = TYPE_CASES
        for code, body in cases:
            with self.subTest(body=body):
                self.assertIsInstance(self.reject(code, body), StaticErr)
        self.accept('event e(n : Num) by D { x := floor(x * 2 / n) ?? 0[KRW] }')
        self.accept('event e by D { x := floor(x * 3 / 2) }')
        self.accept('event e by D { given g = mem(D, note)  c := g }')

    def test_structure_errors_rejected(self):
        cases = STRUCTURE_CASES
        for code, body in cases:
            with self.subTest(body=body):
                self.assertIsInstance(self.reject(code, body), StaticErr)
        # checks that need parameter values run when the model is instantiated, still before any step
        self.assertIsInstance(self.reject('W-Range', IDLE + ' cell y : Qty[KRW] in 0..5 init 9', run=True), StaticErr)
        late = IDLE + ' param t : Time program { at @2 { } at t { } } query q = exists run: reach(end)'
        with self.assertRaises(StaticErr) as caught:
            self.accept(late).analysis({'t': 0})
        self.assertEqual(caught.exception.code, 'W-Clock')
        self.assertTrue(self.accept(late).analysis({'t': 2}).query(self.accept(late).spec.queries['q']))
        # what the rules allow
        self.accept('event e by D { auth true  move L -> D : 1[KRW]  bal(D, KRW) += 1[KRW] }')
        self.accept('event e by D, L { move L -> D : 1[KRW] }')
        self.accept(IDLE + ' program { alt { choose D menu m { a => { } } } or { choose D menu m { b => { } } } }')
        self.accept(IDLE + ' program { attempt e { label done } else { label done } }')
        # the three errors left for run time
        self.assertIsInstance(self.reject('D-Init', IDLE + ' assume false query q = exists run: reach(end)', run=True),
                              ModelErr)
        self.assertIsInstance(self.reject('D-Init', IDLE + ' invariant x > 0[KRW] query q = exists run: reach(end)',
                                          run=True), ModelErr)
        self.assertIsInstance(self.reject(
            'D-Move', 'event e by D { move D -> L : 0[KRW] - 5[KRW] } program { run e }'
                      ' query q = exists run: reach(end)', run=True), ModelErr)
        self.assertIsInstance(self.reject('D-Weight', IDLE + ' weight w = 0 query q = expect w: min run: 1', run=True),
                              ModelErr)

    def test_unknowns_and_solves_typecheck(self):
        for code, body in SOLVE_CASES:
            with self.subTest(body=body):
                self.assertIsInstance(self.reject(code, body), StaticErr)
        for body in SOLVE_OK:
            with self.subTest(body=body):
                self.assertEqual(len(self.accept(body).spec.solves), 1)
        spec = self.accept(SOLVE_OK[-2]).spec                           # every type an unknown can have, as numbers
        self.assertEqual({n: u[1:] for n, u in spec.unknowns.items()},
                         {'u': (0, 3), 'm': (0, 1), 't': (0, 3), 'who': (0, 1), 'b': (0, 1), 'q': (1, 2)})
        self.assertEqual(spec.unknowns['q'][0], ('Qty', (('KRW', 1),)))
        point = self.accept(SOLVE_OK[5]).spec
        self.assertEqual(point.solves['a'][1:4], ('any', None, 'here'))  # form, objective, label
        self.assertEqual(self.accept(SOLVE_OK[6]).spec.sinfo, {'a': (('KRW', 1),)})     # the unit of what is minimised
        self.assertEqual(self.accept(SOLVE_OK[3]).spec.sinfo, {'a': ()})
        for name, src in SOLVE_MODELS.items():
            with self.subTest(model=name):
                self.assertGreater(len(load(src, PRELUDE).spec.solves), 1)     # the aimed models are small
        self.assertEqual(sorted(load(MODELS['Capital'], PRELUDE).spec.solves), ['enough', 'need', 'short', 'spare'])

    def test_templates_expand(self):
        loan = load(MODELS['LoanLifecycle'], PRELUDE)
        self.assertLessEqual({'c.life', 'c.remain', 'c.holder', 'c.late', 'c.waived'}, set(loan.spec.cells))
        self.assertLessEqual({'c.originate', 'c.repay', 'c.assign', 'c.forgive', 'c.close'}, set(loan.spec.events))
        self.assertEqual(loan.spec.cells['c.remain'].type, ('Qty', (('KRW', 1),)))
        self.assertEqual(loan.spec.cells['c.remain'].observers, ['Lender', 'Debtor'])
        self.assertEqual(loan.spec.events['c.repay'].params, [('n', ('Qty', (('KRW', 1),)))])
        self.assertEqual(Machine(loan.spec).ranges['c.remain'], (0, 105))      # face = floor(100 * 10500 / 10000)

        call = load(MODELS['CallExample'], PRELUDE).spec
        self.assertLessEqual({'call.right.state', 'call.deal.stage'}, set(call.cells))
        self.assertLessEqual({'call.pay', 'call.right.grant', 'call.right.use', 'call.deal.spot'}, set(call.events))
        self.assertEqual([q for q, _ in call.batches['call.exercise'][1]], ['call.right.use', 'call.deal.spot'])
        self.assertEqual(call.menus, {'call.callMenu': [('H', ('exercise', 'pass'))]})
        self.assertEqual(call.cells['call.deal.stage'].owner, 'call.deal')
        self.assertEqual(call.program[1], ('at', ('time', 1), [
            ('choose', 'H', 'call.callMenu', [('exercise', [('run', ('batch', 'call.exercise'))]), ('pass', [])])]))

        order = load(MODELS['Order'], PRELUDE).spec
        self.assertEqual(order.menus, {'order': [('T', ('bid(0)', 'bid(1)', 'bid(2)'))]})
        self.assertEqual(order.program[0][3][2], ('bid(2)', [('run', ('ev', 'buy', [('int', 2)]))]))

        proc = self.accept('event e(n : Qty[KRW]) by D { x := n }'
                           ' proc twice(n : Qty[KRW], t : Time) { at t { run e(n) } }'
                           ' program { do twice(3[KRW], @1) }').spec
        self.assertEqual(proc.program, [('at', ('time', 1), [('run', ('ev', 'e', [('qty', 3, [('KRW', 1)])]))])])

        outsider = MODELS['LoanLifecycle'].replace('program {', 'event hack by Lender { c.remain := 0[KRW] }\n  program {')
        with self.assertRaises(StaticErr) as caught:
            load(outsider, PRELUDE)
        self.assertEqual(caught.exception.code, 'W-Writer')
        self.reject('P-Args', IDLE + ' instance bad = Loan(D, L, red, 1[KRW], 1[KRW], @1)')
        self.reject('P-Args', IDLE + ' instance bad = Loan(D, L, KRW, 1[share], 1[KRW], @1)')
        self.reject('P-Unit', IDLE + ' instance bad = Nothing()')


class TestTransitions(Case):
    FLOW = model('''
      actor A, B, C
      asset KRW in 0..200 init A = 100
      time 0..0
      event chain by A, B { move A -> B : 100[KRW]  move B -> C : 80[KRW] }
      event chainNet by A, B net { move A -> B : 100[KRW]  move B -> C : 80[KRW] }
      event tooMuch by A, B net { move A -> B : 100[KRW]  move B -> C : 150[KRW] }
      event nothing by A { move A -> B : 0[KRW] }
    ''')
    OVERDRAFT = model('''
      actor A, B
      asset KRW in -50..100
      time 0..0
      event pay50 by A { move A -> B : 50[KRW] }
      event pay51 by A { move A -> B : 51[KRW] }
    ''')
    FAIL = model('''
      actor A, B
      asset KRW in 0..100
      time 0..0
      cell x   : Qty[KRW] in 0..100 init 0 write bump, solve2
      cell y   : Qty[KRW] in 0..100 init 0 write solve2
      cell tag : Num? in 0..9 init empty write needs
      event plain by A { move A -> B : 100[KRW] }
      event guarded by A { require bal(A, KRW) >= 100[KRW]  move A -> B : 100[KRW] }
      event bump by A { x += 200[KRW] }
      event solve2 by A {
        free x  free y
        ensure post(x) == (if post(y) > 0[KRW] then 100[KRW] else 0[KRW])
        ensure post(y) == (if post(x) > 0[KRW] then 100[KRW] else 0[KRW])
      }
      event needs by A { given t = tag  tag := t }
      event gated by A { auth false }
      event backwards by A { move A -> B : 0[KRW] - 5[KRW] }
      program {
        attempt plain   { label a1 } else { label a2 }
        attempt guarded { label b1 } else { label b2 }
        attempt bump    { label c1 } else { label c2 }
      }
      query plainRefused   = forall run: reach(a2)
      query guardedRefused = forall run: reach(b2)
      query reachedEnd     = exists run: reach(end)
    ''')

    def test_gross_default_net_declared(self):
        debt = Machine(load(MODELS['MutualDebt'], PRELUDE).spec)
        start = debt.worlds[0]
        status, why = debt.step(debt.unit(('batch', 'gross')), start)
        self.assertEqual((status, why), ('BLOCKED', 'funds of Ann in KRW'))
        status, after = debt.step(debt.unit(('batch', 'netted')), start)
        self.assertEqual(status, 'OK')
        self.assertEqual([(bal(s, 'Ann'), bal(s, 'Bob')) for s in after], [(0, 0)])

        flow = Machine(load(self.FLOW).spec)
        start = flow.worlds[0]
        self.assertEqual(flow.step(unit(flow, 'chain'), start), ('BLOCKED', 'funds of B in KRW'))
        status, after = flow.step(unit(flow, 'chainNet'), start)
        self.assertEqual((status, [(bal(s, 'A'), bal(s, 'B'), bal(s, 'C')) for s in after]), ('OK', [(0, 20, 80)]))
        self.assertEqual(flow.step(unit(flow, 'tooMuch'), start)[0], 'BLOCKED')
        self.assertEqual(flow.step(unit(flow, 'nothing'), start)[0], 'OK')       # a zero transfer commits
        self.assertEqual(bal(start, 'A'), 100)                                  # nothing moved on refusal

        over = Machine(load(self.OVERDRAFT).spec)
        status, after = over.step(unit(over, 'pay50'), over.worlds[0])
        self.assertEqual((status, bal(after[0], 'A')), ('OK', -50))
        self.assertEqual(over.step(unit(over, 'pay51'), over.worlds[0])[0], 'BLOCKED')

    def test_failure_classification(self):
        family = load(self.FAIL)
        m = Machine(family.spec)
        start = m.worlds[0]
        self.assertEqual(m.step(unit(m, 'plain'), start), ('BLOCKED', 'funds of A in KRW'))
        self.assertEqual(m.step(unit(m, 'guarded'), start), ('BLOCKED', 'require in guarded'))
        self.assertEqual(m.step(unit(m, 'needs'), start), ('BLOCKED', 'given t in needs'))
        self.assertEqual(m.step(unit(m, 'gated'), start), ('BLOCKED', 'auth in gated'))
        self.assertEqual(m.step(unit(m, 'bump'), start)[0], 'NO_SETTLEMENT')
        status, after = m.step(unit(m, 'solve2'), start)
        self.assertEqual((status, sorted((s.sig['x'], s.sig['y']) for s in after)), ('OK', [(0, 0), (100, 100)]))
        with self.assertRaises(ModelErr) as caught:
            m.step(unit(m, 'backwards'), start)
        self.assertEqual(caught.exception.code, 'D-Move')
        # the same shortage is a refusal however it was written; a range overflow is fatal even in attempt
        self.assertEqual(family.results(), {'plainRefused': 'true', 'guardedRefused': 'true', 'reachedEnd': 'false'})
        (leaf,) = family.analysis().leaves
        self.assertEqual((leaf.fail, leaf.step[:3]), ('NO_SETTLEMENT', ('fail', 'bump', 'NO_SETTLEMENT')))

        support = load(MODELS['Support'], PRELUDE)
        m = Machine(support.spec)
        self.assertEqual(m.step(unit(m, 'aidAfter'), m.worlds[0])[0], 'NO_SETTLEMENT')
        status, after = m.step(unit(m, 'aidBefore'), m.worlds[0])
        self.assertEqual((status, [(s.sig['aid'], s.sig['unpaid']) for s in after]), ('OK', [(100, False)]))
        fails = [leaf for leaf in support.analysis().leaves if leaf.fail]
        self.assertEqual([(leaf.fail, leaf.parent.step) for leaf in fails], [('NO_SETTLEMENT', ('branch', 0))])

        must = load(model('''
          actor A, B
          asset KRW in 0..100
          time 0..0
          event plain by A { move A -> B : 100[KRW] }
          program { run plain  label after }
          query went = exists run: reach(after)
        '''))
        self.assertEqual(must.text('went'), 'false')
        self.assertEqual([leaf.fail for leaf in must.analysis().leaves], ['BLOCKED'])

    def test_arithmetic_and_time(self):
        arith = load(model('''
          actor A
          asset KRW in 0..1000 init A = 20
          asset share in 0..10
          time 0..3
          cell px : Qty[KRW/share] in 0..100 init 7
          def interest(p : Qty[KRW], bp : Num) : Qty[KRW] = floor(p * (10000 + bp) / 10000)
          value owed = interest(100[KRW], 500)
          query withInterest = max run: owed
          query floorNeg  = max run: floor((0 - 7) / 2)
          query ceilNeg   = max run: ceil((0 - 7) / 2)
          query ceilPos   = max run: ceil(7 / 2)
          query exactOk   = max run: exact(6 / 3) ?? 99
          query exactNo   = max run: exact(7 / 2) ?? 99
          query divZero   = max run: floor(7 / (1 - 1)) ?? 42
          query notional  = max run: at(end, px * 3[share]) ?? 0[KRW]
          query unitPrice = max run: at(end, floor(bal(A, KRW) / 2[share])) ?? 0[KRW/share]
          query smaller   = max run: at(end, min(bal(A, KRW), 5[KRW]) - max(1[KRW], 2[KRW])) ?? 0[KRW]
          query clock     = exists run: at(end, now == @0)
        '''))
        self.assertEqual(arith.results(), {
            'withInterest': '105[KRW]', 'floorNeg': '-4', 'ceilNeg': '-3', 'ceilPos': '4', 'exactOk': '2',
            'exactNo': '99', 'divZero': '42', 'notional': '21[KRW]', 'unitPrice': '10[KRW/share]',
            'smaller': '3[KRW]', 'clock': 'true'})

        window = load(model('''
          actor H, W
          time 0..3
          instance r = Right(H, W, @1, @2)
          program {
            at @0 { run r.grant  attempt r.use { label early } else { label tooEarly } }
            at @1 { attempt r.use { label inWindow } else { label refused } }
            at @2 { attempt r.use { label again } else { label spent } }
            at @3 { attempt r.lapse { label lapsed_ } else { label kept } }
          }
          query q1 = forall run: reach(tooEarly) and reach(inWindow) and reach(spent) and reach(kept)
          query q2 = forall run: at(inWindow, now == @1 and r.state == used)
        '''), PRELUDE)
        self.assertEqual(window.results(), {'q1': 'true', 'q2': 'true'})

        repay = load(MODELS['Repay80'], PRELUDE).analysis({'early': True})
        chosen = [n for n in repay.ex.nodes if n.step == ('commit', 'elect(A)')][0]
        self.assertEqual(chosen.state.mu[('D', 'ownChoice')], ('A', 2, 2))      # asof now, received at 2


class TestLibrary(Case):
    def test_library_loan_and_call(self):
        loan = load(MODELS['LoanLifecycle'], PRELUDE)
        self.assertEqual(loan.results(), {'afterFirst': 'true', 'afterSecond': 'true', 'lastRefused': 'true',
                                          'overdue': '5[KRW]', 'debtorKnows': 'true'})
        (leaf,) = loan.analysis().leaves
        end = leaf.state
        self.assertEqual((bal(end, 'Lender'), bal(end, 'Debtor'), bal(end, 'Buyer')), (60, 0, 40))
        self.assertEqual([end.sig['c.' + k] for k in ('remain', 'late', 'holder', 'life')], [5, 5, 'Buyer', 'active'])

        call = load(MODELS['CallExample'], PRELUDE)
        self.assertEqual(call.results(), {'exercised': 'true', 'passed': 'true', 'premiumKept': 'true',
                                          'holderCan': 'true', 'writerCan': 'false'})
        ends = sorted((bal(l.state, 'H'), bal(l.state, 'W'), bal(l.state, 'H', 'share'), bal(l.state, 'W', 'share'),
                       l.state.sig['call.right.state']) for l in call.analysis().leaves)
        self.assertEqual(ends, [(0, 105, 1, 0, 'used'), (100, 5, 0, 1, 'lapsed')])

        lib = Machine(load(model('''
          actor S, B, H, W
          asset KRW   in 0..300 init B = 200, H = 100
          asset share in 0..5   init S = 2
          time 0..3
          instance loan  = Loan(B, H, KRW, 50[KRW], 50[KRW], @2)
          instance sale  = Sale(S, B, share, 2[share], KRW, 150[KRW])
          instance right = Right(H, W, @1, @2)
        '''), PRELUDE).spec)
        s0 = lib.worlds[0]

        def go(state, name, *args):
            status, info = lib.step(unit(lib, name, *args), state)
            return (status, info[0]) if status == 'OK' else (status, info)

        self.assertEqual(go(s0, 'loan.repay', 10), ('BLOCKED', 'require in loan.repay'))    # not yet issued
        _, s1 = go(s0, 'loan.originate')                                                    # zero interest is valid
        self.assertEqual((bal(s1, 'B'), bal(s1, 'H'), s1.sig['loan.remain'], s1.sig['loan.life']),
                         (150, 150, 50, 'active'))
        self.assertEqual(go(s1, 'loan.originate')[0], 'BLOCKED')                            # no double issue
        self.assertEqual(go(s1, 'loan.repay', 60)[0], 'BLOCKED')                            # above the balance
        _, s2 = go(s1, 'loan.forgive', 20)
        self.assertEqual((s2.sig['loan.remain'], s2.sig['loan.waived'], bal(s2, 'H')), (30, 20, 150))
        _, s3 = go(s2, 'loan.assign', 'W')
        _, s4 = go(s3, 'loan.repay', 30)
        self.assertEqual((s4.sig['loan.remain'], s4.sig['loan.life'], bal(s4, 'W'), bal(s4, 'B')),
                         (0, 'settled', 30, 150))
        self.assertEqual(go(s4, 'loan.close')[0], 'BLOCKED')                                # nothing left to close

        self.assertEqual(go(s0, 'sale.settle'), ('BLOCKED', 'auth in sale.settle'))         # no agreement yet
        _, t1 = go(s0, 'sale.agree')
        _, t2 = go(t1, 'sale.settle')
        self.assertEqual((bal(t2, 'S'), bal(t2, 'B'), bal(t2, 'B', 'share'), t2.sig['sale.stage']),
                         (150, 50, 2, 'delivered'))
        self.assertEqual(go(t2, 'sale.spot')[0], 'BLOCKED')

        _, r1 = go(s0, 'right.grant')
        self.assertEqual(go(r1, 'right.use')[0], 'BLOCKED')                                 # before the window
        _, r2 = go(State(1, r1.sig, r1.mu), 'right.use')
        self.assertEqual(r2.sig['right.state'], 'used')
        self.assertEqual(go(r2, 'right.use')[0], 'BLOCKED')                                 # single use
        self.assertEqual(go(State(3, r1.sig, r1.mu), 'right.use')[0], 'BLOCKED')            # after the window
        self.assertEqual(go(State(3, r1.sig, r1.mu), 'right.lapse')[0], 'OK')


class TestEpistemics(Case):
    QUIET = '''
      actor D, L, I
      asset KRW in 0..100 init I = 100 PUBLIC
      enum Regime { boom, slump }
      time 0..2
      cell regime : Regime init in { boom, slump }
      cell hidden : Bool init in { true, false }
      cell owed : Qty[KRW] in 0..80 init 80 write pay observe D, L
      slot memo : Regime for L
      event award by I { move I -> D : if regime == boom then 100[KRW] else 0[KRW] }
      event pay by D { let b = min(bal(D, KRW), owed)  move D -> L : b  owed -= b }
      event leak by I { tell L.memo := regime asof now }
      event wipe by I { forget L.memo }
      program {
        at @1 { run award  label paid }
        at @2 { label next  run pay  run leak  label told  run wipe }
      }
      query debtorBefore  = exists run: at(start, K(D, regime == boom) or K(D, regime == slump))
      query debtorSeesPay = exists run: at(paid, K(D, regime == boom))
      query debtorInTick  = forall run: at(paid, K(D, regime == boom) or K(D, regime == slump))
      query debtorNextTick = forall run: at(next, K(D, regime == boom) or K(D, regime == slump))
      query creditorOwed  = forall run: at(end, K(L, owed == 0[KRW]) or K(L, owed != 0[KRW]))
      query creditorCash  = forall run: at(end, K(L, bal(L, KRW) == 80[KRW]) or K(L, bal(L, KRW) == 0[KRW]))
      query creditorEarly = exists run: at(paid, K(L, regime == boom))
      query issuerOwed    = forall run: at(end, K(I, owed == 0[KRW]) or K(I, owed != 0[KRW]))
      query hiddenStays   = exists run: at(end, K(D, hidden) or K(D, not hidden))
      query toldThenGone  = forall run: at(told, K(L, hidden) or K(L, regime == boom) or K(L, regime == slump))
    '''

    def test_observation_of_holdings(self):
        quiet = load(model(self.QUIET.replace('PUBLIC', '')))
        self.assertEqual(quiet.results(), {
            'debtorBefore': 'false',      # nothing observed yet
            'debtorSeesPay': 'true',      # no message, but D sees 100 arrive in its own balance
            'debtorInTick': 'false',      # a balance of 0 at time 1: paid nothing, or not paid yet?
            'debtorNextTick': 'true',     # once the public clock moves on, 0 can only mean slump
            'creditorOwed': 'true',       # owed is declared observable by L
            'creditorCash': 'true',       # L sees its own balance
            'creditorEarly': 'false',     # D's balance is private, so L cannot see the 100 arrive
            'issuerOwed': 'false',        # I observes neither owed nor the step that changed it
            'hiddenStays': 'false',       # no observer declared
            'toldThenGone': 'true'})
        ex = quiet.analysis().ex
        boom = [l for l in ex.leaves if l.state.sig['regime'] == 'boom' and not l.state.sig['hidden']][0]
        paid = ex.point(boom, ('label', 'paid'))
        self.assertEqual(ex.view(paid, 'D'), (1, None, (), (80, 100)))            # (time, menu, slots, owed and own cash)
        self.assertEqual(ex.view(paid, 'L'), (1, None, (), (80, 0)))
        self.assertEqual(ex.view(paid, 'I'), (1, None, (), (0,)))
        told = ex.point(boom, ('label', 'told'))
        self.assertEqual(ex.view(told, 'L'), (2, None, (('memo', ('boom', 2, 2)),), (0, 80)))
        self.assertEqual(ex.view(boom, 'L'), (2, None, (), (0, 80)))               # forgotten at the end
        self.assertFalse(ex.knows(paid, 'L', ('bin', '==', ('name', 'regime'), ('name', 'boom'))))

        public = load(model(self.QUIET.replace('PUBLIC', 'observe all')))
        self.assertEqual(public.text('creditorEarly'), 'true')                     # a public ledger shows D's balance
        ex = public.analysis().ex
        self.assertEqual(ex.view(ex.point(ex.leaves[0], ('label', 'paid')), 'L')[3],
                         (80,) + tuple(bal(ex.point(ex.leaves[0], ('label', 'paid')).state, who) for who in 'DLI'))

    def test_points_are_prefixes(self):
        analysis = load(MODELS['Repay80'], PRELUDE).analysis({'early': True})
        ex = analysis.ex
        self.assertEqual(len(ex.roots), 2)
        for leaf in ex.leaves:
            path = ex.path(leaf)
            self.assertIsNone(path[0].parent)
            self.assertEqual(path[0].step[0], 'init')
            self.assertTrue(all(b.parent is a for a, b in zip(path, path[1:])))
            self.assertEqual(sum(n.step[0] == 'show' for n in path), 1)             # W-Menu: one visit per history
            self.assertIs(ex.point(leaf, ('start',)), path[0])
            self.assertIs(ex.point(leaf, ('end',)), leaf)
            self.assertEqual(leaf.step, ('end',))
        self.assertTrue(all(ex.under(n) for n in ex.nodes))                         # every point is a prefix of a run
        leaf = [l for l in ex.leaves if l.state.sig['regime'] == 'boom' and l.state.sig['selected'] == 'A'][0]
        menu = ex.point(leaf, ('menu', 'claimMenu'))
        self.assertEqual((menu.step, menu.display), (('show', 'D', 'claimMenu'), ('D', 'claimMenu', ('A', 'B'))))
        self.assertIs(menu.state, menu.parent.state)                                # showing a menu changes no state
        self.assertNotEqual(ex.view(menu, 'D'), ex.view(menu.parent, 'D'))          # but D now sees the menu
        self.assertEqual(ex.view(menu, 'J'), ex.view(menu.parent, 'J'))             # and J sees nothing new
        self.assertEqual(ex.view(menu, 'D')[1], ('claimMenu', ('A', 'B')))
        self.assertEqual(menu.state.mu[('D', 'report')], ('boom', 0, 1))            # (content, as of, received)
        self.assertIsNone(menu.state.sig['proof'])                                  # evidence comes at time 3
        picked = [c for c in menu.children if c.step == ('pick', 'D', 'claimMenu', 'A')][0]
        after = picked.children[0]
        self.assertEqual((picked.state.sig['selected'], after.step, after.state.sig['selected']),
                         ('none', ('commit', 'elect(A)'), 'A'))                     # just before and just after
        funded = ex.point(leaf, ('label', 'funded'))
        self.assertEqual((funded.step, funded.state.t, bal(funded.state, 'D')), (('mark', 'funded'), 3, 100))
        self.assertIsNone(ex.point(leaf, ('label', 'nowhere')))

        support = load(MODELS['Support'], PRELUDE).analysis()
        failed = [l for l in support.leaves if l.fail][0]
        self.assertIsNone(support.ex.point(failed, ('end',)))                       # a failed run has no end point
        self.assertIsNotNone(support.ex.point(failed, ('start',)))


class TestQueries(Case):
    def test_point_queries(self):
        extra = '''
          query atMenu  = forall run: at(menu(claimMenu), K(D, regime == boom) or K(D, regime == slump))
          query atEnd   = forall run: at(end, K(D, regime == boom) or K(D, regime == slump))
          query atStart = exists run: at(start, K(J, regime == boom) or K(J, regime == slump))
          query nobody  = can {}: at(end, remain == 0[KRW])
          query before  = forall run: at(funded, bal(D, KRW) == 100[KRW]) or at(funded, bal(D, KRW) == 0[KRW])
          query proofAt = forall run: at(menu(claimMenu), not known(proof)) and at(funded, known(proof))
        '''
        got = load(with_queries(MODELS['Repay80'], extra), PRELUDE).results()
        self.assertEqual(got['atMenu'], '{early=false: false, early=true: true}')
        self.assertEqual(got['atEnd'], '{early=false: true, early=true: true}')
        self.assertEqual(got['atStart'], '{early=false: false, early=true: false}')
        self.assertEqual(got['nobody'], got['always'])
        self.assertEqual(got['before'], '{early=false: true, early=true: true}')
        self.assertEqual(got['proofAt'], '{early=false: true, early=true: true}')

        stuck = load(model('''
          actor A
          time 0..0
          cell g : Bool init false
          event never by A { require false }
          program { label first  run never  label second }
          query allNotG  = forall run: at(end, not g)
          query someG    = exists run: at(end, g)
          query ends     = exists run: reach(end)
          query started  = forall run: reach(first) and not reach(second)
          query fallback = max run: at(end, 5) ?? 7
        '''))
        self.assertEqual(stuck.results(), {'allNotG': 'false', 'someG': 'false', 'ends': 'false',
                                           'started': 'true', 'fallback': '7'})
        tiny = load(MODELS['Repay80'], PRELUDE, budget=5)
        self.assertEqual(tiny.text('possible'), '{early=false: UNKNOWN, early=true: UNKNOWN}')
        self.assertEqual(tiny.text('canD'), '{} unknown {false, true}')         # undecided values are kept apart

    def test_declared_weight_value_prefer(self):
        extra = '''
          weight zero = 0
          query maximin = region pn: prefer Senior at menu(workout): liquidate over defer by toSenior
          query average = expect chance: min run: toJunior
          query broken  = expect zero: min run: toSenior
        '''
        workout = load(with_queries(MODELS['Workout'], extra), PRELUDE)
        self.assertEqual(workout.text('seniorLiquidates'), '{0..59}')
        self.assertEqual(workout.text('juniorDefers'), '{51..60}')
        self.assertEqual(workout.text('juniorLiquidates'), '{0..49}')
        self.assertEqual(workout.text('maximin'), '{0..60}')
        for name in ('juniorDefers', 'juniorLiquidates'):                           # indifferent at pn = 50
            self.assertIs(workout.evaluate({'pn': 50}, workout.spec.queries[name][2]), False)
        junior = workout.result('average')[2]
        self.assertEqual((junior[(0,)], junior[(60,)], junior[(30,)]), (0, 25, Fraction(25, 2)))
        with self.assertRaises(ModelErr) as caught:
            workout.text('broken')
        self.assertEqual(caught.exception.code, 'D-Weight')

        repay = load(with_queries(MODELS['Repay80'], '''
          query best  = expect prior: max run: paid
          query worstAll = expect prior: min run: paid
          query cashAll  = min run: cash
        '''), PRELUDE)
        self.assertEqual(repay.result('chance'), ('table', ['early'], {(False,): Fraction(1, 2), (True,): Fraction(1)}))
        self.assertEqual(repay.result('worst'), ('table', ['early'], {(False,): 0, (True,): 100}))
        self.assertEqual(repay.text('best'), '{early=false: 1, early=true: 1}')
        self.assertEqual(repay.text('worstAll'), '{early=false: 0, early=true: 0}')
        self.assertEqual(repay.text('cashAll'), '{early=false: 0[KRW], early=true: 0[KRW]}')
        early = repay.analysis({'early': True})
        self.assertEqual(len(list(early.policies({'D'}))), 4)
        self.assertEqual(len(list(repay.analysis({'early': False}).policies({'D'}))), 2)
        self.assertEqual(early.probs('prior'), [Fraction(1, 2), Fraction(1, 2)])

    def test_document_example_end_to_end(self):
        total = 0
        for name in DOC_MODELS:
            with self.subTest(model=name):
                family = load(MODELS[name], PRELUDE)
                expected = expectations(MODELS[name])
                wide = [s for s in family.spec.solves if family.wide(s)]    # written as regions by the solver
                got = family.results()
                self.assertEqual(list(got), list(expected))
                self.assertEqual({k: v for k, v in got.items() if k not in wide},
                                 {k: v for k, v in expected.items() if k not in wide})
                total += len(expected) - len(wide)
        self.assertEqual(total, 42)


class TestSolves(Case):
    def test_solves_by_enumeration(self):
        # an unknown: every value is tried, and the answer is what arithmetic says (repaid exactly from 80 up)
        capital = load(MODELS['Capital'], PRELUDE)
        self.assertEqual(capital.solve('enough').verdicts, {(cap,): cap >= 80 for cap in range(201)})
        short = capital.solve('short')
        self.assertEqual(list(short.verdicts), [(cap,) for cap in range(151)])     # `where` filters before anything runs
        self.assertEqual((short.best, short.value), ((79,), 79))
        self.assertEqual((capital.case('need'), capital.case('enough'), capital.case('spare')), ((80,), (80,), (100,)))
        for solve in capital.spec.solves:                               # a plain query in a case is the query run there
            (cap,) = capital.case(solve)
            direct = Analysis(Machine(capital.spec, {'cap': cap}))
            for q in capital.spec.queries.values():
                self.assertEqual(capital.instance(solve, {}, (cap,)).query(q), direct.query(q))
        self.assertEqual(capital.result('repaid'),
                         ('cases', [('need', True), ('enough', True), ('spare', True), ('short', False)]))

        # the state at a point: every state of `ready` in range, judged by hand
        escrow = load(MODELS['Escrow'], PRELUDE)
        want = {}
        for now, due, paid, b, s in product(range(2), range(4), range(2), range(4), range(4)):
            want[(now, due, paid, b, s)] = (s + due <= 3) if b >= due else bool(paid)   # pays if it can; S must have room
        pre = escrow.solve('pre')
        self.assertEqual(pre.verdicts, want)
        self.assertEqual([v.name for v in pre.vars[(0, 0, 0, 0, 0)]], ['now', 'due', 'paid', 'bal(B, KRW)', 'bal(S, KRW)'])
        self.assertTrue(escrow.wide('pre') and not escrow.wide('least') and not capital.wide('enough'))
        self.assertEqual(escrow.case('cheap'), (0, 2, 0, 2, 0))
        self.assertEqual(escrow.solve('cheap').value, 2)
        self.assertIsNone(escrow.case('full'))
        started = escrow.instance('cheap', {}, (0, 2, 0, 2, 0))          # the case begins at the point, in that state
        self.assertEqual([n.step for n in started.ex.nodes],
                         [('init', 0), ('tick', 1), ('commit', 'pay'), ('end',)])
        self.assertIs(started.ex.point(started.leaves[0], ('label', 'ready')), started.ex.roots[0])
        self.assertEqual(escrow.text('settled'), 'true')                # no unknown: plain queries as before

        # values at which the model is not valid are not solutions, and do not stop the others
        limits = load(SOLVE_MODELS['Limits'], PRELUDE)
        for (n, lim), verdict in limits.solve('ok').verdicts.items():
            broken = n > lim or n < 3 or (n, lim) == (7, 8)             # W-Range, D-Move, D-Init
            self.assertEqual(verdict, INVALID if broken else n >= 4, (n, lim))
        self.assertEqual(limits.solve_text('low'), 'none')
        self.assertEqual(limits.solve_text('biggest'), '8 at n=8, lim=8')
        self.assertEqual(limits.text('moved'), '{ok: true, low: none, biggest: true, still: false}')
        pickup = load(SOLVE_MODELS['Pickup'], PRELUDE)
        late = [v for p, v in pickup.solve('reach3').verdicts.items() if p[0] == 2]     # the clock has passed @1
        self.assertEqual((len(late), late.count(INVALID), late.count(True)), (100, 67, 0))

        # every form, with parameters: one answer per assignment; plain queries only where there is a case
        tiers = load(SOLVE_MODELS['Tiers'], PRELUDE)
        self.assertEqual(tiers.solve_text('need'), '{strict=false, extra=0: 20 at cap=20, strict=false, extra=1: '
                         '25 at cap=25, strict=true, extra=0: 30 at cap=30, strict=true, extra=1: 35 at cap=35}')
        self.assertEqual(tiers.solve_text('band'), '{strict=false, extra=0: {20..33}, strict=false, extra=1: {25..33}, '
                         'strict=true, extra=0: {30..33}, strict=true, extra=1: {}}')
        self.assertEqual(tiers.text('zone'), '{need: {extra=0: {false, true}, extra=1: {false, true}}, band: {extra=0: '
                         '{false, true}, extra=1: {false}}, slim: {extra=0: {false}}, nope: {}}')
        self.assertEqual(load(SOLVE_MODELS['Signal'], PRELUDE).solve_text('best'), '10[KRW] at since=@0, bonus=6')
        self.assertEqual(load(SOLVE_MODELS['Kinds'], PRELUDE).solve_text('late'),
                         '{(off, true, @0, B), (off, true, @1, B), (off, true, @2, B)}')

        # the models aimed at one thing each: the answers are what arithmetic says
        for name, want in SOLVE_EXPECTED.items():
            with self.subTest(aimed=name):
                got = load(SOLVE_MODELS[name], PRELUDE).results()
                self.assertEqual({k: got[k] for k in want}, want)

        # more candidates than the limit: nothing is tried, and there is no case to answer plain queries in
        few = load(MODELS['Capital'], PRELUDE, limit=200)
        self.assertEqual(few.results(), {'repaid': '{need: none, enough: none, spare: none, short: none}',
                                         'left': '{need: none, enough: none, spare: none, short: none}',
                                         'need': 'UNKNOWN', 'enough': 'UNKNOWN', 'spare': 'UNKNOWN', 'short': 'UNKNOWN'})
        self.assertEqual(load(MODELS['Capital'], PRELUDE, limit=201).solve_text('need'), '80 at cap=80')

        # a search limit leaves candidates undecided: kept apart in a set, and UNKNOWN where they could change the answer
        tiny = load(SOLVE_MODELS['Pickup'], PRELUDE, budget=3)
        verdicts = list(tiny.solve('reach3').verdicts.values())
        self.assertEqual((verdicts.count(False), verdicts.count(INVALID), verdicts.count(UNKNOWN)), (99, 67, 134))
        self.assertTrue(tiny.solve_text('reach3').startswith('{} unknown {(@0, 0, 0, 0), '))
        self.assertEqual(tiny.solve_text('sharp'), 'UNKNOWN')            # no solution found, and some were not tried
        self.assertEqual(tiny.solve_text('dull'), 'now=@0, step=3, bal(T, KRW)=2, bal(U, KRW)=0')   # nothing open before it
        self.assertEqual(tiny.solve_text('flat'), 'UNKNOWN')            # an optimum needs every candidate
        whole = load(SOLVE_MODELS['Pickup'], PRELUDE)
        self.assertEqual(whole.solve_text('dull'), tiny.solve_text('dull'))
        self.assertEqual(whole.solve_text('flat'), '0[KRW] at now=@0, step=3, bal(T, KRW)=2, bal(U, KRW)=0')


if __name__ == '__main__':
    unittest.main()
