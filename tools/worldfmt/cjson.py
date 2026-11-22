"""Canonical JSON (RFC 0001, section 5), byte-identical to `op_world::cjson`.

UTF-8, no whitespace, keys sorted by byte value, integers only within
+/-(2^53 - 1), no control characters, and only the escapes for the double
quote and the backslash. `loads` rejects any document whose bytes differ
from `dumps` of its value.
"""

from __future__ import annotations

import json
import re
from typing import Any

MAX_SAFE_INTEGER = 2**53 - 1
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
            parse_constant=_reject_float,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise CanonicalError(f"invalid JSON: {e}") from e
    if dumps(value) != data:
        raise CanonicalError("bytes differ from the canonical form")
    return value


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
