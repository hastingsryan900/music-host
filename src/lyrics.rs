//! Song lyrics: a `.lrc` file next to the song, lyrics embedded in its
//! tags, or LRCLIB (free, no key) — time-synced whenever possible.
//! Online results are cached on disk, including "nothing found".

use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Lyrics {
    /// (start time in seconds, line) — sorted by time.
    Synced(Vec<(f64, String)>),
    Plain(String),
    Instrumental,
    None,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    at: u64,
    lyrics: Lyrics,
}

pub struct Query {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_path(q: &Query) -> PathBuf {
    let dir = crate::library::app_dir().join("lyrics");
    let _ = std::fs::create_dir_all(&dir);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}|{}|{}", q.artist.to_lowercase(), q.title.to_lowercase(), q.duration.round()).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{h:016x}.json"))
}

/// Find lyrics for a song (blocking; call from a worker thread).
pub fn find(q: &Query) -> Lyrics {
    if let Some(l) = local(&q.path) {
        return l;
    }
    let cache = cache_path(q);
    if let Some(c) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Cached>(&b).ok()) {
        // "Not found" is retried after a few days; real lyrics are kept.
        if !matches!(c.lyrics, Lyrics::None) || now().saturating_sub(c.at) < 3 * 86400 {
            return c.lyrics;
        }
    }
    let Some(l) = online(q) else {
        return Lyrics::None; // network trouble: don't cache
    };
    if let Ok(b) = serde_json::to_vec(&Cached { at: now(), lyrics: l.clone() }) {
        let _ = std::fs::write(&cache, b);
    }
    l
}

fn local(path: &Path) -> Option<Lyrics> {
    if let Ok(s) = std::fs::read_to_string(path.with_extension("lrc")) {
        if let Some(l) = parse(&s) {
            return Some(l);
        }
    }
    use lofty::file::TaggedFileExt;
    use lofty::tag::ItemKey;
    let file = lofty::probe::Probe::open(path).ok()?.guess_file_type().ok()?.read().ok()?;
    let text = file.tags().iter().find_map(|t| t.get_string(&ItemKey::Lyrics).map(String::from))?;
    parse(&text)
}

/// LRC text ("[01:23.45]line") → synced lines; anything else → plain text.
pub fn parse(s: &str) -> Option<Lyrics> {
    let mut lines = vec![];
    for raw in s.lines() {
        let mut rest = raw.trim();
        let mut times = vec![];
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            if let Some(t) = parse_time(tag) {
                times.push(t);
            }
            rest = &stripped[end + 1..];
        }
        for t in times {
            lines.push((t, rest.trim().to_string()));
        }
    }
    if lines.len() >= 3 {
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        return Some(Lyrics::Synced(lines));
    }
    let plain = s.trim();
    (!plain.is_empty()).then(|| Lyrics::Plain(plain.to_string()))
}

fn parse_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    Some(m * 60.0 + s)
}

fn from_record(v: &serde_json::Value) -> Option<Lyrics> {
    if v["instrumental"].as_bool() == Some(true) {
        return Some(Lyrics::Instrumental);
    }
    if let Some(l) = v["syncedLyrics"].as_str().and_then(parse) {
        return Some(l);
    }
    v["plainLyrics"].as_str().and_then(|s| (!s.trim().is_empty()).then(|| Lyrics::Plain(s.trim().to_string())))
}

fn get(url: &str, params: &[(&str, &str)]) -> Result<Option<serde_json::Value>, ()> {
    let mut req = ureq::get(url).timeout(Duration::from_secs(12)).set("User-Agent", "Music Host (https://github.com/)");
    for (k, v) in params {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(r) => r.into_json().map(Some).map_err(|_| ()),
        Err(ureq::Error::Status(400 | 404, _)) => Ok(None),
        Err(_) => Err(()),
    }
}

/// LRCLIB lookup. None = couldn't reach it (so don't cache).
fn online(q: &Query) -> Option<Lyrics> {
    let title = crate::ytdl::without_feat(&q.title);
    let artist = q.artist.split(", ").next().unwrap_or(&q.artist).to_string();
    let dur = format!("{}", q.duration.round() as u64);
    // Exact match first (title, artist, album, duration ±2s).
    let exact = if q.album.is_empty() || q.duration <= 0.0 {
        None
    } else {
        get("https://lrclib.net/api/get", &[("track_name", &title), ("artist_name", &artist), ("album_name", &q.album), ("duration", &dur)]).ok()?
    };
    if let Some(l) = exact.as_ref().and_then(from_record) {
        return Some(l);
    }
    // Then search, preferring synced lyrics with the closest length.
    let found = get("https://lrclib.net/api/search", &[("track_name", &title), ("artist_name", &artist)]).ok()?;
    let mut best: Option<(f64, Lyrics)> = None;
    for v in found.as_ref().and_then(|v| v.as_array()).into_iter().flatten() {
        let d = v["duration"].as_f64().unwrap_or(0.0);
        if (d - q.duration).abs() > 8.0 && q.duration > 0.0 {
            continue;
        }
        let Some(l) = from_record(v) else { continue };
        let score = (d - q.duration).abs() + if matches!(l, Lyrics::Synced(_)) { 0.0 } else { 20.0 };
        if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            best = Some((score, l));
        }
    }
    Some(best.map(|(_, l)| l).unwrap_or(Lyrics::None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let s = "[ar:Someone]\n[00:01.50]First\n[00:04.00][01:00.00]Chorus\n[00:07.25]Third\n";
        let Some(Lyrics::Synced(l)) = parse(s) else { panic!() };
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], (1.5, "First".to_string()));
        assert_eq!(l[1].1, "Chorus");
        assert_eq!(l[3].0, 60.0);
        assert!(matches!(parse("just words\nmore words"), Some(Lyrics::Plain(_))));
    }

    #[test]
    #[ignore]
    fn live_lrclib() {
        let q = Query { path: PathBuf::from("/nonexistent.m4a"), title: "Luv(sic.) pt3".into(), artist: "Nujabes".into(), album: "".into(), duration: 0.0 };
        let l = online(&q);
        eprintln!("{l:?}");
        assert!(matches!(l, Some(Lyrics::Synced(_)) | Some(Lyrics::Plain(_))));
    }
}
