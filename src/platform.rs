//! The displays, and click-through around the Island. Its window is a transparent rectangle larger than the
//! Island; only the Island itself may take the pointer, so clicks beside it reach the
//! windows underneath.
//!
//! - Linux (X11): the window's input shape is set to the Island's rect. The X server then
//!   routes the pointer itself; nothing polls. Without a compositing manager X can't draw a
//!   transparent window (it shows black), so the window's bounding shape is cut to the rect too.
//! - Windows and macOS: the whole window ignores the mouse while the pointer is outside the
//!   Island's rect, checked about 30 times a second.

use rune::{Display, DisplayTarget, Tx, WindowHandle};

/// One display as the window factory saw it at startup.
#[derive(Clone, Debug)]
pub struct Screen {
    pub name: String,
    pub target: DisplayTarget,
    /// Size in points.
    pub size: (f32, f32),
    pub scale: f32,
    /// Top-left in the OS's global space: points on macOS, pixels elsewhere.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub origin: (f64, f64),
    /// Top-left as the window factory places windows, in pixels.
    pub position: (i32, i32),
    pub primary: bool,
}

impl Screen {
    pub fn all(displays: &[Display]) -> Vec<Screen> {
        displays.iter().enumerate().map(|(i, d)| {
            let scale = d.scale.max(0.1) as f32;
            let bounds = d.native_bounds();
            Screen {
                name: d.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Display {}", i + 1)),
                target: d.id.clone().map_or(DisplayTarget::Primary, DisplayTarget::Id),
                size: (d.size.0 as f32 / scale, d.size.1 as f32 / scale),
                scale,
                origin: (bounds[0], bounds[1]),
                position: d.position,
                primary: d.primary,
            }
        }).collect()
    }
}

/// How far around the Island, in points, a file drag held down elsewhere reaches it (Windows).
pub const DROP_NEAR: f32 = 40.0;

#[derive(Default)]
pub struct Passthrough {
    window: Option<WindowHandle>,
    /// The region last applied, in device pixels inside the window.
    applied: Option<[i32; 4]>,
    #[cfg(target_os = "linux")]
    /// The X connection and its screen's `_NET_WM_CM_Sn` atom, owned while a compositor runs.
    x11: Option<(x11rb::rust_connection::RustConnection, u32)>,
    #[cfg(target_os = "linux")]
    /// Whether a compositor ran when `applied` was set.
    composited: bool,
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
            if self.x11.is_none() { self.x11 = connect() }
            let Some((conn, cm)) = &self.x11 else { return };
            // Checked every frame, not only on a rect change, as a compositor can start or stop while idle.
            let Ok(composited) = composited(conn, *cm) else { return };
            if self.applied == Some(px) && self.composited == composited { return }
            let Ok(Some(id)) = window.with_raw_handle(x11_window) else { return };
            if set_shape(conn, id, px, composited).is_ok() { (self.applied, self.composited) = (Some(px), composited) }
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.applied = Some(px);
            // The OS's global space is points on macOS and pixels on Windows.
            let unit = if cfg!(target_os = "macos") { 1.0 } else { f64::from(scale) };
            // A press held from elsewhere (maybe a file drag) takes no click here, so the window
            // may take the pointer near the Island too; its drop then lands on `drop-near`.
            let near = if cfg!(windows) && button_held() { DROP_NEAR } else { 0.0 };
            let left = screen.origin.0 + f64::from(window_at.0 + region[0] - near) * unit;
            let top = screen.origin.1 + f64::from(window_at.1 + region[1] - near) * unit;
            let (width, height) = (f64::from(region[2] + 2.0 * near) * unit, f64::from(region[3] + 2.0 * near) * unit);
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

#[cfg(target_os = "linux")]
fn connect() -> Option<(x11rb::rust_connection::RustConnection, u32)> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let cm = conn.intern_atom(false, format!("_NET_WM_CM_S{screen}").as_bytes()).ok()?.reply().ok()?.atom;
    Some((conn, cm))
}

/// A compositing manager owns this selection (EWMH).
#[cfg(target_os = "linux")]
fn composited(conn: &x11rb::rust_connection::RustConnection, cm: u32) -> Result<bool, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    Ok(conn.get_selection_owner(cm)?.reply()?.owner != x11rb::NONE)
}

/// ShapeInput always. Under a compositor the window keeps its full transparent bounds, so
/// nothing redraws; without one, ShapeBounding cuts away what would show black.
#[cfg(target_os = "linux")]
fn set_shape(conn: &x11rb::rust_connection::RustConnection, window: u32, [x, y, w, h]: [i32; 4], composited: bool) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
    use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
    use x11rb::NONE;
    let clamp = |v: i32| v.clamp(0, i32::from(u16::MAX)) as u16;
    let rect = Rectangle { x: x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16, y: y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16, width: clamp(w), height: clamp(h) };
    conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[rect])?.check()?;
    if !composited {
        // shortcut: a rectangle, so its corners and hover margin still show black; use a pixmap mask if that shows.
        conn.shape_rectangles(SO::SET, SK::BOUNDING, ClipOrdering::UNSORTED, window, 0, 0, &[rect])?.check()?;
    } else {
        conn.shape_mask(SO::SET, SK::BOUNDING, window, 0, 0, NONE)?.check()?;
    }
    conn.flush()?;
    Ok(())
}

/// A mouse button is down; physical buttons, so swapped buttons count too.
#[cfg(windows)]
fn button_held() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" { fn GetAsyncKeyState(key: i32) -> i16; }
    // SAFETY: plain virtual-key queries (VK_LBUTTON, VK_RBUTTON).
    [1, 2].into_iter().any(|key| (unsafe { GetAsyncKeyState(key) }) < 0)
}

#[cfg(target_os = "macos")]
fn button_held() -> bool { false }

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

/// The window with keyboard focus now, as an id `focus_window` takes. macOS gives the
/// frontmost app's pid, as apps, not windows, take focus there.
pub fn focused_window() -> Option<u64> {
    #[cfg(target_os = "linux")]
    { x11_active().ok().flatten().map(u64::from) }
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" { fn GetForegroundWindow() -> isize; }
        // SAFETY: no arguments; returns 0 when no window has focus.
        let hwnd = unsafe { GetForegroundWindow() };
        (hwnd != 0).then_some(hwnd as u64)
    }
    #[cfg(target_os = "macos")]
    { crate::mac::frontmost_pid().map(|pid| pid as u64) }
}

/// Gives keyboard focus to a window `focused_window` returned.
pub fn focus_window(window: u64) {
    #[cfg(target_os = "linux")]
    if let Err(e) = x11_activate(window as u32) { eprintln!("Ripple: could not give focus back: {e}") }
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" { fn SetForegroundWindow(hwnd: isize) -> i32; }
        // SAFETY: a stale handle only makes the call fail. Ripple is foreground, so Windows allows it.
        unsafe { SetForegroundWindow(window as isize) };
    }
    #[cfg(target_os = "macos")]
    crate::mac::activate_pid(window as i32);
}

#[cfg(target_os = "linux")]
fn x11_active() -> Result<Option<u32>, Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    // XWayland's active window doesn't follow native Wayland windows.
    if crate::system::wayland_session() { return Ok(None) }
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let active = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
    let reply = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut v| v.next()).filter(|w| *w != x11rb::NONE))
}

/// _NET_ACTIVE_WINDOW as a pager asks it (source 2), which window managers honour without
/// focus-stealing checks.
#[cfg(target_os = "linux")]
fn x11_activate(window: u32) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let active = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
    let event = ClientMessageEvent::new(32, window, active, [2, x11rb::CURRENT_TIME, 0, 0, 0]);
    conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
    conn.flush()?;
    Ok(())
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
    // A user time of 0 asks the window manager not to focus the window when it maps.
    conn.change_property32(PropMode::REPLACE, window, atom("_NET_WM_USER_TIME")?, AtomEnum::CARDINAL, &[0])?;
    for pair in wanted.chunks(2) {
        let event = ClientMessageEvent::new(32, window, state, [1, pair[0], *pair.get(1).unwrap_or(&0), 1, 0]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
    }
    conn.flush()?;
    Ok(())
}
