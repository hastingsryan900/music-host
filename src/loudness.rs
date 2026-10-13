//! Loudness measurement (EBU R128 integrated loudness + sample peak) for
//! volume levelling. Runs once per song on a background thread; results are
//! stored in the library.

use crate::decode::Decoding;
use std::path::Path;

/// (integrated loudness in LUFS, sample peak 0..1+) for a file.
pub fn analyze(path: &Path) -> Option<(f32, f32)> {
    let (mut dec, _) = Decoding::open(path).ok()?;
    let ch = dec.spec.channels.count().max(1);
    let mut meter = ebur128::EbuR128::new(ch as u32, dec.spec.rate, ebur128::Mode::I | ebur128::Mode::SAMPLE_PEAK).ok()?;
    let mut any = false;
    while let Some(samples) = dec.next_packet() {
        // Packets keep the file's channel count (see Decoding).
        if samples.len() % ch == 0 && meter.add_frames_f32(&samples).is_ok() {
            any = true;
        }
    }
    if !any {
        return None;
    }
    let lufs = meter.loudness_global().ok()?;
    if !lufs.is_finite() {
        return None; // silence
    }
    let peak = (0..ch as u32).filter_map(|c| meter.sample_peak(c).ok()).fold(0.0f64, f64::max);
    Some((lufs as f32, peak as f32))
}

#[cfg(test)]
mod tests {
    /// MH_TONE=/path/sine440.flac cargo test loudness -- --nocapture
    #[test]
    fn loudness_of_test_tone() {
        let Some(p) = std::env::var_os("MH_TONE") else { return };
        let (lufs, peak) = super::analyze(std::path::Path::new(&p)).unwrap();
        println!("0.9-amplitude 440 Hz sine: {lufs:.1} LUFS, peak {peak:.3}");
        // EBU R128 sums both channels: a stereo 0.9-amplitude sine at 440 Hz
        // reads ≈ -1.6 LUFS (-0.9 dB level, -0.7 dB K-weighting at 440 Hz).
        assert!((lufs + 1.6).abs() < 0.5, "{lufs}");
        assert!((peak - 0.9).abs() < 0.01);
    }
}
