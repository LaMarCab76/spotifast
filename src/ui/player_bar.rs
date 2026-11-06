//! The now-playing bar along the bottom of the window.

use egui::{Align, Color32, Frame, Layout, Margin, Rect, Sense, UiBuilder, Vec2, pos2, vec2};

use crate::app::{App, NowPlaying};
use crate::i18n::gettext;
use crate::model::{Action, DragTrack, Page};
use crate::player::RepeatMode;
use crate::theme::{self, Icon};
use crate::util;

use super::widgets::{SliderEvent, thin_slider};

/// Controls keep their IDs between sections, but never across surfaces.
#[derive(Clone, Copy)]
pub(super) enum Surface {
    Bar,
    Fullscreen,
    Panel,
}

impl Surface {
    fn id(self, name: &str) -> egui::Id {
        match self {
            Self::Bar => egui::Id::new(name),
            Self::Fullscreen => egui::Id::new(("fullscreen-player", name)),
            Self::Panel => egui::Id::new(("side-player", name)),
        }
    }
}

pub(crate) fn cancel_gestures(ctx: &egui::Context) {
    ctx.stop_dragging();
    for surface in [Surface::Bar, Surface::Fullscreen, Surface::Panel] {
        for name in ["seek-slider", "volume-slider"] {
            ctx.data_mut(|data| data.remove::<f32>(surface.id(name)));
        }
    }
    close_saved_popups(ctx);
}

pub(crate) fn close_saved_popups(ctx: &egui::Context) {
    for surface in [Surface::Bar, Surface::Fullscreen, Surface::Panel] {
        let id = surface.id(LIKED_POPUP_ID);
        egui::Popup::close_id(ctx, id);
        ctx.data_mut(|data| data.remove::<String>(id.with("query")));
    }
}

/// How much of the playing art's tint the bar's fill carries.
const TINT_STRENGTH: f32 = 0.12;
/// How long the bar takes to cross over to a new song's tint.
const TINT_FADE_SECONDS: f32 = 0.45;
const TINT_SESSION_ID: &str = "player-bar-tint-session";
/// The Add to playlist menu a liked song's heart opens.
const LIKED_POPUP_ID: &str = "player-bar-liked-playlists";
/// How strongly the visualizer shows through behind the controls: the
/// spectrum's bars fade from their foot to their top, with a glow around
/// them, a brighter cap above, and a pulse along the foot with the bass.
const SPECTRUM_ALPHA: (f32, f32) = (0.28, 0.08);
const GLOW_ALPHA: f32 = 0.12;
const PEAK_ALPHA: f32 = 0.7;
const PULSE_ALPHA: f32 = 0.35;
/// The peak cap's thickness, and its gap above the band.
const PEAK_HEIGHT: f32 = 2.0;
const PEAK_GAP: f32 = 2.0;
/// The waveform's line, as layers from the widest glow to the core, the
/// shading under it, and its mirrored echo.
const WAVE_LAYERS: [(f32, f32); 3] = [(9.0, 0.06), (4.0, 0.16), (1.6, 0.85)];
const WAVE_FILL_ALPHA: f32 = 0.14;
const WAVE_ECHO_ALPHA: f32 = 0.18;
/// How much more strongly the visualizer paints over a light bar.
const LIGHT_STRENGTH: f32 = 1.5;
/// The gap between spectrum bars.
const SPECTRUM_GAP: f32 = 2.0;

/// Forget this bar's animation session while the sign-in screen is shown.
pub(crate) fn end_tint_session(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<u64>(egui::Id::new(TINT_SESSION_ID)));
}

/// Full-screen progress has its own row; the normal player keeps its 88 px.
const FULLSCREEN_BAR_HEIGHT: f32 = 112.0;
const STRIP_HEIGHT: f32 = 92.0;
const REVEAL_GRACE: f64 = 0.4;

#[derive(Clone, Copy, Default)]
struct ControlsReveal {
    until: f64,
    dragging: bool,
    shown: bool,
}

fn controls_visible(auto_hide: bool, moving: bool, engaged: bool, grace: bool) -> bool {
    !auto_hide || !moving || engaged || grace
}

fn reveal_controls(app: &App, ctx: &egui::Context, bar: Rect, moving: bool) -> (bool, bool) {
    let id = egui::Id::new("player-bar-reveal");
    let mut state = ctx
        .data(|data| data.get_temp::<ControlsReveal>(id))
        .unwrap_or_default();
    let (time, hovered, pressed, down, keyboard) = ctx.input(|input| {
        (
            input.time,
            input
                .pointer
                .hover_pos()
                .is_some_and(|point| bar.contains(point)),
            input.pointer.any_pressed(),
            input.pointer.any_down(),
            input.key_pressed(egui::Key::Tab),
        )
    });
    if pressed && hovered {
        state.dragging = true;
    }
    state.dragging &= down;
    let focused = ctx
        .memory(|memory| memory.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|response| bar.contains(response.rect.center()));
    let engaged = hovered
        || keyboard
        || focused
        || state.dragging
        || app.seek_preview.is_some()
        || app.show_devices
        || egui::Popup::is_any_open(ctx);
    if engaged {
        state.until = time + REVEAL_GRACE;
    }
    let grace = time < state.until;
    if grace && !engaged {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(state.until - time));
    }
    let visible = controls_visible(app.settings.player_bar_auto_hide, moving, engaged, grace);
    let was_visible = state.shown;
    state.shown = visible;
    ctx.data_mut(|data| data.insert_temp(id, state));
    (visible, was_visible)
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    use crate::settings::{LyricsVisualizer, PlayerBarVis};
    let fullscreen = app.lyrics_fullscreen.is_some() && !app.immersive();
    let surface = if fullscreen {
        Surface::Fullscreen
    } else {
        Surface::Bar
    };
    let strip = fullscreen
        && app.settings.lyrics_presentation.visualizer == LyricsVisualizer::Strip
        && app.settings.player_bar_vis != PlayerBarVis::Off
        && app.visualizers_available();
    let height = if fullscreen {
        FULLSCREEN_BAR_HEIGHT
    } else {
        theme::PLAYER_BAR_HEIGHT
    };
    let palette = app.palette;
    let fill = eased_fill(ui.ctx(), palette.panel, app.now_playing_tint());
    egui::Panel::bottom("player-bar")
        .exact_size(height + if strip { STRIP_HEIGHT } else { 0.0 })
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().inner_margin(Margin::ZERO))
        .show(ui, |ui| {
            let all = ui.max_rect();
            let bar = Rect::from_min_max(pos2(all.left(), all.bottom() - height), all.max);
            ui.painter().rect_filled(bar, 0.0, fill);
            let rect = bar.shrink2(vec2(16.0, 0.0));
            let now = app.now_playing();
            let moving = app.settings.player_bar_vis != PlayerBarVis::Off
                && app.visualizers_available()
                && (app.visualizer_sounding(now.as_ref())
                    || (app.settings.player_bar_vis == PlayerBarVis::Spectrum
                        && !app.player_bar_analyser.settled()));
            let (visible, was_visible) = reveal_controls(app, ui.ctx(), bar, moving);
            let amount = ui.ctx().animate_bool_with_time(
                egui::Id::new("player-bar-controls"),
                visible,
                crate::motion::seconds(ui.ctx(), 0.18),
            );
            let soft = if app.settings.player_bar_auto_hide && moving {
                amount
            } else {
                0.0
            };
            let visualizer_rect = if strip {
                Rect::from_min_max(all.min, pos2(all.right(), bar.top()))
            } else {
                bar
            };
            if visualizer(app, ui, visualizer_rect, now.as_ref(), soft) {
                theme::visualiser_frame(ui.ctx(), app.visualiser_fps());
            }
            // Revealing the controls never changes the visualizer accidentally.
            if app.settings.player_bar_vis_click
                && app.visualizers_available()
                && visible
                && (!app.settings.player_bar_auto_hide || was_visible)
            {
                let cycle_rect = if fullscreen {
                    Rect::from_min_max(bar.min, pos2(bar.right(), bar.bottom() - 30.0))
                } else {
                    bar
                };
                let empty = ui.interact(
                    cycle_rect,
                    ui.id().with("player-bar-visualizer"),
                    Sense::click(),
                );
                let empty = if empty.hovered() {
                    empty.on_hover_text_at_pointer(gettext(
                        app.locale,
                        "Click to change the visualizer",
                    ))
                } else {
                    empty
                };
                if empty.clicked() {
                    app.actions.push(Action::CyclePlayerBarVis);
                }
            }
            ui.painter().hline(
                rect.x_range(),
                bar.top() + 0.5,
                egui::Stroke::new(1.0, palette.outline),
            );
            // Keep widgets, IDs and AccessKit nodes present even while hidden.
            // Pointer entry and Tab reveal them before they can be activated.
            let mut controls =
                ui.new_child(UiBuilder::new().id_salt("player-controls").max_rect(rect));
            controls.multiply_opacity(amount);
            let ui = &mut controls;
            let row = if fullscreen {
                Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 30.0))
            } else {
                rect
            };
            let side = (row.width() * 0.3)
                .clamp(200.0, 420.0)
                .min(((row.width() - transport_width() - 16.0) / 2.0).max(0.0));
            let left = Rect::from_min_max(row.min, pos2(row.left() + side, row.bottom()));
            let center = Rect::from_min_max(
                pos2(row.left() + side, row.top()),
                pos2(row.right() - side, row.bottom()),
            );
            now_playing_block(app, ui, left, now.as_ref(), surface);
            transport(
                app,
                ui,
                now.as_ref(),
                center,
                fullscreen.then_some(rect),
                surface,
            );
            let right_band = Rect::from_min_size(
                pos2(row.right() - side, row.center().y - 15.0),
                vec2(side, 30.0),
            );
            let mut right_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(right_band)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            extras(
                app,
                &mut right_ui,
                now.as_ref(),
                center.center().x + transport_width() / 2.0,
                surface,
            );
        });
}

/// Fixed nine-tap geometric softening. The signal is sampled once and only
/// painting is repeated; no offscreen pass, image allocation or disk work.
fn softened(painter: &egui::Painter, rect: Rect, soft: f32, paint: impl Fn(&egui::Painter, Rect)) {
    if soft < 0.01 {
        paint(painter, rect);
        return;
    }
    for (y, wy) in [(-1.0, 0.25), (0.0, 0.5), (1.0, 0.25)] {
        for (x, wx) in [(-1.0, 0.25), (0.0, 0.5), (1.0, 0.25)] {
            let mut layer = painter.clone();
            layer.multiply_opacity(wx * wy);
            paint(&layer, rect.translate(vec2(x, y) * (5.0 * soft)));
        }
    }
}

/// Draws the chosen spectrum or waveform of the song playing on this
/// computer across `rect`, in colours drawn from the cover. It reads the
/// same post-equalizer, pre-volume sound as the mini player's visualizer,
/// so the volume never moves it. Returns whether it is still moving.
fn visualizer(
    app: &mut App,
    ui: &egui::Ui,
    rect: Rect,
    now: Option<&NowPlaying>,
    soft: f32,
) -> bool {
    use crate::settings::PlayerBarVis;
    use crate::vis;
    let mode = app.settings.player_bar_vis;
    let sounding = app.visualizer_sounding(now);
    let dark = ui.visuals().dark_mode;
    let colours = vis_colours(app.now_playing_palette(), app.palette.accent, dark);
    // Over a light bar the same colour paints more strongly, or white
    // would wash it out.
    let strength = if dark { 1.0 } else { LIGHT_STRENGTH };
    let painter = ui.painter().with_clip_rect(rect);
    match mode {
        PlayerBarVis::Off => false,
        PlayerBarVis::Spectrum => {
            if !sounding && app.player_bar_analyser.settled() {
                return false;
            }
            let samples = if sounding {
                app.winamp
                    .tap
                    .window(vis::WIDE_SAMPLES, app.visualizer_lag())
            } else {
                Vec::new()
            };
            let levels = app
                .player_bar_analyser
                .step(&samples, std::time::Instant::now());
            let peaks = app.player_bar_analyser.peaks();
            softened(&painter, rect, soft, |painter, rect| {
                spectrum(painter, rect, &levels, &peaks, &colours, strength);
            });
            sounding || !app.player_bar_analyser.settled()
        }
        PlayerBarVis::Waveform => {
            if !sounding {
                return false;
            }
            // Winamp's scope with a column every eight points or so.
            let count = (rect.width() / 8.0).clamp(75.0, 320.0) as usize;
            let samples = app
                .winamp
                .tap
                .window(count * vis::SCOPE_STEP, app.visualizer_lag());
            let trace = vis::scope_line(&samples, count);
            softened(&painter, rect, soft, |painter, rect| {
                waveform(painter, rect, &trace, &colours, strength);
            });
            true
        }
    }
}

/// The visualizer's colours, swept across from the bass to the treble. With
/// colour from album art they are the cover's own: its hues in the cover's
/// order, then its grey, drawn as a plain grey even when it is a near-black
/// with a cast, so the bass and its pulse carry a hue whenever the cover has
/// one. Otherwise they are the theme's accent for the bass, and the same
/// turned a sixth of the way round the colour wheel for the treble. Each hue
/// is made vivid, then all are kept to a band of
/// brightness, so the words the bars pass behind always stand out: no lighter
/// than [`DARK_CEILING`] under the dark theme's white text, no darker than
/// [`LIGHT_FLOOR`] under the light theme's dark text.
fn vis_colours(art: Option<&[[u8; 3]]>, accent: Color32, dark: bool) -> Vec<Color32> {
    use crate::images::has_hue;
    use egui::ecolor::Hsva;
    let keep = |mut colour: Hsva, vivid: bool| {
        colour.s = if vivid { colour.s.max(0.55) } else { 0.0 };
        colour.v = 1.0;
        colour.a = 1.0;
        within_brightness(Color32::from(colour), dark)
    };
    if let Some(art) = art.filter(|art| !art.is_empty()) {
        let colours = |vivid: bool| {
            art.iter()
                .filter(move |&&rgb| has_hue(rgb) == vivid)
                .map(move |&rgb| keep(Hsva::from_srgb(rgb), vivid))
        };
        return colours(true).chain(colours(false)).collect();
    }
    let low = Hsva::from(accent);
    let mut high = low;
    high.h = (high.h + 1.0 / 6.0).fract();
    vec![keep(low, true), keep(high, true)]
}

/// The colour `at` the given fraction of the way along `colours`, from the
/// first at 0 to the last at 1.
fn swept(colours: &[Color32], at: f32) -> Color32 {
    let Some(last) = colours.len().checked_sub(1) else {
        return Color32::TRANSPARENT;
    };
    if last == 0 {
        return colours[0];
    }
    let place = at.clamp(0.0, 1.0) * last as f32;
    let index = (place as usize).min(last - 1);
    colours[index].lerp_to_gamma(colours[index + 1], place - index as f32)
}

/// The most luminance a visualizer colour has under the dark theme, and the
/// band it is kept in under the light theme, as relative luminance from 0
/// to 1. On white the colour must be dark enough to show at all yet light
/// enough that dark words stay clear over it.
const DARK_CEILING: f32 = 0.35;
const LIGHT_FLOOR: f32 = 0.2;
const LIGHT_CEILING: f32 = 0.4;

/// `colour` with its hue kept and its brightness moved into the band the
/// theme's text reads against: darkened towards black when too light,
/// lightened towards white when too dark.
fn within_brightness(colour: Color32, dark: bool) -> Color32 {
    let linear = egui::Rgba::from(colour);
    let luminance = 0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b();
    let ceiling = if dark { DARK_CEILING } else { LIGHT_CEILING };
    let adjusted = if luminance > ceiling {
        linear * (ceiling / luminance)
    } else if !dark && luminance < LIGHT_FLOOR {
        let toward_white = (LIGHT_FLOOR - luminance) / (1.0 - luminance).max(f32::EPSILON);
        egui::Rgba::from_rgb(
            linear.r() + (1.0 - linear.r()) * toward_white,
            linear.g() + (1.0 - linear.g()) * toward_white,
            linear.b() + (1.0 - linear.b()) * toward_white,
        )
    } else {
        linear
    };
    Color32::from(egui::Rgba::from_rgb(
        adjusted.r(),
        adjusted.g(),
        adjusted.b(),
    ))
}

/// Adds a rectangle shaded from `top` to `bottom`.
fn shaded(mesh: &mut egui::Mesh, rect: Rect, top: Color32, bottom: Color32) {
    let base = mesh.vertices.len() as u32;
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

/// Bars rising from the foot of the player bar, swept across the
/// visualizer's colours from the bass to the treble, with a soft glow, a
/// peak cap hanging above each, and the whole foot of the bar breathing with
/// the bass.
fn spectrum(
    painter: &egui::Painter,
    rect: Rect,
    levels: &[f32],
    peaks: &[f32],
    colours: &[Color32],
    strength: f32,
) {
    let bands = levels.len() as f32;
    let width = (rect.width() - SPECTRUM_GAP * (bands - 1.0)) / bands;
    // A full band reaches the top, with room above for its cap.
    let reach = rect.height() - PEAK_GAP - PEAK_HEIGHT - 1.0;
    let mut mesh = egui::Mesh::default();

    // The bass pulse: a glow along the foot, as strong as the low bands.
    let bass = levels.iter().take(levels.len() / 8).copied().sum::<f32>()
        / (levels.len() / 8).max(1) as f32;
    let pulse = Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.right_bottom());
    shaded(
        &mut mesh,
        pulse,
        Color32::TRANSPARENT,
        swept(colours, 0.0).gamma_multiply(PULSE_ALPHA * strength * bass * bass),
    );

    for (index, (level, peak)) in levels.iter().zip(peaks).enumerate() {
        let left = rect.left() + index as f32 * (width + SPECTRUM_GAP);
        let colour = swept(colours, index as f32 / (bands - 1.0));
        let height = level * reach;
        if height >= 1.0 {
            let bar = Rect::from_min_max(
                pos2(left, rect.bottom() - height),
                pos2(left + width, rect.bottom()),
            );
            // The glow: the bar again, wider and fainter.
            shaded(
                &mut mesh,
                bar.expand2(vec2(SPECTRUM_GAP, 3.0)),
                colour.gamma_multiply(GLOW_ALPHA * strength * 0.3),
                colour.gamma_multiply(GLOW_ALPHA * strength),
            );
            let (foot, top) = (SPECTRUM_ALPHA.0 * strength, SPECTRUM_ALPHA.1 * strength);
            shaded(
                &mut mesh,
                bar,
                colour.gamma_multiply(foot + (top - foot) * level),
                colour.gamma_multiply(foot),
            );
        }
        let cap = peak * reach;
        if cap >= 2.0 {
            let y = rect.bottom() - cap - PEAK_GAP;
            shaded(
                &mut mesh,
                Rect::from_min_max(pos2(left, y - PEAK_HEIGHT), pos2(left + width, y)),
                colour.gamma_multiply(PEAK_ALPHA),
                colour.gamma_multiply(PEAK_ALPHA),
            );
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The wave as a neon line in layers of glow, swept across the visualizer's
/// colours from the bass to the treble, over a faint shading down to the
/// midline and a dim mirrored echo.
fn waveform(
    painter: &egui::Painter,
    rect: Rect,
    trace: &[f32],
    colours: &[Color32],
    strength: f32,
) {
    if trace.len() < 2 {
        return;
    }
    let reach = rect.height() * 0.46;
    let step = rect.width() / (trace.len() - 1) as f32;
    let midline = rect.center().y;
    let point =
        |index: usize, value: f32| pos2(rect.left() + index as f32 * step, midline - value * reach);
    let colour_at = |index: usize| swept(colours, index as f32 / (trace.len() - 1) as f32);

    // The shading between the wave and the midline.
    let mut mesh = egui::Mesh::default();
    for index in 0..trace.len() - 1 {
        let colour = colour_at(index).gamma_multiply(WAVE_FILL_ALPHA * strength);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(point(index, trace[index]), colour);
        mesh.colored_vertex(point(index + 1, trace[index + 1]), colour);
        mesh.colored_vertex(pos2(point(index + 1, 0.0).x, midline), Color32::TRANSPARENT);
        mesh.colored_vertex(pos2(point(index, 0.0).x, midline), Color32::TRANSPARENT);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));

    // Short runs of line, so the colour can sweep along the wave.
    let run = 8;
    let stroke = |mirror: f32, width: f32, alpha: f32| {
        let mut start = 0;
        while start < trace.len() - 1 {
            let end = (start + run).min(trace.len() - 1);
            let points = (start..=end)
                .map(|index| point(index, trace[index] * mirror))
                .collect();
            let colour = colour_at((start + end) / 2).gamma_multiply(alpha);
            painter.add(egui::Shape::line(points, egui::Stroke::new(width, colour)));
            start = end;
        }
    };
    stroke(-0.6, 1.0, WAVE_ECHO_ALPHA);
    for (width, alpha) in WAVE_LAYERS {
        stroke(1.0, width, alpha);
    }
}

/// Ease the final fill's RGB. Untinted custom panels keep their alpha while
/// the colour returns to the panel colour.
pub(super) fn eased_fill(ctx: &egui::Context, panel: Color32, tint: Option<Color32>) -> Color32 {
    let target = tint.map_or(panel, |tint| super::blend(panel, tint, TINT_STRENGTH));
    // Color32 stores premultiplied RGB, so interpolate unmultiplied channels.
    let [r, g, b, _] = target.to_srgba_unmultiplied();
    // A new pass after sign-out gets new ids without clearing other animations.
    let pass = ctx.cumulative_pass_nr();
    let session = ctx.data_mut(|data| {
        *data.get_temp_mut_or_insert_with(egui::Id::new(TINT_SESSION_ID), || pass)
    });
    let channel = |axis: &'static str, value: u8| {
        ctx.animate_value_with_time(
            egui::Id::new(("player-bar-tint", session, axis)),
            f32::from(value),
            crate::motion::seconds(ctx, TINT_FADE_SECONDS),
        )
        .round() as u8
    };
    let eased = [channel("r", r), channel("g", g), channel("b", b)];
    if eased == [r, g, b] {
        target
    } else {
        Color32::from_rgba_unmultiplied(eased[0], eased[1], eased[2], target.a())
    }
}

pub(super) fn now_playing_block(
    app: &mut App,
    ui: &mut egui::Ui,
    region: Rect,
    now: Option<&NowPlaying>,
    surface: Surface,
) {
    let palette = app.palette;
    let cy = region.center().y;
    let cover_rect = Rect::from_min_size(pos2(region.left() + 4.0, cy - 28.0), Vec2::splat(56.0));

    let Some(now) = now else {
        super::widgets::paint_cover(ui, &palette, None, cover_rect, 6.0, Icon::Music, None);
        let text_left = cover_rect.right() + 12.0;
        let text_rect = Rect::from_min_size(
            pos2(text_left, cy - 17.0),
            vec2((region.right() - text_left - 8.0).max(40.0), 34.0),
        );
        let mut text_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        text_ui.spacing_mut().item_spacing.y = 2.0;
        theme::text(
            &mut text_ui,
            gettext(app.locale, "Nothing playing"),
            theme::medium(14.0),
            palette.secondary,
        );
        theme::text(
            &mut text_ui,
            gettext(app.locale, "Pick a song, album, or playlist"),
            theme::regular(12.0),
            palette.dim,
        );
        return;
    };

    super::widgets::playing_cover(
        ui,
        &palette,
        app.backend.art(),
        "player-bar",
        app.small_art(now, cover_rect.width()),
        cover_rect,
        6.0,
    );
    let song = app.now_playing_item();
    let drag_sense = if song.is_some() {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let cover_response = ui
        .interact(cover_rect, surface.id("now-playing-cover"), drag_sense)
        .on_hover_cursor(if song.is_some() {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Default
        });
    // Hovering the cover offers to dock the art large at the sidebar's
    // bottom, the way Spotify expands it. (#92)
    let art_available = now.art_url.is_some() || now.art_small.is_some();
    let expand_rect = Rect::from_center_size(
        pos2(cover_rect.right() - 10.0, cover_rect.top() + 10.0),
        Vec2::splat(18.0),
    );
    let offer_expand = art_available
        && !app.settings.art_expanded
        && app.settings.sidebar_visible
        && !app.show_now_playing_panel;
    let over_expand = offer_expand && ui.rect_contains_pointer(expand_rect);
    if cover_response.clicked() && !over_expand {
        if let Some(id) = &now.album_id {
            app.actions.push(Action::Open(Page::Album(id.clone())));
        } else if let Some(id) = &now.show_id {
            app.actions.push(Action::Open(Page::Show(id.clone())));
        }
    }
    if offer_expand && (cover_response.hovered() || over_expand) {
        let expand = ui.interact(
            expand_rect,
            egui::Id::new("now-playing-art-expand"),
            Sense::click(),
        );
        ui.painter()
            .circle_filled(expand_rect.center(), 9.0, palette.panel.gamma_multiply(0.9));
        Icon::ChevronUp.image(palette.text, 12.0).paint_at(
            ui,
            Rect::from_center_size(expand_rect.center(), Vec2::splat(12.0)),
        );
        if expand.clicked() {
            app.settings.art_expanded = true;
            app.actions.push(Action::SettingsChanged);
        }
    }
    let heart_width = if now.is_episode { 0.0 } else { 42.0 };
    let text_left = cover_rect.right() + 12.0;
    let text_width = (region.right() - text_left - heart_width).max(40.0);
    let displayed_text_width: f32 = {
        let title =
            ui.painter()
                .layout_no_wrap(now.title.clone(), theme::medium(14.0), palette.text);
        let subtitle = ui.painter().layout_no_wrap(
            now.subtitle.clone(),
            theme::regular(12.0),
            palette.secondary,
        );
        title.size().x.max(subtitle.size().x).min(text_width)
    };
    let info_rect = Rect::from_min_size(pos2(text_left, cy - 18.0), vec2(text_width, 36.0));

    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(info_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    text_ui.set_clip_rect(info_rect.intersect(ui.clip_rect()));
    text_ui.spacing_mut().item_spacing.y = 2.0;
    let title_response = theme::link(&mut text_ui, &now.title, theme::medium(14.0), palette.text)
        .interact(drag_sense);
    if title_response.clicked() {
        if let Some(id) = &now.album_id {
            app.actions.push(Action::Open(Page::Album(id.clone())));
        } else if let Some(id) = &now.show_id {
            app.actions.push(Action::Open(Page::Show(id.clone())));
        }
    }
    let mut responses = vec![cover_response, title_response];
    text_ui.horizontal_top(|ui| {
        if now.artists.is_empty() {
            let response = theme::link(ui, &now.subtitle, theme::regular(12.0), palette.secondary)
                .interact(drag_sense);
            if response.clicked()
                && let Some(id) = &now.show_id
            {
                app.actions.push(Action::Open(Page::Show(id.clone())));
            }
            responses.push(response);
        } else {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, artist) in now.artists.iter().enumerate() {
                if index > 0 {
                    theme::text(ui, ", ", theme::regular(12.0), palette.secondary);
                }
                let response = if artist.id.is_some() {
                    theme::link(ui, &artist.name, theme::regular(12.0), palette.secondary)
                } else {
                    theme::text(ui, &artist.name, theme::regular(12.0), palette.secondary)
                }
                .interact(drag_sense);
                if response.clicked()
                    && let Some(id) = &artist.id
                {
                    app.actions.push(Action::Open(Page::Artist(id.clone())));
                }
                responses.push(response);
            }
        }
    });
    if responses
        .iter()
        .any(|response| response.drag_started_by(egui::PointerButton::Primary))
        && let Some(item) = &song
    {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            DragTrack {
                title: item.name().to_string(),
                image: app.item_image(item, 64),
                items: vec![item.clone()],
                from: None,
                source_playlist: None,
            },
        );
    }

    // The playing thing answers the same right-click menu as a table row,
    // from the cover or the words.
    if let Some(item) = &song {
        let context = app.editable_context_playlist();
        for response in &responses {
            egui::Popup::context_menu(response)
                .frame(super::widgets::menu_frame(&palette))
                .show(|ui| super::widgets::item_menu(ui, app, item, context.as_ref(), None));
        }
    }

    if !now.is_episode {
        // Sit the heart just past the actual text, not at the region's far
        // edge, so it stays visually attached to the title.
        let heart_x = (text_left + displayed_text_width + 21.0).min(region.right() - 21.0);
        let heart_rect = Rect::from_center_size(pos2(heart_x, cy), Vec2::splat(30.0));
        let mut heart_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(heart_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        saved_button(app, &mut heart_ui, now, song.as_ref(), surface);
    }
}

pub(super) fn saved_button(
    app: &mut App,
    ui: &mut egui::Ui,
    now: &NowPlaying,
    song: Option<&crate::api::models::PlayableItem>,
    surface: Surface,
) {
    let palette = app.palette;
    let saved = app.is_saved(&now.uri).unwrap_or(false);
    // A liked heart stays green under the pointer, like shuffle.
    let (icon, color, hover, tooltip) = if saved {
        (
            Icon::HeartFilled,
            palette.accent,
            palette.accent_hover,
            gettext(app.locale, "Add to playlist"),
        )
    } else {
        (
            Icon::Heart,
            palette.secondary,
            palette.text,
            gettext(app.locale, "Save to Liked Songs"),
        )
    };
    let response = theme::icon_button(ui, icon, 17.0, color, hover, &tooltip);
    let popup_id = surface.id(LIKED_POPUP_ID);
    // A liked song's heart opens Add to playlist, as Spotify's does,
    // with Remove from Liked Songs below the playlists (#792). The first
    // click on a song not yet liked still likes it at once.
    match song.filter(|_| saved) {
        None => {
            egui::Popup::close_id(ui.ctx(), popup_id);
            if response.clicked() {
                app.actions.push(Action::ToggleSaved(now.uri.clone()));
            }
        }
        Some(item) => {
            let query_id = popup_id.with("query");
            let shown = egui::Popup::menu(&response)
                .id(popup_id)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .frame(super::widgets::menu_frame(&palette))
                .show(|ui| {
                    let stored = ui.data(|data| data.get_temp::<String>(query_id));
                    let fresh = stored.is_none();
                    let mut query = stored.unwrap_or_default();
                    let before = app.actions.len();
                    let field = super::widgets::playlist_picker(
                        ui,
                        app,
                        std::slice::from_ref(item),
                        &mut query,
                    );
                    if fresh {
                        field.request_focus();
                    }
                    ui.data_mut(|data| data.insert_temp(query_id, query));
                    super::widgets::menu_separator(ui, &palette);
                    if super::widgets::menu_item(
                        ui,
                        &palette,
                        Some(Icon::HeartFilled),
                        &gettext(app.locale, "Remove from Liked Songs"),
                    ) {
                        app.actions.push(Action::ToggleSaved(now.uri.clone()));
                    }
                    // A playlist, New playlist, or Remove closes it.
                    if app.actions.len() > before {
                        ui.close();
                    }
                });
            if shown.is_none() {
                ui.data_mut(|data| data.remove::<String>(query_id));
            }
        }
    }
}

/// The transport's button widths: icon buttons occupy icon size + 12; the
/// disc is 36.
const TRANSPORT_WIDTHS: [f32; 5] = [29.0, 30.0, 36.0, 30.0, 29.0];
const TRANSPORT_GAP: f32 = 10.0;

/// The transport's buttons from Shuffle to Repeat, centred in the bar.
fn transport_width() -> f32 {
    TRANSPORT_WIDTHS.iter().sum::<f32>() + TRANSPORT_GAP * 4.0
}

fn transport(
    app: &mut App,
    ui: &mut egui::Ui,
    now: Option<&NowPlaying>,
    region: Rect,
    fullscreen: Option<Rect>,
    surface: Surface,
) {
    let palette = app.palette;
    // Everything here is placed with explicit rects: egui's implicit rows
    // centre each widget in the row height known when it is added, which
    // left earlier icons riding high next to the play disc.
    //
    // The buttons row (36) and the progress row (~15, after a 6px gap) form
    // one cluster, centred as a group in the 88px bar: the buttons sit 8px
    // above the bar's midline and the progress row 23px below it. Measured
    // on screen this puts equal breathing room above and beneath the
    // cluster.
    let cy = region.center().y - 8.0;
    let enabled = now.is_some_and(|now| now.can_control) || app.is_connected();
    let playing = now.is_some_and(|now| now.playing);
    let loading = now.is_some_and(|now| now.loading);
    let shuffle = now.map_or_else(|| app.playing_context_shuffle(), |now| now.shuffle);
    let repeat = now.map(|now| now.repeat).unwrap_or_default();
    let dim = if enabled {
        palette.secondary
    } else {
        palette.dim
    };

    let widths = TRANSPORT_WIDTHS;
    let mut x = region.center().x - transport_width() / 2.0;
    let mut slot = |width: f32| {
        let rect = Rect::from_center_size(pos2(x + width / 2.0, cy), vec2(width, 36.0));
        x += width + TRANSPORT_GAP;
        rect
    };
    let centered = |ui: &mut egui::Ui, rect: Rect| {
        ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        )
    };

    let shuffle_color = if shuffle { palette.accent } else { dim };
    let mut cell = centered(ui, slot(widths[0]));
    let shuffle_button = theme::icon_button(
        &mut cell,
        Icon::Shuffle,
        17.0,
        shuffle_color,
        if shuffle {
            palette.accent_hover
        } else {
            palette.text
        },
        &gettext(app.locale, "Shuffle"),
    );
    shuffle_button.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            cell.is_enabled(),
            shuffle,
            gettext(app.locale, "Shuffle"),
        )
    });
    if shuffle_button.clicked() {
        app.actions.push(Action::ToggleShuffle);
    }

    let mut cell = centered(ui, slot(widths[1]));
    if theme::icon_button(
        &mut cell,
        Icon::SkipBackFilled,
        18.0,
        dim,
        palette.text,
        &gettext(app.locale, "Previous"),
    )
    .clicked()
    {
        app.actions.push(Action::Previous);
    }

    let disc = slot(widths[2]);
    if loading || app.any_play_pending() {
        let mut cell = centered(ui, disc);
        theme::circle_spinner(
            &mut cell,
            36.0,
            palette.text,
            palette.window,
            &gettext(app.locale, "Loading…"),
        );
    } else {
        let icon = if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        };
        let hover = if palette.dark {
            egui::Color32::WHITE
        } else {
            palette.text
        };
        let mut cell = centered(ui, disc);
        if theme::circle_button(
            &mut cell,
            icon,
            36.0,
            palette.text,
            hover,
            palette.window,
            &if playing {
                gettext(app.locale, "Pause")
            } else {
                gettext(app.locale, "Play")
            },
        )
        .clicked()
        {
            app.actions.push(Action::TogglePlay);
        }
    }

    let mut cell = centered(ui, slot(widths[3]));
    if theme::icon_button(
        &mut cell,
        Icon::SkipForwardFilled,
        18.0,
        dim,
        palette.text,
        &gettext(app.locale, "Next"),
    )
    .clicked()
    {
        app.actions.push(Action::Next);
    }

    let (repeat_icon, repeat_color, tooltip) = match repeat {
        RepeatMode::Off => (Icon::Repeat, dim, gettext(app.locale, "Repeat")),
        RepeatMode::Context => (
            Icon::Repeat,
            palette.accent,
            gettext(app.locale, "Repeat one"),
        ),
        RepeatMode::Track => (
            Icon::Repeat1,
            palette.accent,
            gettext(app.locale, "Repeat off"),
        ),
    };
    let mut cell = centered(ui, slot(widths[4]));
    if theme::icon_button(
        &mut cell,
        repeat_icon,
        17.0,
        repeat_color,
        if repeat == RepeatMode::Off {
            palette.text
        } else {
            palette.accent_hover
        },
        &tooltip,
    )
    .clicked()
    {
        app.actions.push(Action::CycleRepeat);
    }

    progress(app, ui, now, region, cy + 31.0, fullscreen, surface);
}

fn progress(
    app: &mut App,
    ui: &mut egui::Ui,
    now: Option<&NowPlaying>,
    region: Rect,
    row_cy: f32,
    fullscreen: Option<Rect>,
    surface: Surface,
) {
    let palette = app.palette;
    let row_cy = fullscreen.map_or(row_cy, |rect| rect.bottom() - 15.0);
    let (position, duration) = now
        .map(|now| (now.position_ms, now.duration_ms))
        .unwrap_or((0, 0));
    let shown_position = match app.seek_preview {
        Some(fraction) => (fraction * duration as f32) as u32,
        None => position,
    };
    let time_color = if now.is_some() {
        palette.secondary
    } else {
        palette.dim
    };
    let font = theme::regular(11.5);
    let (left, right) = (
        util::format_duration_ms(shown_position),
        util::format_duration_ms(duration),
    );
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), time_color)
            .size()
            .x
    };
    let (slider_left, slider_width) = if let Some(rect) = fullscreen {
        let left_width = measure(&left) + 12.0;
        let right_width = measure(&right) + 12.0;
        (
            rect.left() + left_width,
            (rect.width() - left_width - right_width).max(1.0),
        )
    } else {
        let width = (region.width() - 120.0).clamp(120.0, 620.0);
        (region.center().x - width / 2.0, width)
    };
    ui.painter().text(
        pos2(slider_left - 8.0, row_cy),
        egui::Align2::RIGHT_CENTER,
        util::format_duration_ms(shown_position),
        theme::regular(11.5),
        time_color,
    );
    let slider_rect = Rect::from_center_size(
        pos2(slider_left + slider_width / 2.0, row_cy),
        vec2(slider_width, 24.0),
    );
    let mut slider_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(slider_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let fraction = if duration > 0 {
        position as f32 / duration as f32
    } else {
        0.0
    };
    match thin_slider(
        &mut slider_ui,
        &palette,
        surface.id("seek-slider"),
        &gettext(app.locale, "Playback position (%)"),
        fraction,
        slider_width,
        None,
    ) {
        SliderEvent::Dragging(value) => app.seek_preview = Some(value),
        SliderEvent::Committed(value) => {
            app.seek_preview = None;
            if duration > 0 {
                app.actions
                    .push(Action::Seek((value * duration as f32) as u32));
            }
        }
        SliderEvent::None => {}
    }
    ui.painter().text(
        pos2(slider_left + slider_width + 8.0, row_cy),
        egui::Align2::LEFT_CENTER,
        util::format_duration_ms(duration),
        theme::regular(11.5),
        time_color,
    );
}

/// What the right of the bar always holds, in points: the volume slider and
/// its button, the devices, queue and lyrics buttons, and the gaps.
const EXTRAS_WIDTH: f32 = 92.0 + 4.0 * (18.0 + theme::ICON_BUTTON_PAD) + 4.0 * 6.0 + 4.0;

/// Whether the full screen button has room beside the rest, keeping clear of
/// the transport, with `room` points to the right of it.
fn full_screen_button_fits(room: f32) -> bool {
    room >= EXTRAS_WIDTH + 2.0 * (18.0 + theme::ICON_BUTTON_PAD + 6.0) + TRANSPORT_GAP
}

fn extras(
    app: &mut App,
    ui: &mut egui::Ui,
    now: Option<&NowPlaying>,
    transport_right: f32,
    surface: Surface,
) {
    let palette = app.palette;
    ui.spacing_mut().item_spacing.x = 6.0;
    // The bar's last control, right of the volume, like Spotify's. It gives
    // way before anything else, once Listening activity has; **F** stays.
    let room = ui.max_rect().right() - transport_right;
    if full_screen_button_fits(room) {
        let inside = app.lyrics_fullscreen.is_some();
        let (icon, tip) = if inside {
            (Icon::Shrink, gettext(app.locale, "Leave full screen (Esc)"))
        } else {
            (Icon::Expand, gettext(app.locale, "Full screen lyrics"))
        };
        if theme::icon_button(ui, icon, 18.0, palette.secondary, palette.text, &tip).clicked() {
            app.actions.push(Action::SetLyricsFullscreen(!inside));
        }
    }
    // Keep Now playing reachable at the window's minimum width.
    let slider_width = (room - (EXTRAS_WIDTH - 92.0) - 46.0).clamp(48.0, 92.0);
    volume_controls(app, ui, now, slider_width, surface);
    ui.add_space(4.0);
    devices_button(app, ui, now);
    let queue_open = app.show_queue_panel || matches!(app.page(), Page::Queue);
    let queue_button = theme::icon_button(
        ui,
        Icon::ListVideo,
        18.0,
        if queue_open {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Queue"),
    );
    if queue_button.clicked() {
        app.actions.push(Action::ToggleQueuePanel);
    }
    // A dragged song dropped on the queue button queues it, same as the
    // "Add to queue" menu item.
    if let Some(track) = queue_button.dnd_release_payload::<DragTrack>() {
        app.actions.push(Action::QueueMany {
            songs: track
                .items
                .iter()
                .map(|item| (item.uri().to_string(), item.name().to_string()))
                .collect(),
        });
    }
    if theme::icon_button(
        ui,
        Icon::Mic,
        18.0,
        if app.show_lyrics_panel {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Lyrics"),
    )
    .clicked()
    {
        app.actions.push(Action::ToggleLyricsPanel);
    }
    // Now playing stays reachable at the minimum width. Listening activity
    // follows only once fullscreen and the transport have their room.
    let button = 18.0 + theme::ICON_BUTTON_PAD;
    let left = ui.min_rect().left() - ui.spacing().item_spacing.x - button;
    if left - transport_right >= TRANSPORT_GAP
        && theme::icon_button(
            ui,
            Icon::SquarePlay,
            18.0,
            if app.show_now_playing_panel {
                palette.accent
            } else {
                palette.secondary
            },
            palette.text,
            &gettext(app.locale, "Now playing"),
        )
        .clicked()
    {
        app.actions.push(Action::ToggleNowPlayingPanel);
    }
    let left = ui.min_rect().left() - ui.spacing().item_spacing.x - button;
    if full_screen_button_fits(room)
        && left - transport_right >= TRANSPORT_GAP
        && theme::icon_button(
            ui,
            Icon::Users,
            18.0,
            if app.show_friends_panel {
                palette.accent
            } else {
                palette.secondary
            },
            palette.text,
            &gettext(app.locale, "Listening activity"),
        )
        .clicked()
    {
        app.actions.push(Action::ToggleFriendsPanel);
    }
}

pub(super) fn volume_controls(
    app: &mut App,
    ui: &mut egui::Ui,
    now: Option<&NowPlaying>,
    width: f32,
    surface: Surface,
) {
    let palette = app.palette;
    let volume = now
        .map(|now| now.volume_percent)
        .unwrap_or_else(|| crate::app::volume_to_percent(app.local.volume));
    let shown = match app.volume_preview {
        Some(fraction) => (fraction * 100.0).round() as u8,
        None => volume,
    };
    // Some remote devices, such as a phone playing over Bluetooth, refuse
    // volume changes from other apps: show their volume but don't offer to
    // change it.
    let adjustable = !app.soloist_waiting() && now.is_none_or(|now| now.can_set_volume);
    let controls = ui.add_enabled_ui(adjustable, |ui| {
        match thin_slider(
            ui,
            &palette,
            surface.id("volume-slider"),
            &gettext(app.locale, "Volume (%)"),
            shown as f32 / 100.0,
            width,
            Some(0.05),
        ) {
            SliderEvent::Dragging(value) => {
                app.volume_preview = Some(value);
                app.actions
                    .push(Action::PreviewVolume((value * 100.0).round() as u8));
            }
            SliderEvent::Committed(value) => {
                app.volume_preview = None;
                app.actions
                    .push(Action::SetVolume((value * 100.0).round() as u8));
            }
            SliderEvent::None => {}
        }
        let volume_icon = match shown {
            0 => Icon::VolumeX,
            1..=33 => Icon::Volume,
            34..=66 => Icon::Volume1,
            _ => Icon::Volume2,
        };
        if theme::icon_button(
            ui,
            volume_icon,
            18.0,
            palette.secondary,
            palette.text,
            &if shown == 0 {
                gettext(app.locale, "Unmute")
            } else {
                gettext(app.locale, "Mute")
            },
        )
        .clicked()
        {
            app.actions.push(Action::ToggleMute);
        }
    });
    if !adjustable {
        ui.interact(
            controls.response.rect,
            surface.id("volume-fixed"),
            egui::Sense::hover(),
        )
        .on_hover_text(gettext(
            app.locale,
            "This device's volume can't be changed from Spotifast",
        ));
    }
}

pub(super) fn devices_button(app: &mut App, ui: &mut egui::Ui, now: Option<&NowPlaying>) {
    let palette = app.palette;
    let remote = now.is_some_and(|now| !now.local) && !app.targets_own_soloist();
    // Playing elsewhere, the button shows that device's own icon.
    let icon = now
        .filter(|_| remote)
        .and_then(|now| now.device_kind.as_deref())
        .map_or(Icon::Speaker, super::devices::device_icon);
    let devices = theme::icon_button(
        ui,
        icon,
        18.0,
        if remote {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Connect to a device"),
    );
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new(super::devices::BUTTON_RECT_ID), devices.rect)
    });
    if devices.clicked() {
        app.actions.push(Action::ToggleDevicesPopup);
    }
}

/// The panel keeps transport and progress on a stable fixed footer.
pub(super) fn panel_transport(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let now = app.now_playing();
    transport(app, ui, now.as_ref(), rect, Some(rect), Surface::Panel);
}

/// Draw only the visible visualizer surface, using the bar's analyzer and settings.
pub(crate) fn panel_visualizer(app: &mut App, ui: &egui::Ui, rect: Rect) {
    if !app.visualizers_available() {
        return;
    }
    let now = app.now_playing();
    if visualizer(app, ui, rect, now.as_ref(), 0.0) {
        theme::visualiser_frame(ui.ctx(), app.visualiser_fps());
    }
    if app.settings.player_bar_vis_click {
        let response = ui
            .interact(
                rect,
                egui::Id::new("side-player-visualizer"),
                Sense::click(),
            )
            .on_hover_text(gettext(app.locale, "Click to change the visualizer"));
        if response.clicked() {
            app.actions.push(Action::CyclePlayerBarVis);
        }
    }
}

#[cfg(test)]
mod player_bar_tint_tests {
    use super::*;
    use crate::theme::Palette;

    /// Run one frame at `time` and report the bar's actual fill.
    fn frame(ctx: &egui::Context, time: f64, panel: Color32, tint: Option<Color32>) -> Color32 {
        let mut fill = Color32::PLACEHOLDER;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                ..Default::default()
            },
            |ui| fill = eased_fill(ui.ctx(), panel, tint),
        );
        output.textures_delta.clear();
        fill
    }

    #[test]
    fn a_song_without_art_preserves_translucent_panel() {
        for panel in [
            Palette::dark().panel,
            Palette::light().panel,
            Color32::from_rgba_unmultiplied(80, 120, 180, 128),
        ] {
            let ctx = egui::Context::default();
            assert_eq!(frame(&ctx, 0.0, panel, None), panel);
            let art = Color32::from_rgb(200, 40, 90);
            frame(&ctx, 0.1, panel, Some(art));
            frame(&ctx, 0.6, panel, Some(art));
            assert_eq!(frame(&ctx, 0.7, panel, None).a(), panel.a());
            assert_eq!(frame(&ctx, 1.2, panel, None), panel);
        }
    }

    #[test]
    fn the_first_frame_shows_the_tint_without_fading_in() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let art = Color32::from_rgb(200, 40, 90);
        assert_eq!(
            frame(&ctx, 0.0, panel, Some(art)),
            super::super::blend(panel, art, TINT_STRENGTH)
        );
    }

    #[test]
    fn first_art_after_an_untinted_frame_fades_in() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let art = Color32::from_rgb(220, 140, 160);
        let target = super::super::blend(panel, art, TINT_STRENGTH);
        let fade = f64::from(TINT_FADE_SECONDS);

        assert_eq!(frame(&ctx, 0.0, panel, None), panel);
        assert_eq!(frame(&ctx, 0.1, panel, Some(art)), panel);
        let middle = frame(&ctx, 0.1 + fade / 2.0, panel, Some(art));
        assert_ne!(middle, panel);
        assert_ne!(middle, target);
        assert_eq!(frame(&ctx, 0.1 + fade, panel, Some(art)), target);
    }

    #[test]
    fn changing_songs_mid_fade_continues_from_the_visible_colour() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let first = Color32::from_rgb(20, 40, 60);
        let second = Color32::from_rgb(220, 140, 160);
        let third = Color32::from_rgb(40, 230, 30);
        let first_fill = super::super::blend(panel, first, TINT_STRENGTH);
        let second_fill = super::super::blend(panel, second, TINT_STRENGTH);
        let third_fill = super::super::blend(panel, third, TINT_STRENGTH);
        let fade = f64::from(TINT_FADE_SECONDS);

        assert_eq!(frame(&ctx, 0.0, panel, Some(first)), first_fill);
        assert_eq!(frame(&ctx, 0.05, panel, Some(second)), first_fill);
        let middle = frame(&ctx, 0.2, panel, Some(second));
        assert_ne!(middle, first_fill);
        assert_ne!(middle, second_fill);
        assert_eq!(frame(&ctx, 0.2, panel, Some(third)), middle);
        assert_ne!(frame(&ctx, 0.2 + fade / 2.0, panel, Some(third)), middle);
        assert_eq!(frame(&ctx, 0.2 + fade, panel, Some(third)), third_fill);
    }

    #[test]
    fn a_new_session_does_not_reuse_the_previous_tint() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let first = Color32::from_rgb(20, 40, 60);
        let second = Color32::from_rgb(220, 140, 160);
        let next_session = Color32::from_rgb(40, 230, 30);

        frame(&ctx, 0.0, panel, Some(first));
        frame(&ctx, 0.1, panel, Some(second));
        assert_ne!(
            frame(&ctx, 0.2, panel, Some(second)),
            super::super::blend(panel, second, TINT_STRENGTH)
        );

        end_tint_session(&ctx);
        assert_eq!(
            frame(&ctx, 0.21, panel, Some(next_session)),
            super::super::blend(panel, next_session, TINT_STRENGTH)
        );

        end_tint_session(&ctx);
        assert_eq!(frame(&ctx, 0.22, panel, None), panel);
    }

    /// However light or dark the cover or the theme's accent, the
    /// visualizer's colours stay in the band the theme's text reads against,
    /// and keep their hue.
    #[test]
    fn visualizer_colours_stay_behind_the_words() {
        let luminance = |colour: Color32| {
            let linear = egui::Rgba::from(colour);
            0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
        };
        for base in [
            Color32::from_rgb(255, 240, 80),
            Color32::from_rgb(20, 30, 90),
            Color32::WHITE,
            Color32::BLACK,
            Color32::from_rgb(30, 215, 96),
        ] {
            let art = [[base.r(), base.g(), base.b()]];
            for (art, accent) in [(Some(&art[..]), Color32::RED), (None, base)] {
                for colour in vis_colours(art, accent, true) {
                    assert!(
                        luminance(colour) <= DARK_CEILING + 0.01,
                        "{base:?} on dark: {colour:?}"
                    );
                }
                for colour in vis_colours(art, accent, false) {
                    assert!(
                        (LIGHT_FLOOR - 0.01..=LIGHT_CEILING + 0.01).contains(&luminance(colour)),
                        "{base:?} on light: {colour:?}"
                    );
                }
            }
        }
        // A yellow cover stays yellow, only deeper.
        let low = vis_colours(Some(&[[255, 240, 80]]), Color32::RED, true)[0];
        assert!(low.r() > low.b() && low.g() > low.b(), "{low:?}");
    }

    /// With colour from album art the bars take the cover's own colours,
    /// its accent at the bass and its last colour at the treble, and a grey
    /// cover's bars stay grey. Without, they turn the theme's accent.
    #[test]
    fn visualizer_sweeps_across_the_covers_colours() {
        let art = [[200, 40, 40], [40, 200, 60], [40, 40, 200]];
        let colours = vis_colours(Some(&art), Color32::WHITE, true);
        assert_eq!(colours.len(), 3);
        let [red, green, blue] = [colours[0], colours[1], colours[2]];
        assert!(red.r() > red.g() && red.r() > red.b(), "{red:?}");
        assert!(green.g() > green.r() && green.g() > green.b(), "{green:?}");
        assert!(blue.b() > blue.r() && blue.b() > blue.g(), "{blue:?}");
        assert_eq!(swept(&colours, 0.0), red);
        assert_eq!(swept(&colours, 0.5), green);
        assert_eq!(swept(&colours, 1.0), blue);
        assert_eq!(swept(&colours[..1], 0.7), red);

        let spread = |c: Color32| c.r().max(c.g()).max(c.b()) - c.r().min(c.g()).min(c.b());
        for cover in [[150, 140, 140], [40, 20, 8]] {
            let grey = vis_colours(Some(&[cover]), Color32::RED, true)[0];
            assert!(spread(grey) < 4, "{cover:?} stays grey: {grey:?}");
        }
        // A grey accent gives the bass to the cover's hue.
        let colours = vis_colours(
            Some(&[[150, 140, 140], [200, 40, 40]]),
            Color32::WHITE,
            true,
        );
        assert!(colours[0].r() > colours[0].g() * 2, "{colours:?}");
        assert!(spread(colours[1]) < 24, "{colours:?}");

        let accent = Color32::from_rgb(30, 215, 96);
        assert_eq!(vis_colours(None, accent, true).len(), 2);
    }

    /// A window too narrow for it loses the button, not the volume.
    #[test]
    fn the_full_screen_button_goes_when_the_bar_is_tight() {
        assert!(full_screen_button_fits(360.0));
        assert!(
            !full_screen_button_fits(320.0),
            "reserve Now playing before fullscreen"
        );
        assert!(!full_screen_button_fits(200.0));
    }
}
