//! "Add from Link": download audio from YouTube links into the library.
//!
//! Uses the `yt-dlp` command-line tool (Homebrew) for fetching, keeping the
//! original audio stream (no re-encode). Most YouTube uploads carry no album
//! data, so each song is identified from its title and looked up on Apple's
//! catalogue to get the real album, track number, year, genre and HD cover —
//! preferring an album already in the library so songs join it. Songs already
//! in the library are skipped. Full-album videos with chapters are split into
//! tracks.

use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Event {
    /// A job's status line changed.
    Status { job: u64, title: String, status: String, progress: Option<f32> },
    /// Finished: files to scan into the library.
    Added { job: u64, title: String, summary: String, files: Vec<PathBuf> },
    /// Nothing to do (already in library) or failed.
    Skipped { job: u64, title: String, summary: String },
    Failed { job: u64, title: String, error: String },
    /// Stopped by the user.
    Cancelled { job: u64, title: String },
    /// Songs finished mid-job (album downloads): add them to the library now.
    Imported { #[allow(dead_code)] job: u64, files: Vec<PathBuf> },
}

/// Set to true to stop a download (kills yt-dlp, removes partial files).
pub type Cancel = std::sync::Arc<std::sync::atomic::AtomicBool>;

fn stopped(c: &Cancel) -> bool {
    c.load(std::sync::atomic::Ordering::Relaxed)
}

/// What the downloader needs to know about the existing library.
#[derive(Clone, Default)]
pub struct LibrarySnapshot {
    /// normalised "artist|title" of every track
    pub songs: HashSet<String>,
    /// normalised "album|title" of every track (catches duplicates that a
    /// mismatched artist name would miss)
    pub album_songs: HashSet<String>,
    /// Albums in the library: (norm artist, base album name, display title,
    /// display artist, folder) — so downloads join the copy you have.
    pub album_dirs: Vec<(String, String, String, String, PathBuf)>,
    /// normalised "albumartist|album" → display album name
    pub albums: Vec<(String, String, String)>,
}

/// Don't flash a console window for helper processes on Windows.
#[cfg_attr(not(windows), allow(unused_mut))]
pub fn quiet(mut c: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Where the Windows build keeps its own yt-dlp.exe and deno.exe.
#[cfg(windows)]
fn tools_dir() -> PathBuf {
    let d = crate::library::app_dir().join("tools");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn ytdlp_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let candidates = vec![tools_dir().join("yt-dlp.exe")];
    #[cfg(not(windows))]
    let candidates = vec![PathBuf::from("/opt/homebrew/bin/yt-dlp"), PathBuf::from("/usr/local/bin/yt-dlp")];
    candidates.into_iter().find(|p| p.exists())
}

/// Windows: fetch yt-dlp (and deno, which YouTube now needs) on first use.
/// On macOS they come from Homebrew.
pub fn ensure_tools(status: &dyn Fn(&str)) -> Result<(), String> {
    #[cfg(windows)]
    {
        // One setup at a time even if several downloads start together.
        static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = SETUP.lock();
        let dir = tools_dir();
        let fetch = |url: &str, to: &Path| -> Result<(), String> {
            let resp = ureq::get(url).timeout(Duration::from_secs(300)).call().map_err(|e| format!("couldn't download tools ({e})"))?;
            let tmp = to.with_extension("part");
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
            drop(f);
            std::fs::rename(&tmp, to).map_err(|e| e.to_string())
        };
        let ytdlp = dir.join("yt-dlp.exe");
        if !ytdlp.exists() {
            status("Setting up downloader (one time)…");
            fetch("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe", &ytdlp)?;
        }
        if !dir.join("deno.exe").exists() {
            status("Setting up downloader (one time)…");
            let zip = dir.join("deno.zip");
            fetch("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip", &zip)?;
            // tar.exe ships with Windows 10+ and reads zip files.
            let ok = quiet(Command::new("tar")).arg("-xf").arg(&zip).arg("-C").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
            let _ = std::fs::remove_file(&zip);
            if !ok {
                return Err("couldn't unpack deno".into());
            }
        }
    }
    let _ = status;
    Ok(())
}

/// Whether link downloads can work here (Windows fetches its tools itself).
pub fn downloader_available() -> bool {
    cfg!(windows) || ytdlp_path().is_some()
}

fn ytdlp() -> Command {
    let _ = ensure_tools(&|_| {});
    let mut c = quiet(Command::new(ytdlp_path().unwrap_or_else(|| PathBuf::from("yt-dlp"))));
    let path = std::env::var("PATH").unwrap_or_default();
    // Apps launched from Finder get a minimal PATH; yt-dlp needs ffmpeg + deno.
    #[cfg(not(windows))]
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{path}"));
    #[cfg(windows)]
    c.env("PATH", format!("{};{path}", tools_dir().display()));
    c.stdin(Stdio::null());
    c
}

pub fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// "Feather (feat. Cise Starr) [Official Audio]" → "Feather (feat. Cise Starr)".
fn clean_title(s: &str) -> (String, Option<String>) {
    let junk = [
        "official audio", "official video", "official music video", "music video", "official lyric video", "lyric video",
        "lyrics", "lyric", "audio", "video", "hd", "hq", "4k", "visualizer", "visualiser", "mv", "m/v", "official",
        "remastered", "full album", "complete album", "album rip", "albumrip", "high quality", "explicit",
    ];
    let mut out = String::new();
    let mut album_hint = None;
    let mut rest = s;
    while !rest.is_empty() {
        let open = rest.find(['(', '[', '【']);
        let Some(i) = open else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        let ch = rest[i..].chars().next().unwrap();
        let close = match ch {
            '(' => ')',
            '[' => ']',
            _ => '】',
        };
        let after = &rest[i + ch.len_utf8()..];
        match after.find(close) {
            Some(j) => {
                let inner = after[..j].trim();
                let lower = inner.to_lowercase();
                if let Some(a) = lower.strip_prefix("album").map(|_| inner[5..].trim_start_matches([' ', ':', '-']).trim()) {
                    if !a.is_empty() {
                        album_hint = Some(a.to_string());
                    }
                } else if !junk.iter().any(|j| lower == *j || lower.starts_with(&format!("{j} ")) && lower.len() < 30) {
                    out.push(ch);
                    out.push_str(inner);
                    out.push(close);
                }
                rest = &after[j + close.len_utf8()..];
            }
            None => {
                out.push_str(&rest[i..]);
                break;
            }
        }
    }
    let t = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (t.trim_matches([' ', '-', '|']).to_string(), album_hint)
}

fn strip_channel_suffix(s: &str) -> String {
    let mut s = s.trim().to_string();
    for suf in [" - Topic", "VEVO", " Official", " Music"] {
        if s.ends_with(suf) && s.len() > suf.len() + 1 {
            s.truncate(s.len() - suf.len());
        }
    }
    s.trim().to_string()
}

#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub album_artist: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub cover_url: Option<String>,
    /// The album already exists in the library.
    pub joins_existing: bool,
}

fn get_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())
}

/// Work out artist / title (and album hints) from yt-dlp's info JSON.
fn identify(info: &Value) -> (String, String, Option<String>) {
    // YouTube Music "Topic" uploads carry real metadata.
    if let (Some(artist), Some(track)) = (get_str(info, "artist").or(get_str(info, "creator")), get_str(info, "track")) {
        let artist = artist.split(',').next().unwrap_or(artist).trim().to_string();
        return (artist, track.to_string(), get_str(info, "album").map(String::from));
    }
    let raw = get_str(info, "title").unwrap_or("Unknown");
    let (title, hint) = clean_title(raw);
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((a, t)) = title.split_once(sep) {
            let (t, h2) = clean_title(t);
            return (a.trim().to_string(), t, hint.or(h2));
        }
    }
    let uploader = get_str(info, "uploader").or(get_str(info, "channel")).unwrap_or("Unknown Artist");
    (strip_channel_suffix(uploader), title, hint)
}

pub fn without_feat(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut cut = s.len();
    for m in ["(feat", "[feat", " feat.", " ft.", "(with ", " featuring "] {
        if let Some(i) = lower.find(m) {
            cut = cut.min(i);
        }
    }
    s[..cut].trim().to_string()
}

/// Find the song on Apple's catalogue to learn its album.
fn itunes_song(artist: &str, title: &str, album_hint: Option<&str>, lib: &LibrarySnapshot) -> Option<Meta> {
    let term = format!("{} {}", without_feat(artist), without_feat(title));
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=song&limit=25",
        url_encode(&term)
    );
    let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let results = resp.get("results")?.as_array()?;
    let (na, nt) = (norm(&without_feat(artist)), norm(&without_feat(title)));
    let mut best: Option<(i32, &Value)> = None;
    for r in results {
        let ra = norm(get_str(r, "artistName").unwrap_or(""));
        let rt = norm(&without_feat(get_str(r, "trackName").unwrap_or("")));
        if ra.is_empty() || !(ra.contains(&na) || na.contains(&ra)) || rt != nt {
            continue;
        }
        let coll = get_str(r, "collectionName").unwrap_or("");
        let coll_artist = get_str(r, "collectionArtistName").unwrap_or(get_str(r, "artistName").unwrap_or(""));
        let mut score = 10;
        if lib.albums.iter().any(|(a, al, _)| *al == norm(coll) && (a.contains(&na) || na.contains(a.as_str()))) {
            score += 100; // the user already has this album
        }
        if let Some(h) = album_hint {
            if norm(coll).starts_with(&norm(h)) {
                score += 60;
            }
        }
        if coll.ends_with(" - Single") || coll.ends_with(" - EP") {
            score -= 5;
        }
        if norm(coll_artist) == "variousartists" {
            score -= 8; // prefer the artist's own albums/singles over compilations
        }
        if best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, r));
        }
    }
    let (score, r) = best?;
    let mut album = get_str(r, "collectionName").unwrap_or("").to_string();
    let mut album_artist = get_str(r, "collectionArtistName").or(get_str(r, "artistName")).unwrap_or(artist).to_string();
    let mut joins_existing = false;
    // Use the library's exact spelling so it groups with the existing album.
    if score >= 100 {
        if let Some((_, _, display)) = lib.albums.iter().find(|(_, al, _)| *al == norm(&album)) {
            album = display.clone();
            joins_existing = true;
        }
    }
    if album_artist.contains(',') || album_artist.contains('&') {
        album_artist = without_feat(&album_artist);
    }
    Some(Meta {
        artist: get_str(r, "artistName").unwrap_or(artist).to_string(),
        title: title.to_string(),
        album,
        album_artist,
        track: r.get("trackNumber").and_then(|v| v.as_u64()).map(|v| v as u32),
        disc: r.get("discNumber").and_then(|v| v.as_u64()).map(|v| v as u32),
        year: get_str(r, "releaseDate").and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
        genre: get_str(r, "primaryGenreName").map(String::from),
        cover_url: get_str(r, "artworkUrl100").map(|u| u.replace("100x100bb", "1500x1500bb")),
        joins_existing,
    })
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn safe(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    cleaned.trim().trim_matches('.').chars().take(120).collect()
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    let mut bytes = vec![];
    ureq::get(url)
        .timeout(Duration::from_secs(20))
        .call()
        .ok()?
        .into_reader()
        .take(25 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Best cover: Apple's HD artwork; else the YouTube thumbnail centre-cropped
/// to a square (video thumbnails are 16:9 frames).
fn write_cover(dir: &Path, meta: &Meta, info: &Value) {
    let dest = dir.join("cover.jpg");
    if dest.exists() {
        return;
    }
    if let Some(bytes) = meta.cover_url.as_deref().and_then(fetch) {
        if image::load_from_memory(&bytes).is_ok() {
            let _ = std::fs::write(&dest, bytes);
            return;
        }
    }
    let thumb = info
        .get("thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|ts| {
            ts.iter()
                .filter(|t| t.get("width").is_some())
                .max_by_key(|t| t.get("width").and_then(|w| w.as_u64()).unwrap_or(0))
                .and_then(|t| get_str(t, "url"))
        })
        .or(get_str(info, "thumbnail"));
    if let Some(img) = thumb.and_then(fetch).and_then(|b| image::load_from_memory(&b).ok()) {
        let n = img.width().min(img.height());
        let sq = img.crop_imm((img.width() - n) / 2, (img.height() - n) / 2, n, n);
        let _ = sq.to_rgb8().save_with_format(&dest, image::ImageFormat::Jpeg);
    }
}

fn write_tags(path: &Path, m: &Meta, title: &str, track: Option<u32>) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
    let mut tf = crate::library::read_tags(path).map_err(|e| e.to_string())?;
    if tf.primary_tag_mut().is_none() {
        let t = tf.primary_tag_type();
        tf.insert_tag(Tag::new(t));
    }
    let tag = tf.primary_tag_mut().ok_or("no tag")?;
    tag.set_title(title.to_string());
    tag.set_artist(m.artist.clone());
    tag.set_album(m.album.clone());
    tag.insert_text(ItemKey::AlbumArtist, m.album_artist.clone());
    if let Some(t) = track {
        tag.set_track(t);
    }
    if let Some(d) = m.disc {
        tag.set_disk(d);
    }
    if let Some(y) = m.year {
        tag.set_year(y);
    }
    if let Some(g) = &m.genre {
        tag.set_genre(g.clone());
    }
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

/// Download a video's original audio stream (no re-encode) to
/// `dir/stem.<ext>`, reporting progress. Optionally splits chapters into
/// `dir/NN title.<ext>`. Retries once if YouTube rejects the first attempt.
fn download_audio(
    video_url: &str,
    dir: &Path,
    stem: &str,
    split_chapters: Option<&Path>,
    progress: &dyn Fn(Option<f32>),
    cancel: &Cancel,
) -> Result<PathBuf, String> {
    let mut last_err = String::from("download failed");
    for attempt in 0..2 {
        if stopped(cancel) {
            return Err("stopped".into());
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(3));
        }
        let mut cmd = ytdlp();
        cmd.args([
            "--no-playlist",
            "--retries",
            "8",
            "--fragment-retries",
            "8",
            "--extractor-retries",
            "3",
            "--quiet",
            "--no-warnings",
            "--progress",
            "--newline",
            "--progress-template",
            "download:PROG %(progress._percent_str)s",
            "--no-simulate",
            "--print",
            "after_move:filepath",
            "-o",
        ]);
        cmd.arg(dir.join(format!("{stem}.%(ext)s")));
        if cfg!(windows) {
            // No ffmpeg on Windows: take YouTube's AAC stream as-is (.m4a).
            cmd.args(["-f", "bestaudio[ext=m4a]/bestaudio"]);
        } else {
            cmd.args(["-f", "bestaudio", "-x", "--audio-quality", "0"]);
        }
        if let Some(cd) = split_chapters.filter(|_| !cfg!(windows)) {
            cmd.arg("--split-chapters");
            cmd.arg("-o");
            cmd.arg(format!("chapter:{}/%(section_number)02d %(section_title)s.%(ext)s", cd.display()));
        }
        cmd.arg(video_url);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("couldn't run yt-dlp ({e})"))?;
        // Watchdog: kill yt-dlp as soon as the user stops (even mid-stall).
        let pid = child.id();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (cancel, done) = (cancel.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    if stopped(&cancel) {
                        #[cfg(windows)]
                        let _ = quiet(Command::new("taskkill")).args(["/PID", &pid.to_string(), "/T", "/F"]).status();
                        #[cfg(not(windows))]
                        let _ = Command::new("kill").arg(pid.to_string()).status();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            });
        }
        let mut final_path = None;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(p) = line.trim().strip_prefix("PROG") {
                    progress(p.trim().trim_end_matches('%').trim().parse::<f32>().ok().map(|p| p / 100.0));
                } else if !line.trim().is_empty() {
                    final_path = Some(PathBuf::from(line.trim()));
                }
            }
        }
        let mut err_text = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err_text);
        }
        let _ = child.wait();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        if stopped(cancel) {
            remove_partials(dir, stem);
            return Err("stopped".into());
        }
        if let Some(p) = final_path.filter(|p| p.exists()) {
            return Ok(p);
        }
        last_err = err_text.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("download failed").replace("ERROR: ", "");
    }
    Err(last_err)
}

/// Delete half-downloaded files for `stem` (and the folder if now empty).
fn remove_partials(dir: &Path, stem: &str) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let empty = std::fs::read_dir(dir).map(|mut rd| rd.next().is_none()).unwrap_or(false);
    if empty {
        let _ = std::fs::remove_dir(dir);
    }
}

// ---------------------------------------------------------------- album tracklists

/// One song on an album, per Apple's catalogue.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CatalogTrack {
    pub title: String,
    pub artist: String,
    pub track: u32,
    pub disc: u32,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CatalogAlbum {
    pub tracks: Vec<CatalogTrack>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub cover_url: Option<String>,
}

/// An album in an artist's catalogue (for the discography view).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CatalogAlbumRef {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub track_count: u32,
    pub cover_url: Option<String>,
}

fn tracklist_cache(artist: &str, album: &str) -> PathBuf {
    let dir = crate::library::app_dir().join("tracklists");
    let _ = std::fs::create_dir_all(&dir);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}|{}", norm(artist), norm(album)).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{h:016x}.json"))
}

/// The album's full tracklist from Apple's catalogue (cached on disk;
/// `None` if the album isn't in the catalogue).
pub fn album_tracklist(artist: &str, album: &str) -> Option<CatalogAlbum> {
    let cache = tracklist_cache(artist, album);
    if let Ok(bytes) = std::fs::read(&cache) {
        if let Ok(v) = serde_json::from_slice::<Option<CatalogAlbum>>(&bytes) {
            return v;
        }
    }
    let found = fetch_tracklist(artist, album);
    // Only cache definite answers (a network error shouldn't stick).
    if let Some(ref f) = found {
        let _ = std::fs::write(&cache, serde_json::to_vec(&Some(f)).unwrap_or_default());
    } else if fetch_reachable() {
        let _ = std::fs::write(&cache, b"null");
    }
    found
}

fn fetch_reachable() -> bool {
    ureq::get("https://itunes.apple.com/search?term=test&limit=1").timeout(Duration::from_secs(5)).call().is_ok()
}

fn fetch_tracklist(artist: &str, album: &str) -> Option<CatalogAlbum> {
    let short = album.split(['(', '[']).next().unwrap_or(album).trim();
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=album&limit=25",
        url_encode(&format!("{artist} {short}"))
    );
    let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let (na, nt) = (norm(artist), norm(album));
    let base = |s: &str| norm(s.split(['(', '[']).next().unwrap_or(s));
    let results = resp.get("results")?.as_array()?;
    let album_pick = results
        .iter()
        .filter(|r| {
            let ra = norm(get_str(r, "artistName").unwrap_or(""));
            !ra.is_empty() && (ra.contains(&na) || na.contains(&ra) || na == "variousartists")
        })
        .find(|r| norm(get_str(r, "collectionName").unwrap_or("")) == nt)
        .or_else(|| {
            results.iter().find(|r| {
                let ra = norm(get_str(r, "artistName").unwrap_or(""));
                (ra.contains(&na) || na.contains(&ra)) && base(get_str(r, "collectionName").unwrap_or("")) == base(album)
            })
        })
        .cloned();
    // Apple's album search sometimes misses albums its song search knows
    // (e.g. Nujabes' Modal Soul) — find the album through one of its songs.
    let pick = match album_pick {
        Some(p) => p,
        None => {
            let url = format!(
                "https://itunes.apple.com/search?term={}&entity=song&limit=50",
                url_encode(&format!("{artist} {short}"))
            );
            let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
            resp.get("results")?
                .as_array()?
                .iter()
                .find(|r| {
                    let ra = norm(get_str(r, "artistName").unwrap_or(""));
                    (ra.contains(&na) || na.contains(&ra)) && norm(get_str(r, "collectionName").unwrap_or("")) == nt
                })?
                .clone()
        }
    };
    let id = pick.get("collectionId")?.as_u64()?;
    album_by_id(id)
}

/// Full album (tracks, year, genre, cover) by Apple catalogue id.
pub fn album_by_id(id: u64) -> Option<CatalogAlbum> {
    let url = format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit=200");
    let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let results = resp.get("results")?.as_array()?;
    let coll = results.iter().find(|r| get_str(r, "wrapperType") == Some("collection"))?;
    let mut tracks: Vec<CatalogTrack> = results
        .iter()
        .filter(|r| get_str(r, "wrapperType") == Some("track"))
        .filter_map(|r| {
            Some(CatalogTrack {
                title: get_str(r, "trackName")?.to_string(),
                artist: get_str(r, "artistName").unwrap_or("").to_string(),
                track: r.get("trackNumber")?.as_u64()? as u32,
                disc: r.get("discNumber").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                duration_ms: r.get("trackTimeMillis").and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .collect();
    tracks.sort_by_key(|t| (t.disc, t.track));
    if tracks.is_empty() {
        return None;
    }
    Some(CatalogAlbum {
        tracks,
        year: get_str(coll, "releaseDate").and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
        genre: get_str(coll, "primaryGenreName").map(String::from),
        id: Some(id),
        title: get_str(coll, "collectionName").unwrap_or("").to_string(),
        artist: get_str(coll, "artistName").unwrap_or("").to_string(),
        cover_url: get_str(coll, "artworkUrl100").map(|u| u.replace("100x100bb", "1500x1500bb")),
    })
}

/// "Modal Soul (Deluxe Edition) [Remastered]" → "modalsoul" for edition-proof matching.
pub fn base_album(s: &str) -> String {
    norm(s.split(['(', '[']).next().unwrap_or(s).trim_end_matches(" - EP").trim_end_matches(" - Single"))
}

/// An artist's albums and EPs on Apple's catalogue (cached for a week),
/// one entry per release (deluxe/clean editions collapsed).
pub fn artist_albums(artist: &str) -> Option<Vec<CatalogAlbumRef>> {
    let cache = tracklist_cache(artist, "\u{0}discography");
    if let Ok(meta) = std::fs::metadata(&cache) {
        let fresh = meta.modified().ok().and_then(|m| m.elapsed().ok()).map(|e| e < Duration::from_secs(7 * 86400)).unwrap_or(false);
        if fresh {
            if let Ok(v) = serde_json::from_slice::<Option<Vec<CatalogAlbumRef>>>(&std::fs::read(&cache).ok()?) {
                return v;
            }
        }
    }
    let found = fetch_artist_albums(artist);
    if found.is_some() || fetch_reachable() {
        let _ = std::fs::write(&cache, serde_json::to_vec(&found).unwrap_or_default());
    }
    found
}

fn fetch_artist_albums(artist: &str) -> Option<Vec<CatalogAlbumRef>> {
    let url = format!("https://itunes.apple.com/search?term={}&entity=musicArtist&limit=5", url_encode(artist));
    let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let na = norm(artist);
    let id = resp
        .get("results")?
        .as_array()?
        .iter()
        .find(|r| norm(get_str(r, "artistName").unwrap_or("")) == na)?
        .get("artistId")?
        .as_u64()?;
    let url = format!("https://itunes.apple.com/lookup?id={id}&entity=album&limit=200");
    let resp: Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let mut by_base: std::collections::HashMap<String, CatalogAlbumRef> = Default::default();
    for r in resp.get("results")?.as_array()? {
        if get_str(r, "wrapperType") != Some("collection") {
            continue;
        }
        let title = get_str(r, "collectionName").unwrap_or("").to_string();
        let count = r.get("trackCount").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        // Skip singles (≤3 songs) and compilations by others.
        if count <= 3 || norm(get_str(r, "artistName").unwrap_or("")) != na && !norm(get_str(r, "artistName").unwrap_or("")).contains(&na) {
            continue;
        }
        let clean = get_str(r, "collectionExplicitness") == Some("cleaned");
        let item = CatalogAlbumRef {
            id: r.get("collectionId").and_then(|v| v.as_u64()).unwrap_or(0),
            title: title.clone(),
            artist: get_str(r, "artistName").unwrap_or(artist).to_string(),
            year: get_str(r, "releaseDate").and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
            track_count: count,
            cover_url: get_str(r, "artworkUrl100").map(|u| u.replace("100x100bb", "600x600bb")),
        };
        let key = base_album(&title);
        // Prefer the explicit / most complete edition of each release.
        let better = match by_base.get(&key) {
            None => true,
            Some(old) => !clean && (count > old.track_count || old.title.len() > title.len() && count == old.track_count),
        };
        if better {
            by_base.insert(key, item);
        }
    }
    let mut v: Vec<CatalogAlbumRef> = by_base.into_values().collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.year.unwrap_or(0)));
    Some(v)
}

/// Does a library song correspond to this catalogue track?
pub fn same_song(library_title: &str, catalog_title: &str) -> bool {
    let (a, b) = (norm(&without_feat(library_title)), norm(&without_feat(catalog_title)));
    !a.is_empty() && (a == b || (a.len() > 5 && b.len() > 5 && (a.contains(&b) || b.contains(&a))))
}

/// Everything needed to fetch one missing album track.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackRequest {
    pub track: CatalogTrack,
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub dir: PathBuf,
}

/// Find the best YouTube upload for a catalogue track.
fn find_on_youtube(t: &CatalogTrack) -> Result<String, String> {
    let query = format!("ytsearch10:{} {}", without_feat(&t.artist), without_feat(&t.title));
    let out = ytdlp()
        .args(["-J", "--flat-playlist", "--no-warnings", &query])
        .output()
        .map_err(|e| format!("couldn't run yt-dlp ({e})"))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "YouTube search failed".to_string())?;
    let entries = v.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    let want_title = norm(&without_feat(&t.title));
    let want_artist = norm(&without_feat(&t.artist));
    let target = t.duration_ms as f64 / 1000.0;
    let bad = ["live", "cover", "remix", "slowed", "reverb", "nightcore", "8d", "sped up", "spedup", "karaoke", "reaction", "instrumental", "piano version", "tutorial", "1 hour", "loop"];
    let lower_title = t.title.to_lowercase();
    let mut best: Option<(i32, String)> = None;
    for e in &entries {
        let yt_title = get_str(e, "title").unwrap_or("");
        let lt = yt_title.to_lowercase();
        let channel = get_str(e, "channel").or(get_str(e, "uploader")).unwrap_or("");
        let mut score = 0;
        if norm(yt_title).contains(&want_title) {
            score += 25;
        } else {
            score -= 30;
        }
        if norm(yt_title).contains(&want_artist) || norm(channel).contains(&want_artist) {
            score += 10;
        }
        if channel.ends_with(" - Topic") {
            score += 30; // auto-generated "official audio" from the label
        }
        if lt.contains("official audio") {
            score += 8;
        }
        for b in bad {
            if lt.contains(b) && !lower_title.contains(b) {
                score -= 40;
            }
        }
        if let Some(d) = e.get("duration").and_then(|d| d.as_f64()) {
            if target > 0.0 {
                let diff = (d - target).abs();
                score += if diff <= 3.0 {
                    45
                } else if diff <= 8.0 {
                    30
                } else if diff <= 20.0 {
                    5
                } else {
                    -60
                };
            }
        }
        let url = get_str(e, "url")
            .map(String::from)
            .or_else(|| get_str(e, "id").map(|id| format!("https://www.youtube.com/watch?v={id}")));
        if let Some(url) = url {
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, url));
            }
        }
    }
    match best {
        Some((score, url)) if score >= 40 => Ok(url),
        _ => Err("no good match on YouTube".into()),
    }
}

/// Find a catalogue track on YouTube, download and tag it. Blocking.
fn fetch_track(req: &TrackRequest, cancel: &Cancel, status: &dyn Fn(&str, Option<f32>)) -> Result<PathBuf, String> {
    status("Searching YouTube…", None);
    if stopped(cancel) {
        return Err("stopped".into());
    }
    let url = find_on_youtube(&req.track)?;
    let stem = safe(&format!("{:02} {}", req.track.track, req.track.title));
    let _ = std::fs::create_dir_all(&req.dir);
    let path = download_audio(&url, &req.dir, &stem, None, &|p| status("Downloading", p), cancel)?;
    let meta = Meta {
        artist: req.track.artist.clone(),
        title: req.track.title.clone(),
        album: req.album.clone(),
        album_artist: req.album_artist.clone(),
        track: Some(req.track.track),
        disc: Some(req.track.disc),
        year: req.year,
        genre: req.genre.clone(),
        ..Default::default()
    };
    let _ = write_tags(&path, &meta, &meta.title, meta.track);
    Ok(path)
}

/// Download one missing album track into its album. Blocking.
pub fn download_track(job: u64, req: TrackRequest, tx: Sender<Event>, wake: egui::Context, cancel: Cancel) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake.request_repaint();
    };
    let title = req.track.title.clone();
    let status = |s: &str, p: Option<f32>| send(Event::Status { job, title: title.clone(), status: s.into(), progress: p });
    match fetch_track(&req, &cancel, &status) {
        Ok(path) => send(Event::Added { job, title: title.clone(), summary: format!("“{title}” added to {}", req.album), files: vec![path] }),
        Err(_) if stopped(&cancel) => send(Event::Cancelled { job, title }),
        Err(e) => send(Event::Failed { job, title, error: e }),
    }
}

/// Download a whole catalogue album (or the listed subset) as one job.
/// Songs appear in the library as each one finishes.
pub fn download_album(job: u64, album: CatalogAlbum, dir: PathBuf, skip: HashSet<String>, tx: Sender<Event>, wake: egui::Context, cancel: Cancel) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake.request_repaint();
    };
    let label = format!("{} — {}", album.title, album.artist);
    let todo: Vec<&CatalogTrack> = album.tracks.iter().filter(|t| !skip.contains(&norm(&without_feat(&t.title)))).collect();
    if todo.is_empty() {
        return send(Event::Skipped { job, title: label, summary: format!("You already have all of {}", album.title) });
    }
    let _ = std::fs::create_dir_all(&dir);
    // Real cover first, so the album looks right as songs arrive.
    if let Some(bytes) = album.cover_url.as_deref().and_then(fetch) {
        let dest = dir.join("cover.jpg");
        if !dest.exists() && image::load_from_memory(&bytes).is_ok() {
            let _ = std::fs::write(dest, bytes);
        }
    }
    let total = todo.len();
    let (mut got, mut failed) = (vec![], vec![]);
    for (i, t) in todo.into_iter().enumerate() {
        if stopped(&cancel) {
            break;
        }
        let req = TrackRequest {
            track: t.clone(),
            album: album.title.clone(),
            album_artist: album.artist.clone(),
            year: album.year,
            genre: album.genre.clone(),
            dir: dir.clone(),
        };
        let status = |s: &str, p: Option<f32>| {
            send(Event::Status { job, title: label.clone(), status: format!("{}/{} · {} · {s}", i + 1, total, t.title), progress: p })
        };
        match fetch_track(&req, &cancel, &status) {
            Ok(path) => {
                send(Event::Imported { job, files: vec![path.clone()] });
                got.push(path);
            }
            Err(_) if stopped(&cancel) => break,
            Err(e) => failed.push(format!("{} ({e})", t.title)),
        }
    }
    if stopped(&cancel) && got.is_empty() {
        return send(Event::Cancelled { job, title: label });
    }
    let mut summary = format!("{} of {} songs added to {}", got.len(), total, album.title);
    if stopped(&cancel) {
        summary = format!("Stopped — {summary}");
    }
    if !failed.is_empty() {
        summary.push_str(&format!(" · couldn't find: {}", failed.join(", ")));
    }
    if got.is_empty() {
        send(Event::Failed { job, title: label, error: summary })
    } else {
        send(Event::Added { job, title: label, summary, files: got })
    }
}

/// If the library already has this album, use its exact name/artist and
/// folder, and skip the songs it already contains.
pub fn fit_to_library(album: &mut CatalogAlbum, root: &Path, lib: &LibrarySnapshot) -> (PathBuf, HashSet<String>) {
    let (na, nb) = (norm(&album.artist), base_album(&album.title));
    if let Some((_, _, title, artist, dir)) = lib
        .album_dirs
        .iter()
        .find(|(a, b, _, _, _)| *b == nb && (a.contains(&na) || na.contains(a.as_str()) || na == "variousartists"))
    {
        album.title = title.clone();
        album.artist = artist.clone();
        let have: HashSet<String> = album
            .tracks
            .iter()
            .filter(|t| lib.album_songs.contains(&format!("{}|{}", norm(title), norm(&without_feat(&t.title)))))
            .map(|t| norm(&without_feat(&t.title)))
            .collect();
        return (dir.clone(), have);
    }
    (root.join("YouTube").join(safe(&format!("{} - {}", album.artist, album.title))), HashSet::new())
}

/// Apple Music / Spotify link → download that album (or song) from YouTube.
pub fn run_catalog(job: u64, url: String, root: PathBuf, lib: LibrarySnapshot, tx: Sender<Event>, wake: egui::Context, cancel: Cancel) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake.request_repaint();
    };
    send(Event::Status { job, title: url.clone(), status: "Reading link…".into(), progress: None });
    let (mut album, only) = match resolve_catalog_link(&url) {
        Ok(CatalogLink::Album(a)) => (a, None),
        Ok(CatalogLink::Song(a, t)) => (a, Some(t)),
        Err(e) => return send(Event::Failed { job, title: url, error: e }),
    };
    if let Some(t) = &only {
        album.tracks.retain(|c| same_song(t, &c.title));
        if album.tracks.is_empty() {
            return send(Event::Failed { job, title: t.clone(), error: "song not found on its album".into() });
        }
    }
    let (dir, skip) = fit_to_library(&mut album, &root, &lib);
    download_album(job, album, dir, skip, tx, wake, cancel);
}

/// Album from an artist's discography (by catalogue id) → download it.
pub fn run_catalog_album(job: u64, id: u64, root: PathBuf, lib: LibrarySnapshot, tx: Sender<Event>, wake: egui::Context, cancel: Cancel) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake.request_repaint();
    };
    send(Event::Status { job, title: "Album".into(), status: "Getting tracklist…".into(), progress: None });
    let Some(mut album) = album_by_id(id) else {
        return send(Event::Failed { job, title: "Album".into(), error: "couldn't load the tracklist".into() });
    };
    let (dir, skip) = fit_to_library(&mut album, &root, &lib);
    download_album(job, album, dir, skip, tx, wake, cancel);
}

// ---------------------------------------------------------------- Apple Music / Spotify links

/// What an Apple Music / Spotify link points at.
pub enum CatalogLink {
    Album(CatalogAlbum),
    /// One song: the album it's on, plus that song's title.
    Song(CatalogAlbum, String),
}

pub fn is_catalog_link(url: &str) -> bool {
    url.contains("music.apple.com/") || url.contains("open.spotify.com/")
}

/// Resolve an Apple Music or Spotify album/song link to Apple's catalogue.
pub fn resolve_catalog_link(url: &str) -> Result<CatalogLink, String> {
    if url.contains("music.apple.com/") {
        // …/album/<name>/<albumId>?i=<songId>
        let path = url.split('?').next().unwrap_or(url);
        let album_id = path
            .trim_end_matches('/')
            .rsplit('/')
            .find_map(|seg| seg.parse::<u64>().ok())
            .ok_or("couldn't read that Apple Music link")?;
        let album = album_by_id(album_id).ok_or("album not found on Apple Music")?;
        let song = url.split(['?', '&']).find_map(|kv| kv.strip_prefix("i=")).and_then(|id| id.parse::<u64>().ok());
        if let Some(song_id) = song {
            let r: Value = ureq::get(&format!("https://itunes.apple.com/lookup?id={song_id}"))
                .timeout(Duration::from_secs(10))
                .call()
                .ok()
                .and_then(|r| r.into_json().ok())
                .ok_or("song not found")?;
            let title = r.get("results").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|t| get_str(t, "trackName")).ok_or("song not found")?;
            return Ok(CatalogLink::Song(album, title.to_string()));
        }
        return Ok(CatalogLink::Album(album));
    }
    // Spotify: read the public embed page (no account needed).
    if url.contains("/playlist/") {
        return Err("Spotify playlists aren't supported — use an album or song link".into());
    }
    let (kind, id) = spotify_kind_id(url).ok_or("couldn't read that Spotify link")?;
    let e = spotify_embed(kind, &id)?;
    let name = get_str(&e, "name").or(get_str(&e, "title")).unwrap_or("").to_string();
    let artists: Vec<String> = e
        .get("artists")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| get_str(x, "name").map(String::from)).collect())
        .unwrap_or_default();
    let artist = artists.first().cloned().or_else(|| get_str(&e, "subtitle").map(|s| s.split(',').next().unwrap_or(s).replace('\u{a0}', " ").trim().to_string())).unwrap_or_default();
    if kind == "track" {
        // Which album is it on? Ask Apple's catalogue.
        let m = itunes_song(&artist, &name, None, &LibrarySnapshot::default()).ok_or_else(|| format!("couldn't find “{name}” by {artist}"))?;
        let album = fetch_tracklist(&m.album_artist, &m.album).ok_or_else(|| format!("couldn't find the album for “{name}”"))?;
        return Ok(CatalogLink::Song(album, name));
    }
    // Album: prefer Apple's copy (disc numbers, year, genre, HD cover);
    // otherwise use Spotify's own tracklist.
    if let Some(a) = fetch_tracklist(&artist, &name) {
        return Ok(CatalogLink::Album(a));
    }
    let tracks: Vec<CatalogTrack> = e
        .get("trackList")
        .and_then(|t| t.as_array())
        .map(|t| {
            t.iter()
                .enumerate()
                .filter_map(|(i, x)| {
                    Some(CatalogTrack {
                        title: get_str(x, "title")?.to_string(),
                        artist: get_str(x, "subtitle").unwrap_or(&artist).split(',').next().unwrap_or(&artist).replace('\u{a0}', " ").trim().to_string(),
                        track: i as u32 + 1,
                        disc: 1,
                        duration_ms: x.get("duration").and_then(|d| d.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if tracks.is_empty() {
        return Err("that Spotify album has no tracks".into());
    }
    let cover_url = e
        .get("visualIdentity")
        .and_then(|v| v.get("image"))
        .and_then(|i| i.as_array())
        .and_then(|imgs| imgs.iter().max_by_key(|i| i.get("maxWidth").and_then(|w| w.as_u64()).unwrap_or(0)))
        .and_then(|i| get_str(i, "url"))
        .map(String::from);
    let year = get_str(&e, "releaseDate").and_then(|d| d.get(..4)).and_then(|y| y.parse().ok());
    Ok(CatalogLink::Album(CatalogAlbum { tracks, year, genre: None, id: None, title: name, artist, cover_url }))
}

/// "https://open.spotify.com/intl-de/album/<id>?si=…" → ("album", id)
fn spotify_kind_id(url: &str) -> Option<(&'static str, String)> {
    let path = url.split('?').next()?;
    let segs: Vec<&str> = path.split('/').collect();
    for (i, seg) in segs.iter().enumerate() {
        for kind in ["album", "track"] {
            if *seg == kind {
                return segs.get(i + 1).map(|id| (kind, id.to_string()));
            }
        }
    }
    None
}

fn spotify_embed(kind: &str, id: &str) -> Result<Value, String> {
    let html = ureq::get(&format!("https://open.spotify.com/embed/{kind}/{id}"))
        .set("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("couldn't open the Spotify link ({e})"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let start = html.find(r#"<script id="__NEXT_DATA__" type="application/json">"#).ok_or("couldn't read the Spotify page")?;
    let json_start = html[start..].find('>').map(|i| start + i + 1).ok_or("couldn't read the Spotify page")?;
    let json_end = html[json_start..].find("</script>").map(|i| json_start + i).ok_or("couldn't read the Spotify page")?;
    let v: Value = serde_json::from_str(&html[json_start..json_end]).map_err(|e| e.to_string())?;
    v.pointer("/props/pageProps/state/data/entity").cloned().ok_or_else(|| "couldn't read the Spotify page".into())
}


/// Expand a link into individual video infos (playlists → entries).
/// Is this a link to a real playlist/album (vs. a single video that
/// happens to carry a `list=` — e.g. YouTube's auto-generated Mixes)?
fn is_playlist_link(url: &str) -> bool {
    let list = url.split(['?', '&']).find_map(|kv| kv.strip_prefix("list=")).unwrap_or("");
    let has_video = url.contains("watch?v=") || url.contains("&v=") || url.contains("youtu.be/") || url.contains("/shorts/");
    // RD… = Mix/radio (endless, auto-generated), never a real playlist.
    !list.is_empty() && !list.starts_with("RD") && !has_video
}

fn resolve(url: &str) -> Result<Vec<Value>, String> {
    let mut args = vec!["-J", "--flat-playlist", "--no-warnings", "--playlist-end", "200"];
    if !is_playlist_link(url) {
        args.push("--no-playlist");
    }
    args.push(url);
    let out = ytdlp()
        .args(&args)
        .output()
        .map_err(|e| format!("couldn't run yt-dlp ({e})"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("yt-dlp failed").replace("ERROR: ", ""));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if v.get("_type").and_then(|t| t.as_str()) == Some("playlist") {
        Ok(v.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default())
    } else {
        Ok(vec![v])
    }
}

fn full_info(entry: &Value) -> Result<Value, String> {
    // Flat playlist entries are partial; fetch the full info.
    if entry.get("formats").is_some() {
        return Ok(entry.clone());
    }
    let url = get_str(entry, "url")
        .or(get_str(entry, "webpage_url"))
        .map(String::from)
        .or_else(|| get_str(entry, "id").map(|id| format!("https://www.youtube.com/watch?v={id}")))
        .ok_or("no video url")?;
    let out = ytdlp()
        .args(["-j", "--no-playlist", "--no-warnings", &url])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("unavailable").replace("ERROR: ", ""));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

/// Run one link (which may be a playlist). Blocking; call from a thread.
pub fn run(job: u64, url: String, root: PathBuf, lib: LibrarySnapshot, tx: Sender<Event>, wake: egui::Context, cancel: Cancel) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake.request_repaint();
    };
    send(Event::Status { job, title: url.clone(), status: "Looking up…".into(), progress: None });
    let entries = match resolve(&url) {
        Ok(e) if !e.is_empty() => e,
        Ok(_) => return send(Event::Failed { job, title: url, error: "nothing found at that link".into() }),
        Err(e) => return send(Event::Failed { job, title: url, error: e }),
    };
    let total = entries.len();
    let mut added_files = vec![];
    let mut summaries = vec![];
    for (i, entry) in entries.iter().enumerate() {
        if stopped(&cancel) {
            break;
        }
        let label = get_str(entry, "title").unwrap_or(&url).to_string();
        let prefix = if total > 1 { format!("{}/{} · ", i + 1, total) } else { String::new() };
        send(Event::Status { job, title: label.clone(), status: format!("{prefix}Identifying…"), progress: None });
        let info = match full_info(entry) {
            Ok(v) => v,
            Err(e) => {
                summaries.push(format!("skipped “{label}”: {e}"));
                continue;
            }
        };
        let (artist, title, hint) = identify(&info);
        let duration = info.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0);
        let chapters = info.get("chapters").and_then(|c| c.as_array()).map(|c| c.len()).unwrap_or(0);
        let is_album_video = duration > 1200.0 && chapters >= 3;

        if !is_album_video && lib.songs.contains(&format!("{}|{}", norm(&without_feat(&artist)), norm(&without_feat(&title)))) {
            summaries.push(format!("“{title}” is already in your library"));
            continue;
        }

        let meta = if is_album_video {
            let album = hint.clone().unwrap_or_else(|| title.clone());
            Meta { artist: artist.clone(), title: title.clone(), album: album.clone(), album_artist: artist.clone(), ..Default::default() }
        } else {
            itunes_song(&artist, &title, hint.as_deref(), &lib).unwrap_or_else(|| Meta {
                artist: artist.clone(),
                title: title.clone(),
                album: get_str(&info, "album").map(String::from).or(hint.clone()).unwrap_or_else(|| title.clone()),
                album_artist: artist.clone(),
                year: get_str(&info, "upload_date").and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
                ..Default::default()
            })
        };

        // Same song already on that album (artist names can differ)?
        if !is_album_video && meta.joins_existing && lib.album_songs.contains(&format!("{}|{}", norm(&meta.album), norm(&without_feat(&meta.title)))) {
            summaries.push(format!("“{}” is already on {}", meta.title, meta.album));
            continue;
        }

        let dir = root.join("YouTube").join(safe(&format!("{} - {}", meta.album_artist, meta.album)));
        let _ = std::fs::create_dir_all(&dir);
        let stem = match meta.track {
            Some(n) => safe(&format!("{n:02} {}", meta.title)),
            None => safe(&meta.title),
        };
        let video_url = get_str(&info, "webpage_url").unwrap_or(&url).to_string();

        let chapter_dir = if is_album_video { Some(dir.as_path()) } else { None };
        let progress = |pct: Option<f32>| {
            send(Event::Status { job, title: meta.title.clone(), status: format!("{prefix}Downloading"), progress: pct })
        };
        let path = match download_audio(&video_url, &dir, &stem, chapter_dir, &progress, &cancel) {
            Ok(p) => p,
            Err(_) if stopped(&cancel) => break,
            Err(msg) => {
                summaries.push(format!("“{label}” failed: {msg}"));
                continue;
            }
        };

        send(Event::Status { job, title: meta.title.clone(), status: format!("{prefix}Tagging…"), progress: None });
        if is_album_video {
            // Tag each chapter as a track; drop the full-length file.
            let mut tracks: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p != &path && crate::library::is_audio(p)).collect())
                .unwrap_or_default();
            tracks.sort();
            if !tracks.is_empty() {
                let _ = std::fs::remove_file(&path);
            }
            for (n, t) in tracks.iter().enumerate() {
                let name = t.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let chapter_title = name.get(3..).unwrap_or(&name).to_string();
                let _ = write_tags(t, &meta, &clean_title(&chapter_title).0, Some(n as u32 + 1));
                added_files.push(t.clone());
            }
            summaries.push(format!("“{}” split into {} tracks", meta.album, tracks.len()));
        } else {
            let _ = write_tags(&path, &meta, &meta.title, meta.track);
            added_files.push(path);
            summaries.push(if meta.joins_existing {
                format!("“{}” added to your album {}", meta.title, meta.album)
            } else {
                format!("“{}” — {} · {}", meta.title, meta.artist, meta.album)
            });
        }
        if !meta.joins_existing {
            write_cover(&dir, &meta, &info);
        }
    }

    if stopped(&cancel) {
        if added_files.is_empty() {
            return send(Event::Cancelled { job, title: url });
        }
        summaries.insert(0, format!("Stopped — kept {} finished song{}", added_files.len(), if added_files.len() == 1 { "" } else { "s" }));
    }
    let title = if total > 1 { format!("{total} videos") } else { summaries.first().cloned().unwrap_or(url.clone()) };
    let summary = summaries.join("\n");
    if added_files.is_empty() {
        send(Event::Skipped { job, title, summary });
    } else {
        send(Event::Added { job, title, summary, files: added_files });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MH_YT_LIVE=1 cargo test live_mix_and_cancel -- --nocapture
    #[test]
    fn live_mix_and_cancel() {
        if std::env::var("MH_YT_LIVE").is_err() {
            return;
        }
        // A video link that carries an auto-generated Mix must be ONE song.
        let mix = "https://www.youtube.com/watch?v=aqz-KE-bpKQ&list=RDaqz-KE-bpKQ&start_radio=1";
        let n = resolve(mix).unwrap().len();
        println!("mix link resolves to {n} video(s)");
        assert_eq!(n, 1);

        // Stopping mid-download ends cleanly and leaves no files behind.
        let dir = std::env::temp_dir().join("mh-cancel-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cancel: Cancel = Default::default();
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            c2.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let t = std::time::Instant::now();
        let r = download_audio("https://www.youtube.com/watch?v=aqz-KE-bpKQ", &dir, "test", None, &|_| {}, &cancel);
        let left: Vec<_> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        println!("cancel → {:?} after {:.1}s, files left: {:?}", r.as_ref().err(), t.elapsed().as_secs_f32(), left);
        assert!(r.is_err() && left.is_empty());
    }

    /// MH_CAT=1 [MH_SPOTIFY=<album url>] cargo test catalog_live -- --nocapture
    #[test]
    fn catalog_live() {
        if std::env::var("MH_CAT").is_err() {
            return;
        }
        let d = artist_albums("Nujabes").unwrap();
        println!("Nujabes discography ({}): {:?}", d.len(), d.iter().map(|a| format!("{} ({:?}, {} songs)", a.title, a.year, a.track_count)).collect::<Vec<_>>());
        if let Some(first) = d.first() {
            let url = format!("https://music.apple.com/us/album/x/{}", first.id);
            match resolve_catalog_link(&url) {
                Ok(CatalogLink::Album(a)) => println!("apple link → {} by {}: {} tracks, cover {:?}", a.title, a.artist, a.tracks.len(), a.cover_url.is_some()),
                Ok(CatalogLink::Song(a, t)) => println!("apple link → song {t} on {}", a.title),
                Err(e) => println!("apple link error: {e}"),
            }
        }
        if let Ok(sp) = std::env::var("MH_SPOTIFY") {
            match resolve_catalog_link(&sp) {
                Ok(CatalogLink::Album(a)) => println!("spotify → album {} by {}: {} tracks", a.title, a.artist, a.tracks.len()),
                Ok(CatalogLink::Song(a, t)) => println!("spotify → song {t} on {} ({} tracks)", a.title, a.tracks.len()),
                Err(e) => println!("spotify error: {e}"),
            }
        }
    }

    #[test]
    fn playlist_links() {
        assert!(!is_playlist_link("https://www.youtube.com/watch?v=abc&list=RDabc&start_radio=1"));
        assert!(!is_playlist_link("https://www.youtube.com/watch?v=abc&list=PLxyz"));
        assert!(!is_playlist_link("https://youtu.be/abc?list=PLxyz"));
        assert!(is_playlist_link("https://www.youtube.com/playlist?list=PLxyz"));
        assert!(is_playlist_link("https://music.youtube.com/playlist?list=OLAK5uy_abc"));
        assert!(!is_playlist_link("https://www.youtube.com/playlist?list=RDCLAK5uy_abc"));
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Feather (feat. Cise Starr) [Official Audio]").0, "Feather (feat. Cise Starr)");
        let (t, a) = clean_title("Nujabes - Feather [ Album : Modal Soul ]");
        assert_eq!(t, "Nujabes - Feather");
        assert_eq!(a.as_deref(), Some("Modal Soul"));
        assert_eq!(clean_title("Aruarian Dance (HD)").0, "Aruarian Dance");
        assert_eq!(strip_channel_suffix("Nujabes - Topic"), "Nujabes");
    }

    /// MH_YT_URL=<link> cargo test identify_live -- --nocapture  (metadata only)
    #[test]
    fn identify_live() {
        let Ok(url) = std::env::var("MH_YT_URL") else { return };
        let mut lib = LibrarySnapshot::default();
        lib.albums.push((norm("Nujabes"), norm("Modal Soul"), "Modal Soul".into()));
        for e in resolve(&url).unwrap().iter().take(3) {
            let info = full_info(e).unwrap();
            let (a, t, h) = identify(&info);
            println!("yt title: {:?}", get_str(&info, "title"));
            println!("  → artist {a:?}, title {t:?}, album hint {h:?}");
            println!("  → apple: {:?}", itunes_song(&a, &t, h.as_deref(), &lib));
        }
    }

    /// MH_YT_DL_URL=<link> MH_YT_ROOT=<dir> cargo test download_live -- --nocapture
    #[test]
    fn download_live() {
        let (Ok(url), Ok(root)) = (std::env::var("MH_YT_DL_URL"), std::env::var("MH_YT_ROOT")) else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        run(1, url, PathBuf::from(root), LibrarySnapshot::default(), tx, egui::Context::default(), Default::default());
        for e in rx.try_iter() {
            match e {
                Event::Status { .. } => {}
                other => println!("{other:?}"),
            }
        }
    }

    /// MH_TL=1 cargo test tracklist_live -- --nocapture  (metadata only)
    #[test]
    fn tracklist_live() {
        if std::env::var("MH_TL").is_err() {
            return;
        }
        for (artist, album, have) in [
            ("Nujabes", "Modal Soul", vec!["Feather", "ordinary joe", "reflection eternal", "Luv (sic.) pt3", "Eclipse", "The Sign", "Thank you", "World's end Rhapsody", "Modal Soul", "flowers", "Light on the land", "Horizon"]),
            ("Aphex Twin", "Selected Ambient Works Volume II", vec!["#2", "#3", "#6", "#8", "#15", "#16", "#17", "#20", "#21"]),
        ] {
            let Some(cat) = album_tracklist(artist, album) else {
                println!("{album}: not in catalogue");
                continue;
            };
            let missing: Vec<&CatalogTrack> = cat.tracks.iter().filter(|t| !have.iter().any(|h| same_song(h, &t.title))).collect();
            println!("{album}: {} tracks on Apple, missing {}: {:?}", cat.tracks.len(), missing.len(), missing.iter().map(|t| format!("{}. {}", t.track, t.title)).collect::<Vec<_>>());
            if let Some(m) = missing.first() {
                println!("  YouTube pick for “{}” ({}s): {:?}", m.title, m.duration_ms / 1000, find_on_youtube(m));
            }
        }
    }

    #[test]
    fn identifies_artist_and_title() {
        let v: Value = serde_json::json!({"title": "Nujabes - Feather (feat. Cise Starr & Akin from CYNE) [Official Audio]", "uploader": "Nujabes"});
        let (a, t, _) = identify(&v);
        assert_eq!(a, "Nujabes");
        assert_eq!(t, "Feather (feat. Cise Starr & Akin from CYNE)");
    }
}
