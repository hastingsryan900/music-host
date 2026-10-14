//! One-click updates for the Windows build: compare this build's commit with
//! the newest GitHub release, download it, swap the .exe and restart.
//! (The macOS app is built locally with bundle.sh, so this is Windows-only.)

/// The commit this binary was built from ("dev" for local builds).
pub const BUILD: &str = env!("MH_BUILD_SHA");

#[cfg(windows)]
const RELEASE: &str = "https://github.com/hastingsryan900/music-host/releases/latest/download";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    /// A newer build is out (its commit).
    Available(String),
    Installing,
    /// Downloaded and swapped in: restarting.
    Ready,
    Failed(String),
}

/// Messages from the background update thread.
pub enum Msg {
    Available(String),
    Installed,
    Failed(String),
}

/// Whether this build can update itself.
pub fn supported() -> bool {
    cfg!(windows) && BUILD != "dev"
}

/// The newest release's commit, if it differs from ours.
pub fn check() -> Option<String> {
    #[cfg(windows)]
    {
        if !supported() {
            return None;
        }
        let latest = ureq::get(&format!("{RELEASE}/version.txt"))
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .ok()?
            .into_string()
            .ok()?;
        let latest = latest.trim().to_string();
        (!latest.is_empty() && latest != BUILD).then_some(latest)
    }
    #[cfg(not(windows))]
    None
}

/// Download the latest zip and put its .exe in place of ours. The running
/// .exe is renamed out of the way first (Windows allows renaming, not
/// overwriting, a running program); it's deleted on the next start.
pub fn install() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::path::PathBuf;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe.parent().ok_or("can't find the app folder")?.to_path_buf();
        let work = crate::library::app_dir().join("update");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let zip = work.join("update.zip");
        let resp = ureq::get(&format!("{RELEASE}/MusicHost-Windows.zip"))
            .timeout(std::time::Duration::from_secs(300))
            .call()
            .map_err(|e| format!("download failed ({e})"))?;
        let mut f = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| format!("download failed ({e})"))?;
        drop(f);
        let ok = crate::ytdl::quiet(std::process::Command::new("tar"))
            .arg("-xf")
            .arg(&zip)
            .arg("-C")
            .arg(&work)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err("couldn't unpack the update".into());
        }
        let new_exe: PathBuf = std::fs::read_dir(&work)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().map(|x| x.eq_ignore_ascii_case("exe")).unwrap_or(false))
            .ok_or("the update had no app in it")?;
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(|e| format!("couldn't replace the app ({e})"))?;
        if let Err(e) = std::fs::copy(&new_exe, &exe) {
            let _ = std::fs::rename(&old, &exe); // put the working one back
            return Err(format!("couldn't replace the app ({e})"));
        }
        if let Ok(readme) = std::fs::read(work.join("README.md")) {
            let _ = std::fs::write(dir.join("README.md"), readme);
        }
        let _ = std::fs::remove_dir_all(&work);
        Ok(())
    }
    #[cfg(not(windows))]
    Err("updates are only built for Windows".into())
}

/// Start the (new) app; the caller then closes this one.
pub fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// Remove the previous version left behind by an update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
    }
}
