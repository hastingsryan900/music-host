// Statically link libopus (from Homebrew) when available so Ogg/Opus files
// play. The binary stays self-contained either way.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_opus)");
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
