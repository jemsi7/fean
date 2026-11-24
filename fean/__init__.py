"""FEAN engine: model text -> checked model -> image (intermediate representation) -> results.

    image = compile_source(text)        # ahead of time: parse, expand, check, lower, verify
    open('m.feanc', 'w').write(dumps(image))
    results = run_image(image)          # {query or solve name: result written as in the specification}

The language is defined in docs/financial-language.md, the image and the machine in docs/engine.md.
Command line: python3 -m fean compile|run|dump|check (see fean/cli.py).
"""
from pathlib import Path

from . import lower as _lowering        # `fean.lower` stays the module; its function is not re-exported
from .ir import FORMAT, LANGUAGE, OPS, VERSION, ImageErr, dumps, listing, loads, verify
from .static import Spec, build
from .syntax import FeanError, ModelErr, SearchLimit, StaticErr, SyntaxErr, parse_decls, parse_model, tokenize
from .vm import ENUM_LIMIT, INVALID, NONE, UNKNOWN, Session, Solved, Vm

__all__ = ['ENUM_LIMIT', 'FORMAT', 'FeanError', 'INVALID', 'ImageErr', 'LANGUAGE', 'ModelErr', 'NONE', 'OPS',
           'STD_PRELUDE', 'SearchLimit', 'Session', 'Solved', 'Spec', 'StaticErr', 'SyntaxErr', 'UNKNOWN', 'VERSION',
           'Vm', 'build', 'compile_source', 'dumps', 'frontend', 'listing', 'loads', 'parse_decls', 'parse_model',
           'run_image', 'run_source', 'tokenize', 'verify']

# The standard contracts (docs/financial-language.md section 6), placed before every model.
STD_PRELUDE = Path(__file__).with_name('prelude.fean').read_text(encoding='utf-8')


def frontend(src, prelude=None):
    """Parse, expand and check a model text. prelude=None uses the standard contracts, '' uses none."""
    name, decls = parse_model(src)
    before = STD_PRELUDE if prelude is None else prelude
    return build(name, (parse_decls(before) if before else []) + decls)


def compile_source(src, prelude=None):
    """Ahead-of-time compilation: model text -> verified image."""
    return verify(_lowering.lower(frontend(src, prelude)))


def run_image(image, budget=200_000):
    """Run an image: {query or solve name: result text}."""
    return Session(image, budget).results()


def run_source(src, prelude=None, budget=200_000):
    """Compile, then run."""
    return run_image(compile_source(src, prelude), budget)
