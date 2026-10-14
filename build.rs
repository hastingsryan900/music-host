// Statically link libopus (from Homebrew) when available so Ogg/Opus files
// play. The binary stays self-contained either way.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_opus)");
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
        if std::path::Path::new(dir).join("libopus.a").exists() {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=opus");
            println!("cargo:rustc-cfg=has_opus");
            return;
        }
    }
    println!("cargo:warning=libopus not found (brew install opus) — Opus files won't play");
}
