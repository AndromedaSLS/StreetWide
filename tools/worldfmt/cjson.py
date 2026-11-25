"""Canonical JSON (RFC 0001, section 5), byte-identical to `op_world::cjson`.

UTF-8, no whitespace, keys sorted by byte value, integers only within
+/-(2^53 - 1), no control characters, and only the escapes for the double
quote and the backslash. `loads` rejects any document whose bytes differ
from `dumps` of its value.

`loads` reads untrusted bytes: whatever it is given, it returns a value or
raises CanonicalError.
"""

from __future__ import annotations

import json
import re
from typing import Any

MAX_SAFE_INTEGER = 2**53 - 1
MAX_DEPTH = 127
"""Deepest nesting of arrays and objects in a document. It is the limit of
serde_json's parser (its recursion limit of 128), so `op_world` and this
module accept exactly the same documents, on every platform."""
# The longest integer token in range: "-9007199254740991".
_MAX_INT_CHARS = len(str(-MAX_SAFE_INTEGER))
_KEY = re.compile(r"[a-z][a-z0-9_]*")
_BACKSLASH = chr(92)
_QUOTE = chr(34)


class CanonicalError(ValueError):
    """The value or document is not canonical JSON."""


def dumps(value: Any) -> bytes:
    """Serialises `value` canonically."""
    out: list[str] = []
    _write(value, out)
    return "".join(out).encode("utf-8")


def loads(data: bytes) -> Any:
    """Parses canonical JSON, rejecting anything non-canonical."""
    try:
        text = data.decode("utf-8")
        value = json.loads(
            text,
            object_pairs_hook=_reject_duplicates,
            parse_float=_reject_float,
            parse_int=_parse_int,
            parse_constant=_reject_float,
        )
    except CanonicalError:
        raise
    except ValueError as e:  # invalid UTF-8 or JSON (JSONDecodeError is a ValueError)
        raise CanonicalError(f"invalid JSON: {e}") from e
    except RecursionError as e:  # nested far deeper than MAX_DEPTH
        raise CanonicalError(f"arrays and objects nested deeper than {MAX_DEPTH} levels") from e
    # Checked before `dumps`, which recurses once per level.
    if deeper_than(value, MAX_DEPTH):
        raise CanonicalError(f"arrays and objects nested deeper than {MAX_DEPTH} levels")
    if dumps(value) != data:
        raise CanonicalError("bytes differ from the canonical form")
    return value


def _parse_int(token: str) -> int:
    # Longer tokens are out of range anyway; refusing them here keeps int()
    # from spending quadratic time on (or refusing) thousands of digits.
    if len(token) > _MAX_INT_CHARS:
        raise CanonicalError(f"integer {token[:24]}... is outside +/-(2^53 - 1)")
    return int(token)


def deeper_than(value: Any, limit: int) -> bool:
    """Whether arrays and objects nest more than `limit` levels deep."""
    stack = [(value, 1)]
    while stack:
        v, depth = stack.pop()
        children = v.values() if isinstance(v, dict) else v if isinstance(v, list) else None
        if children is None:
            continue
        if depth > limit:
            return True
        stack.extend((c, depth + 1) for c in children if isinstance(c, (dict, list)))
    return False


def _reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for k, v in pairs:
        if k in out:
            raise CanonicalError(f"duplicate key {k!r}")
        out[k] = v
    return out


def _reject_float(token: str) -> Any:
    raise CanonicalError(f"number {token} is not an integer")


def _write(v: Any, out: list[str]) -> None:
    # bool before int: True is an int in Python.
    if v is None:
        out.append("null")
    elif v is True:
        out.append("true")
    elif v is False:
        out.append("false")
    elif isinstance(v, int):
        if not -MAX_SAFE_INTEGER <= v <= MAX_SAFE_INTEGER:
            raise CanonicalError(f"integer {v} is outside +/-(2^53 - 1)")
        out.append(str(v))
    elif isinstance(v, float):
        raise CanonicalError(f"number {v!r} is not an integer")
    elif isinstance(v, str):
        write_string(v, out)
    elif isinstance(v, (list, tuple)):
        out.append("[")
        for i, item in enumerate(v):
            if i:
                out.append(",")
            _write(item, out)
        out.append("]")
    elif isinstance(v, dict):
        keys = list(v)
        for k in keys:
            if not isinstance(k, str) or not _KEY.fullmatch(k):
                raise CanonicalError(f"object key {k!r} does not match [a-z][a-z0-9_]*")
        out.append("{")
        for i, k in enumerate(sorted(keys, key=lambda s: s.encode("utf-8"))):
            if i:
                out.append(",")
            write_string(k, out)
            out.append(":")
            _write(v[k], out)
        out.append("}")
    else:
        raise CanonicalError(f"cannot serialise {type(v).__name__}")


def write_string(s: str, out: list[str]) -> None:
    out.append(_QUOTE)
    for ch in s:
        code = ord(ch)
        if code < 0x20 or code == 0x7F:
            raise CanonicalError(f"string contains control character U+{code:04X}")
        if 0xD800 <= code <= 0xDFFF:
            raise CanonicalError("string contains a lone surrogate")
        if ch == _QUOTE or ch == _BACKSLASH:
            out.append(_BACKSLASH + ch)
        else:
            out.append(ch)
    out.append(_QUOTE)
