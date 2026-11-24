"""Reference implementation of FEAN, used to check docs/financial-language.md.

It reads the model text exactly as written in the specification, expands and type-checks it,
enumerates every history and evaluates the queries. A solve is answered by trying every candidate,
which is its definition. It is a verification aid for the specification (DR-11), not a production
engine.

    family = load(model_text, prelude_text)
    family.results()        # {query or solve name: result written as in the `// =>` comments}
"""
import re
import sys
from pathlib import Path

_ROOT = str(Path(__file__).resolve().parents[2])        # the engine package `fean` holds the front end (DR-23)
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

from .dynamic import Exploration, Machine, Unit  # noqa: E402
from .query import ENUM_LIMIT, INVALID, NONE, UNKNOWN, Analysis, Family, Solved, Var  # noqa: E402
from .static import build  # noqa: E402
from .syntax import (FeanError, ModelErr, SearchLimit, StaticErr, SyntaxErr, parse_decls,  # noqa: E402
                     parse_model, tokenize)

__all__ = ['Analysis', 'ENUM_LIMIT', 'Exploration', 'Family', 'FeanError', 'INVALID', 'Machine', 'ModelErr', 'NONE',
           'SearchLimit', 'Solved', 'StaticErr', 'SyntaxErr', 'UNKNOWN', 'Unit', 'Var', 'build', 'doc_blocks',
           'expectations', 'load', 'parse_decls', 'parse_model', 'tokenize']

FENCE = re.compile(r'^```([\w-]*)[ \t]*\n(.*?)^```[ \t]*$', re.S | re.M)


def load(src, prelude='', budget=200_000, limit=ENUM_LIMIT):
    """Parse, expand and check a model. The prelude's declarations are placed before the model's."""
    name, decls = parse_model(src)
    return Family(build(name, (parse_decls(prelude) if prelude else []) + decls), budget, limit)


def doc_blocks(path, tag):
    """The bodies of the fenced code blocks tagged `tag` in a Markdown file, in order."""
    return [body for info, body in FENCE.findall(Path(path).read_text(encoding='utf-8')) if info == tag]


def expectations(block):
    """{query or solve name: text after `// =>`} for a model block."""
    out, current = {}, None
    for line in block.splitlines():
        m = re.match(r'\s*(?:query|solve)\s+(\w+)\s*=', line)
        if m:
            current = m.group(1)
        if '// =>' in line and current is not None and current not in out:
            out[current] = line.split('// =>', 1)[1].strip()
    return out
