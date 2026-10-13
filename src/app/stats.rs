//! The Listening page: totals, the last 30 days, top songs/artists/albums.

use super::*;

impl V<'_> {
    /// Listening stats: totals, the last 30 days, and your top songs,
    /// artists and albums by time actually listened.
    pub(super) fn stats(&mut self, ui: &mut Ui) {
        let tracks = &self.lib.data.tracks;
        let total: f64 = tracks.iter().map(|t| t.listened_secs).sum();
        let plays: u32 = tracks.iter().map(|t| t.play_count).sum();
        let loved = tracks.iter().filter(|t| t.loved).count();
        let today = library::today();
        let daily = self.lib.data.daily.clone();
        let day = |d: i64| daily.get(&d).copied().unwrap_or(0.0);
        let week: f64 = (0..7).map(|i| day(today - i)).sum();

        let mut top_songs: Vec<(TrackId, f64)> = tracks.iter().filter(|t| t.listened_secs >= 30.0).map(|t| (t.id, t.listened_secs)).collect();
        top_songs.sort_by(|a, b| b.1.total_cmp(&a.1));
        top_songs.truncate(10);
        let mut by_artist: HashMap<String, f64> = HashMap::new();
        for t in tracks {
            *by_artist.entry(t.album_artist.clone()).or_default() += t.listened_secs;
        }
        let mut top_artists: Vec<(String, f64)> = by_artist.into_iter().filter(|(_, s)| *s >= 30.0).collect();
        top_artists.sort_by(|a, b| b.1.total_cmp(&a.1));
        top_artists.truncate(10);
        let mut top_albums: Vec<(usize, f64)> = self
            .lib
            .albums
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.tracks.iter().filter_map(|id| self.lib.track(*id)).map(|t| t.listened_secs).sum::<f64>()))
            .filter(|(_, s)| *s >= 60.0)
            .collect();
        top_albums.sort_by(|a, b| b.1.total_cmp(&a.1));

        let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt("stats");
        if let Some(off) = std::env::var("MH_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            sa = sa.vertical_scroll_offset(off);
        }
        sa.show(ui, |ui| {
            let w = ui.available_width() - PAD * 2.0;
            let x0 = ui.max_rect().left() + PAD;
            let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::hover());
            ui.painter().text(pos2(x0, head.top() + 22.0), Align2::LEFT_TOP, "Listening", bold(30.0), TEXT);

            // Summary tiles.
            let tiles = [
                ("Time listened", long_dur(total)),
                ("This week", if daily.is_empty() { "—".into() } else { long_dur(week) }),
                ("Plays", plays.to_string()),
                ("Loved songs", loved.to_string()),
            ];
            let cols = if w > 480.0 { 4 } else { 2 };
            let tw = (w - 12.0 * (cols - 1) as f32) / cols as f32;
            let rows = tiles.len().div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * 80.0), Sense::hover());
            for (i, (label, value)) in tiles.iter().enumerate() {
                let r = Rect::from_min_size(pos2(x0 + (i % cols) as f32 * (tw + 12.0), area.top() + (i / cols) as f32 * 80.0), vec2(tw, 68.0));
                ui.painter().rect_filled(r, cr(12), Color32::from_rgb(30, 30, 35));
                ui.painter().text(r.left_top() + vec2(14.0, 12.0), Align2::LEFT_TOP, *label, font(12.0), DIM);
                ui.painter().text(r.left_top() + vec2(14.0, 31.0), Align2::LEFT_TOP, value, bold(22.0), if i == 0 { accent() } else { TEXT });
            }

            // Last 30 days.
            ui.add_space(18.0);
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 210.0), Sense::hover());
            ui.painter().text(pos2(x0, area.top()), Align2::LEFT_TOP, "Last 30 Days", bold(20.0), TEXT);
            let chart = Rect::from_min_max(pos2(x0, area.top() + 42.0), pos2(x0 + w, area.bottom() - 26.0));
            let max = (0..30).map(|i| day(today - i)).fold(0.0f64, f64::max).max(600.0);
            let bw = chart.width() / 30.0;
            let since = (self.now - self.view_at) as f32;
            let grow = ease((since - 0.1) / 0.6);
            for i in 0..30 {
                let d = today - 29 + i as i64;
                let secs = day(d);
                let x = chart.left() + i as f32 * bw;
                let col = Rect::from_min_max(pos2(x + bw * 0.18, chart.top()), pos2(x + bw * 0.82, chart.bottom()));
                let h = (secs / max) as f32 * chart.height() * grow;
                let resp = ui.interact(col, Id::new(("day", d)), Sense::hover());
                let hov = anim(ui, resp.id, resp.hovered(), 0.12);
                ui.painter().rect_filled(Rect::from_min_max(pos2(col.left(), chart.bottom() - 3.0), col.right_bottom()), cr(2), Color32::from_rgb(38, 38, 44));
                if h > 0.5 {
                    let bar = Rect::from_min_max(pos2(col.left(), chart.bottom() - h.max(3.0)), col.right_bottom());
                    ui.painter().rect_filled(bar, cr(3), lerp(alpha(accent(), if d == today { 1.0 } else { 0.75 }), accent(), hov));
                }
                let (m, dd) = library::month_day(d);
                if i % 7 == 1 || d == today {
                    let label = if d == today { "Today".to_string() } else { format!("{} {dd}", MONTHS[m as usize - 1]) };
                    ui.painter().text(pos2(col.center().x, chart.bottom() + 8.0), Align2::CENTER_TOP, label, font(11.0), FAINT);
                }
                resp.on_hover_text(format!("{} {dd} · {}", MONTHS[m as usize - 1], if secs > 0.0 { long_dur(secs) } else { "nothing".into() }));
            }
            if grow < 1.0 {
                ui.ctx().request_repaint();
            }
            if daily.len() < 3 {
                ui.painter().text(chart.center(), Align2::CENTER_CENTER, "Fills in as you listen", font(13.0), FAINT);
            }

            // Top songs | top artists.
            ui.add_space(20.0);
            let two = w > 760.0;
            let col_w = if two { (w - 40.0) / 2.0 } else { w };
            let row_h = 54.0;
            let list_h = 40.0 + 10.0 * row_h;
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), if two { list_h } else { list_h * 2.0 + 20.0 }), Sense::hover());
            let songs_at = pos2(x0, area.top());
            let artists_at = if two { pos2(x0 + col_w + 40.0, area.top()) } else { pos2(x0, area.top() + list_h + 20.0) };
            ui.painter().text(songs_at, Align2::LEFT_TOP, "Top Songs", bold(20.0), TEXT);
            ui.painter().text(artists_at, Align2::LEFT_TOP, "Top Artists", bold(20.0), TEXT);
            let top_s = top_songs.first().map(|s| s.1).unwrap_or(1.0);
            if top_songs.is_empty() {
                ui.painter().text(songs_at + vec2(0.0, 44.0), Align2::LEFT_TOP, "Play some music and your favourites show up here", font(13.0), FAINT);
            }
            for (i, (id, secs)) in top_songs.iter().enumerate() {
                let Some(t) = self.lib.track(*id).cloned() else { continue };
                let r = Rect::from_min_size(songs_at + vec2(0.0, 40.0 + i as f32 * row_h), vec2(col_w, row_h - 6.0));
                let resp = ui.interact(r, Id::new(("top-song", id)), Sense::click());
                let hov = anim(ui, resp.id, resp.hovered(), 0.12);
                ui.painter().rect_filled(r, cr(8), Color32::from_white_alpha((14.0 * hov) as u8));
                ui.painter().text(pos2(r.left() + 14.0, r.center().y), Align2::CENTER_CENTER, (i + 1).to_string(), bold(14.0), if i < 3 { accent() } else { FAINT });
                let art = Rect::from_min_size(pos2(r.left() + 32.0, r.top() + 4.0), vec2(40.0, 40.0));
                self.track_art(ui, *id, art, 5);
                let tx = art.right() + 12.0;
                let tw = col_w - (tx - r.left()) - 80.0;
                text(ui, pos2(tx, r.top() + 6.0), &t.title, font(14.0), TEXT, tw);
                text(ui, pos2(tx, r.top() + 25.0), &t.artist, font(12.5), DIM, tw);
                text_right(ui, pos2(r.right() - 10.0, r.top() + 8.0), &long_dur(*secs), font(12.5), DIM);
                let bar = Rect::from_min_size(pos2(r.right() - 70.0, r.top() + 30.0), vec2(60.0, 3.0));
                ui.painter().rect_filled(bar, cr(2), Color32::from_white_alpha(18));
                let mut f = bar;
                f.set_right(bar.left() + bar.width() * (*secs / top_s) as f32 * grow);
                ui.painter().rect_filled(f, cr(2), accent());
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    let ids: Vec<TrackId> = top_songs.iter().map(|s| s.0).collect();
                    self.acts.push(Action::Play(ids, i));
                }
            }
            for (i, (name, secs)) in top_artists.iter().enumerate() {
                let r = Rect::from_min_size(artists_at + vec2(0.0, 40.0 + i as f32 * row_h), vec2(col_w, row_h - 6.0));
                let resp = ui.interact(r, Id::new(("top-artist", name)), Sense::click());
                let hov = anim(ui, resp.id, resp.hovered(), 0.12);
                ui.painter().rect_filled(r, cr(8), Color32::from_white_alpha((14.0 * hov) as u8));
                ui.painter().text(pos2(r.left() + 14.0, r.center().y), Align2::CENTER_CENTER, (i + 1).to_string(), bold(14.0), if i < 3 { accent() } else { FAINT });
                let art = Rect::from_min_size(pos2(r.left() + 32.0, r.top() + 4.0), vec2(40.0, 40.0));
                if let Some(a) = self.lib.artists.get(name).and_then(|v| v.first()).map(|&ai| self.lib.albums[ai].clone()) {
                    self.album_art(ui, &a, art, 20);
                }
                let songs = tracks_by(&self.lib, name);
                text(ui, pos2(art.right() + 12.0, r.top() + 6.0), name, font(14.0), TEXT, col_w - 170.0);
                text(ui, pos2(art.right() + 12.0, r.top() + 25.0), &format!("{songs} song{}", if songs == 1 { "" } else { "s" }), font(12.5), DIM, col_w - 170.0);
                text_right(ui, pos2(r.right() - 10.0, r.top() + 15.0), &long_dur(*secs), font(12.5), DIM);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.acts.push(Action::Go(View::Artists(Some(name.clone()))));
                }
            }
            if top_albums.is_empty() {
                ui.add_space(30.0);
                return;
            }

            // Top albums.
            ui.add_space(16.0);
            let cols = (((w + TILE_GAP) / (150.0 + TILE_GAP)).floor() as usize).clamp(2, 8);
            let tile = (w - TILE_GAP * (cols - 1) as f32) / cols as f32;
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0 + tile + 70.0), Sense::hover());
            ui.painter().text(pos2(x0, area.top()), Align2::LEFT_TOP, "Top Albums", bold(20.0), TEXT);
            for (i, (ai, secs)) in top_albums.iter().take(cols).enumerate() {
                let rect = Rect::from_min_size(pos2(x0 + i as f32 * (tile + TILE_GAP), area.top() + 44.0), vec2(tile, tile));
                let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("top-album", ai)).max_rect(rect.expand2(vec2(16.0, 70.0))));
                self.album_tile(&mut cui, *ai, rect);
                text(ui, pos2(rect.left(), rect.bottom() + 48.0), &long_dur(*secs), font(12.0), FAINT, tile);
            }
            ui.add_space(30.0);
        });
    }
}
