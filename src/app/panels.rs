//! Chrome around the pages: player bar, sidebar, downloads window, settings,
//! queue and the mini player.

use super::*;

impl V<'_> {
    pub(super) fn player_bar(&mut self, ctx: &Context, track_at: f64) {
        egui::TopBottomPanel::bottom("player")
            .exact_height(88.0)
            .show_separator_line(false)
            .frame(egui::Frame::default().fill(BAR).inner_margin(egui::Margin::symmetric(18, 10)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(full.x_range().expand(18.0), full.top() - 10.0, Stroke::new(1.0f32, LINE));
                let cur = self.player.current().and_then(|id| self.lib.track(id)).cloned();
                let since = (self.now - track_at) as f32;
                let slide = ease(since / 0.45);

                // ── Left: now playing.
                let left_w = (full.width() * 0.3).min(360.0);
                let art_rect = Rect::from_min_size(pos2(full.left(), full.center().y - 32.0), vec2(64.0, 64.0));
                if let Some(t) = &cur {
                    let resp = ui.interact(art_rect, ui.id().with("np-art"), Sense::click());
                    let h = anim(ui, resp.id, resp.hovered(), 0.16);
                    let ar = art_rect.expand(2.0 * h);
                    shadow(ui, ar, 6, 12, 3, 0.45);
                    self.track_art(ui, t.id, ar, 6);
                    if h > 0.0 {
                        ui.painter().rect_filled(ar, cr(6), Color32::from_black_alpha((110.0 * h) as u8));
                        ui.painter().text(ar.center(), Align2::CENTER_CENTER, "⤢", font(20.0), alpha(Color32::WHITE, h));
                    }
                    if resp.on_hover_text("Now Playing (⌘P)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.acts.push(Action::Go(View::NowPlaying));
                    }
                    let tx = art_rect.right() + 14.0;
                    let w = full.left() + left_w - tx - 34.0;
                    let dy = (1.0 - slide) * 8.0;
                    text(ui, pos2(tx, full.center().y - 19.0 + dy), &t.title, bold(14.0), alpha(TEXT, slide), w);
                    let r = text(ui, pos2(tx, full.center().y + 2.0 + dy), &format!("{} — {}", t.artist, t.album), font(12.5), alpha(DIM, slide), w);
                    let resp = ui.interact(r, ui.id().with("np-sub"), Sense::click());
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.acts.push(Action::Go(View::Album(t.album_key())));
                    }
                    self.heart(ui, t, pos2(full.left() + left_w - 14.0, full.center().y), 17.0);
                } else {
                    ui.painter().rect_filled(art_rect, cr(6), Color32::from_rgb(40, 40, 46));
                    ui.painter().text(art_rect.center(), Align2::CENTER_CENTER, "♪", font(24.0), FAINT);
                    text(ui, pos2(art_rect.right() + 14.0, full.center().y - 9.0), "Not Playing", font(14.0), DIM, 200.0);
                }

                // ── Centre: transport + scrubber.
                let cw = (full.width() * 0.4).clamp(300.0, 560.0);
                let center = Rect::from_center_size(full.center(), vec2(cw, full.height()));
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(center).layout(egui::Layout::top_down(egui::Align::Center)));
                cui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let total = 30.0 * 2.0 + 36.0 * 2.0 + 40.0 + 10.0 * 4.0;
                    ui.add_space(((cw - total) / 2.0).max(0.0));
                    if glyph_toggle(ui, "🔀", self.player.shuffle, "Shuffle").clicked() {
                        self.acts.push(Action::ToggleShuffle);
                    }
                    if icon_button(ui, Icon::Prev, 17.0, 36.0).clicked() {
                        self.acts.push(Action::Prev);
                    }
                    if play_button(ui, self.player.is_playing(), 40.0).clicked() {
                        self.acts.push(Action::Toggle);
                    }
                    if icon_button(ui, Icon::Next, 17.0, 36.0).clicked() {
                        self.acts.push(Action::Next);
                    }
                    let looping = self.player.repeat == Repeat::One;
                    if glyph_toggle(ui, "🔁", looping, if looping { "Loop: on" } else { "Loop this song" }).clicked() {
                        self.acts.push(Action::ToggleLoop);
                    }
                });
                cui.add_space(2.0);
                cui.horizontal(|ui| {
                    self.progress_row(ui, cur.as_ref(), cw, 11.0);
                });

                // ── Right: volume + queue.
                let right = Rect::from_min_max(pos2(full.right() - 230.0, full.top()), full.max);
                let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
                if glyph_toggle(&mut rui, "☰", self.st.show_queue, "Playing Next").clicked() {
                    self.st.show_queue = !self.st.show_queue;
                }
                if mini_button(&mut rui, self.st.show_mini).clicked() {
                    self.acts.push(Action::ToggleMini);
                }
                rui.add_space(10.0);
                let (_, f) = scrubber(&mut rui, 104.0, self.player.volume(), true, TEXT);
                if let Some(f) = f {
                    self.acts.push(Action::Volume(f));
                }
                let vol = self.player.volume();
                let glyph = if vol <= 0.001 { "🔇" } else if vol < 0.5 { "🔉" } else { "🔊" };
                rui.label(egui::RichText::new(glyph).size(13.0).color(DIM));
            });
    }

    /// "1:23 ━━━━━━○──── -2:01" — shared by the bar and the full player.
    pub(super) fn progress_row(&mut self, ui: &mut Ui, cur: Option<&Track>, width: f32, size: f32) {
        let dur = cur.map(|t| t.duration_secs).unwrap_or(0.0);
        let pos = self.player.position().min(dur.max(0.0));
        let shown = match self.st.seek_drag {
            Some(f) => f as f64 * dur,
            None => pos,
        };
        let tw = size * 4.0;
        ui.allocate_ui(vec2(tw, 16.0), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(fmt_time(shown)).size(size).color(DIM));
            });
        });
        let frac = if dur > 0.0 { (shown / dur) as f32 } else { 0.0 };
        let (resp, f) = scrubber(ui, width - 2.0 * tw - 20.0, frac, cur.is_some(), if cur.is_some() { TEXT } else { FAINT });
        if cur.is_some() {
            if let Some(f) = f {
                self.st.seek_drag = Some(f);
            }
            if resp.drag_stopped() || resp.clicked() {
                if let Some(f) = self.st.seek_drag.take() {
                    self.acts.push(Action::Seek(f as f64 * dur));
                }
            }
        }
        ui.label(egui::RichText::new(format!("-{}", fmt_time(dur - shown))).size(size).color(DIM));
    }

    pub(super) fn heart(&mut self, ui: &Ui, t: &Track, c: egui::Pos2, size: f32) {
        let r = Rect::from_center_size(c, vec2(size + 10.0, size + 10.0));
        let resp = ui.interact(r, ui.id().with(("heart", t.id)), Sense::click());
        let on = anim(ui, resp.id.with("on"), t.loved, 0.35);
        let h = anim(ui, resp.id, resp.hovered(), 0.12);
        // A little pop when toggled on.
        let pop = 1.0 + 0.35 * (on * std::f32::consts::PI).sin() * t.loved as u8 as f32;
        let color = if t.loved { accent() } else { lerp(DIM, TEXT, h) };
        ui.painter().text(c, Align2::CENTER_CENTER, if t.loved { "♥" } else { "♡" }, font(size * pop), color);
        if resp.on_hover_text(if t.loved { "Unlove" } else { "Love" }).clicked() {
            self.acts.push(Action::ToggleLove(t.id));
        }
    }

    /// "Update available" card above the sidebar footer (Windows builds).
    fn update_banner(&mut self, ui: &mut Ui) {
        use crate::update::State;
        let (title, sub, clickable) = match self.update {
            State::Idle => return,
            State::Available(_) => ("Update available", "Click to install and restart", true),
            State::Installing => ("Updating…", "Downloading the new version", false),
            State::Ready => ("Restarting…", "", false),
            State::Failed(_) => ("Update failed", "Click to try again", true),
        };
        let w = ui.available_width();
        let (r, resp) = ui.allocate_exact_size(vec2(w, 50.0), if clickable { Sense::click() } else { Sense::hover() });
        let h = anim(ui, resp.id, resp.hovered() && clickable, 0.15);
        ui.painter().rect_filled(r, cr(10), lerp(alpha(accent(), 0.85), accent(), h));
        let tc = on_accent();
        ui.painter().text(r.left_top() + vec2(12.0, 9.0), Align2::LEFT_TOP, title, bold(13.5), tc);
        ui.painter().text(r.left_top() + vec2(12.0, 28.0), Align2::LEFT_TOP, sub, font(11.5), alpha(tc, 0.8));
        if !clickable {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && clickable {
            self.acts.push(Action::InstallUpdate);
        }
        ui.add_space(8.0);
    }

    pub(super) fn sidebar(&mut self, ctx: &Context, scan: Option<(usize, usize)>, can_back: bool) {
        egui::SidePanel::left("sidebar")
            .exact_width(230.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::default().fill(SIDEBAR).inner_margin(egui::Margin { left: 12, right: 12, top: 16, bottom: 10 }))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(full.right() + 12.0, full.y_range().expand(20.0), Stroke::new(1.0f32, LINE));
                ui.horizontal(|ui| {
                    let back = ui.add_enabled(can_back, egui::Button::new(egui::RichText::new("‹").size(18.0)).frame(false).min_size(vec2(22.0, 26.0)));
                    if back.on_hover_text("Back (⌘[)").clicked() {
                        self.acts.push(Action::Back);
                    }
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.st.search)
                            .hint_text(egui::RichText::new("🔍  Search").color(FAINT))
                            .desired_width(f32::INFINITY)
                            .margin(vec2(10.0, 6.0)),
                    );
                    if std::mem::take(&mut self.st.focus_search) {
                        resp.request_focus();
                    }
                    if resp.changed() {
                        if self.st.search.trim().is_empty() {
                            if *self.view == View::Search {
                                self.acts.push(Action::Back);
                            }
                        } else if *self.view != View::Search {
                            self.acts.push(Action::Go(View::Search));
                        }
                    }
                });
                ui.add_space(16.0);

                egui::ScrollArea::vertical().auto_shrink(false).max_height(ui.available_height() - 52.0).show(ui, |ui| {
                    section(ui, "Library");
                    if self.player.current().is_some() {
                        self.nav(ui, NavIcon::NowPlaying, "Now Playing", View::NowPlaying);
                    }
                    self.nav(ui, NavIcon::Recent, "Recently Added", View::RecentlyAdded);
                    self.nav(ui, NavIcon::Artists, "Artists", View::Artists(None));
                    self.nav(ui, NavIcon::Albums, "Albums", View::Albums);
                    self.nav(ui, NavIcon::Songs, "Songs", View::Songs);
                    self.nav(ui, NavIcon::Loved, "Loved", View::Loved);
                    self.nav(ui, NavIcon::Stats, "Listening", View::Stats);
                    if self.nav(ui, NavIcon::Store, "Store", View::Store).clicked() {
                        self.st.store_focus = true;
                    }
                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        section(ui, "Playlists");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if ui
                                .add(egui::Button::new(egui::RichText::new("+").size(16.0).color(DIM)).frame(false))
                                .on_hover_text("New Playlist")
                                .clicked()
                            {
                                self.acts.push(Action::NewPlaylist(vec![]));
                            }
                        });
                    });
                    let playlists: Vec<(u64, String)> = self.lib.data.playlists.iter().map(|pl| (pl.id, pl.name.clone())).collect();
                    for (id, name) in playlists {
                        if let Some((rid, buf)) = &mut self.st.renaming {
                            if *rid == id {
                                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(f32::INFINITY));
                                if !r.has_focus() && !r.lost_focus() {
                                    r.request_focus();
                                }
                                if r.lost_focus() {
                                    let (rid, buf) = self.st.renaming.take().unwrap();
                                    self.acts.push(Action::RenamePlaylist(rid, buf));
                                }
                                continue;
                            }
                        }
                        let r = self.nav(ui, NavIcon::Playlist, &name, View::Playlist(id));
                        let mut rename = false;
                        r.context_menu(|ui| {
                            if ui.button("Play").clicked() {
                                let ids = self.lib.data.playlists.iter().find(|pl| pl.id == id).map(|pl| pl.tracks.clone()).unwrap_or_default();
                                self.acts.push(Action::Play(ids, 0));
                                ui.close_menu();
                            }
                            if ui.button("Rename").clicked() {
                                rename = true;
                                ui.close_menu();
                            }
                            if ui.button("Delete Playlist").clicked() {
                                self.acts.push(Action::DeletePlaylist(id));
                                ui.close_menu();
                            }
                        });
                        if rename || r.double_clicked() {
                            self.st.renaming = Some((id, name.clone()));
                        }
                    }
                });

                // Footer: scan status + folders.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    self.update_banner(ui);
                    ui.horizontal(|ui| {
                        if let Some((done, total)) = scan {
                            ui.spinner();
                            let msg = if total == 0 { "Looking for music…".to_string() } else { format!("Importing {done} of {total}") };
                            ui.label(egui::RichText::new(msg).size(11.5).color(DIM));
                        } else {
                            ui.label(
                                egui::RichText::new(format!("{} songs · {} albums", self.lib.data.tracks.len(), self.lib.albums.len()))
                                    .size(11.5)
                                    .color(FAINT),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(egui::RichText::new("⚙").size(14.0).color(DIM)).frame(false))
                                .on_hover_text("Music Folders")
                                .clicked()
                            {
                                self.st.show_folders = !self.st.show_folders;
                            }
                            let busy = self.jobs.iter().any(|j| !j.done);
                            let link = egui::RichText::new(if busy { "⬇" } else { "＋" }).size(14.0).color(if busy { accent() } else { DIM });
                            if ui.add(egui::Button::new(link).frame(false)).on_hover_text("Add from Link (⌘U)").clicked() {
                                self.st.show_link = !self.st.show_link;
                            }
                        });
                    });
                });
            });

        if self.st.show_folders {
            self.folders_window(ctx);
        }
        if self.st.show_link {
            self.link_window(ctx);
        }
    }

    pub(super) fn link_window(&mut self, ctx: &Context) {
        let frame = egui::Frame::default()
            .fill(Color32::from_rgb(28, 28, 32))
            .corner_radius(cr(16))
            .stroke(Stroke::new(1.0f32, Color32::from_gray(52)))
            .inner_margin(egui::Margin::same(20))
            .shadow(egui::Shadow { offset: [0, 16], blur: 40, spread: 0, color: Color32::from_black_alpha(150) });
        let mut close = false;
        egui::Window::new("add-from-link")
            .title_bar(false)
            .frame(frame)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .auto_sized()
            .anchor(Align2::CENTER_TOP, vec2(115.0, 24.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                // Header: title + subtitle, Stop all, close.
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Add from Link").font(bold(19.0)).color(TEXT));
                        ui.label(egui::RichText::new("YouTube songs, albums or playlists").size(12.0).color(DIM));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                        let h = anim(ui, resp.id, resp.hovered(), 0.12);
                        ui.painter().circle_filled(r.center(), 14.0, Color32::from_white_alpha((10.0 + 18.0 * h) as u8));
                        paint_x(ui, r.center(), 4.5, lerp(DIM, TEXT, h));
                        if resp.on_hover_text("Close (downloads keep going)").clicked() {
                            close = true;
                        }
                        if self.jobs.iter().any(|j| !j.done) {
                            ui.add_space(6.0);
                            let r = ui.add(egui::Button::new(egui::RichText::new("Stop all").size(12.5).color(Color32::from_rgb(255, 140, 120))).frame(false));
                            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                self.acts.push(Action::StopDownload(None));
                            }
                        }
                    });
                });
                ui.add_space(10.0);

                if !crate::ytdl::downloader_available() {
                    ui.label(egui::RichText::new("Needs yt-dlp — install it with: brew install yt-dlp").color(DIM));
                    return;
                }

                // Link box with Download / Paste on the same row.
                let mut submit = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let input = egui::Frame::default()
                        .fill(Color32::from_rgb(40, 40, 46))
                        .corner_radius(cr(10))
                        .inner_margin(egui::Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.st.link_text)
                                    .hint_text(egui::RichText::new("Paste a YouTube link…").color(FAINT))
                                    .font(font(14.0))
                                    .frame(false)
                                    .desired_width(250.0),
                            )
                        })
                        .inner;
                    if self.st.link_text.is_empty() && !input.has_focus() && self.jobs.is_empty() {
                        input.request_focus();
                    }
                    if input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        submit = true;
                    }
                    if self.st.link_text.trim().is_empty() {
                        if pill(ui, None, "Paste", false).clicked() {
                            #[cfg(windows)]
                            let paste = crate::ytdl::quiet(std::process::Command::new("powershell")).args(["-NoProfile", "-Command", "Get-Clipboard"]).output();
                            #[cfg(not(windows))]
                            let paste = std::process::Command::new("pbpaste").output();
                            if let Ok(out) = paste {
                                self.st.link_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            }
                        }
                    } else if pill(ui, Some("⬇"), "Download", true).clicked() {
                        submit = true;
                    }
                });
                if submit && !self.st.link_text.trim().is_empty() {
                    let text = std::mem::take(&mut self.st.link_text);
                    self.acts.push(Action::AddLinks(text));
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Joins the album if you have it · gets the real HD cover").size(11.5).color(FAINT));

                // Downloads.
                if !self.jobs.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Downloads").font(bold(13.0)).color(DIM));
                        if self.jobs.iter().any(|j| j.done) {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let r = ui.add(egui::Button::new(egui::RichText::new("Clear finished").size(12.0).color(FAINT)).frame(false));
                                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    self.acts.push(Action::ClearFinishedDownloads);
                                }
                            });
                        }
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                        let jobs = self.jobs;
                        for j in jobs.iter() {
                            self.download_row(ui, j);
                        }
                    });
                }
            });
        if close {
            self.st.show_link = false;
        }
    }

    /// One download in the Add from Link window.
    pub(super) fn download_row(&mut self, ui: &mut Ui, j: &LinkJob) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        let h = anim(ui, resp.id, resp.hovered(), 0.12);
        if h > 0.0 {
            ui.painter().rect_filled(row, cr(10), alpha(HOVER, 0.8 * h));
        }
        // Status badge.
        let c = pos2(row.left() + 22.0, row.center().y);
        let (glyph, color) = if j.failed {
            ("!", Color32::from_rgb(255, 120, 110))
        } else if j.cancelled {
            ("–", FAINT)
        } else if j.done {
            ("✓", accent())
        } else {
            ("⬇", accent())
        };
        ui.painter().circle_filled(c, 15.0, alpha(color, 0.16));
        if j.done || j.progress.is_some() {
            ui.painter().text(c, Align2::CENTER_CENTER, glyph, bold(13.0), color);
        } else {
            // Spinning arc while looking things up.
            let t = ui.input(|i| i.time) as f32 * 5.0;
            let pts: Vec<egui::Pos2> = (0..=16).map(|k| {
                let a = t + k as f32 / 16.0 * 4.0;
                c + vec2(a.cos(), a.sin()) * 8.0
            }).collect();
            ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0f32, color)));
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        // Text.
        let tx = row.left() + 46.0;
        let w = row.width() - 46.0 - 44.0;
        text(ui, pos2(tx, row.top() + 8.0), &j.title, bold(13.0), TEXT, w);
        let status = match j.progress {
            Some(p) if !j.done => format!("{} · {:.0}%", j.status, p * 100.0),
            _ => j.status.lines().next().unwrap_or("").to_string(),
        };
        let sc = if j.failed { Color32::from_rgb(255, 140, 120) } else { DIM };
        text(ui, pos2(tx, row.top() + 27.0), &status, font(11.5), sc, w);
        if let Some(p) = j.progress.filter(|_| !j.done) {
            let bar = Rect::from_min_max(pos2(tx, row.bottom() - 6.0), pos2(tx + w, row.bottom() - 3.0));
            ui.painter().rect_filled(bar, cr(2), Color32::from_white_alpha(22));
            let mut f = bar;
            f.set_right(bar.left() + bar.width() * p.clamp(0.0, 1.0));
            ui.painter().rect_filled(f, cr(2), accent());
        }
        // Stop button for running downloads.
        if !j.done {
            let r = Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), vec2(26.0, 26.0));
            let br = ui.interact(r, ui.id().with(("stop", j.id)), Sense::click());
            let bh = anim(ui, br.id, br.hovered(), 0.12);
            ui.painter().circle_filled(r.center(), 13.0, Color32::from_white_alpha((8.0 + 22.0 * bh) as u8));
            paint_x(ui, r.center(), 4.0, lerp(DIM, Color32::from_rgb(255, 140, 120), bh));
            if br.on_hover_text("Stop this download").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Action::StopDownload(Some(j.id)));
            }
        }
    }

    pub(super) fn folders_window(&mut self, ctx: &Context) {
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(Align2::LEFT_BOTTOM, vec2(244.0, -100.0))
            .show(ctx, |ui| {
                if self.lib.data.folders.is_empty() {
                    ui.label(egui::RichText::new("No folders yet.").color(DIM));
                }
                for f in self.lib.data.folders.clone() {
                    ui.horizontal(|ui| {
                        ui.label("📁");
                        ui.add(egui::Label::new(f.to_string_lossy()).truncate()).on_hover_text(f.to_string_lossy());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                self.acts.push(Action::RemoveFolder(f.clone()));
                            }
                            if ui.small_button("Show").clicked() {
                                self.acts.push(Action::Reveal(f.clone()));
                            }
                        });
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if pill(ui, None, "Add Folder…", true).clicked() {
                        self.acts.push(Action::AddFolderDialog);
                    }
                    if pill(ui, None, "Rescan", false).on_hover_text("Re-read all tags and artwork").clicked() {
                        self.acts.push(Action::Rescan);
                    }
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Tip: drag folders or files onto the window to add them.").size(11.0).color(FAINT));
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                // Volume levelling switch.
                ui.horizontal(|ui| {
                    let on = self.player.level_volume;
                    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
                    let k = anim(ui, resp.id, on, 0.15);
                    ui.painter().rect_filled(r, cr(11), lerp(Color32::from_rgb(70, 70, 78), accent(), k));
                    ui.painter().circle_filled(pos2(r.left() + 11.0 + 16.0 * k, r.center().y), 8.5, Color32::WHITE);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.acts.push(Action::SetLevelVolume(!on));
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Even out volume").font(bold(13.0)));
                        let total = self.lib.data.tracks.len();
                        let done = self.lib.data.tracks.iter().filter(|t| t.loudness.is_some()).count();
                        let note = if done < total {
                            format!("Keeps songs at a similar loudness · measuring {done} of {total}")
                        } else {
                            "Keeps songs at a similar loudness (albums keep their own balance)".to_string()
                        };
                        ui.label(egui::RichText::new(note).size(11.0).color(FAINT));
                    });
                });
                ui.add_space(10.0);
                // Phone streaming switch.
                ui.horizontal(|ui| {
                    let on = matches!(self.stream, Some(Ok(_)));
                    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
                    let k = anim(ui, resp.id, on, 0.15);
                    ui.painter().rect_filled(r, cr(11), lerp(Color32::from_rgb(70, 70, 78), accent(), k));
                    ui.painter().circle_filled(pos2(r.left() + 11.0 + 16.0 * k, r.center().y), 8.5, Color32::WHITE);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.acts.push(Action::SetStream(!on));
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Listen on your iPhone").font(bold(13.0)));
                        let note = match self.stream {
                            Some(Err(e)) => format!("Couldn't start: {e}"),
                            Some(Ok(_)) => "Scan with your iPhone camera (same Wi-Fi). Add it to your Home Screen for an app icon.".into(),
                            None => "Play your library on your phone over Wi-Fi".into(),
                        };
                        ui.label(egui::RichText::new(note).size(11.0).color(FAINT));
                    });
                });
                if let Some(Ok(url)) = self.stream {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        qr_code(ui, url, 132.0);
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            ui.add(egui::Label::new(egui::RichText::new(url.split("?t=").next().unwrap_or(url)).font(bold(13.0)).color(TEXT)));
                            ui.label(egui::RichText::new("Only devices with this code can connect.").size(11.0).color(FAINT));
                            ui.add_space(6.0);
                            if pill(ui, None, "Copy Link", false).clicked() {
                                ui.ctx().copy_text(url.to_string());
                            }
                        });
                    });
                }
            });
        if !open {
            self.st.show_folders = false;
        }
    }

    pub(super) fn nav(&mut self, ui: &mut Ui, icon: NavIcon, label: &str, target: View) -> egui::Response {
        let active = match (&target, self.view) {
            (View::Artists(_), View::Artists(_)) => true,
            (a, b) => a == b,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let id = ui.id().with(("nav", label));
        let a = anim(ui, id.with("a"), active, 0.2);
        let h = anim(ui, id.with("h"), resp.hovered(), 0.12);
        let bg = lerp(alpha(HOVER, 0.7 * h), SEL, a);
        if bg.a() > 0 {
            ui.painter().rect_filled(rect, cr(8), bg);
        }
        nav_icon(ui, icon, pos2(rect.left() + 18.0, rect.center().y), accent());
        text(ui, pos2(rect.left() + 38.0, rect.center().y - 9.0), label, font(14.0), TEXT, rect.width() - 44.0);
        if resp.clicked() {
            self.acts.push(Action::Go(target));
        }
        resp
    }

    pub(super) fn queue_panel(&mut self, ctx: &Context) {
        egui::SidePanel::right("queue")
            .exact_width(300.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::default().fill(SIDEBAR).inner_margin(egui::Margin::same(16)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(full.left() - 16.0, full.y_range().expand(20.0), Stroke::new(1.0f32, LINE));
                ui.label(egui::RichText::new("Playing Next").font(bold(18.0)));
                let mode = match (self.player.shuffle, self.player.repeat) {
                    (true, Repeat::Off) => "Shuffle on",
                    (true, _) => "Shuffle · Looping this song",
                    (false, Repeat::One) => "Looping this song",
                    _ => "",
                };
                if !mode.is_empty() {
                    ui.label(egui::RichText::new(mode).size(11.5).color(DIM));
                }
                ui.add_space(8.0);
                let next = self.player.up_next();
                if next.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new("Nothing queued").color(DIM)));
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, 52.0, next.len(), |ui, range| {
                    for i in range {
                        let id = next[i];
                        let Some(t) = self.lib.track(id).cloned() else { continue };
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                        let h = anim(ui, resp.id, resp.hovered(), 0.12);
                        if h > 0.0 {
                            ui.painter().rect_filled(rect, cr(8), alpha(HOVER, h));
                        }
                        let art = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(38.0, 38.0));
                        self.track_art(ui, id, art, 5);
                        let w = rect.width() - 60.0;
                        text(ui, pos2(art.right() + 10.0, rect.top() + 6.0), &t.title, font(13.0), TEXT, w);
                        text(ui, pos2(art.right() + 10.0, rect.top() + 24.0), &t.artist, font(11.5), DIM, w);
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            self.acts.push(Action::JumpUpNext(i));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Play Now").clicked() {
                                self.acts.push(Action::JumpUpNext(i));
                                ui.close_menu();
                            }
                            if ui.button("Remove from Queue").clicked() {
                                self.acts.push(Action::RemoveUpNext(i));
                                ui.close_menu();
                            }
                        });
                    }
                });
            });
    }

    /// Small always-on-top window: cover, title, transport, progress.
    pub(super) fn mini_player(&mut self, ctx: &Context) {
        let builder = egui::ViewportBuilder::default()
            .with_title("Mini Player")
            .with_inner_size([380.0, 104.0])
            .with_resizable(false)
            .with_always_on_top()
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(false)
            .with_fullsize_content_view(true);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("mini-player"), builder, |mctx, _| {
            if mctx.input(|i| i.viewport().close_requested()) {
                self.st.show_mini = false;
            }
            egui::CentralPanel::default().frame(egui::Frame::default().fill(Color32::from_rgb(22, 22, 26))).show(mctx, |ui| self.mini_contents(ui, mctx));
        });
    }

    pub(super) fn mini_contents(&mut self, ui: &mut Ui, mctx: &Context) {
                let full = ui.max_rect();
                // Drag anywhere that isn't a control to move the window.
                let bg = ui.interact(full, ui.id().with("mini-drag"), Sense::click_and_drag());
                if bg.drag_started() {
                    mctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                let hovered = ui.rect_contains_pointer(full);
                let cur = self.player.current().and_then(|id| self.lib.track(id)).cloned();
                let art = Rect::from_min_size(full.min + vec2(12.0, 12.0), vec2(80.0, 80.0));
                match cur.as_ref().and_then(|t| self.lib.album_of(t.id)).cloned() {
                    Some(a) => {
                        self.album_art(ui, &a, art, 8);
                    }
                    None => placeholder(ui, art, 8, "", false),
                }
                let tx = art.right() + 14.0;
                let tw = full.right() - tx - 34.0;
                let (title, artist) = match &cur {
                    Some(t) => (t.title.clone(), t.artist.clone()),
                    None => ("Not Playing".into(), String::new()),
                };
                text(ui, pos2(tx, full.top() + 14.0), &title, bold(14.0), TEXT, tw);
                text(ui, pos2(tx, full.top() + 33.0), &artist, font(12.5), DIM, tw);

                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(tx - 6.0, full.top() + 50.0), vec2(160.0, 36.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                cui.spacing_mut().item_spacing.x = 6.0;
                if icon_button(&mut cui, Icon::Prev, 16.0, 30.0).clicked() {
                    self.acts.push(Action::Prev);
                }
                if play_button(&mut cui, self.player.is_playing(), 34.0).clicked() {
                    self.acts.push(Action::Toggle);
                }
                if icon_button(&mut cui, Icon::Next, 16.0, 30.0).clicked() {
                    self.acts.push(Action::Next);
                }

                // Progress line along the bottom of the text column.
                if let Some(t) = &cur {
                    let pos = self.player.position();
                    let frac = if t.duration_secs > 0.0 { (pos / t.duration_secs) as f32 } else { 0.0 };
                    let bar = Rect::from_min_max(pos2(tx, full.bottom() - 13.0), pos2(full.right() - 14.0, full.bottom() - 10.0));
                    let resp = ui.interact(bar.expand2(vec2(0.0, 6.0)), ui.id().with("mini-seek"), Sense::click());
                    ui.painter().rect_filled(bar, cr(2), Color32::from_rgb(52, 52, 58));
                    let mut f = bar;
                    f.set_right(bar.left() + bar.width() * frac.clamp(0.0, 1.0));
                    ui.painter().rect_filled(f, cr(2), accent());
                    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked()) {
                        self.acts.push(Action::Seek(((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64 * t.duration_secs));
                    }
                    text_right(ui, pos2(full.right() - 14.0, full.bottom() - 32.0), &format!("-{}", fmt_time((t.duration_secs - pos).max(0.0))), font(11.0), FAINT);
                }

                // Hover: close (top-right) and "open the big window" buttons.
                if hovered {
                    let c = pos2(full.right() - 16.0, full.top() + 16.0);
                    let r = ui.interact(Rect::from_center_size(c, vec2(22.0, 22.0)), ui.id().with("mini-close"), Sense::click());
                    paint_x(ui, c, 4.5, if r.hovered() { TEXT } else { DIM });
                    if r.on_hover_text("Close mini player").clicked() {
                        self.st.show_mini = false;
                    }
                    let e = pos2(full.right() - 16.0, full.top() + 42.0);
                    let r = ui.interact(Rect::from_center_size(e, vec2(22.0, 22.0)), ui.id().with("mini-expand"), Sense::click());
                    let col = if r.hovered() { TEXT } else { DIM };
                    let st = Stroke::new(1.5f32, col);
                    ui.painter().rect_stroke(Rect::from_center_size(e, vec2(11.0, 9.0)), cr(2), st, egui::StrokeKind::Middle);
                    if r.on_hover_text("Open Music Host").clicked() {
                        self.acts.push(Action::ShowMain);
                    }
                }
    }
}
