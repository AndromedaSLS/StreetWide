"""Size limits of JSON documents (RFC 0001, section 7), as `op_world` sets
them (crates/op_world/src/limits.rs), with the same messages.

Parsing JSON builds a tree many times the size of its text (serde_json
takes 16 to 105 bytes of memory per byte of text), so every kind of
document has a limit, set from its largest valid document with a wide
margin, and every reader checks it before parsing:

    asset manifests     1 MiB
    merge records       1 MiB
    change sets         8 MiB
    check reports      16 MiB
    tile indexes       16 MiB
    world manifests    64 MiB, the limit of every file (store.MAX_FILE_BYTES):
                       they have no limit of their own, as they grow with
                       the world

Points documents (RFC 0004) and navigation documents (RFC 0005) keep the
64 MiB limit of every file until a limit of their own is set the same way.

Writers check the limits too (`changes.to_bytes`, the tile indexes of
`changes.apply`), so nothing is written that a reader would refuse.
"""

from __future__ import annotations

from dataclasses import dataclass

from tools.worldfmt.store import MAX_FILE_BYTES

MIB = 1 << 20


@dataclass(frozen=True)
class Kind:
    """A kind of JSON document: its `format`, its name in words with its
    article ("an asset manifest") and its limit in bytes."""

    format: str
    name: str
    max_bytes: int

    def fits(self, size: int) -> bool:
        """Whether a document of `size` bytes is within the limit."""
        return size <= self.max_bytes

    def too_large(self) -> str:
        """Why a document of this kind is refused for its size, in
        `Document::too_large`'s words: "larger than 1048576 bytes (1 MiB),
        the limit for an asset manifest"."""
        return f"larger than {self.max_bytes} bytes ({self.max_bytes // MIB} MiB), the limit for {self.name}"


ASSET_MANIFEST = Kind("streetwide.asset", "an asset manifest", MIB)
TILE_INDEX = Kind("streetwide.tile", "a tile index", 16 * MIB)
WORLD_MANIFEST = Kind("streetwide.world", "a world manifest", MAX_FILE_BYTES)
CHANGE_SET = Kind("streetwide.change", "a change set", 8 * MIB)
CHECK_REPORT = Kind("streetwide.check", "a check report", 16 * MIB)
MERGE_RECORD = Kind("streetwide.merge", "a merge record", MIB)

KINDS = (ASSET_MANIFEST, TILE_INDEX, WORLD_MANIFEST, CHANGE_SET, CHECK_REPORT, MERGE_RECORD)
"""Every kind, in the order of `Document::ALL`."""

BY_FORMAT = {kind.format: kind for kind in KINDS}
"""The kinds by their `format`."""
