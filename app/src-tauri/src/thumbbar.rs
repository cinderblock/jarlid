//! Windows taskbar thumbnail toolbar — the little transport row that appears
//! under the window preview when you hover Jarlid's taskbar button.
//!
//! This is a *different OS surface* from SMTC. `souvlaki` gives us media keys,
//! the volume flyout and the lock screen; none of those APIs know anything
//! about the thumbnail toolbar. That one has to be registered directly with the
//! shell through `ITaskbarList3::ThumbBarAddButtons`, and its clicks arrive as
//! `WM_COMMAND`/`THBN_CLICKED` on the window procedure — so we also have to
//! subclass Tauri's window to see them.
//!
//! Everything here runs on the window's own thread. Public state updates are
//! posted as a window message rather than locked, which is both cheaper and the
//! only legal way to touch the apartment-threaded `ITaskbarList3` from the
//! event listeners that produce the state.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD,
};
use windows::Win32::UI::Controls::{
    ImageList_Create, ImageList_Destroy, ImageList_ReplaceIcon, HIMAGELIST, ILC_COLOR32,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, TaskbarList,
    THBF_DISABLED, THBF_ENABLED, THB_BITMAP, THB_FLAGS, THB_TOOLTIP, THUMBBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateIconIndirect, DestroyIcon, KillTimer, PostMessageW,
    RegisterWindowMessageW, SetTimer, HICON, ICONINFO, MSGFLT_ALLOW, SM_CXSMICON, SM_CYSMICON,
    WM_APP, WM_COMMAND, WM_DPICHANGED, WM_NCDESTROY, WM_SETTINGCHANGE, WM_TIMER,
};

include!(concat!(env!("OUT_DIR"), "/glyphs.rs"));

/// `THBN_CLICKED`, the notification code packed into `WM_COMMAND`'s high word.
const THBN_CLICKED: u16 = 0x1800;
/// Arbitrary but stable: 'JARL'.
const SUBCLASS_ID: usize = 0x4a41_524c;
const TIMER_ID: usize = 0x4a41_524d;
/// Posted by [`Thumbbar::set_state`]; wparam carries the packed [`State`].
const WM_SET_STATE: u32 = WM_APP + 0x21;

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// The five buttons, left to right. Values double as `THUMBBUTTON::iId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    ThumbDown = 1,
    Replay = 2,
    PlayPause = 3,
    Skip = 4,
    ThumbUp = 5,
}

impl Button {
    const ALL: [Button; 5] = [
        Button::ThumbDown,
        Button::Replay,
        Button::PlayPause,
        Button::Skip,
        Button::ThumbUp,
    ];

    fn from_id(id: u16) -> Option<Button> {
        Button::ALL.into_iter().find(|b| *b as u16 == id)
    }
}

/// What the buttons should currently depict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub playing: bool,
    pub thumb_up: bool,
    pub thumb_down: bool,
    /// A network player (WiiM/DLNA) is the active source, so only the transport
    /// actions it understands are meaningful — thumbs are Pandora-only.
    pub remote: bool,
}

impl State {
    fn pack(self) -> usize {
        (self.playing as usize)
            | (self.thumb_up as usize) << 1
            | (self.thumb_down as usize) << 2
            | (self.remote as usize) << 3
    }

    fn unpack(bits: usize) -> State {
        State {
            playing: bits & 1 != 0,
            thumb_up: bits & 2 != 0,
            thumb_down: bits & 4 != 0,
            remote: bits & 8 != 0,
        }
    }
}

/// Handle for pushing state at the toolbar from any thread.
pub struct Thumbbar {
    hwnd: isize,
}

// The handle stores only the window handle as an integer and communicates
// exclusively by PostMessage, so it is safe to move and share across threads.
unsafe impl Send for Thumbbar {}
unsafe impl Sync for Thumbbar {}

impl Thumbbar {
    /// Repaint the buttons for `state`. Cheap, non-blocking and idempotent —
    /// the window thread drops it if nothing actually changed.
    pub fn set_state(&self, state: State) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut c_void)),
                WM_SET_STATE,
                WPARAM(state.pack()),
                LPARAM(0),
            );
        }
    }
}

/// Attach a thumbnail toolbar to `hwnd`, invoking `on_click` on the window
/// thread whenever the user presses one of the buttons.
pub fn install(hwnd: isize, on_click: impl Fn(Button) + 'static) -> Result<Thumbbar, String> {
    let window = HWND(hwnd as *mut c_void);

    unsafe {
        // WebView2 already puts this thread in an STA; this is just insurance,
        // and RPC_E_CHANGED_MODE is a perfectly fine outcome.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let taskbar_created = RegisterWindowMessageW(windows::core::w!("TaskbarButtonCreated"));
        if taskbar_created == 0 {
            return Err("RegisterWindowMessageW(TaskbarButtonCreated) failed".into());
        }
        // UIPI blocks window messages sent from a lower-integrity process to a
        // higher-integrity window. Explorer runs at medium integrity, so if
        // Jarlid is running elevated the shell can still *draw* our toolbar
        // (that goes out over COM) but every button press it sends back is
        // silently dropped — buttons that look perfect and do nothing. Both the
        // click notification and the taskbar-button broadcast have to be
        // allowed through explicitly. Harmless when not elevated.
        for message in [taskbar_created, WM_COMMAND] {
            let _ = ChangeWindowMessageFilterEx(window, message, MSGFLT_ALLOW, None);
        }

        let ctx = Box::into_raw(Box::new(Ctx {
            on_click: Box::new(on_click),
            taskbar_created,
            inner: RefCell::new(Inner::default()),
        }));

        if !SetWindowSubclass(window, Some(subclass_proc), SUBCLASS_ID, ctx as usize).as_bool() {
            drop(Box::from_raw(ctx));
            return Err("SetWindowSubclass failed".into());
        }

        // The taskbar button usually exists by now (Tauri shows the window
        // before `setup` runs), in which case this succeeds immediately. If it
        // doesn't, TaskbarButtonCreated and the retry timer both cover us.
        ensure_buttons(&*ctx, window, false);
        if !(*ctx).inner.borrow().added {
            SetTimer(Some(window), TIMER_ID, 500, None);
        }
    }

    Ok(Thumbbar { hwnd })
}

// ---------------------------------------------------------------------------
// Window thread state
// ---------------------------------------------------------------------------

struct Ctx {
    on_click: Box<dyn Fn(Button)>,
    taskbar_created: u32,
    inner: RefCell<Inner>,
}

#[derive(Default)]
struct Inner {
    taskbar: Option<ITaskbarList3>,
    images: Option<HIMAGELIST>,
    /// `ThumbBarAddButtons` may only ever be called once per window; after that
    /// it is `ThumbBarUpdateButtons` or nothing.
    added: bool,
    /// Set while a shell call is in flight. Those calls pump messages, so the
    /// window proc can re-enter this module mid-call; without the guard a timer
    /// tick could add the buttons a second time.
    busy: bool,
    retries: u32,
    /// Size/appearance the current image list was rendered for.
    icon_px: u32,
    light_theme: bool,
    state: State,
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let ctx = &*(data as *const Ctx);

    match msg {
        m if m == ctx.taskbar_created => {
            // Either explorer restarted (our toolbar died with the old taskbar
            // button and must be rebuilt) or this is just the startup broadcast
            // arriving after `install` already built it. We cannot tell the two
            // apart, so always re-attempt — `ensure_buttons` is careful never to
            // treat the resulting failure as "we have no toolbar".
            eprintln!("[thumbbar] TaskbarButtonCreated");
            ensure_buttons(ctx, hwnd, true);
        }
        WM_COMMAND => {
            let notify = (wparam.0 >> 16) as u16;
            let id = wparam.0 as u16;
            if notify == THBN_CLICKED {
                eprintln!("[thumbbar] click id={id}");
                if let Some(button) = Button::from_id(id) {
                    (ctx.on_click)(button);
                }
                return LRESULT(0);
            }
            #[cfg(debug_assertions)]
            eprintln!("[thumbbar] other WM_COMMAND notify=0x{notify:04x} id={id}");
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let mut inner = ctx.inner.borrow_mut();
            inner.retries += 1;
            let give_up = inner.retries > 40; // ~20s
            let done = inner.added;
            drop(inner);
            if done || give_up {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
            } else {
                ensure_buttons(ctx, hwnd, false);
                if ctx.inner.borrow().added {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                }
            }
            return LRESULT(0);
        }
        WM_SET_STATE => {
            let state = State::unpack(wparam.0);
            let changed = {
                let mut inner = ctx.inner.borrow_mut();
                let changed = inner.state != state;
                inner.state = state;
                changed
            };
            #[cfg(debug_assertions)]
            eprintln!("[thumbbar] state {state:?} changed={changed}");
            if changed {
                update_buttons(ctx, hwnd);
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => refresh_images(ctx, hwnd),
        // Light/dark switch: the shell does not recolour our icons for us.
        WM_SETTINGCHANGE if is_color_setting(lparam) => refresh_images(ctx, hwnd),
        WM_NCDESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            if let Some(images) = ctx.inner.borrow_mut().images.take() {
                let _ = ImageList_Destroy(Some(images));
            }
            let result = DefSubclassProc(hwnd, msg, wparam, lparam);
            drop(Box::from_raw(data as *mut Ctx));
            return result;
        }
        _ => {}
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn is_color_setting(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return false;
    }
    let mut len = 0usize;
    let ptr = lparam.0 as *const u16;
    while *ptr.add(len) != 0 && len < 64 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)) == "ImmersiveColorSet"
}

// NOTE for everything below: `ITaskbarList3` lives in explorer, so every call
// on it is a cross-apartment COM call, and those PUMP MESSAGES on this STA
// thread. A `RefCell` borrow held across one can therefore be re-entered by our
// own window proc and panic. Each helper reads what it needs, drops the borrow,
// then talks to the shell.

/// Reserve the shell for one call, returning `None` if it is already in use.
unsafe fn claim(ctx: &Ctx) -> Option<ITaskbarList3> {
    {
        let mut inner = ctx.inner.borrow_mut();
        if inner.busy {
            return None;
        }
        if let Some(existing) = inner.taskbar.clone() {
            inner.busy = true;
            return Some(existing);
        }
        inner.busy = true;
    }
    // Creating the object is itself a COM call, so it happens unborrowed.
    let created = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        .ok()
        .filter(|taskbar| taskbar.HrInit().is_ok());
    match created {
        Some(taskbar) => {
            ctx.inner.borrow_mut().taskbar = Some(taskbar.clone());
            Some(taskbar)
        }
        None => {
            ctx.inner.borrow_mut().busy = false;
            None
        }
    }
}

fn release(ctx: &Ctx) {
    ctx.inner.borrow_mut().busy = false;
}

/// Create the toolbar, if the taskbar button exists yet. Safe to call
/// repeatedly. `force` re-attempts even when we believe a toolbar already
/// exists, for the case where explorer restarted underneath us.
unsafe fn ensure_buttons(ctx: &Ctx, hwnd: HWND, force: bool) {
    if !force && ctx.inner.borrow().added {
        return;
    }
    let Some(taskbar) = claim(ctx) else { return };

    let (images, stale, buttons) = {
        let mut inner = ctx.inner.borrow_mut();
        let stale = build_images(&mut inner, hwnd);
        (inner.images, stale, describe(inner.state))
    };
    if let Some(images) = images {
        let _ = taskbar.ThumbBarSetImageList(hwnd, images);
    }
    discard(stale);
    let ok = taskbar.ThumbBarAddButtons(hwnd, &buttons).is_ok();

    let mut inner = ctx.inner.borrow_mut();
    // NEVER clear `added` on failure. ThumbBarAddButtons is once-per-window, so
    // the TaskbarButtonCreated broadcast that lands during normal startup —
    // after `install` has already added the buttons — is *expected* to fail
    // here. Recording that as "no toolbar" is what previously froze the buttons
    // on their first glyphs: every later update bailed out on `!added`.
    inner.added |= ok;
    inner.busy = false;
    eprintln!(
        "[thumbbar] add attempt: ok={ok}, have toolbar={}",
        inner.added
    );
}

unsafe fn update_buttons(ctx: &Ctx, hwnd: HWND) {
    if !ctx.inner.borrow().added {
        eprintln!("[thumbbar] update skipped: no toolbar");
        return;
    }
    let Some(taskbar) = claim(ctx) else {
        eprintln!("[thumbbar] update skipped: shell busy");
        return;
    };
    let buttons = describe(ctx.inner.borrow().state);
    if let Err(e) = taskbar.ThumbBarUpdateButtons(hwnd, &buttons) {
        eprintln!("[thumbbar] update failed: {e}");
    }
    release(ctx);
}

/// Re-render the glyphs for the current DPI / theme and hand the new image list
/// to the shell.
unsafe fn refresh_images(ctx: &Ctx, hwnd: HWND) {
    let Some(taskbar) = claim(ctx) else { return };

    let (images, stale, buttons, added) = {
        let mut inner = ctx.inner.borrow_mut();
        let stale = build_images(&mut inner, hwnd);
        (inner.images, stale, describe(inner.state), inner.added)
    };
    if let Some(images) = images {
        let _ = taskbar.ThumbBarSetImageList(hwnd, images);
    }
    // Only now that the shell has the replacement is the old list safe to free.
    discard(stale);
    if added {
        if let Err(e) = taskbar.ThumbBarUpdateButtons(hwnd, &buttons) {
            eprintln!("[thumbbar] update failed: {e}");
        }
    }
    release(ctx);
}

unsafe fn discard(list: Option<HIMAGELIST>) {
    if let Some(list) = list {
        let _ = ImageList_Destroy(Some(list));
    }
}

// ---------------------------------------------------------------------------
// Button descriptions
// ---------------------------------------------------------------------------

/// Image-list slots, in the order [`SLOTS`] adds them.
mod slot {
    pub const THUMB_DOWN: u32 = 0;
    pub const THUMB_DOWN_ON: u32 = 1;
    pub const REPLAY: u32 = 2;
    pub const PLAY: u32 = 3;
    pub const PAUSE: u32 = 4;
    pub const SKIP: u32 = 5;
    pub const THUMB_UP: u32 = 6;
    pub const THUMB_UP_ON: u32 = 7;
}

/// `(glyph id in index.html, painted as a solid fill?)` per image-list slot.
///
/// The fill/stroke split mirrors `app/src/styles.css`: play, pause and skip are
/// solid; thumbs and replay are 2px round-capped outlines that fill in when the
/// thumb is active.
const SLOTS: &[(&str, bool)] = &[
    ("thumbDown", false),
    ("thumbDown", true),
    ("replay", false),
    ("play-icon", true),
    ("pause-icon", true),
    ("skip", true),
    ("thumbUp", false),
    ("thumbUp", true),
];

fn describe(state: State) -> [THUMBBUTTON; 5] {
    // While a network player owns playback we can only drive its transport, and
    // the thumb state we hold belongs to the (idle) Pandora page — so thumbs are
    // greyed out and drawn unset rather than left looking live but inert. The
    // left button is also a genuine previous-track there, not Pandora's replay.
    let remote = state.remote;
    let spec = |button: Button| -> (u32, &'static str) {
        match button {
            Button::ThumbDown if state.thumb_down && !remote => {
                (slot::THUMB_DOWN_ON, "Thumbs down (set)")
            }
            Button::ThumbDown if remote => (slot::THUMB_DOWN, "Thumbs down (Pandora only)"),
            Button::ThumbDown => (slot::THUMB_DOWN, "Thumbs down"),
            Button::Replay if remote => (slot::REPLAY, "Previous track"),
            Button::Replay => (slot::REPLAY, "Replay"),
            Button::PlayPause if state.playing => (slot::PAUSE, "Pause"),
            Button::PlayPause => (slot::PLAY, "Play"),
            Button::Skip => (slot::SKIP, "Skip"),
            Button::ThumbUp if state.thumb_up && !remote => (slot::THUMB_UP_ON, "Thumbs up (set)"),
            Button::ThumbUp if remote => (slot::THUMB_UP, "Thumbs up (Pandora only)"),
            Button::ThumbUp => (slot::THUMB_UP, "Thumbs up"),
        }
    };

    Button::ALL.map(|button| {
        let (bitmap, tip) = spec(button);
        let inert = remote && matches!(button, Button::ThumbUp | Button::ThumbDown);
        let mut thumb = THUMBBUTTON {
            dwMask: THB_BITMAP | THB_TOOLTIP | THB_FLAGS,
            iId: button as u32,
            iBitmap: bitmap,
            dwFlags: if inert { THBF_DISABLED } else { THBF_ENABLED },
            ..Default::default()
        };
        for (dst, ch) in thumb.szTip.iter_mut().zip(tip.encode_utf16()) {
            *dst = ch;
        }
        thumb
    })
}

// ---------------------------------------------------------------------------
// Glyph rasterisation
// ---------------------------------------------------------------------------

/// Rebuild `inner.images` if the DPI or theme moved (or there is no list yet).
///
/// Returns the list it displaced, if any. The caller must hold onto that until
/// the shell has been handed the replacement — freeing it first would pull the
/// bitmaps out from under a toolbar that is still pointing at them.
#[must_use]
unsafe fn build_images(inner: &mut Inner, hwnd: HWND) -> Option<HIMAGELIST> {
    let dpi = match GetDpiForWindow(hwnd) {
        0 => 96,
        dpi => dpi,
    };
    let px = GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32;
    let light = light_theme();
    if inner.images.is_some() && inner.icon_px == px && inner.light_theme == light {
        return None;
    }

    let height = GetSystemMetricsForDpi(SM_CYSMICON, dpi).max(16) as u32;
    let images = ImageList_Create(px as i32, height as i32, ILC_COLOR32, SLOTS.len() as i32, 0);
    if images.is_invalid() {
        return None;
    }

    // The thumbnail flyout paints on the taskbar's backdrop, and the shell does
    // not tint our bitmaps — so the glyph colour has to follow the system theme
    // or it disappears into the background on one of them.
    let colour = if light { "#1a1a1a" } else { "#ffffff" };
    for (id, filled) in SLOTS {
        let Some(icon) = render_icon(id, *filled, px, height, colour) else {
            let _ = ImageList_Destroy(Some(images));
            return None;
        };
        ImageList_ReplaceIcon(images, -1, icon);
        let _ = DestroyIcon(icon);
    }

    inner.icon_px = px;
    inner.light_theme = light;
    inner.images.replace(images)
}

/// `SystemUsesLightTheme` — the taskbar (and therefore the thumbnail flyout)
/// follows this one, not the app-mode key.
fn light_theme() -> bool {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut kind = REG_VALUE_TYPE::default();
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            windows::core::w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status.is_ok() && value != 0
}

/// Render one glyph to an `HICON` with a real alpha channel.
unsafe fn render_icon(
    id: &str,
    filled: bool,
    width: u32,
    height: u32,
    colour: &str,
) -> Option<HICON> {
    let pixmap = render_pixmap(id, filled, width, height)?;

    // tiny-skia hands back premultiplied RGBA; Windows DIBs want BGRA. For the
    // flat black/white glyphs we draw, premultiplied and straight alpha are the
    // same bytes, so no un-premultiply step is needed.
    let (red, green, blue) = parse_colour(colour);
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for pixel in pixmap.pixels() {
        let alpha = pixel.alpha();
        bgra.extend_from_slice(&[mul(blue, alpha), mul(green, alpha), mul(red, alpha), alpha]);
    }

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut c_void = std::ptr::null_mut();
    let colour_bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    if bits.is_null() {
        let _ = DeleteObject(colour_bitmap.into());
        return None;
    }
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

    // A 32bpp icon takes its transparency from the alpha channel, but
    // CreateIconIndirect still insists on a same-sized mask; an all-zero one
    // means "fully opaque" and lets alpha do the work.
    // Monochrome scanlines are WORD-aligned.
    let mask_stride = (width as usize).div_ceil(16) * 2;
    let mask_bits = vec![0u8; mask_stride * height as usize];
    let mask_bitmap = CreateBitmap(
        width as i32,
        height as i32,
        1,
        1,
        Some(mask_bits.as_ptr() as *const c_void),
    );

    let info = ICONINFO {
        fIcon: true.into(),
        hbmMask: mask_bitmap,
        hbmColor: colour_bitmap,
        ..Default::default()
    };
    let icon = CreateIconIndirect(&info).ok();

    let _ = DeleteObject(colour_bitmap.into());
    let _ = DeleteObject(mask_bitmap.into());
    icon
}

fn mul(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
}

fn parse_colour(colour: &str) -> (u8, u8, u8) {
    let hex = colour.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(255);
    (byte(0), byte(2), byte(4))
}

/// Rasterise the transport glyph `id` into an alpha-only white pixmap.
fn render_pixmap(
    id: &str,
    filled: bool,
    width: u32,
    height: u32,
) -> Option<resvg::tiny_skia::Pixmap> {
    let inner = GLYPHS.iter().find(|(key, _)| *key == id).map(|(_, v)| *v)?;

    // The artwork is drawn on a 24-unit grid whose strokes touch the edges, so
    // the viewBox is opened up by one unit to keep round caps from clipping.
    // Thin outlines also need a nudge to survive being scaled down to 16px.
    let stroke = f32::max(2.0, 1.6 * 26.0 / width as f32);
    let (fill, line) = if filled {
        ("#fff", "none")
    } else {
        ("none", "#fff")
    };
    let document = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-1 -1 26 26" width="{width}" height="{height}" fill="{fill}" stroke="{line}" stroke-width="{stroke}" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    );

    let tree = resvg::usvg::Tree::from_str(&document, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip(button: &THUMBBUTTON) -> String {
        let end = button.szTip.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&button.szTip[..end])
    }

    /// A network player can only be driven for transport, so the Pandora-only
    /// buttons must go inert rather than sit there looking clickable.
    #[test]
    fn remote_mode_disables_pandora_only_buttons() {
        let local = describe(State {
            playing: true,
            thumb_up: true,
            ..Default::default()
        });
        assert_eq!(local[4].dwFlags, THBF_ENABLED);
        assert_eq!(local[4].iBitmap, slot::THUMB_UP_ON);
        assert_eq!(tip(&local[1]), "Replay");

        let remote = describe(State {
            playing: true,
            thumb_up: true,
            remote: true,
            ..Default::default()
        });
        for i in [0, 4] {
            assert_eq!(
                remote[i].dwFlags, THBF_DISABLED,
                "thumb {i} should be inert"
            );
        }
        // A stale local thumb must not read as "set" against a remote track.
        assert_eq!(remote[4].iBitmap, slot::THUMB_UP);
        // Replay is a real previous-track on a renderer.
        assert_eq!(tip(&remote[1]), "Previous track");
        // Transport stays live.
        for i in [1, 2, 3] {
            assert_eq!(remote[i].dwFlags, THBF_ENABLED, "transport {i} stays live");
        }
        assert_eq!(remote[2].iBitmap, slot::PAUSE);
    }

    /// Renders every glyph as ASCII art so the shapes can be eyeballed without
    /// launching the app — a blank or clipped icon is obvious here.
    #[test]
    fn glyphs_rasterise() {
        for size in [16u32, 24, 32] {
            for (id, filled) in SLOTS {
                let pixmap = render_pixmap(id, *filled, size, size)
                    .unwrap_or_else(|| panic!("{id} (filled={filled}) failed to render"));
                let ink: u32 = pixmap.pixels().iter().map(|p| p.alpha() as u32).sum();
                assert!(ink > 0, "{id} (filled={filled}) rendered blank at {size}px");

                if size == 32 {
                    println!("\n{id} filled={filled}");
                    for y in 0..size {
                        let row: String = (0..size)
                            .map(|x| {
                                let a = pixmap.pixels()[(y * size + x) as usize].alpha();
                                match a {
                                    0..=31 => ' ',
                                    32..=127 => '.',
                                    128..=207 => '+',
                                    _ => '#',
                                }
                            })
                            .collect();
                        println!("|{row}|");
                    }
                }
            }
        }
    }
}
