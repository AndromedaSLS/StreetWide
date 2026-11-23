"""Contribution documents (RFC 0002) from Python: the committed sample
change set and gate report are canonical, schema-valid and consistent."""

from pathlib import Path

from tools.worldfmt import cjson, schemas
from tools.worldfmt.ids import content_id
from tools.worldfmt.store import Store

SAMPLE = Path(__file__).resolve().parents[3] / "fixtures" / "changes" / "facade-sample"
WORLD = Store(Path(__file__).resolve().parents[3] / "fixtures" / "world")


def test_sample_change_set_and_report():
    change_bytes = (SAMPLE / "change.json").read_bytes()
    change = cjson.loads(change_bytes)
    report = cjson.loads((SAMPLE / "report.json").read_bytes())
    assert schemas.errors(change) == []
    assert schemas.errors(report) == []
    assert report["change_id"] == content_id(change_bytes)
    assert report["world_id"] == change["base_world_id"] == WORLD.read_world()[0]
    assert report["passed"] is True
    assert [c["id"] for c in report["checks"]] == [f"G{i}" for i in range(1, 11)]


def test_sample_manifest_meets_the_contribution_rules():
    change = cjson.loads((SAMPLE / "change.json").read_bytes())
    bundle = Store(SAMPLE)
    (entry,) = change["changes"]
    manifest = cjson.loads(bundle.read_object(entry["manifest_id"], "json"))
    assert schemas.errors(manifest) == []
    prov = manifest["provenance"]
    assert manifest["layer"] == "facade" and manifest["physics"]["collider"] == "none"
    assert set(prov["license"].split(" AND ")) == {"CC-BY-SA-4.0"} | {s["license"] for s in prov["sources"]}
    contribution = next(s for s in prov["sources"] if s["dataset"] == "StreetWide contributions")
    assert change["author"] in contribution["source_ids"]
    assert "The StreetWide contributors" in prov["attribution"]
