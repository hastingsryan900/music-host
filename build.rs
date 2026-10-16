// Statically link libopus (from Homebrew) when available so Ogg/Opus files
// play. The binary stays self-contained either way.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_opus)");
    // Which commit this is (set by GitHub Actions), for the updater.
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rustc-env=MH_BUILD_SHA={}", std::env::var("GITHUB_SHA").unwrap_or_else(|_| "dev".into()));
    // Windows: put the icon inside the .exe so Explorer and pinned taskbar
    // shortcuts show it (the window icon alone only lasts while it runs).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Music Host");
        res.set("FileDescription", "Music Host");
        res.compile().expect("embed Windows icon");
    }
    // On Windows, audiopus_sys builds and links libopus.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cfg=has_opus");
        // Windows gives the main thread only 1 MB of stack (macOS: 8 MB).
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        }
        return;
    }
    for dir in ["/opt/homebrew/opt/opus/lib", "/usr/local/opt/opus/lib"] {
        let lib = std::path::Path::new(dir).join("libopus.a");
        if lib.exists() {
            // Link from a folder holding only the .a: next to libopus.dylib,
            // Apple's linker picks the dylib, and the app then needs Homebrew.
            let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("opus");
            std::fs::create_dir_all(&out).unwrap();
            // Homebrew's copy is read-only, so replace rather than overwrite.
            let _ = std::fs::remove_file(out.join("libopus.a"));
            std::fs::copy(&lib, out.join("libopus.a")).unwrap();
            println!("cargo:rerun-if-changed={}", lib.display());
            println!("cargo:rustc-link-search=native={}", out.display());
            println!("cargo:rustc-link-lib=static=opus");
            println!("cargo:rustc-cfg=has_opus");
            return;
        }
    }
    println!("cargo:warning=libopus not found (brew install opus) — Opus files won't play");
}
