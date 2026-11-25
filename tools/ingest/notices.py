"""The licence notices of a world (ADR-0019, "Notices with the world").

The published world is offered as a whole under the Open Database License
(ODbL) 1.0, and each asset keeps the licence and credits of its own
sources (ADR-0019, option A). The ODbL asks for its text or address to
travel with the database (section 4.2), so every world the ingest writes
has, at the root of its store, beside `world.json`:

    NOTICE.txt          the ODbL notice, and the notice for pictures and
                        videos made from the world (ODbL section 4.3)
    CREDITS.txt         each source, its licence and a link
    ODbL-1.0.txt        the text of the ODbL, from LICENSES/
    CC-BY-SA-4.0.txt    the text of CC BY-SA 4.0, from LICENSES/, the
                        licence of contributions, which merges add later

They are not part of the world: no id covers them, readers of a store
never look at them, and merges leave them as they are. Their text depends
only on this module, never on the world, so they stay true when
contributions are merged. When a new source enters the ingest, it is
added to SOURCES here and to ATTRIBUTION.md; the ingest refuses to write
a world with a source that SOURCES does not credit.
"""

from __future__ import annotations

import textwrap
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from tools.worldfmt.store import NOTICE_FILES

REPO = Path(__file__).resolve().parents[2]
REPOSITORY_URL = "https://github.com/AndromedaSLS/StreetWide"
ODBL_URL = "https://opendatacommons.org/licenses/odbl/1-0/"
OSM_COPYRIGHT_URL = "https://www.openstreetmap.org/copyright"
SWISSTOPO_TERMS_URL = "https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices"

NOTICE, CREDITS, ODBL_TEXT, CC_BY_SA_TEXT = NOTICE_FILES
LICENCE_TEXTS = {ODBL_TEXT: REPO / "LICENSES" / "ODbL-1.0.txt", CC_BY_SA_TEXT: REPO / "LICENSES" / "CC-BY-SA-4.0.txt"}


@dataclass(frozen=True)
class Source:
    """A source of world data, as ATTRIBUTION.md credits it. `dataset`,
    `license` and `credit` are what manifests name (`provenance.sources`)."""
    dataset: str
    by: str
    used_for: str
    credit: str
    license: str
    licence_name: str
    licence_url: str
    licence_file: str | None
    url: str


SOURCES = (
    Source("swissBUILDINGS3D 3.0 Beta", "swisstopo", "building shells (roofs, walls, ground plates)",
           "© swisstopo", "LicenseRef-swisstopo-OGD", "swisstopo terms for free geodata", SWISSTOPO_TERMS_URL,
           None, "https://www.swisstopo.admin.ch/en/landscape-model-swissbuildings3d-3-0-beta"),
    Source("swissALTI3D", "swisstopo",
           "terrain; the heights of roads, bridges, water surfaces and OpenStreetMap fallback buildings",
           "© swisstopo", "LicenseRef-swisstopo-OGD", "swisstopo terms for free geodata", SWISSTOPO_TERMS_URL,
           None, "https://www.swisstopo.admin.ch/en/height-model-swissalti3d"),
    Source("OpenStreetMap", "the OpenStreetMap contributors",
           "roads, paths, steps and bridges; water areas; building footprints where swissBUILDINGS3D has "
           "no building", "© OpenStreetMap contributors", "ODbL-1.0", "Open Database License 1.0", ODBL_URL,
           ODBL_TEXT, OSM_COPYRIGHT_URL),
    Source("StreetWide contributions", "the players",
           "what players add, such as facades, once merged into the world",
           "The StreetWide contributors", "CC-BY-SA-4.0",
           "Creative Commons Attribution-ShareAlike 4.0 International",
           "https://creativecommons.org/licenses/by-sa/4.0/", CC_BY_SA_TEXT, REPOSITORY_URL),
)

NOTICE_TEXT = f"""StreetWide world: licence notice

This world is a database built from open data by StreetWide
({REPOSITORY_URL}).
It is made available under the Open Database License (ODbL) 1.0:
{ODBL_URL}
The text of the licence is in {ODBL_TEXT}, beside this file.

It contains information from OpenStreetMap, which is made available here
under the Open Database License (ODbL).
© OpenStreetMap contributors, {OSM_COPYRIGHT_URL}

The world is offered as a whole under the ODbL. Inside it, each asset
keeps the licence and the credits of its own sources: its manifest (a JSON
file in objects/) states them under "provenance", and world.json lists
every credit the world needs under "attribution". {CREDITS} lists each
source, its licence and a link.

Pictures, videos and other works made from this world
If you show such a work in public, show with it the credits that
world.json lists and this notice (ODbL section 4.3):

    Contains information from the StreetWide world, which is made
    available here under the Open Database License (ODbL).

Link "the StreetWide world" to {REPOSITORY_URL},
"Open Database License" to {ODBL_URL}
and "© OpenStreetMap contributors" to {OSM_COPYRIGHT_URL},
or write the addresses out where links are not possible.

Keep this notice, {CREDITS} and the licence texts with the world when you
copy it or pass it on. The ingest that builds the world is open source, in
the repository above.
"""


def _source_text(s: Source) -> str:
    text = f" (text in {s.licence_file})" if s.licence_file else ""
    used_for = "\n            ".join(textwrap.wrap(s.used_for, 74 - 12))
    return (f"{s.dataset}, by {s.by}\n"
            f"  Used for: {used_for}\n"
            f"  Credit:   {s.credit}\n"
            f"  Licence:  {s.licence_name} ({s.license})\n"
            f"            {s.licence_url}{text}\n"
            f"  Source:   {s.url}\n")


CREDITS_TEXT = f"""StreetWide world: credits

This world is made available as a whole under the Open Database License
(ODbL) 1.0: see {NOTICE}. Inside it, each asset keeps the licence and the
credits of its own sources, which its manifest states. Wherever you show
the world, or a picture or video made from it, show the credits that
world.json lists under "attribution".

These are the sources StreetWide builds its worlds from:

""" + "\n".join(_source_text(s) for s in SOURCES)


class NoticeError(ValueError):
    """A source the notices do not credit, or credit differently."""


def check_credited(sources: Iterable[dict]) -> None:
    """Raises NoticeError unless every source (a manifest's
    `provenance.sources` entry) is in SOURCES with the same licence and
    credit: a world must not go out with notices that leave a source out."""
    known = {s.dataset: s for s in SOURCES}
    for source in sources:
        s = known.get(source["dataset"])
        if s is None:
            raise NoticeError(f"the source {source['dataset']!r} is not in the credits: add it to SOURCES in "
                              "tools/ingest/notices.py and to ATTRIBUTION.md")
        if (source["license"], source["attribution"]) != (s.license, s.credit):
            raise NoticeError(f"the source {s.dataset!r} has licence {source['license']!r} and credit "
                              f"{source['attribution']!r}, but the credits say {s.license!r} and {s.credit!r}")


def notices() -> dict[str, bytes]:
    """Every notice file, by name, as written: UTF-8 with LF line ends."""
    files = {NOTICE: NOTICE_TEXT.encode("utf-8"), CREDITS: CREDITS_TEXT.encode("utf-8")}
    files.update({name: path.read_bytes() for name, path in LICENCE_TEXTS.items()})
    return files


def write(root: Path) -> None:
    """Writes the notices at the root of the store in `root`, beside its
    `world.json`."""
    root.mkdir(parents=True, exist_ok=True)
    for name, data in notices().items():
        (root / name).write_bytes(data)
