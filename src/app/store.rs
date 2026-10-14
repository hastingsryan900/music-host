//! The Store: search Apple's catalogue for any artist, album or song and
//! get it — downloads come from YouTube and land in the library, tagged and
//! with HD covers, like everything else in "Add from Link".

use super::*;
use crate::ytdl::{CatalogAlbum, CatalogAlbumRef, StoreSong};

/// Store lookups in flight or finished.
#[derive(Default)]
pub struct StoreData {
    /// query → results (None while searching; Some(None) = couldn't reach Apple)
    pub searches: HashMap<String, Option<Option<crate::ytdl::StoreResults>>>,
    /// album id → tracklist (None while loading)
    pub albums: HashMap<u64, Option<Option<CatalogAlbum>>>,
}

pub enum StoreMsg {
    Search(String, Option<crate::ytdl::StoreResults>),
    Album(u64, Option<CatalogAlbum>),
}

/// Track-job key for a store song (album id + normalised title).
fn song_key(album_id: u64, title: &str) -> (String, String) {
    (format!("store:{album_id}"), crate::ytdl::norm(title))
}

impl App {
    pub(super) fn store_search(&mut self, ctx: &Context, q: String) {
        if q.is_empty() || self.store.searches.contains_key(&q) {
            return;
        }
        self.store.searches.insert(q.clone(), None);
        let (tx, wake) = (self.store_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = crate::ytdl::store_search(&q);
            let _ = tx.send(StoreMsg::Search(q, r));
            wake.request_repaint();
        });
    }

    pub(super) fn store_album(&mut self, ctx: &Context, id: u64) {
        if self.store.albums.contains_key(&id) {
            return;
        }
        self.store.albums.insert(id, None);
        let (tx, wake) = (self.store_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let a = crate::ytdl::album_by_id(id);
            let _ = tx.send(StoreMsg::Album(id, a));
            wake.request_repaint();
        });
    }

    pub(super) fn store_get_song(&mut self, ctx: &Context, s: StoreSong) {
        let key = song_key(s.album_id, &s.track.title);
        if let Some(id) = self.track_jobs.get(&key) {
            if self.jobs.iter().any(|j| j.id == *id && !j.done) {
                return;
            }
        }
        let root = self.download_root();
        let req = crate::ytdl::store_song_request(&s, &root, &self.library_snapshot());
        self.toast(ctx, format!("Getting “{}”…", s.track.title));
        self.dispatch(ctx, vec![Pending::Track(key.0, req)]);
    }

    pub(super) fn pump_store(&mut self) {
        while let Ok(m) = self.store_rx.try_recv() {
            match m {
                StoreMsg::Search(q, r) => {
                    self.store.searches.insert(q, Some(r));
                }
                StoreMsg::Album(id, a) => {
                    self.store.albums.insert(id, Some(a));
                }
            }
        }
    }
}

impl V<'_> {
    pub(super) fn store(&mut self, ui: &mut Ui) {
        // Search bar (searches as you type, after a short pause).
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 132.0), Sense::hover());
        let x0 = bar.left() + PAD;
        ui.painter().text(pos2(x0, bar.top() + 22.0), Align2::LEFT_TOP, "Store", bold(30.0), TEXT);
        ui.painter().text(pos2(x0, bar.top() + 60.0), Align2::LEFT_TOP, "Find any artist, album or song — Get adds it to your library", font(13.0), DIM);
        let field = Rect::from_min_size(pos2(x0, bar.top() + 86.0), vec2((bar.width() - PAD * 2.0).min(560.0), 36.0));
        ui.painter().rect_filled(field, cr(10), Color32::from_rgb(38, 38, 44));
        // Magnifying glass.
        let c = pos2(field.left() + 17.0, field.center().y - 1.5);
        ui.painter().circle_stroke(c, 5.5, Stroke::new(1.6f32, DIM));
        ui.painter().line_segment([c + vec2(4.0, 4.0), c + vec2(8.0, 8.0)], Stroke::new(1.8f32, DIM));
        let mut fui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(field.min + vec2(36.0, 7.0), field.max - vec2(12.0, 6.0))));
        let resp = fui.add(
            egui::TextEdit::singleline(&mut self.st.store_query)
                .hint_text(egui::RichText::new("Artists, albums, songs").color(FAINT))
                .font(font(15.0))
                .frame(false)
                .desired_width(f32::INFINITY),
        );
        if resp.changed() {
            self.st.store_typed_at = self.now;
        }
        if self.st.store_focus {
            self.st.store_focus = false;
            resp.request_focus();
        }
        let q = self.st.store_query.trim().to_string();
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if !q.is_empty() && q != self.st.store_submitted && (enter || self.now - self.st.store_typed_at > 0.5) {
            self.st.store_submitted = q.clone();
            self.st.store_album = None;
            self.st.store_artist = None;
            self.acts.push(Action::StoreSearch(q));
        } else if q != self.st.store_submitted && !q.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(120));
        }

        let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt(("store", &self.st.store_submitted, self.st.store_album, &self.st.store_artist));
        if let Some(off) = std::env::var("MH_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            sa = sa.vertical_scroll_offset(off);
        }
        sa.show(ui, |ui| {
            if let Some(id) = self.st.store_album {
                self.store_album_page(ui, id);
            } else if let Some(name) = self.st.store_artist.clone() {
                self.store_artist_page(ui, &name);
            } else if self.st.store_submitted.is_empty() {
                self.store_home(ui);
            } else {
                self.store_results(ui);
            }
            ui.add_space(30.0);
        });
    }

    fn store_home(&mut self, ui: &mut Ui) {
        ui.add_space(30.0);
        // Suggestions: artists you listen to most → discover the rest of their catalogue.
        let mut by_artist: HashMap<String, f64> = HashMap::new();
        for t in &self.lib.data.tracks {
            *by_artist.entry(t.album_artist.clone()).or_default() += t.listened_secs + 1.0;
        }
        let mut top: Vec<(String, f64)> = by_artist.into_iter().filter(|(a, _)| !a.is_empty() && a != "Various Artists").collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        if top.is_empty() {
            return;
        }
        subhead(ui, "Explore artists you play");
        ui.horizontal_wrapped(|ui| {
            ui.add_space(PAD);
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for (name, _) in top.iter().take(12) {
                if pill(ui, None, name, false).clicked() {
                    self.st.store_artist = Some(name.clone());
                }
            }
        });
    }

    fn back_link(&mut self, ui: &mut Ui, label: &str) -> bool {
        ui.add_space(6.0);
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.add_space(PAD - 4.0);
            let r = ui.add(egui::Button::new(egui::RichText::new(format!("‹ {label}")).size(14.0).color(accent())).frame(false));
            clicked = r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        });
        clicked
    }

    fn store_results(&mut self, ui: &mut Ui) {
        let q = self.st.store_submitted.clone();
        let res = match self.store.searches.get(&q) {
            None | Some(None) => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Searching…").color(DIM));
                });
                return;
            }
            Some(Some(None)) => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("Couldn't reach the catalogue. Check your internet connection.").color(DIM)));
                return;
            }
            Some(Some(Some(r))) => r.clone(),
        };
        if res.artists.is_empty() && res.albums.is_empty() && res.songs.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(format!("Nothing found for “{q}”.")).color(DIM)));
            return;
        }
        if !res.artists.is_empty() {
            subhead(ui, "Artists");
            ui.horizontal_wrapped(|ui| {
                ui.add_space(PAD);
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for name in &res.artists {
                    if pill(ui, None, name, false).clicked() {
                        self.st.store_artist = Some(name.clone());
                    }
                }
            });
        }
        if !res.albums.is_empty() {
            subhead(ui, "Albums");
            // One row up top; the rest after the songs.
            let w = ui.available_width() - PAD * 2.0;
            let cols = (((w + TILE_GAP) / (170.0 + TILE_GAP)).floor() as usize).max(1);
            self.store_grid(ui, &res.albums[..res.albums.len().min(cols)]);
        }
        if !res.songs.is_empty() {
            subhead(ui, "Songs");
            for (i, s) in res.songs.iter().take(12).enumerate() {
                self.store_song_row(ui, s, i, true);
            }
        }
        let w = ui.available_width() - PAD * 2.0;
        let cols = (((w + TILE_GAP) / (170.0 + TILE_GAP)).floor() as usize).max(1);
        if res.albums.len() > cols {
            subhead(ui, "More Albums");
            self.store_grid(ui, &res.albums[cols..]);
        }
    }

    fn store_artist_page(&mut self, ui: &mut Ui, name: &str) {
        if self.back_link(ui, if self.st.store_submitted.is_empty() { "Store" } else { "Results" }) {
            self.st.store_artist = None;
            return;
        }
        heading(ui, name);
        if self.lib.artists.contains_key(name) {
            ui.horizontal(|ui| {
                ui.add_space(PAD);
                if ui.add(egui::Button::new(egui::RichText::new("In your library →").size(13.0).color(accent())).frame(false)).clicked() {
                    self.acts.push(Action::Go(View::Artists(Some(name.to_string()))));
                }
            });
        }
        match self.discos.get(name) {
            None => {
                self.acts.push(Action::FetchDiscography(name.to_string()));
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Disco::Loading) => {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Disco::Ready(None)) => {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("Couldn't find this artist's albums.").color(DIM)));
            }
            Some(Disco::Ready(Some(list))) => {
                let list = list.clone();
                subhead(ui, "Albums");
                self.store_grid(ui, &list);
            }
        }
    }

    fn store_album_page(&mut self, ui: &mut Ui, id: u64) {
        if self.back_link(ui, "Back") {
            self.st.store_album = None;
            return;
        }
        let album = match self.store.albums.get(&id) {
            None => {
                self.acts.push(Action::StoreOpenAlbum(id));
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
                return;
            }
            Some(None) => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
                return;
            }
            Some(Some(None)) => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("Couldn't load this album.").color(DIM)));
                return;
            }
            Some(Some(Some(a))) => a.clone(),
        };
        let r = CatalogAlbumRef {
            id,
            title: album.title.clone(),
            artist: album.artist.clone(),
            year: album.year,
            track_count: album.tracks.len() as u32,
            cover_url: album.cover_url.clone(),
        };
        // Header: cover, title, artist, Get Album.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 236.0), Sense::hover());
        let art = Rect::from_min_size(pos2(head.left() + PAD, head.top() + 12.0), vec2(210.0, 210.0));
        shadow(ui, art, 10, 30, 10, 0.5);
        self.remote_art(ui, r.cover_url.as_deref(), &r.title, art, 10, Color32::WHITE);
        let tx = art.right() + 28.0;
        let tw = head.right() - tx - PAD;
        text(ui, pos2(tx, art.top() + 54.0), &r.title, bold(26.0), TEXT, tw);
        text(ui, pos2(tx, art.top() + 92.0), &r.artist, font(18.0), accent(), tw);
        let sub = [album.genre.clone(), album.year.map(|y| y.to_string()), Some(format!("{} song{}", r.track_count, if r.track_count == 1 { "" } else { "s" }))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        text(ui, pos2(tx, art.top() + 120.0), &sub, font(13.0), DIM, tw);
        let owned = self.owned_album(&r.title, &r.artist);
        let job = self.album_jobs.get(&id).and_then(|j| self.jobs.iter().find(|x| x.id == *j));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(tx, art.top() + 150.0), vec2(tw, 44.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        match job {
            Some(j) if !j.done => {
                bui.spinner();
                let label = match j.progress {
                    Some(p) => format!("{} · {:.0}%", j.status, p * 100.0),
                    None => j.status.clone(),
                };
                bui.label(egui::RichText::new(label).color(DIM));
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            _ => {
                let label = if owned { "Get Missing Songs" } else { "Get Album" };
                if pill(&mut bui, Some("⬇"), label, true).clicked() {
                    self.acts.push(Action::DownloadCatalogAlbum(r.clone()));
                }
                if owned {
                    bui.label(egui::RichText::new("  In your library").size(12.0).color(FAINT));
                }
            }
        }
        ui.add_space(8.0);
        for (i, t) in album.tracks.iter().enumerate() {
            let s = StoreSong {
                track: t.clone(),
                album: album.title.clone(),
                album_artist: album.artist.clone(),
                album_id: id,
                year: album.year,
                genre: album.genre.clone(),
                cover_url: album.cover_url.clone(),
            };
            self.store_song_row(ui, &s, i, false);
        }
    }

    /// Album tiles: click to open, hover for a Get button.
    fn store_grid(&mut self, ui: &mut Ui, albums: &[CatalogAlbumRef]) {
        let w = ui.available_width() - PAD * 2.0;
        let cols = (((w + TILE_GAP) / (170.0 + TILE_GAP)).floor() as usize).max(1);
        let tile = (w - TILE_GAP * (cols - 1) as f32) / cols as f32;
        for chunk in albums.chunks(cols) {
            let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), tile + 62.0), Sense::hover());
            if !ui.is_rect_visible(strip) {
                continue;
            }
            for (k, r) in chunk.iter().enumerate() {
                let art = Rect::from_min_size(pos2(strip.left() + PAD + k as f32 * (tile + TILE_GAP), strip.top()), vec2(tile, tile));
                self.store_tile(ui, r, art);
            }
        }
    }

    fn store_tile(&mut self, ui: &mut Ui, r: &CatalogAlbumRef, art: Rect) {
        let owned = self.owned_album(&r.title, &r.artist);
        let job = self.album_jobs.get(&r.id).and_then(|id| self.jobs.iter().find(|j| j.id == *id));
        let busy = job.map(|j| !j.done).unwrap_or(false);
        let resp = ui.interact(art, Id::new(("store-tile", r.id)), Sense::click());
        let h = anim(ui, resp.id, resp.hovered(), 0.2);
        let grown = art.expand(art.width() * 0.025 * h);
        shadow(ui, grown, 10, (14.0 + 18.0 * h) as u8, (4.0 + 6.0 * h) as i8, 0.35 + 0.25 * h);
        self.remote_art(ui, r.cover_url.as_deref(), &r.title, grown, 10, Color32::WHITE);
        // "Get" button bottom-right on hover; progress while downloading.
        let get = Rect::from_min_size(pos2(grown.right() - 70.0, grown.bottom() - 40.0), vec2(60.0, 30.0));
        let mut get_hit = false;
        if busy {
            let j = job.unwrap();
            ui.painter().rect_filled(grown, cr(10), Color32::from_black_alpha(120));
            let label = j.progress.map(|p| format!("{:.0}%", p * 100.0)).unwrap_or_else(|| "Finding…".into());
            ui.painter().text(grown.center(), Align2::CENTER_CENTER, label, bold(15.0), Color32::WHITE);
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        } else if owned {
            let badge = Rect::from_min_size(pos2(grown.left() + 8.0, grown.top() + 8.0), vec2(84.0, 22.0));
            ui.painter().rect_filled(badge, cr(11), Color32::from_black_alpha(170));
            ui.painter().text(badge.center(), Align2::CENTER_CENTER, "In Library", bold(11.0), Color32::WHITE);
        } else if h > 0.001 {
            get_hit = ui.rect_contains_pointer(get);
            ui.painter().rect_filled(get, cr(15), alpha(if get_hit { lerp(accent(), Color32::WHITE, 0.15) } else { accent() }, h));
            ui.painter().text(get.center(), Align2::CENTER_CENTER, "Get", bold(13.0), alpha(on_accent(), h));
        }
        text(ui, pos2(art.left(), art.bottom() + 10.0), &r.title, font(13.5), TEXT, art.width());
        let sub = match r.year {
            Some(y) => format!("{} · {y}", r.artist),
            None => r.artist.clone(),
        };
        text(ui, pos2(art.left(), art.bottom() + 29.0), &sub, font(12.5), DIM, art.width());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            if get_hit {
                self.acts.push(Action::DownloadCatalogAlbum(r.clone()));
            } else {
                self.st.store_album = Some(r.id);
            }
        }
    }

    /// One song: cover, title, artist · album, length, Get / status.
    fn store_song_row(&mut self, ui: &mut Ui, s: &StoreSong, i: usize, show_art: bool) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if show_art { 52.0 } else { 42.0 }), Sense::click());
        let row = row.shrink2(vec2(PAD - 10.0, 0.0));
        if !ui.is_rect_visible(row) {
            return;
        }
        let owned = self.owned_song(&s.track.title, &s.track.artist);
        let job = self.track_jobs.get(&song_key(s.album_id, &s.track.title)).and_then(|id| self.jobs.iter().find(|j| j.id == *id));
        let busy = job.map(|j| !j.done).unwrap_or(false);
        self.row_bg(ui, row, resp.id, resp.hovered(), false, i);
        let mut x = row.left() + 12.0;
        if show_art {
            let art = Rect::from_min_size(pos2(x, row.center().y - 20.0), vec2(40.0, 40.0));
            self.remote_art(ui, s.cover_url.as_deref(), &s.album, art, 5, Color32::WHITE);
            x = art.right() + 12.0;
        } else {
            ui.painter().text(pos2(x + 10.0, row.center().y), Align2::CENTER_CENTER, s.track.track.to_string(), font(13.0), FAINT);
            x += 36.0;
        }
        let tw = row.right() - x - 190.0;
        let sub = if show_art { format!("{} · {}", s.track.artist, s.album) } else { s.track.artist.clone() };
        if show_art || s.track.artist != s.album_artist {
            text(ui, pos2(x, row.center().y - 17.0), &s.track.title, font(14.0), TEXT, tw);
            text(ui, pos2(x, row.center().y + 1.0), &sub, font(12.5), DIM, tw);
        } else {
            text(ui, pos2(x, row.center().y - 9.0), &s.track.title, font(14.0), TEXT, tw);
        }
        text_right(ui, pos2(row.right() - 96.0, row.center().y - 8.0), &fmt_time(s.track.duration_ms as f64 / 1000.0), font(13.0), FAINT);
        let btn = Rect::from_min_size(pos2(row.right() - 76.0, row.center().y - 13.0), vec2(64.0, 26.0));
        let mut get_hit = false;
        match job {
            Some(j) if !j.done => {
                let label = j.progress.map(|p| format!("{:.0}%", p * 100.0)).unwrap_or_else(|| "…".into());
                ui.painter().text(btn.center(), Align2::CENTER_CENTER, label, bold(12.5), accent());
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            Some(j) if j.failed => {
                get_hit = ui.rect_contains_pointer(btn);
                ui.painter().text(btn.center(), Align2::CENTER_CENTER, "⚠ Retry", font(12.0), Color32::from_rgb(255, 140, 120));
            }
            _ if owned || job.map(|j| j.done && !j.cancelled).unwrap_or(false) => {
                ui.painter().text(btn.center(), Align2::CENTER_CENTER, "✓ Added", font(12.0), FAINT);
            }
            _ => {
                get_hit = ui.rect_contains_pointer(btn);
                let fill = if get_hit { accent() } else { Color32::from_rgb(44, 44, 50) };
                ui.painter().rect_filled(btn, cr(13), fill);
                ui.painter().text(btn.center(), Align2::CENTER_CENTER, "Get", bold(12.5), if get_hit { on_accent() } else { accent() });
            }
        }
        let tip = match job {
            Some(j) if j.failed => format!("Couldn't download: {}", j.status),
            _ => "Click Get to find it on YouTube and add it to your library".into(),
        };
        let resp = resp.on_hover_text(tip);
        if resp.clicked() && get_hit && !busy {
            self.acts.push(Action::StoreGetSong(s.clone()));
        } else if resp.clicked() && show_art && s.album_id != 0 {
            self.st.store_album = Some(s.album_id);
        }
    }

    /// Catalogue cover (cached on disk), dimmed by `tint`.
    fn remote_art(&mut self, ui: &Ui, url: Option<&str>, seed: &str, rect: Rect, radius: u8, tint: Color32) {
        if let Some(url) = url {
            match self.art.get_url(url) {
                Art::Ready { tex, .. } => {
                    egui::Image::new((tex.id(), rect.size())).corner_radius(cr(radius)).tint(tint).paint_at(ui, rect);
                    return;
                }
                Art::Loading => return placeholder(ui, rect, radius, seed, true),
                Art::Missing => {}
            }
        }
        placeholder(ui, rect, radius, seed, false);
    }

    fn owned_album(&self, title: &str, artist: &str) -> bool {
        let (b, a) = (crate::ytdl::base_album(title), crate::ytdl::norm(artist));
        self.lib.albums.iter().any(|x| {
            let xa = crate::ytdl::norm(&x.artist);
            crate::ytdl::base_album(&x.title) == b && (xa == a || xa.contains(&a) || a.contains(&xa) || xa == "variousartists")
        })
    }

    fn owned_song(&self, title: &str, artist: &str) -> bool {
        let a = crate::ytdl::norm(artist.split([',', '&']).next().unwrap_or(artist));
        self.lib.data.tracks.iter().any(|t| crate::ytdl::same_song(&t.title, title) && crate::ytdl::norm(&t.artist).contains(&a))
    }
}
