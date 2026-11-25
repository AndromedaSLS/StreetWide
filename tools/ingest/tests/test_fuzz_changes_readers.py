"""Fuzzing the refresh planner's readers (ADR-0020; roadmap M1): the
overrides file, a written plan (`layout.read_plan`), the two stores
(`stores.read_store`, and the content and meshes it reads on request) and
the gate's report as `verify` checks it. Each must accept its input or
raise its documented error: PlanError for the overrides and the stores,
PlanFileError for a plan, and for `verify` a VerifyError (exit code 2) or a
result that fails (exit code 1). Never another exception, never slowly, and
never with memory out of proportion to its input.

Every file goes through `common.read_input`, so the cases also put
directories, files over the size limit and, where the system allows them,
links and FIFOs where files should be. A read that opened a FIFO would
block, and the test would end as hung (fuzzing.HANG_S).

See tools/worldfmt/tests/fuzzing.py for the knobs (STREETWIDE_FUZZ_ITERS,
STREETWIDE_FUZZ_SEED); the regression tests at the end replay the inputs
that used to fail.
"""

from __future__ import annotations

import copy
import json
import os
import shutil
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path

import pytest

from tools.ingest.changes import codes, common
from tools.ingest.changes.common import PlanError, entries_under, read_input
from tools.ingest.changes.holds import ground_under
from tools.ingest.changes.layout import CHANGE_JSON, REPORT_JSON, PlanFileError, read_plan
from tools.ingest.changes.overrides import (
    accept_matches, hold_matches, loads_overrides, parse_overrides, read_overrides,
)
from tools.ingest.changes.plan import make_plan, write_plan
from tools.ingest.changes.stores import read_manifest, read_store
from tools.ingest.changes.verify import Options, VerifyError, read_gate_report, verify
from tools.ingest.tests.refresh_worlds import A, B, GROUND_MM, Log, Scenario, World, swiss_shell, terrain
from tools.ingest.tests.test_changes_fixture import BASELINE_REASON, FIXTURE_WORLD, pre_water_store
from tools.worldfmt import cjson, glb
from tools.worldfmt.limits import CHECK_REPORT
from tools.worldfmt.changes import INGEST_AUTHOR, Objects, apply, parse
from tools.worldfmt.ids import content_id, hex_of
from tools.worldfmt.store import Store, is_link
from tools.worldfmt.tests.fuzzing import (
    MEMORY,
    Rng,
    dumps_like,
    mutate_bytes,
    mutate_json,
    mutate_json_bytes,
    reading,
    run_cases,
)

REPO = Path(__file__).resolve().parents[3]
RUST_REPORT = REPO / "fixtures" / "changes" / "facade-sample" / "report.json"
LIMIT = 1 << 20
"""`common.MAX_INPUT_BYTES` while a test writes files over the limit: the
largest fixture file (a terrain mesh of 744 KB) fits, and a file over the
limit costs 1 MiB instead of 64."""
FIFOS = hasattr(os, "mkfifo")
ODD_SUFFIX = ".g" + chr(0xDCFF)
"""A file name extension that is not UTF-8: Linux gives a byte 0xFF in a
name as this surrogate, and Windows file names can hold it as such."""


# Files a read must refuse ---------------------------------------------------


def remove(path: Path) -> None:
    """Removes whatever is at `path`: a file, a link, a junction, a FIFO or a
    directory, never following a link."""
    try:
        info = path.lstat()
    except OSError:  # nothing there, or a name this file system cannot hold
        return
    if stat.S_ISDIR(info.st_mode) and not is_link(info):
        shutil.rmtree(path)
    else:
        path.unlink()  # on Windows, also a junction or a link to a directory


def link_directory(target: Path, link: Path) -> bool:
    """Makes `link` a link to the directory `target`: a junction on Windows,
    which needs no privilege, else a symbolic link. False if this system
    cannot."""
    try:
        if os.name == "nt":
            import _winapi

            _winapi.CreateJunction(str(target.resolve()), str(link))
        else:
            os.symlink(target, link, target_is_directory=True)
    except (OSError, NotImplementedError):
        return False
    return True


def can_link(tmp: Path) -> bool:
    """Whether this system lets the tests make symbolic links (Windows needs
    a privilege or developer mode)."""
    probe = tmp / "link-probe"
    try:
        os.symlink(tmp, probe, target_is_directory=True)
    except (OSError, NotImplementedError):
        return False
    probe.unlink()
    return True


UNREADABLE = ("directory", "oversize", "link", "fifo")


def put_unreadable(kind: str, path: Path, data: bytes, links: Path | None) -> None:
    """Puts at `path` (which must be free) something `read_input` refuses:
    a directory, a file one byte over LIMIT, a link to a file holding `data`
    in the directory `links`, or a FIFO. A link or a FIFO the system does
    not allow becomes a directory, so the cases draw the same numbers on
    every system."""
    if kind == "oversize":
        path.write_bytes(b" " * (LIMIT + 1))
    elif kind == "link" and links is not None:
        target = links / path.name
        target.write_bytes(data)
        os.symlink(target, path)
    elif kind == "fifo" and FIFOS:
        os.mkfifo(path)
    else:
        path.mkdir()


class Workspace:
    """A directory whose files one case changes and then puts back. `files`
    holds the original files, by POSIX path relative to `root`. Copies of
    directories that a case replaces by links go into `copies`, beside
    `root`."""

    def __init__(self, root: Path, files: dict[str, bytes], links: Path | None):
        self.root, self.files, self.links = root, files, links
        self.copies = root.parent / f"{root.name}-copies"
        self.touched: set[str] = set()
        self.written = 0
        for rel, data in files.items():
            self._free(rel).write_bytes(data)
        self.touched.clear()

    def path(self, rel: str) -> Path:
        return self.root.joinpath(*rel.split("/"))

    def _free(self, rel: str) -> Path:
        self.touched.add(rel)
        path = self.path(rel)
        remove(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        return path

    def write(self, rel: str, data: bytes) -> None:
        try:
            self._free(rel).write_bytes(data)
        except OSError:
            if rel.isascii():
                raise
            return  # a name that is not UTF-8, which this file system refuses (macOS)
        self.written += len(data)

    def delete(self, rel: str) -> None:
        self.touched.add(rel)
        remove(self.path(rel))

    def link_directory(self, rel: str) -> None:
        """Replaces the directory `rel` by a link (a junction on Windows) to
        a copy of it outside `root`: a reader that follows links takes it."""
        path = self.path(rel)
        try:
            info = path.lstat()
        except OSError:
            return
        if not stat.S_ISDIR(info.st_mode) or is_link(info):
            return
        self.copies.mkdir(exist_ok=True)
        target = self.copies / str(len(os.listdir(self.copies)))
        self.touched.add(rel)
        os.replace(path, target)
        if not link_directory(target, path):
            os.replace(target, path)

    def unreadable(self, rng: Rng, rel: str) -> None:
        kind = rng.choice(UNREADABLE)
        try:
            put_unreadable(kind, self._free(rel), self.files.get(rel, b"{}"), self.links)
        except OSError:
            if rel.isascii():
                raise

    def size(self) -> int:
        """The bytes of the input a case reads from: the original files and
        what the case wrote."""
        return sum(len(d) for d in self.files.values()) + self.written

    def restore(self) -> None:
        """Puts back every original file the case touched, also those under
        a directory it removed, and removes what it added. Directories it
        made for added files stay, empty."""
        for rel in sorted(self.touched, key=lambda r: -r.count("/")):
            remove(self.path(rel))
        for rel in sorted(self.files):
            if any(rel == t or rel.startswith(t + "/") for t in self.touched):
                path = self.path(rel)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(self.files[rel])
        self.touched.clear()
        self.written = 0
        remove(self.copies)


def files_under(root: Path) -> dict[str, bytes]:
    return {p.relative_to(root).as_posix(): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


def object_rel(cid: str, ext: str) -> str:
    h = hex_of(cid)
    return f"objects/{h[:2]}/{h}.{ext}"


def encode(rng: Rng, doc) -> bytes:
    """`doc` as canonical JSON when it can be, so the case reaches the
    checks after parsing; else as other JSON."""
    if rng.chance(90):
        try:
            return cjson.dumps(doc)
        except Exception:  # not representable in canonical JSON: written another way
            pass
    return dumps_like(rng, doc)


# The overrides file --------------------------------------------------------

LIFTABLE = [c.code for c in codes.CODES if c.liftable]
ID1, ID2 = content_id(b"current version"), content_id(b"new version")
OVERRIDES = {
    "overrides_version": "0.1.0",
    "hold": [
        {"layer": "shell", "feature_id": "osm:way:1", "reason": "Wait for the survey."},
        {"layer": "shell", "feature_id": "osm:way:2", "current_manifest_id": ID1, "content_key": None,
         "reason": "Scoped to this version."},
    ],
    "accept": [
        {"layer": "terrain", "feature_id": f"tile:{A}", "current_manifest_id": ID1, "content_key": ID2,
         "lifts": LIFTABLE[:1], "reason": "Checked against the survey."},
        {"layer": "water", "feature_id": f"tile:{B}", "current_manifest_id": None, "content_key": ID2,
         "lifts": LIFTABLE[1:3], "reason": "A new pond."},
    ],
    "allow_removals": [{"layer": "water", "dataset": "OpenStreetMap", "at_most": 8, "reason": "The lake drained."}],
}
OVERRIDE_DOCS = [OVERRIDES, {"overrides_version": "0.1.0"}, {"overrides_version": "0.1.0", "hold": [], "accept": []}]
OVERRIDE_BYTES = [
    json.dumps(OVERRIDES, indent=2).encode(),
    b"\xef\xbb\xbf" + json.dumps(OVERRIDES, indent=4).encode().replace(b"\n", b"\r\n"),
    json.dumps(OVERRIDES, separators=(",", ":"), sort_keys=True).encode(),
    json.dumps(OVERRIDES, ensure_ascii=False).encode(),
    b'{"overrides_version": "0.1.0"}', b"{}", b"[]", b"", b"null",
]


def test_the_overrides_corpus_is_accepted(tmp_path):
    ov = parse_overrides(OVERRIDES)
    assert len(ov.holds) == 2 and len(ov.accepts) == 2 and len(ov.allowances) == 1 and len(LIFTABLE) >= 3
    for i, data in enumerate(OVERRIDE_BYTES[:4]):
        (tmp_path / f"{i}.json").write_bytes(data)
        assert read_overrides(tmp_path / f"{i}.json").id == ov.id


def test_fuzz_overrides(tmp_path):
    """read_overrides, loads_overrides and parse_overrides raise PlanError,
    or return overrides whose id is the content id of the document in
    canonical JSON; what they return goes through the matching rules."""
    path = tmp_path / "overrides.json"

    def case(rng: Rng) -> None:
        doc = None
        if rng.chance(10):
            doc = mutate_json(rng, rng.choice(OVERRIDE_DOCS), OVERRIDE_DOCS)
            try:
                with reading(len(dumps_like(rng, doc))):
                    ov = parse_overrides(doc)
            except PlanError:
                return
        else:
            if rng.chance(75):
                data = mutate_json_bytes(rng, rng.choice(OVERRIDE_DOCS), OVERRIDE_DOCS, OVERRIDE_BYTES)
                if rng.chance(20):
                    data = rng.choice((b"\xef\xbb\xbf", b"", b"\xff\xfe")) + data.replace(b"\n", b"\r\n")
            else:
                data = mutate_bytes(rng, rng.choice(OVERRIDE_BYTES), OVERRIDE_BYTES)
            from_file = rng.chance(10)  # the file is read as `plan` reads it; the rest from memory
            if from_file:
                path.write_bytes(data)
            try:
                with reading(len(data)):
                    ov = read_overrides(path) if from_file else loads_overrides(data)
            except PlanError:
                return
            doc = json.loads(data.decode("utf-8-sig"))
        assert ov.id == content_id(cjson.dumps(doc))
        for hold in ov.holds.values():
            assert hold_matches(hold, ID1, ID2) in (None, "current_manifest_id no longer matches",
                                                    "content_key no longer matches")
        for accept in ov.accepts.values():
            assert all(code in LIFTABLE for code in accept.lifts)
            accept_matches(accept, None, ID2)
        assert all(0 <= a.at_most <= cjson.MAX_SAFE_INTEGER for a in ov.allowances.values())

    run_cases("refresh overrides", 2000, case, memory=MEMORY)


# A written plan --------------------------------------------------------------


def fixture_plan(tmp: Path) -> tuple[Path, Path]:
    """The water refresh of the fixture world (test_changes_fixture.py):
    the current store and the written plan."""
    current = pre_water_store(tmp / "before-water")
    log = Log(tmp / "log")
    log.baseline(current, BASELINE_REASON)
    out = tmp / "plan"
    write_plan(make_plan(current, FIXTURE_WORLD, log.root), out)
    return current, out


def check_plan(plan) -> None:
    """What `read_plan` promises `verify` of a plan it returns: among them,
    that every object file is a regular file within the limit, reached
    through no link."""
    assert [b.path for b in plan.bundles] == [s["path"] for s in plan.report["sets"]]
    for b in plan.bundles:
        assert content_id(read_input(b.directory / CHANGE_JSON)) == b.change_id
        assert b.change["base_world_id"] == b.base_world_id
        top = b.directory / "objects"
        for folder in (b.directory.parent, b.directory, top):
            assert not (os.path.lexists(folder) and is_link(folder.lstat())), f"{folder} is a link"
        for entry in entries_under(top):
            info = entry.lstat()
            assert stat.S_ISREG(info.st_mode) and not is_link(info), f"{entry} is not a regular file"
            assert info.st_size <= common.MAX_INPUT_BYTES
    assert plan.report_id == content_id(read_input(plan.root / REPORT_JSON))


def test_fuzz_read_plan(tmp_path, monkeypatch):
    """read_plan raises PlanFileError for a plan directory it cannot use: a
    report or a change set that is broken, missing, disagrees with the
    other, or cannot be read, an object file a read would refuse, and a
    link (a junction on Windows) at a set's objects or under it."""
    monkeypatch.setattr(common, "MAX_INPUT_BYTES", LIMIT)
    _, root = fixture_plan(tmp_path)
    files = files_under(root)
    change_rel = f"sets/001/{CHANGE_JSON}"
    objects = sorted(rel for rel in files if "/objects/" in rel)
    report, change = cjson.loads(files[REPORT_JSON]), cjson.loads(files[change_rel])
    donors = [report, change, cjson.loads(RUST_REPORT.read_bytes())]
    corpus = list(files.values())
    links = tmp_path / "links" if can_link(tmp_path) else None
    if links is not None:
        links.mkdir()
    check_plan(read_plan(root))
    ws = Workspace(root, files, links)

    def case(rng: Rng) -> None:
        mutate_plan(rng)
        try:
            with reading(ws.size()):
                plan = read_plan(root)
            check_plan(plan)
        except PlanFileError:
            pass
        finally:
            ws.restore()

    def mutate_plan(rng: Rng) -> None:
        doc = copy.deepcopy(report)
        rewrite = False
        for _ in range(1 + rng.below(3)):
            op = rng.below(10)
            if op == 0:  # the report, edited as a document
                doc = mutate_json(rng, doc, donors)
                rewrite = True
            elif op == 1:  # the report's bytes
                data = mutate_json_bytes(rng, report, donors, corpus) if rng.chance(50) else \
                    mutate_bytes(rng, files[REPORT_JSON], corpus)
                ws.write(REPORT_JSON, data)
                rewrite = False
            elif op == 2:  # the change set, edited, and often listed again under its new id
                data = encode(rng, mutate_json(rng, change, donors))
                ws.write(change_rel, data)
                if rng.chance(60) and isinstance(doc, dict) and isinstance(doc.get("sets"), list) and doc["sets"] \
                        and isinstance(doc["sets"][0], dict):
                    doc["sets"][0]["change_id"] = content_id(data)
                    rewrite = True
            elif op == 3:  # the change set's bytes
                ws.write(change_rel, mutate_bytes(rng, files[change_rel], corpus))
            elif op == 4 and isinstance(doc, dict) and isinstance(doc.get("sets"), list):  # sets listed again or not
                if doc["sets"] and rng.chance(70):
                    extra = copy.deepcopy(doc["sets"][0])
                    if isinstance(extra, dict) and rng.chance(70):
                        extra["path"] = rng.choice(("sets/002", "sets/001", "sets/0001", "sets/999"))
                    doc["sets"].append(extra)
                else:
                    doc["sets"] = []
                rewrite = True
            elif op == 5:  # a file a read must refuse
                ws.unreadable(rng, rng.choice([REPORT_JSON, change_rel, *objects]))
            elif op == 6:  # a file or the set removed
                ws.delete(rng.choice([REPORT_JSON, change_rel, "sets/001", "sets", *objects]))
            elif op == 9:  # the set, its objects or a directory under them made a link to a copy of itself
                ws.link_directory(rng.choice(["sets/001", "sets/001/objects",
                                              *sorted({o.rsplit("/", 1)[0] for o in objects})]))
            elif op == 7:  # an object file added, under any name
                name = rng.choice(("objects/ab/x.glb", "objects/x", "objects/ab/cd/ef.json", f"objects/{'0' * 2}/"
                                   f"{'0' * 64}.json", "objects/zz/" + "f" * 64 + ".glb", f"objects/ab/x{ODD_SUFFIX}"))
                rel = f"sets/001/{name}"
                if rng.chance(50):
                    ws.write(rel, rng.bytes(1 + rng.below(64)))
                else:
                    ws.unreadable(rng, rel)
            else:  # an object's bytes
                rel = rng.choice(objects)
                ws.write(rel, mutate_bytes(rng, files[rel], corpus))
        if rewrite:
            ws.write(REPORT_JSON, encode(rng, doc))

    run_cases("refresh plans", 300, case, memory=MEMORY)


# The stores ------------------------------------------------------------------


@dataclass
class StoreBase:
    """The fixture world, read once: every file, and the documents that
    reference each other (world, tile indexes, manifests, meshes)."""

    files: dict[str, bytes]
    world: dict
    indexes: dict[str, dict]
    manifests: dict[str, dict]
    meshes: dict[str, bytes]
    referenced: list[str]
    """world.json and every object the world references."""


def store_base() -> StoreBase:
    files = files_under(FIXTURE_WORLD)
    world = cjson.loads(files["world.json"])
    indexes, manifests, meshes, referenced = {}, {}, {}, ["world.json"]
    for wt in world["tiles"]:
        indexes[wt["tile"]] = cjson.loads(files[object_rel(wt["tile_id"], "json")])
        referenced.append(object_rel(wt["tile_id"], "json"))
        for a in indexes[wt["tile"]]["assets"]:
            manifests[a["manifest_id"]] = cjson.loads(files[object_rel(a["manifest_id"], "json")])
            meshes[a["content_id"]] = files[object_rel(a["content_id"], "glb")]
            referenced += [object_rel(a["manifest_id"], "json"), object_rel(a["content_id"], "glb")]
    return StoreBase(files, world, indexes, manifests, meshes, referenced)


def _sync(entry: dict, m: object) -> None:
    """Copies into a tile-index entry what the manifest `m` says, so that
    the entry agrees with an edited manifest."""
    if not isinstance(m, dict):
        return
    provenance, physics = m.get("provenance"), m.get("physics")
    for name, value in (("layer", m.get("layer")), ("feature_id", m.get("feature_id")),
                        ("content_id", m.get("content_id")), ("bounds_mm", m.get("bounds_mm")),
                        ("license", provenance.get("license") if isinstance(provenance, dict) else None),
                        ("collider", physics.get("collider") if isinstance(physics, dict) else None)):
        if value is not None:
            entry[name] = copy.deepcopy(value)


def mutate_store(rng: Rng, ws: Workspace, base: StoreBase, donors: list) -> list[tuple[str, str]]:
    """One to three edits of the store in `ws`, each kept consistent up to
    world.json (a new object's id goes into the document that references
    it) unless the edit is to break that. Returns the keys of the assets
    whose manifest or mesh changed."""
    world = copy.deepcopy(base.world)
    indexes = copy.deepcopy(base.indexes)
    tiles = sorted(indexes)
    dirty: set[str] = set()
    keys = []
    world_bytes = None
    for _ in range(1 + rng.below(3)):
        op = rng.below(10)
        tile = rng.choice(tiles)
        entries = indexes[tile].get("assets") if isinstance(indexes[tile], dict) else None
        entry = rng.choice(entries) if isinstance(entries, list) and entries else None
        entry = entry if isinstance(entry, dict) else None
        mid = entry.get("manifest_id") if entry is not None else None
        manifest = base.manifests.get(mid) if isinstance(mid, str) else None
        key = (entry.get("layer"), entry.get("feature_id")) if entry is not None else None
        if key is not None and not all(isinstance(k, str) for k in key):
            key = None
        if op == 0:  # world.json, edited as a document or as bytes
            world_bytes = (mutate_json_bytes(rng, base.world, donors, [base.files["world.json"]]) if rng.chance(80)
                           else mutate_bytes(rng, base.files["world.json"], [base.files["world.json"]]))
        elif op == 1:  # a tile index
            indexes[tile] = mutate_json(rng, indexes[tile], donors)
            dirty.add(tile)
        elif op == 2 and manifest is not None:  # a manifest, in the tile index under its new id
            edited = mutate_json(rng, manifest, donors)
            data = encode(rng, edited)
            ws.write(object_rel(content_id(data), "json"), data)
            keys += [key] if key is not None else []
            entry["manifest_id"] = content_id(data)
            if rng.chance(60):
                _sync(entry, edited)
            dirty.add(tile)
        elif op == 3 and manifest is not None:  # a mesh, under its new id in the manifest and the index
            mesh = base.meshes[manifest["content_id"]]
            if rng.chance(75):
                mesh = mutate_bytes(rng, mesh, list(base.meshes.values())[:6])
            else:  # another asset's mesh: valid, but not where the manifest says
                mesh = rng.choice(list(base.meshes.values()))
            ext = "glb" if rng.chance(90) else rng.choice(("json", "a", "bin"))
            ws.write(object_rel(content_id(mesh), ext), mesh)
            edited = dict(manifest, content_id=content_id(mesh))
            if rng.chance(80):
                edited["size_bytes"] = len(mesh)
            data = cjson.dumps(edited)
            ws.write(object_rel(content_id(data), "json"), data)
            keys += [key] if key is not None else []
            entry["manifest_id"] = content_id(data)
            entry["content_id"] = content_id(mesh)
            dirty.add(tile)
        elif op == 4:  # a file removed
            ws.delete(rng.choice(base.referenced))
        elif op == 5:  # a file a read must refuse
            ws.unreadable(rng, rng.choice(base.referenced))
        elif op == 6 and manifest is not None:  # another file of the same content, under another extension
            h = hex_of(manifest["content_id"])
            data = base.meshes[manifest["content_id"]]
            data = data if rng.chance(50) else mutate_bytes(rng, data, [data])
            rel = f"objects/{h[:2]}/{h}.{rng.choice(('a', 'glb0', 'json', 'zz', ODD_SUFFIX[1:]))}"
            if rng.chance(80):
                ws.write(rel, data)
            else:
                ws.unreadable(rng, rel)
            keys += [key] if key is not None else []
        elif op == 7:  # an object whose bytes no longer hash to its name
            rel = rng.choice(base.referenced[1:])
            ws.write(rel, mutate_bytes(rng, base.files[rel], [base.files[rel]]))
        elif op == 8 and entry is not None:  # an asset listed twice
            entries.append(copy.deepcopy(entry))
            dirty.add(tile)
        elif op == 9:  # an object that nothing references
            data = rng.choice((b"{}", rng.bytes(1 + rng.below(32)), base.files["world.json"]))
            ws.write(object_rel(content_id(data), rng.choice(("json", "glb"))), data)
    for tile in sorted(dirty):
        data = encode(rng, indexes[tile])
        ws.write(object_rel(content_id(data), "json"), data)
        for wt in world["tiles"]:
            if wt["tile"] == tile:
                wt["tile_id"] = content_id(data)
    if world_bytes is None and dirty:
        world_bytes = cjson.dumps(world)
    if world_bytes is not None:
        ws.write("world.json", world_bytes)
    return keys


def use_store(rng: Rng, view, keys: list[tuple[str, str]]) -> None:
    """What the planner reads from a store it accepted: content and meshes
    (for bundles, H8 and H14), objects it may lack, and lineage manifests."""
    assets = [view.assets[k] for k in keys if k in view.assets]
    if view.assets and rng.chance(30):
        assets.append(rng.choice(list(view.assets.values())))
    sx, _, sz = view.spawn
    for asset in assets[:3]:
        ext, data = view.content(asset)
        assert content_id(data) == asset.content_id
        assert asset.content_key.startswith("blake3:")
        if ext == "glb":
            mesh = view.mesh(asset)
            mesh.bounds_mm()
            ground_under(mesh, view.origins[asset.tile], sx, sz)
            glb.boundary_vertices(mesh)
    for asset in rng.choice((assets, list(view.assets.values())))[:4]:
        for cid in (asset.manifest_id, asset.content_id, content_id(asset.content_id.encode())):
            view.has_object(cid, rng.choice(("json", "glb")))
            view.object_names(cid)
            assert read_manifest(view, cid) is None or cid == asset.manifest_id


def test_the_fuzzed_store_is_read(tmp_path):
    base = store_base()
    ws = Workspace(tmp_path / "store", base.files, None)
    view = read_store(ws.root, "new")
    assert len(view.assets) == len(base.manifests) == 45
    use_store(Rng(1), view, list(view.assets))


def test_fuzz_store_readers(tmp_path, monkeypatch):
    """read_store, and the content, meshes, objects and manifests a store
    view reads on request, raise PlanError for a store they cannot use:
    broken or inconsistent documents, objects missing or not named after
    their content, and files a read must refuse."""
    monkeypatch.setattr(common, "MAX_INPUT_BYTES", LIMIT)
    base = store_base()
    links = tmp_path / "links" if can_link(tmp_path) else None
    if links is not None:
        links.mkdir()
    ws = Workspace(tmp_path / "store", base.files, links)
    donors = [base.world, *base.indexes.values(), *list(base.manifests.values())[:6]]
    checked: dict = {}  # shared, as the planner shares it between its two stores

    def case(rng: Rng) -> None:
        keys = mutate_store(rng, ws, base, donors)
        try:
            with reading(ws.size()):
                view = read_store(ws.root, rng.choice(("current", "new")), checked)
                use_store(rng, view, keys)
        except PlanError:
            pass
        finally:
            ws.restore()

    run_cases("refresh stores", 120, case, memory=MEMORY)


# The gate's report, as verify checks it ---------------------------------------


def small_plan(tmp: Path) -> tuple[Path, Path]:
    """A plan of one set (a shell added) on a world of two assets, so that a
    verify run is quick: the current store and the written plan."""
    world = World.of(terrain(A), swiss_shell(1))
    sc = Scenario.start(tmp, world)
    added = swiss_shell(3, lo=(60_000, GROUND_MM, 60_000), hi=(70_000, GROUND_MM + 6_000, 70_000))
    out = tmp / "plan"
    write_plan(sc.plan(world.copy().put(added)), out)
    return sc.current, out


def passing_report(current: Path, bundle: Path) -> dict:
    """The report of a passing refresh set, as `op_replay check-change`
    writes it (test_changes_gate.py's Fake)."""
    world_id, world = Store(current).read_world()
    change_bytes = (bundle / CHANGE_JSON).read_bytes()
    change = parse(change_bytes)
    candidate = apply(world, change, Objects(Store(bundle), Store(current)))
    what = [f"{c['tile']} {c['layer']} {c['feature_id']}" for c in change["changes"]]
    names = ["format", "lineage", "layer policy", "licence", "provenance", "privacy", "budgets", "placement",
             "walkability", "simulation"]
    outcomes = {"G3": "review", "G4": "not_applicable", "G5": "not_applicable", "G6": "not_applicable",
                "G9": "not_applicable", "G10": "not_applicable"}
    checks = [{"id": f"G{n}", "name": name, "outcome": outcomes.get(f"G{n}", "pass"),
               "details": [f"{w}: baseline refresh by {INGEST_AUTHOR}; a maintainer reviews it" for w in what]
               if n == 3 else []} for n, name in enumerate(names, 1)]
    return {"format": "streetwide.check", "contract_version": "0.1.0", "gate_version": "0.1.4",
            "change_id": content_id(change_bytes), "world_id": world_id, "candidate_world_id": candidate.world_id,
            "passed": True, "checks": checks}


class FakeTools:
    """`op_replay` and `op_world` for verify: `check-change` writes `report`
    and exits with `code`; `validate` and `compare-worlds` pass."""

    def __init__(self) -> None:
        self.report = b""
        self.code = 0

    def __call__(self, command: list[str]) -> subprocess.CompletedProcess:
        _, sub, *args = command

        def option(name: str) -> Path:
            return Path(args[args.index(name) + 1])

        def world_id(store: str) -> str:
            return content_id((Path(store) / "world.json").read_bytes())

        if sub == "check-change":
            option("--report").write_bytes(self.report)
            return subprocess.CompletedProcess(command, self.code, "fake gate\n", "")
        if sub == "validate":
            return subprocess.CompletedProcess(command, 0, f"world {world_id(args[0])}: 2 tile(s), 0 error(s)\n", "")
        if sub == "compare-worlds":
            report = {"report": "streetwide world comparison", "report_version": "0.1.0",
                      "before_world_id": world_id(args[0]), "after_world_id": world_id(args[1]), "findings": 0}
            option("--report").write_bytes(cjson.dumps(report))
            return subprocess.CompletedProcess(command, 0, "0 findings, for review\n", "")
        raise AssertionError(f"verify ran {sub}")


def run_verify(current: Path, plan: Path, scratch: Path, summary: Path, tools: FakeTools, *,
               compare: bool = True):
    options = Options(current=current, plan=plan, scratch=scratch, op_replay=("fake-op-replay",),
                      op_world=("fake-op-world",), report=summary, compare=compare)
    try:
        return verify(options, runner=tools, say=lambda line: None)
    finally:
        remove(scratch)


def test_the_fuzzed_gate_report_passes(tmp_path):
    current, plan = small_plan(tmp_path)
    tools = FakeTools()
    tools.report = cjson.dumps(passing_report(current, plan / "sets" / "001"))
    result = run_verify(current, plan, tmp_path / "scratch", tmp_path / "summary.json", tools)
    assert result.code == 0, result.problems


def test_fuzz_gate_reports(tmp_path):
    """verify, given any report from the gate, raises VerifyError (exit code
    2) or returns a result that passes or fails, with its summary written
    in canonical JSON."""
    current, plan = small_plan(tmp_path)
    good = passing_report(current, plan / "sets" / "001")
    failed = copy.deepcopy(good)
    failed["passed"] = False
    failed["checks"][8].update(outcome="fail", details=["17/68646/45903: 3 probe points blocked (fake)"])
    reports = [good, failed, cjson.loads(RUST_REPORT.read_bytes())]
    corpus = [cjson.dumps(r) for r in reports]
    tools = FakeTools()
    summary = tmp_path / "summary.json"

    def case(rng: Rng) -> None:
        if rng.chance(85):
            data = mutate_json_bytes(rng, rng.choice(reports), reports, corpus)
        else:
            data = mutate_bytes(rng, rng.choice(corpus), corpus) if rng.chance(70) else rng.choice(corpus)
        tools.report = data
        tools.code = rng.choice((0, 1)) if rng.chance(90) else rng.choice((2, 101))
        remove(summary)
        try:
            with reading(len(data)):
                result = run_verify(current, plan, tmp_path / "scratch", summary, tools, compare=rng.chance(50))
        except VerifyError:
            return
        assert result.code in (0, 1)
        written = cjson.loads(read_input(summary))
        assert written["passed"] is (result.code == 0) and written["problems"] == result.problems

    run_cases("verify gate reports", 120, case, memory=MEMORY)


# Regressions: inputs that crashed, hung or were read where they should not be -


@pytest.mark.parametrize(
    "data, message",
    [
        (b"[" * 100_000, "not valid JSON: arrays and objects are nested too deeply"),  # RecursionError
        (b'{"overrides_version": "0.1.0", "allow_removals": [{"at_most": ' + b"1" * 5000 + b"}]}",
         "not valid JSON: a number has more than 4300 digits"),  # ValueError (int's digit limit)
        # A lone surrogate, which JSON's escapes allow: UnicodeEncodeError
        # when field names or keys were sorted by their UTF-8 bytes.
        (b'{"\\ud800": 1, "overrides_version": "0.1.0"}', 'unknown field "\\ud800"'),
        (b'{"overrides_version": "0.1.0", "hold": [{"\\udc00": 1, "a": 2}]}', 'hold[0] has an unknown field "a"'),
        (b'{"overrides_version": "0.1.0", "hold": [{"layer": "\\ud800", "feature_id": "osm:way:1", "reason": "x"}, '
         b'{"layer": "shell", "feature_id": "osm:way:1", "reason": "x"}]}',
         "not representable as canonical JSON: string contains a lone surrogate"),
        (b'{"overrides_version": "0.1.0", "allow_removals": [{"layer": "water", "dataset": "\\ud800", "at_most": 1, '
         b'"reason": "x"}, {"layer": "water", "dataset": "b", "at_most": 1, "reason": "x"}]}',
         "not representable as canonical JSON: string contains a lone surrogate"),
    ],
    ids=["deep", "digits", "surrogate-field", "surrogate-entry-field", "surrogate-layer", "surrogate-dataset"],
)
def test_malformed_overrides_are_plan_errors(tmp_path, data, message):
    # Each of these but the last raised another exception (found by review).
    with pytest.raises(PlanError) as e:
        loads_overrides(data)
    assert str(e.value) == f"overrides: {message}"
    (tmp_path / "overrides.json").write_bytes(data)
    with pytest.raises(PlanError, match="^overrides: "):
        read_overrides(tmp_path / "overrides.json")


@pytest.mark.parametrize("world, why", [
    (b"[]", "expected a JSON object, not list"),  # TypeError
    (b"{}", "its format is not streetwide.world"),  # KeyError
    (b'{"format":"streetwide.tile"}', "its format is not streetwide.world"),  # KeyError
])
def test_verify_refuses_a_world_that_is_not_a_world(tmp_path, world, why):
    # A plan that names the id of a world.json that is canonical JSON but no
    # world manifest: verify took its name and raised (found by review).
    current, plan = small_plan(tmp_path)
    report = cjson.loads((plan / REPORT_JSON).read_bytes())
    report["current_world_id"] = content_id(world)
    (plan / REPORT_JSON).write_bytes(cjson.dumps(report))
    (current / "world.json").write_bytes(world)
    with pytest.raises(VerifyError) as e:
        run_verify(current, plan, tmp_path / "scratch", tmp_path / "summary.json", FakeTools())
    assert str(e.value) == f"{current}: world.json is not a streetwide.world document: {why}"


def test_a_gate_report_of_another_kind_says_so(tmp_path):
    # A valid document of another format gave an empty reason.
    current, plan = small_plan(tmp_path)
    tools = FakeTools()
    tools.report = (current / "world.json").read_bytes()
    with pytest.raises(VerifyError) as e:
        run_verify(current, plan, tmp_path / "scratch", tmp_path / "summary.json", tools)
    assert str(e.value) == "gate/001.json: the gate's report is not a streetwide.check document: its format is not " \
                           "streetwide.check"


def test_a_gate_report_over_its_limit_is_refused_unparsed(tmp_path):
    # RFC 0001 section 7: a check report is at most 16 MiB, checked before
    # it is parsed, as op_world checks it. One-key objects take a parser the
    # most memory per byte: about 1.4 GB for this one in serde_json.
    data = b"[" + b'{"a":0},' * (CHECK_REPORT.max_bytes // 8)
    data = data[: CHECK_REPORT.max_bytes].ljust(CHECK_REPORT.max_bytes, b" ") + b"]"
    assert len(data) == CHECK_REPORT.max_bytes + 1
    message = f"gate/001.json: the gate's report is {CHECK_REPORT.too_large()}"

    def case(rng: Rng) -> None:
        with reading(len(data)):
            with pytest.raises(VerifyError) as e:
                read_gate_report(data, "gate/001.json")
        assert str(e.value) == message

    run_cases("verify gate report over its limit", 1, case, memory=(1, 1 << 20))
    current, plan = small_plan(tmp_path)
    tools = FakeTools()
    tools.report = data
    with pytest.raises(VerifyError) as e:
        run_verify(current, plan, tmp_path / "scratch", tmp_path / "summary.json", tools)
    assert str(e.value) == message


def test_a_store_has_an_object_only_as_a_regular_file(tmp_path):
    # As op_world's Store::has_object: a directory or a link where an
    # object should be does not count, and reading it is an error.
    base = store_base()
    ws = Workspace(tmp_path / "store", base.files, None)
    view = read_store(ws.root, "new")
    asset = next(a for a in view.assets.values() if a.layer == "water")
    mesh = ws.path(object_rel(asset.content_id, "glb"))
    remove(mesh)
    mesh.mkdir()
    assert not view.has_object(asset.content_id, "glb")
    assert view.object_names(asset.content_id) == []
    assert view.object_names(asset.content_id, regular=False) == [mesh.name]
    with pytest.raises(PlanError, match=f"content {asset.content_id}: not a regular file$"):
        view.content(asset)
    manifest = ws.path(object_rel(asset.manifest_id, "json"))
    remove(manifest)
    manifest.mkdir()
    assert not view.has_object(asset.manifest_id, "json")
    assert read_manifest(view, asset.manifest_id) is None, "the cache of checked documents does not skip the read"
    with pytest.raises(PlanError, match=f"manifest: object {asset.manifest_id}: not a regular file$"):
        read_store(ws.root, "new")


def test_a_link_in_the_current_store_does_not_keep_an_object_out_of_the_bundle(tmp_path):
    # The current store has the added shell's mesh, but only as a link: the
    # gate would not read it there (a link is not a regular file), so the
    # bundle must ship it. has_object and object_names followed links.
    if not can_link(tmp_path):
        pytest.skip("cannot create symbolic links here")
    world = World.of(terrain(A), swiss_shell(1))
    sc = Scenario.start(tmp_path, world)
    added = swiss_shell(3, lo=(60_000, GROUND_MM, 60_000), hi=(70_000, GROUND_MM + 6_000, 70_000))
    (tmp_path / "elsewhere.glb").write_bytes(added.mesh.data)
    rel = object_rel(added.content_id, "glb")
    (sc.current / rel).parent.mkdir(parents=True, exist_ok=True)
    os.symlink(tmp_path / "elsewhere.glb", sc.current / rel)
    plan = sc.plan(world.copy().put(added))
    assert f"sets/001/{rel}" in plan.files


def test_the_limits_are_the_rust_tools(tmp_path):
    from tools.ingest.changes import verify as verify_module
    from tools.worldfmt.store import MAX_FILE_BYTES

    assert common.MAX_INPUT_BYTES == MAX_FILE_BYTES == 64 << 20
    # op_replay::format::MAX_REPLAY_BYTES: a 64 KiB header, one day of 8-byte
    # input records at 60 Hz and a 40-byte checkpoint at every tick.
    assert verify_module.MAX_REPLAY_BYTES == 248_897_556


def test_a_file_name_that_is_not_utf8_is_shown_escaped(tmp_path):
    # A crafted store or plan can hold such a name (a lone surrogate in
    # Python): sorting the names of an object by their UTF-8 bytes raised
    # UnicodeEncodeError, and verify's summary in canonical JSON could not
    # hold the name (found by review).
    base = store_base()
    ws = Workspace(tmp_path / "store", base.files, None)
    view = read_store(ws.root, "new")
    asset = next(a for a in view.assets.values() if a.layer == "water")
    odd = ws.path(object_rel(asset.content_id, "glb")).with_suffix(ODD_SUFFIX)
    try:
        odd.write_bytes(base.meshes[asset.content_id])
    except OSError:
        pytest.skip("this file system refuses names that are not UTF-8")
    assert view.object_names(asset.content_id) == [f"{hex_of(asset.content_id)}.glb", odd.name]
    assert view.content(asset)[0] == "glb"

    current, plan = small_plan(tmp_path / "verify")
    (glb_file,) = (plan / "sets" / "001" / "objects").rglob("*.glb")
    glb_file.with_suffix(ODD_SUFFIX).write_bytes(b"x")
    tools = FakeTools()
    tools.report = cjson.dumps(passing_report(current, plan / "sets" / "001"))
    result = run_verify(current, plan, tmp_path / "scratch", tmp_path / "summary.json", tools)
    rel = glb_file.relative_to(plan / "sets" / "001").with_suffix("").as_posix() + ".g\\udcff"
    assert result.problems == [f"sets/001: {rel} is not named objects/<aa>/<hex>.<ext>"]
    assert cjson.loads(read_input(tmp_path / "summary.json"))["problems"] == result.problems


@pytest.mark.parametrize("where", ["set", "objects", "under objects"])
def test_a_link_at_or_under_a_sets_objects_is_refused(tmp_path, where):
    # read_plan tested `objects.is_dir()`, which follows a link, then listed
    # `objects.rglob("*")`, which starts at the link's target and, on
    # Windows, also searches junctions (which need no privilege to make):
    # a plan whose sets/001/objects named the repository's target/ made it
    # list 33,145 entries outside the plan, and verify's own walk did the
    # same. Links and junctions are now refused, and never searched. Found
    # by review.
    _, root = fixture_plan(tmp_path)
    rel = {"set": "sets/001", "objects": "sets/001/objects"}.get(where)
    if rel is None:
        rel = "sets/001/objects/" + next((root / "sets" / "001" / "objects").iterdir()).name
    linked = root.joinpath(*rel.split("/"))
    outside = tmp_path / "outside"
    os.replace(linked, outside)
    if not link_directory(outside, linked):
        pytest.skip("this system cannot make a link to a directory")
    with pytest.raises(PlanFileError) as e:
        read_plan(root)
    if where == "under objects":
        assert str(e.value) == f"{rel}: not a regular file"
        assert linked in entries_under(root / "sets" / "001" / "objects"), "a link is an entry, not searched"
    else:
        assert str(e.value) == (f"{rel} is not a directory; a plan holds its sets and their objects in directories, "
                                "not in links to them")
        assert entries_under(linked) == [], "a link at the top is not searched"
    remove(linked)
    os.replace(outside, linked)
    check_plan(read_plan(root))
