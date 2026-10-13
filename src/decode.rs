//! Audio decoding.
//!
//! - Symphonia demuxes/decodes (libopus for Opus); errors never panic.
//! - Each song gets a decoder thread that keeps ~1.5 s of audio buffered, so
//!   the real-time audio callback never touches the disk or the decoder.
//! - Sample-rate conversion (e.g. 44.1 kHz files → 48 kHz MacBook speakers)
//!   happens here with a high-quality FFT resampler (rubato). rodio's built-in
//!   converter is linear interpolation: audible aliasing on cymbals and dulled
//!   highs, and it clicked whenever it was rebuilt mid-song.

use rodio::source::SeekError;
use rodio::Source;
use rubato::{FftFixedInOut, Resampler};
use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL, CODEC_TYPE_OPUS};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Seconds of audio the decoder thread keeps ready ahead of playback.
const BUFFER_SECS: f32 = 1.5;

// ---------------------------------------------------------------- decoding

enum Dec {
    Sym(Box<dyn Decoder>),
    /// Symphonia can demux Ogg/Opus but not decode it; libopus does that.
    Opus(crate::opus::Opus),
}

impl Dec {
    fn reset(&mut self) {
        match self {
            Dec::Sym(d) => d.reset(),
            Dec::Opus(o) => o.reset(),
        }
    }
}

/// Packet-level decoder at the file's native rate (runs on its own thread).
pub(crate) struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Dec,
    track_id: u32,
    pub(crate) spec: SignalSpec,
    /// Frames to drop after an accurate seek lands slightly early.
    skip_frames: u64,
}

impl Decoding {
    pub(crate) fn open(path: &Path) -> Result<(Self, Option<Duration>), String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|e| format!("unsupported format ({e})"))?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("no audio track")?;
        let params = track.codec_params.clone();
        let decoder = if params.codec == CODEC_TYPE_OPUS {
            let ch = params.channels.map(|c| c.count()).unwrap_or(2);
            Dec::Opus(crate::opus::Opus::new(ch)?)
        } else {
            Dec::Sym(
                symphonia::default::get_codecs()
                    .make(&params, &DecoderOptions::default())
                    .map_err(|e| format!("unsupported codec ({e})"))?,
            )
        };
        let duration = match (params.time_base, params.n_frames) {
            (Some(tb), Some(n)) => {
                let t = tb.calc_time(n);
                Some(Duration::from_secs_f64(t.seconds as f64 + t.frac))
            }
            _ => None,
        };
        let spec = SignalSpec::new(
            params.sample_rate.unwrap_or(44_100),
            params
                .channels
                .unwrap_or(symphonia::core::audio::Channels::FRONT_LEFT | symphonia::core::audio::Channels::FRONT_RIGHT),
        );
        Ok((Decoding { track_id: track.id, format, decoder, spec, skip_frames: 0 }, duration))
    }

    /// Decode the next packet as interleaved samples. `None` at end of stream.
    pub(crate) fn next_packet(&mut self) -> Option<Vec<f32>> {
        let mut errors = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(Error::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(_) => return None, // EOF or unrecoverable
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded: Result<(Vec<f32>, SignalSpec), Error> = match &mut self.decoder {
                Dec::Sym(d) => d.decode(&packet).map(|decoded| {
                    let spec = *decoded.spec();
                    let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    sb.copy_interleaved_ref(decoded);
                    (sb.samples().to_vec(), spec)
                }),
                Dec::Opus(o) => match o.decode(&packet.data) {
                    Ok(pcm) => {
                        let spec = self.spec;
                        let ch = spec.channels.count().max(1);
                        // Apply gapless trimming (Opus pre-skip / end padding).
                        let frames = pcm.len() / ch;
                        let start = (packet.trim_start as usize).min(frames);
                        let end = frames.saturating_sub(packet.trim_end as usize).max(start);
                        Ok((pcm[start * ch..end * ch].to_vec(), spec))
                    }
                    Err(_) => Err(Error::DecodeError("opus: corrupt packet")),
                },
            };
            match decoded {
                Ok((mut samples, spec)) => {
                    let ch = spec.channels.count().max(1);
                    let frames = samples.len() / ch;
                    if frames == 0 {
                        continue;
                    }
                    if self.skip_frames > 0 {
                        let drop = (self.skip_frames as usize).min(frames);
                        self.skip_frames -= drop as u64;
                        samples.drain(..(drop * ch).min(samples.len()));
                        if samples.is_empty() {
                            continue;
                        }
                    }
                    if spec.channels.count() == self.spec.channels.count() {
                        self.spec = spec;
                    }
                    return Some(samples);
                }
                Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => {
                    // Corrupt packet: skip it, but don't spin forever.
                    errors += 1;
                    if errors > 50 {
                        return None;
                    }
                }
                Err(_) => return None,
            }
        }
    }

    fn seek(&mut self, secs: f64) -> bool {
        let to = SeekTo::Time { time: Time::new(secs.trunc() as u64, secs.fract()), track_id: Some(self.track_id) };
        match self.format.seek(SeekMode::Accurate, to) {
            Ok(seeked) => {
                self.decoder.reset();
                self.skip_frames = seeked.required_ts.saturating_sub(seeked.actual_ts);
                true
            }
            Err(_) => false,
        }
    }
}

/// Interleaved `ch`-channel samples → interleaved stereo.
fn to_stereo(samples: Vec<f32>, ch: usize) -> Vec<f32> {
    match ch {
        2 => samples,
        1 => samples.iter().flat_map(|&s| [s, s]).collect(),
        _ => samples.chunks_exact(ch).flat_map(|f| [f[0], f[1]]).collect(),
    }
}

// ---------------------------------------------------------------- resampling

/// Streaming wrapper around rubato's fixed-size FFT resampler.
struct Resample {
    r: FftFixedInOut<f32>,
    channels: usize,
    fifo: Vec<Vec<f32>>,
    /// Output frames still to discard (the filter's latency).
    delay: usize,
}

impl Resample {
    fn new(from: u32, to: u32, channels: usize) -> Option<Self> {
        let r = FftFixedInOut::<f32>::new(from as usize, to as usize, 1024, channels).ok()?;
        let delay = r.output_delay();
        Some(Resample { r, channels, fifo: vec![vec![]; channels], delay })
    }

    fn reset(&mut self) {
        self.r.reset();
        for c in &mut self.fifo {
            c.clear();
        }
        self.delay = self.r.output_delay();
    }

    fn emit(&mut self, planar: Vec<Vec<f32>>, out: &mut Vec<f32>) {
        let frames = planar.first().map(|c| c.len()).unwrap_or(0);
        let skip = self.delay.min(frames);
        self.delay -= skip;
        for i in skip..frames {
            for c in &planar {
                out.push(c[i]);
            }
        }
    }

    /// Feed interleaved input; append whatever output is ready.
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for frame in input.chunks_exact(self.channels) {
            for (c, &v) in frame.iter().enumerate() {
                self.fifo[c].push(v);
            }
        }
        loop {
            let need = self.r.input_frames_next();
            if self.fifo[0].len() < need {
                break;
            }
            let chunk: Vec<Vec<f32>> = self.fifo.iter_mut().map(|c| c.drain(..need).collect()).collect();
            match self.r.process(&chunk, None) {
                Ok(planar) => self.emit(planar, out),
                Err(_) => break,
            }
        }
    }

    /// End of stream: push out the remainder and the filter tail — exactly
    /// the output that corresponds to real input (the resampler lags by
    /// `output_delay` frames), nothing from the zero padding.
    fn flush(&mut self, out: &mut Vec<f32>) {
        let rest: Vec<Vec<f32>> = self.fifo.iter_mut().map(std::mem::take).collect();
        let real = rest[0].len();
        let needed = (real as f64 * self.ratio()).round() as usize + self.r.output_delay();
        let mut got = 0;
        let mut first = true;
        while got < needed {
            let res = if first { self.r.process_partial(Some(&rest), None) } else { self.r.process_partial::<Vec<f32>>(None, None) };
            first = false;
            let Ok(planar) = res else { break };
            let len = planar.first().map(|c| c.len()).unwrap_or(0);
            if len == 0 {
                break;
            }
            let take = (needed - got).min(len);
            got += take;
            let trimmed = planar
                .into_iter()
                .map(|mut c| {
                    c.truncate(take);
                    c
                })
                .collect();
            self.emit(trimmed, out);
        }
    }

    fn ratio(&self) -> f64 {
        self.r.output_frames_next() as f64 / self.r.input_frames_next() as f64
    }
}

// ---------------------------------------------------------------- threaded source

#[derive(Default)]
struct Shared {
    queue: VecDeque<f32>,
    /// Decoder reached the end of the song.
    eof: bool,
    /// Pending seek target (seconds) for the decoder thread.
    seek: Option<f64>,
    /// Increments after every completed seek.
    generation: u64,
    stop: bool,
}

type Link = Arc<(Mutex<Shared>, Condvar)>;

/// The rodio source: a thin consumer of the decoder thread's buffer.
pub struct AudioSource {
    link: Link,
    local: Vec<f32>,
    pos: usize,
    channels: u16,
    rate: u32,
    duration: Option<Duration>,
    /// Generation we're waiting for after a seek.
    want_gen: u64,
}

impl AudioSource {
    /// Open at the file's own sample rate.
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_at(path, None)
    }

    /// Open, converting to `out_rate` (the output device's rate) if needed.
    pub fn open_at(path: &Path, out_rate: Option<u32>) -> Result<Self, String> {
        let (mut dec, duration) = Decoding::open(path)?;
        // Decode the first packet now so broken files fail here, not later.
        let first = dec.next_packet().ok_or("file contains no playable audio")?;
        // Everything leaves here as stereo, so songs can follow one another
        // in a single gapless stream.
        let channels = 2usize;
        let first = to_stereo(first, dec.spec.channels.count().max(1));
        let in_rate = dec.spec.rate;
        let mut resample = match out_rate {
            Some(r) if r != in_rate => Resample::new(in_rate, r, channels),
            _ => None,
        };
        let rate = if resample.is_some() { out_rate.unwrap() } else { in_rate };

        let link: Link = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let mut first_out = vec![];
        match &mut resample {
            Some(r) => r.push(&first, &mut first_out),
            None => first_out = first,
        }
        link.0.lock().unwrap().queue.extend(first_out);

        let target = (rate as f32 * channels as f32 * BUFFER_SECS) as usize;
        let thread_link = link.clone();
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decoder_thread(dec, resample, thread_link, target))
            .map_err(|e| e.to_string())?;

        Ok(AudioSource { link, local: Vec::with_capacity(4096), pos: 0, channels: channels as u16, rate, duration, want_gen: 0 })
    }

    /// Pull the next block from the shared buffer. Returns false at the end.
    fn fill(&mut self) -> bool {
        let (lock, cv) = &*self.link;
        let mut s = lock.lock().unwrap();
        // Normally data is already there; this only waits on underrun or
        // right after a seek (bounded, so the audio thread never hangs).
        let mut waits = 0;
        while (s.generation < self.want_gen || s.queue.is_empty()) && !(s.eof && s.generation >= self.want_gen) {
            if waits > 25 {
                // Still nothing: play a short silence rather than stall.
                self.local.clear();
                self.local.resize(256 * self.channels as usize, 0.0);
                self.pos = 0;
                return true;
            }
            s = cv.wait_timeout(s, Duration::from_millis(2)).unwrap().0;
            waits += 1;
        }
        if s.queue.is_empty() {
            return false;
        }
        let n = s.queue.len().min(4096);
        self.local.clear();
        self.local.extend(s.queue.drain(..n));
        self.pos = 0;
        drop(s);
        cv.notify_all(); // room for the decoder to refill
        true
    }
}

fn decoder_thread(mut dec: Decoding, mut resample: Option<Resample>, link: Link, target: usize) {
    let (lock, cv) = &*link;
    let mut done = false;
    loop {
        // Wait until there's a seek, room in the buffer, or we're stopped.
        let seek = {
            let mut s = lock.lock().unwrap();
            loop {
                if s.stop {
                    return;
                }
                if s.seek.is_some() || (!done && s.queue.len() < target) {
                    break;
                }
                s = cv.wait(s).unwrap();
            }
            s.seek.take()
        };
        if let Some(secs) = seek {
            dec.seek(secs);
            if let Some(r) = &mut resample {
                r.reset();
            }
            done = false;
            let mut s = lock.lock().unwrap();
            s.queue.clear();
            s.eof = false;
            s.generation += 1;
            cv.notify_all();
            continue;
        }
        // Decode a little batch outside the lock.
        let mut out = Vec::with_capacity(8192);
        while out.len() < 8192 {
            match dec.next_packet() {
                Some(samples) => {
                    let samples = to_stereo(samples, dec.spec.channels.count().max(1));
                    match &mut resample {
                        Some(r) => r.push(&samples, &mut out),
                        None => out.extend_from_slice(&samples),
                    }
                }
                None => {
                    if let Some(r) = &mut resample {
                        r.flush(&mut out);
                    }
                    done = true;
                    break;
                }
            }
        }
        let mut s = lock.lock().unwrap();
        if s.seek.is_some() {
            continue; // stale audio from before the seek
        }
        s.queue.extend(out);
        if done {
            s.eof = true;
        }
        cv.notify_all();
    }
}

impl Drop for AudioSource {
    fn drop(&mut self) {
        let (lock, cv) = &*self.link;
        if let Ok(mut s) = lock.lock() {
            s.stop = true;
        }
        cv.notify_all();
    }
}

impl Iterator for AudioSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.local.len() && !self.fill() {
            return None;
        }
        let s = self.local[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for AudioSource {
    fn current_frame_len(&self) -> Option<usize> {
        // The format is fixed for the whole song. Reporting per-packet frames
        // here made rodio rebuild its sample-rate converter at every packet,
        // which clicked ~10×/s whenever the device rate differed (44.1→48k).
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let (lock, cv) = &*self.link;
        let mut s = lock.lock().unwrap();
        s.seek = Some(pos.as_secs_f64());
        self.want_gen = s.generation + 1;
        drop(s);
        cv.notify_all();
        self.local.clear();
        self.pos = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MH_TEST_MUSIC=/path/to/folder cargo test -- --nocapture
    #[test]
    fn decodes_and_seeks_every_file() {
        let Some(dir) = std::env::var_os("MH_TEST_MUSIC") else { return };
        let mut n = 0;
        for e in walkdir::WalkDir::new(dir).into_iter().flatten() {
            if !crate::library::is_audio(e.path()) {
                continue;
            }
            // Exercise the real playback path: resampled to 48 kHz.
            let mut src = AudioSource::open_at(e.path(), Some(48_000)).unwrap_or_else(|err| panic!("{:?}: {err}", e.path()));
            let (ch, rate) = (src.channels() as usize, src.sample_rate() as usize);
            let total = src.by_ref().take(rate * ch).count();
            assert_eq!(total, rate * ch, "{:?} first second", e.path());
            src.try_seek(Duration::from_secs(10)).expect("seek");
            let rest = src.count() as f64 / (rate * ch) as f64;
            let meta = crate::library::read_tags(e.path()).unwrap();
            let dur = lofty::file::AudioFile::properties(&meta).duration().as_secs_f64();
            println!("{:<50} {ch}ch {rate}Hz dur={dur:.1}s after-seek-remaining={rest:.2}s", e.file_name().to_string_lossy());
            assert!((rest - (dur - 10.0)).abs() < 0.2, "seek landed wrong: {rest} vs {}", dur - 10.0);
            n += 1;
        }
        assert!(n > 0);
    }
}

#[cfg(test)]
pub(crate) mod quality {
    /// Magnitude of `freq` in `x` (Goertzel), normalised to sine amplitude.
    pub fn tone_level(x: &[f32], rate: f32, freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / rate;
        let c = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + c * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let p = s1 * s1 + s2 * s2 - c * s1 * s2;
        2.0 * p.max(0.0).sqrt() / x.len() as f32
    }

    pub fn db(r: f32) -> f32 {
        20.0 * r.max(1e-12).log10()
    }
}

#[cfg(test)]
mod audio_quality_tests {
    use super::*;

    /// What actually reaches the speakers: our source, then rodio's own
    /// format conversion to `device` rate (a no-op when we already match).
    fn render(path: &Path, device: u32, ours: bool) -> (Vec<f32>, usize) {
        let src = AudioSource::open_at(path, if ours { Some(device) } else { None }).unwrap();
        let ch = src.channels() as usize;
        let mut out = vec![];
        for x in rodio::source::UniformSourceIterator::<_, f32>::new(src, ch as u16, device) {
            out.push(x);
        }
        (out, ch)
    }

    /// MH_TONE=/path/sine440.flac MH_TONE15=/path/sine15k.flac \
    ///   cargo test --release audio_quality -- --nocapture
    #[test]
    fn audio_quality() {
        if let Some(p) = std::env::var_os("MH_TONE") {
            for (label, ours) in [("rodio linear", false), ("rubato FFT ", true)] {
                let (out, ch) = render(Path::new(&p), 48_000, ours);
                let left: Vec<f32> = out.chunks_exact(ch).map(|f| f[0]).collect();
                let ideal = 2.0 * std::f32::consts::PI * 440.0 / 48_000.0 * 0.9;
                let bad: Vec<(usize, f32)> = left
                    .windows(2)
                    .enumerate()
                    .skip(4800)
                    .filter(|(_, w)| (w[1] - w[0]).abs() > ideal * 1.2)
                    .map(|(i, w)| (i, w[1] - w[0]))
                    .collect();
                if !bad.is_empty() {
                    println!("  jumps at {:?} of {} (last samples {:?})", &bad[..bad.len().min(5)], left.len(), &left[left.len().saturating_sub(4)..]);
                }
                let clicks = bad.len();
                println!("{label}: 440 Hz @48k, {:.2}s out, clicks {clicks}", left.len() as f64 / 48_000.0);
                assert_eq!(clicks, 0);
            }
        }
        if let Some(p) = std::env::var_os("MH_TONE15") {
            for (label, ours) in [("rodio linear", false), ("rubato FFT ", true)] {
                let (out, ch) = render(Path::new(&p), 48_000, ours);
                let left: Vec<f32> = out.chunks_exact(ch).map(|f| f[0]).skip(4800).take(48_000).collect();
                let main = quality::tone_level(&left, 48_000.0, 15_000.0);
                let ghost = quality::tone_level(&left, 48_000.0, 18_900.0);
                println!(
                    "{label}: 15 kHz tone {:.1} dB (ideal -6.0), ghost at 18.9 kHz {:.1} dB below it",
                    quality::db(main),
                    -quality::db(ghost / main)
                );
            }
        }
    }
}
