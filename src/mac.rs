//! macOS pieces Rune doesn't provide: a status-bar item (Rune's `Tray` covers Linux and
//! Windows), and an Island window AppKit never pushes below the menu bar (Notch Mode puts it in
//! the notch, over the menu bar).

use std::ffi::{CString, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, NSObject, Sel};
use objc2::{ClassType, class, define_class, msg_send, sel};

#[repr(C)]
#[derive(Clone, Copy)]
struct Point { x: f64, y: f64 }
#[repr(C)]
#[derive(Clone, Copy)]
struct Size { width: f64, height: f64 }
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect { origin: Point, size: Size }

// SAFETY: these match CoreGraphics' CGPoint, CGSize and CGRect on 64-bit macOS.
unsafe impl Encode for Point { const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]); }
unsafe impl Encode for Size { const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]); }
unsafe impl Encode for Rect { const ENCODING: Encoding = Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]); }

fn ns_string(s: &str) -> *mut AnyObject {
    let s = CString::new(s).unwrap_or_default();
    // SAFETY: a NUL-terminated UTF-8 string; NSString copies it.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()] }
}

// --- The status-bar item ---

/// Called with the picked item's index.
static HANDLER: OnceLock<Box<dyn Fn(usize) + Send + Sync>> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "RippleTrayTarget"]
    struct TrayTarget;

    impl TrayTarget {
        #[unsafe(method(pick:))]
        fn pick(&self, sender: *mut AnyObject) {
            // SAFETY: the sender is one of the NSMenuItems below.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Some(handler) = HANDLER.get() { handler(tag as usize) }
        }
    }
);

/// Puts Ripple's icon in the menu bar with `items` as (id, title), a separator before the last.
/// `on_pick` gets the picked item's id. Call it once, on the main thread.
pub fn tray(icon_png: &[u8], items: &'static [(&'static str, &'static str)], on_pick: impl Fn(&str) + Send + Sync + 'static) {
    let handler = move |i: usize| if let Some(&(id, _)) = items.get(i) { on_pick(id) };
    if HANDLER.set(Box::new(handler)).is_err() { return }
    // SAFETY: AppKit calls on the main thread; the item, menu and target live for the process.
    unsafe {
        let target: Retained<TrayTarget> = msg_send![TrayTarget::class(), new];
        let bar: *mut AnyObject = msg_send![class!(NSStatusBar), systemStatusBar];
        // NSVariableStatusItemLength
        let item: *mut AnyObject = msg_send![bar, statusItemWithLength: -1.0f64];
        let _: *mut AnyObject = msg_send![item, retain];
        let button: *mut AnyObject = msg_send![item, button];
        let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: icon_png.as_ptr().cast::<c_void>(), length: icon_png.len()];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if !image.is_null() {
            let _: () = msg_send![image, setSize: Size { width: 18.0, height: 18.0 }];
            let _: () = msg_send![button, setImage: image];
        }
        let _: () = msg_send![button, setToolTip: ns_string("Ripple")];

        let menu: *mut AnyObject = msg_send![class!(NSMenu), new];
        for (i, (_, title)) in items.iter().enumerate() {
            if i + 1 == items.len() {
                let separator: *mut AnyObject = msg_send![class!(NSMenuItem), separatorItem];
                let _: () = msg_send![menu, addItem: separator];
            }
            let entry: *mut AnyObject = msg_send![class!(NSMenuItem), alloc];
            let entry: *mut AnyObject = msg_send![entry, initWithTitle: ns_string(title), action: sel!(pick:), keyEquivalent: ns_string("")];
            let _: () = msg_send![entry, setTarget: &*target];
            let _: () = msg_send![entry, setTag: i as isize];
            let _: () = msg_send![menu, addItem: entry];
        }
        let _: () = msg_send![item, setMenu: menu];
        std::mem::forget(target);
    }
}

// --- No frame constraint for the Island ---

/// The Island's current NSWindow (retained); AppKit's frame constraint is skipped for it alone.
static ISLAND_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL: OnceLock<usize> = OnceLock::new();

unsafe extern "C-unwind" fn constrain(this: *mut AnyObject, sel: Sel, frame: Rect, screen: *mut AnyObject) -> Rect {
    if ISLAND_WINDOW.load(Ordering::Acquire) == this as usize { return frame }
    let original: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Rect, *mut AnyObject) -> Rect =
        // SAFETY: ORIGINAL holds the replaced implementation of this same method.
        unsafe { std::mem::transmute(*ORIGINAL.get().unwrap_or(&0)) };
    // SAFETY: forwards the call unchanged.
    unsafe { original(this, sel, frame, screen) }
}

/// Lets the Island's window sit exactly where it's placed, over the menu bar in Notch Mode, as
/// the Electron app's native helper did. Call it on the main thread with the window's NSView.
pub fn unconstrain(ns_view: *mut c_void) {
    if ns_view.is_null() { return }
    // SAFETY: a live NSView on the main thread; the method swap keeps the original for every
    // other window of the class.
    unsafe {
        let window: *mut AnyObject = msg_send![ns_view.cast::<AnyObject>(), window];
        if window.is_null() { return }
        let Some(method) = (*window).class().instance_method(sel!(constrainFrameRect:toScreen:)) else { return };
        // A recreated window replaces the old one; the old one is released, the new one retained
        // so its address can't be reused by another object while it's remembered.
        let _: *mut AnyObject = msg_send![window, retain];
        let old = ISLAND_WINDOW.swap(window as usize, Ordering::AcqRel);
        if old != 0 && old != window as usize { let _: () = msg_send![old as *mut AnyObject, release]; }
        // Swap the method once; the original is stored first, so a racing call never misses it.
        if ORIGINAL.get().is_some() { return }
        let _ = ORIGINAL.set(method.implementation() as usize);
        let replacement: Imp = std::mem::transmute(constrain as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Rect, *mut AnyObject) -> Rect);
        method.set_implementation(replacement);
    }
}

/// NSWindowSharingReadOnly, or NSWindowSharingNone to keep the window out of captures. Call it
/// on the main thread with the window's NSView.
pub fn set_sharing(ns_view: *mut c_void, shared: bool) {
    if ns_view.is_null() { return }
    // SAFETY: a live NSView on the main thread.
    unsafe {
        let window: *mut AnyObject = msg_send![ns_view.cast::<AnyObject>(), window];
        if window.is_null() { return }
        let _: () = msg_send![window, setSharingType: usize::from(shared)];
    }
}

// --- Desktop drags ---

/// `system::watch_drags`: a drag writes the drag pasteboard as it starts, with a mouse button down.
pub fn watch_drags(on: impl Fn(bool)) {
    // SAFETY: NSPasteboard and NSEvent's class queries are safe off the main thread; a nil board ends the watch.
    let board: Option<Retained<AnyObject>> = objc2::rc::autoreleasepool(|_| unsafe {
        msg_send![class!(NSPasteboard), pasteboardWithName: ns_string("Apple CFPasteboard drag")]
    });
    let Some(board) = board else { return };
    // SAFETY: as above.
    let read = || unsafe {
        let count: isize = msg_send![&*board, changeCount];
        let buttons: usize = msg_send![class!(NSEvent), pressedMouseButtons];
        (count, buttons != 0)
    };
    // SAFETY: as above; the types array is autoreleased, so it's read inside a pool.
    let has_files = || objc2::rc::autoreleasepool(|_| unsafe {
        let types: *mut AnyObject = msg_send![&*board, types];
        !types.is_null() && { let on: objc2::runtime::Bool = msg_send![types, containsObject: ns_string("public.file-url")]; on.as_bool() }
    });
    let (mut last, mut dragging) = (read().0, false);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let (count, held) = read();
        if !dragging && held && count != last && has_files() { dragging = true; on(true) }
        else if dragging && !held { dragging = false; on(false) }
        last = count;
    }
}

// --- Focus hand-back ---

pub fn frontmost_pid() -> Option<i32> {
    // SAFETY: NSWorkspace is safe from any thread; a nil app gives None.
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() { return None }
        let pid: i32 = msg_send![app, processIdentifier];
        Some(pid)
    }
}

/// The frontmost app has a window covering a whole display at the normal level: a full screen
/// Space (a zoomed window leaves the menu bar showing, so it doesn't count).
pub fn frontmost_fullscreen() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut AnyObject;
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> Rect;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { fn CFRelease(object: *const c_void); }
    let Some(pid) = frontmost_pid() else { return false };
    let mut ids = [0u32; 16];
    let mut count = 0;
    // SAFETY: `ids` holds 16 entries and `count` receives how many were filled.
    if unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut count) } != 0 { return false }
    // SAFETY: plain display queries.
    let sizes: Vec<(f64, f64)> = ids[..count as usize].iter().map(|&d| { let r = unsafe { CGDisplayBounds(d) }; (r.size.width, r.size.height) }).collect();
    // SAFETY: on-screen windows (1) without desktop elements (16). The CFArray of CFDictionaries is
    // toll-free bridged to NSArray/NSDictionary, read by key, then released as the Copy rule says.
    // The pool drains the key strings; this runs on a polling thread that has none.
    objc2::rc::autoreleasepool(|_| unsafe {
        let list = CGWindowListCopyWindowInfo(1 | 16, 0);
        if list.is_null() { return false }
        let n: usize = msg_send![list, count];
        let num = |w: *mut AnyObject, key: &str| -> Option<f64> {
            let v: *mut AnyObject = msg_send![w, objectForKey: ns_string(key)];
            (!v.is_null()).then(|| msg_send![v, doubleValue])
        };
        let full = (0..n).any(|i| {
            let w: *mut AnyObject = msg_send![list, objectAtIndex: i];
            let bounds: *mut AnyObject = msg_send![w, objectForKey: ns_string("kCGWindowBounds")];
            num(w, "kCGWindowOwnerPID") == Some(f64::from(pid)) && num(w, "kCGWindowLayer") == Some(0.0) && !bounds.is_null()
                && matches!((num(bounds, "Width"), num(bounds, "Height")), (Some(w), Some(h)) if sizes.contains(&(w, h)))
        });
        CFRelease(list as *const c_void);
        full
    })
}

/// Brings the app with `pid` to the front; Ripple, being active, may hand activation over.
pub fn activate_pid(pid: i32) {
    // SAFETY: a nil app (it quit) is skipped.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
        if app.is_null() { return }
        let _: objc2::runtime::Bool = msg_send![app, activateWithOptions: 0usize];
    }
}

// --- Download progress ---

/// Calls `put` once a second with the share done (None: not known yet) of each download another
/// app publishes for a file in `dir`: the NSProgress Finder draws on a downloading file.
/// Runs for good: call it on a thread of its own.
pub fn watch_progress(dir: &std::path::Path, put: impl Fn(Vec<Option<f64>>)) {
    use block2::{Block, RcBlock};
    use std::sync::{Arc, Mutex};
    // Each published progress by address, with the block that drops it when it is unpublished.
    type Live = Arc<Mutex<Vec<(usize, Retained<AnyObject>, RcBlock<dyn Fn()>)>>>;
    let live: Live = Arc::default();
    let handler = {
        let live = live.clone();
        RcBlock::new(move |progress: *mut AnyObject| -> *mut Block<dyn Fn()> {
            // SAFETY: the system hands over a live NSProgress (or nil, skipped).
            let Some(p) = (unsafe { Retained::retain(progress) }) else { return std::ptr::null_mut() };
            let key = progress as usize;
            let gone: RcBlock<dyn Fn()> = { let live = live.clone(); RcBlock::new(move || live.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| e.0 != key)) };
            // The caller copies the block it is given; ours stays alive in `live` until it runs.
            let ptr = RcBlock::as_ptr(&gone);
            live.lock().unwrap_or_else(|e| e.into_inner()).push((key, p, gone));
            ptr
        })
    };
    // SAFETY: a file URL for the folder; the subscriber token is kept for good, as the watch is.
    let _token: *mut AnyObject = objc2::rc::autoreleasepool(|_| unsafe {
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_string(&dir.to_string_lossy())];
        let token: *mut AnyObject = msg_send![class!(NSProgress), addSubscriberForFileURL: url, withPublishingHandler: &*handler];
        let _: *mut AnyObject = msg_send![token, retain];
        token
    });
    loop {
        let started = std::time::Instant::now();
        // Publishing calls can arrive on this thread's run loop; run it for up to a second.
        // SAFETY: this thread's own run loop; the pool drains the date.
        objc2::rc::autoreleasepool(|_| unsafe {
            let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
            let until: *mut AnyObject = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 1.0f64];
            let _: () = msg_send![run_loop, runUntilDate: until];
        });
        // A run loop with no sources returns at once: wait out the rest of the second.
        std::thread::sleep(std::time::Duration::from_secs(1).saturating_sub(started.elapsed()));
        // Read outside the lock, so an unpublish arriving meanwhile can't wait on it.
        let progresses: Vec<Retained<AnyObject>> = live.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|e| e.1.clone()).collect();
        // SAFETY: NSProgress is thread-safe; these are plain property reads.
        let fractions = progresses.iter().filter_map(|p| unsafe {
            let ended: bool = msg_send![&**p, isFinished];
            let cancelled: bool = msg_send![&**p, isCancelled];
            let unknown: bool = msg_send![&**p, isIndeterminate];
            (!ended && !cancelled).then(|| (!unknown).then(|| -> f64 { msg_send![&**p, fractionCompleted] }))
        }).collect();
        put(fractions);
    }
}
