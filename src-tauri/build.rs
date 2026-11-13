use std::path::PathBuf;

/// Embeds `shunguang_explorer.dll` (built by `npm run hook` into `target/hook/release/`) into the exe. If the file is missing,
/// an empty array is embedded: the app still runs, and the Desktop text module uses its fallback method (so `cargo test` does not require building the DLL).
fn embed_explorer_dll() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dll = manifest.join("..").join("target").join("hook").join("release").join("sg_explorer_hook.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let bytes = if windows { std::fs::read(&dll).unwrap_or_default() } else { Vec::new() };
    if windows && bytes.is_empty() {
        println!("cargo:warning=shunguang_explorer.dll not found (run `npm run hook` first); desktop labels will use the fallback");
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("shunguang_explorer.dll");
    std::fs::write(out, bytes).expect("write embedded DLL");
}

fn main() {
    embed_explorer_dll();
    // Custom manifest: same as Tauri's default (Common-Controls 6) plus a supportedOS declaration,
    // needed to create layered child windows for the wallpaper.
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri build failed");
}
