//! macOS Now Playing + media keys (F7/F8/F9, AirPods, Control Center).

use crate::library::Track;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

pub enum Cmd {
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
    Seek(f64),
    SeekBy(f64),
}

pub struct Media {
    controls: MediaControls,
    rx: Receiver<MediaControlEvent>,
}

impl Media {
    /// `hwnd` is the main window's handle; Windows needs it, macOS doesn't.
    pub fn new(hwnd: Option<*mut std::ffi::c_void>) -> Option<Self> {
        if cfg!(windows) && hwnd.is_none() {
            return None;
        }
        let config = PlatformConfig { dbus_name: "music_host", display_name: "Music Host", hwnd };
        // Media keys are a nice-to-have: never let them stop the app opening.
        let mut controls = std::panic::catch_unwind(move || MediaControls::new(config)).ok()?.ok()?;
        let (tx, rx) = channel();
        controls
            .attach(move |e| {
                let _ = tx.send(e);
            })
            .ok()?;
        Some(Media { controls, rx })
    }

    pub fn poll(&mut self) -> Option<Cmd> {
        let e = self.rx.try_recv().ok()?;
        Some(match e {
            MediaControlEvent::Play => Cmd::Play,
            MediaControlEvent::Pause | MediaControlEvent::Stop => Cmd::Pause,
            MediaControlEvent::Toggle => Cmd::Toggle,
            MediaControlEvent::Next => Cmd::Next,
            MediaControlEvent::Previous => Cmd::Prev,
            MediaControlEvent::SetPosition(MediaPosition(d)) => Cmd::Seek(d.as_secs_f64()),
            MediaControlEvent::Seek(SeekDirection::Forward) => Cmd::SeekBy(10.0),
            MediaControlEvent::Seek(SeekDirection::Backward) => Cmd::SeekBy(-10.0),
            MediaControlEvent::SeekBy(SeekDirection::Forward, d) => Cmd::SeekBy(d.as_secs_f64()),
            MediaControlEvent::SeekBy(SeekDirection::Backward, d) => Cmd::SeekBy(-d.as_secs_f64()),
            _ => return None,
        })
    }

    pub fn update(&mut self, track: Option<&Track>, cover: Option<String>, playing: bool, pos: f64) {
        match track {
            Some(t) => {
                let _ = self.controls.set_metadata(MediaMetadata {
                    title: Some(&t.title),
                    artist: Some(&t.artist),
                    album: Some(&t.album),
                    cover_url: cover.as_deref(),
                    duration: Some(Duration::from_secs_f64(t.duration_secs)),
                });
                let progress = Some(MediaPosition(Duration::from_secs_f64(pos)));
                let _ = self.controls.set_playback(if playing {
                    MediaPlayback::Playing { progress }
                } else {
                    MediaPlayback::Paused { progress }
                });
            }
            None => {
                let _ = self.controls.set_playback(MediaPlayback::Stopped);
            }
        }
    }
}
