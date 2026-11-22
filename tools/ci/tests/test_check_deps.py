"""Tests for tools/ci/check_deps.py using synthetic `cargo metadata` output."""

from __future__ import annotations

import tomllib
from pathlib import Path

from tools.ci import check_deps

POLICY = tomllib.loads(
    (Path(__file__).parents[1] / "deps-policy.toml").read_text(encoding="utf-8")
)

MEMBERS = ["op_kernel", "op_sim", "op_world", "op_stream", "op_replay", "op_client"]


def pid(name: str) -> str:
    return f"path+file:///ws/{name}#0.1.0"


class Graph:
    """Builds a minimal cargo-metadata document."""

    def __init__(self) -> None:
        self.packages: dict[str, dict] = {}
        self.deps: dict[str, list[dict]] = {}
        for m in MEMBERS:
            self.add(m)

    def add(self, name: str) -> None:
        self.packages.setdefault(pid(name), {"id": pid(name), "name": name})
        self.deps.setdefault(pid(name), [])

    def dep(self, src: str, dst: str, kind: str | None = None) -> Graph:
        self.add(src)
        self.add(dst)
        self.deps[pid(src)].append(
            {"name": dst, "pkg": pid(dst), "dep_kinds": [{"kind": kind, "target": None}]}
        )
        return self

    def metadata(self) -> dict:
        return {
            "packages": list(self.packages.values()),
            "workspace_members": [pid(m) for m in MEMBERS],
            "resolve": {"nodes": [{"id": i, "deps": d} for i, d in self.deps.items()]},
        }


def planned_graph() -> Graph:
    g = Graph()
    g.dep("op_sim", "op_kernel").dep("op_sim", "rapier3d")
    g.dep("op_stream", "op_kernel").dep("op_stream", "op_world")
    for d in ["op_kernel", "op_sim", "op_world", "op_stream"]:
        g.dep("op_replay", d)
    for d in ["op_kernel", "op_sim", "op_world", "op_stream", "op_replay"]:
        g.dep("op_client", d)
    g.dep("op_client", "bevy").dep("bevy", "bevy_render").dep("bevy_render", "wgpu")
    return g


def test_planned_graph_passes() -> None:
    assert check_deps.check(planned_graph().metadata(), POLICY) == []


def test_sim_must_not_depend_on_world() -> None:
    errors = check_deps.check(planned_graph().dep("op_sim", "op_world").metadata(), POLICY)
    assert len(errors) == 1 and "`op_sim` depends on `op_world`" in errors[0]


def test_dev_dependencies_count_for_direction() -> None:
    g = planned_graph().dep("op_kernel", "op_sim", kind="dev")
    errors = check_deps.check(g.metadata(), POLICY)
    assert any("`op_kernel` depends on `op_sim` (dev)" in e for e in errors)


def test_nothing_may_depend_on_client() -> None:
    errors = check_deps.check(planned_graph().dep("op_replay", "op_client").metadata(), POLICY)
    assert any("`op_replay` depends on `op_client`" in e for e in errors)


def test_transitive_rendering_dependency_is_caught() -> None:
    g = planned_graph().dep("op_world", "fancy_gltf").dep("fancy_gltf", "bevy_ecs")
    errors = check_deps.check(g.metadata(), POLICY)
    # op_world pulls it in, and so does everything that depends on op_world.
    assert any("`op_world` must not depend on rendering" in e and "bevy_ecs" in e for e in errors)
    assert any(e.startswith("`op_stream` must not depend on rendering") for e in errors)
    assert "op_world -> fancy_gltf -> bevy_ecs" in "\n".join(errors)


def test_root_dev_dependency_on_rendering_is_caught() -> None:
    g = planned_graph().dep("op_sim", "winit", kind="dev")
    errors = check_deps.check(g.metadata(), POLICY)
    assert any("`op_sim` must not depend on rendering" in e and "winit" in e for e in errors)


def test_dev_dependencies_of_third_party_crates_are_ignored() -> None:
    g = planned_graph().dep("rapier3d", "bevy", kind="dev")
    assert check_deps.check(g.metadata(), POLICY) == []


def test_unknown_workspace_member_is_rejected() -> None:
    meta = planned_graph().metadata()
    meta["packages"].append({"id": pid("op_extra"), "name": "op_extra"})
    meta["workspace_members"].append(pid("op_extra"))
    meta["resolve"]["nodes"].append({"id": pid("op_extra"), "deps": []})
    errors = check_deps.check(meta, POLICY)
    assert any("`op_extra` is not listed" in e for e in errors)
