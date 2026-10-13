//! The application: state, actions, and every view.

use crate::art::{Art, ArtCache};
use crate::kit::*;
use crate::library::{self, Album, Library, ScanMsg, Track, TrackId};
use crate::media::Media;
use crate::player::{Player, Repeat};
use egui::{pos2, vec2, Align2, Color32, Context, Id, Key, Rect, Sense, Stroke, Ui};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    NowPlaying,
    RecentlyAdded,
    Albums,
    Artists(Option<String>),
    Songs,
    Loved,
    Stats,
    Album(String),
    Playlist(u64),
    Search,
}

#[derive(Clone, Copy, PartialEq)]
enum SongSort {
    Title,
    Artist,
    Album,
    Time,
}

enum Action {
    Play(Vec<TrackId>, usize),
    Shuffle(Vec<TrackId>),
    PlayNext(Vec<TrackId>),
    AddQueue(Vec<TrackId>),
    Toggle,
    Next,
    Prev,
    Seek(f64),
    Volume(f32),
    ToggleShuffle,
    ToggleLoop,
    Go(View),
    Back,
    AddToPlaylist(u64, Vec<TrackId>),
    NewPlaylist(Vec<TrackId>),
    RemoveFromPlaylist(u64, usize),
    RenamePlaylist(u64, String),
    DeletePlaylist(u64),
    ToggleLove(TrackId),
    Reveal(PathBuf),
    AddFolderDialog,
    AddPaths(Vec<PathBuf>),
    RemoveFolder(PathBuf),
    Rescan,
    JumpUpNext(usize),
    RemoveUpNext(usize),
    /// Play this track right now, keeping the rest of the queue.
    PlayNow(TrackId),
    /// Download audio from one or more YouTube links.
    AddLinks(String),
    /// Look up an album's full tracklist (album key, artist, title).
    FetchTracklist(String, String, String),
    /// Find these missing album tracks on YouTube and download them.
    DownloadMissing(String, Vec<crate::ytdl::CatalogTrack>),
    /// Stop one download (Some(job)) or all of them (None).
    StopDownload(Option<u64>),
    ClearFinishedDownloads,
    SetLevelVolume(bool),
    /// Look up an artist's full discography.
    FetchDiscography(String),
    FetchLyrics(TrackId),
    ToggleMini,
    ShowMain,
    ToggleLyrics,
    /// Download a whole album from an artist's discography.
    DownloadCatalogAlbum(crate::ytdl::CatalogAlbumRef),
}

/// Discography lookup state for an artist.
pub enum Disco {
    Loading,
    Ready(Option<Vec<crate::ytdl::CatalogAlbumRef>>),
}

/// A download that hasn't finished — saved so it picks up after a restart.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum Pending {
    Link(String),
    /// Catalogue album id, display title.
    Album(u64, String),
    /// Album key, the missing song.
    Track(String, crate::ytdl::TrackRequest),
}

fn pending_path() -> PathBuf {
    library::app_dir().join("downloads.json")
}

/// Full tracklist lookup state for an album.
pub enum Tracklist {
    Loading,
    Ready(Option<crate::ytdl::CatalogAlbum>),
}

#[derive(Default)]
struct UiState {
    search: String,
    focus_search: bool,
    selected: Option<TrackId>,
    renaming: Option<(u64, String)>,
    song_sort: Option<(SongSort, bool)>,
    seek_drag: Option<f32>,
    show_queue: bool,
    show_folders: bool,
    show_link: bool,
    link_text: String,
    show_lyrics: bool,
    show_mini: bool,
}

/// One "Add from Link" download, as shown in the window.
pub struct LinkJob {
    id: u64,
    title: String,
    status: String,
    progress: Option<f32>,
    done: bool,
    failed: bool,
    cancelled: bool,
    cancel: crate::ytdl::Cancel,
}

impl LinkJob {
    fn new(id: u64, title: String) -> Self {
        LinkJob {
            id,
            title,
            status: "Queued".into(),
            progress: None,
            done: false,
            failed: false,
            cancelled: false,
            cancel: Default::default(),
        }
    }
}

struct V<'a> {
    lib: &'a Library,
    art: &'a mut ArtCache,
    player: &'a Player,
    st: &'a mut UiState,
    acts: Vec<Action>,
    view: &'a View,
    now: f64,
    view_at: f64,
    recs: &'a [TrackId],
    xmb: Option<Arc<Mutex<crate::xmb::Xmb>>>,
    jobs: &'a [LinkJob],
    tracklists: &'a HashMap<String, Tracklist>,
    /// (album key, normalised title) → download job
    track_jobs: &'a HashMap<(String, String), u64>,
    discos: &'a HashMap<String, Disco>,
    /// catalogue album id → download job
    album_jobs: &'a HashMap<u64, u64>,
    /// None while still looking.
    lyrics: &'a HashMap<TrackId, Option<crate::lyrics::Lyrics>>,
}

pub struct App {
    lib: Library,
    player: Player,
    art: ArtCache,
    media: Option<Media>,
    view: View,
    view_at: f64,
    history: Vec<View>,
    st: UiState,
    scan_rx: Option<Receiver<ScanMsg>>,
    scan_progress: Option<(usize, usize)>,
    pending_scan: Vec<PathBuf>,
    last_save: Instant,
    frame: u64,
    shot: Option<String>,
    last_media: (Option<TrackId>, bool),
    toast: Option<(String, f64)>,
    last_track: Option<TrackId>,
    track_at: f64,
    accent: [f32; 3],
    title: String,
    // Listening stats for the current track.
    listen_track: Option<TrackId>,
    session_secs: f64,
    pending_secs: f64,
    last_tick: Instant,
    // Cached recommendations (recomputed per song / every 20s).
    recs: Vec<TrackId>,
    recs_for: Option<TrackId>,
    recs_at: f64,
    fps_frames: u32,
    fps_since: Instant,
    fps_last: Instant,
    fps_worst: f64,
    fps_slow: u32,
    frame_start: Instant,
    /// GPU renderer for the PS3 wave (None if OpenGL setup failed).
    xmb: Option<Arc<Mutex<crate::xmb::Xmb>>>,
    jobs: Vec<LinkJob>,
    next_job: u64,
    yt_tx: std::sync::mpsc::Sender<crate::ytdl::Event>,
    yt_rx: Receiver<crate::ytdl::Event>,
    tracklists: HashMap<String, Tracklist>,
    track_jobs: HashMap<(String, String), u64>,
    tl_tx: std::sync::mpsc::Sender<(String, Option<crate::ytdl::CatalogAlbum>)>,
    tl_rx: Receiver<(String, Option<crate::ytdl::CatalogAlbum>)>,
    /// Background loudness analysis (volume levelling).
    loud_tx: std::sync::mpsc::Sender<(TrackId, PathBuf)>,
    loud_rx: Receiver<(TrackId, Option<(f32, f32)>)>,
    loud_pending: std::collections::HashSet<TrackId>,
    discos: HashMap<String, Disco>,
    disco_tx: std::sync::mpsc::Sender<(String, Option<Vec<crate::ytdl::CatalogAlbumRef>>)>,
    disco_rx: Receiver<(String, Option<Vec<crate::ytdl::CatalogAlbumRef>>)>,
    album_jobs: HashMap<u64, u64>,
    /// Unfinished downloads by job id (persisted to downloads.json).
    pending: std::collections::BTreeMap<u64, Pending>,
    lyrics: HashMap<TrackId, Option<crate::lyrics::Lyrics>>,
    lyr_tx: std::sync::mpsc::Sender<(TrackId, crate::lyrics::Lyrics)>,
    lyr_rx: Receiver<(TrackId, crate::lyrics::Lyrics)>,
    menubar: Option<crate::menubar::MenuBar>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let lib = Library::load();
        let player = Player::new(lib.data.volume.unwrap_or(0.8));
        let d = DEFAULT_ACCENT;
        let (yt_tx, yt_rx) = channel();
        let (tl_tx, tl_rx) = channel();
        let (disco_tx, disco_rx) = channel();
        let (lyr_tx, lyr_rx) = channel();
        // One worker measures loudness, a song at a time.
        let (loud_tx, loud_jobs) = channel::<(TrackId, PathBuf)>();
        let (loud_res_tx, loud_rx) = channel();
        let wake = cc.egui_ctx.clone();
        std::thread::Builder::new()
            .name("loudness".into())
            .spawn(move || {
                for (id, path) in loud_jobs {
                    let r = crate::loudness::analyze(&path);
                    if loud_res_tx.send((id, r)).is_err() {
                        return;
                    }
                    wake.request_repaint();
                }
            })
            .ok();
        let mut app = App {
            art: ArtCache::new(&cc.egui_ctx),
            media: if std::env::var("MH_SCREENSHOT").is_ok() || std::env::var("MH_FPS").is_ok() { None } else { Media::new() },
            lib,
            player,
            view: View::RecentlyAdded,
            view_at: 0.0,
            history: vec![],
            st: UiState::default(),
            scan_rx: None,
            scan_progress: None,
            pending_scan: vec![],
            last_save: Instant::now(),
            frame: 0,
            shot: std::env::var("MH_SCREENSHOT").ok(),
            last_media: (None, false),
            toast: None,
            last_track: None,
            track_at: 0.0,
            accent: [d.r() as f32, d.g() as f32, d.b() as f32],
            title: String::new(),
            listen_track: None,
            session_secs: 0.0,
            pending_secs: 0.0,
            last_tick: Instant::now(),
            recs: vec![],
            recs_for: None,
            recs_at: 0.0,
            fps_frames: 0,
            fps_since: Instant::now(),
            fps_last: Instant::now(),
            fps_worst: 0.0,
            fps_slow: 0,
            frame_start: Instant::now(),
            jobs: vec![],
            next_job: 1,
            yt_tx: yt_tx.clone(),
            yt_rx,
            tracklists: HashMap::new(),
            track_jobs: HashMap::new(),
            tl_tx,
            tl_rx,
            loud_tx,
            loud_rx,
            loud_pending: Default::default(),
            discos: HashMap::new(),
            disco_tx,
            disco_rx,
            album_jobs: HashMap::new(),
            pending: Default::default(),
            lyrics: HashMap::new(),
            lyr_tx,
            lyr_rx,
            menubar: if std::env::var("MH_SCREENSHOT").is_ok() || std::env::var("MH_FPS").is_ok() { None } else { crate::menubar::MenuBar::new(&cc.egui_ctx) },
            xmb: cc.gl.as_ref().and_then(|gl| match crate::xmb::Xmb::new(gl) {
                Ok(x) => Some(Arc::new(Mutex::new(x))),
                Err(e) => {
                    eprintln!("XMB background unavailable: {e}");
                    None
                }
            }),
        };
        // Pick up downloads that were still running when the app closed.
        if std::env::var("MH_SCREENSHOT").is_err() {
            let saved: Vec<Pending> = std::fs::read(pending_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            if !saved.is_empty() {
                let n = saved.len();
                app.dispatch(&cc.egui_ctx, saved);
                app.toast(&cc.egui_ctx, format!("Resuming {n} download{}", if n == 1 { "" } else { "s" }));
            }
        }
        // Dev helpers for screenshots.
        if let Ok(v) = std::env::var("MH_VIEW") {
            app.view = match v.as_str() {
                "albums" => View::Albums,
                "songs" => View::Songs,
                "now" => View::NowPlaying,
                "stats" => View::Stats,
                "artists" => View::Artists(app.lib.artists.keys().next().cloned()),
                "album" => app.lib.albums.first().map(|a| View::Album(a.key.clone())).unwrap_or(View::Albums),
                v if v.starts_with("artist:") => {
                    let q = v[7..].to_lowercase();
                    View::Artists(app.lib.artists.keys().find(|a| a.to_lowercase().contains(&q)).cloned())
                }
                v if v.starts_with("album:") => {
                    let q = v[6..].to_lowercase();
                    app.lib.albums.iter().find(|a| a.title.to_lowercase().contains(&q)).map(|a| View::Album(a.key.clone())).unwrap_or(View::Albums)
                }
                _ => View::RecentlyAdded,
            };
        }
        app.st.show_lyrics = std::env::var("MH_LYRICS").is_ok();
        app.st.show_mini = std::env::var("MH_MINI").is_ok();
        // Test screenshots: MH_LINK=demo opens Add from Link with sample rows.
        if std::env::var("MH_LINK").as_deref() == Ok("demo") {
            app.st.show_link = true;
            let mut a = LinkJob::new(90, "Music Is Mine".into());
            a.status = "Downloading".into();
            a.progress = Some(0.42);
            let mut b = LinkJob::new(91, "Follow Me".into());
            b.status = "“Follow Me” — Rochelle Jordan · Follow Me".into();
            b.done = true;
            let mut c = LinkJob::new(92, "Sea of Cloud".into());
            c.status = "no good match on YouTube".into();
            c.done = true;
            c.failed = true;
            let mut d = LinkJob::new(93, "Blue Calx".into());
            d.status = "Stopped".into();
            d.done = true;
            d.cancelled = true;
            app.jobs = vec![a, b, c, d];
        }
        if std::env::var("MH_PLAY").is_ok() {
            let ids: Vec<_> = app.lib.data.tracks.iter().map(|t| t.id).collect();
            let q = std::env::var("MH_PLAY").unwrap_or_default().to_lowercase();
            let start = app.lib.data.tracks.iter().position(|t| !q.is_empty() && t.title.to_lowercase().contains(&q)).unwrap_or(0);
            app.player.play_list(&app.lib, ids, start);
            if let Some(at) = std::env::var("MH_SEEK").ok().and_then(|v| v.parse::<f64>().ok()) {
                app.player.seek(at);
            }
            app.st.show_queue = std::env::var("MH_QUEUE").is_ok();
        }
        app.player.level_volume = app.lib.data.level_volume.unwrap_or(true);
        app.queue_loudness();
        let folders = app.lib.data.folders.clone();
        if !folders.is_empty() {
            app.start_scan(&cc.egui_ctx, folders);
        }
        app
    }

    fn start_scan(&mut self, ctx: &Context, roots: Vec<PathBuf>) {
        if self.scan_rx.is_some() {
            self.pending_scan.extend(roots);
            return;
        }
        let known: HashMap<PathBuf, Track> = self.lib.data.tracks.iter().map(|t| (t.path.clone(), t.clone())).collect();
        let (tx, rx) = channel();
        library::spawn_scan(roots, known, tx, ctx.clone());
        self.scan_rx = Some(rx);
        self.scan_progress = Some((0, 0));
    }

    fn pump_scan(&mut self, ctx: &Context) {
        let Some(rx) = &self.scan_rx else { return };
        let mut finished = None;
        while let Ok(m) = rx.try_recv() {
            match m {
                ScanMsg::Progress { done, total } => self.scan_progress = Some((done, total)),
                ScanMsg::Done { roots, tracks } => finished = Some((roots, tracks)),
            }
        }
        if let Some((roots, tracks)) = finished {
            let before = self.lib.data.tracks.len();
            self.lib.apply_scan(&roots, tracks);
            self.lib.save_if_dirty();
            let added = self.lib.data.tracks.len().saturating_sub(before);
            if added > 0 && before > 0 {
                self.toast(ctx, format!("Added {added} song{}", if added == 1 { "" } else { "s" }));
            }
            self.scan_rx = None;
            self.scan_progress = None;
            self.queue_loudness();
            if !self.pending_scan.is_empty() {
                let next = std::mem::take(&mut self.pending_scan);
                self.start_scan(ctx, next);
            }
        }
    }

    /// Queue every song without a loudness measurement for analysis.
    fn queue_loudness(&mut self) {
        for t in &self.lib.data.tracks {
            if t.loudness.is_none() && self.loud_pending.insert(t.id) {
                let _ = self.loud_tx.send((t.id, t.path.clone()));
            }
        }
    }

    fn pump_loudness(&mut self) {
        let mut refresh = false;
        let soon: Vec<TrackId> = self.player.current().into_iter().chain(self.player.up_next().into_iter().take(1)).collect();
        while let Ok((id, r)) = self.loud_rx.try_recv() {
            if let (Some((lufs, peak)), Some(t)) = (r, self.lib.track_mut(id)) {
                t.loudness = Some(lufs);
                t.peak = Some(peak);
                refresh |= soon.contains(&id);
            }
        }
        if refresh {
            self.player.refresh_gain(&self.lib);
        }
    }

    fn toast(&mut self, ctx: &Context, msg: String) {
        self.toast = Some((msg, ctx.input(|i| i.time)));
    }

    fn go(&mut self, ctx: &Context, v: View) {
        if v != self.view {
            self.history.push(std::mem::replace(&mut self.view, v));
            if self.history.len() > 50 {
                self.history.remove(0);
            }
            self.view_at = ctx.input(|i| i.time);
        }
    }

    fn apply(&mut self, ctx: &Context, acts: Vec<Action>) {
        let now = ctx.input(|i| i.time);
        let songs = |n: usize| format!("{n} song{}", if n == 1 { "" } else { "s" });
        for a in acts {
            match a {
                Action::Play(ids, i) => {
                    // A double-click sends two clicks; don't restart the song
                    // we just started.
                    let same = ids.get(i).copied() == self.player.current() && now - self.track_at < 0.6;
                    if !same {
                        self.player.play_list(&self.lib, ids, i);
                        self.track_at = now;
                    }
                }
                Action::Shuffle(ids) => self.player.shuffle_list(&self.lib, ids),
                Action::PlayNext(ids) => {
                    let n = ids.len();
                    for id in ids.into_iter().rev() {
                        self.player.play_next(id);
                    }
                    self.toast(ctx, format!("Playing next · {}", songs(n)));
                }
                Action::AddQueue(ids) => {
                    let n = ids.len();
                    for id in ids {
                        self.player.add_to_queue(id);
                    }
                    self.toast(ctx, format!("Added to queue · {}", songs(n)));
                }
                Action::Toggle => self.player.toggle(&self.lib),
                Action::Next => self.player.next(&self.lib),
                Action::Prev => self.player.prev(&self.lib),
                Action::Seek(s) => self.player.seek(s),
                Action::Volume(v) => {
                    self.player.set_volume(v);
                    self.lib.data.volume = Some(self.player.volume());
                    self.lib.mark_dirty();
                }
                Action::ToggleShuffle => self.player.toggle_shuffle(),
                Action::ToggleLoop => self.player.toggle_loop(),
                Action::Go(v) => self.go(ctx, v),
                Action::Back => {
                    if let Some(v) = self.history.pop() {
                        self.view = v;
                        self.view_at = ctx.input(|i| i.time);
                    }
                }
                Action::AddToPlaylist(pid, ids) => {
                    let n = ids.len();
                    let mut name = String::new();
                    if let Some(pl) = self.lib.playlist_mut(pid) {
                        pl.tracks.extend(ids);
                        name = pl.name.clone();
                    }
                    self.toast(ctx, format!("Added {} to “{name}”", songs(n)));
                }
                Action::NewPlaylist(ids) => {
                    let n = self.lib.data.playlists.len() + 1;
                    let name = format!("Playlist {n}");
                    let id = self.lib.new_playlist(name.clone());
                    if let Some(pl) = self.lib.playlist_mut(id) {
                        pl.tracks = ids;
                    }
                    self.st.renaming = Some((id, name));
                    self.go(ctx, View::Playlist(id));
                }
                Action::RemoveFromPlaylist(pid, i) => {
                    if let Some(pl) = self.lib.playlist_mut(pid) {
                        if i < pl.tracks.len() {
                            pl.tracks.remove(i);
                        }
                    }
                }
                Action::RenamePlaylist(pid, name) => {
                    if let Some(pl) = self.lib.playlist_mut(pid) {
                        if !name.trim().is_empty() {
                            pl.name = name.trim().to_string();
                        }
                    }
                }
                Action::DeletePlaylist(pid) => {
                    self.lib.data.playlists.retain(|pl| pl.id != pid);
                    self.lib.mark_dirty();
                    if self.view == View::Playlist(pid) {
                        self.view = View::RecentlyAdded;
                    }
                    self.history.retain(|v| *v != View::Playlist(pid));
                    self.toast(ctx, "Playlist deleted".into());
                }
                Action::ToggleLove(id) => {
                    if let Some(t) = self.lib.track_mut(id) {
                        t.loved = !t.loved;
                    }
                }
                Action::Reveal(path) => {
                    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
                }
                Action::AddFolderDialog => {
                    if let Some(dirs) = rfd::FileDialog::new()
                        .set_title("Add Music Folder")
                        .set_directory(dirs::audio_dir().unwrap_or_default())
                        .pick_folders()
                    {
                        self.add_paths(ctx, dirs);
                    }
                }
                Action::AddPaths(paths) => self.add_paths(ctx, paths),
                Action::RemoveFolder(f) => self.lib.remove_folder(&f),
                Action::Rescan => {
                    self.art.clear();
                    for t in &mut self.lib.data.tracks {
                        t.mtime = 0;
                    }
                    let folders = self.lib.data.folders.clone();
                    self.start_scan(ctx, folders);
                }
                Action::JumpUpNext(n) => self.player.jump_up_next(&self.lib, n),
                Action::RemoveUpNext(n) => self.player.remove_up_next(n),
                Action::AddLinks(text) => self.add_links(ctx, &text),
                Action::FetchTracklist(key, artist, album) => {
                    if !self.tracklists.contains_key(&key) {
                        self.tracklists.insert(key.clone(), Tracklist::Loading);
                        let (tx, wake) = (self.tl_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let tl = crate::ytdl::album_tracklist(&artist, &album);
                            let _ = tx.send((key, tl));
                            wake.request_repaint();
                        });
                    }
                }
                Action::DownloadMissing(key, tracks) => self.download_missing(ctx, &key, tracks),
                Action::StopDownload(which) => {
                    let mut n = 0;
                    for j in self.jobs.iter_mut().filter(|j| !j.done && which.map(|w| w == j.id).unwrap_or(true)) {
                        j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        j.status = "Stopping…".into();
                        n += 1;
                    }
                    if which.is_none() && n > 0 {
                        self.toast(ctx, format!("Stopped {n} download{}", if n == 1 { "" } else { "s" }));
                    }
                }
                Action::ClearFinishedDownloads => self.jobs.retain(|j| !j.done),
                Action::SetLevelVolume(on) => {
                    self.player.level_volume = on;
                    self.lib.data.level_volume = Some(on);
                    self.lib.mark_dirty();
                    self.player.refresh_gain(&self.lib);
                }
                Action::PlayNow(id) => {
                    self.player.play_next(id);
                    self.player.next(&self.lib);
                }
                Action::ToggleMini => self.st.show_mini = !self.st.show_mini,
                Action::ShowMain => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    crate::menubar::activate();
                }
                Action::ToggleLyrics => {
                    self.st.show_lyrics = !self.st.show_lyrics;
                    if self.st.show_lyrics && self.view != View::NowPlaying {
                        self.go(ctx, View::NowPlaying);
                    }
                }
                Action::FetchLyrics(id) => {
                    if let (false, Some(t)) = (self.lyrics.contains_key(&id), self.lib.track(id)) {
                        self.lyrics.insert(id, None);
                        let q = crate::lyrics::Query {
                            path: t.path.clone(),
                            title: t.title.clone(),
                            artist: t.artist.clone(),
                            album: t.album.clone(),
                            duration: t.duration_secs,
                        };
                        let (tx, wake) = (self.lyr_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send((id, crate::lyrics::find(&q)));
                            wake.request_repaint();
                        });
                    }
                }
                Action::FetchDiscography(artist) => {
                    if !self.discos.contains_key(&artist) {
                        self.discos.insert(artist.clone(), Disco::Loading);
                        let (tx, wake) = (self.disco_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let d = crate::ytdl::artist_albums(&artist);
                            let _ = tx.send((artist, d));
                            wake.request_repaint();
                        });
                    }
                }
                Action::DownloadCatalogAlbum(r) => {
                    let busy = self.album_jobs.get(&r.id).map(|id| self.jobs.iter().any(|j| j.id == *id && !j.done)).unwrap_or(false);
                    if !busy {
                        self.toast(ctx, format!("Getting “{}” from YouTube…", r.title));
                        self.dispatch(ctx, vec![Pending::Album(r.id, r.title)]);
                    }
                }
            }
        }
    }

    /// Start downloads for every link in `text` (one per line/space).
    fn add_links(&mut self, ctx: &Context, text: &str) {
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
    fn download_root(&mut self) -> PathBuf {
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
    fn dispatch(&mut self, ctx: &Context, specs: Vec<Pending>) {
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

    fn save_pending(&self) {
        let list: Vec<&Pending> = self.pending.values().collect();
        if let Ok(b) = serde_json::to_vec_pretty(&list) {
            let _ = std::fs::write(pending_path(), b);
        }
    }

    fn library_snapshot(&self) -> crate::ytdl::LibrarySnapshot {
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

    fn download_missing(&mut self, ctx: &Context, key: &str, tracks: Vec<crate::ytdl::CatalogTrack>) {
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

    fn pump_links(&mut self, ctx: &Context) {
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

    fn add_paths(&mut self, ctx: &Context, paths: Vec<PathBuf>) {
        let mut roots = vec![];
        for path in paths {
            let path = path.canonicalize().unwrap_or(path);
            if path.is_dir() || library::is_audio(&path) {
                if !self.lib.data.folders.iter().any(|f| path.starts_with(f)) {
                    if path.is_dir() {
                        self.lib.data.folders.retain(|f| !f.starts_with(&path));
                    }
                    self.lib.data.folders.push(path.clone());
                }
                roots.push(path);
            }
        }
        if !roots.is_empty() {
            self.lib.mark_dirty();
            self.start_scan(ctx, roots);
        }
    }

    fn handle_keys(&mut self, ctx: &Context, acts: &mut Vec<Action>) {
        let typing = ctx.wants_keyboard_input();
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            if cmd && i.key_pressed(Key::F) {
                self.st.focus_search = true;
            }
            if cmd && i.key_pressed(Key::ArrowRight) {
                acts.push(Action::Next);
            }
            if cmd && i.key_pressed(Key::ArrowLeft) {
                acts.push(Action::Prev);
            }
            if cmd && i.key_pressed(Key::ArrowUp) {
                acts.push(Action::Volume(self.player.volume() + 0.1));
            }
            if cmd && i.key_pressed(Key::ArrowDown) {
                acts.push(Action::Volume(self.player.volume() - 0.1));
            }
            if cmd && i.key_pressed(Key::U) {
                self.st.show_link = !self.st.show_link;
            }
            if cmd && i.key_pressed(Key::O) {
                acts.push(Action::AddFolderDialog);
            }
            if cmd && i.key_pressed(Key::L) {
                if let Some(a) = self.player.current().and_then(|id| self.lib.album_of(id)) {
                    acts.push(Action::Go(View::Album(a.key.clone())));
                }
            }
            if cmd && i.modifiers.shift && i.key_pressed(Key::M) {
                acts.push(Action::ToggleMini);
            }
            if cmd && i.key_pressed(Key::P) {
                acts.push(Action::Go(View::NowPlaying));
            }
            if cmd && i.key_pressed(Key::OpenBracket) {
                acts.push(Action::Back);
            }
            if !typing && !cmd {
                if i.key_pressed(Key::Space) {
                    acts.push(Action::Toggle);
                }
                if i.key_pressed(Key::L) {
                    acts.push(Action::ToggleLyrics);
                }
                if i.key_pressed(Key::ArrowRight) {
                    acts.push(Action::Seek(self.player.position() + 10.0));
                }
                if i.key_pressed(Key::ArrowLeft) {
                    acts.push(Action::Seek(self.player.position() - 10.0));
                }
                if i.key_pressed(Key::Enter) {
                    if let Some(id) = self.st.selected {
                        acts.push(Action::Play(vec![id], 0));
                    }
                }
            }
            if i.key_pressed(Key::Escape) {
                if !self.st.search.is_empty() {
                    self.st.search.clear();
                } else if self.view == View::NowPlaying {
                    acts.push(Action::Back);
                }
            }
            if i.pointer.button_pressed(egui::PointerButton::Extra1) {
                acts.push(Action::Back);
            }
        });
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            acts.push(Action::AddPaths(dropped));
        }
    }

    fn sync_media(&mut self) {
        let state = (self.player.current(), self.player.is_playing());
        let Some(media) = &mut self.media else { return };
        if let Some(cmd) = media.poll() {
            use crate::media::Cmd;
            match cmd {
                Cmd::Play => self.player.play(&self.lib),
                Cmd::Pause => self.player.pause(),
                Cmd::Toggle => self.player.toggle(&self.lib),
                Cmd::Next => self.player.next(&self.lib),
                Cmd::Prev => self.player.prev(&self.lib),
                Cmd::Seek(s) => self.player.seek(s),
                Cmd::SeekBy(d) => self.player.seek(self.player.position() + d),
            }
            self.last_media = (None, false);
        }
        if state != self.last_media {
            self.last_media = state;
            let t = state.0.and_then(|id| self.lib.track(id));
            let cover = t
                .map(|t| crate::art::cache_path(&t.album_key()))
                .filter(|path| path.exists())
                .map(|path| format!("file://{}", path.display()));
            media.update(t, cover, self.player.is_playing(), self.player.position());
        }
    }

    /// Accumulate real listening time per track, and count early bail-outs
    /// as skips. Wall-clock based so it's right even when frames are sparse.
    fn track_listening(&mut self) {
        let elapsed = self.last_tick.elapsed().as_secs_f64().min(120.0);
        self.last_tick = Instant::now();
        let cur = self.player.current();
        if cur != self.listen_track {
            if let Some(prev) = self.listen_track {
                let session = self.session_secs;
                let pending = self.pending_secs;
                self.lib.log_listening(pending);
                if let Some(t) = self.lib.track_mut(prev) {
                    t.listened_secs += pending;
                    if session < 30.0 && session < t.duration_secs * 0.5 && t.duration_secs > 0.0 {
                        t.skips += 1;
                    }
                }
            }
            self.listen_track = cur;
            self.session_secs = 0.0;
            self.pending_secs = 0.0;
        }
        if self.player.is_playing() {
            self.session_secs += elapsed;
            self.pending_secs += elapsed;
            if self.pending_secs >= 10.0 {
                let p = std::mem::take(&mut self.pending_secs);
                if let Some(t) = cur.and_then(|id| self.lib.track_mut(id)) {
                    t.listened_secs += p;
                }
                self.lib.log_listening(p);
            }
        }
    }

    /// Glide the accent colour toward the current cover's colour.
    fn update_accent(&mut self, ctx: &Context, dt: f32) -> bool {
        let target = self
            .player
            .current()
            .and_then(|id| self.lib.album_of(id))
            .and_then(|a| {
                let path = self.lib.track(a.tracks[0])?.path.clone();
                match self.art.get(&a.key, &path, &a.artist, &a.title) {
                    Art::Ready { tint, .. } => Some(vivid(*tint)),
                    _ => None,
                }
            })
            .unwrap_or(DEFAULT_ACCENT);
        let tgt = [target.r() as f32, target.g() as f32, target.b() as f32];
        let k = (dt * 3.0).min(1.0);
        let mut moving = false;
        for i in 0..3 {
            let d = tgt[i] - self.accent[i];
            if d.abs() > 0.5 {
                moving = true;
                self.accent[i] += d * k;
            } else {
                self.accent[i] = tgt[i];
            }
        }
        let c = Color32::from_rgb(self.accent[0] as u8, self.accent[1] as u8, self.accent[2] as u8);
        if c != accent() {
            set_accent(c);
            apply_visuals(ctx);
        }
        moving
    }
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        // Frame limiter. When the window is visible, vsync paces us (120Hz on
        // ProMotion) and this never kicks in. When macOS stops pacing (window
        // covered by others), frames would otherwise run back-to-back.
        let since = self.frame_start.elapsed();
        let min_frame = Duration::from_micros(7_500);
        if since < min_frame {
            std::thread::sleep(min_frame - since);
        }
        self.frame_start = Instant::now();
        self.frame += 1;
        let on_screen = window_on_screen(frame);
        // MH_FPS=1 prints the measured frame rate once a second.
        if std::env::var("MH_FPS").is_ok() {
            self.fps_frames += 1;
            let gap = self.fps_last.elapsed().as_secs_f64() * 1000.0;
            self.fps_last = Instant::now();
            self.fps_worst = self.fps_worst.max(gap);
            if gap > 12.0 {
                self.fps_slow += 1;
            }
            if self.fps_since.elapsed() >= Duration::from_secs(1) {
                eprintln!(
                    "fps: {:.1}  worst frame gap: {:.1}ms  slow frames (>12ms): {}",
                    self.fps_frames as f64 / self.fps_since.elapsed().as_secs_f64(),
                    self.fps_worst,
                    self.fps_slow
                );
                self.fps_slow = 0;
                self.fps_worst = 0.0;
                self.fps_frames = 0;
                self.fps_since = Instant::now();
            }
        }
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.art.pump(ctx);
        self.pump_scan(ctx);
        self.pump_links(ctx);
        self.pump_loudness();
        if let Some(name) = self.player.device_changed.take() {
            if std::env::var("MH_DEVTEST").is_ok() {
                eprintln!("device switch → {name}: resumed {:?} at {:.2}s, playing={}", self.player.current(), self.player.position(), self.player.is_playing());
            }
            self.toast(ctx, format!("Now playing on {name}"));
        }
        // Testing aid: MH_DEVTEST=1 simulates an output switch 3 s in.
        if std::env::var("MH_DEVTEST").is_ok() && self.frame % 60 == 0 {
            eprintln!("t={now:.1}s pos {:.2}s current {:?}", self.player.position(), self.player.current());
            if (now - 3.0).abs() < 0.6 {
                self.player.debug_force_device_change();
            }
        }
        self.player.tick(&self.lib);
        if let Some(id) = self.player.just_started.take() {
            if let Some(t) = self.lib.track_mut(id) {
                t.play_count += 1;
                t.last_played = library::now_secs();
            }
        }
        if self.player.current() != self.last_track {
            self.last_track = self.player.current();
            self.track_at = now;
        }
        if let Some(err) = self.player.error.take() {
            self.toast(ctx, err);
        }
        self.sync_media();
        self.track_listening();
        let playing = self.player.is_playing();
        if self.player.current() != self.recs_for || now - self.recs_at > 20.0 {
            self.recs_for = self.player.current();
            self.recs_at = now;
            self.recs = match self.player.current() {
                Some(cur) => {
                    let exclude: std::collections::HashSet<TrackId> = self.player.up_next().into_iter().take(5).collect();
                    crate::recommend::recommend(&self.lib, cur, &exclude, library::now_secs(), 12)
                }
                None => vec![],
            };
        }
        let accent_moving = self.update_accent(ctx, dt);

        let mut acts = vec![];
        self.handle_keys(ctx, &mut acts);
        if let Some(mb) = &mut self.menubar {
            use crate::menubar::Cmd;
            let label = match self.player.current().and_then(|id| self.lib.track(id)) {
                Some(t) => format!("{} — {}", t.title, t.artist),
                None => String::new(),
            };
            mb.update(&label, playing);
            match mb.poll() {
                Some(Cmd::Toggle) => acts.push(Action::Toggle),
                Some(Cmd::Next) => acts.push(Action::Next),
                Some(Cmd::Prev) => acts.push(Action::Prev),
                Some(Cmd::Show) => acts.push(Action::ShowMain),
                Some(Cmd::Mini) => acts.push(Action::ToggleMini),
                None => {}
            }
        }

        let title = match self.player.current().and_then(|id| self.lib.track(id)) {
            Some(t) => format!("{} — {}", t.title, t.artist),
            None => "Music Host".into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        let mut v = V {
            lib: &self.lib,
            art: &mut self.art,
            player: &self.player,
            st: &mut self.st,
            acts,
            view: &self.view,
            now,
            view_at: self.view_at,
            recs: &self.recs,
            xmb: self.xmb.clone(),
            jobs: &self.jobs,
            tracklists: &self.tracklists,
            track_jobs: &self.track_jobs,
            discos: &self.discos,
            album_jobs: &self.album_jobs,
            lyrics: &self.lyrics,
        };
        v.player_bar(ctx, self.track_at);
        v.sidebar(ctx, self.scan_progress, !self.history.is_empty());
        if v.st.show_queue {
            v.queue_panel(ctx);
        }
        // Page transition: fade + gentle rise.
        let view_t = ease(((now - self.view_at) / 0.32) as f32);
        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG)).show(ctx, |ui| {
            let full = ui.max_rect();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(full.translate(vec2(0.0, (1.0 - view_t) * 14.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            child.multiply_opacity(view_t);
            child.set_clip_rect(full);
            v.main(&mut child);
            // Test screenshots: MH_MINI_PREVIEW draws the mini player in the page.
            if std::env::var("MH_MINI_PREVIEW").is_ok() {
                let r = Rect::from_min_size(full.min + vec2(40.0, 40.0), vec2(380.0, 104.0));
                ui.painter().rect_filled(r, cr(10), Color32::from_rgb(22, 22, 26));
                let mut mui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                v.mini_contents(&mut mui, ctx);
            }
        });
        if v.st.show_mini {
            v.mini_player(ctx);
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            drop_overlay(ctx, now);
        }
        let acts = std::mem::take(&mut v.acts);
        self.apply(ctx, acts);

        if let Some((msg, at)) = &self.toast {
            if now - at > 2.8 {
                self.toast = None;
            } else {
                draw_toast(ctx, msg, now - at);
            }
        }

        if self.last_save.elapsed() > Duration::from_secs(5) {
            self.lib.save_if_dirty();
            self.last_save = Instant::now();
        }
        // Smooth while anything moves; nearly idle otherwise.
        let transition = now - self.view_at < 0.6 || self.toast.is_some() || accent_moving;
        if !on_screen {
            // Covered or hidden: nothing to animate, just keep time moving.
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.view == View::NowPlaying || transition {
            // Animated backdrop / transitions at the display's refresh rate
            // (120Hz on ProMotion). Covered windows are handled above.
            // Redraw immediately; vsync paces it. (A timed deadline here lands
            // right on the refresh boundary and occasionally misses it.)
            ctx.request_repaint();
        } else if playing {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        if let Some(path) = self.shot.clone() {
            let at: u64 = std::env::var("MH_SHOT_FRAME").ok().and_then(|f| f.parse().ok()).unwrap_or(150);
            if self.frame == at {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            if std::env::var("MH_FPS").is_err() {
                ctx.request_repaint_after(Duration::from_micros(8_000));
            }
            let img = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = img {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    let _ = buf.save(&path);
                }
                self.shot = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.lib.data.volume = Some(self.player.volume());
        self.lib.mark_dirty();
        self.lib.save_if_dirty();
    }
}

/// Is any part of our window actually visible? (macOS occlusion state.)
/// Lets the animated backdrop stop rendering when nobody can see it.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // from the objc macros
fn window_on_screen(frame: &eframe::Frame) -> bool {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    const NS_WINDOW_OCCLUSION_STATE_VISIBLE: u64 = 1 << 1;
    let Ok(handle) = frame.window_handle() else { return true };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return true };
    unsafe {
        let view = h.ns_view.as_ptr() as *mut Object;
        let window: *mut Object = msg_send![view, window];
        if window.is_null() {
            return true;
        }
        let state: u64 = msg_send![window, occlusionState];
        state & NS_WINDOW_OCCLUSION_STATE_VISIBLE != 0
    }
}

#[cfg(not(target_os = "macos"))]
fn window_on_screen(_frame: &eframe::Frame) -> bool {
    true
}

// ---------------------------------------------------------------- overlays

fn overlay_painter(ctx: &Context, name: &str) -> egui::Painter {
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new(name)))
}

fn drop_overlay(ctx: &Context, now: f64) {
    let screen = ctx.screen_rect();
    let p = overlay_painter(ctx, "drop");
    p.rect_filled(screen, 0.0, Color32::from_black_alpha(190));
    let pulse = ((now * 3.0).sin() as f32 + 1.0) * 0.5;
    let r = Rect::from_center_size(screen.center(), vec2(460.0, 200.0) + vec2(10.0, 10.0) * pulse);
    p.rect_filled(r, cr(22), alpha(accent(), 0.10));
    p.rect_stroke(r, cr(22), Stroke::new(2.0f32, accent()), egui::StrokeKind::Inside);
    p.text(screen.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, "Drop to add to your library", bold(22.0), Color32::WHITE);
    p.text(screen.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, "Folders and audio files", font(13.0), DIM);
    ctx.request_repaint();
}

fn draw_toast(ctx: &Context, msg: &str, age: f64) {
    let screen = ctx.screen_rect();
    let k_in = ease((age / 0.28) as f32);
    let k_out = 1.0 - ease(((age - 2.4) / 0.4).max(0.0) as f32);
    let k = k_in.min(k_out);
    let p = overlay_painter(ctx, "toast");
    let galley = p.layout_no_wrap(msg.to_owned(), font(13.5), Color32::WHITE);
    let size = vec2(galley.size().x + 40.0, 38.0);
    let center = pos2(screen.center().x + 114.0, screen.bottom() - 92.0 - 34.0 + (1.0 - k) * 16.0);
    let r = Rect::from_center_size(center, size);
    let s = egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha((120.0 * k) as u8) };
    p.add(s.as_shape(r, cr(19)));
    p.rect_filled(r, cr(19), alpha(Color32::from_rgb(48, 48, 54), 0.97 * k));
    p.circle_filled(pos2(r.left() + 18.0, r.center().y), 3.5, alpha(accent(), k));
    p.text(pos2(r.left() + 28.0, r.center().y), Align2::LEFT_CENTER, msg, font(13.5), alpha(Color32::WHITE, k));
}

// ---------------------------------------------------------------- views

const TILE_GAP: f32 = 24.0;
const PAD: f32 = 32.0;

impl<'a> V<'a> {
    fn art_state(&mut self, album: &Album) -> Option<(egui::TextureId, egui::TextureId, Color32, f32)> {
        let path = self.lib.track(album.tracks[0]).map(|t| t.path.clone()).unwrap_or_default();
        match self.art.get(&album.key, &path, &album.artist, &album.title) {
            Art::Ready { tex, blur, tint, loaded_at } => {
                let k = ease(((self.now - *loaded_at) / 0.45) as f32);
                Some((tex.id(), blur.id(), *tint, k))
            }
            _ => None,
        }
    }

    /// Album art with a fade-in once it arrives. Returns the average colour.
    fn album_art(&mut self, ui: &Ui, album: &Album, rect: Rect, radius: u8) -> Option<Color32> {
        match self.art_state(album) {
            Some((tex, _, tint, k)) => {
                if k < 1.0 {
                    placeholder(ui, rect, radius, &album.title, false);
                    ui.ctx().request_repaint();
                }
                egui::Image::new((tex, rect.size()))
                    .corner_radius(cr(radius))
                    .tint(Color32::from_white_alpha((k * 255.0) as u8))
                    .paint_at(ui, rect);
                Some(tint)
            }
            None => {
                let path = self.lib.track(album.tracks[0]).map(|t| t.path.clone()).unwrap_or_default();
                let loading = matches!(self.art.get(&album.key, &path, &album.artist, &album.title), Art::Loading);
                placeholder(ui, rect, radius, &album.title, loading);
                None
            }
        }
    }

    /// Like `album_art`, but swaps in the full-resolution cover once loaded.
    fn album_art_hd(&mut self, ui: &Ui, album: &Album, rect: Rect, radius: u8) {
        self.album_art(ui, album, rect, radius);
        let path = self.lib.track(album.tracks[0]).map(|t| t.path.clone()).unwrap_or_default();
        if let Some((tex, at)) = self.art.get_hd(&album.key, &path, &album.artist, &album.title, self.now) {
            let k = ease(((self.now - at) / 0.3) as f32);
            if k < 1.0 {
                ui.ctx().request_repaint();
            }
            egui::Image::new((tex, rect.size()))
                .corner_radius(cr(radius))
                .tint(Color32::from_white_alpha((k * 255.0) as u8))
                .paint_at(ui, rect);
        }
    }

    fn track_art(&mut self, ui: &Ui, id: TrackId, rect: Rect, radius: u8) {
        if let Some(a) = self.lib.album_of(id).cloned() {
            self.album_art(ui, &a, rect, radius);
        } else {
            placeholder(ui, rect, radius, "", false);
        }
    }

    /// Apple-style ambient wash: the blurred cover stretched behind `rect`,
    /// darkened and faded into the background at the bottom.
    fn ambient(&mut self, ui: &Ui, album: &Album, rect: Rect, strength: f32) {
        let Some((_, blur, _, k)) = self.art_state(album) else { return };
        let a = (strength * k).clamp(0.0, 1.0);
        // Oversize so the blur's soft edges fall outside the visible area.
        let big = rect.expand2(vec2(rect.width() * 0.15, rect.height() * 0.25));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .image(blur, big, uv, Color32::from_white_alpha((a * 255.0) as u8));
        // Darken for legibility, then fade to the page colour.
        ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(90));
        let mut mesh = egui::Mesh::default();
        let fade_top = rect.top() + rect.height() * 0.45;
        mesh.colored_vertex(pos2(rect.left(), fade_top), alpha(BG, 0.0));
        mesh.colored_vertex(pos2(rect.right(), fade_top), alpha(BG, 0.0));
        mesh.colored_vertex(rect.left_bottom(), BG);
        mesh.colored_vertex(rect.right_bottom(), BG);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);
    }

    // ------------------------------------------------------------ player bar

    fn player_bar(&mut self, ctx: &Context, track_at: f64) {
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
    fn progress_row(&mut self, ui: &mut Ui, cur: Option<&Track>, width: f32, size: f32) {
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

    fn heart(&mut self, ui: &Ui, t: &Track, c: egui::Pos2, size: f32) {
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

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ctx: &Context, scan: Option<(usize, usize)>, can_back: bool) {
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

    fn link_window(&mut self, ctx: &Context) {
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

                if crate::ytdl::ytdlp_path().is_none() {
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
                            if let Ok(out) = std::process::Command::new("pbpaste").output() {
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
    fn download_row(&mut self, ui: &mut Ui, j: &LinkJob) {
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

    fn folders_window(&mut self, ctx: &Context) {
        let mut open = true;
        egui::Window::new("Music Folders")
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
            });
        if !open {
            self.st.show_folders = false;
        }
    }

    fn nav(&mut self, ui: &mut Ui, icon: NavIcon, label: &str, target: View) -> egui::Response {
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

    // ------------------------------------------------------------ queue

    fn queue_panel(&mut self, ctx: &Context) {
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

    // ------------------------------------------------------------ main area

    fn main(&mut self, ui: &mut Ui) {
        if self.lib.data.tracks.is_empty() {
            self.empty_state(ui);
            return;
        }
        match self.view.clone() {
            View::NowPlaying => self.now_playing(ui),
            View::RecentlyAdded => {
                let mut albums: Vec<usize> = (0..self.lib.albums.len()).collect();
                albums.sort_by_key(|&i| std::cmp::Reverse(self.lib.albums[i].added_at));
                self.album_grid(ui, "Recently Added", &albums, None, &[]);
            }
            View::Albums => {
                let albums: Vec<usize> = (0..self.lib.albums.len()).collect();
                self.album_grid(ui, "Albums", &albums, None, &[]);
            }
            View::Artists(sel) => self.artists(ui, sel),
            View::Songs => {
                let mut ids: Vec<TrackId> = self.lib.data.tracks.iter().map(|t| t.id).collect();
                self.sort_songs(&mut ids);
                self.song_page(ui, "Songs", ids, None, true);
            }
            View::Loved => {
                let ids: Vec<TrackId> = self.lib.data.tracks.iter().filter(|t| t.loved).map(|t| t.id).collect();
                if ids.is_empty() {
                    heading(ui, "Loved");
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new("Tap ♡ on any song to love it.").color(DIM)));
                } else {
                    self.song_page(ui, "Loved", ids, None, false);
                }
            }
            View::Stats => self.stats(ui),
            View::Album(key) => self.album_page(ui, &key),
            View::Playlist(id) => {
                let Some(pl) = self.lib.data.playlists.iter().find(|pl| pl.id == id) else { return };
                let (name, ids) = (pl.name.clone(), pl.tracks.clone());
                self.song_page(ui, &name, ids, Some(id), false);
            }
            View::Search => self.search(ui),
        }
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        let c = ui.max_rect().center();
        let float = (self.now * 1.4).sin() as f32 * 4.0;
        let disc = Rect::from_center_size(c - vec2(0.0, 96.0 - float), vec2(84.0, 84.0));
        shadow(ui, disc, 42, 30, 8, 0.5);
        ui.painter().circle_filled(disc.center(), 42.0, accent());
        ui.painter().text(disc.center(), Align2::CENTER_CENTER, "♫", font(40.0), Color32::WHITE);
        ui.painter().text(c - vec2(0.0, 16.0), Align2::CENTER_CENTER, "Add Your Music", bold(28.0), TEXT);
        ui.painter().text(c + vec2(0.0, 18.0), Align2::CENTER_CENTER, "Choose a folder of songs. Album art is pulled from your files automatically.", font(14.0), DIM);
        ui.painter().text(c + vec2(0.0, 38.0), Align2::CENTER_CENTER, "MP3 · AAC/M4A · ALAC · FLAC · WAV · AIFF · OGG", font(12.0), FAINT);
        let mut bui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(c + vec2(0.0, 86.0), vec2(220.0, 40.0)))
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        if pill(&mut bui, None, "Choose Folder…", true).clicked() {
            self.acts.push(Action::AddFolderDialog);
        }
        ui.painter().text(c + vec2(0.0, 130.0), Align2::CENTER_CENTER, "or drag a folder onto this window", font(12.0), FAINT);
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    fn album_grid(&mut self, ui: &mut Ui, title: &str, albums: &[usize], header: Option<&dyn Fn(&mut Ui)>, missing: &[crate::ytdl::CatalogAlbumRef]) {
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
    fn missing_tile(&mut self, ui: &mut Ui, r: &crate::ytdl::CatalogAlbumRef, art: Rect) {
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

    fn album_tile(&mut self, ui: &mut Ui, ai: usize, art: Rect) {
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

    fn album_menu(&mut self, resp: &egui::Response, album: &Album) {
        resp.context_menu(|ui| self.album_menu_items(ui, album));
    }

    fn album_menu_items(&mut self, ui: &mut Ui, album: &Album) {
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
        if ui.button("Show in Finder").clicked() {
            if let Some(t) = self.lib.track(album.tracks[0]) {
                self.acts.push(Action::Reveal(t.path.clone()));
            }
            ui.close_menu();
        }
    }

    fn playlist_submenu(&mut self, ui: &mut Ui, ids: Vec<TrackId>) {
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

    fn album_page(&mut self, ui: &mut Ui, key: &str) {
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
            if !missing.is_empty() && crate::ytdl::ytdlp_path().is_some() {
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
    fn missing_tracks(&mut self, album: &Album) -> Vec<crate::ytdl::CatalogTrack> {
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
    fn missing_track_row(&mut self, ui: &mut Ui, album: &Album, m: &crate::ytdl::CatalogTrack, i: usize) {
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
    fn row_bg(&mut self, ui: &Ui, row: Rect, id: Id, hovered: bool, selected: bool, i: usize) {
        let h = anim(ui, id.with("h"), hovered, 0.12);
        let s = anim(ui, id.with("s"), selected, 0.15);
        let base = if i % 2 == 1 { ROW_ALT } else { alpha(BG, 0.0) };
        let c = lerp(lerp(base, HOVER, h), alpha(accent(), 0.28), s);
        if c.a() > 0 {
            ui.painter().rect_filled(row, cr(8), c);
        }
    }

    fn album_track_row(&mut self, ui: &mut Ui, t: &Track, i: usize, list: &[TrackId]) {
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

    fn love_and_time(&mut self, ui: &mut Ui, t: &Track, row: Rect, hovered: bool) {
        text_right(ui, pos2(row.right() - 14.0, row.center().y - 8.0), &fmt_time(t.duration_secs), font(13.0), DIM);
        if t.loved || hovered {
            self.heart(ui, t, pos2(row.right() - 80.0, row.center().y), 14.0);
        }
    }

    fn track_menu(&mut self, resp: &egui::Response, t: &Track, list: &[TrackId], i: usize, playlist: Option<u64>) {
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
            if ui.button("Show in Finder").clicked() {
                self.acts.push(Action::Reveal(t.path.clone()));
                ui.close_menu();
            }
        });
    }

    fn sort_songs(&self, ids: &mut [TrackId]) {
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
    fn song_page(&mut self, ui: &mut Ui, title: &str, ids: Vec<TrackId>, playlist: Option<u64>, sortable: bool) {
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

    fn song_row(&mut self, ui: &mut Ui, t: &Track, i: usize, list: &[TrackId], cols: &Cols, playlist: Option<u64>) {
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

    fn artists(&mut self, ui: &mut Ui, sel: Option<String>) {
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

    fn search(&mut self, ui: &mut Ui) {
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

    // ------------------------------------------------------------ now playing

    /// Full-screen player: blurred cover + XMB wave backdrop, cover and
    /// controls up top, "Recommended for You" along the bottom.
    fn now_playing(&mut self, ui: &mut Ui) {
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

            let art_size = (col_w * 0.82).min(inner.height() - 270.0).max(110.0);
            let block_h = art_size + 270.0;
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

    fn np_art(&mut self, ui: &mut Ui, album: Option<&Album>, art: Rect) {
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
    fn np_info(&mut self, ui: &mut Ui, t: &Track, tx: f32, mut y: f32, right_w: f32) {
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

    /// Listening stats: totals, the last 30 days, and your top songs,
    /// artists and albums by time actually listened.
    fn stats(&mut self, ui: &mut Ui) {
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

    /// Small always-on-top window: cover, title, transport, progress.
    fn mini_player(&mut self, ctx: &Context) {
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

    fn mini_contents(&mut self, ui: &mut Ui, mctx: &Context) {
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

    /// Lyrics beside the cover: synced lines scroll along with the song and
    /// the current one lights up; click a line to jump there.
    fn lyrics_panel(&mut self, ui: &mut Ui, r: Rect, t: &Track) {
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
    fn xmb_background(&mut self, ui: &Ui, rect: Rect) {
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
    fn recommendations(&mut self, ui: &mut Ui, r: Rect) {
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

struct Cols {
    title: f32,
    artist: f32,
    album: f32,
    end: f32,
}

impl Cols {
    fn new(w: f32) -> Self {
        let start = 54.0;
        let end = w - 110.0;
        let span = end - start;
        Cols {
            title: start,
            artist: start + span * 0.42,
            album: start + span * 0.70,
            end,
        }
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "3h 12m", "45m", "20s".
fn long_dur(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if secs < 60.0 {
        format!("{}s", secs.round() as u64)
    } else if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {}m", m / 60, m % 60)
    }
}

fn tracks_by(lib: &Library, artist: &str) -> usize {
    lib.data.tracks.iter().filter(|t| t.album_artist == artist).count()
}

fn heading(ui: &mut Ui, s: &str) {
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(egui::RichText::new(s).font(bold(30.0)));
    });
}

fn subhead(ui: &mut Ui, s: &str) {
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(egui::RichText::new(s).font(bold(20.0)));
    });
    ui.add_space(8.0);
}

fn section(ui: &mut Ui, s: &str) {
    ui.label(egui::RichText::new(s).font(bold(11.5)).color(FAINT));
    ui.add_space(2.0);
}

/// A small painted ✕ (the glyph isn't in the UI font).
/// Two overlapping windows: toggles the mini player.
fn mini_button(ui: &mut Ui, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let on = anim(ui, resp.id.with("on"), active, 0.2);
    let h = anim(ui, resp.id, resp.hovered(), 0.15);
    let color = lerp(lerp(DIM, TEXT, h), accent(), on);
    let st = Stroke::new(1.5f32, color);
    let c = rect.center() - vec2(0.0, 2.0);
    ui.painter().rect_stroke(Rect::from_center_size(c, vec2(16.0, 12.0)), cr(2), st, egui::StrokeKind::Middle);
    ui.painter().rect_filled(Rect::from_min_size(c + vec2(0.0, 0.5), vec2(7.0, 5.0)), cr(1), color);
    if on > 0.0 {
        ui.painter().circle_filled(pos2(rect.center().x, rect.bottom() - 3.0), 2.0 * on, alpha(accent(), on));
    }
    resp.on_hover_text("Mini Player (⇧⌘M)").on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_x(ui: &Ui, c: egui::Pos2, r: f32, color: Color32) {
    let st = Stroke::new(1.6f32, color);
    ui.painter().line_segment([c + vec2(-r, -r), c + vec2(r, r)], st);
    ui.painter().line_segment([c + vec2(-r, r), c + vec2(r, -r)], st);
}

/// Animated equalizer bars for the currently-playing item.
fn eq_bars(ui: &Ui, c: egui::Pos2, playing: bool, now: f64, color: Color32) {
    let t = now as f32;
    for k in 0..3 {
        let phase = k as f32 * 1.7;
        let h = if playing { 4.0 + 8.0 * (0.5 + 0.5 * (t * (5.5 + k as f32) + phase).sin()) } else { 4.0 };
        let x = c.x - 6.0 + k as f32 * 5.0;
        ui.painter().rect_filled(Rect::from_min_max(pos2(x, c.y + 6.0 - h), pos2(x + 3.0, c.y + 6.0)), cr(1), color);
    }
}

/// Gradient placeholder for albums without artwork (shimmers while loading).
fn placeholder(ui: &Ui, rect: Rect, radius: u8, seed: &str, loading: bool) {
    let mut h: u32 = 2166136261;
    for b in seed.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    let top: Color32 = egui::ecolor::Hsva::new(hue, 0.28, 0.32, 1.0).into();
    let bottom: Color32 = egui::ecolor::Hsva::new((hue + 0.06) % 1.0, 0.35, 0.18, 1.0).into();
    let (top, bottom) = if seed.is_empty() { (Color32::from_gray(52), Color32::from_gray(40)) } else { (top, bottom) };
    ui.painter().rect_filled(rect, cr(radius), bottom);
    let mut upper = rect;
    upper.set_bottom(rect.center().y + rect.height() * 0.15);
    ui.painter().rect_filled(upper, egui::CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 }, lerp(top, bottom, 0.25));
    if loading {
        let t = ui.input(|i| i.time) as f32;
        let a = 0.04 + 0.04 * (t * 3.0).sin();
        ui.painter().rect_filled(rect, cr(radius), Color32::from_white_alpha((a * 255.0) as u8));
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    } else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "♪", font((rect.height() * 0.32).max(10.0)), Color32::from_white_alpha(80));
    }
}
