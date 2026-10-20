// Windows targets: embed assets/rusty-beaver.ico as icon resource 1, which Explorer shows
// for the .exe and gpui loads for the window and taskbar. Decided by the *target* OS, so a
// cross build (mingw windres) gets the icon as well. macOS gets its icon from the .app
// bundle and at runtime (Dock), Linux from the installed .desktop file + hicolor icons.
fn main() {
    println!("cargo:rerun-if-changed=assets/rusty-beaver.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico =
            format!("{}/assets/rusty-beaver.ico", env!("CARGO_MANIFEST_DIR")).replace('\\', "/");
        let rc = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("rusty-beaver.rc");
        std::fs::write(&rc, format!("1 ICON \"{ico}\"\n")).unwrap();
        embed_resource::compile(&rc, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
