"""World store layout (RFC 0001, section 7), byte-compatible with
`op_world::Store`:

    world.json                  the world manifest
    objects/<aa>/<hex>.json     tile indexes and asset manifests
    objects/<aa>/<hex>.glb      meshes

A store may come from elsewhere (a downloaded world, a contributor's
bundle), so reads check what they read: an object's path comes only from a
well-formed content id, its bytes must hash to that id, and a file must be a
regular file of at most MAX_FILE_BYTES.
"""

from __future__ import annotations

import os
import stat
from pathlib import Path

from tools.worldfmt import cjson
from tools.worldfmt.ids import content_id, hex_of

MAX_FILE_BYTES = 64 << 20
"""Largest file the store reads: the largest asset a manifest may describe
(`op_world`'s MAX_ASSET_BYTES), far more than any JSON document needs."""

_NAME_SURROGATE = 0x20000000
"""The bit of a Windows reparse tag that marks a reparse point naming
another path: a symbolic link or a junction."""


def is_link(info: os.stat_result) -> bool:
    """Whether `info`, from `lstat` or `DirEntry.stat(follow_symlinks=False)`,
    describes a link: a symbolic link, or on Windows a junction, which
    `lstat` reports as a directory, and which needs no privilege to make.
    Readers of untrusted directories follow neither."""
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_reparse_tag", 0) & _NAME_SURROGATE)


def read_file(path: Path, limit: int = MAX_FILE_BYTES, *, follow_links: bool = False) -> bytes:
    """The bytes of a regular file of at most `limit` bytes.

    A crafted store can put a FIFO, a device such as /dev/zero or a link to
    a file outside the store where a file should be (git keeps symbolic
    links), which would block, never end or read another file; those are
    refused (ValueError) before the file is opened. The path must name a
    regular file itself, as `op_world`'s `store::read_file` requires.

    With `follow_links`, a symbolic link is followed, and the file it leads
    to must be a regular file of at most `limit` bytes. That is for the
    ingest's own download cache, which a user may share or manage with
    links; world stores never follow them.
    """
    info = path.stat() if follow_links else path.lstat()
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f"{path} is not a regular file")
    if info.st_size > limit:
        raise ValueError(f"{path} is larger than {limit} bytes")
    with path.open("rb") as f:
        # read(n) allocates n bytes first, so ask for the size, not the limit.
        data = f.read(info.st_size + 1)
    if len(data) != info.st_size:
        raise ValueError(f"{path} changed while it was read")
    return data


class Store:
    def __init__(self, root: Path):
        self.root = Path(root)

    def object_path(self, cid: str, ext: str) -> Path:
        h = hex_of(cid)
        return self.root / "objects" / h[:2] / f"{h}.{ext}"

    def write_object(self, data: bytes, ext: str) -> str:
        if ext not in ("json", "glb"):
            raise ValueError(f"unknown object kind {ext}")
        cid = content_id(data)
        path = self.object_path(cid, ext)
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return cid

    def write_json(self, doc: dict) -> str:
        return self.write_object(cjson.dumps(doc), "json")

    def read_object(self, cid: str, ext: str) -> bytes:
        """The object's bytes, checked against its id. Raises ValueError for a
        malformed id, a mismatching or unreadable object, OSError if missing."""
        data = read_file(self.object_path(cid, ext))
        if content_id(data) != cid:
            raise ValueError(f"object {cid} does not match its hash")
        return data

    def write_world(self, doc: dict) -> str:
        data = cjson.dumps(doc)
        self.root.mkdir(parents=True, exist_ok=True)
        (self.root / "world.json").write_bytes(data)
        return content_id(data)

    def read_world(self) -> tuple[str, dict]:
        """The world id and the parsed world manifest. Raises CanonicalError
        (a ValueError) if world.json is not canonical JSON."""
        data = read_file(self.root / "world.json")
        return content_id(data), cjson.loads(data)
