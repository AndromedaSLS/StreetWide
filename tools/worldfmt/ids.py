"""Content ids (RFC 0001, section 5): `blake3:` + 64 lowercase hex digits."""

from __future__ import annotations

import re

import blake3

PREFIX = "blake3:"
_ID = re.compile(r"blake3:[0-9a-f]{64}")


def content_id(data: bytes) -> str:
    return PREFIX + blake3.blake3(data).hexdigest()


def is_content_id(s: object) -> bool:
    # Ids come from untrusted documents, where they can be any JSON value.
    return isinstance(s, str) and bool(_ID.fullmatch(s))


def hex_of(cid: str) -> str:
    if not is_content_id(cid):
        shown = repr(cid[:80]) if isinstance(cid, str) else f"a {type(cid).__name__}"
        raise ValueError(f"{shown} is not a content id")
    return cid[len(PREFIX):]
