//! Runtime app icon. macOS: the Dock icon (also when not started from the .app bundle).
//! Windows embeds the icon in the .exe (build.rs), Linux uses the installed .desktop + hicolor icons.

#[cfg(target_os = "macos")]
pub fn set_app_icon(png: &'static [u8]) {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(png);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_app_icon(_: &'static [u8]) {}

/// Puts the app's windows in front without activating the app, so a covered window paints
/// again (macOS stops drawing fully covered windows) while keyboard focus stays where it is.
#[cfg(target_os = "macos")]
pub fn order_front_quietly() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for w in NSApplication::sharedApplication(mtm).windows().iter() {
        if w.isVisible() {
            unsafe { w.orderFrontRegardless() };
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn order_front_quietly() {}
