//! On-screen text: data credits (always shown), status, help, loading
//! and replay results; and the end of `--replay --exit-when-done` runs.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::Options;
use crate::scene::StreamStats;
use crate::session::{Playback, Session};

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct CenterText;

/// A system font with the glyphs the credits need (`©`, `·`); Bevy's
/// built-in font covers ASCII only.
#[derive(Resource, Default)]
struct UiFont(Option<Handle<Font>>);

impl UiFont {
    fn style(&self, size: f32) -> (TextFont, TextColor) {
        let mut font = TextFont {
            font_size: FontSize::Px(size),
            ..default()
        };
        if let Some(h) = &self.0 {
            font.font = h.clone().into();
        }
        (font, TextColor(Color::WHITE))
    }

    /// The text as it can be displayed: with the built-in font, non-ASCII
    /// marks are spelled out (`(c)` for `©`).
    fn show(&self, text: &str) -> String {
        if self.0.is_some() {
            return text.to_string();
        }
        text.replace('©', "(c)")
            .replace('·', "|")
            .replace('…', "...")
    }
}

fn system_font_paths() -> Vec<String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:/Windows".into());
    let mut paths = vec![
        format!("{windir}/Fonts/segoeui.ttf"),
        format!("{windir}/Fonts/arial.ttf"),
    ];
    paths.extend(
        [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]
        .map(String::from),
    );
    paths
}

/// Frames to wait between asking for a screenshot and exiting.
#[derive(Resource, Default)]
struct Finish {
    frames_left: Option<u32>,
    shot_taken: bool,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Finish>()
            .init_resource::<UiFont>()
            .add_systems(Startup, setup)
            .add_systems(Update, (update_text, finish));
    }
}

fn setup(mut commands: Commands, session: Res<Session>, mut fonts: ResMut<Assets<Font>>) {
    let font = UiFont(
        system_font_paths()
            .iter()
            .find_map(|p| std::fs::read(p).ok())
            .map(|bytes| fonts.add(Font::from_bytes(bytes))),
    );
    let credits = font.show(&session.streamer().index().world().attribution.join(" · "));
    commands.spawn((
        Text::new(credits),
        font.style(14.0),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
            ..default()
        },
    ));
    commands.spawn((
        Text::new(""),
        font.style(15.0),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        StatusText,
    ));
    commands.spawn((
        Text::new(""),
        font.style(28.0),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(30.0),
            right: Val::Percent(30.0),
            top: Val::Percent(42.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        TextLayout::justify(Justify::Center),
        CenterText,
    ));
    commands.insert_resource(font);
}

fn update_text(
    time: Res<Time>,
    font: Res<UiFont>,
    mut session: ResMut<Session>,
    stats: Res<StreamStats>,
    mut status: Single<&mut Text, (With<StatusText>, Without<CenterText>)>,
    mut center: Single<&mut Text, (With<CenterText>, Without<StatusText>)>,
) {
    let sim = session.sim();
    let p = sim.player();
    let [x, y, z] = p.feet();
    let mode = if session.is_playback() {
        "replay"
    } else {
        "live"
    };
    let mut s = format!(
        "{} ({mode})  tick {}  feet ({x:.1}, {y:.1}, {z:.1})  colliders {}  meshes {}/{}\n",
        session.world_name(),
        sim.ticks(),
        sim.active_colliders(),
        stats.shown,
        stats.shown + stats.wanted,
    );
    if !session.is_playback() {
        s.push_str("click: look around · WASD: move · Shift: sprint · Space: jump · F9: save replay · R: restart · Esc: release mouse");
    }
    if let Some((msg, left)) = &mut session.message {
        s.push('\n');
        s.push_str(msg);
        *left -= time.delta_secs();
        if *left <= 0.0 {
            session.message = None;
        }
    }
    status.0 = font.show(&s);

    center.0 = font.show(&if let Some(e) = &session.error {
        format!("Simulation error: {e}")
    } else if session.stalled {
        "Loading…".into()
    } else {
        match &session.playback {
            Some(Playback::Matched(h)) => format!(
                "Replay matches every checkpoint\nfinal state {}",
                &h.hex()[..16]
            ),
            Some(Playback::Diverged { tick, .. }) => format!("Replay DIVERGED at tick {tick}"),
            _ => String::new(),
        }
    });
}

/// With `--exit-when-done`: once a replay has finished, take the optional
/// screenshot and exit (code 0 if every checkpoint matched, 1 otherwise).
fn finish(
    mut commands: Commands,
    options: Res<Options>,
    session: Res<Session>,
    mut state: ResMut<Finish>,
    mut exit: MessageWriter<AppExit>,
) {
    if let (Some(path), Some(at)) = (&options.screenshot, options.screenshot_tick)
        && !state.shot_taken
        && session.sim().ticks() >= at
    {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        state.shot_taken = true;
    }
    let live_shot_done =
        !session.is_playback() && options.screenshot_tick.is_some() && state.shot_taken;
    let done = matches!(
        session.playback,
        Some(Playback::Matched(_)) | Some(Playback::Diverged { .. })
    ) || session.error.is_some()
        || live_shot_done;
    if !done {
        return;
    }
    match state.frames_left {
        None => {
            match &session.playback {
                Some(Playback::Matched(h)) => {
                    info!("replay matches every checkpoint, final state {h}")
                }
                Some(Playback::Diverged {
                    tick,
                    expected,
                    actual,
                }) => {
                    error!("replay diverged at tick {tick}: recorded {expected}, got {actual}")
                }
                _ => {}
            }
            if let Some(path) = &options.screenshot
                && !state.shot_taken
            {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path.clone()));
                state.shot_taken = true;
            }
            state.frames_left = Some(if options.screenshot.is_some() { 30 } else { 1 });
        }
        Some(0) => {
            if options.exit_when_done {
                let ok = session.error.is_none()
                    && (live_shot_done || matches!(session.playback, Some(Playback::Matched(_))));
                exit.write(if ok {
                    AppExit::Success
                } else {
                    AppExit::from_code(1)
                });
            }
        }
        Some(n) => state.frames_left = Some(n - 1),
    }
}
