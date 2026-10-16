# Music Host

A native music player for your own library, for macOS and Windows, written in Rust (egui + Symphonia). It works like Apple Music, in dark mode, and the accent color follows whatever album is playing.

## Windows

1. Download **MusicHost-Windows.zip** from the [latest release](https://github.com/hastingsryan900/music-host/releases/latest).
2. Unzip it and run **Music Host.exe**.
3. If Windows shows "Windows protected your PC", click **More info → Run anyway**. The app isn't code-signed, so Windows warns about it.
4. Click ⚙ → **Add Folder** and pick the folder with your music, or drag folders onto the window.

The first time you use **Add from Link**, the app downloads yt-dlp and deno (about 60 MB) into its data folder. Windows keeps the library in `%APPDATA%\Music Host\`. The macOS menu-bar item isn't available on Windows.

## Mac (Apple Silicon)

1. Download **MusicHost-macOS.zip** from the [latest release](https://github.com/hastingsryan900/music-host/releases/latest).
2. Open the zip and drag **Music Host** into **Applications**.
3. Open it. The first time, macOS says it can't check the app because it isn't from the App Store. Click **Done**, go to **System Settings → Privacy & Security**, scroll down, and click **Open Anyway**. You only do this once.
4. Click ⚙ → **Add Folder** and pick the folder with your music.

When a new version is out, **Update available** shows in the sidebar. Click it and the app updates itself and restarts.

## Publishing an update

Push to `main`. GitHub Actions builds the Mac and Windows apps (about 10 minutes) and replaces the files on the `latest` release, and every installed copy then offers the update.

## Building it yourself (macOS)

```bash
./bundle.sh
```

This builds a release binary and installs **Music Host.app** to `~/Applications`. Builds you make yourself don't update themselves; only the ones from GitHub do.

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
