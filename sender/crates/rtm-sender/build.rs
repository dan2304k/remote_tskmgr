//! Windows targets: embed a version resource (FileVersion/ProductVersion from the crate
//! version, shown by Explorer and Inno Setup) and `assets/icon.ico` as the exe icon.
//! Other targets: nothing.
//!
//! Failure (typically a cross `cargo check` from Linux without a resource compiler) is a
//! cargo warning, not an error. Set `RTM_REQUIRE_WINRES=1` (the release workflow does) to make
//! it fatal so a release never ships without version info.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=RTM_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Remote Task Manager sender")
        .set(
            "FileDescription",
            "rtm-sender: sends system stats to Remote Task Manager viewers",
        )
        .set("LegalCopyright", "MIT License");
    if let Err(e) = res.compile() {
        let required = std::env::var_os("RTM_REQUIRE_WINRES").is_some_and(|v| !v.is_empty());
        if required {
            panic!("embedding the Windows version resource failed: {e}");
        }
        println!("cargo:warning=no Windows version resource / exe icon embedded ({e}); fine for a cross check");
    }
}
