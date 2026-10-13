//! Album artwork.
//!
//! - Every album gets a *master* cover on disk (up to 1500×1500, JPEG q92).
//!   If the art inside your files is under 1000px, a background worker looks
//!   the album up on Apple's iTunes catalogue (artist + album name only) and
//!   saves the HD cover instead. Lookups are rate-limited and remembered.
//! - Grids and lists use small thumbnails; the album page and Now Playing ask
//!   for the full-resolution texture (`get_hd`), of which only a few are kept.

use crate::library::{app_dir, find_artwork, AlbumKey};
use egui::{Color32, ColorImage, Context, TextureHandle, TextureId, TextureOptions};
use image::DynamicImage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const THUMB: u32 = 512;
const MASTER: u32 = 1500;
const HD_MIN: u32 = 1000;
const HD_KEEP: usize = 6;
const WORKERS: usize = 4;

pub enum Art {
    Loading,
    Ready {
        tex: TextureHandle,
        /// Heavily blurred copy, stretched behind pages as an ambient wash.
        blur: TextureHandle,
        tint: Color32,
        loaded_at: f64,
    },
    Missing,
}

#[derive(Clone)]
struct Job {
    key: AlbumKey,
    track_path: PathBuf,
    artist: String,
    album: String,
    hd: bool,
    /// Remote cover (catalogue albums you don't own yet).
    url: Option<String>,
}

enum Done {
    Thumb { key: AlbumKey, img: Option<(ColorImage, ColorImage, Color32)> },
    Hd { key: AlbumKey, img: Option<ColorImage> },
    /// An HD master was just saved: reload this album's textures.
    Upgraded { key: AlbumKey },
}

struct Hd {
    tex: TextureHandle,
    loaded_at: f64,
    used: f64,
}

type Queue = Arc<(Mutex<VecDeque<Job>>, Condvar)>;

pub struct ArtCache {
    map: HashMap<AlbumKey, Art>,
    hd: HashMap<AlbumKey, Option<Hd>>,
    queue: Queue,
    upgrades: Queue,
    queued_upgrades: HashSet<AlbumKey>,
    rx: Receiver<Done>,
}

fn cache_dir() -> PathBuf {
    let d = app_dir().join("artwork");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn hash(key: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The HD master cover for an album (may not exist yet).
pub fn cache_path(key: &str) -> PathBuf {
    cache_dir().join(format!("{}.jpg", hash(key)))
}

fn misses_path() -> PathBuf {
    cache_dir().join("lookup-misses.txt")
}

fn known_miss(key: &str) -> bool {
    std::fs::read_to_string(misses_path())
        .map(|s| s.lines().any(|l| l == hash(key)))
        .unwrap_or(false)
}

fn record_miss(key: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(misses_path()) {
        let _ = writeln!(f, "{}", hash(key));
    }
}

fn square(img: DynamicImage, max: u32) -> DynamicImage {
    let n = img.width().min(img.height()).min(max);
    if img.width() == n && img.height() == n {
        img
    } else {
        img.resize_to_fill(n, n, image::imageops::FilterType::Lanczos3)
    }
}

fn save_master(img: &DynamicImage, path: &Path) {
    let tmp = path.with_extension("tmp");
    if let Ok(f) = std::fs::File::create(&tmp) {
        let mut w = std::io::BufWriter::new(f);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 92);
        if img.to_rgb8().write_with_encoder(enc).is_ok() {
            drop(w);
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Best available image right now: the saved master, else the file's own art.
fn best_local(job: &Job) -> Option<DynamicImage> {
    if let Ok(img) = image::open(cache_path(&job.key)) {
        return Some(img);
    }
    let bytes = find_artwork(&job.track_path)?;
    let img = square(image::load_from_memory(&bytes).ok()?, MASTER);
    // Already HD? Make it the master so we never look it up.
    if img.width() >= HD_MIN {
        save_master(&img, &cache_path(&job.key));
    }
    Some(img)
}

fn needs_upgrade(job: &Job) -> bool {
    !cache_path(&job.key).exists() && !known_miss(&job.key)
}

/// A remote cover, cached on disk after the first download.
fn remote(url: &str) -> Option<DynamicImage> {
    let path = cache_dir().join(format!("remote-{}.jpg", hash(url)));
    if let Ok(img) = image::open(&path) {
        return Some(img);
    }
    let mut bytes = vec![];
    let reader = ureq::get(url).timeout(Duration::from_secs(15)).call().ok()?.into_reader();
    std::io::Read::take(reader, 10 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let _ = std::fs::write(&path, &bytes);
    Some(img)
}

fn thumb(job: &Job) -> Option<(ColorImage, ColorImage, Color32)> {
    let img = match &job.url {
        Some(u) => remote(u)?,
        None => best_local(job)?,
    };
    let small = img.resize(THUMB, THUMB, image::imageops::FilterType::Lanczos3);
    let rgba = small.to_rgba8();
    // Average colour drives the accent + page tint.
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in rgba.pixels().step_by(5) {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
        n += 1;
    }
    let n = n.max(1);
    let tint = Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8);
    // Tiny downscale → upscale → gaussian blur gives a smooth colour field.
    let tiny = image::imageops::resize(&rgba, 12, 12, image::imageops::FilterType::Triangle);
    let big = image::imageops::resize(&tiny, 192, 192, image::imageops::FilterType::CatmullRom);
    let blurred = image::imageops::blur(&big, 18.0);
    let blur = ColorImage::from_rgba_unmultiplied([192, 192], blurred.as_raw());
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some((ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), blur, tint))
}

fn full(job: &Job) -> Option<ColorImage> {
    let rgba = best_local(job)?.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

// ---------------------------------------------------------------- iTunes lookup

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

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// "Take Care (Deluxe)" → "take care"
fn base_title(s: &str) -> String {
    let cut = s.find(['(', '[']).unwrap_or(s.len());
    norm(&s[..cut])
}

fn itunes_search(term: &str) -> Option<Vec<serde_json::Value>> {
    let url = format!("https://itunes.apple.com/search?term={}&entity=album&limit=25", url_encode(term));
    let resp: serde_json::Value = ureq::get(&url).timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    resp.get("results")?.as_array().cloned()
}

fn field<'a>(r: &'a serde_json::Value, k: &str) -> &'a str {
    r.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

/// Ask Apple's public search API for an album cover at 1500px.
///
/// First by artist + album (artist must match). If that finds nothing —
/// e.g. "Various Artists" soundtracks, or artists tagged in another
/// language — fall back to the album title alone, accepting only an exact
/// title match on a reasonably distinctive title.
fn itunes_cover(artist: &str, album: &str) -> Option<Vec<u8>> {
    let short = album.split(['(', '[']).next().unwrap_or(album).trim();
    let (na, nt, bt) = (norm(artist), norm(album), base_title(album));
    let mut best: Option<serde_json::Value> = None;
    if !na.is_empty() && na != "variousartists" {
        if let Some(results) = itunes_search(&format!("{artist} {short}")) {
            let score = |r: &serde_json::Value| -> i32 {
                let ra = norm(field(r, "artistName"));
                let rt_raw = field(r, "collectionName");
                let rt = norm(rt_raw);
                if ra.is_empty() || !(ra == na || ra.contains(&na) || na.contains(&ra)) {
                    return 0;
                }
                if rt == nt {
                    100
                } else if base_title(rt_raw) == bt {
                    80
                } else if rt.starts_with(&bt) || bt.starts_with(&rt) {
                    50
                } else {
                    0
                }
            };
            best = results.iter().filter(|r| score(r) > 0).max_by_key(|r| score(r)).cloned();
        }
    }
    if best.is_none() && nt.chars().count() >= 10 {
        if let Some(results) = itunes_search(short) {
            best = results
                .iter()
                .find(|r| norm(field(r, "collectionName")) == nt)
                .or_else(|| results.iter().find(|r| base_title(field(r, "collectionName")) == bt && bt.chars().count() >= 10))
                .cloned();
        }
    }
    let best = best?;
    let art = field(&best, "artworkUrl100").replace("100x100bb", &format!("{MASTER}x{MASTER}bb"));
    if art.is_empty() {
        return None;
    }
    let mut bytes = vec![];
    use std::io::Read;
    ureq::get(&art)
        .timeout(Duration::from_secs(20))
        .call()
        .ok()?
        .into_reader()
        .take(20 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn upgrade(job: &Job) -> bool {
    if !needs_upgrade(job) {
        return false;
    }
    let local = find_artwork(&job.track_path).and_then(|b| image::load_from_memory(&b).ok());
    let local_size = local.as_ref().map(|i| i.width().min(i.height())).unwrap_or(0);
    if local_size >= HD_MIN {
        return false; // best_local() already promoted it
    }
    match itunes_cover(&job.artist, &job.album).and_then(|b| image::load_from_memory(&b).ok()) {
        Some(img) if img.width().min(img.height()) > local_size => {
            save_master(&square(img, MASTER), &cache_path(&job.key));
            true
        }
        _ => {
            record_miss(&job.key);
            false
        }
    }
}

fn spawn_worker<F>(q: Queue, lifo: bool, ctx: Context, mut f: F)
where
    F: FnMut(Job) + Send + 'static,
{
    std::thread::spawn(move || loop {
        let job = {
            let (lock, cv) = &*q;
            let mut guard = lock.lock().unwrap();
            loop {
                // LIFO for on-screen art: the most recent request is what's visible.
                let j = if lifo { guard.pop_back() } else { guard.pop_front() };
                if let Some(j) = j {
                    break j;
                }
                guard = cv.wait(guard).unwrap();
            }
        };
        f(job);
        ctx.request_repaint();
    });
}

impl ArtCache {
    pub fn new(ctx: &Context) -> Self {
        let queue: Queue = Default::default();
        let upgrades: Queue = Default::default();
        let (tx, rx) = channel::<Done>();
        for _ in 0..WORKERS {
            let tx: Sender<Done> = tx.clone();
            spawn_worker(queue.clone(), true, ctx.clone(), move |job| {
                let msg = if job.hd {
                    Done::Hd { key: job.key.clone(), img: full(&job) }
                } else {
                    Done::Thumb { key: job.key.clone(), img: thumb(&job) }
                };
                let _ = tx.send(msg);
            });
        }
        // One slow, polite worker for online lookups (Apple allows ~20/min).
        let utx = tx.clone();
        let mut last = Instant::now() - Duration::from_secs(10);
        spawn_worker(upgrades.clone(), false, ctx.clone(), move |job| {
            if !needs_upgrade(&job) {
                return;
            }
            let wait = Duration::from_millis(3200).saturating_sub(last.elapsed());
            std::thread::sleep(wait);
            last = Instant::now();
            if upgrade(&job) {
                let _ = utx.send(Done::Upgraded { key: job.key });
            }
        });
        ArtCache {
            map: HashMap::new(),
            hd: HashMap::new(),
            queue,
            upgrades,
            queued_upgrades: HashSet::new(),
            rx,
        }
    }

    /// Upload finished images. Call once per frame.
    pub fn pump(&mut self, ctx: &Context) {
        let now = ctx.input(|i| i.time);
        let mut budget = 6; // spread GPU uploads over frames
        while budget > 0 {
            let Ok(d) = self.rx.try_recv() else { break };
            budget -= 1;
            match d {
                Done::Thumb { key, img } => {
                    let art = match img {
                        Some((img, blur, tint)) => Art::Ready {
                            tex: ctx.load_texture(format!("art:{key}"), img, TextureOptions::LINEAR),
                            blur: ctx.load_texture(format!("blur:{key}"), blur, TextureOptions::LINEAR),
                            tint,
                            loaded_at: now,
                        },
                        None => Art::Missing,
                    };
                    self.map.insert(key, art);
                }
                Done::Hd { key, img } => {
                    let hd = img.map(|img| Hd {
                        tex: ctx.load_texture(format!("hd:{key}"), img, TextureOptions::LINEAR),
                        loaded_at: now,
                        used: now,
                    });
                    self.hd.insert(key, hd);
                    self.evict_hd();
                }
                Done::Upgraded { key } => {
                    // Reload from the new HD master on next request.
                    self.map.remove(&key);
                    self.hd.remove(&key);
                }
            }
        }
        if budget == 0 {
            ctx.request_repaint();
        }
    }

    fn evict_hd(&mut self) {
        while self.hd.values().filter(|h| h.is_some()).count() > HD_KEEP {
            let oldest = self
                .hd
                .iter()
                .filter_map(|(k, h)| h.as_ref().map(|h| (k.clone(), h.used)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k);
            match oldest {
                Some(k) => {
                    self.hd.remove(&k);
                }
                None => break,
            }
        }
    }

    fn job(key: &AlbumKey, track_path: &Path, artist: &str, album: &str, hd: bool) -> Job {
        Job {
            key: key.clone(),
            track_path: track_path.to_path_buf(),
            artist: artist.to_string(),
            album: album.to_string(),
            hd,
            url: None,
        }
    }

    /// Cover art from a URL (e.g. albums you don't own yet).
    pub fn get_url(&mut self, url: &str) -> &Art {
        let key = format!("url:{url}");
        if !self.map.contains_key(&key) {
            self.map.insert(key.clone(), Art::Loading);
            let mut job = Self::job(&key, Path::new(""), "", "", false);
            job.url = Some(url.to_string());
            let (lock, cv) = &*self.queue;
            lock.lock().unwrap().push_back(job);
            cv.notify_one();
        }
        &self.map[&key]
    }

    /// Thumbnail-sized art for grids and lists.
    pub fn get(&mut self, key: &AlbumKey, track_path: &Path, artist: &str, album: &str) -> &Art {
        if !self.map.contains_key(key) {
            self.map.insert(key.clone(), Art::Loading);
            let job = Self::job(key, track_path, artist, album, false);
            let (lock, cv) = &*self.queue;
            lock.lock().unwrap().push_back(job.clone());
            cv.notify_one();
            if self.queued_upgrades.insert(key.clone()) {
                let (lock, cv) = &*self.upgrades;
                lock.lock().unwrap().push_back(job);
                cv.notify_one();
            }
        }
        &self.map[key]
    }

    /// Full-resolution art for big displays. `None` until loaded — callers
    /// draw the thumbnail meanwhile. Returns (texture, loaded_at).
    pub fn get_hd(&mut self, key: &AlbumKey, track_path: &Path, artist: &str, album: &str, now: f64) -> Option<(TextureId, f64)> {
        match self.hd.get_mut(key) {
            Some(Some(h)) => {
                h.used = now;
                Some((h.tex.id(), h.loaded_at))
            }
            Some(None) => None,
            None => {
                self.hd.insert(key.clone(), None);
                let (lock, cv) = &*self.queue;
                lock.lock().unwrap().push_back(Self::job(key, track_path, artist, album, true));
                cv.notify_one();
                None
            }
        }
    }

    /// Forget cached art (e.g. after a rescan) so it gets re-extracted.
    pub fn clear(&mut self) {
        self.map.clear();
        self.hd.clear();
        self.queued_upgrades.clear();
        self.queue.0.lock().unwrap().clear();
        self.upgrades.0.lock().unwrap().clear();
        let _ = std::fs::remove_dir_all(cache_dir());
    }
}
