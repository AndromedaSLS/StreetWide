"""How op_world writes values in its messages, for Python readers whose
messages must read the same (`changes`, `log`, `glb`). Standard library
only, so every reader can use it.
"""

from __future__ import annotations

import json
import re
import unicodedata

_CONTROL = re.compile(r"[\x00-\x1f\x7f]")
_ESCAPES = {'"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t", "\0": "\\0"}


def rust_debug(value: object) -> str:
    """How Rust's `{:?}` writes a string: in double quotes, with quotes,
    backslashes and control characters escaped, and `\\u{...}` for a
    character that is not printable or that extends the grapheme before it
    (a combining mark, categories Mn and Me). Other values are written as
    JSON. Python's Unicode tables decide what is printable, so a character
    that one of the two Unicode versions does not know yet is written
    differently; Rust also escapes the few characters of
    Other_Grapheme_Extend that are not marks."""
    if not isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    out = ['"']
    for ch in value:
        if ch in _ESCAPES:
            out.append(_ESCAPES[ch])
        elif _CONTROL.match(ch) or not ch.isprintable() or unicodedata.category(ch) in ("Mn", "Me"):
            out.append(f"\\u{{{ord(ch):x}}}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)
