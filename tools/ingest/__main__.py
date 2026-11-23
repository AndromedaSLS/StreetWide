"""Command line: python -m tools.ingest --help"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from tools.ingest.geo import Area, BBox, parse_tile_key
from tools.ingest.pipeline import Spawn, run, write_report

REPO = Path(__file__).resolve().parents[2]
WORLDS = REPO / "worlds"


def _spawn(text: str) -> Spawn:
    parts = [float(p) for p in text.split(",")]
    if len(parts) not in (2, 3):
        raise argparse.ArgumentTypeError("spawn must be lon,lat[,yaw_deg]")
    return Spawn(*parts)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        prog="python -m tools.ingest",
        description="Build a StreetWide world from swisstopo and OpenStreetMap data.",
    )
    what = p.add_mutually_exclusive_group(required=True)
    what.add_argument("--bbox", type=BBox.parse, help="minlon,minlat,maxlon,maxlat (WGS84)")
    what.add_argument("--tile", help="a single z17 tile, e.g. 17/68646/45902")
    what.add_argument("--world", help=f"a world definition in {WORLDS.relative_to(REPO)}/<name>.json")
    p.add_argument("--name", help="world name (lowercase words joined by '-'); default: from --world, else 'custom'")
    p.add_argument("--spawn", type=_spawn, help="lon,lat[,yaw_deg]; default: centre of the area, facing north")
    p.add_argument("--out", type=Path, help="output store directory; default: data/world/<name>")
    p.add_argument("--cache", type=Path, default=Path(os.environ.get("STREETWIDE_CACHE", REPO / "data" / "cache")),
                   help="download cache (default: $STREETWIDE_CACHE or data/cache)")
    p.add_argument("--offline", action="store_true", help="use only cached data; fail if anything is missing")
    p.add_argument("--refresh", action="store_true", help="re-query STAC and Overpass instead of using cached answers")
    args = p.parse_args(argv)

    spawn = args.spawn
    name = args.name
    if args.world:
        spec = json.loads((WORLDS / f"{args.world}.json").read_text(encoding="utf-8"))
        name = name or spec["name"]
        if "tile" in spec:
            area = Area.from_tile(*parse_tile_key(spec["tile"]))
            if "bbox" in spec:
                area = area.intersect(Area.from_bbox(BBox(*spec["bbox"])))
        else:
            area = Area.from_bbox(BBox(*spec["bbox"]))
        if spawn is None and "spawn" in spec:
            s = spec["spawn"]
            spawn = Spawn(s["lon"], s["lat"], s.get("yaw_deg", 0.0))
    elif args.tile:
        area = Area.from_tile(*parse_tile_key(args.tile))
    else:
        area = Area.from_bbox(args.bbox)
    name = name or "custom"
    out = args.out or REPO / "data" / "world" / name

    report = run(name, area, spawn, out, args.cache, offline=args.offline, refresh=args.refresh)
    report_path = REPO / "data" / "reports" / f"{name}.json"
    write_report(report, report_path)
    print(json.dumps({k: report[k] for k in ("world_id", "tiles", "assets", "files", "bytes", "seconds")}, indent=2))
    print(f"report: {report_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
