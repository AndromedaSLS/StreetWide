"""Command line: python -m tools.contrib --help"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from tools.contrib.facade import ContributionError, export_reference, find_building, make_bundle
from tools.contrib.gltf_in import MAX_BYTES


def _read_model(path: Path) -> bytes:
    """The model file, read no further than the glb reader accepts (it
    refuses anything longer), so a huge or endless file is not loaded."""
    try:
        with path.open("rb") as f:
            return f.read(MAX_BYTES + 1)
    except OSError as e:
        raise ContributionError(f"cannot read {path}: {e}") from e


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m tools.contrib", description="Make StreetWide contributions (RFC 0002).")
    sub = p.add_subparsers(dest="command", required=True)

    ref = sub.add_parser("reference", help="find a building and export its shell to model on")
    ref.add_argument("--world", type=Path, required=True, help="world store, e.g. data/world/zurich-altstadt")
    where = ref.add_mutually_exclusive_group(required=True)
    where.add_argument("--at", help="lon,lat (WGS84) of the building")
    where.add_argument("--building", help="the building's feature id")
    ref.add_argument("--out", type=Path, default=Path("contrib"), help="where to write the reference (default: contrib/)")

    fac = sub.add_parser("facade", help="turn a facade model (.glb) into a contribution bundle")
    fac.add_argument("--world", type=Path, required=True)
    fac.add_argument("--building", required=True, help="the building's feature id (from `reference`)")
    fac.add_argument("--model", type=Path, required=True, help="your facade, exported as glTF binary (.glb)")
    fac.add_argument("--author", required=True, help="your public handle, e.g. github:<login>")
    fac.add_argument("--title", required=True)
    fac.add_argument("--rationale", required=True, help="what you did and from which sources")
    fac.add_argument("--derived-from-shell", action="store_true",
                     help="the model reuses the shell's geometry (adds the shell's sources and licence)")
    fac.add_argument("--fictional", action="store_true", help="the facade is invented, not observed")
    fac.add_argument("--out", type=Path, required=True, help="bundle directory to create")
    args = p.parse_args(argv)

    try:
        if args.command == "reference":
            if args.at:
                lon, lat = (float(v) for v in args.at.split(","))
                feature = find_building(args.world, lon, lat)
            else:
                feature = args.building
            path = export_reference(args.world, feature, args.out)
            print(f"building {feature}")
            print(f"reference {path}  (open it in your 3D editor: metres, Y up; model the facade on it, don't move it)")
        else:
            if args.out.exists() and any(args.out.iterdir()):
                raise ContributionError(f"{args.out} is not empty")
            change_id = make_bundle(
                args.world,
                args.building,
                _read_model(args.model),
                args.out,
                author=args.author,
                title=args.title,
                rationale=args.rationale,
                derived_from_shell=args.derived_from_shell,
                fictional=args.fictional,
            )
            print(f"bundle {args.out}, change {change_id}")
            print(f"check it: cargo run -p op_replay -- check-change {args.world.as_posix()} {args.out.as_posix()}")
    except ContributionError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
