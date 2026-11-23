//! Screen sampling: the sRGB colour of a pixel the user clicks anywhere on screen. Blocks until
//! they click or cancel; call it off the UI thread.

/// The colour the user picks, or None when they cancel or the OS can't sample.
pub fn pick() -> Option<[u8; 3]> {
    #[cfg(target_os = "linux")]
    {
        // The portal's sampler works on Wayland too; plain X11 window managers may have none.
        match portal() {
            Ok(picked) => picked,
            Err(_) if !crate::system::wayland_session() => x11().ok().flatten(),
            Err(_) => None,
        }
    }
    #[cfg(target_os = "macos")]
    { mac() }
    #[cfg(windows)]
    { windows() }
}

/// 0..1 channels to bytes.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn bytes(rgb: [f64; 3]) -> [u8; 3] { rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8) }

/// xdg-desktop-portal's `Screenshot.PickColor`: the desktop shows its own sampler.
#[cfg(target_os = "linux")]
fn portal() -> zbus::Result<Option<[u8; 3]>> {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    let conn = zbus::blocking::Connection::session()?;
    // Listen on the request's path before asking, so a quick answer isn't missed.
    let token = format!("ripple_pick_{}", std::process::id());
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    let request = |path: String| -> zbus::Result<zbus::blocking::Proxy<'static>> {
        zbus::blocking::proxy::Builder::new(&conn)
            .destination("org.freedesktop.portal.Desktop")?
            .path(path)?
            .interface("org.freedesktop.portal.Request")?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
    };
    let expected = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let mut responses = request(expected.clone())?.receive_signal("Response")?;
    let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
    let handle: OwnedObjectPath = conn
        .call_method(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.Screenshot"), "PickColor", &("", options))?
        .body().deserialize()?;
    // Portals older than handle_token answer on a path of their own.
    if handle.as_str() != expected { responses = request(handle.to_string())?.receive_signal("Response")? }
    let Some(answer) = responses.next() else { return Ok(None) };
    let (code, results): (u32, HashMap<String, OwnedValue>) = answer.body().deserialize()?;
    let Some(color) = results.get("color").filter(|_| code == 0) else { return Ok(None) };
    let (r, g, b): (f64, f64, f64) = color.try_clone()?.try_into()?;
    Ok(Some(bytes([r, g, b])))
}

/// Grabs the pointer with a crosshair and reads the pixel under the first left click; any
/// other button or a key cancels.
#[cfg(target_os = "linux")]
fn x11() -> Result<Option<[u8; 3]>, Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ConnectionExt as _, EventMask, GrabMode, GrabStatus, ImageFormat};
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let (font, cursor) = (conn.generate_id()?, conn.generate_id()?);
    conn.open_font(font, b"cursor")?;
    // Glyph 34 of the cursor font is the crosshair, 35 its mask.
    conn.create_glyph_cursor(cursor, font, font, 34, 35, 0, 0, 0, 0xffff, 0xffff, 0xffff)?;
    // The press on Pick may still hold Ripple's own grab for a moment; wait for it to end.
    let mut tries = 0;
    loop {
        let grab = conn.grab_pointer(false, root, EventMask::BUTTON_PRESS, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, cursor, x11rb::CURRENT_TIME)?.reply()?;
        if grab.status == GrabStatus::SUCCESS { break }
        tries += 1;
        if tries == 40 { return Ok(None) }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // The keyboard too, so Escape cancels; without it only a click ends the grab.
    conn.grab_keyboard(false, root, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?;
    let at = loop {
        match conn.wait_for_event()? {
            Event::ButtonPress(e) => break (e.detail == 1).then_some((e.root_x, e.root_y)),
            Event::KeyPress(_) => break None,
            _ => {}
        }
    };
    conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
    conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
    conn.flush()?;
    let Some((x, y)) = at else { return Ok(None) };
    let image = conn.get_image(ImageFormat::Z_PIXMAP, root, x, y, 1, 1, !0)?.reply()?;
    // shortcut: reads a 24/32-bit TrueColor pixel (B, G, R, X), as current X servers use; decode visuals for 16-bit or big-endian servers.
    Ok(match image.data[..] { [b, g, r, ..] => Some([r, g, b]), _ => None })
}

/// AppKit's `NSColorSampler`: the system magnifier, shown from the main thread; needs no
/// screen recording permission.
#[cfg(target_os = "macos")]
fn mac() -> Option<[u8; 3]> {
    use std::ffi::c_void;
    use std::sync::mpsc::{Sender, channel};
    use block2::RcBlock;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }

    extern "C" fn show(context: *mut c_void) {
        // SAFETY: `context` is the boxed Sender `mac` leaked for this call, taken back once.
        let answer = *unsafe { Box::from_raw(context.cast::<Sender<Option<[u8; 3]>>>()) };
        let handler = RcBlock::new(move |color: *mut AnyObject| {
            // SAFETY: a nil colour is a cancel; the sRGB copy may be nil for a pattern colour.
            let rgb = (!color.is_null()).then(|| unsafe {
                let srgb: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
                let c: *mut AnyObject = msg_send![color, colorUsingColorSpace: srgb];
                (!c.is_null()).then(|| { let (r, g, b): (f64, f64, f64) = (msg_send![c, redComponent], msg_send![c, greenComponent], msg_send![c, blueComponent]); bytes([r, g, b]) })
            }).flatten();
            let _ = answer.send(rgb);
        });
        // SAFETY: runs on the main thread, as AppKit needs. The sampler, owned +1 by `new`, is
        // leaked: it must outlive the handler, and one tiny object per pick is cheap.
        unsafe {
            let sampler: *mut AnyObject = msg_send![class!(NSColorSampler), new];
            let _: () = msg_send![sampler, showSamplerWithSelectionHandler: &*handler];
        }
    }

    let (tx, rx) = channel();
    // SAFETY: the main queue is libdispatch's static; `show` takes the box back.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, Box::into_raw(Box::new(tx)).cast(), show) };
    rx.recv().ok().flatten()
}

/// Windows has no system sampler and no notice of clicks outside Ripple's windows, so this
/// watches the mouse buttons: the next left click picks, a right click or Escape cancels.
/// shortcut: the click also reaches the window under it; cover the screen with a transparent window if that bites.
#[cfg(windows)]
fn windows() -> Option<[u8; 3]> {
    use std::time::{Duration, Instant};
    #[repr(C)]
    #[derive(Default)]
    struct Point { x: i32, y: i32 }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn GetDC(window: isize) -> isize;
        fn ReleaseDC(window: isize, dc: isize) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" { fn GetPixel(dc: isize, x: i32, y: i32) -> u32; }
    // SAFETY: plain virtual-key queries: VK_LBUTTON, VK_RBUTTON, VK_ESCAPE.
    let down = |key: i32| (unsafe { GetAsyncKeyState(key) }) < 0;
    let start = Instant::now();
    let tick = || std::thread::sleep(Duration::from_millis(10));
    // The press on Pick itself.
    while down(1) { tick() }
    loop {
        if down(2) || down(0x1b) || start.elapsed() > Duration::from_secs(60) { return None }
        if down(1) { break }
        tick();
    }
    let mut p = Point::default();
    // SAFETY: `p` is a valid POINT; the screen DC is released once read. CLR_INVALID is all ones.
    let c = unsafe {
        if GetCursorPos(&mut p) == 0 { return None }
        let dc = GetDC(0);
        let c = GetPixel(dc, p.x, p.y);
        ReleaseDC(0, dc);
        c
    };
    // COLORREF is 0x00BBGGRR.
    (c != u32::MAX).then(|| [c as u8, (c >> 8) as u8, (c >> 16) as u8])
}
