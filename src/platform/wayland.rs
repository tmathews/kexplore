//! Hand-rolled Wayland client layer: registry, xdg-shell window with
//! server-side decorations, pointer input, cursor, and output scale tracking
//! (fractional via wp-fractional-scale-v1 + viewporter, with integer
//! fallbacks). Port of the C app's klib/ layer.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::{
    delegate_noop, event_created_child,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
        wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_surface,
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use xkbcommon::xkb;
use wayland_cursor::CursorTheme;
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1, zwp_pointer_gestures_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

pub const BTN_LEFT: u32 = 272;
pub const BTN_RIGHT: u32 = 273;
pub const BTN_MIDDLE: u32 = 274;

/// MIME types we advertise when copying, and accept when pasting, in order of
/// preference. All are plain UTF-8 text.
const TEXT_MIMES: [&str; 4] =
    ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING"];
/// MIME advertised for a "copy file" so file managers can paste the file.
const URI_LIST_MIME: &str = "text/uri-list";

fn is_text_mime(m: &str) -> bool {
    TEXT_MIMES.contains(&m) || m == "TEXT"
}

/// Build a `file://` URI from an absolute path, percent-encoding bytes that
/// aren't URI-safe (RFC 3986 unreserved plus `/`).
fn path_to_file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

#[derive(Clone, Copy, Default)]
pub struct PointerState {
    pub x: f64,
    pub y: f64,
    pub is_down: bool,
    pub pressed: bool,
    pub released: bool,
    /// Middle button: held / just-pressed this iteration (canvas pan).
    pub middle_down: bool,
    pub middle_pressed: bool,
    /// Right button pressed this iteration (opens the context menu).
    pub right_pressed: bool,
    /// Accumulated scroll this iteration (positive dy = down, dx = right).
    pub scroll_dx: f64,
    pub scroll_dy: f64,
    /// True when this iteration's scroll came from a touchpad (2-finger)
    /// rather than a mouse wheel.
    pub scroll_finger: bool,
    /// Net multiplicative zoom from a pinch gesture this iteration; 0.0 means
    /// no pinch (a real factor is >0, e.g. 1.05 to zoom in a little).
    pub pinch: f64,
}

/// A resolved key press (or repeat) ready for the UI to consume.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub keysym: xkb::Keysym,
    /// Committed text for this key (empty for e.g. arrows).
    pub utf8: String,
    pub ctrl: bool,
    pub shift: bool,
}

struct RepeatState {
    /// evdev keycode currently held (xkb keycode - 8)
    key: u32,
    next: Instant,
}

struct Keyboard {
    _kbd: wl_keyboard::WlKeyboard,
    context: xkb::Context,
    state: Option<xkb::State>,
    /// repeats per second / initial delay, from wl_keyboard.repeat_info
    repeat_rate: u32,
    repeat_delay_ms: u32,
    repeat: Option<RepeatState>,
}

pub struct Platform {
    pub surface: wl_surface::WlSurface,
    /// Kept alive for the protocol objects' lifetimes.
    _xdg_surface: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    viewport: Option<wp_viewport::WpViewport>,
    _fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    _decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pointer: Option<wl_pointer::WlPointer>,
    gestures: Option<zwp_pointer_gestures_v1::ZwpPointerGesturesV1>,
    _pinch: Option<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1>,
    /// Axis source of the scroll frame currently being assembled (finger vs
    /// wheel), and the last absolute pinch scale (updates are relative to it).
    scroll_source_finger: bool,
    pinch_last_scale: f64,
    keyboard: Option<Keyboard>,
    /// Resolved key presses/repeats, drained by the main loop.
    pub key_events: Vec<KeyEvent>,

    // Native clipboard (wl_data_device_manager). The seat is kept alive for the
    // data device's lifetime.
    _seat: wl_seat::WlSeat,
    data_device_mgr: Option<wl_data_device_manager::WlDataDeviceManager>,
    data_device: Option<wl_data_device::WlDataDevice>,
    /// Latest input-event serial, needed to take the selection.
    last_serial: u32,
    /// Text we currently own on the clipboard, written on demand when another
    /// client pastes; kept alive alongside `data_source`.
    clipboard: Option<String>,
    /// When we own a "copy file" selection, the `text/uri-list` payload served
    /// to file managers (the path is still served as plain text).
    clipboard_uris: Option<String>,
    data_source: Option<wl_data_source::WlDataSource>,
    /// MIME types announced by the offer currently being described (before its
    /// `selection` event promotes it to the active one).
    pending_mimes: Vec<String>,
    /// The current clipboard offer from the compositor and its MIME types.
    selection_offer: Option<wl_data_offer::WlDataOffer>,
    selection_mimes: Vec<String>,
    cursor_theme: Option<CursorTheme>,
    cursor_surface: Option<wl_surface::WlSurface>,
    outputs: Vec<(wl_output::WlOutput, i32)>,

    pub running: bool,
    pub configured: bool,
    pub logical_size: (u32, u32),
    pending_size: Option<(u32, u32)>,
    pub resized: bool,

    fractional_scale: Option<f64>,
    preferred_buffer_scale: Option<i32>,
    output_scale: i32,
    applied_buffer_scale: i32,
    pub scale_changed: bool,

    pub pointer_state: PointerState,
}

pub struct Init {
    pub conn: Connection,
    pub queue: EventQueue<Platform>,
    pub platform: Platform,
}

pub fn init(title: &str, app_id: &str, logical: (u32, u32)) -> Result<Init, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Platform>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
    let qh = queue.handle();

    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|e| format!("wl_compositor: {e}"))?;
    let wm_base: xdg_wm_base::XdgWmBase =
        globals.bind(&qh, 1..=6, ()).map_err(|e| format!("xdg_wm_base: {e}"))?;
    let seat: wl_seat::WlSeat =
        globals.bind(&qh, 1..=7, ()).map_err(|e| format!("wl_seat: {e}"))?;
    // Core clipboard protocol (the thing wl-copy/wl-paste wrap). Optional so a
    // stripped compositor still runs; copy/paste just no-op without it.
    let data_device_mgr: Option<wl_data_device_manager::WlDataDeviceManager> =
        globals.bind(&qh, 1..=3, ()).ok();
    let data_device = data_device_mgr.as_ref().map(|m| m.get_data_device(&seat, &qh, ()));
    let shm: Option<wl_shm::WlShm> = globals.bind(&qh, 1..=1, ()).ok();
    let deco_mgr: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let fs_mgr: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let gestures: Option<zwp_pointer_gestures_v1::ZwpPointerGesturesV1> =
        globals.bind(&qh, 1..=3, ()).ok();

    // Bind every output so we can track integer scales for the fallback chain.
    let mut outputs = Vec::new();
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            let output: wl_output::WlOutput =
                globals.registry().bind(global.name, global.version.min(4), &qh, ());
            outputs.push((output, 1));
        }
    }

    let surface = compositor.create_surface(&qh, ());
    let fractional = fs_mgr.map(|m| m.get_fractional_scale(&surface, &qh, ()));
    let viewport = viewporter.map(|v| v.get_viewport(&surface, &qh, ()));
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title(title.to_string());
    toplevel.set_app_id(app_id.to_string());
    // The C app passed mode 1 (client-side) by mistake; we actually want the
    // compositor to decorate us.
    let decoration = deco_mgr.map(|m| {
        let d = m.get_toplevel_decoration(&toplevel, &qh, ());
        d.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        d
    });
    surface.commit();

    let cursor_theme = shm.and_then(|shm| CursorTheme::load(&conn, shm, 24).ok());
    let cursor_surface = cursor_theme.as_ref().map(|_| compositor.create_surface(&qh, ()));

    let mut platform = Platform {
        surface,
        _xdg_surface: xdg_surface,
        _toplevel: toplevel,
        viewport,
        _fractional: fractional,
        _decoration: decoration,
        pointer: None,
        gestures,
        _pinch: None,
        scroll_source_finger: false,
        pinch_last_scale: 1.0,
        keyboard: None,
        key_events: Vec::new(),
        _seat: seat,
        data_device_mgr,
        data_device,
        last_serial: 0,
        clipboard: None,
        clipboard_uris: None,
        data_source: None,
        pending_mimes: Vec::new(),
        selection_offer: None,
        selection_mimes: Vec::new(),
        cursor_theme,
        cursor_surface,
        outputs,
        running: true,
        configured: false,
        logical_size: logical,
        pending_size: None,
        resized: false,
        fractional_scale: None,
        preferred_buffer_scale: None,
        output_scale: 1,
        applied_buffer_scale: 1,
        scale_changed: false,
        pointer_state: PointerState::default(),
    };

    while !platform.configured {
        queue
            .blocking_dispatch(&mut platform)
            .map_err(|e| format!("wayland dispatch: {e}"))?;
    }
    platform.apply_scale();

    Ok(Init { conn, queue, platform })
}

impl Platform {
    /// Effective output scale: fractional protocol wins, then the
    /// compositor's preferred integer buffer scale, then the scale of the
    /// output the surface is on.
    pub fn scale(&self) -> f64 {
        if let Some(s) = self.fractional_scale {
            return s;
        }
        self.preferred_buffer_scale.unwrap_or(self.output_scale).max(1) as f64
    }

    /// Swapchain size in physical pixels for the current scale.
    pub fn physical_extent(&self) -> (u32, u32) {
        let (w, h) = self.logical_size;
        if self.viewport.is_some() {
            let s = self.scale();
            (
                ((w as f64 * s).round() as u32).max(1),
                ((h as f64 * s).round() as u32).max(1),
            )
        } else {
            let s = self.scale() as u32;
            (w * s, h * s)
        }
    }

    /// Push the current scale/size mapping to the compositor. With a
    /// viewport, the buffer (any size) is mapped onto the logical window
    /// size 1:1; without one we can only do integer buffer scales.
    pub fn apply_scale(&mut self) {
        if let Some(vp) = &self.viewport {
            let (w, h) = self.logical_size;
            vp.set_destination(w as i32, h as i32);
        } else {
            let s = self.scale() as i32;
            if s != self.applied_buffer_scale {
                self.surface.set_buffer_scale(s);
                self.applied_buffer_scale = s;
            }
        }
    }

    /// Clear per-iteration edge flags after input has been processed.
    pub fn end_input_frame(&mut self) {
        self.pointer_state.pressed = false;
        self.pointer_state.released = false;
        self.pointer_state.middle_pressed = false;
        self.pointer_state.right_pressed = false;
        self.pointer_state.scroll_dx = 0.0;
        self.pointer_state.scroll_dy = 0.0;
        self.pointer_state.scroll_finger = false;
        self.pointer_state.pinch = 0.0;
    }

    /// When the next key repeat is due, if a repeating key is held.
    pub fn next_repeat_deadline(&self) -> Option<Instant> {
        self.keyboard.as_ref().and_then(|k| k.repeat.as_ref()).map(|r| r.next)
    }

    /// Take ownership of the clipboard selection, offering `text` as UTF-8 to
    /// any client that pastes. No-op if the compositor lacks the data-device
    /// protocol. Replaces our previous selection, if any.
    pub fn set_clipboard(&mut self, qh: &QueueHandle<Self>, text: String) {
        let (Some(mgr), Some(dd)) = (&self.data_device_mgr, &self.data_device) else {
            return;
        };
        let source = mgr.create_data_source(qh, ());
        for m in TEXT_MIMES {
            source.offer(m.to_string());
        }
        dd.set_selection(Some(&source), self.last_serial);
        self.clipboard = Some(text);
        self.clipboard_uris = None;
        self.data_source = Some(source);
    }

    /// Take ownership of the clipboard as a "copy file": offers a
    /// `text/uri-list` (so file managers paste the file) plus the path as plain
    /// text. No-op if the data-device protocol is unavailable.
    pub fn set_clipboard_file(&mut self, qh: &QueueHandle<Self>, path: &std::path::Path) {
        let (Some(mgr), Some(dd)) = (&self.data_device_mgr, &self.data_device) else {
            return;
        };
        let uri = path_to_file_uri(path);
        let source = mgr.create_data_source(qh, ());
        source.offer(URI_LIST_MIME.to_string());
        for m in TEXT_MIMES {
            source.offer(m.to_string());
        }
        dd.set_selection(Some(&source), self.last_serial);
        // uri-list entries are CRLF-terminated per RFC 2483.
        self.clipboard_uris = Some(format!("{uri}\r\n"));
        self.clipboard = Some(path.to_string_lossy().into_owned());
        self.data_source = Some(source);
    }

    /// Read the clipboard selection as text. If we own it, return our copy
    /// directly; otherwise pipe it from the owning client (a brief blocking
    /// read — clipboard payloads here are tiny). None if nothing text-like is
    /// offered or the protocol is unavailable.
    pub fn clipboard_text(&mut self, conn: &Connection) -> Option<String> {
        // We own the selection: hand back our copy (avoids a round-trip and a
        // self-write deadlock, since our own Send would need this queue).
        if self.data_source.is_some() {
            return self.clipboard.clone();
        }
        let offer = self.selection_offer.clone()?;
        let mime = self.selection_mimes.iter().find(|m| is_text_mime(m))?.clone();
        let (mut reader, writer) = std::io::pipe().ok()?;
        offer.receive(mime, writer.as_fd());
        drop(writer); // close our write end so the read hits EOF
        conn.flush().ok()?;
        let mut buf = String::new();
        reader.read_to_string(&mut buf).ok()?;
        buf.retain(|c| c != '\0');
        Some(buf)
    }

    /// Emit synthetic key events for any repeats that have come due.
    pub fn process_key_repeats(&mut self, now: Instant) {
        let Some(kb) = self.keyboard.as_mut() else { return };
        if kb.repeat_rate == 0 {
            return;
        }
        let interval = Duration::from_micros(1_000_000 / kb.repeat_rate.max(1) as u64);
        let mut events = Vec::new();
        if let (Some(rep), Some(state)) = (kb.repeat.as_mut(), kb.state.as_ref()) {
            while rep.next <= now {
                events.push(resolve_key(state, rep.key));
                rep.next += interval;
            }
        }
        self.key_events.extend(events);
    }

    fn set_cursor(&mut self, serial: u32) {
        let (Some(theme), Some(csurf), Some(pointer)) =
            (self.cursor_theme.as_mut(), self.cursor_surface.as_ref(), self.pointer.as_ref())
        else {
            return;
        };
        let Some(cursor) = theme.get_cursor("left_ptr") else { return };
        let img = &cursor[0];
        let (hx, hy) = img.hotspot();
        csurf.attach(Some(img), 0, 0);
        csurf.damage(0, 0, i32::MAX, i32::MAX);
        csurf.commit();
        pointer.set_cursor(serial, Some(csurf), hx as i32, hy as i32);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Platform {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Dynamic global add/remove (e.g. hotplugged outputs) is ignored,
        // matching the C app.
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Platform {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Platform {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configured = true;
            if let Some(size) = state.pending_size.take() {
                if size != state.logical_size {
                    state.logical_size = size;
                    state.resized = true;
                }
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    state.pending_size = Some((width as u32, height as u32));
                }
            }
            xdg_toplevel::Event::Close => state.running = false,
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Platform {
    fn event(
        state: &mut Self,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Cursor surface events are irrelevant.
        if Some(surface) == state.cursor_surface.as_ref() {
            return;
        }
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some((_, s)) = state.outputs.iter().find(|(o, _)| *o == output) {
                    if state.output_scale != *s {
                        state.output_scale = *s;
                        state.scale_changed = true;
                    }
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                if state.preferred_buffer_scale != Some(factor) {
                    state.preferred_buffer_scale = Some(factor);
                    state.scale_changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Platform {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Scale { factor } = event {
            if let Some(entry) = state.outputs.iter_mut().find(|(o, _)| o == output) {
                entry.1 = factor;
            }
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let s = scale as f64 / 120.0;
            if state.fractional_scale != Some(s) {
                state.fractional_scale = Some(s);
                state.scale_changed = true;
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Platform {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                let pointer = seat.get_pointer(qh, ());
                // Touchpad pinch-to-zoom, if the compositor supports gestures.
                if let Some(g) = &state.gestures {
                    state._pinch = Some(g.get_pinch_gesture(&pointer, qh, ()));
                }
                state.pointer = Some(pointer);
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(Keyboard {
                    _kbd: seat.get_keyboard(qh, ()),
                    context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
                    state: None,
                    repeat_rate: 25,
                    repeat_delay_ms: 400,
                    repeat: None,
                });
            }
        }
    }
}

fn resolve_key(state: &xkb::State, key: u32) -> KeyEvent {
    let keycode = xkb::Keycode::new(key + 8);
    let keysym = state.key_get_one_sym(keycode);
    let mut utf8 = state.key_get_utf8(keycode);
    // Strip control characters (Return, Escape, ^C...) from committed text.
    utf8.retain(|c| !c.is_control());
    KeyEvent {
        keysym,
        utf8,
        ctrl: state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE),
        shift: state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE),
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(kb) = state.keyboard.as_mut() else { return };
        match event {
            wl_keyboard::Event::Keymap { format: WEnum::Value(format), fd, size } => {
                if format == wl_keyboard::KeymapFormat::XkbV1 {
                    let keymap = unsafe {
                        xkb::Keymap::new_from_fd(
                            &kb.context,
                            fd,
                            size as usize,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    };
                    if let Ok(Some(keymap)) = keymap {
                        kb.state = Some(xkb::State::new(&keymap));
                    }
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                kb.repeat_rate = rate.max(0) as u32;
                kb.repeat_delay_ms = delay.max(0) as u32;
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed, mods_latched, mods_locked, group, ..
            } => {
                if let Some(s) = kb.state.as_mut() {
                    s.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key {
                serial, key, state: WEnum::Value(key_state), ..
            } => {
                state.last_serial = serial;
                let Some(s) = kb.state.as_ref() else { return };
                match key_state {
                    wl_keyboard::KeyState::Pressed => {
                        state.key_events.push(resolve_key(s, key));
                        let keymap = s.get_keymap();
                        if kb.repeat_rate > 0
                            && keymap.key_repeats(xkb::Keycode::new(key + 8))
                        {
                            kb.repeat = Some(RepeatState {
                                key,
                                next: Instant::now()
                                    + Duration::from_millis(kb.repeat_delay_ms as u64),
                            });
                        }
                    }
                    wl_keyboard::KeyState::Released => {
                        if kb.repeat.as_ref().is_some_and(|r| r.key == key) {
                            kb.repeat = None;
                        }
                    }
                    _ => {}
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                kb.repeat = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, surface_x, surface_y, .. } => {
                state.pointer_state.x = surface_x;
                state.pointer_state.y = surface_y;
                state.set_cursor(serial);
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                state.pointer_state.x = surface_x;
                state.pointer_state.y = surface_y;
            }
            wl_pointer::Event::Button { serial, button, state: btn_state, .. } => {
                state.last_serial = serial;
                if button == BTN_LEFT {
                    match btn_state {
                        WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                            state.pointer_state.is_down = true;
                            state.pointer_state.pressed = true;
                        }
                        WEnum::Value(wl_pointer::ButtonState::Released) => {
                            state.pointer_state.is_down = false;
                            state.pointer_state.released = true;
                        }
                        _ => {}
                    }
                } else if button == BTN_MIDDLE {
                    match btn_state {
                        WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                            state.pointer_state.middle_down = true;
                            state.pointer_state.middle_pressed = true;
                        }
                        WEnum::Value(wl_pointer::ButtonState::Released) => {
                            state.pointer_state.middle_down = false;
                        }
                        _ => {}
                    }
                } else if button == BTN_RIGHT {
                    if let WEnum::Value(wl_pointer::ButtonState::Pressed) = btn_state {
                        state.pointer_state.right_pressed = true;
                    }
                }
            }
            // AxisSource precedes the Axis events of a scroll frame; it tells
            // us whether the scroll is a mouse wheel or a touchpad 2-finger
            // swipe (which we pan with, not zoom).
            wl_pointer::Event::AxisSource { axis_source: WEnum::Value(src) } => {
                state.scroll_source_finger =
                    matches!(src, wl_pointer::AxisSource::Finger | wl_pointer::AxisSource::Continuous);
            }
            wl_pointer::Event::Axis { axis: WEnum::Value(axis), value, .. } => {
                match axis {
                    wl_pointer::Axis::VerticalScroll => state.pointer_state.scroll_dy += value,
                    wl_pointer::Axis::HorizontalScroll => state.pointer_state.scroll_dx += value,
                    _ => {}
                }
                state.pointer_state.scroll_finger = state.scroll_source_finger;
            }
            // Reset the source at each frame boundary so a scroll with no
            // AxisSource (or the next wheel after a touchpad) defaults to wheel
            // rather than reusing the previous frame's source.
            wl_pointer::Event::Frame => {
                state.scroll_source_finger = false;
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_state.is_down = false;
                state.pointer_state.middle_down = false;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_pointer_gesture_pinch_v1::Event;
        match event {
            // `scale` is the absolute scale since the gesture began (1.0 at
            // start); accumulate the per-update factor into this frame's pinch.
            Event::Begin { .. } | Event::End { .. } => state.pinch_last_scale = 1.0,
            Event::Update { scale, .. } => {
                if state.pinch_last_scale > 0.0 && scale > 0.0 {
                    let factor = scale / state.pinch_last_scale;
                    let acc = &mut state.pointer_state.pinch;
                    *acc = if *acc == 0.0 { factor } else { *acc * factor };
                }
                state.pinch_last_scale = scale;
            }
            _ => {}
        }
    }
}

// Clipboard: the data device delivers incoming selection offers. A new offer
// arrives as DataOffer (its MIME types then stream in via wl_data_offer), and
// Selection promotes one (or None) to the active clipboard.
impl Dispatch<wl_data_device::WlDataDevice, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { .. } => {
                // A fresh offer begins; collect its MIME types until Selection.
                state.pending_mimes.clear();
            }
            wl_data_device::Event::Selection { id } => {
                // Replace the active offer; destroy the previous to free it.
                if let Some(old) = state.selection_offer.take() {
                    old.destroy();
                }
                match id {
                    Some(offer) => {
                        state.selection_mimes = std::mem::take(&mut state.pending_mimes);
                        state.selection_offer = Some(offer);
                    }
                    None => {
                        state.selection_mimes.clear();
                        state.pending_mimes.clear();
                    }
                }
            }
            _ => {} // DnD (Enter/Leave/Motion/Drop) — unused.
        }
    }
    // The data_offer event creates a child wl_data_offer object; the sys
    // backend needs this to know how to construct it (with `()` user data).
    event_created_child!(Platform, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.pending_mimes.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for Platform {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // Another client is pasting: write our text to the pipe fd. The
            // payload is small (paths/URLs), so a blocking write is fine.
            wl_data_source::Event::Send { mime_type, fd } => {
                let payload = if mime_type == URI_LIST_MIME {
                    state.clipboard_uris.as_deref()
                } else if is_text_mime(&mime_type) {
                    state.clipboard.as_deref()
                } else {
                    None
                };
                if let Some(payload) = payload {
                    let mut file = std::fs::File::from(fd);
                    let _ = file.write_all(payload.as_bytes());
                }
            }
            // We lost ownership (someone else took the selection). Drop our copy
            // if this was still the live source.
            wl_data_source::Event::Cancelled => {
                if state.data_source.as_ref() == Some(source) {
                    state.data_source = None;
                    state.clipboard = None;
                    state.clipboard_uris = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

delegate_noop!(Platform: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(Platform: ignore zwp_pointer_gestures_v1::ZwpPointerGesturesV1);
delegate_noop!(Platform: ignore wl_compositor::WlCompositor);
delegate_noop!(Platform: ignore wl_shm::WlShm);
delegate_noop!(Platform: ignore wp_viewporter::WpViewporter);
delegate_noop!(Platform: ignore wp_viewport::WpViewport);
delegate_noop!(Platform: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(Platform: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Platform: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
