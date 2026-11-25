"""Command line: python -m tools.ingest.changes <command> --help

Commands register themselves in `COMMANDS`: each module has a
`register(subparsers)` function that adds its parser and sets `run`, the
function that runs it and returns the exit code:
- `plan` writes change sets and a refresh report (ADR-0020, section 2);
- `verify` runs a plan's change sets through the real gate, in order, and
  checks every id it predicted (ADR-0020, section 2). It reads the plan through
  `tools.ingest.changes.layout.read_plan`.
"""

from __future__ import annotations

import argparse
import sys

from tools.ingest.changes import VERSION, plan, verify

COMMANDS = (plan, verify)


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="python -m tools.ingest.changes",
        description="Baseline refreshes (RFC 0002 section 7): plan change sets from a fresh ingest, "
                    "and verify them through the gate.",
    )
    p.add_argument("--version", action="version", version=f"%(prog)s {VERSION}")
    sub = p.add_subparsers(dest="command", metavar="COMMAND")
    for module in COMMANDS:
        module.register(sub)
    return p


def _tolerant_output() -> None:
    """Writes what the console's code page cannot show as backslash escapes
    instead of failing: a path given on the command line may hold such
    characters, and a print after the plan is written must not turn a
    written plan into a traceback (Windows writes piped output in its ANSI
    code page)."""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(errors="backslashreplace")


def main(argv: list[str] | None = None) -> int:
    _tolerant_output()
    p = parser()
    try:
        args = p.parse_args(argv)
    except SystemExit as e:  # argparse reports usage errors with exit code 2
        return int(e.code or 0)
    if args.command is None:
        p.print_usage(sys.stderr)
        return 2
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
