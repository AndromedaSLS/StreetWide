//! StreetWide desktop client.
//!
//! Stage 1: an empty window, proving the pinned Bevy version builds on every
//! CI platform. Tile streaming and first-person walking arrive in stage 5.

use bevy::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "StreetWide".into(),
                ..default()
            }),
            ..default()
        }))
        .run()
}
