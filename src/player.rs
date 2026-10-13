//! Audio playback + play queue (shuffle, loop, up next).
//!
//! Playback is one continuous stream (`Chain`) fed to rodio. The next song is
//! opened and pre-buffered while the current one plays and is handed over on
//! the exact sample — gapless, and independent of how often the UI runs.
//! Each song carries a gain (volume levelling). The player also follows the
//! macOS default output device (AirPods, speakers, DACs…) and re-opens at the
//! new device's sample rate, resuming at the same spot.

use crate::decode::AudioSource;
use crate::library::{Library, TrackId};
use rand::seq::SliceRandom;
use rodio::source::SeekError;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Off,
    /// Loop the current song.
    One,
}

// ---------------------------------------------------------------- gapless chain

struct Slot {
    src: AudioSource,
    id: TrackId,
    gain: f32,
}

/// State shared between the audio thread (Chain) and the Player.
#[derive(Default)]
struct ChainShared {
    next: Mutex<Option<Slot>>,
    current_id: AtomicU64,
    /// Bumped each time the chain moves on to the queued next song.
    transitions: AtomicU64,
    /// Frames played of the current song.
    frames: AtomicU64,
    /// Linear gain of the current song (f32 bits) — adjustable live.
    gain: AtomicU32,
    ended: AtomicBool,
}

struct Chain {
    cur: Option<Slot>,
    shared: Arc<ChainShared>,
    rate: u32,
    odd: bool,
}

impl Iterator for Chain {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(c) = &mut self.cur {
                if let Some(s) = c.src.next() {
                    // Stereo: count a frame every second sample.
                    self.odd = !self.odd;
                    if !self.odd {
                        self.shared.frames.fetch_add(1, Ordering::Relaxed);
                    }
                    return Some(s * f32::from_bits(self.shared.gain.load(Ordering::Relaxed)));
                }
            }
            // Current song finished: hand over to the queued one, if any.
            let next = self.shared.next.lock().ok().and_then(|mut n| n.take());
            match next {
                Some(slot) => {
                    self.shared.current_id.store(slot.id, Ordering::Relaxed);
                    self.shared.gain.store(slot.gain.to_bits(), Ordering::Relaxed);
                    self.shared.frames.store(0, Ordering::Relaxed);
                    self.shared.transitions.fetch_add(1, Ordering::Relaxed);
                    self.odd = false;
                    self.cur = Some(slot);
                }
                None => {
                    self.cur = None;
                    self.shared.ended.store(true, Ordering::Relaxed);
                    return None;
                }
            }
        }
    }
}

impl Source for Chain {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        if let Some(c) = &mut self.cur {
            c.src.try_seek(pos)?;
            self.shared.frames.store((pos.as_secs_f64() * self.rate as f64) as u64, Ordering::Relaxed);
            self.odd = false;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- output device

struct Output {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    rate: u32,
    name: String,
}

fn default_device() -> Option<(String, u32)> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let d = rodio::cpal::default_host().default_output_device()?;
    let rate = d.default_output_config().ok()?.sample_rate().0;
    Some((d.name().unwrap_or_default(), rate))
}

fn open_output() -> Result<Output, String> {
    let (name, rate) = default_device().ok_or("no audio output device")?;
    let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
    Ok(Output { _stream: stream, handle, rate, name })
}

// ---------------------------------------------------------------- player

pub struct Player {
    out: Option<Output>,
    sink: Option<Sink>,
    chain: Option<Arc<ChainShared>>,
    seen_transitions: u64,
    /// Order index of the song pre-buffered as next (if any).
    preloaded: Option<usize>,
    needs_preload: bool,
    last_device_check: Instant,
    /// The queue in its original (unshuffled) order.
    source: Vec<TrackId>,
    /// Indices into `source`, in play order.
    order: Vec<usize>,
    /// Position within `order`.
    pos: usize,
    current: Option<TrackId>,
    playing: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    volume: f32,
    /// Even out loudness between songs.
    pub level_volume: bool,
    pub error: Option<String>,
    /// Set when a new track starts, so the app can bump play counts / Now Playing.
    pub just_started: Option<TrackId>,
    /// Set when playback moved to a different output device.
    pub device_changed: Option<String>,
}

/// Target loudness for levelling (LUFS, the streaming-service norm).
const TARGET_LUFS: f32 = -14.0;

impl Player {
    pub fn new(volume: f32) -> Self {
        let (out, error) = match open_output() {
            Ok(o) => (Some(o), None),
            Err(e) => (None, Some(format!("No audio output: {e}"))),
        };
        Player {
            out,
            sink: None,
            chain: None,
            seen_transitions: 0,
            preloaded: None,
            needs_preload: false,
            last_device_check: Instant::now(),
            source: vec![],
            order: vec![],
            pos: 0,
            current: None,
            playing: false,
            shuffle: false,
            repeat: Repeat::Off,
            volume,
            level_volume: true,
            error,
            just_started: None,
            device_changed: None,
        }
    }

    pub fn current(&self) -> Option<TrackId> {
        self.current
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(s) = &self.sink {
            // Perceptual curve so the slider feels linear.
            s.set_volume(self.volume * self.volume);
        }
    }

    pub fn position(&self) -> f64 {
        match (&self.chain, &self.out) {
            (Some(c), Some(o)) => c.frames.load(Ordering::Relaxed) as f64 / o.rate as f64,
            _ => 0.0,
        }
    }

    /// Linear gain that levels `id` to the target loudness. Uses the whole
    /// album's loudness when the neighbouring queue items are from the same
    /// album (so an album's quiet and loud songs keep their balance).
    fn gain_for(&self, lib: &Library, order_idx: usize) -> f32 {
        if !self.level_volume {
            return 1.0;
        }
        let id = self.source[self.order[order_idx]];
        let Some(t) = lib.track(id) else { return 1.0 };
        let Some(track_lufs) = t.loudness else { return 1.0 };
        let key = t.album_key();
        let same_album = |i: Option<usize>| {
            i.and_then(|i| self.order.get(i))
                .and_then(|&j| lib.track(self.source[j]))
                .map(|n| n.album_key() == key)
                .unwrap_or(false)
        };
        let album_context = same_album(order_idx.checked_sub(1)) || same_album(Some(order_idx + 1));
        let (lufs, peak) = match (album_context, lib.album_of(id)) {
            (true, Some(a)) => {
                // Energy-average loudness and max peak over the album.
                let mut energy = 0.0f64;
                let mut weight = 0.0f64;
                let mut peak = 0.0f32;
                for tid in &a.tracks {
                    if let Some(at) = lib.track(*tid) {
                        if let Some(l) = at.loudness {
                            let w = at.duration_secs.max(1.0);
                            energy += w * 10f64.powf(l as f64 / 10.0);
                            weight += w;
                            peak = peak.max(at.peak.unwrap_or(1.0));
                        }
                    }
                }
                if weight > 0.0 {
                    ((10.0 * (energy / weight).log10()) as f32, peak)
                } else {
                    (track_lufs, t.peak.unwrap_or(1.0))
                }
            }
            _ => (track_lufs, t.peak.unwrap_or(1.0)),
        };
        let mut db = (TARGET_LUFS - lufs).clamp(-15.0, 10.0);
        // Never push a song into clipping.
        let max_db = 20.0 * (0.98 / peak.max(1e-4)).log10();
        db = db.min(max_db);
        10f32.powf(db / 20.0)
    }

    /// Replace the queue with `tracks` and start at `start`.
    pub fn play_list(&mut self, lib: &Library, tracks: Vec<TrackId>, start: usize) {
        if tracks.is_empty() {
            return;
        }
        self.source = tracks;
        self.order = (0..self.source.len()).collect();
        let start = start.min(self.source.len() - 1);
        if self.shuffle {
            self.reshuffle(start);
        } else {
            self.pos = start;
        }
        self.load_current(lib);
    }

    /// Replace the queue and play it shuffled.
    pub fn shuffle_list(&mut self, lib: &Library, tracks: Vec<TrackId>) {
        if tracks.is_empty() {
            return;
        }
        self.shuffle = true;
        let start = rand::random::<usize>() % tracks.len();
        self.play_list(lib, tracks, start);
    }

    fn reshuffle(&mut self, keep_first: usize) {
        let mut rest: Vec<usize> = (0..self.source.len()).filter(|&i| i != keep_first).collect();
        rest.shuffle(&mut rand::thread_rng());
        self.order = std::iter::once(keep_first).chain(rest).collect();
        self.pos = 0;
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        if self.order.is_empty() {
            return;
        }
        let cur = self.order[self.pos];
        if self.shuffle {
            self.reshuffle(cur);
        } else {
            self.order = (0..self.source.len()).collect();
            self.pos = cur;
        }
        self.needs_preload = true;
    }

    /// Loop the current song on/off.
    pub fn toggle_loop(&mut self) {
        self.repeat = if self.repeat == Repeat::One { Repeat::Off } else { Repeat::One };
        self.needs_preload = true;
    }

    /// Tracks that will play after the current one.
    pub fn up_next(&self) -> Vec<TrackId> {
        if self.order.is_empty() {
            return vec![];
        }
        self.order[self.pos + 1..].iter().map(|&i| self.source[i]).collect()
    }

    /// Jump to the n-th item of `up_next()`.
    pub fn jump_up_next(&mut self, lib: &Library, n: usize) {
        if self.pos + 1 + n < self.order.len() {
            self.pos += 1 + n;
            self.load_current(lib);
        }
    }

    pub fn remove_up_next(&mut self, n: usize) {
        let i = self.pos + 1 + n;
        if i < self.order.len() {
            self.order.remove(i);
            self.needs_preload = true;
        }
    }

    pub fn play_next(&mut self, id: TrackId) {
        self.insert(id, true);
    }

    pub fn add_to_queue(&mut self, id: TrackId) {
        self.insert(id, false);
    }

    fn insert(&mut self, id: TrackId, next: bool) {
        self.source.push(id);
        let idx = self.source.len() - 1;
        if self.order.is_empty() {
            self.order.push(idx);
            self.pos = 0;
            return;
        }
        if next {
            self.order.insert(self.pos + 1, idx);
        } else {
            self.order.push(idx);
        }
        self.needs_preload = true;
    }

    pub fn toggle(&mut self, lib: &Library) {
        match &self.sink {
            Some(s) if self.playing => {
                s.pause();
                self.playing = false;
            }
            Some(s) if !s.empty() => {
                s.play();
                self.playing = true;
            }
            _ => {
                if !self.order.is_empty() {
                    self.load_current(lib);
                }
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(s) = &self.sink {
            s.pause();
        }
        self.playing = false;
    }

    pub fn play(&mut self, lib: &Library) {
        if !self.playing {
            self.toggle(lib);
        }
    }

    pub fn seek(&mut self, secs: f64) {
        if let Some(s) = &self.sink {
            let _ = s.try_seek(Duration::from_secs_f64(secs.max(0.0)));
        }
    }

    pub fn next(&mut self, lib: &Library) {
        if self.order.is_empty() {
            return;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat != Repeat::Off {
            self.pos = 0;
        } else {
            self.stop();
            self.pos = 0;
            return;
        }
        self.load_current(lib);
    }

    pub fn prev(&mut self, lib: &Library) {
        if self.position() > 3.0 || self.pos == 0 {
            self.seek(0.0);
            if !self.playing {
                self.play(lib);
            }
            return;
        }
        self.pos -= 1;
        self.load_current(lib);
    }

    fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.chain = None;
        self.preloaded = None;
        self.playing = false;
        self.current = None;
    }

    fn load_current(&mut self, lib: &Library) {
        // Try the current item, skipping over unplayable files.
        for _ in 0..self.order.len().max(1) {
            let Some(&i) = self.order.get(self.pos) else { return };
            let id = self.source[i];
            match self.start(lib, id, None) {
                Ok(()) => {
                    self.error = None;
                    return;
                }
                Err(e) => {
                    let name = lib.track(id).map(|t| t.title.clone()).unwrap_or_default();
                    self.error = Some(format!("Couldn't play “{name}”: {e}"));
                    if self.pos + 1 < self.order.len() {
                        self.pos += 1;
                    } else {
                        self.stop();
                        return;
                    }
                }
            }
        }
    }

    fn open_source(&self, lib: &Library, id: TrackId) -> Result<AudioSource, String> {
        let out = self.out.as_ref().ok_or("no audio device")?;
        let track = lib.track(id).ok_or("track missing from library")?;
        AudioSource::open_at(&track.path, Some(out.rate))
    }

    /// Start a fresh stream with `id` (optionally from `at` seconds).
    fn start(&mut self, lib: &Library, id: TrackId, at: Option<f64>) -> Result<(), String> {
        let mut src = self.open_source(lib, id)?;
        if let Some(secs) = at {
            let _ = src.try_seek(Duration::from_secs_f64(secs));
        }
        let out = self.out.as_ref().ok_or("no audio device")?;
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        let gain = self.gain_for(lib, self.pos);
        let shared = Arc::new(ChainShared::default());
        shared.current_id.store(id, Ordering::Relaxed);
        shared.gain.store(gain.to_bits(), Ordering::Relaxed);
        shared.frames.store((at.unwrap_or(0.0) * out.rate as f64) as u64, Ordering::Relaxed);
        let chain = Chain { cur: Some(Slot { src, id, gain }), shared: shared.clone(), rate: out.rate, odd: false };
        let sink = Sink::try_new(&out.handle).map_err(|e| e.to_string())?;
        sink.set_volume(self.volume * self.volume);
        sink.append(chain);
        sink.play();
        self.sink = Some(sink);
        self.chain = Some(shared);
        self.seen_transitions = 0;
        self.preloaded = None;
        self.current = Some(id);
        self.playing = true;
        if at.is_none() {
            self.just_started = Some(id);
        }
        self.preload_next(lib);
        Ok(())
    }

    /// The order index that should follow the current song.
    fn next_index(&self) -> Option<usize> {
        if self.order.is_empty() {
            return None;
        }
        if self.repeat == Repeat::One {
            Some(self.pos)
        } else if self.pos + 1 < self.order.len() {
            Some(self.pos + 1)
        } else {
            None
        }
    }

    /// Open + pre-buffer the following song and queue it for a gapless handoff.
    fn preload_next(&mut self, lib: &Library) {
        self.needs_preload = false;
        let Some(chain) = self.chain.clone() else { return };
        let want = self.next_index();
        let slot = want.and_then(|i| {
            let id = self.source[self.order[i]];
            let src = self.open_source(lib, id).ok()?;
            Some(Slot { src, id, gain: self.gain_for(lib, i) })
        });
        self.preloaded = if slot.is_some() { want } else { None };
        // Dropping a stale preload stops its decoder thread.
        let old = match chain.next.lock() {
            Ok(mut n) => std::mem::replace(&mut *n, slot),
            Err(_) => None,
        };
        drop(old);
    }

    /// Re-apply levelling (e.g. after the setting changed or loudness data
    /// arrived) to the playing song and the queued one.
    pub fn refresh_gain(&mut self, lib: &Library) {
        if let Some(chain) = &self.chain {
            chain.gain.store(self.gain_for(lib, self.pos).to_bits(), Ordering::Relaxed);
        }
        self.needs_preload = true;
    }

    /// Call every frame: tracks gapless handoffs, keeps the next song
    /// queued, and follows output-device changes.
    pub fn tick(&mut self, lib: &Library) {
        if let Some(chain) = self.chain.clone() {
            let t = chain.transitions.load(Ordering::Relaxed);
            if t > self.seen_transitions {
                self.seen_transitions = t;
                if let Some(i) = self.preloaded.take() {
                    self.pos = i;
                }
                let id = chain.current_id.load(Ordering::Relaxed);
                self.current = Some(id);
                self.just_started = Some(id);
                self.preload_next(lib);
            } else if self.needs_preload {
                self.preload_next(lib);
            }
            if self.playing && chain.ended.load(Ordering::Relaxed) {
                // End of the queue.
                self.stop();
                self.pos = 0;
            }
        }
        if self.last_device_check.elapsed() > Duration::from_millis(1500) {
            self.last_device_check = Instant::now();
            self.follow_device(lib);
        }
    }

    /// Testing aid: make the next device check believe the output changed.
    pub fn debug_force_device_change(&mut self) {
        if let Some(o) = &mut self.out {
            o.name = String::from("(previous device)");
        }
        self.last_device_check = Instant::now() - Duration::from_secs(10);
    }

    /// If macOS switched outputs (AirPods connected, etc.), move playback
    /// there at the same position, at the new device's sample rate.
    fn follow_device(&mut self, lib: &Library) {
        let Some((name, rate)) = default_device() else { return };
        let changed = match &self.out {
            Some(o) => o.name != name || o.rate != rate,
            None => true,
        };
        if !changed {
            return;
        }
        let resume = self.current.map(|id| (id, self.position(), self.playing));
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.chain = None;
        self.out = None; // close the old stream before opening the new one
        match open_output() {
            Ok(o) => {
                self.device_changed = Some(o.name.clone());
                self.out = Some(o);
            }
            Err(e) => {
                self.error = Some(format!("No audio output: {e}"));
                return;
            }
        }
        if let Some((id, at, was_playing)) = resume {
            if self.start(lib, id, Some(at)).is_ok() && !was_playing {
                self.pause();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MH_GAPLESS_A=… MH_GAPLESS_B=… cargo test gapless -- --nocapture
    /// Two halves of one continuous sine must come out as one unbroken sine.
    #[test]
    fn gapless_handoff() {
        let (Some(a), Some(b)) = (std::env::var_os("MH_GAPLESS_A"), std::env::var_os("MH_GAPLESS_B")) else { return };
        let rate = 48_000;
        let shared = Arc::new(ChainShared::default());
        shared.gain.store(1f32.to_bits(), Ordering::Relaxed);
        let first = AudioSource::open_at(std::path::Path::new(&a), Some(rate)).unwrap();
        let second = AudioSource::open_at(std::path::Path::new(&b), Some(rate)).unwrap();
        *shared.next.lock().unwrap() = Some(Slot { src: second, id: 2, gain: 1.0 });
        let chain = Chain { cur: Some(Slot { src: first, id: 1, gain: 1.0 }), shared: shared.clone(), rate, odd: false };
        let out: Vec<f32> = chain.collect();
        let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let secs = left.len() as f64 / rate as f64;
        let ideal = 2.0 * std::f32::consts::PI * 440.0 / rate as f32 * 0.9;
        let clicks = left.windows(2).skip(2400).take(left.len().saturating_sub(4800)).filter(|w| (w[1] - w[0]).abs() > ideal * 1.2).count();
        // Longest run of near-silence (a gap would show up here).
        let mut run = 0;
        let mut longest = 0;
        for s in &left[2400..left.len() - 2400] {
            if s.abs() < 1e-3 {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        println!(
            "joined length {secs:.4}s (expected 4.0000), transitions {}, clicks {clicks}, longest silence {longest} samples",
            shared.transitions.load(Ordering::Relaxed)
        );
        assert!((secs - 4.0).abs() < 0.002);
        assert_eq!(clicks, 0);
        assert!(longest < 3);
    }
}
