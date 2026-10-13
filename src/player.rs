//! Audio playback + play queue (shuffle, repeat, up next).

use crate::library::{Library, TrackId};
use rand::seq::SliceRandom;
use crate::decode::AudioSource;
use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Off,
    /// Loop the current song.
    One,
}

pub struct Player {
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
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
    /// The output device's sample rate; songs are converted to it with a
    /// high-quality resampler before reaching rodio.
    device_rate: Option<u32>,
    pub error: Option<String>,
    /// Set when a new track starts, so the app can bump play counts / Now Playing.
    pub just_started: Option<TrackId>,
}

impl Player {
    pub fn new(volume: f32) -> Self {
        let (stream, handle, error) = match OutputStream::try_default() {
            Ok((s, h)) => (Some(s), Some(h), None),
            Err(e) => (None, None, Some(format!("No audio output: {e}"))),
        };
        let device_rate = {
            use rodio::cpal::traits::{DeviceTrait, HostTrait};
            rodio::cpal::default_host()
                .default_output_device()
                .and_then(|d| d.default_output_config().ok())
                .map(|c| c.sample_rate().0)
        };
        Player {
            _stream: stream,
            handle,
            sink: None,
            source: vec![],
            order: vec![],
            pos: 0,
            current: None,
            playing: false,
            shuffle: false,
            repeat: Repeat::Off,
            volume,
            device_rate,
            error,
            just_started: None,
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
        self.sink
            .as_ref()
            .map(|s| s.get_pos().as_secs_f64())
            .unwrap_or(0.0)
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
    }

    /// Loop the current song on/off.
    pub fn toggle_loop(&mut self) {
        self.repeat = if self.repeat == Repeat::One { Repeat::Off } else { Repeat::One };
    }

    /// Tracks that will play after the current one.
    pub fn up_next(&self) -> Vec<TrackId> {
        if self.order.is_empty() {
            return vec![];
        }
        self.order[self.pos + 1..]
            .iter()
            .map(|&i| self.source[i])
            .collect()
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
            if self.shuffle {
                let first = self.order[0];
                self.reshuffle(first);
            } else {
                self.pos = 0;
            }
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
        self.playing = false;
        self.current = None;
    }

    fn load_current(&mut self, lib: &Library) {
        // Try the current item, skipping over unplayable files.
        for _ in 0..self.order.len().max(1) {
            let Some(&i) = self.order.get(self.pos) else { return };
            let id = self.source[i];
            match self.open(lib, id) {
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

    fn open(&mut self, lib: &Library, id: TrackId) -> Result<(), String> {
        let handle = self.handle.as_ref().ok_or("no audio device")?;
        let track = lib.track(id).ok_or("track missing from library")?;
        let decoder = AudioSource::open_at(&track.path, self.device_rate)?;
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;
        sink.set_volume(self.volume * self.volume);
        sink.append(decoder);
        sink.play();
        self.sink = Some(sink);
        self.current = Some(id);
        self.playing = true;
        self.just_started = Some(id);
        Ok(())
    }

    /// Call every frame: advances to the next track when one finishes.
    pub fn tick(&mut self, lib: &Library) {
        let finished = self.playing && self.sink.as_ref().map(|s| s.empty()).unwrap_or(false);
        if finished {
            if self.repeat == Repeat::One {
                self.load_current(lib);
            } else {
                self.next(lib);
            }
        }
    }
}
