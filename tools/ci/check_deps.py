"""Check that workspace dependencies point inward (ADR-0002).

Reads `cargo metadata` and tools/ci/deps-policy.toml and fails if

* a workspace crate is missing from the policy,
* a crate depends (normal, build or dev) on a workspace crate it is not
  allowed to depend on (this also forbids depending on op_client), or
* a non-render crate has a banned package (Bevy, wgpu, winit, ...) anywhere in
  its dependency closure.

Usage:
    python tools/ci/check_deps.py [--metadata FILE] [--policy FILE]

Without --metadata it runs `cargo metadata --format-version 1 --locked`.
Standard library only, so CI needs no extra packages.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import subprocess
import sys
import tomllib
from collections import deque
from pathlib import Path

DEFAULT_POLICY = Path(__file__).with_name("deps-policy.toml")


def load_metadata(path: Path | None) -> dict:
    if path is not None:
        return json.loads(path.read_text(encoding="utf-8"))
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(out.stdout)


def check(metadata: dict, policy: dict) -> list[str]:
    """Return a list of human-readable violations (empty means OK)."""
    errors: list[str] = []
    packages = {p["id"]: p for p in metadata["packages"]}
    members = sorted(metadata["workspace_members"], key=lambda i: packages[i]["name"])
    member_names = {packages[i]["name"]: i for i in members}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    crate_policy: dict = policy.get("crates", {})
    banned: list[str] = policy.get("banned", {}).get("packages", [])

    for name in sorted(crate_policy):
        if name not in member_names:
            errors.append(f"policy lists `{name}`, which is not a workspace member")

    for member_id in members:
        name = packages[member_id]["name"]
        rules = crate_policy.get(name)
        if rules is None:
            errors.append(
                f"`{name}` is not listed in deps-policy.toml; "
                "new crates need an explicit dependency rule (and an ADR update)"
            )
            continue
        allowed = set(rules.get("allow", []))

        # Direct dependencies of every kind (normal, build, dev).
        for dep in sorted(nodes[member_id]["deps"], key=lambda d: d["name"]):
            dep_name = packages[dep["pkg"]]["name"]
            if dep["pkg"] in members and dep_name not in allowed:
                kinds = sorted({k["kind"] or "normal" for k in dep["dep_kinds"]})
                errors.append(
                    f"`{name}` depends on `{dep_name}` ({', '.join(kinds)}), "
                    f"which deps-policy.toml does not allow"
                )

        if rules.get("render", False):
            continue
        for pkg_name, path in sorted(closure(member_id, nodes, packages).items()):
            if any(fnmatch.fnmatchcase(pkg_name, pat) for pat in banned):
                errors.append(
                    f"`{name}` must not depend on rendering, but pulls in "
                    f"`{pkg_name}` via {' -> '.join(path)}"
                )
    return errors


def closure(root_id: str, nodes: dict, packages: dict) -> dict[str, list[str]]:
    """All packages reachable from root, mapped to one shortest path of names.

    The root's dev-dependencies count (they are compiled into its tests); for
    every other package only normal and build dependencies are followed.
    """
    seen: dict[str, list[str]] = {}
    queue: deque[tuple[str, list[str]]] = deque([(root_id, [packages[root_id]["name"]])])
    visited = {root_id}
    while queue:
        pkg_id, path = queue.popleft()
        for dep in nodes[pkg_id]["deps"]:
            kinds = {k["kind"] for k in dep["dep_kinds"]}
            if pkg_id != root_id and kinds <= {"dev"}:
                continue
            dep_id = dep["pkg"]
            if dep_id in visited:
                continue
            visited.add(dep_id)
            dep_path = path + [packages[dep_id]["name"]]
            seen.setdefault(packages[dep_id]["name"], dep_path)
            queue.append((dep_id, dep_path))
    return seen


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--metadata", type=Path, help="read cargo metadata JSON from a file")
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    args = parser.parse_args(argv)

    policy = tomllib.loads(args.policy.read_text(encoding="utf-8"))
    errors = check(load_metadata(args.metadata), policy)
    if errors:
        print("Dependency-direction check FAILED (see ADR-0002):", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 1
    print("Dependency-direction check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
