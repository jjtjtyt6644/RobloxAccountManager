//! Embeds ram-login.exe into the manager so users only download one file.
//!
//! The sign-in helper is still a separate process (no webview code is linked into the manager); the
//! manager just carries its bytes and unpacks them on first sign-in (auth::login::helper_path).
//! The bytes sit in the exe image and are only paged in when unpacked, so idle memory is unaffected.
//!
//! ram-login must be built BEFORE the manager — use scripts\build-release.ps1. If it isn't found, an
//! empty file is embedded and the manager falls back to a ram-login.exe sitting next to it.
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ram-login.bin");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| manifest.join("..").join("target"));
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".into());

    println!("cargo:rerun-if-env-changed=RAM_LOGIN_EXE");
    let mut candidates: Vec<PathBuf> = std::env::var("RAM_LOGIN_EXE").map(PathBuf::from).into_iter().collect();
    candidates.push(target.join(&profile).join("ram-login.exe"));
    candidates.push(target.join("release").join("ram-login.exe"));
    for c in &candidates {
        println!("cargo:rerun-if-changed={}", c.display());
    }

    match candidates.iter().find(|c| c.is_file()) {
        Some(src) => {
            std::fs::copy(src, &out).expect("copy ram-login.exe");
        }
        None => {
            std::fs::write(&out, b"").unwrap();
            println!(
                "cargo:warning=ram-login.exe not found — this build will NOT contain the sign-in helper. \
                 Build with scripts\\build-release.ps1 (it builds ram-login first)."
            );
        }
    }
}
