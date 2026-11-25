"""Small synthetic world stores and world logs for the refresh planner's
tests (ADR-0020).

- Meshes: boxes and terrain grids, written by `tools.worldfmt.glb`.
- Assets: manifests in the ingest's style (`swiss_shell`, `osm_shell`,
  `terrain`, `ribbon`, `deck`, `water`, with OpenStreetMap and swissALTI3D
  sources like tools/ingest/pipeline.py writes them) or in a contribution's
  style (`facade`, `contributed`).
- Worlds: `World` writes manifests, meshes, tile indexes and `world.json`,
  over up to four tiles of one region with their own origins (both
  committed world fixtures have one tile only).
- Logs: `Log` writes a baseline record as `op_world baseline` does, and
  merged and takedown records with their change sets, applying each change
  set to the store as a merge would. `worlds/` can be left out.

Everything here is deterministic; nothing reads the network or the cache.
"""

from __future__ import annotations

import dataclasses
import json
import shutil
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from tools.ingest.changes.plan import Plan, make_plan
from tools.worldfmt import cjson, glb, log as worldlog, schemas
from tools.worldfmt.changes import INGEST_AUTHOR, Objects, apply, parse, sort_key, tile_xy, to_bytes
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

NAME = "refresh-test"
REGION = {"key": "12/2145/1434", "anchor_e9": {"lat": 47368594345, "lon": 8569335938}}
A, B, C, D = "17/68646/45903", "17/68647/45903", "17/68646/45904", "17/68647/45904"
E = "17/68648/45903"
"""Tile E is in no world unless a test puts an asset there."""
TILE_MM = 200_000
ORIGINS = {
    A: (0, 0, 0),
    B: (TILE_MM, 0, 0),
    C: (0, 0, TILE_MM),
    D: (TILE_MM, 0, TILE_MM),
    E: (2 * TILE_MM, 0, 0),
}
GROUND_MM = 400_000
SPAWN = (100_000, GROUND_MM + 1_000, 100_000)
"""Region frame: in tile A, 1 m above flat ground, as the ingest places it."""

OSM_V1 = "2026-11-23T08:38:57Z"
OSM_V2 = "2026-11-24T13:28:35Z"
DEM_2026 = ("2026", "swissalti3d_2026_2683-1247")
DEM_2027 = ("2027", "swissalti3d_2027_2683-1247")
SWISS_YEAR = "2025"
INGEST = ("streetwide-ingest", "0.1.0")
CONTRIB = ("streetwide-contrib", "0.1.0")

SWISSTOPO = "LicenseRef-swisstopo-OGD"
OSM_LICENSE = "ODbL-1.0"
OSM_ON_TERRAIN = f"{OSM_LICENSE} AND {SWISSTOPO}"
STAC = "https://data.geo.admin.ch/api/stac/v1/collections/"
REVIEW = {"reviewer": "github:reviewer", "verdict": "better", "notes": "Checked against the evidence."}


# ----------------------------------------------------------------- meshes --


@dataclass(frozen=True)
class Mesh:
    data: bytes
    bounds: dict


def _mesh(name: str, material: str, positions: list[tuple[int, int, int]], indices: list[int]) -> Mesh:
    node = glb.Node(name=name, primitives=[glb.Primitive(
        material=material, positions_mm=positions, normals=[(0.0, 1.0, 0.0)] * len(positions), indices=indices)])
    return Mesh(glb.write([node]), {
        "max": [max(p[k] for p in positions) for k in range(3)],
        "min": [min(p[k] for p in positions) for k in range(3)],
    })


def box_mesh(name: str, lo: Sequence[int], hi: Sequence[int], material: str = "wall") -> Mesh:
    """A closed box from `lo` to `hi` (tile frame, millimetres)."""
    (x0, y0, z0), (x1, y1, z1) = lo, hi
    positions = [(x, y, z) for x in (x0, x1) for y in (y0, y1) for z in (z0, z1)]
    faces = [(0, 1, 3, 2), (4, 6, 7, 5), (0, 4, 5, 1), (2, 3, 7, 6), (0, 2, 6, 4), (1, 5, 7, 3)]
    indices = [i for a, b, c, d in faces for i in (a, b, c, a, c, d)]
    return _mesh(name, material, positions, indices)


def grid_mesh(name: str, heights: Sequence[Sequence[int]], *, x0: int = 0, z0: int = 0,
              size: int = TILE_MM, material: str = "terrain") -> Mesh:
    """A height grid over a square of `size` millimetres: `heights[j][i]` is
    the height at column i (x) and row j (z). The open edges are the
    grid's border."""
    n = len(heights) - 1
    step = size // n
    positions = [(x0 + i * step, heights[j][i], z0 + j * step) for j in range(n + 1) for i in range(n + 1)]
    indices = []
    for j in range(n):
        for i in range(n):
            a, b = j * (n + 1) + i, j * (n + 1) + i + 1
            c, d = a + n + 1, b + n + 1
            indices += [a, c, d, a, d, b]
    return _mesh(name, material, positions, indices)


def flat(n: int = 4, height: int = GROUND_MM) -> list[list[int]]:
    return [[height] * (n + 1) for _ in range(n + 1)]


# ----------------------------------------------------------------- assets --


def source(dataset: str, version: str, license: str, credit: str, url: str, ids: Iterable[str]) -> dict:
    return {"dataset": dataset, "version": version, "license": license, "attribution": credit, "url": url,
            "source_ids": sorted(set(ids))}


def osm(version: str = OSM_V1, ids: Iterable[str] = ("w1",)) -> dict:
    return source("OpenStreetMap", version, OSM_LICENSE, "© OpenStreetMap contributors",
                  "https://www.openstreetmap.org/", ids)


def dem(item: tuple[str, str] = DEM_2026) -> dict:
    return source("swissALTI3D", item[0], SWISSTOPO, "© swisstopo", STAC + "ch.swisstopo.swissalti3d", [item[1]])


def swiss(gml_id: str, year: str = SWISS_YEAR) -> dict:
    return source("swissBUILDINGS3D 3.0 Beta", year, SWISSTOPO, "© swisstopo",
                  STAC + "ch.swisstopo.swissbuildings3d_3_0", [gml_id])


def contribution_source(author: str) -> dict:
    return source("StreetWide contributions", "0.1.0", "CC-BY-SA-4.0", "The StreetWide contributors",
                  "https://github.com/AndromedaSLS/StreetWide", [author])


@dataclass(frozen=True, eq=False)
class Asset:
    """One asset: its mesh and everything its manifest says."""

    layer: str
    feature_id: str
    tile: str
    mesh: Mesh
    collider: str
    license: str
    sources: tuple = ()
    generator: tuple[str, str] = INGEST
    parent: str | None = None
    fictional: bool = False

    @property
    def key(self) -> tuple[str, str]:
        return (self.layer, self.feature_id)

    def manifest(self) -> dict:
        m = {
            "format": "streetwide.asset",
            "contract_version": "0.1.0",
            "content_id": content_id(self.mesh.data),
            "size_bytes": len(self.mesh.data),
            "layer": self.layer,
            "feature_id": self.feature_id,
            "tile": self.tile,
            "bounds_mm": self.mesh.bounds,
            "physics": {"collider": self.collider},
            "fictional": self.fictional,
            "parent_version_id": self.parent,
            "provenance": {
                "license": self.license,
                "attribution": sorted({s["attribution"] for s in self.sources}),
                "sources": [dict(s) for s in self.sources],
                "generator": {"name": self.generator[0], "version": self.generator[1]},
            },
        }
        errors = schemas.errors(m)
        assert not errors, (self.key, errors)
        return m

    @property
    def manifest_bytes(self) -> bytes:
        return cjson.dumps(self.manifest())

    @property
    def manifest_id(self) -> str:
        return content_id(self.manifest_bytes)

    @property
    def content_id(self) -> str:
        return content_id(self.mesh.data)

    def but(self, **changes) -> Asset:
        return dataclasses.replace(self, **changes)

    def write_to(self, store: Store) -> str:
        store.write_object(self.mesh.data, "glb")
        return store.write_object(self.manifest_bytes, "json")


def uuid(n: int) -> str:
    return f"00000000-0000-4000-8000-{n:012d}"


def swiss_shell(n: int, tile: str = A, lo=(10_000, GROUND_MM, 10_000), hi=(20_000, GROUND_MM + 15_000, 20_000), *,
                year: str = SWISS_YEAR) -> Asset:
    fid = f"swisstopo:swissbuildings3d:{uuid(n)}"
    return Asset("shell", fid, tile, box_mesh(fid, lo, hi), "trimesh", SWISSTOPO,
                 (swiss(f"ID_{uuid(n).upper()}", year),))


def osm_shell(way: int, tile: str = A, lo=(30_000, GROUND_MM, 30_000), hi=(40_000, GROUND_MM + 12_000, 40_000), *,
              version: str = OSM_V1, item=DEM_2026) -> Asset:
    fid = f"osm:way:{way}"
    return Asset("shell", fid, tile, box_mesh(fid, lo, hi), "trimesh", OSM_ON_TERRAIN,
                 (osm(version, [f"w{way}"]), dem(item)))


def terrain(tile: str = A, heights=None, *, item=DEM_2026) -> Asset:
    fid = f"tile:{tile}"
    return Asset("terrain", fid, tile, grid_mesh(fid, heights or flat()), "trimesh", SWISSTOPO, (dem(item),))


def ribbon(tile: str = A, lo=(0, GROUND_MM, 90_000), hi=(200_000, GROUND_MM + 100, 110_000), *,
           version: str = OSM_V1, item=DEM_2026, ways: Iterable[str] = ("w900",)) -> Asset:
    fid = f"tile:{tile}"
    return Asset("transport", fid, tile, box_mesh(fid, lo, hi, "road"), "none", OSM_ON_TERRAIN,
                 (osm(version, ways), dem(item)))


def deck(way: int, tile: str = A, lo=(50_000, GROUND_MM + 2_000, 60_000), hi=(70_000, GROUND_MM + 3_000, 64_000), *,
         version: str = OSM_V1, item=DEM_2026) -> Asset:
    fid = f"osm:way:{way}"
    return Asset("transport", fid, tile, box_mesh(fid, lo, hi, "bridge"), "trimesh", OSM_ON_TERRAIN,
                 (osm(version, [f"w{way}"]), dem(item)))


def water(tile: str = A, lo=(120_000, GROUND_MM - 2_000, 120_000), hi=(190_000, GROUND_MM - 1_000, 190_000), *,
          version: str = OSM_V1, item=DEM_2026) -> Asset:
    fid = f"tile:{tile}"
    return Asset("water", fid, tile, box_mesh(fid, lo, hi, "water"), "none", OSM_ON_TERRAIN,
                 (osm(version, ["r7"]), dem(item)))


def facade(shell: Asset, author: str = "github:alice", grow: int = 30) -> Asset:
    """A contributed facade on `shell`: a box `grow` mm outside it."""
    b = shell.mesh.bounds
    lo = [b["min"][0] - grow, b["min"][1], b["min"][2] - grow]
    hi = [b["max"][0] + grow, b["max"][1], b["max"][2] + grow]
    sources = (contribution_source(author),) + tuple(shell.sources)
    return Asset("facade", shell.feature_id, shell.tile, box_mesh(shell.feature_id, lo, hi, "wall"), "none",
                 " AND ".join(sorted({s["license"] for s in sources})), sources, CONTRIB)


def contributed(asset: Asset, author: str = "github:alice", *, parent: str | None = None, mesh: Mesh | None = None,
                keep_sources: bool = True) -> Asset:
    """A contribution's version of `asset`: same key and tile, the
    contribution's generator and source, a new mesh if given."""
    sources = (contribution_source(author),) + (tuple(asset.sources) if keep_sources else ())
    return asset.but(mesh=mesh or asset.mesh, sources=sources, generator=CONTRIB, parent=parent,
                     license=" AND ".join(sorted({s["license"] for s in sources})))


def moved_box(asset: Asset, dx: int = 0, dy: int = 0, dz: int = 0, *, tile: str | None = None,
              material: str = "wall") -> Asset:
    """`asset` with its box shifted by (dx, dy, dz), in its own tile or in
    `tile` (keeping its place in the region frame when it changes tile)."""
    b = asset.mesh.bounds
    tile = tile or asset.tile
    shift = [ORIGINS[asset.tile][k] - ORIGINS[tile][k] + d for k, d in enumerate((dx, dy, dz))]
    lo = [b["min"][k] + shift[k] for k in range(3)]
    hi = [b["max"][k] + shift[k] for k in range(3)]
    return asset.but(tile=tile, mesh=box_mesh(asset.feature_id, lo, hi, material))


# ----------------------------------------------------------------- worlds --


@dataclass
class World:
    """A world built from assets. `write` makes a store of it."""

    assets: dict = field(default_factory=dict)
    name: str = NAME
    spawn: tuple[int, int, int] = SPAWN
    yaw: int = 0

    @staticmethod
    def of(*assets: Asset, **kw) -> World:
        w = World(**kw)
        w.put(*assets)
        return w

    def put(self, *assets: Asset) -> World:
        for a in assets:
            self.assets[a.key] = a
        return self

    def drop(self, *keys: tuple[str, str]) -> World:
        for k in keys:
            del self.assets[k]
        return self

    def copy(self) -> World:
        return World(dict(self.assets), self.name, self.spawn, self.yaw)

    def __getitem__(self, key: tuple[str, str]) -> Asset:
        return self.assets[key]

    def world_doc(self, ids: dict[tuple[str, str], str] | None = None) -> tuple[dict, dict[str, bytes]]:
        """The world manifest and its tile indexes (by id)."""
        by_tile: dict[str, list[dict]] = {}
        credits: set[str] = set()
        for a in self.assets.values():
            m = a.manifest()
            by_tile.setdefault(a.tile, []).append({
                "layer": a.layer, "feature_id": a.feature_id,
                "manifest_id": (ids or {}).get(a.key) or a.manifest_id,
                "content_id": a.content_id, "license": a.license, "collider": a.collider,
                "bounds_mm": a.mesh.bounds,
            })
            credits.update(m["provenance"]["attribution"])
        tiles, indexes = [], {}
        for tile in sorted(by_tile, key=tile_xy):
            entries = sorted(by_tile[tile], key=lambda e: (e["layer"].encode(), e["feature_id"].encode()))
            index = {"format": "streetwide.tile", "contract_version": "0.1.0", "tile": tile,
                     "region": REGION["key"], "origin_mm": list(ORIGINS[tile]), "assets": entries}
            data = cjson.dumps(index)
            indexes[content_id(data)] = data
            tiles.append({
                "tile": tile, "tile_id": content_id(data), "origin_mm": list(ORIGINS[tile]),
                "bounds_mm": {"max": [max(e["bounds_mm"]["max"][k] for e in entries) for k in range(3)],
                              "min": [min(e["bounds_mm"]["min"][k] for e in entries) for k in range(3)]},
            })
        world = {"format": "streetwide.world", "contract_version": "0.1.0", "name": self.name, "region": REGION,
                 "attribution": sorted(credits, key=lambda s: s.encode()),
                 "spawn": {"position_mm": list(self.spawn), "yaw": self.yaw}, "tiles": tiles}
        assert not schemas.errors(world), schemas.errors(world)
        return world, indexes

    def write(self, root: Path) -> str:
        """Writes the store; returns its world id."""
        store = Store(Path(root))
        for a in self.assets.values():
            a.write_to(store)
        world, indexes = self.world_doc()
        for data in indexes.values():
            store.write_object(data, "json")
        return store.write_world(world)


def world_id(root: Path) -> str:
    return Store(Path(root)).read_world()[0]


# ------------------------------------------------------------------- logs --


def entry_add(asset: Asset) -> dict:
    return {"op": "add", "tile": asset.tile, "layer": asset.layer, "feature_id": asset.feature_id,
            "parent_version_id": None, "manifest_id": asset.manifest_id}


def entry_replace(asset: Asset) -> dict:
    assert asset.parent is not None, "a replacing version names its parent"
    return {"op": "replace", "tile": asset.tile, "layer": asset.layer, "feature_id": asset.feature_id,
            "parent_version_id": asset.parent, "manifest_id": asset.manifest_id}


def entry_remove(current: Asset, manifest_id: str | None = None) -> dict:
    return {"op": "remove", "tile": current.tile, "layer": current.layer, "feature_id": current.feature_id,
            "parent_version_id": manifest_id or current.manifest_id, "manifest_id": None}


def current_manifest_id(root: Path, layer: str, feature_id: str) -> str:
    """The manifest id the store at `root` lists for a key."""
    store = Store(Path(root))
    _, world = store.read_world()
    for wt in world["tiles"]:
        for a in cjson.loads(store.read_object(wt["tile_id"], "json"))["assets"]:
            if (a["layer"], a["feature_id"]) == (layer, feature_id):
                return a["manifest_id"]
    raise KeyError((layer, feature_id))


class Log:
    """A world log in the layout of tools/worldfmt/log.py, kept next to a
    store as merges would keep it."""

    def __init__(self, root: Path, *, keep_worlds: bool = True):
        self.root = Path(root)
        self.keep_worlds = keep_worlds
        self.records: list[tuple[str, dict]] = []

    def _put(self, rel: str, data: bytes) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def _append(self, record: dict, world_bytes: bytes) -> dict:
        assert worldlog.record_problems(record) == [], worldlog.record_problems(record)
        if self.keep_worlds:
            self._put(worldlog.world_path(record["world_after"]), world_bytes)
        data = cjson.dumps(record)
        self._put(worldlog.record_path(len(self.records)), data)
        self.records.append((content_id(data), record))
        return record

    def baseline(self, store_root: Path, reason: str = "Baseline of the test world.") -> dict:
        """The record `op_world baseline <store> --log <log> --reason <reason>` writes."""
        world_bytes = (Path(store_root) / "world.json").read_bytes()
        return self._append({
            "format": "streetwide.merge", "contract_version": "0.1.0", "decision": "baseline",
            "change_id": None, "world_before": None, "world_after": content_id(world_bytes),
            "check_report_id": None, "reviews": [], "reasons": reason, "previous_record_id": None,
        }, world_bytes)

    def merge(self, store_root: Path, entries: list[dict], assets: Iterable[Asset] = (), *,
              author: str = "github:alice", decision: str = "merged", reason: str = "Test merge.") -> str:
        """Applies a change set to the store as a merge does and appends its
        record; returns the new world id."""
        store = Store(Path(store_root))
        before_id, world = store.read_world()
        for a in assets:
            a.write_to(store)
        change = to_bytes({
            "format": "streetwide.change", "contract_version": "0.1.0", "base_world_id": before_id,
            "title": "Test change set", "author": author, "rationale": "Made by a test.",
            "changes": sorted(entries, key=sort_key),
        })
        candidate = apply(world, parse(change), Objects(store))
        after_id = candidate.write(store)
        report = cjson.dumps({"note": f"check report for {content_id(change)}"})
        previous_id, _ = self.records[-1]
        self._put(worldlog.change_path(content_id(change)), change)
        self._put(worldlog.report_path(content_id(report)), report)
        self._append({
            "format": "streetwide.merge", "contract_version": "0.1.0", "decision": decision,
            "change_id": content_id(change), "world_before": before_id, "world_after": after_id,
            "check_report_id": content_id(report), "reviews": [dict(REVIEW)] if decision == "merged" else [],
            "reasons": reason, "previous_record_id": previous_id,
        }, candidate.world_bytes)
        return after_id

    def refresh(self, store_root: Path, entries: list[dict], assets: Iterable[Asset] = ()) -> str:
        """A merged refresh set, authored by streetwide:ingest."""
        return self.merge(store_root, entries, assets, author=INGEST_AUTHOR, reason="Test refresh.")


# ------------------------------------------------------------- scenarios --


@dataclass
class Scenario:
    """A current store with its log, ready to plan against new stores."""

    root: Path
    current: Path
    log: Log
    plans: int = 0

    @staticmethod
    def start(tmp_path: Path, world: World, *, keep_worlds: bool = True) -> Scenario:
        current = tmp_path / "current"
        world.write(current)
        log = Log(tmp_path / "log", keep_worlds=keep_worlds)
        log.baseline(current)
        return Scenario(tmp_path, current, log)

    def write_new(self, world: World, name: str | None = None) -> Path:
        self.plans += 1
        root = self.root / (name or f"new-{self.plans}")
        if root.exists():
            shutil.rmtree(root)
        world.write(root)
        return root

    def overrides(self, doc: dict | None) -> Path | None:
        if doc is None:
            return None
        path = self.root / f"overrides-{self.plans}.json"
        path.write_bytes(json.dumps({"overrides_version": "0.1.0", **doc}, indent=2).encode())
        return path

    def plan(self, new: World | Path, *, overrides: dict | None = None, max_set_bytes: int | None = None) -> Plan:
        root = new if isinstance(new, Path) else self.write_new(new)
        return make_plan(self.current, root, self.log.root, self.overrides(overrides), max_set_bytes)

    def args(self, new: World | Path, out: Path | None = None, *, overrides: dict | None = None) -> list[str]:
        """Command-line arguments of `plan` for this scenario."""
        root = new if isinstance(new, Path) else self.write_new(new)
        out = out or self.root / f"out-{self.plans}"
        args = ["plan", "--current", str(self.current), "--new", str(root), "--log", str(self.log.root),
                "--out", str(out)]
        path = self.overrides(overrides)
        return args + (["--overrides", str(path)] if path else [])


def entries(plan: Plan, kind: str = "conflicts") -> dict[tuple[str, str], list[str]]:
    """`{(layer, feature_id): [codes]}` of the report's conflicts or held-back entries."""
    return {(e["layer"], e["feature_id"]): [r["code"] for r in e["reasons"]] for e in plan.report[kind]}


def held(plan: Plan) -> dict[tuple[str, str], list[str]]:
    """Every entry that is not emitted, conflicts and held back together."""
    return {**entries(plan, "conflicts"), **entries(plan, "held_back")}


def emitted(plan: Plan) -> list[tuple[str, str, str, str]]:
    """`(set path, op, layer, feature_id)` of every emitted change, in order."""
    return [(s.path, e["op"], e["layer"], e["feature_id"]) for s in plan.sets for e in s.entries]


def ops(plan: Plan) -> dict[tuple[str, str], list[str]]:
    out: dict[tuple[str, str], list[str]] = {}
    for _, op, layer, fid in emitted(plan):
        out.setdefault((layer, fid), []).append(op)
    return out
