//! The Store, modelled on Apple Music: search Apple's catalogue for any
//! artist, album or song, browse artist pages and charts, and Get anything —
//! downloads come from YouTube and land in the library, tagged with HD covers.

use super::*;
use egui::Pos2;
use crate::ytdl::{ArtistPage, CatalogAlbum, CatalogAlbumRef, Charts, StoreArtist, StoreResults, StoreSong};

/// A page in the Store's back stack (the root is home / search results).
#[derive(Clone)]
pub enum StorePage {
    Artist(u64),
    Album(u64),
    AllAlbums(String, Vec<CatalogAlbumRef>),
    AllSongs(String, Vec<StoreSong>),
}

/// Store lookups in flight (None) or finished (Some(result)).
#[derive(Default)]
pub struct StoreData {
    pub searches: HashMap<String, Option<Option<StoreResults>>>,
    pub albums: HashMap<u64, Option<Option<CatalogAlbum>>>,
    pub artists: HashMap<u64, Option<Option<ArtistPage>>>,
    /// artist id → photo base URL
    pub images: HashMap<u64, Option<Option<String>>>,
    /// library artist name → catalogue id
    pub name_ids: HashMap<String, Option<Option<u64>>>,
    pub charts: Option<Option<Charts>>,
}

pub enum StoreMsg {
    Search(String, Option<StoreResults>),
    Album(u64, Option<CatalogAlbum>),
    Artist(u64, Option<ArtistPage>),
    Image(u64, Option<String>),
    NameId(String, Option<u64>),
    Charts(Option<Charts>),
}

/// Something the Store page wants fetched.
pub enum StoreLoad {
    Search(String),
    Album(u64),
    Artist(u64),
    Image(u64),
    NameId(String),
    Charts,
}

/// Track-job key for a store song (album id + normalised title).
fn song_key(album_id: u64, title: &str) -> (String, String) {
    (format!("store:{album_id}"), crate::ytdl::norm(title))
}

const CARD: f32 = 172.0;
const CIRCLE: f32 = 150.0;
const SONG_ROW: f32 = 58.0;

impl App {
    fn store_fetch<T: Send + 'static>(&self, ctx: &Context, work: impl FnOnce() -> T + Send + 'static, wrap: impl FnOnce(T) -> StoreMsg + Send + 'static) {
        let (tx, wake) = (self.store_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(wrap(work()));
            wake.request_repaint();
        });
    }

    pub(super) fn store_load(&mut self, ctx: &Context, what: StoreLoad) {
        match what {
            StoreLoad::Search(q) => {
                if q.is_empty() || self.store.searches.contains_key(&q) {
                    return;
                }
                self.store.searches.insert(q.clone(), None);
                let q2 = q.clone();
                self.store_fetch(ctx, move || crate::ytdl::store_search(&q2), move |r| StoreMsg::Search(q, r));
            }
            StoreLoad::Album(id) => {
                if self.store.albums.contains_key(&id) {
                    return;
                }
                self.store.albums.insert(id, None);
                self.store_fetch(ctx, move || crate::ytdl::album_by_id(id), move |a| StoreMsg::Album(id, a));
            }
            StoreLoad::Artist(id) => {
                if self.store.artists.contains_key(&id) {
                    return;
                }
                self.store.artists.insert(id, None);
                self.store_fetch(ctx, move || crate::ytdl::artist_page(id), move |a| StoreMsg::Artist(id, a));
            }
            StoreLoad::Image(id) => {
                if self.store.images.contains_key(&id) {
                    return;
                }
                self.store.images.insert(id, None);
                self.store_fetch(ctx, move || crate::ytdl::artist_image(id), move |a| StoreMsg::Image(id, a));
            }
            StoreLoad::NameId(name) => {
                if self.store.name_ids.contains_key(&name) {
                    return;
                }
                self.store.name_ids.insert(name.clone(), None);
                let n2 = name.clone();
                self.store_fetch(ctx, move || crate::ytdl::artist_id(&n2), move |id| StoreMsg::NameId(name, id));
            }
            StoreLoad::Charts => {
                if self.store.charts.is_some() {
                    return;
                }
                self.store.charts = Some(None);
                self.store_fetch(ctx, crate::ytdl::charts, StoreMsg::Charts);
            }
        }
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
                StoreMsg::Artist(id, a) => {
                    if let Some(img) = a.as_ref().map(|p| p.image.clone()) {
                        self.store.images.insert(id, Some(img));
                    }
                    self.store.artists.insert(id, Some(a));
                }
                StoreMsg::Image(id, url) => {
                    self.store.images.insert(id, Some(url));
                }
                StoreMsg::NameId(name, id) => {
                    self.store.name_ids.insert(name, Some(id));
                }
                StoreMsg::Charts(c) => self.store.charts = Some(c),
            }
        }
    }
}

/// What a Get button shows.
enum GetState {
    Get,
    Busy(Option<f32>),
    Done,
    Failed,
}

impl V<'_> {
    pub(super) fn store(&mut self, ui: &mut Ui) {
        let depth = self.st.store_stack.len();
        self.store_header(ui, depth == 0);
        let page_key = match self.st.store_stack.last() {
            None => format!("root:{}", self.st.store_submitted),
            Some(StorePage::Artist(id)) => format!("artist:{id}"),
            Some(StorePage::Album(id)) => format!("album:{id}"),
            Some(StorePage::AllAlbums(t, _)) => format!("all-albums:{t}"),
            Some(StorePage::AllSongs(t, _)) => format!("all-songs:{t}"),
        };
        let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt(("store", depth, page_key));
        if let Some(off) = std::env::var("MH_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            sa = sa.vertical_scroll_offset(off);
        }
        sa.show(ui, |ui| {
            match self.st.store_stack.last().cloned() {
                Some(StorePage::Artist(id)) => self.store_artist_page(ui, id),
                Some(StorePage::Album(id)) => self.store_album_page(ui, id),
                Some(StorePage::AllAlbums(title, list)) => self.store_all_albums(ui, &title, &list),
                Some(StorePage::AllSongs(title, list)) => self.store_all_songs(ui, &title, &list),
                None if self.st.store_submitted.is_empty() => self.store_home(ui),
                None => self.store_results(ui),
            }
            ui.add_space(36.0);
        });
    }

    fn open(&mut self, page: StorePage) {
        self.st.store_stack.push(page);
    }

    /// Title (root only), back button and the search field.
    fn store_header(&mut self, ui: &mut Ui, root: bool) {
        let h = if root { 128.0 } else { 64.0 };
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let mut x = bar.left() + PAD;
        let field_y = if root {
            ui.painter().text(pos2(x, bar.top() + 22.0), Align2::LEFT_TOP, "Store", bold(30.0), TEXT);
            bar.top() + 76.0
        } else {
            bar.top() + 16.0
        };
        if !root {
            let b = Rect::from_min_size(pos2(x - 6.0, field_y), vec2(36.0, 36.0));
            let r = ui.interact(b, ui.id().with("store-back"), Sense::click());
            let hov = anim(ui, r.id, r.hovered(), 0.12);
            ui.painter().rect_filled(b, cr(18), Color32::from_white_alpha((14.0 * hov) as u8));
            let c = b.center();
            let st = Stroke::new(2.2f32, lerp(DIM, TEXT, hov));
            ui.painter().line_segment([c + vec2(3.0, -7.0), c + vec2(-4.0, 0.0)], st);
            ui.painter().line_segment([c + vec2(-4.0, 0.0), c + vec2(3.0, 7.0)], st);
            if r.on_hover_text("Back").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.st.store_stack.pop();
            }
            x += 40.0;
        }
        let field = Rect::from_min_size(pos2(x, field_y), vec2((bar.right() - PAD - x).min(520.0), 36.0));
        ui.painter().rect_filled(field, cr(10), Color32::from_rgb(38, 38, 44));
        let c = pos2(field.left() + 17.0, field.center().y - 1.5);
        ui.painter().circle_stroke(c, 5.5, Stroke::new(1.6f32, DIM));
        ui.painter().line_segment([c + vec2(4.0, 4.0), c + vec2(8.0, 8.0)], Stroke::new(1.8f32, DIM));
        let mut fui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(field.min + vec2(34.0, 8.0), field.max - vec2(12.0, 6.0))));
        let resp = fui.add(
            egui::TextEdit::singleline(&mut self.st.store_query)
                .id(Id::new("store-query"))
                .hint_text(egui::RichText::new("Artists, albums, songs").color(FAINT))
                .font(font(15.0))
                .frame(false)
                .desired_width(f32::INFINITY),
        );
        if resp.changed() {
            self.st.store_typed_at = self.now;
        }
        if std::mem::take(&mut self.st.store_focus) {
            resp.request_focus();
        }
        let q = self.st.store_query.trim().to_string();
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if q != self.st.store_submitted && (enter || self.now - self.st.store_typed_at > 0.45) {
            // A new search starts from the top.
            self.st.store_submitted = q.clone();
            self.st.store_stack.clear();
            if !q.is_empty() {
                self.acts.push(Action::StoreLoad(StoreLoad::Search(q)));
            }
        } else if q != self.st.store_submitted {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    // ---------------------------------------------------------------- pages

    fn store_home(&mut self, ui: &mut Ui) {
        // Artists you listen to most, then Apple's charts.
        let mut by_artist: HashMap<String, f64> = HashMap::new();
        for t in &self.lib.data.tracks {
            *by_artist.entry(t.album_artist.clone()).or_default() += t.listened_secs + 1.0;
        }
        let mut top: Vec<(String, f64)> = by_artist.into_iter().filter(|(a, _)| !a.is_empty() && a != "Various Artists").collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut yours = vec![];
        for (name, _) in top.iter().take(14) {
            match self.store.name_ids.get(name) {
                None => self.acts.push(Action::StoreLoad(StoreLoad::NameId(name.clone()))),
                Some(Some(Some(id))) => yours.push(StoreArtist { id: *id, name: name.clone() }),
                _ => {}
            }
        }
        if !yours.is_empty() {
            self.artist_shelf(ui, "Artists You Play", &yours);
        }
        match &self.store.charts {
            None => self.acts.push(Action::StoreLoad(StoreLoad::Charts)),
            Some(None) => spinner_block(ui),
            Some(Some(c)) => {
                let c = c.clone();
                if !c.albums.is_empty() {
                    self.album_shelf(ui, "top-albums", "Top Albums", &c.albums, true);
                }
                if !c.songs.is_empty() {
                    self.song_grid(ui, "top-songs", "Top Songs", &c.songs, true);
                }
            }
        }
    }

    fn store_results(&mut self, ui: &mut Ui) {
        let q = self.st.store_submitted.clone();
        let res = match self.store.searches.get(&q) {
            None => {
                self.acts.push(Action::StoreLoad(StoreLoad::Search(q)));
                return spinner_block(ui);
            }
            Some(None) => return spinner_block(ui),
            Some(Some(None)) => return note_block(ui, "Couldn't reach the catalogue. Check your internet connection."),
            Some(Some(Some(r))) => r.clone(),
        };
        if res.artists.is_empty() && res.albums.is_empty() && res.songs.is_empty() {
            return note_block(ui, &format!("No results for “{q}”."));
        }
        if !res.artists.is_empty() {
            self.artist_shelf(ui, "Artists", &res.artists);
        }
        if !res.albums.is_empty() {
            self.album_shelf(ui, "res-albums", "Albums", &res.albums, true);
        }
        if !res.songs.is_empty() {
            self.song_grid(ui, "res-songs", "Songs", &res.songs, true);
        }
    }

    fn store_artist_page(&mut self, ui: &mut Ui, id: u64) {
        let page = match self.store.artists.get(&id) {
            None => {
                self.acts.push(Action::StoreLoad(StoreLoad::Artist(id)));
                return spinner_block(ui);
            }
            Some(None) => return spinner_block(ui),
            Some(Some(None)) => return note_block(ui, "Couldn't load this artist."),
            Some(Some(Some(p))) => p.clone(),
        };
        // Hero: the artist's photo, round, over a blurred wash of itself
        // (Apple's artist images are round cut-outs), name beside it.
        let w = ui.available_width();
        let hero_h = 300.0;
        let (hero, _) = ui.allocate_exact_size(vec2(w, hero_h), Sense::hover());
        let hero = hero.shrink2(vec2(PAD, 0.0));
        let photo = page.image.as_ref().map(|b| crate::ytdl::sized(b, "600x600cc.jpg"));
        let mut drew = false;
        if let Some(url) = &photo {
            if let Art::Ready { blur, .. } = self.art.get_url(url) {
                // Cover-fit the square blur into the wide banner.
                let f = hero.height() / hero.width();
                let uv = Rect::from_min_max(pos2(0.0, 0.5 - f / 2.0), pos2(1.0, 0.5 + f / 2.0));
                egui::Image::new((blur.id(), hero.size())).uv(uv).corner_radius(cr(14)).paint_at(ui, hero);
                drew = true;
            }
        }
        if !drew {
            ui.painter().rect_filled(hero, cr(14), lerp(Color32::from_rgb(30, 30, 36), accent(), 0.25));
        }
        ui.painter().rect_filled(hero, cr(14), Color32::from_black_alpha(90));
        let d = hero_h - 72.0;
        let circ = Rect::from_min_size(pos2(hero.left() + 36.0, hero.center().y - d / 2.0), vec2(d, d));
        shadow(ui, circ, (d / 2.0) as u8, 30, 10, 0.5);
        match &photo {
            Some(u) => self.remote_art(ui, Some(u), &page.name, circ, (d / 2.0) as u8),
            None => {
                ui.painter().circle_filled(circ.center(), d / 2.0, Color32::from_rgb(44, 44, 52));
                let initials: String = page.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
                ui.painter().text(circ.center(), Align2::CENTER_CENTER, initials.to_uppercase(), bold(64.0), DIM);
            }
        }
        let nx = circ.right() + 36.0;
        let nw = hero.right() - nx - 24.0;
        if let Some(g) = &page.genre {
            text(ui, pos2(nx, hero.center().y - 44.0), &g.to_uppercase(), bold(12.0), Color32::from_white_alpha(210), nw);
        }
        text(ui, pos2(nx, hero.center().y - 24.0), &page.name, bold(44.0), Color32::WHITE, nw);
        let counts = format!(
            "{} album{} · {} single{} & EPs",
            page.albums.len(),
            if page.albums.len() == 1 { "" } else { "s" },
            page.singles.len(),
            if page.singles.len() == 1 { "" } else { "s" }
        );
        text(ui, pos2(nx, hero.center().y + 34.0), &counts, font(14.0), Color32::from_white_alpha(200), nw);
        if self.lib.artists.contains_key(&page.name) {
            let b = Rect::from_min_size(pos2(nx, hero.center().y + 66.0), vec2(148.0, 34.0));
            let r = ui.interact(b, ui.id().with("in-lib"), Sense::click());
            ui.painter().rect_filled(b, cr(17), Color32::from_white_alpha(if r.hovered() { 60 } else { 34 }));
            ui.painter().text(b.center(), Align2::CENTER_CENTER, "In Your Library", bold(12.5), Color32::WHITE);
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Action::Go(View::Artists(Some(page.name.clone()))));
            }
        }
        ui.add_space(8.0);
        if !page.top_songs.is_empty() {
            self.song_grid(ui, &format!("artist-top-{id}"), "Top Songs", &page.top_songs, true);
        }
        if !page.albums.is_empty() {
            self.album_shelf(ui, &format!("artist-albums-{id}"), "Albums", &page.albums, true);
        }
        if !page.singles.is_empty() {
            self.album_shelf(ui, &format!("artist-singles-{id}"), "Singles & EPs", &page.singles, true);
        }
    }

    fn store_album_page(&mut self, ui: &mut Ui, id: u64) {
        let album = match self.store.albums.get(&id) {
            None => {
                self.acts.push(Action::StoreLoad(StoreLoad::Album(id)));
                return spinner_block(ui);
            }
            Some(None) => return spinner_block(ui),
            Some(Some(None)) => return note_block(ui, "Couldn't load this album."),
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
        // Header: cover left; title, artist link, genre · year, Get.
        let w = ui.available_width();
        let art_size = (w * 0.26).clamp(170.0, 270.0);
        let (head, _) = ui.allocate_exact_size(vec2(w, art_size + 30.0), Sense::hover());
        let art = Rect::from_min_size(pos2(head.left() + PAD, head.top() + 8.0), vec2(art_size, art_size));
        shadow(ui, art, 8, 34, 12, 0.55);
        self.remote_art(ui, r.cover_url.as_deref(), &r.title, art, 8);
        let tx = art.right() + 30.0;
        let tw = head.right() - tx - PAD;
        let ty = art.bottom() - 150.0;
        let galley = ui.painter().layout(r.title.clone(), bold(28.0), TEXT, tw);
        let th = galley.size().y.min(70.0);
        ui.painter().galley(pos2(tx, ty - th + 34.0), galley, TEXT);
        let ar = text(ui, pos2(tx, ty + 42.0), &r.artist, font(20.0), accent(), tw);
        if let Some(aid) = album.artist_id {
            if ui.interact(ar, ui.id().with("album-artist"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.open(StorePage::Artist(aid));
            }
        }
        let meta = [album.genre.clone(), album.year.map(|y| y.to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · ").to_uppercase();
        text(ui, pos2(tx, ty + 72.0), &meta, bold(11.5), DIM, tw);
        let owned = self.owned_album(&r.title, &r.artist);
        let job = self.album_jobs.get(&id).and_then(|j| self.jobs.iter().find(|x| x.id == *j));
        let brow = Rect::from_min_size(pos2(tx, ty + 104.0), vec2(tw, 40.0));
        match job {
            Some(j) if !j.done => {
                let label = match j.progress {
                    Some(p) => format!("{} · {:.0}%", j.status, p * 100.0),
                    None => j.status.clone(),
                };
                ring(ui, pos2(brow.left() + 14.0, brow.center().y), 11.0, j.progress);
                text(ui, pos2(brow.left() + 34.0, brow.center().y - 9.0), &label, font(13.0), DIM, tw - 40.0);
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            _ => {
                let label = if owned { "Get Missing Songs" } else { "Get Album" };
                let b = Rect::from_min_size(brow.min, vec2(if owned { 176.0 } else { 128.0 }, 36.0));
                let resp = ui.interact(b, ui.id().with("get-album"), Sense::click());
                let hov = anim(ui, resp.id, resp.hovered(), 0.12);
                ui.painter().rect_filled(b, cr(8), lerp(accent(), Color32::WHITE, 0.12 * hov));
                ui.painter().text(b.center(), Align2::CENTER_CENTER, label, bold(13.5), on_accent());
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.acts.push(Action::DownloadCatalogAlbum(r.clone()));
                }
                if owned {
                    text(ui, pos2(b.right() + 14.0, b.center().y - 8.0), "In your library", font(12.5), FAINT, 200.0);
                }
            }
        }
        ui.add_space(14.0);
        let multi_disc = album.tracks.iter().any(|t| t.disc > 1);
        let mut disc = 0;
        for (i, t) in album.tracks.iter().enumerate() {
            if multi_disc && t.disc != disc {
                disc = t.disc;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(PAD);
                    ui.label(egui::RichText::new(format!("Disc {disc}")).font(bold(13.0)).color(DIM));
                });
            }
            let s = StoreSong {
                track: t.clone(),
                album: album.title.clone(),
                album_artist: album.artist.clone(),
                album_id: id,
                year: album.year,
                genre: album.genre.clone(),
                cover_url: album.cover_url.clone(),
            };
            self.track_row(ui, &s, i);
        }
        // Footer: release date, length, copyright.
        let mins = album.tracks.iter().map(|t| t.duration_ms).sum::<u64>() / 60_000;
        let length = if mins >= 60 { format!("{} hour{} {} minutes", mins / 60, if mins >= 120 { "s" } else { "" }, mins % 60) } else { format!("{mins} minutes") };
        ui.add_space(16.0);
        for line in [
            album.release.as_deref().map(long_date),
            Some(format!("{} song{}, {length}", album.tracks.len(), if album.tracks.len() == 1 { "" } else { "s" })),
            album.copyright.clone(),
        ]
        .into_iter()
        .flatten()
        {
            ui.horizontal(|ui| {
                ui.add_space(PAD);
                ui.label(egui::RichText::new(line).size(12.0).color(FAINT));
            });
        }
    }

    fn store_all_albums(&mut self, ui: &mut Ui, title: &str, list: &[CatalogAlbumRef]) {
        heading(ui, title);
        ui.add_space(10.0);
        let w = ui.available_width() - PAD * 2.0;
        let cols = (((w + TILE_GAP) / (CARD + TILE_GAP)).floor() as usize).max(1);
        let tile = (w - TILE_GAP * (cols - 1) as f32) / cols as f32;
        for chunk in list.chunks(cols) {
            let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), tile + 58.0), Sense::hover());
            if !ui.is_rect_visible(strip) {
                continue;
            }
            for (k, r) in chunk.iter().enumerate() {
                let rect = Rect::from_min_size(pos2(strip.left() + PAD + k as f32 * (tile + TILE_GAP), strip.top()), vec2(tile, tile));
                self.album_card(ui, r, rect);
            }
        }
    }

    fn store_all_songs(&mut self, ui: &mut Ui, title: &str, list: &[StoreSong]) {
        heading(ui, title);
        ui.add_space(10.0);
        let w = ui.available_width() - PAD * 2.0;
        for s in list {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), SONG_ROW), Sense::hover());
            if ui.is_rect_visible(row) {
                self.song_cell(ui, s, Rect::from_min_size(pos2(row.left() + PAD, row.top()), vec2(w, SONG_ROW)));
            }
        }
    }

    // ---------------------------------------------------------------- shelves

    /// "Title ›" (clickable when there's a See All) plus ‹ › scroll arrows.
    /// Returns (see-all clicked, scroll direction).
    fn shelf_header(&mut self, ui: &mut Ui, id: &str, title: &str, see_all: bool, arrows: bool) -> (bool, f32) {
        ui.add_space(18.0);
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let tr = ui.painter().text(pos2(bar.left() + PAD, bar.center().y), Align2::LEFT_CENTER, title, bold(20.0), TEXT);
        let mut all = false;
        if see_all {
            let hit = Rect::from_min_max(tr.min - vec2(4.0, 4.0), tr.max + vec2(22.0, 4.0));
            let r = ui.interact(hit, Id::new(("see-all", id)), Sense::click());
            let c = pos2(tr.right() + 12.0, bar.center().y);
            let col = if r.hovered() { TEXT } else { DIM };
            let st = Stroke::new(2.0f32, col);
            ui.painter().line_segment([c + vec2(-3.0, -6.0), c + vec2(3.0, 0.0)], st);
            ui.painter().line_segment([c + vec2(3.0, 0.0), c + vec2(-3.0, 6.0)], st);
            all = r.on_hover_text("See All").on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        }
        let mut dir = 0.0;
        if arrows {
            for (k, d) in [(1.0f32, -1.0f32), (0.0, 1.0)] {
                let c = pos2(bar.right() - PAD - 14.0 - k * 34.0, bar.center().y);
                let b = Rect::from_center_size(c, vec2(28.0, 28.0));
                let r = ui.interact(b, Id::new(("shelf-arrow", id, k as i32)), Sense::click());
                let hov = anim(ui, r.id, r.hovered(), 0.12);
                ui.painter().circle_filled(c, 14.0, Color32::from_white_alpha((10.0 + 20.0 * hov) as u8));
                let st = Stroke::new(2.0f32, lerp(DIM, TEXT, hov));
                ui.painter().line_segment([c + vec2(-2.5 * d, -5.5), c + vec2(3.0 * d, 0.0)], st);
                ui.painter().line_segment([c + vec2(3.0 * d, 0.0), c + vec2(-2.5 * d, 5.5)], st);
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    dir = d;
                }
            }
        }
        ui.add_space(6.0);
        (all, dir)
    }

    /// A row of album cards you scroll sideways (arrows or trackpad).
    fn album_shelf(&mut self, ui: &mut Ui, id: &str, title: &str, albums: &[CatalogAlbumRef], see_all: bool) {
        // Whole cards always fit the width: no half-cut card at the edge.
        let avail = ui.available_width() - PAD * 2.0;
        let fits = (((avail + TILE_GAP) / (CARD + TILE_GAP)).floor() as usize).max(1);
        let card = (avail - TILE_GAP * (fits - 1) as f32) / fits as f32;
        let overflow = albums.len() > fits;
        let (all, dir) = self.shelf_header(ui, id, title, see_all && overflow, overflow);
        if all {
            self.open(StorePage::AllAlbums(title.to_string(), albums.to_vec()));
        }
        let page = (fits as f32) * (card + TILE_GAP);
        {
            // Hide whatever scrolls into the right margin (no half-cut cards).
            let mut clip = ui.clip_rect();
            clip.set_right(ui.max_rect().right() - PAD + 4.0);
            ui.set_clip_rect(clip);
        }
        egui::ScrollArea::horizontal()
            .id_salt(("shelf", id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                if dir != 0.0 {
                    ui.scroll_with_delta(vec2(-dir * page, 0.0));
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    ui.add_space(PAD);
                    for r in albums {
                        let (cell, _) = ui.allocate_exact_size(vec2(card, card + 56.0), Sense::hover());
                        if ui.is_rect_visible(cell) {
                            self.album_card(ui, r, Rect::from_min_size(cell.min, vec2(card, card)));
                        }
                    }
                    ui.add_space(PAD);
                });
            });
    }

    /// Round artist photos.
    fn artist_shelf(&mut self, ui: &mut Ui, title: &str, artists: &[StoreArtist]) {
        let avail = ui.available_width() - PAD * 2.0;
        let fits = (((avail + TILE_GAP) / (CIRCLE + TILE_GAP)).floor() as usize).max(1);
        let circle = (avail - TILE_GAP * (fits - 1) as f32) / fits as f32;
        let overflow = artists.len() > fits;
        let id = format!("artists-{title}");
        let (_, dir) = self.shelf_header(ui, &id, title, false, overflow);
        let page = (fits as f32) * (circle + TILE_GAP);
        {
            // Hide whatever scrolls into the right margin (no half-cut cards).
            let mut clip = ui.clip_rect();
            clip.set_right(ui.max_rect().right() - PAD + 4.0);
            ui.set_clip_rect(clip);
        }
        egui::ScrollArea::horizontal()
            .id_salt(("shelf", &id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                if dir != 0.0 {
                    ui.scroll_with_delta(vec2(-dir * page, 0.0));
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    ui.add_space(PAD);
                    for a in artists {
                        let (cell, resp) = ui.allocate_exact_size(vec2(circle, circle + 40.0), Sense::click());
                        if !ui.is_rect_visible(cell) {
                            continue;
                        }
                        let hov = anim(ui, resp.id, resp.hovered(), 0.18);
                        let circ = Rect::from_min_size(cell.min, vec2(circle, circle)).expand(3.0 * hov);
                        shadow(ui, circ, (circle / 2.0) as u8, 18, 6, 0.3 + 0.2 * hov);
                        let url = match self.store.images.get(&a.id) {
                            None => {
                                self.acts.push(Action::StoreLoad(StoreLoad::Image(a.id)));
                                None
                            }
                            Some(Some(Some(base))) => Some(crate::ytdl::sized(base, "400x400cc.jpg")),
                            _ => None,
                        };
                        match url {
                            Some(u) => self.remote_art(ui, Some(&u), &a.name, circ, (circ.width() / 2.0) as u8),
                            None => {
                                ui.painter().circle_filled(circ.center(), circ.width() / 2.0, Color32::from_rgb(44, 44, 52));
                                let initials: String = a.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
                                ui.painter().text(circ.center(), Align2::CENTER_CENTER, initials.to_uppercase(), bold(40.0), DIM);
                            }
                        }
                        let galley = ui.painter().layout(a.name.clone(), font(13.5), TEXT, circle);
                        let gw = galley.size().x.min(circle);
                        ui.painter().with_clip_rect(cell.expand(2.0)).galley(pos2(cell.center().x - gw / 2.0, cell.top() + circle + 10.0), galley, TEXT);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.open(StorePage::Artist(a.id));
                        }
                    }
                    ui.add_space(PAD);
                });
            });
    }

    /// Songs in columns of four rows, scrolled sideways like Apple Music.
    fn song_grid(&mut self, ui: &mut Ui, id: &str, title: &str, songs: &[StoreSong], see_all: bool) {
        let avail = ui.available_width() - PAD * 2.0;
        let per_col = 4;
        let visible_cols = if avail > 1100.0 { 3 } else if avail > 640.0 { 2 } else { 1 };
        let col_w = (avail - TILE_GAP * (visible_cols - 1) as f32) / visible_cols as f32;
        let cols = songs.len().div_ceil(per_col).min(6);
        let overflow = cols > visible_cols;
        let (all, dir) = self.shelf_header(ui, id, title, see_all && songs.len() > per_col * visible_cols, overflow);
        if all {
            self.open(StorePage::AllSongs(title.to_string(), songs.to_vec()));
        }
        let page = visible_cols as f32 * (col_w + TILE_GAP);
        {
            // Hide whatever scrolls into the right margin (no half-cut cards).
            let mut clip = ui.clip_rect();
            clip.set_right(ui.max_rect().right() - PAD + 4.0);
            ui.set_clip_rect(clip);
        }
        egui::ScrollArea::horizontal()
            .id_salt(("grid", id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                if dir != 0.0 {
                    ui.scroll_with_delta(vec2(-dir * page, 0.0));
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    ui.add_space(PAD);
                    for c in 0..cols {
                        let (col, _) = ui.allocate_exact_size(vec2(col_w, SONG_ROW * per_col as f32), Sense::hover());
                        if !ui.is_rect_visible(col) {
                            continue;
                        }
                        for r in 0..per_col {
                            let Some(s) = songs.get(c * per_col + r) else { break };
                            let cell = Rect::from_min_size(pos2(col.left(), col.top() + r as f32 * SONG_ROW), vec2(col_w, SONG_ROW));
                            self.song_cell(ui, s, cell);
                            if r + 1 < per_col && songs.get(c * per_col + r + 1).is_some() {
                                ui.painter().hline(cell.left() + 56.0..=cell.right(), cell.bottom(), Stroke::new(1.0f32, LINE));
                            }
                        }
                    }
                    ui.add_space(PAD);
                });
            });
    }

    // ---------------------------------------------------------------- cells

    fn album_card(&mut self, ui: &mut Ui, r: &CatalogAlbumRef, art: Rect) {
        let owned = self.owned_album(&r.title, &r.artist);
        let state = self.album_state(r.id);
        let resp = ui.interact(art, Id::new(("store-card", r.id, art.left() as i32, art.top() as i32)), Sense::click());
        let h = anim(ui, resp.id, resp.hovered(), 0.18);
        shadow(ui, art, 8, (12.0 + 14.0 * h) as u8, (4.0 + 4.0 * h) as i8, 0.35 + 0.2 * h);
        self.remote_art(ui, r.cover_url.as_deref(), &r.title, art, 8);
        let mut get_hit = false;
        match state {
            GetState::Busy(p) => {
                ui.painter().rect_filled(art, cr(8), Color32::from_black_alpha(120));
                ring(ui, art.center(), 18.0, p);
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            _ if h > 0.001 => {
                ui.painter().rect_filled(art, cr(8), Color32::from_black_alpha((60.0 * h) as u8));
                // Get button, bottom-left (where Apple puts Play).
                let c = pos2(art.left() + 24.0, art.bottom() - 24.0);
                get_hit = ui.rect_contains_pointer(Rect::from_center_size(c, vec2(34.0, 34.0)));
                ui.painter().circle_filled(c, 16.0 * (0.7 + 0.3 * h), alpha(if get_hit { accent() } else { Color32::from_rgb(28, 28, 32) }, 0.92 * h));
                let a = alpha(if get_hit { on_accent() } else { Color32::WHITE }, h);
                let st = Stroke::new(2.0f32, a);
                ui.painter().line_segment([c + vec2(0.0, -6.5), c + vec2(0.0, 5.0)], st);
                ui.painter().line_segment([c + vec2(-5.0, 0.5), c + vec2(0.0, 5.5)], st);
                ui.painter().line_segment([c + vec2(5.0, 0.5), c + vec2(0.0, 5.5)], st);
            }
            _ => {}
        }
        if owned && !matches!(state, GetState::Busy(_)) {
            let c = pos2(art.right() - 19.0, art.top() + 19.0);
            ui.painter().circle_filled(c, 11.0, Color32::from_black_alpha(170));
            check(ui, c, 4.5, Color32::WHITE);
        }
        text(ui, pos2(art.left(), art.bottom() + 9.0), &r.title, font(13.0), TEXT, art.width());
        let sub = match r.year {
            Some(y) => format!("{} · {y}", r.artist),
            None => r.artist.clone(),
        };
        text(ui, pos2(art.left(), art.bottom() + 27.0), &sub, font(12.0), DIM, art.width());
        let tip = if owned { "In your library — Get adds any missing songs" } else { "Click to open · ⬇ gets the whole album" };
        let resp = resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            if get_hit {
                self.acts.push(Action::DownloadCatalogAlbum(r.clone()));
            } else {
                self.open(StorePage::Album(r.id));
            }
        }
    }

    /// Song with cover: title, artist · album · year, GET.
    fn song_cell(&mut self, ui: &mut Ui, s: &StoreSong, cell: Rect) {
        let resp = ui.interact(cell, Id::new(("store-song", s.album_id, &s.track.title, cell.left() as i32, cell.top() as i32)), Sense::click());
        let hov = anim(ui, resp.id, resp.hovered(), 0.12);
        if hov > 0.0 {
            ui.painter().rect_filled(cell.shrink2(vec2(-6.0, 2.0)), cr(8), Color32::from_white_alpha((10.0 * hov) as u8));
        }
        let art = Rect::from_min_size(pos2(cell.left(), cell.center().y - 22.0), vec2(44.0, 44.0));
        self.remote_art(ui, s.cover_url.as_deref(), &s.album, art, 5);
        let tx = art.right() + 12.0;
        let tw = cell.right() - tx - 80.0;
        text(ui, pos2(tx, cell.center().y - 18.0), &s.track.title, font(13.5), TEXT, tw);
        let sub = match s.year {
            Some(y) if !s.album.is_empty() => format!("{} · {} · {y}", s.track.artist, s.album),
            _ => s.track.artist.clone(),
        };
        text(ui, pos2(tx, cell.center().y + 1.0), &sub, font(12.0), DIM, tw);
        let btn = Rect::from_min_size(pos2(cell.right() - 64.0, cell.center().y - 13.0), vec2(60.0, 26.0));
        let state = self.song_state(s);
        let clicked_get = self.get_button(ui, btn, &state);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if clicked_get {
            self.acts.push(Action::StoreGetSong(s.clone()));
        } else if resp.clicked() && s.album_id != 0 && !ui.rect_contains_pointer(btn) {
            self.open(StorePage::Album(s.album_id));
        }
    }

    /// Album tracklist row: number, title (+ artist when different), length, GET.
    fn track_row(&mut self, ui: &mut Ui, s: &StoreSong, i: usize) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        if !ui.is_rect_visible(row) {
            return;
        }
        let row = row.shrink2(vec2(PAD - 10.0, 0.0));
        self.row_bg(ui, row, resp.id, resp.hovered(), false, i);
        ui.painter().text(pos2(row.left() + 24.0, row.center().y), Align2::CENTER_CENTER, s.track.track.to_string(), font(13.0), FAINT);
        let tx = row.left() + 52.0;
        let tw = row.right() - tx - 180.0;
        if s.track.artist != s.album_artist {
            text(ui, pos2(tx, row.center().y - 16.0), &s.track.title, font(14.0), TEXT, tw);
            text(ui, pos2(tx, row.center().y + 1.0), &s.track.artist, font(12.0), DIM, tw);
        } else {
            text(ui, pos2(tx, row.center().y - 9.0), &s.track.title, font(14.0), TEXT, tw);
        }
        text_right(ui, pos2(row.right() - 88.0, row.center().y - 8.0), &fmt_time(s.track.duration_ms as f64 / 1000.0), font(13.0), FAINT);
        let btn = Rect::from_min_size(pos2(row.right() - 72.0, row.center().y - 13.0), vec2(60.0, 26.0));
        let state = self.song_state(s);
        if self.get_button(ui, btn, &state) {
            self.acts.push(Action::StoreGetSong(s.clone()));
        }
    }

    /// App Store–style GET pill; progress ring while downloading.
    fn get_button(&mut self, ui: &mut Ui, btn: Rect, state: &GetState) -> bool {
        let resp = ui.interact(btn, ui.id().with(("get", btn.min.x as i32, btn.min.y as i32)), Sense::click());
        match state {
            GetState::Busy(p) => {
                ring(ui, btn.center(), 10.0, *p);
                ui.ctx().request_repaint_after(Duration::from_millis(150));
                false
            }
            GetState::Done => {
                check(ui, btn.center(), 5.0, FAINT);
                false
            }
            GetState::Get | GetState::Failed => {
                let hov = anim(ui, resp.id, resp.hovered(), 0.12);
                ui.painter().rect_filled(btn, cr(13), lerp(Color32::from_rgb(44, 44, 52), accent(), hov));
                let (label, col) = match state {
                    GetState::Failed => ("RETRY", Color32::from_rgb(255, 140, 120)),
                    _ => ("GET", accent()),
                };
                ui.painter().text(btn.center(), Align2::CENTER_CENTER, label, bold(12.0), lerp(col, on_accent(), hov));
                let tip = if matches!(state, GetState::Failed) { "Couldn't download it — try again" } else { "Find it on YouTube and add it to your library" };
                resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
            }
        }
    }

    // ---------------------------------------------------------------- helpers

    fn song_state(&self, s: &StoreSong) -> GetState {
        let job = self.track_jobs.get(&song_key(s.album_id, &s.track.title)).and_then(|id| self.jobs.iter().find(|j| j.id == *id));
        match job {
            Some(j) if !j.done => GetState::Busy(j.progress),
            Some(j) if j.failed => GetState::Failed,
            Some(j) if !j.cancelled => GetState::Done,
            _ if self.owned_song(&s.track.title, &s.track.artist) => GetState::Done,
            _ => GetState::Get,
        }
    }

    fn album_state(&self, id: u64) -> GetState {
        match self.album_jobs.get(&id).and_then(|j| self.jobs.iter().find(|x| x.id == *j)) {
            Some(j) if !j.done => GetState::Busy(j.progress),
            Some(j) if j.failed => GetState::Failed,
            _ => GetState::Get,
        }
    }

    /// Catalogue artwork (cached on disk).
    fn remote_art(&mut self, ui: &Ui, url: Option<&str>, seed: &str, rect: Rect, radius: u8) {
        if let Some(url) = url {
            match self.art.get_url(url) {
                Art::Ready { tex, .. } => {
                    egui::Image::new((tex.id(), rect.size())).corner_radius(cr(radius)).paint_at(ui, rect);
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
        if b.is_empty() || a.is_empty() {
            return false;
        }
        self.lib.albums.iter().any(|x| {
            let xa = crate::ytdl::norm(&x.artist);
            !xa.is_empty() && crate::ytdl::base_album(&x.title) == b && (xa == a || xa.contains(&a) || a.contains(&xa) || xa == "variousartists")
        })
    }

    fn owned_song(&self, title: &str, artist: &str) -> bool {
        let a = crate::ytdl::norm(artist.split([',', '&']).next().unwrap_or(artist));
        self.lib.data.tracks.iter().any(|t| crate::ytdl::same_song(&t.title, title) && crate::ytdl::norm(&t.artist).contains(&a))
    }
}

fn spinner_block(ui: &mut Ui) {
    ui.add_space(50.0);
    ui.vertical_centered(|ui| ui.spinner());
}

fn note_block(ui: &mut Ui, msg: &str) {
    ui.add_space(50.0);
    ui.vertical_centered(|ui| ui.label(egui::RichText::new(msg).color(DIM)));
}

/// Download progress ring (spins while the percentage isn't known yet).
fn ring(ui: &Ui, c: Pos2, r: f32, p: Option<f32>) {
    ui.painter().circle_stroke(c, r, Stroke::new(2.5f32, Color32::from_white_alpha(30)));
    let (start, sweep) = match p {
        Some(p) => (-std::f32::consts::FRAC_PI_2, p.clamp(0.02, 1.0) * std::f32::consts::TAU),
        None => (ui.input(|i| i.time) as f32 * 5.0, 1.4),
    };
    let pts: Vec<Pos2> = (0..=32)
        .map(|i| {
            let a = start + sweep * i as f32 / 32.0;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.5f32, accent())));
    if p.is_none() {
        ui.ctx().request_repaint();
    }
}

fn check(ui: &Ui, c: Pos2, s: f32, color: Color32) {
    let st = Stroke::new(2.0f32, color);
    ui.painter().line_segment([c + vec2(-s, 0.0), c + vec2(-s * 0.3, s * 0.7)], st);
    ui.painter().line_segment([c + vec2(-s * 0.3, s * 0.7), c + vec2(s, -s * 0.7)], st);
}

/// "2016-08-20" → "August 20, 2016".
fn long_date(d: &str) -> String {
    const M: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut it = d.split('-');
    match (it.next(), it.next().and_then(|m| m.parse::<usize>().ok()), it.next().and_then(|d| d.parse::<u32>().ok())) {
        (Some(y), Some(m), Some(day)) if (1..=12).contains(&m) => format!("{} {day}, {y}", M[m - 1]),
        _ => d.to_string(),
    }
}
