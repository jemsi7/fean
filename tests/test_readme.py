"""Checks for what the repository shows a newcomer: the README in two languages (C-18), and the
licence files and the ignore list (C-19).

A README is judged against the things it talks about, not against itself (DR-73). The model it
prints is an example file, the output under a command is what that command prints, and its numbers
are counted from the specification and the tools. The Korean README is the reference (DR-67): it
is judged against the tools, and the English one against the Korean one. Every test name here is
an acceptance criterion of the project's concept map, a design record kept outside this repository
(C-18 to C-20).

The executables come from `bash rust/build.sh`; the tests refuse to run against stale ones.
"""
import ast
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
import unittest
from collections import Counter
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import test_fean_rust as R  # noqa: E402
from fean.syntax import KEYWORDS  # noqa: E402

KO = ROOT / 'README.md'
EN = ROOT / 'README.en.md'
SPEC = ROOT / 'docs' / 'financial-language.md'
ENGINE = ROOT / 'docs' / 'engine.md'
MANUAL = ROOT / 'docs' / 'manual' / 'manual.html'
EXAMPLES = ROOT / 'examples'

ANCHORS = ['problem', 'glance', 'premises', 'principle', 'install', 'ide', 'cli', 'examples', 'docs',
           'verification', 'layout', 'contributing', 'license']
TITLES = {
    KO: ['문제', '한눈에', '전제와 공리', '원리', '설치', 'IDE', '명령줄', '예제', '문서', '검증', '저장소', '기여', '라이선스'],
    EN: ['problem', 'glance', 'premises and axioms', 'how it works', 'install', 'IDE', 'command line', 'examples',
         'documentation', 'verification', 'layout', 'contributing', 'license'],
}
OTHER = {KO: 'README.en.md', EN: 'README.md'}
LANGS = {'bash', 'text', 'fean', 'python', 'math', 'mermaid'}
SAME = ('bash', 'text', 'fean', 'python', 'math')        # blocks that no translation may touch
NAV = ['#glance', '#principle', '#install', 'docs/financial-language.md', 'docs/manual/FEAN-IDE-설명서.pdf', 'docs/engine.md']
HERO = 'docs/manual/img/02-split.png'
LAYOUT = ['fean/', 'rust/fean-core/', 'rust/fean-cli/', 'rust/fean-ide/', 'examples/', 'tests/', 'docs/']
WITHHELD = ('models', 'docs/maps')      # folders at the root that are not published (DR-76, DR-77)
FINDINGS = ['컴파일 오류', '실행 오류', '실패한 필수 사건', '깨진 보장', '해 없음']
VERDICTS = ['BLOCKED', 'NO_SETTLEMENT', 'UNKNOWN']
OPTIONS = ['--json', '--budget', '--no-prelude', '--views', '-o']
OUTSIDE = {'https://rustup.rs', 'https://github.com/emilk/egui'}   # addresses the documents do not already cite
# one lower-case word in backticks is a reserved word, a command, a name in an example model, or one of these
WORDS = {'fean', 'draft', 'findings'}

APACHE_SHA256 = 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'       # the text at apache.org
MIT_NOTICE_SHA256 = 'fe2a9817987f862eaced948f0468c7f51d2fedfc48c5c505b246a49a3870e9a5'   # the permission notice, spaces folded
COPYRIGHT = 'Copyright (c) 2026 Minchan_J'

# what a README states as a number: (name, pattern in Korean, pattern in English)
CLAIMS = [
    ('version', r'판은 (\d+\.\d+)', r'at version (\d+\.\d+)'),
    ('keywords', r'예약어 (\d+)개', r'(\d+) reserved words'),
    ('rules', r'규칙 (\d+)개', r'(\d+) named rules'),
    ('contracts', r'표준 계약 (\d+)개', r'(\d+) standard contracts'),
    ('examples', r'예제 모델 (\d+)개', r'(\d+) example models'),
    ('views', r'보기 (\d+)가지', r'(\d+) views'),
    ('features', r'기능 (\d+)개', r'(\d+) features'),
    ('tests', r'검산 (\d+)건', r'(\d+) checks'),
    ('extras', r'가운데 (\d+)건', r'(\d+) of them'),
    ('budget', r'기본값은 ([\d,]+)', r'defaults to ([\d,]+)'),
    ('runs', r'실행 (\d+)번', r'(\d+) runs'),
    ('rust', r'Rust (\d+\.\d+)', r'Rust (\d+\.\d+)'),
    ('python', r'Python (\d+\.\d+) 이상', r'Python (\d+\.\d+) or later'),
]


class Block:
    def __init__(self, lang, body, start, end):
        self.lang, self.body, self.start, self.end = lang, body, start, end


def read(path):
    return path.read_text(encoding='utf-8')


def blocks(text):
    """The fenced blocks of a document, in order."""
    return [Block(m.group(1), m.group(2), m.start(), m.end())
            for m in re.finditer(r'^```(\w*)\n(.*?)\n```[ \t]*$', text, re.S | re.M)]


def prose(text):
    """The document without its fenced blocks."""
    out, at = [], 0
    for block in blocks(text):
        out.append(text[at:block.start])
        at = block.end
    return ''.join(out + [text[at:]])


def anchors(text):
    return re.findall(r'<a id="([^"]+)"></a>', text)


def head(text):
    """What comes before the first section."""
    return text[:text.index(f'<a id="{ANCHORS[0]}"></a>')]


def section(text, anchor):
    """One section, from its anchor to the next one."""
    start = text.index(f'<a id="{anchor}"></a>')
    later = [text.index(f'<a id="{other}"></a>') for other in ANCHORS if f'<a id="{other}"></a>' in text]
    ends = [at for at in later if at > start]
    return text[start:min(ends)] if ends else text[start:]


def targets(text):
    """Every address a document points at: Markdown links and images, HTML href and src."""
    return re.findall(r'\]\(([^)\s]+)\)', text) + re.findall(r'(?:href|src)="([^"]+)"', text)


def runnable(text):
    """(language, body, expected output) for every block that is followed at once by a `text` block."""
    found, all_blocks = [], blocks(text)
    for block, after in zip(all_blocks, all_blocks[1:]):
        if block.lang in ('bash', 'python') and after.lang == 'text' and not text[block.end:after.start].strip():
            found.append((block.lang, block.body, after.body))
    return found


def rule_names(text):
    return set(re.findall(r'\b([STEWPQDI]-[A-Z][a-z]+)\b', text))


def published():
    """The files of the working folder that are published: all but what is built, what the system
    adds, and the folders that are withheld."""
    found = []
    for folder, folders, files in os.walk(ROOT):
        here = Path(folder).relative_to(ROOT).as_posix()
        folders[:] = [name for name in folders
                      if name not in ('dist', 'target', '__pycache__', '.git')
                      and (name if here == '.' else f'{here}/{name}') not in WITHHELD]
        for name in files:
            if name.startswith('._') or name in ('.DS_Store', '.git') or name.endswith(('.feanc', '.pyc')):
                continue
            # macOS hands back a Korean file name decomposed; git stores it composed
            found.append(unicodedata.normalize('NFC', (Path(folder) / name).relative_to(ROOT).as_posix()))
    return sorted(found)


def run_example(name):
    done = R.rust('run', f'examples/{name}')
    assert done.returncode == 0, done.stderr
    return [re.sub(r'\s+=', ' =', line) for line in done.stdout.splitlines()]


class TestReadme(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls):
        R.fresh(R.FEAN, 'fean-core', 'fean-cli')
        R.fresh(R.IDE, 'fean-core', 'fean-ide')
        cls.spec, cls.engine, cls.manual = read(SPEC), read(ENGINE), read(MANUAL)
        cls.features = json.loads(subprocess.run([str(R.IDE), '--features'], capture_output=True, text=True, check=True).stdout)
        # the eight views, in the order of the tabs: the manual names them once in its contents
        cls.views = re.search(r'<li>(개요와 질의[^<]*)</li>', cls.manual).group(1).split(' · ')

    def test_readme_has_required_sections(self):
        for path in (KO, EN):
            with self.subTest(readme=path.name):
                text = read(path)
                self.assertEqual(anchors(text), ANCHORS)
                self.assertTrue({block.lang for block in blocks(text)} <= LANGS)
                for anchor, word in zip(ANCHORS, TITLES[path]):
                    title = re.search(rf'<a id="{anchor}"></a>\n## (.+)', text)
                    self.assertIsNotNone(title, anchor)
                    self.assertIn(word.lower(), title.group(1).lower())
                self.assertEqual(len(re.findall(r'^## ', prose(text), re.M)), len(ANCHORS))
                top = head(text)
                self.assertTrue(top.startswith('<div align="center">'))
                self.assertIn('<h1>FEAN</h1>', top)
                self.assertIn(f'href="{OTHER[path]}"', top)
                self.assertEqual(len(re.findall(r'src="https://img\.shields\.io/badge/[^"]+"', top)), 5)
                for target in NAV:
                    self.assertIn(f'href="{target}"', top)
                self.assertIn(f'src="{HERO}"', top)
                self.assertLess(top.index('</div>'), top.index(HERO))
                for anchor in ANCHORS:                      # the contents name every section
                    self.assertIn(f'](#{anchor})', top)
        # the three things the README was asked to carry, in the words they were asked in
        ko = read(KO)
        opening = section(ko, 'problem').split('\n\n')[1]
        self.assertIn('모순', opening)
        self.assertIn('인지', opening)
        self.assertIn('부하', opening)
        self.assertIn('cognitive load', section(read(EN), 'problem').split('\n\n')[1])
        self.assertIn('### 전제', section(ko, 'premises'))
        self.assertIn('### 공리', section(ko, 'premises'))
        self.assertIn('컴파일러', section(ko, 'principle'))
        self.assertIn('dist/fean-ide', section(ko, 'ide'))
        self.assertIn('dist/fean ', section(ko, 'cli'))
        self.assertIn('python3 -m fean', section(ko, 'cli'))

    def test_readme_examples_run_as_written(self):
        text = read(KO)
        pairs = runnable(text)
        self.assertEqual([lang for lang, _, _ in pairs], ['bash'] * 7 + ['python'])
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
        with tempfile.TemporaryDirectory() as tmp:
            # a folder that looks like a fresh checkout with the executables built, so that a file a
            # command writes (support.feanc) does not land in the repository
            for name in ('dist', 'examples', 'fean'):
                os.symlink(ROOT / name, Path(tmp) / name)
            (Path(tmp) / 'bin').mkdir()
            os.symlink(sys.executable, Path(tmp) / 'bin' / 'python3')
            env['PATH'] = str(Path(tmp) / 'bin') + os.pathsep + env['PATH']
            for lang, body, expected in pairs:
                with self.subTest(command=body.splitlines()[0]):
                    command = ['bash', '-c', body] if lang == 'bash' else [sys.executable, '-B', '-c', body]
                    done = subprocess.run(command, cwd=tmp, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                          text=True, timeout=60)
                    self.assertEqual(done.stdout.rstrip('\n'), expected.rstrip('\n'))
            self.assertTrue((Path(tmp) / 'support.feanc').exists())
        commands = [body for _, body, _ in pairs]
        self.assertEqual(commands[0], 'python3 -m fean run examples/support.fean')
        self.assertTrue(commands[1].startswith('dist/fean inspect examples/support.fean | python3 -c '))
        self.assertEqual(commands[2], 'dist/fean check examples/support.fean')
        self.assertTrue(commands[3].startswith("dist/fean check - <<'EOF'\nmodel Mix {"))
        self.assertEqual(commands[4:7], ['dist/fean run examples/capital.fean',
                                         'dist/fean compile examples/support.fean -o support.feanc',
                                         'dist/fean run support.feanc'])
        self.assertIn('fean.compile_source', commands[7])
        self.assertIn('fean.run_image', commands[7])
        # the model that is printed is an example file, to the letter
        models = [block.body for block in blocks(text) if block.lang == 'fean']
        files = {read(p).strip(): p.name for p in EXAMPLES.glob('*.fean') if not p.name.startswith('._')}
        self.assertGreaterEqual(len(models), 1)
        for model in models:
            self.assertIn(model.strip(), files)
        self.assertEqual(files[models[0].strip()], 'support.fean')
        self.assertFalse(list(ROOT.glob('*.feanc')))

    def test_readme_claims_match_spec(self):
        spec_anchors = set(anchors(self.spec))
        spec_rules = rule_names(self.spec)
        tests = sum(len(re.findall(r'^\s*def test_\w+', read(p), re.M)) for p in (ROOT / 'tests').glob('test_*.py'))
        extras = sum(1 for part in re.split(r'^\s*def (?=test_)', read(ROOT / 'tests' / 'test_fean_rust.py'), flags=re.M)[1:]
                     if re.search(r'^\s*from (PIL|pypdf) import', part, re.M))
        need = next(s for s in json.loads(R.rust('inspect', 'examples/capital.fean').stdout)['solves'] if s['name'] == 'need')
        prelude = read(ROOT / 'fean' / 'prelude.fean')
        names = set()
        for example in EXAMPLES.glob('*.fean'):
            if not example.name.startswith('._'):
                names |= set(re.findall(r'[A-Za-z_]\w*', read(example)))
        counted = {
            'version': re.search(r'^version: "(\d+\.\d+)', self.spec, re.M).group(1),
            'keywords': str(len(KEYWORDS)),
            'rules': str(len(spec_rules)),
            'contracts': str(len(re.findall(r'^contract ', prelude, re.M))),
            'examples': str(len([p for p in EXAMPLES.glob('*.fean') if not p.name.startswith('._')])),
            'views': str(len(self.views)),
            'features': str(len(self.features)),
            'tests': str(tests),
            'extras': str(extras),
            'budget': re.search(r'기본 (\d+)', R.rust('--help').stdout).group(1),
            'runs': str(need['rows'][0]['runs']),
            'rust': re.search(r'Rust\((\d+\.\d+) 이상\)', self.manual).group(1),
        }
        self.assertEqual((counted['keywords'], counted['rules'], counted['contracts'], counted['examples'], counted['views']),
                         ('93', '42', '4', '10', '8'))
        for path, column in ((KO, 1), (EN, 2)):
            text = read(path)
            with self.subTest(readme=path.name):
                # premises and axioms: four and seven, each pointing at a section of the specification
                parts = section(text, 'premises').split('\n### ')[1:]
                self.assertEqual(len(parts), 2)
                for part, size in zip(parts, (4, 7)):
                    items = re.findall(r'^\d+\. \*\*.+$', part, re.M)
                    self.assertEqual(len(items), size)
                    for item in items:
                        cited = re.findall(r'\]\(docs/financial-language\.md#(\w+)\)', item)
                        self.assertTrue(cited, item[:40])
                        self.assertTrue(set(cited) <= spec_anchors, item[:40])
                # names that belong to the language
                self.assertTrue(rule_names(text))
                self.assertEqual(rule_names(text) - spec_rules - rule_names(self.engine), set())
                for verdict in VERDICTS:
                    self.assertIn(f'`{verdict}`', text)
                    self.assertIn(verdict, self.spec)
                commands = set(re.search(r'\{([a-z,]+)\}', R.rust('--help').stdout).group(1).split(','))
                single = set(re.findall(r'`([a-z]+)`', prose(text)))
                self.assertEqual(single - set(KEYWORDS) - commands - names - WORDS, set())
                # numbers
                for claim in CLAIMS:
                    name, pattern = claim[0], claim[column]
                    said = [value.replace(',', '') for value in re.findall(pattern, prose(text))]
                    self.assertTrue(said, name)
                    if name == 'python':
                        self.assertEqual(set(said), {'3.9'})
                    else:
                        self.assertEqual(set(said), {counted[name]}, name)
                self.assertIn(f'FEAN-{counted["version"]}-', head(text))
                self.assertIn(f'Rust-{counted["rust"]}-', head(text))
                self.assertIn('Python-3.9%2B-', head(text))
                self.assertIn('draft', section(text, 'verification'))
                for region in need['rows'][0]['regions']:
                    self.assertIn(f'`{region["text"]}`', section(text, 'principle'))
                # the examples: every file once, and what a row quotes is what the example prints
                rows = [row for row in section(text, 'examples').splitlines() if row.startswith('| [')]
                named = [re.match(r'\| \[`?([\w.]+)`?\]\(examples/([\w.]+)\)', row).groups() for row in rows]
                self.assertEqual(sorted(file for _, file in named),
                                 sorted(p.name for p in EXAMPLES.glob('*.fean') if not p.name.startswith('._')))
                for row, (shown, file) in zip(rows, named):
                    self.assertEqual(shown, file)
                    quoted = re.findall(r'`(\w+ = [^`]+)`', row)
                    self.assertTrue(quoted, file)
                    printed = run_example(file)
                    for line in quoted:
                        self.assertIn(line, printed, file)
                # the figures of the problem table are the specification's
                for number in ('170', '200', '180'):
                    self.assertIn(number, section(text, 'problem'))
                    self.assertIn(f'{number}원', self.spec)
                self.assertIn('{0..90, 110..200}', section(text, 'problem'))
        self.assertEqual(self.spec.count('status: draft'), 1)
        ko = read(KO)
        for kind in FINDINGS:                       # the five kinds of contradiction, as the IDE names them
            self.assertIn(f'| {kind} |', section(ko, 'principle'))
            self.assertIn(kind, self.manual)
        # the sources of the Python engine and the reference implementation are written in the syntax of the
        # oldest Python the README names
        for source in list((ROOT / 'fean').glob('*.py')) + list((ROOT / 'tests' / 'fean_ref').glob('*.py')):
            if not source.name.startswith('._'):
                ast.parse(read(source), filename=str(source), feature_version=(3, 9))

    def test_readme_covers_cli_and_ide(self):
        usage = R.rust('--help').stdout
        commands = re.search(r'\{([a-z,]+)\}', usage).group(1).split(',')
        py = subprocess.run([sys.executable, '-B', '-m', 'fean', '--help'], cwd=ROOT, capture_output=True, text=True).stdout
        py_commands = re.search(r'\{([a-z,]+)\}', py).group(1).split(',')
        self.assertEqual(sorted(commands), ['batch', 'check', 'compile', 'dump', 'inspect', 'run'])
        self.assertEqual(sorted(py_commands), ['check', 'compile', 'dump', 'run'])
        known = set(re.findall(r'<kbd>([^<]+)</kbd>', self.manual))
        tour = re.search(r'모순이 (\d+)건에서 (\d+)건', self.manual).groups()
        names = [feature['name'] for feature in self.features]
        self.assertEqual([name for name in names if name in self.views], self.views)
        for path in (KO, EN):
            text = read(path)
            with self.subTest(readme=path.name):
                cli = section(text, 'cli')
                rows = {re.match(r'\| `fean (\w+)', row).group(1): row for row in cli.splitlines() if row.startswith('| `fean ')}
                self.assertEqual(list(rows), ['check', 'run', 'compile', 'dump', 'inspect', 'batch'])
                for command in commands:
                    self.assertEqual('Rust' in rows[command], command not in py_commands, command)
                for option in OPTIONS:
                    self.assertIn(f'`{option}', cli)
                self.assertEqual(re.findall(r'^\| ([0-3]) \|', cli, re.M), ['0', '1', '2', '3'])
                ide = section(text, 'ide')
                shown = [row.split('|')[1].strip() for row in ide.splitlines() if row.startswith('| `')]
                self.assertEqual([re.match(r'`([^`]+)`', cell).group(1) for cell in shown][:len(self.views)], self.views)
                bodies = [block.body for block in blocks(ide) if block.lang == 'bash']
                self.assertIn('dist/fean-ide', bodies)
                self.assertIn('dist/fean-ide examples/repay80.fean', bodies)
                keys = re.findall(r'<kbd>([^<]+)</kbd>', ide)
                self.assertGreaterEqual(len(keys), 10)
                self.assertEqual(set(keys) - known, set())
                self.assertIn(tour, [(a, b) for a, b in re.findall(r'(\d+)(?:건에서| to) (\d+)', ide)])
        # the four exit codes do what the table says
        mix = next(body for _, body, _ in runnable(read(KO)) if body.startswith('dist/fean check - <<'))
        model = mix.split('\n', 1)[1].rsplit('\nEOF', 1)[0]
        void = ('model Void {\n  actor A\n  time 0..0\n  cell x : Bool init in { true, false }\n  assume x and not x\n'
                '  program { }\n  query q = forall run: reach(end)\n}\n')
        self.assertEqual(R.rust('check', 'examples/support.fean').returncode, 0)
        self.assertEqual(R.rust('check', '-', stdin=model).returncode, 1)
        self.assertEqual(R.rust('run', '-', stdin=void).returncode, 2)
        self.assertEqual(R.rust('run', 'no-such-model.fean').returncode, 3)

    def test_readme_links_resolve(self):
        cited = set(re.findall(r'https?://[^\s)"<>]+', self.spec + self.engine))
        for path in (KO, EN):
            text = read(path)
            with self.subTest(readme=path.name):
                inside = outside = 0
                for target in targets(text):
                    if target.startswith('http'):
                        outside += 1
                        self.assertTrue(target.startswith('https://img.shields.io/badge/') or target in cited or target in OUTSIDE,
                                        target)
                        continue
                    inside += 1
                    name, _, fragment = target.partition('#')
                    dest = (ROOT / unquote(name)) if name else path
                    self.assertTrue(dest.exists(), target)
                    if fragment and dest.suffix == '.md':
                        self.assertIn(fragment, anchors(read(dest)), target)
                    if dest.suffix == '.png':
                        self.assertEqual(dest.read_bytes()[:8], b'\x89PNG\r\n\x1a\n', target)
                self.assertGreater(inside, 40)
                self.assertGreater(outside, 5)
                pictures = [t for t in targets(text) if t.endswith('.png')]
                self.assertGreaterEqual(len(set(pictures)), 6)
                paths = re.findall(r'^\| `([\w./-]+/)` \|', section(text, 'layout'), re.M)
                self.assertEqual(paths, LAYOUT)
                for cell in paths:
                    self.assertTrue((ROOT / cell).is_dir(), cell)
                self.assertNotIn('models/', text)           # the case model is not part of what the README presents (DR-75)
                self.assertNotIn('docs/maps', text)         # nor are the design records (DR-77)
                self.assertIsNone(re.search(r'[\w.+-]+@[\w-]+\.[a-z]{2,}', text))
                self.assertIsNone(re.search(r'TODO|TBD|FIXME', text))
                self.assertNotIn('github.com/', re.sub(r'https://github\.com/emilk/egui', '', text))

    def test_readme_translations_agree(self):
        ko, en = read(KO), read(EN)
        self.assertEqual(anchors(en), anchors(ko))
        self.assertEqual([block.lang for block in blocks(en)], [block.lang for block in blocks(ko)])
        for lang in SAME:
            self.assertEqual([b.body for b in blocks(en) if b.lang == lang], [b.body for b in blocks(ko) if b.lang == lang], lang)
        mutual = set(OTHER.values())
        self.assertEqual(sorted(t for t in targets(en) if t not in mutual), sorted(t for t in targets(ko) if t not in mutual))
        for anchor in ANCHORS:
            with self.subTest(section=anchor):
                a, b = prose(section(ko, anchor)), prose(section(en, anchor))
                for what, pattern in (('table rows', r'^\|'), ('numbered items', r'^\d+\. '), ('bullets', r'^- ')):
                    self.assertEqual(len(re.findall(pattern, b, re.M)), len(re.findall(pattern, a, re.M)), what)
                self.assertEqual(re.findall(r'^(#+) ', b, re.M), re.findall(r'^(#+) ', a, re.M), 'heading levels')
        number = r'\d+(?:[.,]\d+)*'
        self.assertEqual(Counter(re.findall(number, prose(en))), Counter(re.findall(number, prose(ko))))
        # Korean in the English README is only what the screen and the files are called, and the name of
        # the other language in the line that switches to it
        bare = re.sub(r'`[^`\n]*`', '', prose(en))
        bare = re.sub(r'\]\([^)\s]+\)', ']', bare)
        bare = re.sub(r'(?:href|src)="[^"]+"', '', bare)
        self.assertEqual(re.findall(r'[가-힣]+', bare), ['한국어'])
        self.assertGreater(len(re.findall(r'[가-힣]', prose(ko))), 3000)


class TestRelease(unittest.TestCase):
    def test_license_files_are_standard(self):
        apache = (ROOT / 'LICENSE-APACHE').read_bytes()
        self.assertEqual(hashlib.sha256(apache).hexdigest(), APACHE_SHA256)
        mit = read(ROOT / 'LICENSE-MIT')
        lines = mit.splitlines()
        self.assertEqual(lines[:4], ['MIT License', '', COPYRIGHT, ''])
        self.assertEqual([line for line in lines if 'opyright (c)' in line], [COPYRIGHT])
        notice = ' '.join(mit[mit.index('Permission is hereby granted'):].split())
        self.assertEqual(hashlib.sha256(notice.encode('utf-8')).hexdigest(), MIT_NOTICE_SHA256)
        self.assertTrue(notice.endswith('SOFTWARE.'))

    def test_license_is_stated_consistently(self):
        year, holder = re.search(r'^Copyright \(c\) (\d{4}) (.+)$', read(ROOT / 'LICENSE-MIT'), re.M).groups()
        for path in (KO, EN):
            text = read(path)
            with self.subTest(readme=path.name):
                part = section(text, 'license')
                self.assertIn('](LICENSE-MIT)', part)
                self.assertIn('](LICENSE-APACHE)', part)
                self.assertIn('`MIT OR Apache-2.0`', part)
                self.assertIn(f'Copyright © {year} {holder}', part)
                for other in ('GPL', 'BSD', 'MPL', 'Unlicense'):
                    self.assertNotIn(other, part)
                self.assertIn('license-MIT%20OR%20Apache--2.0-', head(text))
                self.assertEqual(text.count(holder), 1)

    def test_gitignore_filters_only_generated_and_withheld_files(self):
        git = shutil.which('git')
        self.assertIsNotNone(git, 'git이 있어야 무시 목록을 판정할 수 있습니다')
        generated = ['dist/fean', 'dist/fean-ide', 'rust/target/release/fean', 'examples/repay80.feanc',
                     'fean/__pycache__/vm.cpython-313.pyc', '.DS_Store', 'docs/._engine.md', '._README.md']
        # what is not published (DR-76, DR-77): probes, and whatever those folders hold in this working folder
        withheld = ['models/case.fean', 'models/sub/note.txt', 'docs/maps/notes.md']
        for folder in WITHHELD:
            for path in sorted((ROOT / folder).rglob('*')) if (ROOT / folder).is_dir() else []:
                if path.is_file() and not path.name.startswith('._') and path.name != '.DS_Store':
                    withheld.append(unicodedata.normalize('NFC', path.relative_to(ROOT).as_posix()))
        elsewhere = ['docs/models/note.md', 'rust/docs/maps/note.md']   # only the folders at the root are withheld
        kept = published()
        self.assertGreater(len(kept), 90)
        for must in ('LICENSE-MIT', 'LICENSE-APACHE', '.gitignore', 'rust/Cargo.lock', 'rust/build.sh', 'examples/support.fean',
                     'fean/prelude.fean', 'docs/financial-language.md', 'docs/manual/FEAN-IDE-설명서.pdf', 'tests/test_readme.py'):
            self.assertIn(must, kept)
        with tempfile.TemporaryDirectory() as tmp:
            # an empty repository that sees no ignore list but this one
            env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=tmp, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)
            subprocess.run([git, 'init', '-q', tmp], env=env, check=True, capture_output=True)
            shutil.copyfile(ROOT / '.gitignore', Path(tmp) / '.gitignore')

            def ignored(paths):
                done = subprocess.run([git, '-C', tmp, 'check-ignore', '--no-index', '-z', '--stdin'], env=env,
                                      input='\0'.join(paths) + '\0', capture_output=True, text=True, encoding='utf-8')
                self.assertIn(done.returncode, (0, 1), done.stderr)
                return [path for path in done.stdout.split('\0') if path]

            self.assertEqual(ignored(generated + withheld), generated + withheld)
            self.assertEqual(ignored(elsewhere + kept), [])
        self.assertFalse([path for path in kept if path.startswith(('models/', 'docs/maps/'))])
        patterns = [line for line in read(ROOT / '.gitignore').splitlines() if line and not line.startswith('#')]
        self.assertEqual(patterns, ['/dist/', 'target/', '*.feanc', '__pycache__/', '*.py[cod]', '.DS_Store', '._*',
                                    '/models/', '/docs/maps/'])

    def test_repository_tracks_only_published_files(self):
        git = shutil.which('git')
        if git is None or not (ROOT / '.git').exists():
            self.skipTest('git 저장소가 아닙니다')
        done = subprocess.run([git, '-C', str(ROOT), 'ls-files', '-z'], capture_output=True, check=True)
        tracked = [unicodedata.normalize('NFC', path) for path in done.stdout.decode('utf-8').split('\0') if path]
        self.assertGreater(len(tracked), 80)
        for path in tracked:
            parts = path.split('/')
            self.assertFalse(path.startswith(('dist/', 'models/', 'docs/maps/')), path)
            self.assertFalse(any(part.startswith('._') or part in ('.DS_Store', '__pycache__', 'target') for part in parts), path)
            self.assertFalse(path.endswith(('.feanc', '.pyc')), path)
        must = ['README.md', 'README.en.md', 'LICENSE-MIT', 'LICENSE-APACHE', '.gitignore', 'rust/Cargo.toml', 'rust/Cargo.lock',
                'rust/build.sh', 'fean/prelude.fean', 'docs/financial-language.md', 'docs/engine.md',
                'docs/manual/FEAN-IDE-설명서.pdf', 'docs/manual/manual.html']
        must += [f'examples/{p.name}' for p in EXAMPLES.glob('*.fean') if not p.name.startswith('._')]
        must += [f'tests/{p.name}' for p in (ROOT / 'tests').glob('test_*.py')]
        self.assertEqual(len(must), 13 + 10 + 5)
        for path in must:
            self.assertIn(path, tracked)

    def test_published_files_carry_no_private_traces(self):
        in_text = {
            'an e-mail address': r'[\w.+-]+@[\w-]+\.[a-z]{2,}',
            'a home folder': r'/Users/[A-Za-z0-9_.-]+',
            'a volume': r'/Volumes/[A-Za-z0-9_.-]+',
            'a GitHub token': r'gh[pousr]_[A-Za-z0-9]{20,}',
            'an API key': r'\bsk-[A-Za-z0-9_-]{20,}',
            'a private key': r'BEGIN [A-Z ]*PRIVATE KEY',
        }
        in_bytes = [b'/Users/', b'/Volumes/', b'file://']
        pictures = 0
        for name in published():
            path = ROOT / name
            if path.suffix in ('.png', '.pdf'):
                pictures += 1
                data = path.read_bytes()
                for trace in in_bytes:
                    self.assertNotIn(trace, data, name)
                continue
            text = read(path)               # whatever is not a picture or the manual must read as text
            for what, pattern in in_text.items():
                found = [hit for hit in re.findall(pattern, text) if not hit.endswith(('@example.com', '@example.org'))]
                self.assertEqual(found, [], f'{name}: {what}')
        self.assertGreater(pictures, 20)


if __name__ == '__main__':
    unittest.main()
