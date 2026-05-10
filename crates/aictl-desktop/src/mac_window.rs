//! macOS traffic-light positioning.
//!
//! Tauri 2.11's `trafficLightPosition` (in `tauri.conf.json`) renders
//! the close/min/zoom buttons at a different vertical offset under
//! `cargo tauri dev` than it does inside a bundled `.app`. The
//! high-level `WebviewWindow` API also doesn't expose
//! `set_traffic_light_position` for runtime adjustment.
//!
//! Workaround: mirror tao/wry's internal `inset_traffic_lights` here
//! and call it ourselves from the setup hook + on key window events.
//! With `trafficLightPosition` removed from `tauri.conf.json`, wry's
//! `drawRect` re-application is a no-op, so our value is the only
//! one in play.
//!
//! `apply` must run on the main thread — Tauri's setup hook, window
//! event callbacks, and `run_on_main_thread` satisfy that.
#![cfg(target_os = "macos")]

use objc2_app_kit::{NSView, NSWindow, NSWindowButton};
use objc2_foundation::NSPoint;

const X: f64 = 20.0;
/// Distance (logical px) from the top of the window to the top of the
/// traffic-light buttons. Same value in dev and release — the
/// rendering path is identical once `inset` actually runs against a
/// valid view chain. Tuned to leave visible breathing room below the
/// buttons inside the webview's dark title bar.
const Y: f64 = 17.0;

/// Set to `true` to log `apply` outcomes to stderr. Useful when
/// diagnosing why the buttons end up at the macOS default position
/// (apply silently bailing on a missing button or superview).
const DEBUG_LOG: bool = false;

/// Reposition the traffic-light buttons on the given Tauri window.
pub fn apply<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    let ns_window_ptr = match window.ns_window() {
        Ok(ptr) => ptr,
        Err(err) => {
            if DEBUG_LOG {
                eprintln!("[mac_window] ns_window() failed: {err}");
            }
            return;
        }
    };
    if ns_window_ptr.is_null() {
        if DEBUG_LOG {
            eprintln!("[mac_window] ns_window() returned null");
        }
        return;
    }
    // SAFETY: `ns_window()` returns a valid retained autoreleased
    // `NSWindow*` for the lifetime of the autorelease pool we're
    // running in (the main run loop). We dereference once and let
    // objc2's `Retained` ref-count the buttons we hold across the call.
    unsafe {
        let ns_window: &NSWindow = &*ns_window_ptr.cast();
        inset(ns_window, X, Y);
    }
}

unsafe fn inset(window: &NSWindow, x: f64, y: f64) {
    let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
        if DEBUG_LOG {
            eprintln!("[mac_window] CloseButton not present");
        }
        return;
    };
    let Some(miniaturize) = window.standardWindowButton(NSWindowButton::MiniaturizeButton) else {
        if DEBUG_LOG {
            eprintln!("[mac_window] MiniaturizeButton not present");
        }
        return;
    };
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton);

    let Some(buttons_super) = (unsafe { close.superview() }) else {
        if DEBUG_LOG {
            eprintln!("[mac_window] close.superview() is None");
        }
        return;
    };
    let Some(title_bar_container) = (unsafe { buttons_super.superview() }) else {
        if DEBUG_LOG {
            eprintln!("[mac_window] buttons_super.superview() is None");
        }
        return;
    };

    let close_rect = close.frame();
    let window_height = window.frame().size.height;
    let title_bar_height = close_rect.size.height + y;
    let mut title_bar_rect = title_bar_container.frame();
    title_bar_rect.size.height = title_bar_height;
    title_bar_rect.origin.y = window_height - title_bar_height;
    title_bar_container.setFrame(title_bar_rect);

    let space_between = miniaturize.frame().origin.x - close_rect.origin.x;

    // Desired button-origin in window coords: top edge `y` below the
    // window top, accounting for AppKit's flipped (bottom-left) origin.
    // Convert into `buttons_super` coords once — `setFrameOrigin:`
    // operates in the immediate superview's coordinate system, and
    // AppKit can hand us a different baseline `origin.y` here in dev vs.
    // a bundled release build (the bug this function exists to paper
    // over). Computing from the window frame instead of trusting the
    // existing button origin removes that variance.
    let target_y_window = window_height - y - close_rect.size.height;
    let target_y_local = buttons_super
        .convertPoint_fromView(NSPoint::new(0.0, target_y_window), None)
        .y;

    let mut row = vec![close, miniaturize];
    if let Some(z) = zoom {
        row.push(z);
    }
    for (i, button) in row.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let offset = (i as f64) * space_between;
        let origin = NSPoint::new(x + offset, target_y_local);
        // SAFETY: NSButton inherits NSView; `setFrameOrigin:` is safe
        // to call on the main thread, which we are on.
        let view: &NSView = &button;
        view.setFrameOrigin(origin);
    }
    if DEBUG_LOG {
        eprintln!("[mac_window] applied y={y} → local_y={target_y_local}");
    }
}
