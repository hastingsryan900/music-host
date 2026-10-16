//! One-click updates for the release builds: compare this build's commit with
//! the newest GitHub release, download it, swap the .exe (Windows) or the
//! .app (macOS) and restart. Local builds ("dev") never update.

/// The commit this binary was built from ("dev" for local builds).
pub const BUILD: &str = env!("MH_BUILD_SHA");

#[cfg(any(windows, target_os = "macos"))]
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
    cfg!(any(windows, target_os = "macos")) && BUILD != "dev"
}

/// The newest release's commit, if it differs from ours.
pub fn check() -> Option<String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if !supported() {
            return None;
        }
        let file = if cfg!(windows) { "version.txt" } else { "version-macos.txt" };
        let latest = ureq::get(&format!("{RELEASE}/{file}"))
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .ok()?
            .into_string()
            .ok()?;
        let latest = latest.trim().to_string();
        (!latest.is_empty() && latest != BUILD).then_some(latest)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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
    #[cfg(target_os = "macos")]
    {
        let app = app_bundle().ok_or("the app isn't running from Music Host.app")?;
        let work = crate::library::app_dir().join("update");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let zip = work.join("update.zip");
        let resp = ureq::get(&format!("{RELEASE}/MusicHost-macOS.zip"))
            .timeout(std::time::Duration::from_secs(300))
            .call()
            .map_err(|e| format!("download failed ({e})"))?;
        let mut f = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| format!("download failed ({e})"))?;
        drop(f);
        let unpacked = work.join("new");
        let ok = std::process::Command::new("ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&unpacked)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let new_app = unpacked.join("Music Host.app");
        if !ok || !new_app.join("Contents/MacOS/Music Host").exists() {
            return Err("couldn't unpack the update".into());
        }
        // Put the new bundle next to the running one first (same folder, so
        // the final swap is two renames), then trade places. A running app
        // can be moved; it keeps working until it quits.
        let staged = app.with_extension("new.app");
        let old = app.with_extension("old.app");
        let _ = std::fs::remove_dir_all(&staged);
        let _ = std::fs::remove_dir_all(&old);
        let ok = std::process::Command::new("ditto")
            .arg(&new_app)
            .arg(&staged)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&staged);
            return Err("couldn't replace the app (is it in a folder you can write to?)".into());
        }
        std::fs::rename(&app, &old).map_err(|e| format!("couldn't replace the app ({e})"))?;
        if let Err(e) = std::fs::rename(&staged, &app) {
            let _ = std::fs::rename(&old, &app); // put the working one back
            return Err(format!("couldn't replace the app ({e})"));
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&work);
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err("updates aren't built for this system".into())
}

/// The running .app bundle (…/Music Host.app), when there is one.
#[cfg(target_os = "macos")]
fn app_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?.to_path_buf();
    (app.extension()? == "app").then_some(app)
}

/// Start the (new) app; the caller then closes this one.
pub fn relaunch() {
    // `open -n` starts a second copy even though this one is still running.
    #[cfg(target_os = "macos")]
    if let Some(app) = app_bundle() {
        let _ = std::process::Command::new("open").arg("-n").arg(app).spawn();
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// Remove the previous version left behind by an update.
pub fn cleanup() {
    #[cfg(target_os = "macos")]
    if let Some(app) = app_bundle() {
        let _ = std::fs::remove_dir_all(app.with_extension("old.app"));
        let _ = std::fs::remove_dir_all(app.with_extension("new.app"));
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
    }
}
