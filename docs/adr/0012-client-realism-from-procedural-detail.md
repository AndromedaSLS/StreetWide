# ADR-0012: Make the client look real with procedural detail and physically based light

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** @victorpicaud
- **Supersedes / superseded by:** none (extends ADR-0008)

## Context

Until now the client drew every surface in one flat palette colour per
material (RFC 0001: no textures in format v0), under a fixed sun and a
plain sky. The geometry is accurate, but the old town looked like grey and
beige blocks: no windows, no roof tiles, no paving, no time of day.

The constraints:
- **Data and decoration are different things.** The canonical world holds
  measured or contributed data with provenance. Anything the client
  invents must not pass for data. In particular, facades are what
  contributors add (RFC 0002), so the client must not compete with them.
- **Every asset carries provenance and licence.** Images from third
  parties would need manifests, licences and downloads; large files don't
  belong in Git.
- **Rendering may be nondeterministic; the simulation may not.** Nothing
  here may reach `op_sim`.
- The world format has no textures yet, and changing it needs an RFC.

## Options considered

### A. Procedural detail in the client, and physically based light (recommended)

The client generates its textures in code at start-up:
- plaster with windows, three styles of window and shutter;
- three matching ground floors;
- clay tiles, granite setts, cobbles, asphalt, paving, stone and grass.

It maps them in world space, so a cobble is the same size everywhere. It
chooses a style and colour per building from the building's id. Light
comes from Bevy's atmosphere, with the sun at its real position for the
date and time.

- Pros:
  - no files and no new licences: the textures are code (MIT OR
    Apache-2.0);
  - no format change and no simulation change;
  - the biggest visible improvement for the effort;
  - works for any world without per-building work.
- Cons:
  - the detail is generic, not the real building. A church gets windows
    like a house, and every house's windows are evenly spaced.
  - It needs care so it is never mistaken for data (below).

### B. Photographic textures from CC0 libraries

For example ambientCG or Poly Haven, downloaded by a tool.

- Pros: richer materials than procedural ones.
- Cons:
  - still generic, not the real buildings;
  - needs an asset pipeline, manifests and downloads;
  - adds hundreds of megabytes for the gain of better-looking plaster.

### C. Real imagery: swisstopo orthophotos

SWISSIMAGE is 10 cm aerial imagery, free under the same terms as the
other swisstopo data. It would be draped on roofs and open ground.

- Pros: real roof colours, real squares, real river; data with
  provenance, like the rest of the world.
- Cons:
  - needs textures in the world format (an RFC), texture streaming and
    about 300 MB of images per km²;
  - shows nothing of facades.

### D. Keep the palette

- Pros: nothing to do; everything shown is data.
- Cons: the world does not look like a place, which works against
  attracting contributors.

## Decision

Option A now. Option C is the strongest candidate for the next world-format
RFC (roofs and open ground). Facades stay with contributors.

**What the client draws** (`op_client`: `texgen`, `materials`, `heights`,
`sky`):
- **Walls**, for buildings 4.5 m or taller:
  - windows in one of three styles, one style per building, which is
    stable because it is chosen from the feature id;
  - each wall is divided into a whole number of bays, so no window is cut
    at a corner; walls under 1.6 m wide stay plain;
  - storeys count from the terrain at the middle of each wall, so the
    ground floor (shop fronts, doors, or rusticated stone) sits at street
    level even on slopes; the wall below street level is a plain base;
  - buildings that have a contributed facade get plain plaster, so the
    contribution is what you see.
- **Roofs:** clay tiles in rows across the slope; flat roofs get gravel.
- **Ground:**
  - flat open terrain (under 12°) is paved with granite setts, steeper
    terrain is grass and soil;
  - paths are cobbles, roads asphalt, bridges stone slabs.
- **Colour:** eight plaster colours and four roof variations, chosen per
  building.
- **Light:**
  - a physically based atmosphere (sky, sun disc, aerial perspective,
    sky light);
  - the sun at its computed position (NOAA algorithm) for the date and
    Swiss civil time;
  - exposure that follows the sun's elevation;
  - ACES tonemapping, chosen after comparing three operators on the same
    street;
  - cascaded shadows, ambient occlusion and temporal anti-aliasing. F4
    switches to a lighter set for slower GPUs.

**Keeping decoration apart from data:**
- F2 switches to the plain look, which shows exactly the world data's
  palette colours.
- The README and the client's help say which details are drawn by the
  client.
- Nothing drawn is saved, merged or checked by the contribution gate.

## Consequences

- **Measured:**
  - generating the textures takes 0.9 s at start-up;
  - 169 frames per second at 1280 × 720 on an RTX 3070 with everything
    loaded, on the high setting.
- **Known limits:**
  - **Churches and other special buildings** get house windows: the
    swissBUILDINGS3D shells carry no building type. A type from
    OpenStreetMap could choose styles; that needs the ingest to keep it.
  - **The Limmat** is terrain in the data, so it now looks like a large
    paved square. A water layer is the next step for realism, and needs
    data. Options:
    - swisstopo's swissTLM3D water areas, which can join the terrain
      asset because they come from the same licensor;
    - OpenStreetMap water areas, which would be a separate ODbL asset.
  - **Streets** have no furniture, trees or people.
- **Not testable in CI:** CI has no GPU. The texture, mapping, height and
  sun code have unit tests. The look was checked with screenshots taken
  by the client's own `--screenshot` option.
- **Revisit** when the world format gains textures (option C), or when
  building types or water exist in the data.
