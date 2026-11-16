//! Click-through around the Island. Its window is a transparent rectangle larger than the
//! Island; only the Island itself may take the pointer, so clicks beside it reach the
//! windows underneath.
//!
//! - Linux (X11): the window's input shape is set to the Island's rect. The X server then
//!   routes the pointer itself; nothing polls.
//! - Windows and macOS: the whole window ignores the mouse while the pointer is outside the
//!   Island's rect, checked about 30 times a second.

use rune::{Tx, WindowHandle};

use crate::island::Screen;

#[derive(Default)]
pub struct Passthrough {
    window: Option<WindowHandle>,
    /// The region last applied, in device pixels inside the window.
    applied: Option<[i32; 4]>,
    #[cfg(target_os = "linux")]
    x11: Option<x11rb::rust_connection::RustConnection>,
    #[cfg(not(target_os = "linux"))]
    through: Option<bool>,
}

impl Passthrough {
    /// A new native window: whatever was applied to the old one is gone.
    pub fn set_window(&mut self, window: WindowHandle) {
        self.window = Some(window);
        self.applied = None;
        #[cfg(not(target_os = "linux"))]
        { self.through = None; }
    }

    /// Makes `region` (points inside the window) the only part of the window that takes the
    /// pointer. `window_at` is the window's top-left on `screen`, in points. Call it before
    /// every frame; it does nothing while nothing changed.
    pub fn sync(&mut self, region: [f32; 4], window_at: (f32, f32), screen: &Screen, tx: &mut Tx) {
        let Some(window) = &self.window else { return };
        let scale = screen.scale;
        let px = [
            (region[0] * scale).floor() as i32,
            (region[1] * scale).floor() as i32,
            (region[2] * scale).ceil() as i32,
            (region[3] * scale).ceil() as i32,
        ];
        #[cfg(target_os = "linux")]
        {
            let _ = (window_at, tx);
            if self.applied == Some(px) { return }
            if self.x11.is_none() { self.x11 = x11rb::connect(None).ok().map(|(conn, _)| conn) }
            let Some(conn) = &self.x11 else { return };
            let Ok(Some(id)) = window.with_raw_handle(x11_window) else { return };
            if set_input_shape(conn, id, px).is_ok() { self.applied = Some(px) }
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.applied = Some(px);
            // The OS's global space is points on macOS and pixels on Windows.
            let unit = if cfg!(target_os = "macos") { 1.0 } else { f64::from(scale) };
            let left = screen.origin.0 + f64::from(window_at.0 + region[0]) * unit;
            let top = screen.origin.1 + f64::from(window_at.1 + region[1]) * unit;
            let (width, height) = (f64::from(region[2]) * unit, f64::from(region[3]) * unit);
            if let Some((x, y)) = pointer() {
                let inside = x >= left && y >= top && x < left + width && y < top + height;
                if self.through != Some(!inside) && window.set_click_through(!inside).is_ok() { self.through = Some(!inside) }
            }
            tx.wake_after(std::time::Duration::from_millis(33));
        }
    }
}

#[cfg(target_os = "linux")]
fn x11_window(handle: rune::raw_window_handle::WindowHandle<'_>) -> Option<u32> {
    use rune::raw_window_handle::RawWindowHandle;
    match handle.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Only ShapeInput: the window keeps its full transparent bounds, so nothing redraws.
#[cfg(target_os = "linux")]
fn set_input_shape(conn: &x11rb::rust_connection::RustConnection, window: u32, [x, y, w, h]: [i32; 4]) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
    let clamp = |v: i32| v.clamp(0, i32::from(u16::MAX)) as u16;
    let rect = Rectangle { x: x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16, y: y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16, width: clamp(w), height: clamp(h) };
    conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[rect])?.check()?;
    conn.flush()?;
    Ok(())
}

/// The pointer in the OS's global space, top-left origin.
#[cfg(windows)]
fn pointer() -> Option<(f64, f64)> {
    #[repr(C)]
    #[derive(Default)]
    struct Point { x: i32, y: i32 }
    #[link(name = "user32")]
    unsafe extern "system" { fn GetCursorPos(point: *mut Point) -> i32; }
    let mut p = Point::default();
    // SAFETY: `p` is a valid POINT for the call.
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| (f64::from(p.x), f64::from(p.y)))
}

#[cfg(target_os = "macos")]
fn pointer() -> Option<(f64, f64)> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Point { x: f64, y: f64 }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *const c_void) -> Point;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { fn CFRelease(object: *const c_void); }
    // SAFETY: a null source gives an event carrying the current pointer location; it is
    // released once read.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() { return None }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x, p.y))
    }
}

/// Keeps the Island's window off the taskbar and on every desktop.
pub fn adopt(window: &WindowHandle) {
    #[cfg(target_os = "linux")]
    if let Ok(Some(id)) = window.with_raw_handle(x11_window)
        && let Err(e) = x11_sticky(id) { eprintln!("Ripple: could not keep the Island on every desktop: {e}") }
    #[cfg(windows)]
    {
        use rune::raw_window_handle::RawWindowHandle;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
            fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        }
        const GWL_EXSTYLE: i32 = -20;
        const WS_EX_TOOLWINDOW: isize = 0x80;
        const WS_EX_APPWINDOW: isize = 0x40000;
        let _ = window.with_raw_handle(|h| if let RawWindowHandle::Win32(h) = h.as_raw() {
            let hwnd = h.hwnd.get();
            // SAFETY: a live window handle, on the UI thread that owns it.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW);
            }
        });
    }
    // MacWindowOptions' collection behaviour keeps it on every Space; this keeps AppKit from
    // moving it below the menu bar.
    #[cfg(target_os = "macos")]
    let _ = window.with_raw_handle(|h| if let rune::raw_window_handle::RawWindowHandle::AppKit(h) = h.as_raw() {
        crate::mac::unconstrain(h.ns_view.as_ptr());
    });
}

/// _NET_WM_STATE: sticky, above, and off the taskbar and pager; set on the window before
/// it maps, and asked of the window manager in case it already has.
#[cfg(target_os = "linux")]
fn x11_sticky(window: u32) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let state = atom("_NET_WM_STATE")?;
    let wanted = [atom("_NET_WM_STATE_STICKY")?, atom("_NET_WM_STATE_SKIP_TASKBAR")?, atom("_NET_WM_STATE_SKIP_PAGER")?, atom("_NET_WM_STATE_ABOVE")?];
    conn.change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &wanted)?;
    // All desktops.
    conn.change_property32(PropMode::REPLACE, window, atom("_NET_WM_DESKTOP")?, AtomEnum::CARDINAL, &[u32::MAX])?;
    for pair in wanted.chunks(2) {
        let event = ClientMessageEvent::new(32, window, state, [1, pair[0], *pair.get(1).unwrap_or(&0), 1, 0]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
    }
    conn.flush()?;
    Ok(())
}
