"""Check that every commit in a range is signed off (Developer Certificate of Origin).

A commit passes when its message contains a `Signed-off-by: Name <email>`
line whose email matches the commit author's email (case-insensitive), which
is what `git commit -s` produces. Merge commits are skipped. Commits reachable
from the baseline (history from before the project adopted the DCO) are never
checked. See CONTRIBUTING.md.

Usage:
    python tools/ci/check_dco.py BASE HEAD
    python tools/ci/check_dco.py BASE HEAD --baseline <sha>

BASE may be empty or all zeros (a newly pushed branch); then every commit in
HEAD that is not reachable from the baseline is checked. Standard library only.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass

# Last commit before the DCO was adopted ("Init repo").
BASELINE = "761f362ffc54957724d5162974f8adae446fbcb3"

SIGNOFF = re.compile(r"^signed-off-by:\s*(?P<name>.*?)\s*<(?P<email>[^<>\s]+)>\s*$", re.I | re.M)


@dataclass(frozen=True)
class Commit:
    sha: str
    author_name: str
    author_email: str
    message: str

    @property
    def subject(self) -> str:
        return self.message.splitlines()[0] if self.message else ""


def git(repo: str, *args: str) -> str:
    out = subprocess.run(
        ["git", "-C", repo, *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return out.stdout


def commit_exists(repo: str, rev: str) -> bool:
    if not rev or set(rev) == {"0"}:
        return False
    probe = subprocess.run(
        ["git", "-C", repo, "cat-file", "-e", f"{rev}^{{commit}}"],
        capture_output=True,
    )
    return probe.returncode == 0


def commits_to_check(repo: str, base: str, head: str, baseline: str) -> list[Commit]:
    args = ["rev-list", "--no-merges", "--reverse", head]
    for exclude in (base, baseline):
        if commit_exists(repo, exclude):
            args.append(f"^{exclude}")
    shas = git(repo, *args).split()
    result = []
    for sha in shas:
        raw = git(repo, "show", "-s", "--format=%an%x00%ae%x00%B", sha)
        name, email, message = raw.split("\x00", 2)
        result.append(Commit(sha, name, email, message.strip()))
    return result


def is_signed_off(commit: Commit) -> bool:
    author = commit.author_email.strip().lower()
    return any(m.group("email").lower() == author for m in SIGNOFF.finditer(commit.message))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base", help="exclusive start of the range (may be empty or zeros)")
    parser.add_argument("head", help="inclusive end of the range")
    parser.add_argument("--baseline", default=BASELINE, help="commits reachable from this are exempt")
    parser.add_argument("--repo", default=".", help="path to the git repository")
    args = parser.parse_args(argv)

    commits = commits_to_check(args.repo, args.base, args.head, args.baseline)
    unsigned = [c for c in commits if not is_signed_off(c)]
    if unsigned:
        print("DCO check FAILED. These commits lack a matching Signed-off-by line:", file=sys.stderr)
        for c in unsigned:
            print(f"  - {c.sha[:12]} {c.subject!r} (author <{c.author_email}>)", file=sys.stderr)
        print(
            "\nSign off with `git commit -s` (see CONTRIBUTING.md). To fix existing commits:\n"
            "  git rebase --signoff <base>   then force-push your branch.",
            file=sys.stderr,
        )
        return 1
    print(f"DCO check passed ({len(commits)} commit(s) checked).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
