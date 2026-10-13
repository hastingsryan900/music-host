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

mod downloads;
mod library_ui;
mod now_playing;
mod panels;
mod stats;
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
    /// Turn "Listen on your iPhone" on or off.
    SetStream(bool),
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
    /// Phone streaming: Ok(url) while on, Err(reason) if it couldn't start.
    stream: Option<Result<&'a str, &'a str>>,
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
    stream: Option<Result<crate::stream::Server, String>>,
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
            stream: None,
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
                "settings" => {
                    app.st.show_folders = true;
                    if std::env::var("MH_STREAM").is_ok() {
                        app.stream = Some(crate::stream::Server::start(app.stream_catalog()));
                    }
                    View::RecentlyAdded
                }
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
        if app.lib.data.stream == Some(true) && std::env::var("MH_SCREENSHOT").is_err() {
            app.stream = Some(crate::stream::Server::start(app.stream_catalog()));
        }
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
            if let Some(Ok(s)) = &self.stream {
                s.set_catalog(self.stream_catalog());
            }
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
                Action::SetStream(on) => {
                    self.lib.data.stream = Some(on);
                    self.lib.mark_dirty();
                    self.stream = on.then(|| crate::stream::Server::start(self.stream_catalog()));
                }
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

    /// The library as the phone player sees it.
    fn stream_catalog(&self) -> crate::stream::Catalog {
        let mut c = crate::stream::Catalog::default();
        let mut albums = vec![];
        for a in &self.lib.albums {
            let id = crate::art::cache_path(&a.key).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let mut tracks = vec![];
            for tid in &a.tracks {
                let Some(t) = self.lib.track(*tid) else { continue };
                c.tracks.insert(t.id, t.path.clone());
                tracks.push(serde_json::json!({ "id": t.id.to_string(), "title": t.title, "artist": t.artist, "duration": t.duration_secs }));
            }
            let Some(first) = a.tracks.first().and_then(|id| self.lib.track(*id)) else { continue };
            c.covers.insert(id.clone(), (crate::art::cache_path(&a.key), first.path.clone()));
            albums.push(serde_json::json!({ "id": id, "title": a.title, "artist": a.artist, "year": a.year, "genre": a.genre, "added": a.added_at, "tracks": tracks }));
        }
        albums.sort_by_key(|a| std::cmp::Reverse(a["added"].as_u64().unwrap_or(0)));
        c.json = serde_json::json!({ "albums": albums }).to_string();
        c
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
            stream: self.stream.as_ref().map(|s| s.as_ref().map(|s| s.url.as_str()).map_err(|e| e.as_str())),
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

    // ------------------------------------------------------------ sidebar

    // ------------------------------------------------------------ queue

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

    // ------------------------------------------------------------ now playing

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
/// A scannable QR code (dark modules on a white card).
fn qr_code(ui: &mut Ui, data: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(rect, cr(8), Color32::WHITE);
    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else { return };
    let n = code.width();
    let colors = code.to_colors();
    let quiet = 2.0;
    let cell = size / (n as f32 + quiet * 2.0);
    for y in 0..n {
        for x in 0..n {
            if colors[y * n + x] == qrcode::Color::Dark {
                let min = rect.min + vec2((x as f32 + quiet) * cell, (y as f32 + quiet) * cell);
                ui.painter().rect_filled(Rect::from_min_size(min, vec2(cell + 0.3, cell + 0.3)), 0.0, Color32::from_rgb(18, 18, 20));
            }
        }
    }
}

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
