//! Add from Link downloads: YouTube / Apple Music / Spotify links, missing
//! album tracks, and the restart-proof download queue.

use super::*;

impl App {
    /// Start downloads for every link in `text` (one per line/space).
    pub(super) fn add_links(&mut self, ctx: &Context, text: &str) {
        let urls: Vec<String> = text
            .split_whitespace()
            .filter(|w| w.starts_with("http://") || w.starts_with("https://") || w.starts_with("www."))
            .map(String::from)
            .collect();
        if urls.is_empty() {
            self.toast(ctx, "Paste a YouTube, Apple Music or Spotify link".into());
            return;
        }
        self.dispatch(ctx, urls.into_iter().map(Pending::Link).collect());
        self.st.link_text.clear();
    }

    /// Where new downloads go: the main library folder (under "YouTube/").
    pub(super) fn download_root(&mut self) -> PathBuf {
        let root = self
            .lib
            .data
            .folders
            .iter()
            .find(|f| f.ends_with("Music Host"))
            .or(self.lib.data.folders.first())
            .cloned()
            .unwrap_or_else(|| dirs::audio_dir().unwrap_or_default().join("Music Host"));
        if !self.lib.data.folders.iter().any(|f| root.starts_with(f)) {
            self.lib.data.folders.push(root.clone());
            self.lib.mark_dirty();
        }
        root
    }

    /// Start downloads: each gets a row in the downloads window and is
    /// remembered until it finishes, so a restart picks it back up.
    pub(super) fn dispatch(&mut self, ctx: &Context, specs: Vec<Pending>) {
        use crate::ytdl;
        let root = self.download_root();
        let snap = self.library_snapshot();
        let mut tracks = vec![];
        for spec in specs {
            let id = self.next_job;
            self.next_job += 1;
            let title = match &spec {
                Pending::Link(u) => u.clone(),
                Pending::Album(_, t) => t.clone(),
                Pending::Track(_, r) => r.track.title.clone(),
            };
            let job = LinkJob::new(id, title);
            let cancel = job.cancel.clone();
            self.jobs.insert(0, job);
            self.pending.insert(id, spec.clone());
            let (tx, root, snap, wake) = (self.yt_tx.clone(), root.clone(), snap.clone(), ctx.clone());
            match spec {
                Pending::Link(url) => {
                    std::thread::spawn(move || {
                        if ytdl::is_catalog_link(&url) {
                            ytdl::run_catalog(id, url, root, snap, tx, wake, cancel)
                        } else {
                            ytdl::run(id, url, root, snap, tx, wake, cancel)
                        }
                    });
                }
                Pending::Album(aid, _) => {
                    self.album_jobs.insert(aid, id);
                    std::thread::spawn(move || ytdl::run_catalog_album(id, aid, root, snap, tx, wake, cancel));
                }
                Pending::Track(key, req) => {
                    self.track_jobs.insert((key, ytdl::norm(&req.track.title)), id);
                    tracks.push((id, cancel, req));
                }
            }
        }
        if !tracks.is_empty() {
            let (tx, wake) = (self.yt_tx.clone(), ctx.clone());
            // One at a time: gentler on YouTube, and progress reads naturally.
            std::thread::spawn(move || {
                for (id, cancel, req) in tracks {
                    ytdl::download_track(id, req, tx.clone(), wake.clone(), cancel);
                }
            });
        }
        self.save_pending();
    }

    pub(super) fn save_pending(&self) {
        let list: Vec<&Pending> = self.pending.values().collect();
        if let Ok(b) = serde_json::to_vec_pretty(&list) {
            let _ = std::fs::write(pending_path(), b);
        }
    }

    pub(super) fn library_snapshot(&self) -> crate::ytdl::LibrarySnapshot {
        use crate::ytdl::{norm, without_feat};
        let mut snap = crate::ytdl::LibrarySnapshot::default();
        for t in &self.lib.data.tracks {
            let title = norm(&without_feat(&t.title));
            let first = t.artist.split(", ").next().unwrap_or(&t.artist);
            snap.songs.insert(format!("{}|{title}", norm(first)));
            snap.songs.insert(format!("{}|{title}", norm(&t.album_artist)));
        }
        for a in &self.lib.albums {
            snap.albums.push((norm(&a.artist), norm(&a.title), a.title.clone()));
            if let Some(dir) = a.tracks.first().and_then(|id| self.lib.track(*id)).and_then(|t| t.path.parent()) {
                snap.album_dirs.push((norm(&a.artist), crate::ytdl::base_album(&a.title), a.title.clone(), a.artist.clone(), dir.to_path_buf()));
            }
            for id in &a.tracks {
                if let Some(t) = self.lib.track(*id) {
                    snap.album_songs.insert(format!("{}|{}", norm(&a.title), norm(&without_feat(&t.title))));
                }
            }
        }
        snap
    }

    pub(super) fn download_missing(&mut self, ctx: &Context, key: &str, tracks: Vec<crate::ytdl::CatalogTrack>) {
        let Some(&ai) = self.lib.album_index.get(key) else { return };
        let album = self.lib.albums[ai].clone();
        // Save next to the album's existing files so it stays together.
        let dir = self
            .lib
            .track(album.tracks[0])
            .and_then(|t| t.path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| dirs::audio_dir().unwrap_or_default().join("Music Host"));
        let (year, genre) = match self.tracklists.get(key) {
            Some(Tracklist::Ready(Some(c))) => (c.year.or(album.year), c.genre.clone()),
            _ => (album.year, None),
        };
        let mut reqs = vec![];
        for t in tracks {
            let nk = (key.to_string(), crate::ytdl::norm(&t.title));
            // Skip ones already queued/running.
            if let Some(id) = self.track_jobs.get(&nk) {
                if self.jobs.iter().any(|j| j.id == *id && !j.done) {
                    continue;
                }
            }
            reqs.push(Pending::Track(
                key.to_string(),
                crate::ytdl::TrackRequest {
                    track: t,
                    album: album.title.clone(),
                    album_artist: album.artist.clone(),
                    year,
                    genre: genre.clone().or_else(|| (!album.genre.is_empty()).then(|| album.genre.clone())),
                    dir: dir.clone(),
                },
            ));
        }
        if reqs.is_empty() {
            return;
        }
        let n = reqs.len();
        self.toast(ctx, format!("Finding {n} song{} on YouTube…", if n == 1 { "" } else { "s" }));
        self.dispatch(ctx, reqs);
    }

    pub(super) fn pump_links(&mut self, ctx: &Context) {
        use crate::ytdl::Event;
        while let Ok((key, tl)) = self.tl_rx.try_recv() {
            self.tracklists.insert(key, Tracklist::Ready(tl));
        }
        while let Ok((id, l)) = self.lyr_rx.try_recv() {
            self.lyrics.insert(id, Some(l));
        }
        while let Ok((artist, d)) = self.disco_rx.try_recv() {
            self.discos.insert(artist, Disco::Ready(d));
        }
        let before = self.pending.len();
        while let Ok(e) = self.yt_rx.try_recv() {
            match &e {
                Event::Added { job, .. } | Event::Skipped { job, .. } | Event::Failed { job, .. } | Event::Cancelled { job, .. } => {
                    self.pending.remove(job);
                }
                _ => {}
            }
            match e {
                Event::Status { job, title, status, progress } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                        j.title = title;
                        j.status = status;
                        j.progress = progress;
                    }
                }
                Event::Added { job, title, summary, files } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                        j.title = title;
                        j.status = summary.clone();
                        j.progress = None;
                        j.done = true;
                    }
                    self.start_scan(ctx, files);
                    self.toast(ctx, summary.lines().next().unwrap_or("Added").to_string());
                }
                Event::Skipped { job, title, summary } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                        j.title = title;
                        j.status = summary.clone();
                        j.done = true;
                    }
                    self.toast(ctx, summary.lines().next().unwrap_or("Nothing new").to_string());
                }
                Event::Imported { files, .. } => self.start_scan(ctx, files),
                Event::Cancelled { job, title } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                        if j.title.starts_with("http") {
                            j.title = title;
                        }
                        j.status = "Stopped".into();
                        j.progress = None;
                        j.done = true;
                        j.cancelled = true;
                    }
                }
                Event::Failed { job, title, error } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) {
                        j.title = title;
                        j.status = error.clone();
                        j.done = true;
                        j.failed = true;
                    }
                    self.toast(ctx, format!("Download failed: {error}"));
                }
            }
        }
        if self.pending.len() != before {
            self.save_pending();
        }
    }
}
