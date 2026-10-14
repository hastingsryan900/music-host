//! Music library: scanning folders, reading tags, grouping into albums/artists,
//! and persisting everything to disk so launches are instant.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "aac", "mp4", "flac", "wav", "aif", "aiff", "ogg", "oga", "opus", "alac", "caf",
];
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork", "art"];

pub type TrackId = u64;

const SCAN_VERSION: u32 = 4;
pub type AlbumKey = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_secs: f64,
    pub added_at: u64,
    pub mtime: u64,
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub last_played: u64,
    #[serde(default)]
    pub loved: bool,
    /// Total seconds actually listened (drives recommendations).
    #[serde(default)]
    pub listened_secs: f64,
    /// Times this was abandoned early (under 30s and under half its length).
    #[serde(default)]
    pub skips: u32,
    /// Integrated loudness (LUFS) and sample peak, for volume levelling.
    #[serde(default)]
    pub loudness: Option<f32>,
    #[serde(default)]
    pub peak: Option<f32>,
    /// Whether album_artist came from a tag (vs. derived from the artist).
    #[serde(default)]
    pub album_artist_tagged: bool,
}

impl Track {
    pub fn album_key(&self) -> AlbumKey {
        album_key(&self.album_artist, &self.album)
    }
}

pub fn album_key(album_artist: &str, album: &str) -> AlbumKey {
    format!("{}\u{1f}{}", album_artist.to_lowercase(), album.to_lowercase())
}

#[derive(Clone, Debug)]
pub struct Album {
    pub key: AlbumKey,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub genre: String,
    pub tracks: Vec<TrackId>,
    pub added_at: u64,
    pub duration_secs: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub tracks: Vec<TrackId>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct LibraryData {
    pub folders: Vec<PathBuf>,
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    #[serde(default)]
    pub volume: Option<f32>,
    /// Bumped when tag parsing changes, forcing a re-read of every file.
    #[serde(default)]
    pub scan_version: u32,
    /// Even out loudness between songs (default on).
    #[serde(default)]
    pub level_volume: Option<bool>,
    /// Seconds listened per local day (days since 1970-01-01).
    #[serde(default)]
    pub daily: BTreeMap<i64, f64>,
    /// "Listen on your iPhone" (off unless turned on).
    #[serde(default)]
    pub stream: Option<bool>,
}

/// Today's local date as days since 1970-01-01.
pub fn today() -> i64 {
    let now = chrono::Local::now();
    (now.timestamp() + now.offset().local_minus_utc() as i64).div_euclid(86400)
}

/// (month 1-12, day 1-31) for days since 1970-01-01 (civil calendar).
pub fn month_day(days: i64) -> (u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (m, d)
}

pub struct Library {
    pub data: LibraryData,
    pub by_id: HashMap<TrackId, usize>,
    pub albums: Vec<Album>,
    pub album_index: HashMap<AlbumKey, usize>,
    /// artist name -> album indices
    pub artists: BTreeMap<String, Vec<usize>>,
    dirty: bool,
}

pub fn app_dir() -> PathBuf {
    let dir = std::env::var_os("MH_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music Host")
    });
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn library_file() -> PathBuf {
    app_dir().join("library.json")
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn path_id(p: &Path) -> TrackId {
    // FNV-1a: stable across runs (std's hasher is randomly seeded).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Library {
    pub fn load() -> Self {
        let data = std::fs::read(library_file())
            .ok()
            .and_then(|b| serde_json::from_slice::<LibraryData>(&b).ok())
            .unwrap_or_default();
        let mut data = data;
        if data.scan_version < SCAN_VERSION {
            for t in &mut data.tracks {
                t.mtime = 0;
            }
            data.scan_version = SCAN_VERSION;
        }
        let mut lib = Library {
            data,
            by_id: HashMap::new(),
            albums: vec![],
            album_index: HashMap::new(),
            artists: BTreeMap::new(),
            dirty: false,
        };
        lib.rebuild();
        lib
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Ok(json) = serde_json::to_vec(&self.data) {
            let tmp = library_file().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, library_file());
            }
        }
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.by_id.get(&id).map(|&i| &self.data.tracks[i])
    }

    pub fn log_listening(&mut self, secs: f64) {
        if secs > 0.0 {
            *self.data.daily.entry(today()).or_default() += secs;
            self.dirty = true;
        }
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        let i = *self.by_id.get(&id)?;
        self.dirty = true;
        Some(&mut self.data.tracks[i])
    }

    pub fn album_of(&self, id: TrackId) -> Option<&Album> {
        let t = self.track(id)?;
        self.album_index.get(&t.album_key()).map(|&i| &self.albums[i])
    }

    /// Merge scan results (replaces tracks under scanned roots, keeps stats).
    pub fn apply_scan(&mut self, roots: &[PathBuf], found: Vec<Track>) {
        let old: HashMap<TrackId, Track> = self
            .data
            .tracks
            .drain(..)
            .map(|t| (t.id, t))
            .collect();
        let found_ids: HashSet<TrackId> = found.iter().map(|t| t.id).collect();
        let mut tracks = Vec::with_capacity(old.len() + found.len());
        for (id, t) in &old {
            let under_scanned = roots.iter().any(|r| t.path.starts_with(r));
            if !under_scanned && !found_ids.contains(id) {
                tracks.push(t.clone());
            }
        }
        for mut t in found {
            if let Some(prev) = old.get(&t.id) {
                t.added_at = prev.added_at;
                t.play_count = prev.play_count;
                t.last_played = prev.last_played;
                t.loved = prev.loved;
                t.listened_secs = prev.listened_secs;
                t.skips = prev.skips;
                if prev.mtime == t.mtime {
                    t.loudness = prev.loudness;
                    t.peak = prev.peak;
                }
            }
            tracks.push(t);
        }
        self.data.tracks = tracks;
        let live: HashSet<TrackId> = self.data.tracks.iter().map(|t| t.id).collect();
        for p in &mut self.data.playlists {
            p.tracks.retain(|id| live.contains(id));
        }
        self.dirty = true;
        self.rebuild();
    }

    pub fn remove_folder(&mut self, folder: &Path) {
        self.data.folders.retain(|f| f != folder);
        let folders = self.data.folders.clone();
        self.data.tracks.retain(|t| {
            !t.path.starts_with(folder) || folders.iter().any(|f| t.path.starts_with(f))
        });
        self.dirty = true;
        self.rebuild();
    }

    pub fn rebuild(&mut self) {
        self.assign_album_artists();
        self.data.tracks.sort_by(|a, b| {
            a.album_artist
                .to_lowercase()
                .cmp(&b.album_artist.to_lowercase())
                .then(a.album.to_lowercase().cmp(&b.album.to_lowercase()))
                .then(a.disc_no.unwrap_or(1).cmp(&b.disc_no.unwrap_or(1)))
                .then(a.track_no.unwrap_or(9999).cmp(&b.track_no.unwrap_or(9999)))
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.by_id = self
            .data
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();

        self.albums.clear();
        self.album_index.clear();
        for t in &self.data.tracks {
            let key = t.album_key();
            let idx = *self.album_index.entry(key.clone()).or_insert_with(|| {
                self.albums.push(Album {
                    key,
                    title: t.album.clone(),
                    artist: t.album_artist.clone(),
                    year: t.year,
                    genre: t.genre.clone(),
                    tracks: vec![],
                    added_at: t.added_at,
                    duration_secs: 0.0,
                });
                self.albums.len() - 1
            });
            let a = &mut self.albums[idx];
            a.tracks.push(t.id);
            a.duration_secs += t.duration_secs;
            a.added_at = a.added_at.max(t.added_at);
            if a.year.is_none() {
                a.year = t.year;
            }
            if a.genre.is_empty() {
                a.genre = t.genre.clone();
            }
        }
        // Sort albums by artist then title, and rebuild the index.
        self.albums.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.album_index = self
            .albums
            .iter()
            .enumerate()
            .map(|(i, a)| (a.key.clone(), i))
            .collect();
        self.artists.clear();
        for (i, a) in self.albums.iter().enumerate() {
            self.artists.entry(a.artist.clone()).or_default().push(i);
        }
        for list in self.artists.values_mut() {
            let albums = &self.albums;
            list.sort_by_key(|&i| (albums[i].year.unwrap_or(0), albums[i].title.to_lowercase()));
        }
    }

    /// Tracks without an album-artist tag get one inferred per album folder,
    /// so featured artists and split albums don't fragment the library:
    /// a tagged sibling wins; otherwise a folder named "Album - Artist" that
    /// names one of the artists; otherwise a clear majority; otherwise
    /// "Various Artists".
    fn assign_album_artists(&mut self) {
        let mut groups: HashMap<(PathBuf, String), Vec<usize>> = HashMap::new();
        for (i, t) in self.data.tracks.iter().enumerate() {
            groups.entry((album_dir(&t.path), t.album.to_lowercase())).or_default().push(i);
        }
        for ((dir, _), idxs) in groups {
            let tracks = &self.data.tracks;
            let mut tagged: HashMap<&str, usize> = HashMap::new();
            let mut derived: HashMap<String, usize> = HashMap::new();
            for &i in &idxs {
                let t = &tracks[i];
                if t.album_artist_tagged {
                    *tagged.entry(t.album_artist.as_str()).or_default() += 1;
                } else {
                    *derived.entry(primary_artist(&t.artist)).or_default() += 1;
                }
            }
            if derived.is_empty() {
                continue;
            }
            let pick = if let Some((name, _)) = tagged.iter().max_by_key(|(_, n)| **n) {
                name.to_string()
            } else if derived.len() == 1 {
                derived.keys().next().unwrap().clone()
            } else {
                let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
                let folder = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let suffix = folder.rsplit_once(" - ").map(|(_, a)| norm(a)).unwrap_or_default();
                let total: usize = derived.values().sum();
                let (top, n) = derived.iter().max_by_key(|(_, n)| **n).map(|(a, n)| (a.clone(), *n)).unwrap();
                if let Some(a) = derived.keys().find(|a| !suffix.is_empty() && norm(a) == suffix) {
                    a.clone()
                } else if n * 10 >= total * 7 {
                    top
                } else {
                    "Various Artists".to_string()
                }
            };
            for i in idxs {
                if !self.data.tracks[i].album_artist_tagged {
                    self.data.tracks[i].album_artist = pick.clone();
                }
            }
        }
    }

    pub fn new_playlist(&mut self, name: String) -> u64 {
        let id = self.data.playlists.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        self.data.playlists.push(Playlist {
            id,
            name,
            tracks: vec![],
        });
        self.dirty = true;
        id
    }

    pub fn playlist_mut(&mut self, id: u64) -> Option<&mut Playlist> {
        self.dirty = true;
        self.data.playlists.iter_mut().find(|p| p.id == id)
    }
}

// ---------------------------------------------------------------- scanning

pub enum ScanMsg {
    Progress { done: usize, total: usize },
    Done { roots: Vec<PathBuf>, tracks: Vec<Track> },
}

/// Read tags, detecting the format from file contents rather than the
/// extension (e.g. `.ogg` files that actually contain Opus).
pub fn read_tags(path: &Path) -> lofty::error::Result<lofty::file::TaggedFile> {
    lofty::probe::Probe::open(path)?.guess_file_type()?.read()
}

pub fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Scan `roots` on a background thread. `known` lets us skip re-reading
/// files whose mtime hasn't changed.
pub fn spawn_scan(roots: Vec<PathBuf>, known: HashMap<PathBuf, Track>, tx: Sender<ScanMsg>, wake: egui::Context) {
    std::thread::spawn(move || {
        let mut files = vec![];
        for root in &roots {
            if root.is_file() {
                if is_audio(root) {
                    files.push(root.clone());
                }
                continue;
            }
            for e in walkdir::WalkDir::new(root).follow_links(true).into_iter().flatten() {
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_file() && !name.starts_with("._") && is_audio(e.path()) {
                    files.push(e.into_path());
                }
            }
        }
        let total = files.len();
        let mut out = Vec::with_capacity(total);
        let now = now_secs();
        for (i, f) in files.into_iter().enumerate() {
            let mtime = std::fs::metadata(&f)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match known.get(&f) {
                Some(t) if t.mtime == mtime => out.push(t.clone()),
                _ => out.push(read_track(&f, mtime, now)),
            }
            if i % 25 == 0 {
                let _ = tx.send(ScanMsg::Progress { done: i, total });
                wake.request_repaint();
            }
        }
        let _ = tx.send(ScanMsg::Done { roots, tracks: out });
        wake.request_repaint();
    });
}

fn clean(s: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    s.map(|s| s.trim().trim_matches('\0').to_string())
        .filter(|s| !s.is_empty())
}

fn read_track(path: &Path, mtime: u64, now: u64) -> Track {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Album".into());

    let mut t = Track {
        id: path_id(path),
        path: path.to_path_buf(),
        title: stem.clone(),
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        genre: String::new(),
        year: None,
        track_no: None,
        disc_no: None,
        duration_secs: 0.0,
        added_at: now,
        mtime,
        play_count: 0,
        last_played: 0,
        loved: false,
        album_artist_tagged: false,
        listened_secs: 0.0,
        skips: 0,
        loudness: None,
        peak: None,
    };

    if let Ok(tf) = read_tags(path) {
        t.duration_secs = tf.properties().duration().as_secs_f64();
        if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
            if let Some(v) = clean(tag.title()) {
                t.title = v;
            }
            t.artist = clean(tag.artist()).unwrap_or_default();
            t.album = clean(tag.album()).unwrap_or_default();
            t.genre = clean(tag.genre()).unwrap_or_default();
            t.year = tag.year().filter(|y| *y > 0);
            t.track_no = tag.track();
            t.disc_no = tag.disk();
            t.album_artist = tag
                .get_string(&ItemKey::AlbumArtist)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            t.album_artist_tagged = !t.album_artist.is_empty();
        }
    }

    // Fallbacks so nothing shows up blank.
    if t.track_no.is_none() {
        // "01 - Song" / "01. Song" / "1 Song"
        let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && digits.len() <= 3 {
            t.track_no = digits.parse().ok();
            if t.title == stem {
                let rest = stem[digits.len()..].trim_start_matches(|c: char| c == ' ' || c == '-' || c == '.' || c == '_');
                if !rest.is_empty() {
                    t.title = rest.to_string();
                }
            }
        }
    }
    if t.album.is_empty() {
        t.album = parent;
    }
    if t.artist.is_empty() {
        t.artist = if t.album_artist.is_empty() {
            "Unknown Artist".into()
        } else {
            t.album_artist.clone()
        };
    }
    // "Drake; Rihanna" → shown as "Drake, Rihanna"; album filed under "Drake".
    t.artist = t.artist.replace("; ", ", ").replace(';', ", ");
    if t.album_artist.is_empty() {
        t.album_artist = primary_artist(&t.artist);
    }
    t
}

/// The folder that represents an album: "Album/Disc 2/x.flac" → "Album".
fn album_dir(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    let name = parent.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let is_disc = ["disc", "disk", "cd"].iter().any(|p| {
        name.strip_prefix(p).map(|rest| {
            let rest = rest.trim();
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
        }) == Some(true)
    });
    if is_disc {
        parent.parent().unwrap_or(parent).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

/// The main credited artist, without features: "Drake, Rihanna" → "Drake",
/// "Kanye West feat. Jay-Z" → "Kanye West". Commas inside names like
/// "Tyler, The Creator" are kept when nothing else follows a feature marker.
fn primary_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let mut cut = artist.len();
    for marker in [" feat. ", " feat ", " ft. ", " featuring ", " with ", " / "] {
        if let Some(i) = lower.find(marker) {
            cut = cut.min(i);
        }
    }
    let head = &artist[..cut];
    // Multi-artist lists from ";"-separated tags were normalised to ", ".
    let head = match head.split_once(", ") {
        Some((first, rest)) if !rest.to_lowercase().starts_with("the ") => first,
        _ => head,
    };
    head.trim().to_string()
}

/// Find artwork bytes for a track: embedded picture first, then an image
/// file in the same folder (cover.jpg, folder.png, ...).
pub fn find_artwork(path: &Path) -> Option<Vec<u8>> {
    if let Ok(tf) = read_tags(path) {
        for tag in tf.tags() {
            let pics = tag.pictures();
            let best = pics
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
                .or_else(|| pics.first());
            if let Some(p) = best {
                return Some(p.data().to_vec());
            }
        }
    }
    let dir = path.parent()?;
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
                Some("jpg" | "jpeg" | "png" | "webp")
            )
        })
        .collect();
    candidates.sort_by_key(|p| {
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        COVER_NAMES
            .iter()
            .position(|n| stem == *n || stem.starts_with(n))
            .unwrap_or(COVER_NAMES.len())
    });
    candidates.first().and_then(|p| std::fs::read(p).ok())
}

#[cfg(test)]
mod tests {
    use super::primary_artist;

    #[test]
    fn primary_artist_strips_features() {
        assert_eq!(primary_artist("Drake, Rihanna"), "Drake");
        assert_eq!(primary_artist("Drake"), "Drake");
        assert_eq!(primary_artist("Kanye West feat. Jay-Z"), "Kanye West");
        assert_eq!(primary_artist("Tyler, The Creator"), "Tyler, The Creator");
        assert_eq!(primary_artist("Simon & Garfunkel"), "Simon & Garfunkel");
    }
}
