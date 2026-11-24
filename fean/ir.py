"""FEAN image: the intermediate representation that is produced ahead of time and run by the machine.

Tables, instructions and verification rules: docs/engine.md sections 3-5. Map unit: I-06-b.
An image is a plain dict that serialises to JSON. Every reference inside it is a number.
"""
import json

from .syntax import FeanError

FORMAT, VERSION, LANGUAGE = 'feanc', 2, '0.3'


class ImageErr(FeanError):
    """The image is malformed (I-Format, I-Ref, I-Mode, I-Stack)."""


ALL, STATE = 'CSPQRGEVX', 'SPQGEX'
# instruction: (inline arguments, values popped, values pushed, layers that may use it)
OPS = {
    'PUSH': (1, 0, 1, ALL), 'PARAM': (1, 0, 1, ALL), 'LOCAL': (1, 0, 1, ALL), 'STORE': (1, 1, 0, ALL),
    'DUP': (0, 1, 2, ALL), 'POP': (0, 1, 0, ALL),
    'LOAD': (1, 0, 1, STATE), 'BAL': (1, 1, 1, STATE), 'NOW': (0, 0, 1, STATE),
    'MEM': (1, 0, 1, STATE), 'ASOF': (1, 0, 1, STATE), 'RECV': (1, 0, 1, STATE),
    'POST': (1, 0, 1, 'P'),
    'ADD': (0, 2, 1, ALL), 'SUB': (0, 2, 1, ALL), 'MUL': (0, 2, 1, ALL), 'MIN': (0, 2, 1, ALL),
    'MAX': (0, 2, 1, ALL), 'DIVF': (0, 2, 1, ALL), 'DIVC': (0, 2, 1, ALL), 'DIVX': (0, 2, 1, ALL),
    'EQ': (0, 2, 1, ALL), 'NE': (0, 2, 1, ALL), 'LT': (0, 2, 1, ALL), 'LE': (0, 2, 1, ALL),
    'GT': (0, 2, 1, ALL), 'GE': (0, 2, 1, ALL),
    'NEG': (0, 1, 1, ALL), 'NOT': (0, 1, 1, ALL), 'KNOWN': (0, 1, 1, ALL),
    'JMP': (1, 0, 0, ALL), 'JF': (1, 1, 0, ALL), 'JT': (1, 1, 0, ALL),
    'K': (2, 0, 1, 'Q'), 'AT': (4, 0, 1, 'R'), 'REACH': (2, 0, 1, 'R'),
    'REQUIRE': (1, 1, 0, 'G'), 'GIVEN': (2, 1, 0, 'G'),
    'FXSET': (1, 1, 0, 'E'), 'FXADD': (1, 1, 0, 'E'), 'FXFREE': (1, 0, 0, 'E'),
    'FXMOVE': (1, 3, 0, 'E'), 'FXTELL': (1, 2, 0, 'E'), 'FXFORGET': (1, 0, 0, 'E'),
    'UNK': (1, 0, 1, ALL), 'QUERY': (1, 0, 1, 'VX'),
}
# values a finished code block leaves on the stack, per layer
RESULT = {'C': 1, 'S': 1, 'P': 1, 'Q': 1, 'R': 1, 'G': 0, 'E': 0, 'V': 1, 'X': 1}
TABLES = ('model', 'language', 'actors', 'assets', 'enums', 'time', 'params', 'cells', 'slots', 'obs', 'mems',
          'assumes', 'invariants', 'events', 'units', 'blocks', 'entry', 'menus', 'marks', 'actions', 'weights',
          'queries', 'codes', 'unknowns', 'solves')
CELL_OPS = ('LOAD', 'POST', 'FXSET', 'FXADD', 'FXFREE')
ASSET_OPS = ('BAL', 'FXMOVE')
SLOT_OPS = ('MEM', 'ASOF', 'RECV', 'FXTELL', 'FXFORGET')
DOMAINS = ('qty', 'bool', 'enum', 'time', 'actor')
PARAM_KINDS = ('bool', 'enum', 'time', 'actor', 'num')
SOLVE_FORMS = ('find', 'any', 'min', 'max')
POINTS = ('start', 'end', 'menu', 'label')


def verify(image):
    """Check that an image is well formed; return it, or raise ImageErr."""
    if not isinstance(image, dict) or image.get('format') != FORMAT:
        raise ImageErr('I-Format', 'this is not a FEAN image')
    if image.get('version') != VERSION:
        raise ImageErr('I-Format', f'image version {image.get("version")!r} is not {VERSION}')
    for key in TABLES:
        if key not in image:
            raise ImageErr('I-Format', f'table {key} is missing')
    try:
        _Verifier(image).run()
    except (KeyError, TypeError, IndexError, AttributeError, ValueError) as exc:
        raise ImageErr('I-Format', f'a table has the wrong shape ({exc!r})') from None
    return image


class _Verifier:
    def __init__(self, im):
        self.im = im
        self.n = {k: len(im[k]) for k in ('actors', 'assets', 'enums', 'params', 'slots', 'events', 'units',
                                           'blocks', 'menus', 'marks', 'actions', 'weights', 'codes',
                                           'unknowns', 'queries')}
        self.n['cells'] = len(im['cells']) + len(im['assets']) * len(im['actors'])

    def ref(self, i, table, what):
        if isinstance(i, bool) or not isinstance(i, int) or not 0 <= i < self.n[table]:
            raise ImageErr('I-Ref', f'{what}: {table} number {i!r} is out of range')
        return i

    def code(self, i, layers, what, frame=None):
        c = self.im['codes'][self.ref(i, 'codes', what)]
        if c['mode'] not in layers:
            raise ImageErr('I-Mode', f'{what}: code {i} is in layer {c["mode"]}, expected one of {layers}')
        if frame is not None and c['locals'] != frame:
            raise ImageErr('I-Ref', f'{what}: code {i} has {c["locals"]} locals, its event has {frame}')

    def run(self):
        im = self.im
        lo, hi = im['time']
        if not (isinstance(lo, int) and isinstance(hi, int) and lo <= hi):
            raise ImageErr('I-Format', 'the time range is not two ordered integers')
        for a in im['assets']:
            self.code(a['lo'], 'C', f'asset {a["name"]}')
            self.code(a['hi'], 'C', f'asset {a["name"]}')
            if len(a['init']) != self.n['actors'] or not all(a['init']):
                raise ImageErr('I-Format', f'asset {a["name"]} needs initial holdings for every actor')
            for values in a['init']:
                for v in values:
                    self.code(v, 'C', f'asset {a["name"]}')
        for p in im['params']:
            if p['kind'] not in PARAM_KINDS or len(p['values']) != len(p['show']) or not p['values']:
                raise ImageErr('I-Format', f'parameter {p["name"]} is malformed')
        for c in im['cells']:
            self.domain(c['dom'], f'cell {c["name"]}')
            if (c['lo'] is None) != (c['hi'] is None) or (c['dom']['kind'] == 'qty') != (c['lo'] is not None):
                raise ImageErr('I-Format', f'cell {c["name"]}: only quantity cells have a range, and they need one')
            for bound in (c['lo'], c['hi']):
                if bound is not None:
                    self.code(bound, 'C', f'cell {c["name"]}')
            if not c['init']:
                raise ImageErr('I-Format', f'cell {c["name"]} has no initial value')
            for v in c['init']:
                self.code(v, 'C', f'cell {c["name"]}')
        for s in im['slots']:
            self.ref(s['actor'], 'actors', f'slot {s["key"]}')
            self.domain(s['dom'], f'slot {s["key"]}')
        for table, target in (('obs', 'cells'), ('mems', 'slots')):
            if len(im[table]) != self.n['actors']:
                raise ImageErr('I-Format', f'table {table} needs one list per actor')
            for row in im[table]:
                for i in row:
                    self.ref(i, target, table)
        for i in im['assumes'] + im['invariants']:
            self.code(i, 'S', 'assume/invariant')
        for ev in im['events']:
            what = f'event {ev["name"]}'
            if not 0 <= ev['params'] <= ev['locals']:
                raise ImageErr('I-Ref', f'{what}: {ev["params"]} parameters do not fit in {ev["locals"]} locals')
            self.code(ev['guard'], 'G', what, ev['locals'])
            self.code(ev['effects'], 'E', what, ev['locals'])
            for i in ev['ensure']:
                self.code(i, 'P', what, ev['locals'])
        for u in im['units']:
            what = f'unit {u["name"]}'
            if not u['members'] or not isinstance(u['net'], bool):
                raise ImageErr('I-Format', f'{what} is malformed')
            for m in u['members']:
                ev = im['events'][self.ref(m['event'], 'events', what)]
                if len(m['args']) != ev['params']:
                    raise ImageErr('I-Ref', f'{what}: {ev["name"]} takes {ev["params"]} arguments')
                for i in m['args']:
                    self.code(i, 'C', what)
        self.blocks()
        for w in im['weights']:
            self.code(w['code'], 'S', f'weight {w["name"]}')
        for q in im['queries']:
            if not isinstance(q['unit'], str):
                raise ImageErr('I-Format', f'query {q["name"]} has no unit text')
            self.query(q['q'], f'query {q["name"]}')
        for u in im['unknowns']:
            what = f'unknown {u["name"]}'
            whole = lambda v: isinstance(v, int) and not isinstance(v, bool)    # noqa: E731
            if u['kind'] not in PARAM_KINDS or not whole(u['lo']) or not whole(u['hi']) or u['lo'] > u['hi']:
                raise ImageErr('I-Format', f'{what} is malformed')
            limits = {'bool': (0, 1), 'actor': (0, self.n['actors'] - 1), 'time': (lo, hi)}.get(u['kind'])
            if u['kind'] == 'enum':
                limits = (0, len(im['enums'][self.ref(u['enum'], 'enums', what)]['values']) - 1)
            if limits is not None and not limits[0] <= u['lo'] <= u['hi'] <= limits[1]:
                raise ImageErr('I-Format', f'{what} ranges outside its kind')
        for sv in im['solves']:
            what = f'solve {sv["name"]}'
            if sv['form'] not in SOLVE_FORMS or not isinstance(sv['unit'], str):
                raise ImageErr('I-Format', f'{what} is malformed')
            if (sv['objective'] is not None) != (sv['form'] in ('min', 'max')):
                raise ImageErr('I-Format', f'{what}: only min and max have an objective, and they need one')
            if sv['point'] is not None:
                self.ref(sv['point'], 'marks', what)
            mode, before = ('V', 'C') if sv['point'] is None else ('X', 'S')
            if sv['where'] is not None:
                self.code(sv['where'], before, what)
            if sv['objective'] is not None:
                self.code(sv['objective'], mode, what)
            self.code(sv['goal'], mode, what)
        for i, c in enumerate(im['codes']):
            self.block_of_code(i, c)

    def domain(self, dom, what):
        if dom['kind'] not in DOMAINS:
            raise ImageErr('I-Format', f'{what}: unknown domain {dom["kind"]!r}')
        if dom['kind'] == 'enum':
            self.ref(dom['enum'], 'enums', what)

    def blocks(self):
        im, uses = self.im, [0] * self.n['blocks']

        def use(b, what):
            uses[self.ref(b, 'blocks', what)] += 1

        self.ref(im['entry'], 'blocks', 'entry')
        for bi, stmts in enumerate(im['blocks']):
            for st in stmts:
                what, kind = f'block {bi}', st[0]
                if kind == 'at' and len(st) == 3:
                    self.code(st[1], 'C', what)
                    use(st[2], what)
                elif kind == 'run' and len(st) == 2:
                    self.ref(st[1], 'units', what)
                elif kind == 'attempt' and len(st) == 4:
                    self.ref(st[1], 'units', what)
                    use(st[2], what)
                    use(st[3], what)
                elif kind == 'alt' and len(st) == 2 and len(st[1]) >= 2:
                    for b in st[1]:
                        use(b, what)
                elif kind == 'choose' and len(st) == 4 and st[3]:
                    self.ref(st[1], 'actors', what)
                    self.ref(st[2], 'menus', what)
                    for action, b in st[3]:
                        self.ref(action, 'actions', what)
                        use(b, what)
                elif kind == 'mark' and len(st) == 2:
                    self.ref(st[1], 'marks', what)
                else:
                    raise ImageErr('I-Format', f'{what}: malformed statement {st!r}')
        for b, count in enumerate(uses):
            if count != (0 if b == im['entry'] else 1):
                raise ImageErr('I-Ref', f'block {b} is referenced {count} times; the program must be a tree')

    def query(self, q, what):
        kind = q['kind']
        if kind in ('exists', 'forall'):
            self.code(q['code'], 'R', what)
        elif kind == 'can':
            for a in q['who']:
                self.ref(a, 'actors', what)
            self.code(q['code'], 'R', what)
        elif kind == 'num':
            for a in q['policy'] or ():
                self.ref(a, 'actors', what)
            if q['weight'] is not None:
                self.ref(q['weight'], 'weights', what)
            if q['opt'] not in ('max', 'min'):
                raise ImageErr('I-Format', f'{what}: unknown aggregate {q["opt"]!r}')
            self.code(q['code'], 'R', what)
        elif kind == 'region':
            for p in q['params']:
                self.ref(p, 'params', what)
            self.query(q['inner'], what)
        elif kind == 'prefer':
            self.ref(q['who'], 'actors', what)
            self.ref(q['menu'], 'menus', what)
            self.ref(q['a'], 'actions', what)
            self.ref(q['b'], 'actions', what)
            if q['weight'] is not None:
                self.ref(q['weight'], 'weights', what)
            self.code(q['code'], 'R', what)
        else:
            raise ImageErr('I-Format', f'{what}: unknown kind {kind!r}')

    def point(self, kind, ident, what):
        if isinstance(kind, bool) or kind not in (0, 1, 2, 3):
            raise ImageErr('I-Ref', f'{what}: unknown point kind {kind!r}')
        if kind >= 2:
            self.ref(ident, 'menus' if kind == 2 else 'marks', what)
        elif ident != 0:
            raise ImageErr('I-Ref', f'{what}: start and end take no number')

    def block_of_code(self, ci, c):
        what, mode, ins = f'code {ci}', c['mode'], c['ins']
        if mode not in RESULT:
            raise ImageErr('I-Mode', f'{what}: unknown layer {mode!r}')
        if isinstance(c['locals'], bool) or not isinstance(c['locals'], int) or c['locals'] < 0:
            raise ImageErr('I-Format', f'{what}: bad local count')
        size = len(ins)
        for pc, instr in enumerate(ins):
            op = instr[0]
            if op not in OPS:
                raise ImageErr('I-Mode', f'{what}: unknown instruction {op!r}')
            argc, _, _, layers = OPS[op]
            if mode not in layers:
                raise ImageErr('I-Mode', f'{what}: {op} is not allowed in layer {mode}')
            if len(instr) != argc + 1:
                raise ImageErr('I-Format', f'{what}: {op} takes {argc} arguments')
            a = instr[1:]
            if op == 'PUSH':
                if not (a[0] is None or isinstance(a[0], (bool, int))):
                    raise ImageErr('I-Format', f'{what}: PUSH takes an integer, true, false or null')
            elif op == 'PARAM':
                self.ref(a[0], 'params', what)
            elif op == 'UNK':
                self.ref(a[0], 'unknowns', what)
            elif op == 'QUERY':
                q = self.im['queries'][self.ref(a[0], 'queries', what)]['q']
                if q['kind'] == 'region' or q.get('weight') is not None and q['kind'] == 'num':
                    raise ImageErr('I-Ref', f'{what}: query {a[0]} has no single value to use')
            elif op in ('LOCAL', 'STORE', 'GIVEN'):
                if isinstance(a[0], bool) or not isinstance(a[0], int) or not 0 <= a[0] < c['locals']:
                    raise ImageErr('I-Ref', f'{what}: local {a[0]!r} is out of range')
                if op == 'GIVEN' and not isinstance(a[1], str):
                    raise ImageErr('I-Format', f'{what}: GIVEN needs a reason text')
            elif op in CELL_OPS:
                self.ref(a[0], 'cells', what)
            elif op in ASSET_OPS:
                self.ref(a[0], 'assets', what)
            elif op in SLOT_OPS:
                self.ref(a[0], 'slots', what)
            elif op in ('JMP', 'JF', 'JT'):                 # forward only, so a block always ends
                if isinstance(a[0], bool) or not isinstance(a[0], int) or not pc < a[0] <= size:
                    raise ImageErr('I-Stack', f'{what}: jump at {pc} to {a[0]!r} must go forward inside the block')
            elif op == 'K':
                self.ref(a[0], 'actors', what)
                self.inner(a[1], ci, what)
            elif op == 'AT':
                self.point(a[0], a[1], what)
                self.inner(a[2], ci, what)
                if not isinstance(a[3], bool):
                    raise ImageErr('I-Format', f'{what}: AT needs a true/false flag')
            elif op == 'REACH':
                self.point(a[0], a[1], what)
            elif op == 'REQUIRE' and not isinstance(a[0], str):
                raise ImageErr('I-Format', f'{what}: REQUIRE needs a reason text')
        depth = [None] * (size + 1)
        depth[0], todo = 0, [0]
        while todo:
            pc = todo.pop()
            if pc == size:
                continue
            op = ins[pc][0]
            _, pops, pushes, _ = OPS[op]
            if depth[pc] < pops:
                raise ImageErr('I-Stack', f'{what}: {op} at {pc} needs {pops} values, the stack has {depth[pc]}')
            after = depth[pc] - pops + pushes
            nexts = [ins[pc][1]] if op == 'JMP' else [pc + 1, ins[pc][1]] if op in ('JF', 'JT') else [pc + 1]
            for nxt in nexts:
                if depth[nxt] is None:
                    depth[nxt] = after
                    todo.append(nxt)
                elif depth[nxt] != after:
                    raise ImageErr('I-Stack', f'{what}: paths meet at {nxt} with different stack depths')
        if depth[size] != RESULT[mode]:
            raise ImageErr('I-Stack', f'{what}: leaves {depth[size]} values, layer {mode} needs {RESULT[mode]}')

    def inner(self, sub, ci, what):
        self.code(sub, 'Q', what)
        if sub >= ci:
            raise ImageErr('I-Ref', f'{what}: inner code {sub} must come before it')


def dumps(image):
    """The image as file text: sorted keys, no spaces, so equal images are equal bytes."""
    return json.dumps(image, ensure_ascii=False, sort_keys=True, separators=(',', ':')) + '\n'


def loads(text):
    """Read file text into a verified image."""
    try:
        image = json.loads(text)
    except ValueError as exc:
        raise ImageErr('I-Format', f'the file is not JSON ({exc})') from None
    return verify(image)


def _instr(image, instr):
    op, a = instr[0], instr[1:]
    cells = [c['name'] for c in image['cells']] + [f'bal({who},{x["name"]})' for x in image['assets']
                                                    for who in image['actors']]
    note = ''
    if op in CELL_OPS:
        note = cells[a[0]]
    elif op in ASSET_OPS:
        note = image['assets'][a[0]]['name']
    elif op in SLOT_OPS:
        slot = image['slots'][a[0]]
        note = f'{image["actors"][slot["actor"]]}.{slot["key"]}'
    elif op == 'PARAM':
        note = image['params'][a[0]]['name']
    elif op == 'UNK':
        note = image['unknowns'][a[0]]['name']
    elif op == 'QUERY':
        note = image['queries'][a[0]]['name']
    elif op == 'K':
        note = image['actors'][a[0]]
    elif op in ('AT', 'REACH'):
        note = POINTS[a[0]] + (f' {image["menus" if a[0] == 2 else "marks"][a[1]]}' if a[0] >= 2 else '')
    text = ' '.join([op] + [json.dumps(x, ensure_ascii=False) for x in a])
    return f'{text:24s}; {note}' if note else text


def listing(image):
    """A readable listing of a verified image."""
    im, out = image, []
    add = out.append
    add(f'image {im["model"]}   format {im["format"]} {im["version"]}   language {im["language"]}')
    add('actors   ' + '  '.join(f'{i} {a}' for i, a in enumerate(im['actors'])))
    add(f'time     {im["time"][0]}..{im["time"][1]}')
    for i, p in enumerate(im['params']):
        add(f'param    {i} {p["name"]} : {p["kind"]} {{{", ".join(p["show"])}}}')
    for i, u in enumerate(im['unknowns']):
        add(f'unknown  {i} {u["name"]} : {u["kind"]} {u["lo"]}..{u["hi"]}')
    for i, e in enumerate(im['enums']):
        add(f'enum     {i} {e["name"]} {{{", ".join(e["values"])}}}')
    add('cells')
    for i, c in enumerate(im['cells']):
        dom = c['dom']['kind'] + (' ' + im['enums'][c['dom']['enum']]['name'] if c['dom']['kind'] == 'enum' else '')
        rng = f'  range c{c["lo"]}..c{c["hi"]}' if c['lo'] is not None else ''
        add(f'  {i:4d} {c["name"]:18s} {dom}{"?" if c["opt"] else ""}{rng}  init ' + ' '.join(f'c{v}' for v in c['init']))
    base = len(im['cells'])
    for x, a in enumerate(im['assets']):
        first = base + x * len(im['actors'])
        add(f'  {first:4d}..{first + len(im["actors"]) - 1} holdings of {a["name"]}  range c{a["lo"]}..c{a["hi"]}'
            f'  one cell per actor')
    for i, s in enumerate(im['slots']):
        add(f'slot     {i} {im["actors"][s["actor"]]}.{s["key"]}')
    for i, who in enumerate(im['actors']):
        add(f'view     {who}: cells {im["obs"][i]}  slots {im["mems"][i]}')
    add('events')
    for i, ev in enumerate(im['events']):
        ensure = ''.join(f' c{c}' for c in ev['ensure'])
        add(f'  {i:4d} {ev["name"]:18s} params {ev["params"]} locals {ev["locals"]}{" net" if ev["net"] else ""}'
            f'  guard c{ev["guard"]}  effects c{ev["effects"]}' + (f'  ensure{ensure}' if ensure else ''))
    add('units')
    for i, u in enumerate(im['units']):
        members = ', '.join(im['events'][m['event']]['name'] + '(' + ' '.join(f'c{c}' for c in m['args']) + ')'
                            for m in u['members'])
        add(f'  {i:4d} {u["name"]}{" net" if u["net"] else ""}: {members}')
    add(f'program  entry b{im["entry"]}')
    for b, stmts in enumerate(im['blocks']):
        parts = []
        for st in stmts:
            kind = st[0]
            if kind == 'at':
                parts.append(f'at c{st[1]} b{st[2]}')
            elif kind == 'run':
                parts.append(f'run u{st[1]}')
            elif kind == 'attempt':
                parts.append(f'attempt u{st[1]} b{st[2]} else b{st[3]}')
            elif kind == 'alt':
                parts.append('alt ' + ' or '.join(f'b{x}' for x in st[1]))
            elif kind == 'choose':
                arms = ' '.join(f'{im["actions"][a]}=>b{x}' for a, x in st[3])
                parts.append(f'choose {im["actors"][st[1]]} menu {im["menus"][st[2]]} {{{arms}}}')
            else:
                parts.append(f'mark {im["marks"][st[1]]}')
        add(f'  b{b}: ' + ('; '.join(parts) if parts else '(empty)'))
    for i, w in enumerate(im['weights']):
        add(f'weight   {i} {w["name"]} c{w["code"]}')
    add('queries')
    for q in im['queries']:
        add(f'  {q["name"]} = {_query(im, q["q"])}' + (f'  [{q["unit"]}]' if q['unit'] else ''))
    if im['solves']:
        add('solves')
    for sv in im['solves']:
        head = sv['form'] + (f' c{sv["objective"]}' if sv['objective'] is not None else '')
        head += f' at {im["marks"][sv["point"]]}' if sv['point'] is not None else ''
        head += f' where c{sv["where"]}' if sv['where'] is not None else ''
        add(f'  {sv["name"]} = {head}: c{sv["goal"]}' + (f'  [{sv["unit"]}]' if sv['unit'] else ''))
    add('codes')
    for i, c in enumerate(im['codes']):
        add(f'  c{i} [{c["mode"]}{", locals " + str(c["locals"]) if c["locals"] else ""}]')
        for pc, instr in enumerate(c['ins']):
            add(f'      {pc:3d} {_instr(im, instr)}')
    return '\n'.join(out) + '\n'


def _query(im, q):
    kind = q['kind']
    names = lambda xs: '{' + ', '.join(im['actors'][a] for a in xs) + '}'
    if kind in ('exists', 'forall'):
        return f'{kind} run c{q["code"]}'
    if kind == 'can':
        return f'can {names(q["who"])} c{q["code"]}'
    if kind == 'num':
        head = (f'max policy {names(q["policy"])} ' if q['policy'] is not None else '')
        head += f'expect {im["weights"][q["weight"]]["name"]} ' if q['weight'] is not None else ''
        return f'{head}{q["opt"]} run c{q["code"]}'
    if kind == 'region':
        return 'region ' + ', '.join(im['params'][p]['name'] for p in q['params']) + ': ' + _query(im, q['inner'])
    tail = f' under {im["weights"][q["weight"]]["name"]}' if q['weight'] is not None else ''
    return (f'prefer {im["actors"][q["who"]]} at menu {im["menus"][q["menu"]]}: {im["actions"][q["a"]]} over '
            f'{im["actions"][q["b"]]} by c{q["code"]}{tail}')
