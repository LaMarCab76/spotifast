//! One side player: content sections above a persistent transport.

use egui::{Align, Frame, Layout, Margin, Rect, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, DragTrack, PlayerPanelTab};
use crate::theme::{self, Icon};

use super::player_bar::{self, Surface};

pub(crate) const PANEL_ID: &str = "now-playing-panel";
const SONG_HEIGHT: f32 = 64.0;
const CONTROLS_HEIGHT: f32 = SONG_HEIGHT + 132.0;

/// The occupied rectangle from this frame also keeps notifications off the controls.
pub(crate) fn show(app: &mut App, ui: &mut egui::Ui) -> Rect {
    let palette = app.palette;
    let fill = player_bar::eased_fill(ui.ctx(), palette.panel, app.now_playing_tint());
    let fit = super::yielding_panel(
        ui.ctx(),
        PANEL_ID,
        theme::SIDE_PANEL_MIN_WIDTH..=560.0,
        app.settings.queue_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let response = super::docked_panel(PANEL_ID, super::panels_on_left(app))
        .resizable(true)
        .default_size(app.settings.queue_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(Frame::new().fill(fill).inner_margin(Margin::same(12)))
        .show(ui, |ui| {
            let reservation =
                super::window_controls_reservation(ui.ctx(), app, ui.available_width());
            ui.add_space(
                reservation.queue_top
                    + super::left_corner_inset(ui.ctx(), app, super::Header::Queue),
            );
            header(app, ui);
            ui.add_space(10.0);
            egui::Panel::bottom("side-player-controls")
                .exact_size(CONTROLS_HEIGHT)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new())
                .show(ui, |ui| controls(app, ui));
            // The short window keeps its transport; the artwork and details yield.
            if app.settings.player_bar_vis != crate::settings::PlayerBarVis::Off
                && app.visualizers_available()
                && ui.available_height() >= 280.0
            {
                egui::Panel::bottom("side-player-visualizer")
                    .exact_size(64.0)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(Frame::new())
                    .show(ui, |ui| {
                        player_bar::panel_visualizer(app, ui, ui.max_rect())
                    });
            }
            match app.player_panel_tab().unwrap_or(PlayerPanelTab::NowPlaying) {
                PlayerPanelTab::NowPlaying => super::now_playing::section(app, ui),
                PlayerPanelTab::Queue => super::queue::section(app, ui),
                PlayerPanelTab::Lyrics => super::lyrics::section(app, ui),
                PlayerPanelTab::Friends => super::friends::section(app, ui),
            }
        });
    let width = response.response.rect.width();
    if (width - app.settings.queue_width).abs() > 1.0 && super::panel_dragged(ui.ctx(), PANEL_ID) {
        app.settings.queue_width = width;
        app.actions.push(Action::SettingsChanged);
    }
    response.response.rect
}

fn header(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let active = app.player_panel_tab().unwrap_or(PlayerPanelTab::NowPlaying);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (tab, icon, label) in [
            (
                PlayerPanelTab::NowPlaying,
                Icon::SquarePlay,
                gettext(app.locale, "Now playing"),
            ),
            (
                PlayerPanelTab::Queue,
                Icon::ListVideo,
                gettext(app.locale, "Queue"),
            ),
            (
                PlayerPanelTab::Lyrics,
                Icon::Mic,
                gettext(app.locale, "Lyrics"),
            ),
            (
                PlayerPanelTab::Friends,
                Icon::Users,
                gettext(app.locale, "Listening activity"),
            ),
        ] {
            let selected = active == tab;
            let response = theme::icon_button(
                ui,
                icon,
                18.0,
                if selected {
                    palette.accent
                } else {
                    palette.secondary
                },
                palette.text,
                &label,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    ui.is_enabled(),
                    selected,
                    label.as_ref(),
                )
            });
            if response.clicked() {
                app.actions.push(Action::SelectPlayerPanelTab(tab));
            }
            if tab == PlayerPanelTab::Queue
                && let Some(track) = response.dnd_release_payload::<DragTrack>()
            {
                app.actions.push(Action::QueueMany {
                    songs: track
                        .items
                        .iter()
                        .map(|item| (item.uri().to_string(), item.name().to_string()))
                        .collect(),
                });
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::ChevronDown,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Collapse player"),
            )
            .clicked()
            {
                app.actions.push(Action::ClosePlayerPanel);
            }
            if theme::icon_button(
                ui,
                Icon::Expand,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Full screen lyrics"),
            )
            .clicked()
            {
                app.actions.push(Action::SetLyricsFullscreen(true));
            }
        });
    });
}

fn controls(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, app.palette.outline),
    );
    let now = app.now_playing();
    let song = Rect::from_min_size(rect.min, vec2(rect.width(), SONG_HEIGHT));
    player_bar::now_playing_block(app, ui, song, now.as_ref(), Surface::Panel);
    let transport = Rect::from_min_size(
        pos2(rect.left(), rect.top() + SONG_HEIGHT),
        vec2(rect.width(), 88.0),
    );
    player_bar::panel_transport(app, ui, transport);
    let band = Rect::from_min_size(
        pos2(rect.left(), rect.top() + SONG_HEIGHT + 100.0),
        vec2(rect.width(), 30.0),
    );
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(band)
            .layout(Layout::left_to_right(Align::Center)),
    );
    player_bar::devices_button(app, &mut row, now.as_ref());
    let volume = Rect::from_min_max(pos2(band.left() + 38.0, band.top()), band.max);
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(volume)
            .layout(Layout::right_to_left(Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    player_bar::volume_controls(
        app,
        &mut row,
        now.as_ref(),
        (volume.width() - 36.0).max(1.0),
        Surface::Panel,
    );
}
