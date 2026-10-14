//! Library pages: album grids (with missing catalogue albums), album pages,
//! song lists, artists and search.

use super::*;

impl V<'_> {
    pub(super) fn album_grid(&mut self, ui: &mut Ui, title: &str, albums: &[usize], header: Option<&dyn Fn(&mut Ui)>, missing: &[crate::ytdl::CatalogAlbumRef]) {
        let width = ui.available_width() - PAD * 2.0;
        let min_tile = 172.0;
        let cols = (((width + TILE_GAP) / (min_tile + TILE_GAP)).floor() as usize).max(1);
        let tile = (width - TILE_GAP * (cols - 1) as f32) / cols as f32;
        let row_h = tile + 58.0;
        let rows = albums.len().div_ceil(cols);
        let title_h = if header.is_some() { 128.0 } else { 80.0 };
        let since = (self.now - self.view_at) as f32;
        let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt(("grid", title));
        if let Some(off) = std::env::var("MH_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            sa = sa.vertical_scroll_offset(off);
        }
        sa.show_viewport(ui, |ui, viewport| {
            // "More by…" section: catalogue albums you don't have yet.
            let more_y = title_h + rows as f32 * row_h + 24.0;
            let more_rows = missing.len().div_ceil(cols);
            let more_h = if missing.is_empty() { 0.0 } else { 64.0 + more_rows as f32 * row_h };
            let total_h = title_h + rows as f32 * row_h + 40.0 + more_h;
            let (full, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
            let origin = full.min;
            ui.painter().text(origin + vec2(PAD, 22.0), Align2::LEFT_TOP, title, bold(30.0), TEXT);
            if let Some(h) = header {
                let mut hui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(origin + vec2(PAD, 72.0), vec2(width, 44.0))));
                h(&mut hui);
            }
            let first = (((viewport.top() - title_h) / row_h).floor().max(0.0)) as usize;
            let last = ((((viewport.bottom() - title_h) / row_h).ceil().max(0.0)) as usize + 1).min(rows);
            for r in first..last {
                for c in 0..cols {
                    let idx = r * cols + c;
                    let Some(&ai) = albums.get(idx) else { break };
                    let pos = origin + vec2(PAD + c as f32 * (tile + TILE_GAP), title_h + r as f32 * row_h);
                    // Tiles cascade in after a page change.
                    let k = ease((since - (r + c).min(14) as f32 * 0.035) / 0.4);
                    let rect = Rect::from_min_size(pos + vec2(0.0, (1.0 - k) * 18.0), vec2(tile, tile));
                    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("tile", ai)).max_rect(rect.expand2(vec2(16.0, 70.0))));
                    cui.multiply_opacity(k);
                    self.album_tile(&mut cui, ai, rect);
                    if k < 1.0 {
                        ui.ctx().request_repaint();
                    }
                }
            }
            if !missing.is_empty() {
                let y = origin.y + more_y;
                ui.painter().text(pos2(origin.x + PAD, y), Align2::LEFT_TOP, format!("More by {title}"), bold(20.0), TEXT);
                ui.painter().text(pos2(origin.x + PAD, y + 28.0), Align2::LEFT_TOP, "Not in your library — click one to download it from YouTube", font(13.0), FAINT);
                for (i, r) in missing.iter().enumerate() {
                    let (row, c) = (i / cols, i % cols);
                    let top = more_y + 64.0 + row as f32 * row_h;
                    if top > viewport.bottom() || top + row_h < viewport.top() {
                        continue;
                    }
                    let rect = Rect::from_min_size(origin + vec2(PAD + c as f32 * (tile + TILE_GAP), top), vec2(tile, tile));
                    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("mtile", r.id)).max_rect(rect.expand2(vec2(16.0, 70.0))));
                    self.missing_tile(&mut cui, r, rect);
                }
            }
        });
    }

    /// A greyed-out album you don't own; click to download the whole thing.
    pub(super) fn missing_tile(&mut self, ui: &mut Ui, r: &crate::ytdl::CatalogAlbumRef, art: Rect) {
        let job = self.album_jobs.get(&r.id).and_then(|id| self.jobs.iter().find(|j| j.id == *id));
        let busy = job.map(|j| !j.done).unwrap_or(false);
        let resp = ui.interact(art, Id::new(("mtile", r.id)), Sense::click());
        let h = anim(ui, resp.id, resp.hovered() && !busy, 0.2);
        let grown = art.expand(art.width() * 0.02 * h);
        // Dimmed, like the missing songs on an album page.
        let shade = Color32::from_gray((95.0 + 85.0 * h) as u8);
        let mut drawn = false;
        if let Some(url) = &r.cover_url {
            match self.art.get_url(url) {
                Art::Ready { tex, .. } => {
                    egui::Image::new((tex.id(), grown.size())).corner_radius(cr(10)).tint(shade).paint_at(ui, grown);
                    drawn = true;
                }
                Art::Loading => {
                    placeholder(ui, grown, 10, &r.title, true);
                    drawn = true;
                }
                Art::Missing => {}
            }
        }
        if !drawn {
            placeholder(ui, grown, 10, &r.title, false);
        }
        ui.painter().rect_stroke(grown, cr(10), Stroke::new(1.0_f32, Color32::from_white_alpha(18)), egui::StrokeKind::Inside);
        let center = grown.center();
        match job {
            Some(j) if !j.done => {
                ui.painter().rect_filled(grown, cr(10), Color32::from_black_alpha(110));
                let label = match j.progress {
                    Some(p) => format!("{:.0}%", p * 100.0),
                    None => "Finding songs…".into(),
                };
                ui.painter().text(center - vec2(0.0, 10.0), Align2::CENTER_CENTER, label, bold(15.0), Color32::WHITE);
                ui.painter().text(center + vec2(0.0, 12.0), Align2::CENTER_CENTER, &j.status, font(11.5), DIM);
                if let Some(p) = j.progress {
                    let bar = Rect::from_min_max(pos2(grown.left() + 14.0, grown.bottom() - 14.0), pos2(grown.right() - 14.0, grown.bottom() - 11.0));
                    ui.painter().rect_filled(bar, cr(2), Color32::from_white_alpha(30));
                    let mut f = bar;
                    f.set_right(bar.left() + bar.width() * p);
                    ui.painter().rect_filled(f, cr(2), accent());
                }
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
            Some(j) if j.failed => {
                ui.painter().text(center, Align2::CENTER_CENTER, "⚠ Try again", bold(13.0), Color32::from_rgb(255, 140, 120));
            }
            _ => {
                if h > 0.001 {
                    let pill = Rect::from_center_size(center, vec2(122.0, 34.0) * (0.85 + 0.15 * h));
                    ui.painter().rect_filled(pill, cr(17), alpha(accent(), 0.95 * h));
                    ui.painter().text(pill.center(), Align2::CENTER_CENTER, "⬇  Get Album", bold(13.5), alpha(on_accent(), h));
                }
            }
        }
        let y0 = art.bottom() + 10.0;
        text(ui, pos2(art.left(), y0), &r.title, font(13.5), DIM, art.width());
        let sub = match r.year {
            Some(y) => format!("{y} · {} songs", r.track_count),
            None => format!("{} songs", r.track_count),
        };
        text(ui, pos2(art.left(), y0 + 19.0), &sub, font(12.5), FAINT, art.width());
        let tip = match job {
            Some(j) if j.failed => format!("Couldn't download: {}. Click to try again.", j.status),
            _ => "Not in your library — click to find every song on YouTube and add the album".into(),
        };
        let resp = resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() && !busy {
            self.acts.push(Action::DownloadCatalogAlbum(r.clone()));
        }
    }

    pub(super) fn album_tile(&mut self, ui: &mut Ui, ai: usize, art: Rect) {
        let album = self.lib.albums[ai].clone();
        let resp = ui.interact(art, Id::new(("tile", &album.key)), Sense::click());
        let h = anim(ui, resp.id, resp.hovered(), 0.2);
        // Hover: grow slightly, deepen the shadow.
        let grown = art.expand(art.width() * 0.025 * h);
        shadow(ui, grown, 10, (14.0 + 18.0 * h) as u8, (4.0 + 6.0 * h) as i8, 0.35 + 0.25 * h);
        self.album_art(ui, &album, grown, 10);

        let mut play_hit = false;
        let mut more_hit = false;
        if h > 0.001 {
            ui.painter().rect_filled(grown, cr(10), Color32::from_black_alpha((70.0 * h) as u8));
            // Play button pops in bottom-left.
            let c = pos2(grown.left() + 28.0, grown.bottom() - 28.0);
            let hp = ui.rect_contains_pointer(Rect::from_center_size(c, vec2(40.0, 40.0)));
            let pr = 18.0 * (0.6 + 0.4 * h) * if hp { 1.08 } else { 1.0 };
            ui.painter().circle_filled(c, pr, alpha(if hp { accent() } else { Color32::from_rgb(30, 30, 34) }, 0.92 * h));
            paint_icon(ui, Icon::Play, c + vec2(1.5, 0.0), 14.0 * (0.6 + 0.4 * h), alpha(Color32::WHITE, h));
            play_hit = hp;
            // "…" menu bottom-right.
            let m = pos2(grown.right() - 26.0, grown.bottom() - 28.0);
            let hm = ui.rect_contains_pointer(Rect::from_center_size(m, vec2(34.0, 34.0)));
            ui.painter().circle_filled(m, 15.0 * (0.6 + 0.4 * h), alpha(if hm { Color32::from_rgb(70, 70, 78) } else { Color32::from_rgb(30, 30, 34) }, 0.9 * h));
            ui.painter().text(m - vec2(0.0, 3.0), Align2::CENTER_CENTER, "…", bold(16.0), alpha(Color32::WHITE, h));
            more_hit = hm;
        }
        if resp.clicked() {
            if play_hit {
                self.acts.push(Action::Play(album.tracks.clone(), 0));
            } else if !more_hit {
                self.acts.push(Action::Go(View::Album(album.key.clone())));
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if more_hit && resp.clicked() {
            // Open the same context menu from the "…" button.
            ui.memory_mut(|m| m.open_popup(resp.id.with("menu")));
        }
        self.album_menu(&resp, &album);
        egui::popup::popup_below_widget(ui, resp.id.with("menu"), &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(180.0);
            self.album_menu_items(ui, &album);
        });

        let playing_here = self.player.current().map(|id| album.tracks.contains(&id)).unwrap_or(false);
        let mut tx = art.left();
        let y0 = art.bottom() + 10.0;
        if playing_here {
            eq_bars(ui, pos2(tx + 6.0, y0 + 8.0), self.player.is_playing(), self.now, accent());
            tx += 18.0;
        }
        text(ui, pos2(tx, y0), &album.title, font(13.5), TEXT, art.right() - tx);
        let ar = text(ui, pos2(art.left(), y0 + 19.0), &album.artist, font(12.5), DIM, art.width());
        let a = ui.interact(ar, Id::new(("tile-artist", &album.key)), Sense::click());
        if a.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.acts.push(Action::Go(View::Artists(Some(album.artist.clone()))));
        }
    }

    pub(super) fn album_menu(&mut self, resp: &egui::Response, album: &Album) {
        resp.context_menu(|ui| self.album_menu_items(ui, album));
    }

    pub(super) fn album_menu_items(&mut self, ui: &mut Ui, album: &Album) {
        if ui.button("Play").clicked() {
            self.acts.push(Action::Play(album.tracks.clone(), 0));
            ui.close_menu();
        }
        if ui.button("Shuffle").clicked() {
            self.acts.push(Action::Shuffle(album.tracks.clone()));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Play Next").clicked() {
            self.acts.push(Action::PlayNext(album.tracks.clone()));
            ui.close_menu();
        }
        if ui.button("Add to Queue").clicked() {
            self.acts.push(Action::AddQueue(album.tracks.clone()));
            ui.close_menu();
        }
        self.playlist_submenu(ui, album.tracks.clone());
        ui.separator();
        if ui.button("Go to Artist").clicked() {
            self.acts.push(Action::Go(View::Artists(Some(album.artist.clone()))));
            ui.close_menu();
        }
        if ui.button(REVEAL_LABEL).clicked() {
            if let Some(t) = self.lib.track(album.tracks[0]) {
                self.acts.push(Action::Reveal(t.path.clone()));
            }
            ui.close_menu();
        }
    }

    pub(super) fn playlist_submenu(&mut self, ui: &mut Ui, ids: Vec<TrackId>) {
        ui.menu_button("Add to Playlist", |ui| {
            if ui.button("New Playlist…").clicked() {
                self.acts.push(Action::NewPlaylist(ids.clone()));
                ui.close_menu();
            }
            if !self.lib.data.playlists.is_empty() {
                ui.separator();
            }
            for pl in &self.lib.data.playlists {
                if ui.button(&pl.name).clicked() {
                    self.acts.push(Action::AddToPlaylist(pl.id, ids.clone()));
                    ui.close_menu();
                }
            }
        });
    }

    pub(super) fn album_page(&mut self, ui: &mut Ui, key: &str) {
        let Some(&ai) = self.lib.album_index.get(key) else {
            ui.label("Album not found.");
            return;
        };
        let album = self.lib.albums[ai].clone();
        let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt(("album", key));
        // Test screenshots: MH_SCROLL=<points> opens the page scrolled down.
        if let Some(off) = std::env::var("MH_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            sa = sa.vertical_scroll_offset(off);
        }
        sa.show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let art_size = 260.0f32.min(full.width() * 0.34);
            let head_h = art_size + PAD * 2.0 + 20.0;
            let head = Rect::from_min_size(full.min, vec2(full.width(), head_h + 60.0));
            self.ambient(ui, &album, head, 0.9);

            let art_rect = Rect::from_min_size(head.min + vec2(PAD, PAD + 4.0), vec2(art_size, art_size));
            shadow(ui, art_rect, 12, 40, 14, 0.6);
            self.album_art_hd(ui, &album, art_rect, 12);

            let tx = art_rect.right() + 32.0;
            let tw = head.right() - tx - PAD;
            let bottom = art_rect.bottom();
            // Bottom-aligned text block, like Apple Music.
            let mut meta = vec![];
            if !album.genre.is_empty() {
                meta.push(album.genre.to_uppercase());
            }
            if let Some(yr) = album.year {
                meta.push(yr.to_string());
            }
            let galley = ui.painter().layout(album.title.clone(), bold(32.0), TEXT, tw);
            let title_h = galley.size().y.min(80.0);
            let y_title = bottom - 128.0 - title_h;
            ui.painter().galley(pos2(tx, y_title), galley, TEXT);
            let ar = text(ui, pos2(tx, y_title + title_h + 4.0), &album.artist, font(21.0), accent(), tw);
            if ui.interact(ar, ui.id().with("album-artist"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Action::Go(View::Artists(Some(album.artist.clone()))));
            }
            text(ui, pos2(tx, y_title + title_h + 34.0), &meta.join(" · "), bold(11.5), DIM, tw);
            let mut bui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(pos2(tx, bottom - 38.0), vec2(tw, 40.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            bui.spacing_mut().item_spacing.x = 12.0;
            if pill(&mut bui, Some("play"), "Play", true).clicked() {
                self.acts.push(Action::Play(album.tracks.clone(), 0));
            }
            if pill(&mut bui, Some("🔀"), "Shuffle", false).clicked() {
                self.acts.push(Action::Shuffle(album.tracks.clone()));
            }
            let missing = self.missing_tracks(&album);
            if !missing.is_empty() && crate::ytdl::downloader_available() {
                let n = missing.len();
                let tip = "Find the songs you don't have on YouTube and add them to this album";
                let clicked = if tw >= 420.0 {
                    pill(&mut bui, Some("⬇"), &format!("{n} Missing"), false).on_hover_text(tip).clicked()
                } else {
                    // Not enough room beside Play/Shuffle: a link under the year.
                    let label = format!("⬇  Download {n} missing song{}", if n == 1 { "" } else { "s" });
                    let r = text(ui, pos2(tx, y_title + title_h + 56.0), &label, bold(12.5), accent(), tw);
                    ui.interact(r.expand(4.0), ui.id().with("dl-missing"), Sense::click())
                        .on_hover_text(tip)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                };
                if clicked {
                    self.acts.push(Action::DownloadMissing(album.key.clone(), missing.clone()));
                }
            }

            ui.allocate_space(vec2(full.width(), head_h));

            // Your songs plus the album's missing songs (greyed out), in
            // their real order.
            enum Row {
                Own(usize, Track),
                Missing(crate::ytdl::CatalogTrack),
            }
            let mut rows: Vec<((u32, u32, u8), Row)> = album
                .tracks
                .iter()
                .enumerate()
                .filter_map(|(i, &id)| self.lib.track(id).cloned().map(|t| (i, t)))
                .map(|(i, t)| ((t.disc_no.unwrap_or(1), t.track_no.unwrap_or(9_000 + i as u32), 0), Row::Own(i, t)))
                .collect();
            for m in &missing {
                rows.push(((m.disc, m.track, 1), Row::Missing(m.clone())));
            }
            rows.sort_by_key(|(k, _)| *k);
            let multi_disc = rows.iter().map(|((d, _, _), _)| *d).max().unwrap_or(1) > 1;
            let mut last_disc = None;
            for (n, ((disc, _, _), row)) in rows.into_iter().enumerate() {
                if multi_disc && last_disc != Some(disc) {
                    last_disc = Some(disc);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(PAD);
                        ui.label(egui::RichText::new(format!("Disc {disc}")).font(bold(14.0)).color(DIM));
                    });
                }
                match row {
                    Row::Own(i, t) => self.album_track_row(ui, &t, i, &album.tracks),
                    Row::Missing(m) => self.missing_track_row(ui, &album, &m, n),
                }
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(PAD);
                ui.label(egui::RichText::new(format!("{} songs, {}", album.tracks.len(), fmt_long(album.duration_secs))).size(12.0).color(DIM));
            });

            let more: Vec<usize> = self.lib.artists.get(&album.artist).map(|v| v.iter().copied().filter(|&j| j != ai).collect()).unwrap_or_default();
            if !more.is_empty() {
                ui.add_space(30.0);
                ui.horizontal(|ui| {
                    ui.add_space(PAD);
                    ui.label(egui::RichText::new(format!("More by {}", album.artist)).font(bold(20.0)));
                });
                ui.add_space(12.0);
                let tile = 160.0;
                let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), tile + 60.0), Sense::hover());
                for (k, &j) in more.iter().enumerate() {
                    let x = strip.left() + PAD + k as f32 * (tile + TILE_GAP);
                    if x + tile > strip.right() {
                        break;
                    }
                    self.album_tile(ui, j, Rect::from_min_size(pos2(x, strip.top()), vec2(tile, tile)));
                }
            }
            ui.add_space(30.0);
        });
    }

    /// Album tracks (per Apple's catalogue) that aren't in the library.
    /// Kicks off the tracklist lookup the first time an album is opened.
    pub(super) fn missing_tracks(&mut self, album: &Album) -> Vec<crate::ytdl::CatalogTrack> {
        match self.tracklists.get(&album.key) {
            None => {
                self.acts.push(Action::FetchTracklist(album.key.clone(), album.artist.clone(), album.title.clone()));
                vec![]
            }
            Some(Tracklist::Loading) | Some(Tracklist::Ready(None)) => vec![],
            Some(Tracklist::Ready(Some(cat))) => {
                let owned: Vec<String> = album.tracks.iter().filter_map(|id| self.lib.track(*id)).map(|t| t.title.clone()).collect();
                cat.tracks.iter().filter(|c| !owned.iter().any(|o| crate::ytdl::same_song(o, &c.title))).cloned().collect()
            }
        }
    }

    /// A song on the album you don't have: greyed out, click to download.
    pub(super) fn missing_track_row(&mut self, ui: &mut Ui, album: &Album, m: &crate::ytdl::CatalogTrack, i: usize) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
        let row = row.shrink2(vec2(PAD - 10.0, 0.0));
        let job = self
            .track_jobs
            .get(&(album.key.clone(), crate::ytdl::norm(&m.title)))
            .and_then(|id| self.jobs.iter().find(|j| j.id == *id));
        let busy = job.map(|j| !j.done).unwrap_or(false);
        self.row_bg(ui, row, resp.id, resp.hovered() && !busy, false, i);
        let num_c = pos2(row.left() + 22.0, row.center().y);
        ui.painter().text(num_c, Align2::CENTER_CENTER, m.track.to_string(), font(13.0), FAINT);
        let tc = if resp.hovered() && !busy { DIM } else { FAINT };
        text(ui, pos2(row.left() + 48.0, row.center().y - 9.0), &m.title, font(14.0), tc, row.width() - 220.0);
        text_right(ui, pos2(row.right() - 14.0, row.center().y - 8.0), &fmt_time(m.duration_ms as f64 / 1000.0), font(13.0), FAINT);
        // Right side: status / download button.
        let sx = row.right() - 70.0;
        match job {
            Some(j) if !j.done => {
                let label = match j.progress {
                    Some(p) => format!("{:.0}%", p * 100.0),
                    None => j.status.clone(),
                };
                text_right(ui, pos2(sx, row.center().y - 8.0), &label, font(12.0), accent());
                if let Some(p) = j.progress {
                    let bar = Rect::from_min_max(pos2(row.left() + 48.0, row.bottom() - 6.0), pos2(sx, row.bottom() - 4.0));
                    ui.painter().rect_filled(bar, cr(1), Color32::from_white_alpha(20));
                    let mut f = bar;
                    f.set_right(bar.left() + bar.width() * p);
                    ui.painter().rect_filled(f, cr(1), accent());
                }
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            Some(j) if j.failed => {
                text_right(ui, pos2(sx, row.center().y - 8.0), "⚠ retry", font(12.0), Color32::from_rgb(255, 140, 120));
            }
            Some(j) if j.cancelled => {
                text_right(ui, pos2(sx, row.center().y - 8.0), "stopped", font(12.0), FAINT);
            }
            _ => {
                let h = anim(ui, resp.id.with("dl"), resp.hovered(), 0.12);
                let c = lerp(FAINT, accent(), h);
                ui.painter().text(pos2(sx - 8.0, row.center().y), Align2::CENTER_CENTER, "⬇", font(15.0), c);
            }
        }
        let tip = match job {
            Some(j) if j.failed => format!("Couldn't download: {}. Click to try again.", j.status),
            _ => "Not in your library — click to find it on YouTube and add it".into(),
        };
        let resp = resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() && !busy {
            self.acts.push(Action::DownloadMissing(album.key.clone(), vec![m.clone()]));
        }
    }

    /// Row background: zebra stripe, animated hover, selection.
    pub(super) fn row_bg(&mut self, ui: &Ui, row: Rect, id: Id, hovered: bool, selected: bool, i: usize) {
        let h = anim(ui, id.with("h"), hovered, 0.12);
        let s = anim(ui, id.with("s"), selected, 0.15);
        let base = if i % 2 == 1 { ROW_ALT } else { alpha(BG, 0.0) };
        let c = lerp(lerp(base, HOVER, h), alpha(accent(), 0.28), s);
        if c.a() > 0 {
            ui.painter().rect_filled(row, cr(8), c);
        }
    }

    pub(super) fn album_track_row(&mut self, ui: &mut Ui, t: &Track, i: usize, list: &[TrackId]) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
        let row = row.shrink2(vec2(PAD - 10.0, 0.0));
        let is_cur = self.player.current() == Some(t.id);
        self.row_bg(ui, row, resp.id, resp.hovered(), self.st.selected == Some(t.id), i);
        let num_c = pos2(row.left() + 22.0, row.center().y);
        if resp.hovered() {
            paint_icon(ui, Icon::Play, num_c, 11.0, TEXT);
        } else if is_cur {
            eq_bars(ui, num_c, self.player.is_playing(), self.now, accent());
        } else {
            let n = t.track_no.map(|n| n.to_string()).unwrap_or_default();
            ui.painter().text(num_c, Align2::CENTER_CENTER, n, font(13.0), DIM);
        }
        let title_color = if is_cur { accent() } else { TEXT };
        let right_w = 110.0;
        let mut title_w = row.width() - 56.0 - right_w;
        let show_artist = t.artist != t.album_artist;
        if show_artist {
            title_w *= 0.6;
        }
        text(ui, pos2(row.left() + 48.0, row.center().y - 9.0), &t.title, font(14.0), title_color, title_w);
        if show_artist {
            text(ui, pos2(row.left() + 56.0 + title_w, row.center().y - 8.0), &t.artist, font(13.0), DIM, row.width() - 56.0 - right_w - title_w - 8.0);
        }
        self.love_and_time(ui, t, row, resp.hovered());
        // One click anywhere on the row plays it.
        if resp.clicked() {
            self.acts.push(Action::Play(list.to_vec(), i));
            self.st.selected = Some(t.id);
        }
        self.track_menu(&resp, t, list, i, None);
    }

    pub(super) fn love_and_time(&mut self, ui: &mut Ui, t: &Track, row: Rect, hovered: bool) {
        text_right(ui, pos2(row.right() - 14.0, row.center().y - 8.0), &fmt_time(t.duration_secs), font(13.0), DIM);
        if t.loved || hovered {
            self.heart(ui, t, pos2(row.right() - 80.0, row.center().y), 14.0);
        }
    }

    pub(super) fn track_menu(&mut self, resp: &egui::Response, t: &Track, list: &[TrackId], i: usize, playlist: Option<u64>) {
        resp.context_menu(|ui| {
            if ui.button("Play").clicked() {
                self.acts.push(Action::Play(list.to_vec(), i));
                ui.close_menu();
            }
            if ui.button("Play Next").clicked() {
                self.acts.push(Action::PlayNext(vec![t.id]));
                ui.close_menu();
            }
            if ui.button("Add to Queue").clicked() {
                self.acts.push(Action::AddQueue(vec![t.id]));
                ui.close_menu();
            }
            ui.separator();
            self.playlist_submenu(ui, vec![t.id]);
            if let Some(pid) = playlist {
                if ui.button("Remove from Playlist").clicked() {
                    self.acts.push(Action::RemoveFromPlaylist(pid, i));
                    ui.close_menu();
                }
            }
            if ui.button(if t.loved { "Unlove" } else { "Love" }).clicked() {
                self.acts.push(Action::ToggleLove(t.id));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Go to Album").clicked() {
                self.acts.push(Action::Go(View::Album(t.album_key())));
                ui.close_menu();
            }
            if ui.button("Go to Artist").clicked() {
                self.acts.push(Action::Go(View::Artists(Some(t.album_artist.clone()))));
                ui.close_menu();
            }
            if ui.button(REVEAL_LABEL).clicked() {
                self.acts.push(Action::Reveal(t.path.clone()));
                ui.close_menu();
            }
        });
    }

    pub(super) fn sort_songs(&self, ids: &mut [TrackId]) {
        let Some((key, asc)) = self.st.song_sort else { return };
        let lib = self.lib;
        ids.sort_by(|a, b| {
            let (a, b) = (lib.track(*a).unwrap(), lib.track(*b).unwrap());
            let o = match key {
                SongSort::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                SongSort::Artist => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                SongSort::Album => a.album.to_lowercase().cmp(&b.album.to_lowercase()),
                SongSort::Time => a.duration_secs.total_cmp(&b.duration_secs),
            };
            if asc {
                o
            } else {
                o.reverse()
            }
        });
    }

    /// Generic song table page (Songs, Loved, playlists).
    pub(super) fn song_page(&mut self, ui: &mut Ui, title: &str, ids: Vec<TrackId>, playlist: Option<u64>, sortable: bool) {
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.label(egui::RichText::new(title).font(bold(30.0)));
        });
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            let dur: f64 = ids.iter().filter_map(|id| self.lib.track(*id)).map(|t| t.duration_secs).sum();
            ui.label(egui::RichText::new(format!("{} songs, {}", ids.len(), fmt_long(dur))).size(12.0).color(DIM));
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.spacing_mut().item_spacing.x = 12.0;
            if pill(ui, Some("play"), "Play", true).clicked() {
                self.acts.push(Action::Play(ids.clone(), 0));
            }
            if pill(ui, Some("🔀"), "Shuffle", false).clicked() {
                self.acts.push(Action::Shuffle(ids.clone()));
            }
        });
        ui.add_space(16.0);
        if ids.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("This playlist is empty.\nRight-click any song or album → Add to Playlist.").color(DIM));
            });
            return;
        }

        let full_w = ui.available_width() - PAD * 2.0 + 20.0;
        let cols = Cols::new(full_w);
        let (hdr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let hdr = hdr.shrink2(vec2(PAD - 10.0, 0.0));
        let headers = [
            ("Title", hdr.left() + cols.title, SongSort::Title),
            ("Artist", hdr.left() + cols.artist, SongSort::Artist),
            ("Album", hdr.left() + cols.album, SongSort::Album),
        ];
        for (label, x, key) in headers {
            let arrow = match self.st.song_sort {
                Some((k, true)) if k == key && sortable => " ▲",
                Some((k, false)) if k == key && sortable => " ▼",
                _ => "",
            };
            let r = text(ui, pos2(x, hdr.top() + 4.0), &format!("{label}{arrow}"), bold(11.5), DIM, 200.0);
            if sortable {
                let resp = ui.interact(r.expand(4.0), ui.id().with(("hdr", label)), Sense::click());
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.st.song_sort = match self.st.song_sort {
                        Some((k, true)) if k == key => Some((key, false)),
                        Some((k, false)) if k == key => None,
                        _ => Some((key, true)),
                    };
                }
            }
        }
        let time_r = Rect::from_min_max(pos2(hdr.right() - 60.0, hdr.top()), hdr.max);
        text_right(ui, pos2(hdr.right() - 14.0, hdr.top() + 4.0), "Time", bold(11.5), DIM);
        if sortable && ui.interact(time_r, ui.id().with("hdr-time"), Sense::click()).clicked() {
            self.st.song_sort = match self.st.song_sort {
                Some((SongSort::Time, true)) => Some((SongSort::Time, false)),
                Some((SongSort::Time, false)) => None,
                _ => Some((SongSort::Time, true)),
            };
        }
        ui.painter().hline(hdr.x_range(), hdr.bottom(), Stroke::new(1.0f32, LINE));

        egui::ScrollArea::vertical().auto_shrink(false).id_salt(("songs", title)).show_rows(ui, 46.0, ids.len(), |ui, range| {
            for i in range {
                let Some(t) = self.lib.track(ids[i]).cloned() else { continue };
                self.song_row(ui, &t, i, &ids, &cols, playlist);
            }
        });
    }

    pub(super) fn song_row(&mut self, ui: &mut Ui, t: &Track, i: usize, list: &[TrackId], cols: &Cols, playlist: Option<u64>) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
        let row = row.shrink2(vec2(PAD - 10.0, 1.0));
        let is_cur = self.player.current() == Some(t.id);
        self.row_bg(ui, row, resp.id, resp.hovered(), self.st.selected == Some(t.id), i);
        let art = Rect::from_min_size(pos2(row.left() + 6.0, row.top() + 5.0), vec2(34.0, 34.0));
        self.track_art(ui, t.id, art, 5);
        let h = anim(ui, resp.id.with("art"), resp.hovered(), 0.12);
        if h > 0.0 {
            ui.painter().rect_filled(art, cr(5), Color32::from_black_alpha((120.0 * h) as u8));
            paint_icon(ui, Icon::Play, art.center(), 12.0, alpha(Color32::WHITE, h));
        } else if is_cur {
            ui.painter().rect_filled(art, cr(5), Color32::from_black_alpha(120));
            eq_bars(ui, art.center(), self.player.is_playing(), self.now, Color32::WHITE);
        }
        let y = row.center().y - 9.0;
        let c = if is_cur { accent() } else { TEXT };
        text(ui, pos2(row.left() + cols.title, y), &t.title, font(13.5), c, cols.artist - cols.title - 16.0);
        text(ui, pos2(row.left() + cols.artist, y + 1.0), &t.artist, font(13.0), DIM, cols.album - cols.artist - 16.0);
        let ar = text(ui, pos2(row.left() + cols.album, y + 1.0), &t.album, font(13.0), DIM, cols.end - cols.album - 16.0);
        let album_link = ui.interact(ar, ui.id().with(("alb", t.id, i)), Sense::click());
        if album_link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.acts.push(Action::Go(View::Album(t.album_key())));
        }
        self.love_and_time(ui, t, row, resp.hovered());
        // One click anywhere on the row plays it.
        if resp.clicked() {
            self.acts.push(Action::Play(list.to_vec(), i));
            self.st.selected = Some(t.id);
        }
        self.track_menu(&resp, t, list, i, playlist);
    }

    pub(super) fn artists(&mut self, ui: &mut Ui, sel: Option<String>) {
        let names: Vec<String> = self.lib.artists.keys().cloned().collect();
        let sel = sel.filter(|s| self.lib.artists.contains_key(s)).or_else(|| names.first().cloned());
        egui::SidePanel::left("artist-list")
            .exact_width(240.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::default().inner_margin(egui::Margin { left: 14, right: 8, top: 20, bottom: 8 }))
            .show_inside(ui, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(full.right() + 8.0, full.y_range().expand(20.0), Stroke::new(1.0f32, LINE));
                ui.label(egui::RichText::new("Artists").font(bold(24.0)));
                ui.add_space(8.0);
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, 52.0, names.len(), |ui, range| {
                    for i in range {
                        let name = &names[i];
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                        let active = sel.as_deref() == Some(name.as_str());
                        let a = anim(ui, resp.id.with("a"), active, 0.2);
                        let h = anim(ui, resp.id, resp.hovered(), 0.12);
                        let bg = lerp(alpha(HOVER, 0.7 * h), SEL, a);
                        if bg.a() > 0 {
                            ui.painter().rect_filled(rect, cr(8), bg);
                        }
                        let circ = Rect::from_center_size(pos2(rect.left() + 24.0, rect.center().y), vec2(36.0, 36.0));
                        if let Some(&ai) = self.lib.artists[name].first() {
                            let a = self.lib.albums[ai].clone();
                            self.album_art(ui, &a, circ, 18);
                        }
                        text(ui, pos2(rect.left() + 52.0, rect.center().y - 9.0), name, font(14.0), TEXT, rect.width() - 58.0);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.acts.push(Action::Go(View::Artists(Some(name.clone()))));
                        }
                    }
                });
            });
        let Some(name) = sel else { return };
        let albums = self.lib.artists[&name].clone();
        let all: Vec<TrackId> = albums.iter().flat_map(|&i| self.lib.albums[i].tracks.clone()).collect();
        let acts = std::cell::RefCell::new(vec![]);
        let header = |ui: &mut Ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if pill(ui, Some("play"), "Play", true).clicked() {
                    acts.borrow_mut().push(Action::Play(all.clone(), 0));
                }
                if pill(ui, Some("🔀"), "Shuffle", false).clicked() {
                    acts.borrow_mut().push(Action::Shuffle(all.clone()));
                }
            });
        };
        // Albums on Apple's catalogue that aren't in the library yet.
        let missing: Vec<crate::ytdl::CatalogAlbumRef> = match self.discos.get(&name) {
            None => {
                self.acts.push(Action::FetchDiscography(name.clone()));
                vec![]
            }
            Some(Disco::Ready(Some(list))) => {
                let owned: std::collections::HashSet<String> = self.lib.albums.iter().map(|a| crate::ytdl::base_album(&a.title)).collect();
                list.iter().filter(|r| !owned.contains(&crate::ytdl::base_album(&r.title))).cloned().collect()
            }
            _ => vec![],
        };
        self.album_grid(ui, &name, &albums, Some(&header), &missing);
        self.acts.extend(acts.into_inner());
    }

    pub(super) fn search(&mut self, ui: &mut Ui) {
        let q = self.st.search.trim().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let hit = |s: &str| {
            let s = s.to_lowercase();
            words.iter().all(|w| s.contains(w))
        };
        let artists: Vec<String> = self.lib.artists.keys().filter(|a| hit(a)).take(12).cloned().collect();
        let albums: Vec<usize> = (0..self.lib.albums.len())
            .filter(|&i| {
                let a = &self.lib.albums[i];
                hit(&format!("{} {}", a.title, a.artist))
            })
            .take(24)
            .collect();
        let songs: Vec<TrackId> = self
            .lib
            .data
            .tracks
            .iter()
            .filter(|t| hit(&format!("{} {} {}", t.title, t.artist, t.album)))
            .take(200)
            .map(|t| t.id)
            .collect();

        egui::ScrollArea::vertical().auto_shrink(false).id_salt("search").show(ui, |ui| {
            heading(ui, &format!("Results for “{}”", self.st.search.trim()));
            if artists.is_empty() && albums.is_empty() && songs.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("No results.").color(DIM)));
                return;
            }
            if !artists.is_empty() {
                subhead(ui, "Artists");
                let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
                for (k, name) in artists.iter().enumerate() {
                    let x = strip.left() + PAD + k as f32 * 130.0;
                    if x + 110.0 > strip.right() {
                        break;
                    }
                    let circ = Rect::from_min_size(pos2(x, strip.top()), vec2(110.0, 110.0));
                    let r = ui.interact(circ, ui.id().with(("sa", k)), Sense::click());
                    let h = anim(ui, r.id, r.hovered(), 0.18);
                    let c2 = circ.expand(4.0 * h);
                    shadow(ui, c2, 55, 20, 6, 0.3 + 0.2 * h);
                    let a = self.lib.albums[self.lib.artists[name][0]].clone();
                    self.album_art(ui, &a, c2, 58);
                    let galley = ui.painter().layout(name.clone(), font(13.0), TEXT, 120.0);
                    let gw = galley.size().x;
                    ui.painter().galley(pos2(circ.center().x - gw / 2.0, circ.bottom() + 10.0), galley, TEXT);
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.acts.push(Action::Go(View::Artists(Some(name.clone()))));
                    }
                }
            }
            if !albums.is_empty() {
                subhead(ui, "Albums");
                let tile = 160.0;
                let per_row = (((ui.available_width() - PAD * 2.0 + TILE_GAP) / (tile + TILE_GAP)) as usize).max(1);
                for chunk in albums.chunks(per_row) {
                    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), tile + 60.0), Sense::hover());
                    for (k, &ai) in chunk.iter().enumerate() {
                        let x = strip.left() + PAD + k as f32 * (tile + TILE_GAP);
                        self.album_tile(ui, ai, Rect::from_min_size(pos2(x, strip.top()), vec2(tile, tile)));
                    }
                }
            }
            if !songs.is_empty() {
                subhead(ui, "Songs");
                let cols = Cols::new(ui.available_width() - PAD * 2.0 + 20.0);
                for (i, &id) in songs.iter().enumerate() {
                    if let Some(t) = self.lib.track(id).cloned() {
                        self.song_row(ui, &t, i, &songs, &cols, None);
                    }
                }
            }
            ui.add_space(30.0);
        });
    }
}
