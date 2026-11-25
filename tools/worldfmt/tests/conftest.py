"""Shared by the tools/worldfmt tests.

`op_world` runs the Rust tool, and `check_change` runs its gate
(`op_world check-change`) on a bundle and returns the report. Tests that
use them need cargo, so they run only with STREETWIDE_GATE=1, like the
refresh gate tests (tools/ingest/tests/test_changes_gate.py).
STREETWIDE_OP_WORLD sets the command prefix; the default is
`cargo run -q --locked -p op_world --`, run from the repository root.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
DEFAULT_OP_WORLD = "cargo run -q --locked -p op_world --"


@pytest.fixture
def op_world() -> Callable[..., subprocess.CompletedProcess]:
    if os.environ.get("STREETWIDE_GATE") != "1":
        pytest.skip("runs op_world; set STREETWIDE_GATE=1 (needs cargo)")
    prefix = os.environ.get("STREETWIDE_OP_WORLD", DEFAULT_OP_WORLD).split()

    def run(*args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [*prefix, *args],
            cwd=REPO,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=900,
        )

    return run


@pytest.fixture
def check_change(op_world, tmp_path_factory: pytest.TempPathFactory) -> Callable[[Path, Path], dict]:
    reports = tmp_path_factory.mktemp("gate-reports")
    runs = 0

    def run(world_dir: Path, bundle_dir: Path) -> dict:
        nonlocal runs
        runs += 1
        report = reports / f"report-{runs}.json"
        done = op_world("check-change", str(world_dir), str(bundle_dir), "--report", str(report))
        if not report.is_file():
            pytest.fail(f"op_world check-change wrote no report (exit {done.returncode}): {done.stderr.strip()}")
        return json.loads(report.read_bytes())

    return run
