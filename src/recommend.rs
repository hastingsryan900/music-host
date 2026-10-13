//! "Recommended for You": scores every track in the library against what
//! you actually listen to. Everything is local — no network.
//!
//! Signals, roughly in order of weight:
//! - time spent listening to each artist (and genre)
//! - similarity to the current song (same artist / genre)
//! - the track's own listening time, and whether it's loved
//! - a nudge toward never-played songs by artists you like
//! - penalties for songs you skip and songs played very recently

use crate::library::{Library, Track, TrackId};
use std::collections::{HashMap, HashSet};

pub fn recommend(lib: &Library, current: TrackId, exclude: &HashSet<TrackId>, now: u64, n: usize) -> Vec<TrackId> {
    let Some(cur) = lib.track(current) else { return vec![] };
    let tracks = &lib.data.tracks;

    let mut artist_secs: HashMap<&str, f64> = HashMap::new();
    let mut genre_secs: HashMap<String, f64> = HashMap::new();
    for t in tracks {
        *artist_secs.entry(t.album_artist.as_str()).or_default() += t.listened_secs;
        if !t.genre.is_empty() {
            *genre_secs.entry(t.genre.to_lowercase()).or_default() += t.listened_secs;
        }
    }
    let max_artist = artist_secs.values().cloned().fold(1.0, f64::max);
    let max_genre = genre_secs.values().cloned().fold(1.0, f64::max);
    let max_track = tracks.iter().map(|t| t.listened_secs).fold(60.0, f64::max);
    let cur_genre = cur.genre.to_lowercase();

    let score = |t: &Track| -> f64 {
        let artist = artist_secs.get(t.album_artist.as_str()).copied().unwrap_or(0.0) / max_artist;
        let genre = if t.genre.is_empty() { 0.0 } else { genre_secs.get(&t.genre.to_lowercase()).copied().unwrap_or(0.0) / max_genre };
        let same_artist = (t.album_artist == cur.album_artist) as u8 as f64;
        let same_genre = (!cur_genre.is_empty() && t.genre.to_lowercase() == cur_genre) as u8 as f64;
        let same_album = (t.album_key() == cur.album_key()) as u8 as f64;
        let fav = (1.0 + t.listened_secs / 60.0).ln() / (1.0 + max_track / 60.0).ln();
        let discovery = if t.listened_secs < 1.0 && artist > 0.15 { 0.2 } else { 0.0 };

        let mut s = 0.9 * artist + 0.4 * genre + 0.55 * same_artist + 0.3 * same_genre + 0.45 * fav + discovery;
        if t.loved {
            s += 0.4;
        }
        // Prefer branching out a little from the album already playing.
        s -= 0.25 * same_album;
        // Don't suggest what you just heard.
        let age = now.saturating_sub(t.last_played);
        if t.last_played > 0 && age < 3600 {
            s *= 0.35;
        } else if t.last_played > 0 && age < 6 * 3600 {
            s *= 0.75;
        }
        s /= 1.0 + 0.6 * t.skips as f64;
        // Small stable jitter (per current song) so ties vary between songs.
        let mut h = t.id ^ current.rotate_left(17);
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51afd7ed558ccd);
        h ^= h >> 33;
        s + (h % 1000) as f64 / 1000.0 * 0.12
    };

    let mut scored: Vec<(f64, &Track)> = tracks
        .iter()
        .filter(|t| t.id != current && !exclude.contains(&t.id))
        .map(|t| (score(t), t))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    // Keep it varied: at most two picks per album.
    let mut per_album: HashMap<String, usize> = HashMap::new();
    let mut out = vec![];
    let mut seen = HashSet::new();
    for (_, t) in scored {
        let c = per_album.entry(t.album_key()).or_default();
        if *c >= 2 || !seen.insert(t.id) {
            continue;
        }
        *c += 1;
        out.push(t.id);
        if out.len() == n {
            break;
        }
    }
    out
}
