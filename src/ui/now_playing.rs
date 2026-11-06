//! The playing song beside the page, as Spotify's own client shows it in
//! its Now Playing view: the cover, the song and its artists, the first
//! artist's card, the song's artists, and what plays next.

use egui::{Frame, Margin, Sense, Vec2};

use crate::api::models::{ArtistRef, PlayableItem, pick_image};
use crate::app::{App, NowPlaying};
use crate::i18n::{gettext, ngettext, pgettext};
use crate::model::{Action, Page, RowContext};
use crate::theme::{self, Icon};
use crate::util;

use super::widgets;

/// The panel's id, which egui remembers its width under.
pub(crate) const PANEL_ID: &str = super::player_panel::PANEL_ID;
/// Rows of the biography shown before it is cut.
const BIOGRAPHY_ROWS: usize = 4;
/// The corner of the cover and the cards.
const RADIUS: f32 = 8.0;

pub(super) fn section(app: &mut App, ui: &mut egui::Ui) {
    let now = app.now_playing();
    if let Some(from) = now.as_ref().and_then(|_| app.playing_from()) {
        if let Some(page) = from.page {
            if theme::link(ui, from.name, theme::bold(16.0), app.palette.text).clicked() {
                app.actions.push(Action::Open(page));
            }
        } else {
            theme::text(ui, from.name, theme::bold(16.0), app.palette.text);
        }
        ui.add_space(10.0);
    }
    let art_side = ui
        .available_width()
        .min((ui.available_height() * 0.6).max(64.0));
    egui::ScrollArea::vertical()
        .id_salt("now-playing-panel-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| match now {
            Some(now) => contents(app, ui, &now, art_side),
            None => widgets::empty_state(
                ui,
                &app.palette,
                Icon::Music,
                &gettext(app.locale, "Nothing playing"),
                &gettext(app.locale, "Pick a song, album, or playlist"),
            ),
        });
}

fn contents(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying, art_side: f32) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - art_side) / 2.0).max(0.0));
        cover(app, ui, now, art_side);
    });
    ui.add_space(14.0);
    title(app, ui, now);
    // An episode, a local file, or a song without artists has no card.
    if !now.is_episode
        && !local_file(now)
        && let Some(artist) = now.artists.first().filter(|artist| artist.id.is_some())
    {
        ui.add_space(18.0);
        about_artist(app, ui, artist);
    }
    if now.artists.len() > 1 {
        ui.add_space(14.0);
        artists(app, ui, &now.artists);
    }
    ui.add_space(14.0);
    next_up(app, ui);
    ui.add_space(12.0);
}

/// The cover at the panel's width; a click opens its album or podcast.
fn cover(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying, width: f32) {
    let palette = app.palette;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(width), Sense::click_and_drag());
    let url = now.art_url.clone().or_else(|| now.art_small.clone());
    widgets::playing_cover(
        ui,
        &palette,
        app.backend.art(),
        PANEL_ID,
        url.as_deref(),
        rect,
        RADIUS,
    );
    song_interaction(app, ui, now, &response);
    if let Some(page) = cover_page(now)
        && response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        app.actions.push(Action::Open(page));
    }
}

fn song_interaction(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying, response: &egui::Response) {
    let Some(item) = app.now_playing_item().filter(|item| item.uri() == now.uri) else {
        return;
    };
    if response.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            crate::model::DragTrack {
                title: item.name().to_string(),
                image: app.item_image(&item, 64),
                items: vec![item.clone()],
                from: None,
                source_playlist: None,
            },
        );
    }
    let context = app.editable_context_playlist();
    egui::Popup::context_menu(response)
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| widgets::item_menu(ui, app, &item, context.as_ref(), None));
}

/// A song from the local files folder: Spotify knows only its tags.
fn local_file(now: &NowPlaying) -> bool {
    now.uri.starts_with("spotify:local:")
}

/// The page the cover and title open: the album, or the podcast.
fn cover_page(now: &NowPlaying) -> Option<Page> {
    now.album_id
        .clone()
        .map(Page::Album)
        .or_else(|| now.show_id.clone().map(Page::Show))
}

/// The song's large title and artists. Its heart stays in the persistent footer.
fn title(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying) {
    let palette = app.palette;
    ui.horizontal_top(|ui| {
        let text_width = ui.available_width().max(40.0);
        ui.vertical(|ui| {
            ui.set_width(text_width);
            ui.spacing_mut().item_spacing.y = 2.0;
            let galley = crate::bidi::layout(
                ui.painter(),
                &now.title,
                theme::bold(22.0),
                palette.text,
                text_width,
                2,
                Some(crate::bidi::ELLIPSIS),
            );
            let response = ui.add(
                egui::Label::new(galley)
                    .selectable(false)
                    .sense(Sense::click_and_drag()),
            );
            song_interaction(app, ui, now, &response);
            if let Some(page) = cover_page(now)
                && response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            {
                app.actions.push(Action::Open(page));
            }
            let artists = ui.horizontal_wrapped(|ui| {
                if now.artists.is_empty() {
                    let response =
                        theme::link(ui, &now.subtitle, theme::regular(14.0), palette.secondary);
                    if response.clicked()
                        && let Some(id) = &now.show_id
                    {
                        app.actions.push(Action::Open(Page::Show(id.clone())));
                    }
                } else {
                    widgets::artist_links(
                        ui,
                        app,
                        &now.artists,
                        theme::regular(14.0),
                        palette.secondary,
                    );
                }
            });
            let response = artists.response.interact(Sense::drag());
            song_interaction(app, ui, now, &response);
        });
    });
}

/// A card for the first artist: their portrait, monthly listeners, the
/// start of their biography, and whether the account follows them. What
/// Spotify has not answered yet is left out, never another artist's.
fn about_artist(app: &mut App, ui: &mut egui::Ui, artist: &ArtistRef) {
    let palette = app.palette;
    let locale = app.locale;
    let (Some(id), Some(uri)) = (artist.id.clone(), super::artist_uri(artist)) else {
        return;
    };
    let profile = app.artist_profiles.get(&uri).cloned();
    let portrait = profile
        .as_ref()
        .and_then(|profile| profile.avatar.clone())
        .or_else(|| {
            app.known_artist(&id)
                .and_then(|known| pick_image(&known.images, 640).map(str::to_string))
        });
    let listeners = app.play_counts.get(&uri).copied();
    let following = app.is_saved(&uri);
    card(ui, &palette, |ui| {
        let width = ui.available_width();
        theme::text(
            ui,
            gettext(locale, "About the artist"),
            theme::bold(15.0),
            palette.text,
        );
        ui.add_space(10.0);
        if let Some(url) = &portrait {
            let height = (width * 0.62).round();
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
            // The portrait fills the frame, cut to its shape.
            widgets::paint_cover(
                ui,
                &palette,
                Some(url),
                rect,
                RADIUS,
                Icon::User,
                Some(app.backend.art()),
            );
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.actions.push(Action::Open(Page::Artist(id.clone())));
            }
            ui.add_space(10.0);
        }
        let mut open = false;
        let mut follow = false;
        egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                open = theme::link(ui, &artist.name, theme::bold(16.0), palette.text).clicked();
            },
            |ui| {
                // Until Spotify says whether the account follows them, the
                // button would only guess.
                if let Some(following) = following {
                    follow = theme::pill_button(
                        ui,
                        &palette,
                        &if following {
                            pgettext(locale, "artist", "Following")
                        } else {
                            pgettext(locale, "artist", "Follow")
                        },
                        false,
                    )
                    .clicked();
                }
            },
        );
        if open {
            app.actions.push(Action::Open(Page::Artist(id.clone())));
        }
        if follow {
            app.actions.push(Action::ToggleSaved(uri.clone()));
        }
        if let Some(listeners) = listeners {
            theme::text(
                ui,
                ngettext(
                    locale,
                    // Translators: {count} is the number of people who listened
                    // to an artist in the last 28 days, as Spotify counts them.
                    "{count} monthly listener",
                    "{count} monthly listeners",
                    u32::try_from(listeners).unwrap_or(u32::MAX),
                )
                .replace("{count}", &util::format_count(listeners)),
                theme::regular(13.0),
                palette.secondary,
            );
        }
        if let Some(biography) = profile.and_then(|profile| profile.biography) {
            ui.add_space(6.0);
            let galley = crate::bidi::layout(
                ui.painter(),
                &biography,
                theme::regular(13.0),
                palette.secondary,
                width,
                BIOGRAPHY_ROWS,
                Some(crate::bidi::ELLIPSIS),
            );
            let response = ui.add(
                egui::Label::new(galley)
                    .selectable(false)
                    .sense(Sense::click()),
            );
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.actions.push(Action::Open(Page::Artist(id)));
            }
        }
    });
}

/// Every artist credited on the song, each opening their page.
fn artists(app: &mut App, ui: &mut egui::Ui, artists: &[ArtistRef]) {
    let palette = app.palette;
    let locale = app.locale;
    card(ui, &palette, |ui| {
        theme::text(
            ui,
            gettext(locale, "Artists"),
            theme::bold(15.0),
            palette.text,
        );
        ui.add_space(6.0);
        for artist in artists {
            match &artist.id {
                Some(id) => {
                    if theme::link(ui, &artist.name, theme::medium(14.0), palette.text).clicked() {
                        app.actions.push(Action::Open(Page::Artist(id.clone())));
                    }
                }
                None => {
                    theme::text(ui, &artist.name, theme::medium(14.0), palette.text);
                }
            }
        }
    });
}

/// The first song still to play, and a way to the whole queue.
fn next_up(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let next: Option<PlayableItem> = app
        .queue
        .get()
        .and_then(|queue| queue.queue.first().cloned());
    card(ui, &palette, |ui| {
        let mut open = false;
        egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                theme::text(
                    ui,
                    gettext(locale, "Next in queue"),
                    theme::bold(15.0),
                    palette.text,
                );
            },
            |ui| {
                open = theme::link(
                    ui,
                    gettext(locale, "Open queue"),
                    theme::semibold(13.0),
                    palette.secondary,
                )
                .clicked();
            },
        );
        if open {
            app.actions.push(Action::ToggleQueuePanel);
        }
        ui.add_space(6.0);
        match next {
            Some(item) => {
                // The queue's own first row: playing it moves through the
                // queue and keeps what follows, as the Queue panel does.
                widgets::track_row(
                    ui,
                    app,
                    widgets::TrackRow {
                        index: 0,
                        number: None,
                        item: &item,
                        context: &RowContext::Queue,
                        show_cover: true,
                        show_album: false,
                        added_at: None,
                        added_by: None,
                        show_added_by: false,
                        show_plays: false,
                        compact: true,
                        thin: false,
                        shift: 0.0,
                        picked: false,
                        picked_songs: &[],
                        picked_positions: &[],
                        position: None,
                    },
                );
            }
            None => {
                theme::text(
                    ui,
                    gettext(locale, "Nothing else is queued."),
                    theme::regular(13.0),
                    palette.secondary,
                );
            }
        }
    });
}

/// A rounded block on the panel, as Spotify sets each part of the view.
fn card(ui: &mut egui::Ui, palette: &theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(RADIUS)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}
