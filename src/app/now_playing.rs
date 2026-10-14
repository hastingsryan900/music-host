//! Now Playing: cover, controls, synced lyrics, the PS3 XMB backdrop and
//! recommendations.

use super::*;

impl V<'_> {
    /// Full-screen player: blurred cover + XMB wave backdrop, cover and
    /// controls up top, "Recommended for You" along the bottom.
    pub(super) fn now_playing(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        self.xmb_background(ui, full);
        let Some(t) = self.player.current().and_then(|id| self.lib.track(id)).cloned() else {
            ui.painter().text(full.center(), Align2::CENTER_CENTER, "Nothing playing", bold(22.0), DIM);
            return;
        };
        let album = self.lib.album_of(t.id).cloned();
        let lyr_k = anim(ui, ui.id().with("lyr-k"), self.st.show_lyrics, 0.3);

        let recs_h = if !self.st.show_lyrics && full.height() > 430.0 && !self.recs.is_empty() { (full.height() * 0.4).clamp(176.0, 242.0) } else { 0.0 };
        let top = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - recs_h));
        let inner = top.shrink2(vec2(56.0, 24.0));

        if self.st.show_lyrics {
            // Lyrics view: cover + controls in a column on the left, lyrics on the right.
            let col_w = (inner.width() * 0.4).clamp(260.0, 420.0);
            let lyr = Rect::from_min_max(pos2(inner.left() + col_w + 56.0, full.top() + 24.0), pos2(inner.right(), full.bottom() - 24.0));
            // Soft scrim so lines stay readable on bright covers.
            let scrim = Rect::from_min_max(pos2(lyr.left() - 40.0, full.top()), full.max);
            let mut mesh = egui::Mesh::default();
            let (c0, c1) = (Color32::from_black_alpha(0), Color32::from_black_alpha((95.0 * lyr_k) as u8));
            mesh.colored_vertex(scrim.left_top(), c0);
            mesh.colored_vertex(scrim.left_top() + vec2(120.0, 0.0), c1);
            mesh.colored_vertex(scrim.left_bottom(), c0);
            mesh.colored_vertex(scrim.left_bottom() + vec2(120.0, 0.0), c1);
            mesh.colored_vertex(scrim.right_top(), c1);
            mesh.colored_vertex(scrim.right_bottom(), c1);
            for tri in [[0, 1, 2], [1, 2, 3], [1, 4, 3], [4, 3, 5]] {
                mesh.add_triangle(tri[0], tri[1], tri[2]);
            }
            ui.painter().add(mesh);
            let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(lyr));
            lui.multiply_opacity(lyr_k);
            self.lyrics_panel(&mut lui, lyr, &t);

            // Size the cover so cover + info + Lyrics/Next row always fit
            // above the player bar (long titles wrap to two lines).
            let title_h = ui.painter().layout(t.title.clone(), bold(28.0), TEXT, col_w - 36.0).size().y.min(70.0);
            let info_h = title_h + 4.0 + 26.0 + 40.0 + 36.0 + 76.0 + 28.0;
            let art_size = (col_w * 0.82).min(inner.height() - info_h - 24.0).max(100.0);
            let block_h = art_size + 24.0 + info_h;
            let y0 = inner.top() + ((inner.height() - block_h) / 2.0).max(0.0);
            let art = Rect::from_min_size(pos2(inner.left() + (col_w - art_size) / 2.0, y0), vec2(art_size, art_size));
            self.np_art(ui, album.as_ref(), art);
            self.np_info(ui, &t, inner.left(), art.bottom() + 24.0, col_w);
            return;
        }

        let art_size = (inner.height() * 0.9).min(inner.width() * 0.3).clamp(140.0, 360.0);
        let gap = 52.0;
        let right_w = (inner.width() - art_size - gap).min(500.0);
        let total_w = art_size + gap + right_w;
        let x0 = inner.center().x - total_w / 2.0;
        let art = Rect::from_min_size(pos2(x0, inner.center().y - art_size / 2.0), vec2(art_size, art_size));
        self.np_art(ui, album.as_ref(), art);
        let y = (art.center().y - 112.0).max(top.top() + 12.0);
        self.np_info(ui, &t, art.right() + gap, y, right_w);

        if recs_h > 0.0 {
            self.recommendations(ui, Rect::from_min_max(pos2(full.left(), full.bottom() - recs_h), full.max));
        }
    }

    pub(super) fn np_art(&mut self, ui: &mut Ui, album: Option<&Album>, art: Rect) {
        shadow(ui, art, 14, 50, 18, 0.6);
        if let Some(a) = album {
            self.album_art_hd(ui, a, art, 14);
            let r = ui.interact(art, ui.id().with("np-big-art"), Sense::click());
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Go to Album").clicked() {
                self.acts.push(Action::Go(View::Album(a.key.clone())));
            }
        }
    }

    /// Title, artist, progress, transport controls and "Next" line.
    pub(super) fn np_info(&mut self, ui: &mut Ui, t: &Track, tx: f32, mut y: f32, right_w: f32) {
        let galley = ui.painter().layout(t.title.clone(), bold(28.0), TEXT, right_w - 36.0);
        let gh = galley.size().y.min(70.0);
        ui.painter().galley(pos2(tx, y), galley, TEXT);
        self.heart(ui, t, pos2(tx + right_w - 12.0, y + 16.0), 20.0);
        y += gh + 4.0;
        let ar = text(ui, pos2(tx, y), &t.artist, font(18.0), Color32::from_white_alpha(235), right_w);
        if ui.interact(ar, ui.id().with("np-artist"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.acts.push(Action::Go(View::Artists(Some(t.album_artist.clone()))));
        }
        y += 26.0;
        let sub = format!("{}{}", t.album, t.year.map(|y| format!(" · {y}")).unwrap_or_default());
        text(ui, pos2(tx, y), &sub, font(13.5), Color32::from_white_alpha(170), right_w);
        y += 40.0;
        let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(tx, y), vec2(right_w, 24.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        self.progress_row(&mut pui, Some(t), right_w, 12.0);
        y += 36.0;
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(tx, y), vec2(right_w, 64.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        cui.spacing_mut().item_spacing.x = 18.0;
        let controls_w = 30.0 * 2.0 + 48.0 * 2.0 + 62.0 + 18.0 * 4.0;
        cui.add_space(((right_w - controls_w) / 2.0).max(0.0));
        if glyph_toggle(&mut cui, "🔀", self.player.shuffle, "Shuffle").clicked() {
            self.acts.push(Action::ToggleShuffle);
        }
        if icon_button(&mut cui, Icon::Prev, 24.0, 48.0).clicked() {
            self.acts.push(Action::Prev);
        }
        if play_button(&mut cui, self.player.is_playing(), 62.0).clicked() {
            self.acts.push(Action::Toggle);
        }
        if icon_button(&mut cui, Icon::Next, 24.0, 48.0).clicked() {
            self.acts.push(Action::Next);
        }
        let looping = self.player.repeat == Repeat::One;
        if glyph_toggle(&mut cui, "🔁", looping, if looping { "Loop: on" } else { "Loop this song" }).clicked() {
            self.acts.push(Action::ToggleLoop);
        }
        y += 76.0;
        // Lyrics toggle, right-aligned under the controls.
        let lr = Rect::from_min_size(pos2(tx + right_w - 64.0, y - 4.0), vec2(64.0, 24.0));
        let lresp = ui.interact(lr, ui.id().with("np-lyrics"), Sense::click());
        let lh = anim(ui, lresp.id, lresp.hovered(), 0.15);
        let on = self.st.show_lyrics;
        if on || lh > 0.0 {
            ui.painter().rect_filled(lr, cr(12), if on { alpha(accent(), 0.9) } else { Color32::from_white_alpha((28.0 * lh) as u8) });
        }
        let lc = if on { on_accent() } else { lerp(DIM, TEXT, lh) };
        ui.painter().text(lr.center(), Align2::CENTER_CENTER, "Lyrics", bold(12.0), lc);
        if lresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show lyrics (L)").clicked() {
            self.acts.push(Action::ToggleLyrics);
        }
        if let Some(next) = self.player.up_next().first().and_then(|id| self.lib.track(*id)).cloned() {
            let r = text(ui, pos2(tx, y), &format!("Next: {} — {}", next.title, next.artist), font(12.5), DIM, right_w - 76.0);
            if ui.interact(r, ui.id().with("np-next"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Action::JumpUpNext(0));
            }
        }
    }

    /// Lyrics beside the cover: synced lines scroll along with the song and
    /// the current one lights up; click a line to jump there.
    pub(super) fn lyrics_panel(&mut self, ui: &mut Ui, r: Rect, t: &Track) {
        use crate::lyrics::Lyrics;
        let note = |ui: &Ui, s: &str| {
            ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, s, bold(20.0), Color32::from_white_alpha(120));
        };
        let lines = match self.lyrics.get(&t.id) {
            None => {
                self.acts.push(Action::FetchLyrics(t.id));
                return note(ui, "Looking for lyrics…");
            }
            Some(None) => return note(ui, "Looking for lyrics…"),
            Some(Some(Lyrics::None)) => return note(ui, "No lyrics found for this song"),
            Some(Some(Lyrics::Instrumental)) => return note(ui, "♪  Instrumental"),
            Some(Some(Lyrics::Plain(text))) => {
                let text = text.clone();
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                egui::ScrollArea::vertical().id_salt(("plain-lyrics", t.id)).auto_shrink(false).show(&mut cui, |ui| {
                    ui.add_space(r.height() * 0.2);
                    ui.label(egui::RichText::new(text).font(bold(20.0)).color(Color32::from_white_alpha(200)).line_height(Some(32.0)));
                    ui.add_space(r.height() * 0.4);
                });
                return;
            }
            Some(Some(Lyrics::Synced(lines))) => lines.clone(),
        };
        let pos = self.player.position();
        // Lines light up a touch early so they arrive with the singer.
        let cur = lines.partition_point(|(at, _)| *at <= pos + 0.25).checked_sub(1);
        let painter = ui.painter_at(r);
        let gap = 20.0;
        let galleys: Vec<_> = lines
            .iter()
            .map(|(_, s)| {
                let s = if s.trim().is_empty() { "♪".to_string() } else { s.clone() };
                painter.layout(s, bold(24.0), TEXT, r.width() - 12.0)
            })
            .collect();
        let mut ys = Vec::with_capacity(galleys.len());
        let mut y = 0.0;
        for g in &galleys {
            ys.push(y);
            y += g.size().y + gap;
        }
        let focus = match cur {
            Some(i) => ys[i] + galleys[i].size().y / 2.0,
            None => 0.0,
        };
        let target = focus - r.height() * 0.38;
        let off = ui.ctx().animate_value_with_time(ui.id().with(("lyr-off", t.id)), target, 0.5);
        if (off - target).abs() > 0.5 {
            ui.ctx().request_repaint();
        }
        for (i, g) in galleys.into_iter().enumerate() {
            let top = r.top() + ys[i] - off;
            let h = g.size().y;
            if top + h < r.top() || top > r.bottom() {
                continue;
            }
            let rect = Rect::from_min_size(pos2(r.left(), top), vec2(r.width(), h));
            let resp = ui.interact(rect.expand2(vec2(0.0, gap / 2.0)), ui.id().with(("lyr", t.id, i)), Sense::click());
            let hov = anim(ui, resp.id, resp.hovered(), 0.12);
            let lit = anim(ui, resp.id.with("lit"), cur == Some(i), 0.3);
            // Fade lines out toward the panel's top and bottom edges.
            let edge = ((top - r.top()) / 70.0).min((r.bottom() - top - h) / 70.0).clamp(0.0, 1.0);
            let a = (0.32 + 0.68 * lit).max(0.32 + 0.3 * hov) * (0.15 + 0.85 * edge);
            painter.galley_with_override_text_color(rect.min, g, Color32::from_white_alpha((255.0 * a) as u8));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Action::Seek(lines[i].0));
            }
        }
    }

    /// The PS3 XMB wave, drawn on the GPU behind everything on this page.
    /// Gradient colours follow the current album (via the accent colour).
    pub(super) fn xmb_background(&mut self, ui: &Ui, rect: Rect) {
        let a = egui::ecolor::Hsva::from(accent());
        let rgb = |c: Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
        let top: Color32 = egui::ecolor::Hsva::new(a.h, (a.s * 0.75).min(0.6), 0.40, 1.0).into();
        let bottom: Color32 = egui::ecolor::Hsva::new(a.h, a.s.min(0.8), 0.05, 1.0).into();
        let Some(xmb) = self.xmb.clone() else {
            ui.painter().rect_filled(rect, 0.0, bottom);
            return;
        };
        let frame = crate::xmb::Frame { time: self.now as f32, top: rgb(top), bottom: rgb(bottom) };
        let cb = egui::PaintCallback {
            rect,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                let v = info.viewport_in_pixels();
                if let Ok(mut x) = xmb.lock() {
                    x.paint(painter.gl(), &frame, [v.left_px, v.from_bottom_px, v.width_px, v.height_px], info.pixels_per_point);
                }
            })),
        };
        ui.painter().add(cb);
    }

    /// Horizontal strip of suggested songs.
    pub(super) fn recommendations(&mut self, ui: &mut Ui, r: Rect) {
        // Darkened shelf behind the strip.
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(r.left_top(), Color32::from_black_alpha(0));
        mesh.colored_vertex(r.right_top(), Color32::from_black_alpha(0));
        mesh.colored_vertex(r.left_bottom(), Color32::from_black_alpha(110));
        mesh.colored_vertex(r.right_bottom(), Color32::from_black_alpha(110));
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);

        let x0 = r.left() + PAD + 8.0;
        ui.painter().text(pos2(x0, r.top() + 4.0), Align2::LEFT_TOP, "Recommended for You", bold(17.0), TEXT);
        ui.painter().text(pos2(x0, r.top() + 27.0), Align2::LEFT_TOP, "Based on what you listen to most", font(12.0), DIM);
        // Header (50) + titles under the tiles (46) + bottom breathing room (28).
        let tile = (r.height() - 124.0).clamp(80.0, 118.0);
        let gap = 18.0;
        let y = r.top() + 50.0;
        let fits = (((r.width() - 2.0 * (PAD + 8.0) + gap) / (tile + gap)) as usize).max(1);
        let since = (self.now - self.view_at) as f32;
        for (i, &id) in self.recs.to_vec().iter().take(fits).enumerate() {
            let Some(t) = self.lib.track(id).cloned() else { continue };
            let k = ease((since - 0.15 - i as f32 * 0.05) / 0.4);
            let rect = Rect::from_min_size(pos2(x0 + i as f32 * (tile + gap), y + (1.0 - k) * 16.0), vec2(tile, tile));
            let resp = ui.interact(rect, ui.id().with(("rec", id)), Sense::click());
            let h = anim(ui, resp.id, resp.hovered(), 0.18);
            let g = rect.expand(3.0 * h);
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(g.expand2(vec2(10.0, 50.0))));
            cui.multiply_opacity(k);
            shadow(&cui, g, 8, (12.0 + 10.0 * h) as u8, 4, 0.4 + 0.2 * h);
            self.track_art(&cui, id, g, 8);
            if h > 0.0 {
                cui.painter().rect_filled(g, cr(8), Color32::from_black_alpha((90.0 * h) as u8));
                cui.painter().circle_filled(g.center(), 18.0 * (0.6 + 0.4 * h), alpha(accent(), 0.95 * h));
                paint_icon(&cui, Icon::Play, g.center() + vec2(1.5, 0.0), 13.0, alpha(on_accent(), h));
            }
            text(&cui, pos2(rect.left(), rect.bottom() + 8.0), &t.title, font(12.5), TEXT, tile);
            text(&cui, pos2(rect.left(), rect.bottom() + 25.0), &t.artist, font(11.5), DIM, tile);
            if k < 1.0 {
                ui.ctx().request_repaint();
            }
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                self.acts.push(Action::PlayNow(id));
            }
            resp.context_menu(|ui| {
                if ui.button("Play Now").clicked() {
                    self.acts.push(Action::PlayNow(id));
                    ui.close_menu();
                }
                if ui.button("Play Next").clicked() {
                    self.acts.push(Action::PlayNext(vec![id]));
                    ui.close_menu();
                }
                if ui.button("Add to Queue").clicked() {
                    self.acts.push(Action::AddQueue(vec![id]));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Go to Album").clicked() {
                    self.acts.push(Action::Go(View::Album(t.album_key())));
                    ui.close_menu();
                }
            });
        }
    }
}
