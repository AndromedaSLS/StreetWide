"""The hold codes (ADR-0020, section 6), in rule order.

A pending change is emitted only when no code is left on it. A *conflict*
code lists the change under `conflicts` in the report; the other codes hold
it back (`held_back`). An accept in the overrides file lifts only the
liftable codes it names; an allowance lifts the two removal guards.
Accepting `contribution`, `origin-unclear` or `removed-by-contribution`
puts the change's whole unit into a decision set of its own.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Code:
    code: str
    rule: str
    kind: str
    """`conflict` or `held`."""
    why_not: str | None
    """Why an accept cannot lift it, or None when it can."""
    todo: str
    """What the maintainer can do, for the Markdown report."""

    @property
    def liftable(self) -> bool:
        return self.why_not is None


CODES = (
    Code("takedown", "H1", "conflict",
         "a refresh never changes a key whose last change was a takedown; "
         "reversing it is a maintainer's own change set (Q18)",
         "A takedown froze this key. A refresh never adds it back or changes it. Reversing the takedown is a "
         "maintainer's own change set, not part of a refresh (Q18: until it is decided, nothing lifts this)."),
    Code("contribution", "H2", "conflict", None,
         "A contribution holds this key, and upstream changed, gained or lost the feature since the version the "
         "contribution replaced. Review it. The contribution stays either way; a hold with its content key records "
         "that you reviewed it, until upstream changes again. Or accept the change with a reason: it then goes into "
         "a decision set of its own."),
    Code("origin-unclear", "H3", "conflict", None,
         "The manifest and the world log disagree about who made this asset. Check the log. Accepting the change "
         "puts it into a decision set of its own."),
    Code("removed-by-contribution", "H4", "conflict", None,
         "A contribution removed this key, and upstream still has the feature. Accepting the add puts it into a "
         "decision set of its own."),
    Code("dependent-contribution", "H5", "conflict",
         "remove or move the facade first (RFC 0002 G8)",
         "A contributed facade sits on this shell, and G8 never checks the facade again when only the shell "
         "changes. Nothing lifts this (Q6, Proposed): the facade is removed or moved first, by a change set of "
         "its own, or the shell waits for amendment A2."),
    Code("possible-duplicate", "H6", "conflict", None,
         "This new feature, or this feature whose bounds grow, newly overlaps a contribution, or a feature that a "
         "contribution or a takedown removed. Accept it with a reason if it is not the same feature coming back, "
         "under a new id or inside another one."),
    Code("removed-bounds-unknown", "H6b", "conflict", None,
         "A feature that a contribution or a takedown removed lay in this tile, and its bounds can no longer be "
         "read. Accept the change with a reason if it is not that feature."),
    Code("tile-not-in-world", "H7", "held",
         "the gate would fail it (G2)",
         "The tile is not in the current world, and change sets cannot add tiles (amendment A5)."),
    Code("terrain-edges", "H8", "held",
         "the gate would fail it (G8)",
         "The new terrain moves vertices on the tile's open edges, which the gate fails (G8). It waits for "
         "amendment A1 (Q9, Proposed)."),
    Code("height-coupled", "H9", "held", None,
         "This change is draped on the same height-model items as a change that is held, or on the height model "
         "over a contributed terrain that stays (Q17), so it would float or sink. Accept it with a reason to let it "
         "through on its own."),
    Code("per-tile-removal", "H10a", "held", None,
         "This removes a tile's whole terrain, ribbons or water. If upstream really lost it, add an "
         "allow_removals entry for its ingest group, or accept it."),
    Code("mass-removal", "H10b", "held", None,
         "Unusually many removals in one ingest group that no add, move or replace of their unit accounts for. "
         "Check upstream; then add an allow_removals entry for the group, or accept them."),
    Code("unexplained-change", "H11", "held", None,
         "The content changed, but the sources, their versions and the generator did not: another platform, an "
         "ingest change made without a VERSION bump, or an item republished in place under the same id. Accept "
         "it with a reason once explained."),
    Code("manual-hold", "H12", "held",
         "remove the hold instead",
         "The overrides file holds it. Remove the hold to let it through."),
    Code("unit-held", "H13", "held",
         "units are emitted whole; lift the codes of the held member instead",
         "Another change of its unit is held, and a unit is emitted whole."),
    Code("unit-too-large", "H13", "held",
         "a change set holds at most 50 changes (RFC 0002 section 2)",
         "Its unit has more than 50 changes, the cap of one change set (amendment A3)."),
    Code("would-empty-tile", "H13", "held",
         "the gate would fail it (G1)",
         "The removals would leave the tile empty, which the gate fails (G1), if only until a later set of this "
         "refresh fills it again. In that case, plan again once this refresh is merged."),
    Code("spawn", "H14", "held",
         "change sets cannot move the spawn (amendment A5)",
         "The change would bury the spawn or put a collider where the player stands at the spawn, and change sets "
         "cannot move the spawn (amendment A5)."),
)

BY_CODE = {c.code: c for c in CODES}
ORDER = {c.code: i for i, c in enumerate(CODES)}
LIFTABLE = tuple(c.code for c in CODES if c.liftable)
DECISION = ("contribution", "origin-unclear", "removed-by-contribution")
"""Accepting one of these puts the unit into a decision set (ADR-0020, section 5)."""
ALLOWANCE = ("per-tile-removal", "mass-removal")
"""The codes an allowance lifts (ADR-0020, section 6)."""


def in_rule_order(codes) -> list[str]:
    return sorted(codes, key=ORDER.__getitem__)


def is_conflict(code: str) -> bool:
    return BY_CODE[code].kind == "conflict"
