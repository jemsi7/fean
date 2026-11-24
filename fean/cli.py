"""Command line of the FEAN engine (docs/engine.md section 2). Map unit: I-08-a.

    python3 -m fean compile model.fean [-o model.feanc]    model text -> image file
    python3 -m fean run     model.fean | model.feanc       print the query results
    python3 -m fean dump    model.feanc                     readable listing of an image
    python3 -m fean check   model.fean                      parse and check only

Exit codes: 0 success, 1 compile error (syntax, static rule, bad image), 2 model error while running,
3 unreadable input or wrong usage. A source of `-` is read from standard input.
"""
import argparse
import json
import os
import sys
from pathlib import Path

from . import Session, compile_source, dumps, frontend, listing, loads
from .syntax import FeanError, ModelErr


def _read(path):
    if path == '-':
        return sys.stdin.read()
    return Path(path).read_text(encoding='utf-8')


def _shown(path):
    return '<stdin>' if path == '-' else path


def _prelude(args):
    return '' if args.no_prelude else None


def _compile(args):
    if args.source == '-' and not args.output:
        print('fean compile: 표준입력을 읽을 때에는 -o 로 출력 파일을 정해야 합니다', file=sys.stderr)
        return 3
    image = compile_source(_read(args.source), _prelude(args))
    out = Path(args.output) if args.output else Path(args.source).with_suffix('.feanc')
    tmp = out.with_name(out.name + '.tmp')
    tmp.write_text(dumps(image), encoding='utf-8')
    os.replace(tmp, out)                # the image appears whole or not at all
    print(f'{_shown(args.source)} → {out}  (셀 {len(image["cells"])}, 사건 {len(image["events"])}, '
          f'코드 블록 {len(image["codes"])}, 질의 {len(image["queries"])})')
    return 0


def _run(args):
    text = _read(args.path)
    if args.path.endswith('.feanc'):
        image = loads(text)
    else:
        image = compile_source(text, _prelude(args))
    results = Session(image, args.budget).results()
    if args.json:
        print(json.dumps(results, ensure_ascii=False, indent=2))
    else:
        width = max(map(len, results), default=0)
        for name, value in results.items():
            print(f'{name:{width}s} = {value}')
    return 0


def _dump(args):
    sys.stdout.write(listing(loads(_read(args.path))))
    return 0


def _check(args):
    spec = frontend(_read(args.source), _prelude(args))
    print(f'{_shown(args.source)}: 검사 통과 (셀 {len(spec.cells)}, 사건 {len(spec.events)}, '
          f'질의 {len(spec.queries)})')
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog='fean', description='FEAN 엔진: 모델 글을 이미지로 컴파일하고 실행한다.')
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('compile', help='모델 글 → 이미지 파일(.feanc)')
    p.add_argument('source')
    p.add_argument('-o', '--output')
    p.add_argument('--no-prelude', action='store_true', help='표준 계약을 앞에 붙이지 않는다')
    p.set_defaults(action=_compile, shown='source')
    p = sub.add_parser('run', help='모델 글이나 이미지를 실행해 질의 결과를 출력')
    p.add_argument('path')
    p.add_argument('--json', action='store_true')
    p.add_argument('--budget', type=int, default=200_000, help='관찰 지점 수의 한도')
    p.add_argument('--no-prelude', action='store_true')
    p.set_defaults(action=_run, shown='path')
    p = sub.add_parser('dump', help='이미지의 목록을 출력')
    p.add_argument('path')
    p.set_defaults(action=_dump, shown='path')
    p = sub.add_parser('check', help='모델 글의 구문과 규칙만 검사')
    p.add_argument('source')
    p.add_argument('--no-prelude', action='store_true')
    p.set_defaults(action=_check, shown='source')
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:           # argparse has already printed its message
        return 0 if exc.code == 0 else 3
    shown = _shown(getattr(args, args.shown))
    try:
        return args.action(args)
    except OSError as exc:
        print(f'{shown}: {exc.strerror or exc}', file=sys.stderr)
        return 3
    except UnicodeDecodeError:
        print(f'{shown}: UTF-8 글이 아닙니다', file=sys.stderr)
        return 3
    except ModelErr as exc:
        print(f'{shown}: [{exc.code}] {exc.message}', file=sys.stderr)
        return 2
    except FeanError as exc:
        where = f'{shown}:{exc.line}' if exc.line else shown
        print(f'{where}: [{exc.code}] {exc.message}', file=sys.stderr)
        return 1
