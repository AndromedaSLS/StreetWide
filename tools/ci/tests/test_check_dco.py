"""Tests for tools/ci/check_dco.py using throwaway git repositories."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from tools.ci import check_dco

ALICE = ("Alice Example", "alice@example.org")
BOB = ("Bob Example", "bob@example.org")


@pytest.fixture
def repo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    # Isolate from the developer's git config (signing, hooks, templates).
    empty = tmp_path / "empty.gitconfig"
    empty.write_text("", encoding="utf-8")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(empty))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    path = tmp_path / "repo"
    path.mkdir()
    run(path, "init", "-q", "-b", "main")
    return path


def run(repo: Path, *args: str, author: tuple[str, str] = ALICE) -> str:
    env = dict(os.environ)
    env.update(
        GIT_AUTHOR_NAME=author[0],
        GIT_AUTHOR_EMAIL=author[1],
        GIT_COMMITTER_NAME=author[0],
        GIT_COMMITTER_EMAIL=author[1],
    )
    out = subprocess.run(
        ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True, env=env
    )
    return out.stdout.strip()


def commit(repo: Path, message: str, author: tuple[str, str] = ALICE) -> str:
    run(repo, "commit", "-q", "--allow-empty", "-m", message, author=author)
    return run(repo, "rev-parse", "HEAD")


def signoff(person: tuple[str, str]) -> str:
    return f"\n\nSigned-off-by: {person[0]} <{person[1]}>"


def check(repo: Path, base: str, head: str, baseline: str = "") -> int:
    return check_dco.main([base, head, "--repo", str(repo), "--baseline", baseline])


def test_signed_commits_pass(repo: Path) -> None:
    base = commit(repo, "root" + signoff(ALICE))
    head = commit(repo, "feature" + signoff(ALICE))
    assert check(repo, base, head) == 0


def test_missing_signoff_fails(repo: Path, capsys: pytest.CaptureFixture[str]) -> None:
    base = commit(repo, "root" + signoff(ALICE))
    commit(repo, "good" + signoff(ALICE))
    head = commit(repo, "forgot the sign-off")
    assert check(repo, base, head) == 1
    assert "forgot the sign-off" in capsys.readouterr().err


def test_signoff_must_match_author_email(repo: Path) -> None:
    base = commit(repo, "root" + signoff(ALICE))
    head = commit(repo, "signed by someone else" + signoff(ALICE), author=BOB)
    assert check(repo, base, head) == 1


def test_signoff_email_is_case_insensitive(repo: Path) -> None:
    base = commit(repo, "root" + signoff(ALICE))
    head = commit(repo, "shouting\n\nsigned-off-by: Alice Example <ALICE@example.org>")
    assert check(repo, base, head) == 0


def test_empty_base_checks_whole_history_after_baseline(repo: Path) -> None:
    baseline = commit(repo, "pre-DCO history, unsigned")
    head = commit(repo, "after adoption" + signoff(ALICE))
    assert check(repo, "", head, baseline=baseline) == 0
    assert check(repo, "0" * 40, head, baseline=baseline) == 0
    # Without the baseline exemption the unsigned root commit is caught.
    assert check(repo, "", head) == 1


def test_merge_commits_are_skipped(repo: Path) -> None:
    base = commit(repo, "root" + signoff(ALICE))
    run(repo, "checkout", "-q", "-b", "topic")
    commit(repo, "topic work" + signoff(ALICE))
    run(repo, "checkout", "-q", "main")
    commit(repo, "main work" + signoff(ALICE))
    run(repo, "merge", "-q", "--no-ff", "-m", "Merge topic (unsigned merge commit)", "topic")
    head = run(repo, "rev-parse", "HEAD")
    assert check(repo, base, head) == 0
