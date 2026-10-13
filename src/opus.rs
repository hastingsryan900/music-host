//! Minimal libopus decoder binding for Ogg/Opus (Symphonia has no Opus codec).

use std::os::raw::{c_int, c_uchar};

#[repr(C)]
struct OpusDecoder {
    _private: [u8; 0],
}

#[cfg(has_opus)]
extern "C" {
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decode_float(st: *mut OpusDecoder, data: *const c_uchar, len: i32, pcm: *mut f32, frame_size: c_int, fec: c_int) -> c_int;
    fn opus_decoder_ctl(st: *mut OpusDecoder, request: c_int, ...) -> c_int;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
}

const OPUS_RESET_STATE: c_int = 4028;
/// 120 ms at 48 kHz: the largest possible Opus frame.
const MAX_FRAME: usize = 5760;

pub struct Opus {
    st: *mut OpusDecoder,
    channels: usize,
    buf: Vec<f32>,
}

// The decoder is only ever used from one thread at a time.
unsafe impl Send for Opus {}

impl Opus {
    #[cfg(has_opus)]
    pub fn new(channels: usize) -> Result<Self, String> {
        if !(1..=2).contains(&channels) {
            return Err(format!("{channels}-channel Opus isn't supported"));
        }
        let mut err = 0;
        let st = unsafe { opus_decoder_create(48_000, channels as c_int, &mut err) };
        if st.is_null() || err != 0 {
            return Err(format!("opus init failed ({err})"));
        }
        Ok(Opus { st, channels, buf: vec![0.0; MAX_FRAME * channels] })
    }

    #[cfg(not(has_opus))]
    pub fn new(_channels: usize) -> Result<Self, String> {
        Err("Opus support not built in".into())
    }

    /// Decode one packet; returns interleaved samples.
    #[cfg(has_opus)]
    pub fn decode(&mut self, packet: &[u8]) -> Result<&[f32], String> {
        let n = unsafe {
            opus_decode_float(self.st, packet.as_ptr(), packet.len() as i32, self.buf.as_mut_ptr(), MAX_FRAME as c_int, 0)
        };
        if n < 0 {
            return Err(format!("opus decode error {n}"));
        }
        Ok(&self.buf[..n as usize * self.channels])
    }

    #[cfg(not(has_opus))]
    pub fn decode(&mut self, _packet: &[u8]) -> Result<&[f32], String> {
        Err("Opus support not built in".into())
    }

    pub fn reset(&mut self) {
        #[cfg(has_opus)]
        unsafe {
            opus_decoder_ctl(self.st, OPUS_RESET_STATE);
        }
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        #[cfg(has_opus)]
        unsafe {
            opus_decoder_destroy(self.st)
        }
    }
}
