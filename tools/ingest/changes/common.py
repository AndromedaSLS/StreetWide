"""Shared by the planner's modules: errors, orders, boxes and small helpers.

Everything here is integer arithmetic and sorted iteration, so the plan is
the same on every platform (ADR-0020, Context).
"""

from __future__ import annotations

import re
from collections.abc import Callable, Hashable, Iterable, Sequence
from dataclasses import dataclass
from typing import TypeVar

Key = tuple[str, str]
"""An asset's key, `(layer, feature_id)`: assets are matched by it."""

T = TypeVar("T")


class PlanError(Exception):
    """A usage or input error: the plan is not written, exit code 2."""


class InternalError(Exception):
    """A plan-time check failed (`checks`): a bug in the planner. Nothing
    is written, exit code 1."""


def key_order(key: Key) -> tuple[bytes, bytes]:
    """Keys in byte order, as tile indexes sort their assets."""
    return key[0].encode("utf-8"), key[1].encode("utf-8")


def text_order(s: str) -> bytes:
    return s.encode("utf-8")


def show(key: Key) -> str:
    """`shell osm:way:123`, as messages name an asset."""
    return f"{key[0]} {key[1]}"


_VERSION_PART = re.compile(r"[0-9]+|[^0-9]+")


def version_order(version: str) -> tuple:
    """An order for version strings such as `0.1.0` or `0.10.2-rc1`:
    runs of ASCII digits compare as numbers, everything else as text
    (other digits, such as `²`, included)."""
    return tuple((0, int(p), "") if p.isascii() and p.isdigit() else (1, 0, p)
                 for p in _VERSION_PART.findall(version))


# ------------------------------------------------------------ union-find --


class UnionFind:
    """Union-find over hashable items. The root of a set is its smallest
    member under `order`, so components do not depend on the order of the
    unions."""

    def __init__(self, items: Iterable[Hashable], order: Callable[[Hashable], object]):
        self._parent = {i: i for i in items}
        self._order = order

    def find(self, x: Hashable) -> Hashable:
        root = x
        while self._parent[root] != root:
            root = self._parent[root]
        while self._parent[x] != root:
            self._parent[x], x = root, self._parent[x]
        return root

    def union(self, a: Hashable, b: Hashable) -> None:
        ra, rb = self.find(a), self.find(b)
        if ra == rb:
            return
        if self._order(rb) < self._order(ra):
            ra, rb = rb, ra
        self._parent[rb] = ra

    def groups(self) -> list[list[Hashable]]:
        """The components, each sorted by `order`, sorted by their smallest
        member."""
        by_root: dict[Hashable, list[Hashable]] = {}
        for x in sorted(self._parent, key=self._order):
            by_root.setdefault(self.find(x), []).append(x)
        return sorted(by_root.values(), key=lambda g: self._order(g[0]))


# ------------------------------------------------------------------ boxes --


@dataclass(frozen=True)
class Box:
    """Axis-aligned bounds in the region frame, in integer millimetres:
    an asset's `bounds_mm` plus its tile's `origin_mm`."""

    lo: tuple[int, int, int]
    hi: tuple[int, int, int]

    @staticmethod
    def of(bounds_mm: dict, origin_mm: Sequence[int]) -> Box:
        return Box(
            tuple(bounds_mm["min"][k] + origin_mm[k] for k in range(3)),
            tuple(bounds_mm["max"][k] + origin_mm[k] for k in range(3)),
        )

    def area(self) -> int:
        """The horizontal area (x and z), in square millimetres."""
        return max(0, self.hi[0] - self.lo[0]) * max(0, self.hi[2] - self.lo[2])

    def contains(self, point: Sequence[int]) -> bool:
        return all(self.lo[k] <= point[k] <= self.hi[k] for k in range(3))

    def contains_xz(self, x: int, z: int) -> bool:
        return self.lo[0] <= x <= self.hi[0] and self.lo[2] <= z <= self.hi[2]

    def as_list(self) -> dict:
        return {"max": list(self.hi), "min": list(self.lo)}


def overlap_area(a: Box, b: Box) -> int:
    """The horizontal overlap of two boxes (x and z), in square millimetres."""
    dx = min(a.hi[0], b.hi[0]) - max(a.lo[0], b.lo[0])
    dz = min(a.hi[2], b.hi[2]) - max(a.lo[2], b.lo[2])
    return dx * dz if dx > 0 and dz > 0 else 0


def overlaps(a: Box, b: Box) -> bool:
    """ADR-0020, Q8: two assets overlap when their horizontal overlap is
    positive and at least half the area of the smaller box. A box with no
    area overlaps nothing."""
    shared = overlap_area(a, b)
    return shared > 0 and 2 * shared >= min(a.area(), b.area())


def touches(a: Box, b: Box) -> bool:
    """Any positive horizontal overlap (H6, for keys a takedown removed)."""
    return overlap_area(a, b) > 0


CELL_MM = 1 << 16
"""Cell size of `BoxGrid`, about 65 m."""
MAX_CELLS = 1024
"""A box over more cells than this is kept aside and compared with all."""


class BoxGrid:
    """A grid over horizontal boxes, to find pairs that may overlap without
    comparing every pair. Boxes without area are not indexed: they overlap
    nothing. Very large boxes are not split into cells but compared with
    every query, so no box costs more than `MAX_CELLS` cells."""

    def __init__(self) -> None:
        self._cells: dict[tuple[int, int], list[int]] = {}
        self._items: list[tuple[Box, object]] = []
        self._large: list[int] = []

    @staticmethod
    def _span(box: Box) -> tuple[range, range]:
        return (range(box.lo[0] // CELL_MM, box.hi[0] // CELL_MM + 1),
                range(box.lo[2] // CELL_MM, box.hi[2] // CELL_MM + 1))

    def add(self, box: Box, value: object) -> None:
        if box.area() == 0:
            return
        index = len(self._items)
        self._items.append((box, value))
        xs, zs = self._span(box)
        if len(xs) * len(zs) > MAX_CELLS:
            self._large.append(index)
            return
        for cx in xs:
            for cz in zs:
                self._cells.setdefault((cx, cz), []).append(index)

    def near(self, box: Box) -> list[tuple[Box, object]]:
        """Indexed boxes that may overlap `box`, in the order they were
        added."""
        if box.area() == 0:
            return []
        xs, zs = self._span(box)
        if len(xs) * len(zs) > MAX_CELLS:
            return list(self._items)
        found: set[int] = set(self._large)
        for cx in xs:
            for cz in zs:
                found.update(self._cells.get((cx, cz), ()))
        return [self._items[i] for i in sorted(found)]


# ------------------------------------------------------------------- text --


def joined(items: Sequence[str], limit: int = 8) -> str:
    """`a, b and c`; past `limit` items, `a, b, ... and N more`."""
    items = list(items)
    if len(items) > limit:
        shown = items[:limit]
        return ", ".join(shown) + f" and {len(items) - limit} more"
    if len(items) <= 1:
        return "".join(items)
    return ", ".join(items[:-1]) + " and " + items[-1]


def clip(text: str, max_chars: int) -> str:
    """`text`, cut to at most `max_chars` characters with "..." at the end
    when it is longer."""
    if len(text) <= max_chars:
        return text
    return text[: max(0, max_chars - 3)] + "..."


def counted(n: int, one: str, many: str | None = None) -> str:
    """`1 change`, `2 changes`."""
    return f"{n} {one if n == 1 else (many or one + 's')}"


def unique_sorted(values: Iterable[str]) -> list[str]:
    return sorted(set(values), key=text_order)
