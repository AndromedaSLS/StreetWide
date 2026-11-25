"""Licences and credits of an asset against its sources, as
`Provenance::licence_problems` (crates/op_world/src/manifest.rs) checks them
in whole-world validation, message for message: ADR-0017's rules, which
ADR-0015 and ADR-0016 ask for before their data enters a world.

- the asset's licence joins licence ids with ` AND ` only;
- each source names the one licence id it is used under;
- every source's licence is among the asset's;
- every source's credit is in the asset's attribution.

The ingest checks every manifest with `licence_problems` before it writes
it; `op_world validate`, and so the gate's G1, checks every world.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

from tools.worldfmt.rust import rust_debug

_LICENSE_ID = re.compile(r"[A-Za-z0-9.+:-]+")


def is_license_id(s: object) -> bool:
    """`rules::is_license_id`: one licence id, `^[A-Za-z0-9.+:-]+$`, at most
    256 characters: no spaces, parentheses or operators."""
    return isinstance(s, str) and len(s) <= 256 and _LICENSE_ID.fullmatch(s) is not None


def license_ids(s: str) -> list[str] | None:
    """`rules::license_ids`: the ids of a licence that joins them with
    ` AND ` only, such as `CC0-1.0 AND LicenseRef-swisstopo-OGD`; None
    otherwise."""
    ids = s.split(" AND ")
    return ids if all(is_license_id(i) for i in ids) else None


def licence_problems(provenance: Mapping[str, Any]) -> list[str]:
    """`Provenance::licence_problems` of a manifest's `provenance` (which
    must pass the asset-manifest schema): [] when it keeps ADR-0017's rules."""
    found = []
    ids = license_ids(provenance["license"])
    if ids is None:
        found.append(f"provenance.license {rust_debug(provenance['license'])} must join licence ids with AND only "
                     "(ADR-0017)")
    for i, s in enumerate(provenance["sources"]):
        if not is_license_id(s["license"]):
            found.append(f"provenance.sources[{i}].license {rust_debug(s['license'])} must be one licence id (ADR-0017)")
        elif ids is not None and s["license"] not in ids:
            found.append(f"provenance.sources[{i}].license {rust_debug(s['license'])} is not in the asset's licence "
                         f"{rust_debug(provenance['license'])} (ADR-0017)")
        if s["attribution"] not in provenance["attribution"]:
            found.append(f"provenance.sources[{i}].attribution {rust_debug(s['attribution'])} is not in the asset's "
                         "attribution (ADR-0017)")
    return found
