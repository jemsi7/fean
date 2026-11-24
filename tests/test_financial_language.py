"""Finite witnesses for the examples in docs/financial-language.md (not a parser)."""

from collections import Counter
from dataclasses import dataclass, replace
from fractions import Fraction
from itertools import product
import unittest


@dataclass(frozen=True)
class Qty:
    unit: str
    n: int

    def __add__(self, other):
        if self.unit != other.unit:
            raise TypeError("incompatible units")
        return Qty(self.unit, self.n + other.n)


def post_legs(before, legs, *, net=False):
    """Return a new ledger. In the gross rule, outgoing funds must exist beforehand."""
    outgoing, incoming = Counter(), Counter()
    for sender, receiver, amount in legs:
        if amount.n < 0:
            raise ValueError("negative transfer")
        outgoing[sender, amount.unit] += amount.n
        incoming[receiver, amount.unit] += amount.n
    after = dict(before)
    for key in set(outgoing) | set(incoming):
        start = before.get(key, 0)
        if not net and outgoing[key] > start:
            raise ValueError("gross funds unavailable")
        after[key] = start + incoming[key] - outgoing[key]
        if after[key] < 0:
            raise ValueError("negative closing balance")
    return after


@dataclass(frozen=True)
class Claim:
    holder: str
    debtor: str
    remaining: Qty


def pay(ledger, claim, amount):
    if claim.remaining.unit != amount.unit:
        raise TypeError("wrong settlement asset")
    if not 0 < amount.n <= claim.remaining.n:
        raise ValueError("payment outside claim")
    after = post_legs(ledger, [(claim.debtor, claim.holder, amount)])
    return after, replace(claim, remaining=Qty(amount.unit, claim.remaining.n - amount.n))


class TestContracts(unittest.TestCase):
    def test_loan_lifecycle(self):
        cash = {("lender", "KRW"): 100}
        originated = post_legs(cash, [("lender", "debtor", Qty("KRW", 100))])
        claim = Claim("lender", "debtor", Qty("KRW", 105))
        after, residual = pay(originated, claim, Qty("KRW", 60))
        self.assertEqual((after["lender", "KRW"], residual.remaining.n), (60, 45))
        self.assertEqual(originated["lender", "KRW"], 0)
        assigned = replace(residual, holder="new_holder")
        after, assigned = pay(after, assigned, Qty("KRW", 40))
        self.assertEqual((after["new_holder", "KRW"], assigned.remaining.n), (40, 5))
        with self.assertRaises(ValueError):
            pay(after, assigned, Qty("KRW", 5))

    def test_nested_call(self):
        initial = {("holder", "KRW"): 105, ("writer", "share"): 1}
        after_premium = post_legs(initial, [("holder", "writer", Qty("KRW", 5))])
        exercise = [("writer", "holder", Qty("share", 1)),
                    ("holder", "writer", Qty("KRW", 100))]
        expiry = post_legs(after_premium, [])
        delivery = post_legs(after_premium, exercise)
        self.assertEqual((expiry["holder", "KRW"], expiry["writer", "share"]), (100, 1))
        self.assertEqual((delivery["writer", "KRW"], delivery["holder", "share"]), (105, 1))

    def test_units_and_capacity(self):
        with self.assertRaises(TypeError):
            _ = Qty("KRW", 2) + Qty("share", 1)
        empty = {("a", "KRW"): 0}
        with self.assertRaises(ValueError):
            post_legs(empty, [("a", "b", Qty("KRW", 1))])
        self.assertEqual(empty, {("a", "KRW"): 0})
        with self.assertRaises(ValueError):
            pay(empty, Claim("b", "a", Qty("KRW", 1)), Qty("KRW", 2))


@dataclass(frozen=True)
class Receipt:
    value: str
    as_of: int
    received: int


@dataclass(frozen=True)
class Point:
    regime: str
    clock: int
    memory: dict
    evidence: dict
    menus: dict

    def tell(self, actor, key, value, as_of):
        memory = dict(self.memory)
        memory[actor, key] = Receipt(value, as_of, self.clock)
        return replace(self, memory=memory)

    def forget(self, actor, key):
        return replace(self, memory={k: v for k, v in self.memory.items() if k != (actor, key)})

    def view(self, actor):
        slots = tuple(sorted((k, v) for (owner, k), v in self.memory.items() if owner == actor))
        return self.clock, self.menus.get(actor), slots


def decision_points(early):
    points = []
    for regime in ("boom", "slump"):
        p = Point(regime, 0, {}, {}, {}).tell("J", "report", regime, 0)
        if early:
            p = replace(p, clock=1).tell("D", "report", regime, 0)
        points.append(replace(p, clock=2, menus={"D": ("claim", ("A", "B"))}))
    return points


def knowledge(points, actual, actor, proposition):
    possibilities = {proposition(p) for p in points if actual.view(actor) == p.view(actor)}
    if not possibilities:
        raise ValueError("observation outside model")
    return {frozenset({True}): "known true", frozenset({False}): "known false",
            frozenset({False, True}): "undetermined"}[frozenset(possibilities)]


class TestKnowledge(unittest.TestCase):
    def test_private_delivery_at_cut(self):
        boom = lambda p: p.regime == "boom"
        early, late = decision_points(True), decision_points(False)
        self.assertEqual(knowledge(early, early[0], "D", boom), "known true")
        self.assertEqual(knowledge(early, early[1], "D", boom), "known false")
        self.assertEqual(knowledge(late, late[0], "D", boom), "undetermined")
        for points in (early, late):
            self.assertEqual(knowledge(points, points[0], "J", boom), "known true")
            self.assertEqual(knowledge(points, points[0], "L", boom), "undetermined")

    def test_report_and_certificate(self):
        reports = [p.tell("D", "report", "boom", 0) for p in decision_points(False)]
        reports[0] = replace(reports[0], evidence={("J", "regime"): "boom"})
        self.assertEqual(reports[1].memory["D", "report"].value, "boom")
        self.assertEqual(knowledge(reports, reports[1], "D", lambda p: p.regime == "boom"),
                         "undetermined")
        self.assertEqual(reports[0].evidence.get(("J", "regime")), "boom")
        self.assertIsNone(reports[1].evidence.get(("J", "regime")))

    def test_retention(self):
        early = decision_points(True)
        erased = [replace(p, clock=3).forget("D", "report") for p in early]
        self.assertEqual(early[0].memory["D", "report"], Receipt("boom", 0, 1))
        self.assertNotIn(("D", "report"), erased[0].memory)
        self.assertEqual(knowledge(erased, erased[0], "D", lambda p: p.regime == "boom"),
                         "undetermined")
        late = decision_points(False)[0]
        informed = replace(late, clock=3).tell("D", "report", "boom", 0)
        self.assertNotIn(("D", "report"), late.memory)
        self.assertEqual(informed.memory["D", "report"], Receipt("boom", 0, 3))


class TestSettlement(unittest.TestCase):
    def test_atomic_balance(self):
        before = {("a", "KRW"): 100, ("b", "share"): 0}
        exchange = [("a", "b", Qty("KRW", 100)), ("b", "a", Qty("share", 1))]
        with self.assertRaises(ValueError):
            post_legs(before, exchange)
        self.assertEqual(before, {("a", "KRW"): 100, ("b", "share"): 0})
        no_cash = {("a", "KRW"): 0, ("b", "KRW"): 0}
        mutual = [("a", "b", Qty("KRW", 100)), ("b", "a", Qty("KRW", 100))]
        with self.assertRaises(ValueError):
            post_legs(no_cash, mutual)
        self.assertEqual(post_legs(no_cash, mutual, net=True), no_cash)

    def test_cancellation_orders(self):
        def execute(schedule):
            orders = {"high-1": 100, "high-2": 100, "low": 70}
            cash, cancelled = 0, False
            for event in schedule:
                if event == "cancel":
                    cancelled = orders.pop("high-2", None) is not None
                else:
                    bids = sorted(orders, key=orders.get, reverse=True)
                    cash = sum(orders.pop(order) for order in bids[:2])
            return cash, cancelled

        self.assertEqual(execute(("cancel", "sell")), (170, True))
        self.assertEqual(execute(("sell", "cancel")), (200, False))

    def test_clearing_solutions(self):
        candidates = list(product((False, True), (0, 100)))
        post_trigger = [(d, q) for d, q in candidates
                        if d == (q < 100) and q == (100 if d else 0)]
        cash_before = 0
        pre_trigger = [(d, q) for d, q in candidates
                       if d == (q < 100) and q == (100 if cash_before < 100 else 0)]
        self.assertEqual(post_trigger, [])
        self.assertEqual(pre_trigger, [(False, 100)])
        mutual_activation = [(a, b) for a, b in product((0, 100), repeat=2)
                             if a == (100 if b else 0) and b == (100 if a else 0)]
        self.assertEqual(mutual_activation, [(0, 0), (100, 100)])


def repayment_policies(points):
    """Pick one action for each distinct borrower view, then reuse it in every world."""
    if not points:
        raise ValueError("empty initial worlds")
    views = list(dict.fromkeys(p.view("D") for p in points))
    for actions in product(("A", "B"), repeat=len(views)):
        policy = dict(zip(views, actions))
        outcomes = []
        for p in points:
            action = policy[p.view("D")]
            chosen = p.tell("D", "ownChoice", action, 2)
            at_three = replace(chosen, clock=3, menus={})
            report = at_three.memory["J", "report"]
            if ("D", "report") not in at_three.memory:
                at_three = at_three.tell("D", "report", report.value, report.as_of)
            certified = replace(at_three, evidence={("J", "regime"): report.value})
            pair = certified.memory["D", "ownChoice"].value, certified.evidence["J", "regime"]
            award = 100 if pair in {("A", "boom"), ("B", "slump")} else 0
            ledger = post_legs({("I", "KRW"): 100}, [("I", "D", Qty("KRW", award))])
            claim = Claim("L", "D", Qty("KRW", 80))
            funding = ledger["D", "KRW"]
            payment = min(funding, claim.remaining.n)
            if payment:
                ledger, claim = pay(ledger, claim, Qty("KRW", payment))
            outcomes.append((funding, claim.remaining.n, ledger.get(("L", "KRW"), 0)))
        yield outcomes


def terminal_query(operator, terminals, *, complete):
    if operator not in {"ALL", "POSSIBLE"}:
        raise ValueError(operator)
    if complete and not terminals:
        raise ValueError("empty model or missing mandatory failure")
    if operator == "ALL" and any(t != "GOAL" for t in terminals):
        return False
    if operator == "POSSIBLE" and "GOAL" in terminals:
        return True
    if not complete:
        return "UNKNOWN"
    return operator == "ALL"


class TestAnalysis(unittest.TestCase):
    def test_repayment_with_disclosure(self):
        for early in (True, False):
            with self.subTest(early=early):
                policies = list(repayment_policies(decision_points(early)))
                self.assertEqual(len(policies), 4 if early else 2)
                success_counts = [sum(debt == 0 for _, debt, _ in policy) for policy in policies]
                self.assertGreater(max(success_counts), 0)          # POSSIBLE
                self.assertLess(min(success_counts), 2)             # ALL is false
                self.assertEqual(max(success_counts) == 2, early)  # CAN
                worst_funds = [min(funds for funds, _, _ in policy) for policy in policies]
                self.assertEqual(max(worst_funds), 100 if early else 0)
                self.assertEqual(Fraction(max(success_counts), 2), 1 if early else Fraction(1, 2))
                for policy in policies:
                    for _, debt, paid in policy:
                        self.assertEqual(debt + paid, 80)

    def test_queries_with_failure(self):
        for failure in ("BLOCKED", "NO_SETTLEMENT", "GOAL_FALSE"):
            self.assertFalse(terminal_query("ALL", ["GOAL", failure], complete=True))
            self.assertFalse(terminal_query("POSSIBLE", [failure], complete=True))
        self.assertTrue(terminal_query("POSSIBLE", ["GOAL"], complete=False))
        self.assertFalse(terminal_query("ALL", ["BLOCKED"], complete=False))
        self.assertEqual(terminal_query("ALL", ["GOAL"], complete=False), "UNKNOWN")
        self.assertEqual(terminal_query("POSSIBLE", [], complete=False), "UNKNOWN")
        with self.assertRaises(ValueError):
            list(repayment_policies([]))

    def test_safety_region(self):
        prices = range(201)
        safe = {s for s in prices if max(s - 100, 0) + max(100 - s, 0) >= 10}
        self.assertEqual(safe, set(range(91)) | set(range(110, 201)))

    def test_incentive_threshold(self):
        def allocation(cash):
            senior = min(50, cash)
            return senior, min(30, cash - senior)

        now_senior, now_junior = allocation(75)
        good_senior, good_junior = allocation(80)
        bad_senior, bad_junior = allocation(0)
        grid = {Fraction(n, 60) for n in range(61)}
        disagreement = set()
        for prob in grid:
            wait_senior = prob * good_senior + (1 - prob) * bad_senior
            wait_junior = prob * good_junior + (1 - prob) * bad_junior
            if now_senior > wait_senior and now_junior < wait_junior:
                disagreement.add(prob)
            if prob == Fraction(5, 6):
                self.assertEqual(wait_junior, now_junior)
        self.assertEqual(disagreement, {p for p in grid if Fraction(5, 6) < p < 1})


if __name__ == "__main__":
    unittest.main()
