//! On-screen text: data credits (always shown), status, help (F1), debug
//! details (F3), loading and replay results, and inspect mode's crosshair
//! and panel; and the end of `--replay --exit-when-done` runs.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::Options;
use crate::controls::Look;
use crate::inspect::{self, Inspect, InspectStats, InspectSystems};
use crate::materials::MaterialSet;
use crate::scene::{PlayerCamera, Quality, StreamStats};
use crate::session::{Playback, Session};
use crate::sky::{Clock, MoonState, SunState, exposure_ev100};
use crate::travel::{Travel, View};

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct CenterText;

#[derive(Component)]
struct HelpText;

#[derive(Component)]
struct DebugText;

/// One bar of inspect mode's crosshair: bright, or its dark border.
#[derive(Component)]
struct Crosshair {
    bright: bool,
}

#[derive(Component)]
struct InspectPanel;

#[derive(Component)]
struct InspectHeader;

#[derive(Component)]
struct InspectBody;

/// Which panels are open.
#[derive(Resource)]
pub struct Panels {
    pub help: bool,
    pub debug: bool,
}

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
            .add_systems(
                Update,
                (panel_keys, update_text, update_panels, finish).chain(),
            )
            .add_systems(Update, update_inspect.after(InspectSystems));
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
    commands.spawn((
        Text::new(""),
        font.style(14.0),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(48.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        HelpText,
    ));
    commands.spawn((
        Text::new(""),
        font.style(14.0),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        DebugText,
    ));
    // Inspect mode: a crosshair at the centre of the screen, where the
    // camera's ray goes, with a dark border so it shows on bright walls.
    for (bright, grow) in [(false, 2.0), (true, 0.0)] {
        for (w, h) in [(14.0 + grow, 2.0 + grow), (2.0 + grow, 14.0 + grow)] {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(w),
                    height: Val::Px(h),
                    margin: UiRect {
                        left: Val::Px(-w / 2.0),
                        top: Val::Px(-h / 2.0),
                        ..default()
                    },
                    ..default()
                },
                BackgroundColor(if bright {
                    Color::WHITE
                } else {
                    Color::srgba(0.0, 0.0, 0.0, 0.6)
                }),
                Visibility::Hidden,
                Crosshair { bright },
            ));
        }
    }
    // ... and its panel, bottom left, above the credits.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(26.0),
                // A definite width: with only a maximum, the box takes the
                // height of the text laid out on one long line, and a
                // wrapped command spills out below it.
                width: Val::Percent(62.0),
                padding: UiRect::all(Val::Px(8.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            Visibility::Hidden,
            InspectPanel,
        ))
        .with_children(|panel| {
            panel.spawn((Text::new(""), font.style(16.0), InspectHeader));
            panel.spawn((Text::new(""), font.style(14.0), InspectBody));
        });
    commands.insert_resource(font);
}

fn panel_keys(keys: Res<ButtonInput<KeyCode>>, mut panels: ResMut<Panels>) {
    if keys.just_pressed(KeyCode::F1) {
        panels.help = !panels.help;
    }
    if keys.just_pressed(KeyCode::F3) {
        panels.debug = !panels.debug;
    }
}

fn help_text(session: &Session, travel: &Travel) -> String {
    let mut lines = vec![
        "WALK    mouse: look (click to capture, Esc to release)".to_string(),
        "        W A S D (Z Q S D on AZERTY): move · Shift: sprint · Space: jump".into(),
        "        R: back to the start · F9: save a replay of this session".into(),
        "TRAVEL  V: fly (W A S D; Space up, Ctrl down; Shift faster;".into(),
        "        wheel: speed) · Enter while flying: land below".into(),
        "        M: map (W A S D to pan, wheel to zoom, click to go there)".into(),
    ];
    let places: Vec<String> = travel
        .places
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, p)| format!("{}: {}", i + 1, p.name))
        .collect();
    for row in places.chunks(3) {
        lines.push(format!("        {}", row.join(" · ")));
    }
    lines.push("VIEW    T, Shift+T: time of day ±30 min · F2: detailed or plain look".into());
    lines.push("        F4: graphics high or fast · F3: details · F1: hide this help".into());
    lines.push("INSPECT I: what the crosshair is on, who made it, its licence;".into());
    lines.push("        amber: an improvable building · blue: contributed;".into());
    lines.push("        click: print it, with the facade tool's command".into());
    // OpenStreetMap data (streets, water) comes with its licence and where
    // to read it (ADR-0014).
    let attribution = &session.streamer().index().world().attribution;
    if attribution.iter().any(|a| a.contains("OpenStreetMap")) {
        lines.push("DATA    © OpenStreetMap contributors, under the Open Database".into());
        lines.push("        Licence (ODbL): openstreetmap.org/copyright".into());
    }
    if session.is_playback() {
        lines.push("(replay: travel and saving are off)".into());
    }
    lines.join("\n")
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_panels(
    time: Res<Time>,
    font: Res<UiFont>,
    panels: Res<Panels>,
    session: Res<Session>,
    travel: Res<Travel>,
    clock: Res<Clock>,
    sun: Res<SunState>,
    look: Res<Look>,
    quality: Res<Quality>,
    set: Res<MaterialSet>,
    stats: Res<StreamStats>,
    (inspect, pick, moon): (Res<Inspect>, Res<InspectStats>, Res<MoonState>),
    camera: Single<&Transform, With<PlayerCamera>>,
    mut help: Single<(&mut Text, &mut Visibility), (With<HelpText>, Without<DebugText>)>,
    mut debug: Single<(&mut Text, &mut Visibility), (With<DebugText>, Without<HelpText>)>,
    mut fps: Local<f32>,
) {
    let dt = time.delta_secs().max(1e-4);
    *fps = if *fps == 0.0 {
        1.0 / dt
    } else {
        *fps * 0.95 + 0.05 / dt
    };
    *help.1 = if panels.help {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    *debug.1 = if panels.debug {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if panels.help {
        help.0.0 = font.show(&help_text(&session, &travel));
    }
    if !panels.debug {
        return;
    }
    let cam = session.render_to_region(camera.translation);
    let (lon, lat, h) = session.frame().unproject(cam);
    let sim = session.sim();
    let [x, y, z] = sim.player().feet();
    let view = match travel.view {
        View::Walk => "walk".to_string(),
        View::Fly { .. } => format!("fly, {:.0} m/s", travel.fly_speed),
        View::Map { height, .. } => format!("map, {height:.0} m up"),
    };
    let d = clock.local;
    let mut lines = vec![
        format!("{:.0} fps ({:.1} ms)", *fps, 1000.0 / *fps),
        format!("camera {lon:.6}, {lat:.6}, {h:.1} m"),
        format!(
            "heading {:.0}°, pitch {:.0}°",
            look.yaw.to_degrees(),
            look.pitch.to_degrees()
        ),
        format!("feet (sim frame) {x:.2}, {y:.2}, {z:.2}"),
        format!("view {view}"),
        format!(
            "tick {}{} · colliders {}",
            sim.ticks(),
            if session.stalled { " (loading)" } else { "" },
            sim.active_colliders()
        ),
        format!("meshes {} shown, {} to load", stats.shown, stats.wanted),
        format!(
            "{}-{:02}-{:02} {} Swiss time",
            d.year,
            d.month,
            d.day,
            clock.hhmm()
        ),
        format!(
            "sun azimuth {:.0}°, elevation {:.1}°, EV100 {:.1}",
            sun.azimuth,
            sun.elevation,
            exposure_ev100(sun.elevation)
        ),
        format!(
            "moon azimuth {:.0}°, elevation {:.1}°, {:.0}% lit",
            moon.0.azimuth,
            moon.0.elevation,
            moon.0.illuminated * 100.0
        ),
        format!(
            "graphics {} · look {}",
            if quality.high { "high" } else { "fast" },
            if set.detailed_on { "detailed" } else { "plain" }
        ),
    ];
    if inspect.on {
        let p = pick.pick;
        let away = pick
            .distance
            .map_or("nothing hit".into(), |d| format!("{d:.1} m away"));
        lines.push(format!(
            "inspect pick {:.0} µs, {away} · searched {} of {} assets, {} triangles · {} grids built",
            pick.micros, p.assets, pick.shown, p.triangles, p.grids_built
        ));
    }
    debug.0.0 = font.show(&lines.join("\n"));
}

#[allow(clippy::too_many_arguments)]
fn update_text(
    time: Res<Time>,
    font: Res<UiFont>,
    mut session: ResMut<Session>,
    stats: Res<StreamStats>,
    travel: Res<Travel>,
    clock: Res<Clock>,
    mut status: Single<&mut Text, (With<StatusText>, Without<CenterText>)>,
    mut center: Single<&mut Text, (With<CenterText>, Without<StatusText>)>,
) {
    let sim = session.sim();
    let mode = if session.is_playback() {
        "replay"
    } else {
        "live"
    };
    let mut s = format!(
        "{} ({mode}) · {} · {} · tick {} · meshes {}/{} · F1: help",
        session.world_name(),
        travel.view.name(),
        clock.hhmm(),
        sim.ticks(),
        stats.shown,
        stats.shown + stats.wanted,
    );
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

/// Inspect mode's crosshair and panel. Text is set only when it changes.
#[allow(clippy::type_complexity)]
fn update_inspect(
    inspect: Res<Inspect>,
    font: Res<UiFont>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    mut panel: Single<&mut Visibility, (With<InspectPanel>, Without<Crosshair>)>,
    header: Single<(&mut Text, &mut TextColor), (With<InspectHeader>, Without<InspectBody>)>,
    mut body: Single<&mut Text, (With<InspectBody>, Without<InspectHeader>)>,
    mut bars: Query<(&Crosshair, &mut Visibility, &mut BackgroundColor), Without<InspectPanel>>,
) {
    let shown = |on: bool| {
        if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    panel.set_if_neq(shown(inspect.on));
    let selected = inspect.selected();
    let color = selected.map_or(Color::WHITE, |s| s.description.class.color());
    for (bar, mut visibility, mut background) in &mut bars {
        visibility.set_if_neq(shown(inspect.on && !inspect.paused));
        if bar.bright {
            background.set_if_neq(BackgroundColor(color));
        }
    }
    if !inspect.on {
        return;
    }
    let captured = cursor.grab_mode != CursorGrabMode::None;
    let (head, text) = match selected {
        _ if inspect.paused => inspect::paused_text(),
        None => inspect::nothing_text(),
        Some(s) => (
            s.panel.0.clone(),
            format!(
                "{}\n{}",
                s.panel.1,
                inspect::footer(inspect.printed(), captured)
            ),
        ),
    };
    let (head, text) = (font.show(&head), font.show(&text));
    let (mut header_text, mut header_color) = header.into_inner();
    if header_text.0 != head {
        header_text.0 = head;
    }
    header_color.set_if_neq(TextColor(color));
    if body.0 != text {
        body.0 = text;
    }
}

/// With `--exit-when-done`: once a replay has finished, take the optional
/// screenshot and exit (code 0 if every checkpoint matched, 1 otherwise).
fn finish(
    mut commands: Commands,
    options: Res<Options>,
    session: Res<Session>,
    travel: Res<Travel>,
    mut state: ResMut<Finish>,
    mut exit: MessageWriter<AppExit>,
) {
    if travel.pending.is_some() {
        return;
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A world built from OpenStreetMap credits it in the help panel, with
    /// its licence and where to read it.
    #[test]
    fn help_credits_openstreetmap_and_its_licence() {
        let world = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world");
        let session = Session::start(&Options {
            world: Some(world),
            ..Options::default()
        })
        .unwrap();
        let travel = Travel {
            view: View::Walk,
            fly_speed: 25.0,
            places: Vec::new(),
            pending: None,
            cut: false,
        };
        let text = help_text(&session, &travel);
        assert!(text.contains("© OpenStreetMap contributors"), "{text}");
        // Inspect mode and its colours.
        assert!(
            text.contains("INSPECT I: what the crosshair is on, who made it")
                && text.contains("amber: an improvable building")
                && text.contains("blue: contributed")
                && text.contains("click: print it"),
            "{text}"
        );
        assert!(
            text.contains("(ODbL)") && text.contains("openstreetmap.org/copyright"),
            "{text}"
        );
    }
}
