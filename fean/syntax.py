"""FEAN v0.2 surface syntax: tokens and a recursive-descent parser.

Grammar: docs/financial-language.md section 1.1. Map unit: I-05-b.
Expressions, clauses and statements are tuples whose first item is a tag;
declarations are dicts with 'kind' and 'line'.
"""
import re


class FeanError(Exception):
    """Base error. `code` is a rule name from the specification."""

    def __init__(self, code, message, line=None):
        where = f' (line {line})' if line else ''
        super().__init__(f'[{code}] {message}{where}')
        self.code, self.message, self.line = code, message, line


class SyntaxErr(FeanError):
    """The text is not in the language (S-Lex, S-Parse)."""


class StaticErr(FeanError):
    """A type or structure rule is broken; found before execution."""


class ModelErr(FeanError):
    """INVALID_MODEL that only shows when the model is instantiated or run (D-*)."""


class SearchLimit(Exception):
    """The exploration budget was exceeded; the verdict is UNKNOWN."""


KEYWORDS = frozenset('''
model actor asset enum time param cell slot assume invariant def event contract instance batch proc
program weight value query in init write observe all for by net
auth require given let ensure move tell asof forget free
run attempt else at alt or choose menu do label
if then and not true false empty now known floor ceil exact min max mem recv bal post K reach start end
exists forall can policy expect region prefer over under POSSIBLE ALL CAN
unknown solve find any where
Bool Time Actor Asset Num Qty
'''.split())

TOKEN = re.compile(r'''
    (?P<ws>\s+|//[^\n]*)
  | (?P<time>@[0-9]+)
  | (?P<int>[0-9]+)
  | (?P<id>[A-Za-z_][A-Za-z0-9_]*)
  | (?P<sym>\.\.|:=|\+=|-=|->|=>|==|!=|<=|>=|\?\?|[{}()\[\],:.<>+\-*/=?])
''', re.X)

COMPARE = ('==', '!=', '<', '<=', '>', '>=')
BASE_TYPES = {'Bool': ('Bool',), 'Time': ('Time',), 'Actor': ('Actor',), 'Asset': ('Asset',)}


def tokenize(src):
    out, pos, line = [], 0, 1
    while pos < len(src):
        m = TOKEN.match(src, pos)
        if m is None:
            raise SyntaxErr('S-Lex', f'unexpected character {src[pos]!r}', line)
        kind, text = m.lastgroup, m.group()
        if kind != 'ws':
            if kind == 'id' and text in KEYWORDS:
                kind = 'kw'
            out.append((kind, text, line))
        line += text.count('\n')
        pos = m.end()
    out.append(('eof', '', line))
    return out


class Parser:
    def __init__(self, src):
        self.toks = tokenize(src)
        self.i = 0

    # ---- tokens -------------------------------------------------------------
    def peek(self, k=0):
        return self.toks[min(self.i + k, len(self.toks) - 1)]

    def at(self, text, k=0):
        kind, s, _ = self.peek(k)
        return s == text and kind in ('kw', 'sym')

    def accept(self, text):
        if self.at(text):
            self.i += 1
            return True
        return False

    def expect(self, text):
        if not self.accept(text):
            self.fail(f'expected {text!r}')

    def fail(self, msg):
        kind, s, line = self.peek()
        found = 'end of input' if kind == 'eof' else repr(s)
        raise SyntaxErr('S-Parse', f'{msg}, found {found}', line)

    def name(self):
        kind, s, _ = self.peek()
        if kind != 'id':
            self.fail('expected a name')
        self.i += 1
        return s

    def names(self):
        out = [self.name()]
        while self.accept(','):
            out.append(self.name())
        return out

    def qname(self):
        parts = [self.name()]
        while self.accept('.'):
            parts.append(self.name())
        return '.'.join(parts)

    def integer(self):
        kind, s, _ = self.peek()
        if kind != 'int':
            self.fail('expected an integer')
        self.i += 1
        return int(s)

    def args_until(self, close):
        out = []
        if not self.accept(close):
            out.append(self.expr())
            while self.accept(','):
                out.append(self.expr())
            self.expect(close)
        return out

    # ---- declarations -------------------------------------------------------
    def model(self):
        self.expect('model')
        name = self.name()
        self.expect('{')
        decls = []
        while not self.at('}'):
            decls.append(self.decl())
        self.expect('}')
        if self.peek()[0] != 'eof':
            self.fail('expected end of input')
        return name, decls

    def decls_only(self):
        out = []
        while self.peek()[0] != 'eof':
            out.append(self.decl())
        return out

    def decl(self, members_only=False):
        kind, text, line = self.peek()
        method = getattr(self, 'd_' + text, None) if kind == 'kw' else None
        if method is None or (members_only and text not in ('cell', 'event', 'batch', 'proc', 'instance')):
            self.fail('expected a contract member' if members_only else 'expected a declaration')
        self.i += 1
        d = method()
        d['kind'], d['line'] = text, line
        return d

    def d_actor(self):
        return {'names': self.names()}

    def d_asset(self):
        name = self.name()
        self.expect('in')
        lo = self.sum()
        self.expect('..')
        hi = self.sum()
        init = []
        if self.accept('init'):
            while True:
                who = self.name()
                if self.accept('='):
                    init.append((who, [self.expr()]))
                else:
                    self.expect('in')
                    self.expect('{')
                    init.append((who, self.args_until('}')))
                if not self.accept(','):
                    break
        public = False
        if self.accept('observe'):
            self.expect('all')
            public = True
        return {'name': name, 'lo': lo, 'hi': hi, 'init': init, 'public': public}

    def d_enum(self):
        name = self.name()
        self.expect('{')
        values = self.names()
        self.expect('}')
        return {'name': name, 'values': values}

    def d_time(self):
        lo = self.integer()
        self.expect('..')
        return {'lo': lo, 'hi': self.integer()}

    def d_param(self):
        name = self.name()
        self.expect(':')
        d = {'name': name, 'type': self.type_(), 'lo': None, 'hi': None}
        if self.accept('in'):
            d['lo'] = self.sum()
            self.expect('..')
            d['hi'] = self.sum()
        return d

    d_unknown = d_param

    def d_solve(self):
        """solve NAME = (find | any | (min | max) e) [at(L)] [where c] : goal"""
        name = self.name()
        self.expect('=')
        objective = label = where = None
        if self.at('find') or self.at('any') or self.at('min') or self.at('max'):
            form = self.peek()[1]
            self.i += 1
            if form in ('min', 'max'):
                objective = self.expr()
        else:
            self.fail('expected find, any, min or max')
        if self.accept('at'):
            self.expect('(')
            label = self.qname()
            self.expect(')')
        if self.accept('where'):
            where = self.expr()
        self.expect(':')
        return {'name': name, 'solve': ('solve', form, objective, label, where, self.expr())}

    def d_slot(self):
        name = self.name()
        self.expect(':')
        ty = self.type_()
        self.expect('for')
        return {'name': name, 'type': ty, 'for': self.names()}

    def d_assume(self):
        return {'expr': self.expr()}

    d_invariant = d_assume

    def d_def(self):
        name = self.name()
        self.expect('(')
        params = self.params(')')
        self.expect(':')
        ty = self.type_()
        self.expect('=')
        return {'name': name, 'params': params, 'type': ty, 'body': self.expr()}

    def d_contract(self):
        name = self.name()
        self.expect('(')
        params = self.params(')')
        self.expect('{')
        members = []
        while not self.at('}'):
            members.append(self.decl(members_only=True))
        self.expect('}')
        return {'name': name, 'params': params, 'members': members}

    def d_program(self):
        return {'body': self.block()}

    def d_weight(self):
        name = self.name()
        self.expect('=')
        return {'name': name, 'expr': self.expr()}

    d_value = d_weight

    def d_query(self):
        name = self.name()
        self.expect('=')
        return {'name': name, 'query': self.query()}

    def d_cell(self):
        name = self.name()
        self.expect(':')
        d = {'name': name, 'type': self.type_(), 'lo': None, 'hi': None, 'write': [], 'observe': []}
        if self.accept('in'):
            d['lo'] = self.sum()
            self.expect('..')
            d['hi'] = self.sum()
        self.expect('init')
        if self.accept('in'):
            self.expect('{')
            d['init'] = self.args_until('}')
        else:
            d['init'] = [self.expr()]
        if self.accept('write'):
            d['write'] = [self.qname()]
            while self.accept(','):
                d['write'].append(self.qname())
        if self.accept('observe'):
            d['observe'] = 'all' if self.accept('all') else self.names()
        return d

    def d_event(self):
        name = self.name()
        params = []
        if self.accept('('):
            params = self.params(')')
        self.expect('by')
        by = [self.expr()]
        while self.accept(','):
            by.append(self.expr())
        net = self.accept('net')
        self.expect('{')
        clauses = []
        while not self.at('}'):
            clauses.append(self.clause())
        self.expect('}')
        return {'name': name, 'params': params, 'by': by, 'net': net, 'clauses': clauses}

    def d_batch(self):
        name = self.name()
        self.expect('=')
        net = self.accept('net')
        self.expect('{')
        members = [self.call()]
        while self.accept(','):
            members.append(self.call())
        self.expect('}')
        return {'name': name, 'net': net, 'members': members}

    def d_proc(self):
        name = self.name()
        self.expect('(')
        params = self.params(')')
        return {'name': name, 'params': params, 'body': self.block()}

    def d_instance(self):
        name = self.name()
        self.expect('=')
        contract = self.name()
        self.expect('(')
        return {'name': name, 'contract': contract, 'args': self.args_until(')')}

    def params(self, close):
        out = []
        if not self.accept(close):
            while True:
                name = self.name()
                self.expect(':')
                out.append((name, self.type_()))
                if not self.accept(','):
                    break
            self.expect(close)
        return out

    def type_(self):
        kind, text, _ = self.peek()
        if kind == 'kw' and text in BASE_TYPES:
            self.i += 1
            ty = BASE_TYPES[text]
        elif kind == 'kw' and text == 'Num':
            self.i += 1
            ty = ('Qty', [])
        elif kind == 'kw' and text == 'Qty':
            self.i += 1
            self.expect('[')
            ty = ('Qty', self.unit())
            self.expect(']')
        else:
            ty = ('Enum', self.name())
        return ('Opt', ty) if self.accept('?') else ty

    def unit(self):
        out = [(self.name(), 1)]
        while self.at('*') or self.at('/'):
            sign = 1 if self.peek()[1] == '*' else -1
            self.i += 1
            out.append((self.name(), sign))
        return out

    # ---- events -------------------------------------------------------------
    def clause(self):
        kind, text, _ = self.peek()
        if kind == 'kw' and text != 'bal':
            self.i += 1
            if text in ('auth', 'require', 'ensure'):
                return (text, self.expr())
            if text in ('given', 'let'):
                x = self.name()
                self.expect('=')
                return (text, x, self.expr())
            if text == 'free':
                return ('free', self.target())
            if text == 'move':
                a = self.expr()
                self.expect('->')
                b = self.expr()
                self.expect(':')
                return ('move', a, b, self.expr())
            if text == 'tell':
                who = self.name()
                self.expect('.')
                slot = self.name()
                self.expect(':=')
                v = self.expr()
                self.expect('asof')
                return ('tell', who, slot, v, self.expr())
            if text == 'forget':
                who = self.name()
                self.expect('.')
                return ('forget', who, self.name())
            self.i -= 1
            self.fail('expected a clause')
        tgt = self.target()
        if self.accept(':='):
            return ('set', tgt, self.expr())
        if self.accept('+='):
            return ('delta', tgt, 1, self.expr())
        if self.accept('-='):
            return ('delta', tgt, -1, self.expr())
        self.fail("expected ':=', '+=' or '-='")

    def target(self):
        if self.accept('bal'):
            self.expect('(')
            who = self.name()
            self.expect(',')
            asset = self.name()
            self.expect(')')
            return ('balc', who, asset)
        return ('cell', self.qname())

    # ---- programs -----------------------------------------------------------
    def block(self):
        self.expect('{')
        out = []
        while not self.at('}'):
            out.append(self.stmt())
        self.expect('}')
        return out

    def call(self):
        q = self.qname()
        return (q, self.args_until(')') if self.accept('(') else [])

    def unitref(self):
        if self.accept('batch'):
            return ('batch', self.qname())
        q, args = self.call()
        return ('ev', q, args)

    def stmt(self):
        if self.accept('run'):
            return ('run', self.unitref())
        if self.accept('attempt'):
            unit = self.unitref()
            yes = self.block()
            return ('attempt', unit, yes, self.block() if self.accept('else') else [])
        if self.accept('at'):
            t = self.expr()
            return ('at', t, self.block())
        if self.accept('alt'):
            blocks = [self.block()]
            self.expect('or')
            blocks.append(self.block())
            while self.accept('or'):
                blocks.append(self.block())
            return ('alt', blocks)
        if self.accept('choose'):
            who = self.name()
            self.expect('menu')
            menu = self.name()
            self.expect('{')
            actions = [self.action()]
            while not self.at('}'):
                actions.append(self.action())
            self.expect('}')
            return ('choose', who, menu, actions)
        if self.accept('do'):
            q = self.qname()
            self.expect('(')
            return ('do', q, self.args_until(')'))
        if self.accept('label'):
            return ('label', self.name())
        self.fail('expected a statement')

    def action(self):
        label, param = self.name(), None
        if self.accept('('):
            x = self.name()
            self.expect('in')
            if self.accept('{'):
                values = self.args_until('}')
            else:
                lo = self.sum()
                self.expect('..')
                values = ('range', lo, self.sum())
            self.expect(')')
            param = (x, values)
        self.expect('=>')
        return (label, param, self.block())

    # ---- expressions --------------------------------------------------------
    def expr(self):
        e = self.conj()
        while self.accept('or'):
            e = ('bin', 'or', e, self.conj())
        return e

    def conj(self):
        e = self.neg()
        while self.accept('and'):
            e = ('bin', 'and', e, self.neg())
        return e

    def neg(self):
        if self.accept('not'):
            return ('not', self.neg())
        return self.cmp()

    def cmp(self):
        a = self.sum()
        kind, text, _ = self.peek()
        if kind == 'sym' and text in COMPARE:
            self.i += 1
            b = self.sum()
            kind2, text2, _ = self.peek()
            if kind2 == 'sym' and text2 in COMPARE:
                self.fail('comparisons do not chain')
            return ('bin', text, a, b)
        return a

    def sum(self):
        e = self.prod()
        while self.at('+') or self.at('-'):
            op = self.peek()[1]
            self.i += 1
            e = ('bin', op, e, self.prod())
        return e

    def prod(self):
        e = self.unary()
        while self.accept('*'):
            e = ('bin', '*', e, self.unary())
        return e

    def unary(self):
        if self.accept('-'):
            return ('neg', self.unary())
        e = self.atom()
        while self.accept('??'):
            e = ('default', e, self.atom())
        return e

    def atom(self):
        kind, text, _ = self.peek()
        if kind == 'int':
            self.i += 1
            if self.accept('['):
                unit = self.unit()
                self.expect(']')
                return ('qty', int(text), unit)
            return ('int', int(text))
        if kind == 'time':
            self.i += 1
            return ('time', int(text[1:]))
        if kind == 'id':
            q = self.qname()
            if self.accept('('):
                return ('call', q, self.args_until(')'))
            return ('name', q)
        if kind == 'sym' and text == '(':
            self.i += 1
            e = self.expr()
            self.expect(')')
            return e
        if kind == 'kw':
            self.i += 1
            if text in ('true', 'false'):
                return ('bool', text == 'true')
            if text == 'empty':
                return ('empty',)
            if text == 'now':
                return ('now',)
            if text == 'if':
                c = self.expr()
                self.expect('then')
                a = self.expr()
                self.expect('else')
                return ('if', c, a, self.expr())
            if text in ('floor', 'ceil', 'exact'):
                self.expect('(')
                a = self.sum()
                self.expect('/')
                b = self.sum()
                self.expect(')')
                return ('div', text, a, b)
            if text in ('min', 'max'):
                self.expect('(')
                a = self.expr()
                self.expect(',')
                b = self.expr()
                self.expect(')')
                return ('minmax', text, a, b)
            if text == 'known':
                self.expect('(')
                e = self.expr()
                self.expect(')')
                return ('known', e)
            if text in ('mem', 'asof', 'recv'):
                self.expect('(')
                who = self.name()
                self.expect(',')
                slot = self.name()
                self.expect(')')
                return (text, who, slot)
            if text == 'bal':
                self.expect('(')
                who = self.expr()
                self.expect(',')
                asset = self.name()
                self.expect(')')
                return ('bal', who, asset)
            if text == 'post':
                self.expect('(')
                tgt = self.target()
                self.expect(')')
                return ('post', tgt)
            if text == 'K':
                self.expect('(')
                who = self.name()
                self.expect(',')
                e = self.expr()
                self.expect(')')
                return ('K', who, e)
            if text == 'at':
                self.expect('(')
                p = self.point()
                self.expect(',')
                e = self.expr()
                self.expect(')')
                return ('at', p, e)
            if text == 'reach':
                self.expect('(')
                p = self.point()
                self.expect(')')
                return ('reach', p)
            self.i -= 1
        self.fail('expected an expression')

    def point(self):
        if self.accept('start'):
            return ('start',)
        if self.accept('end'):
            return ('end',)
        if self.accept('menu'):
            self.expect('(')
            q = self.qname()
            self.expect(')')
            return ('menu', q)
        return ('label', self.qname())

    # ---- queries ------------------------------------------------------------
    def coalition(self):
        self.expect('{')
        out = [] if self.at('}') else self.names()
        self.expect('}')
        return out

    def query(self):
        if self.accept('exists') or self.at('forall'):
            kind = 'forall' if self.accept('forall') else 'exists'
            self.expect('run')
            self.expect(':')
            return (kind, self.expr())
        if self.accept('can'):
            who = self.coalition()
            self.expect(':')
            return ('can', who, self.expr())
        if self.accept('region'):
            params = self.names()
            self.expect(':')
            return ('region', params, self.query())
        if self.accept('prefer'):
            who = self.name()
            self.expect('at')
            p = self.point()
            self.expect(':')
            a = self.name()
            self.expect('over')
            b = self.name()
            self.expect('by')
            v = self.name()
            return ('prefer', who, p, a, b, v, self.name() if self.accept('under') else None)
        if self.at('POSSIBLE') or self.at('ALL'):
            kind = 'exists' if self.peek()[1] == 'POSSIBLE' else 'forall'
            self.i += 1
            self.expect('(')
            goal = self.expr()
            self.expect(')')
            return (kind, ('at', ('end',), goal))
        if self.accept('CAN'):
            self.expect('(')
            who = self.coalition()
            self.expect(',')
            goal = self.expr()
            self.expect(')')
            return ('can', who, ('at', ('end',), goal))
        policy = weight = None
        if self.at('max') and self.at('policy', 1):
            self.i += 2
            policy = self.coalition()
            self.expect(':')
        if self.accept('expect'):
            weight = self.name()
            self.expect(':')
        if self.at('max') or self.at('min'):
            opt = self.peek()[1]
            self.i += 1
            self.expect('run')
            self.expect(':')
            return ('num', policy, weight, opt, self.expr())
        self.fail('expected a query')


def parse_model(src):
    """Return (model name, declarations)."""
    return Parser(src).model()


def parse_decls(src):
    """Parse bare declarations (the standard prelude)."""
    return Parser(src).decls_only()
