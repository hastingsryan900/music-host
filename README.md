# Music Host

A native macOS music player for your own library, written in Rust (egui + Symphonia). It works like Apple Music, in dark mode, and the accent color follows whatever album is playing.

## Install / update

```bash
./bundle.sh
```

This builds a release binary and installs **Music Host.app** to `~/Applications`.

## Using it

- **Music folder:** `~/Music/Music Host`. To add more music, drop album folders in there, click ⚙ → Add Folder, or drag folders or files onto the window. Folders are rescanned at every launch.
- **HD covers:** if the art inside your files is under 1000px, the app looks up the album on Apple's iTunes catalog (it sends only the artist and album name) and saves a 1500px cover. Albums with no match keep their own art.
- **Formats:** MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF, OGG Vorbis, Opus. Opus needs `brew install opus` at build time; it's linked statically.
- **Shortcuts:** Space play/pause · ←/→ seek ±10s · ⌘←/⌘→ previous/next · ⌘↑/⌘↓ volume · ⌘P Now Playing · ⌘F search · ⌘L go to the current album · ⌘[ back. Media keys, AirPods and Control Center work too.
- **Right-click** any song or album for Play Next, Add to Queue, Add to Playlist, Love, and Show in Finder.

The library and artwork cache are stored in `~/Library/Application Support/Music Host/`. Your music files are only read, never changed.

## Dev

```bash
cargo run
MH_TEST_MUSIC=~/Music/Music\ Host cargo test -- --nocapture   # decode + seek every file
cargo run --release --example make_icon                       # regenerate assets/icon.png
MH_FPS=1 cargo run --release                                  # print frame rate + slow frames each second
```

## Credits

- The Now Playing background is a Rust/OpenGL port of [linkev/PlayStation-3-XMB](https://github.com/linkev/PlayStation-3-XMB) (MIT License, © 2025 Mart): its reverse-engineered PS3 spline wave, shaders and particles. The gradient colors follow the current album cover instead of the PS3's monthly themes.
- Audio: [Symphonia](https://github.com/pdeljanov/Symphonia) for decoding, libopus for Opus, and [rubato](https://github.com/HEnquist/rubato) for sample-rate conversion.
