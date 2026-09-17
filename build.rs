//! Build script: make the compiler's embedded files first-class cargo dependencies.
//!
//! `include_dir!` and `include_str!` read files at compile time but do **not** tell cargo that
//! those files exist. Changing only `web/out` (or the bundled model library) therefore leaves
//! `target/` stale: the crate is not rebuilt, and the binary keeps serving the previous UI.
//! That is a silent failure - the build succeeds and the running process behaves normally while
//! serving outdated assets.
//!
//! Emitting `cargo:rerun-if-changed` for every embedded file closes that gap, so cargo always
//! re-embeds the current `web/out`. Keeping `web/out` itself current is the build scripts' job:
//! `build.ps1` and `build.sh` rebuild the frontend unconditionally on every run rather than
//! trying to detect staleness, which is the failure mode this file exists to prevent.

use std::path::{Path, PathBuf};

/// Every file under `dir`, recursively. Returns just the directory when it is empty/missing so
/// the watch still registers (a missing directory is what a fresh checkout looks like before the
/// frontend has been built).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    // Re-run when the build script itself or the manifest changes.
    watch(&root.join("build.rs"));
    watch(&root.join("Cargo.toml"));

    // 1. The embedded web console (src/webui.rs). Watch the directory as well as its contents so
    //    that creating web/out for the first time, or deleting it, also triggers a rebuild.
    let web_out = root.join("web").join("out");
    watch(&web_out);
    let mut files = Vec::new();
    walk(&web_out, &mut files);
    for f in &files {
        watch(f);
    }

    // 2. Anything under assets/ that is embedded with include_str!/include_bytes!.
    let assets = root.join("assets");
    watch(&assets);
    let mut asset_files = Vec::new();
    walk(&assets, &mut asset_files);
    for f in &asset_files {
        watch(f);
    }

    // Surface the situation during the build: an empty web/out means the console is not embedded
    // and the binary will fall back to reading it from disk at runtime.
    if !web_out.join("index.html").exists() {
        println!(
            "cargo:warning=web/out/index.html is missing - the web console will NOT be embedded.              Run npm run build in web/ (or use build.ps1 / build.sh) before building."
        );
    }
}
