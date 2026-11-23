# A minimap

Labels: `good first issue`, `area: client`

## Why

In the old town's narrow streets it is easy to lose your bearings. A small
top-down map with your position and heading helps exploring.

## What to do

- Add a minimap in a corner of the screen, about 200 × 200 px, showing
  about 150 m around the player, north up.
- Show buildings (shell assets) as filled shapes, streets (transport
  assets) as lines, and the player as an arrow pointing along the heading.
- Two approaches:
  - a second `Camera3d` with an orthographic projection looking down,
    rendering into a UI image or a viewport;
  - drawing the map yourself from the asset bounds or meshes with Bevy
    gizmos or UI nodes.

  The second camera is the simpler start.
- **M** toggles the minimap.

## Done when

- The minimap follows the player and rotates the arrow, not the map.
- It costs little frame time with the whole district loaded. Mention what
  you measured in the pull request.
- Clippy and tests pass.

## Pointers

- `crates/op_client/src/scene.rs` (camera, streamed meshes),
  `crates/op_client/src/hud.rs`
- Bevy examples on multiple cameras and render-to-texture (Bevy 0.20)
