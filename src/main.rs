mod geom;
mod gfx;
mod handlers;
mod model;
mod platform;
mod preview;
mod tasks;
mod text;
mod textfield;
mod ui;

use std::ffi::OsStr;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ash::vk;
use geom::Point;
use gfx::renderer2d::{DrawList, Renderer2d, TexSets};
use gfx::upload::{self, PendingUpload, Texture};
use model::{NodeArena, NodeId};
use platform::wayland::KeyEvent;
use preview::Preview;
use tasks::{TaskResult, Tasks};
use text::TextSystem;
use textfield::TextField;
use ui::{Action, Ui};
use wayland_client::Proxy;
use xkbcommon::xkb::keysyms as ks;

const CARET_BLINK_MS: u64 = 530;

/// The current preview texture plus textures waiting out their last
/// possibly-in-flight frames before destruction.
struct PreviewTex {
    current: Option<(Texture, vk::DescriptorSet, (u32, u32))>,
    retired: Vec<(u64, Texture, vk::DescriptorSet)>,
}

impl PreviewTex {
    fn retire_current(&mut self, frame_counter: u64) {
        if let Some((tex, set, _)) = self.current.take() {
            self.retired.push((frame_counter, tex, set));
        }
    }

    fn destroy_old(&mut self, device: &ash::Device, renderer: &Renderer2d, frame_counter: u64) {
        self.retired.retain(|(at, tex, set)| {
            if frame_counter.saturating_sub(*at) > gfx::FRAMES_IN_FLIGHT as u64 {
                unsafe { upload::destroy_texture(device, tex) };
                renderer.free_texture_set(device, *set);
                false
            } else {
                true
            }
        });
    }
}

fn main() {
    let platform::wayland::Init { conn, mut queue, mut platform } =
        platform::wayland::init("Kallos Explore", "kallos-explore", (600, 440))
            .unwrap_or_else(|e| {
                eprintln!("failed to init wayland: {e}");
                std::process::exit(1);
            });

    let display_ptr = conn.backend().display_ptr() as *mut std::ffi::c_void;
    let surface_ptr = platform.surface.id().as_ptr() as *mut std::ffi::c_void;
    let band_h = |scale: f64| (ui::TOOLBAR_H as f64 * scale).round().max(1.0) as u32;
    let mut gfx = gfx::Gfx::new(
        display_ptr,
        surface_ptr,
        platform.physical_extent(),
        band_h(platform.scale()),
    )
    .unwrap_or_else(|e| {
        eprintln!("failed to init vulkan: {e}");
        std::process::exit(1);
    });
    let mut renderer = Renderer2d::new(&gfx.device, gfx.mem_props, gfx.swapchain.format)
        .unwrap_or_else(|e| {
            eprintln!("failed to init renderer: {e}");
            std::process::exit(1);
        });
    let mut ts = TextSystem::new(&gfx.device, &gfx.mem_props, platform.scale() as f32)
        .unwrap_or_else(|e| {
            eprintln!("failed to init text system: {e}");
            std::process::exit(1);
        });
    let atlas_set = renderer.register_texture(&gfx.device, &ts.texture).expect("atlas set");
    let mut scene_set = renderer.register_texture(&gfx.device, &gfx.blur.scene).expect("scene set");
    let mut blur_set = renderer.register_texture(&gfx.device, &gfx.blur.blur_b).expect("blur set");

    // Root node: the user's home directory, like the C app.
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let file_handlers = handlers::read_handlers(&home.join(".config/kallos/handlers"));
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut arena = NodeArena::new();
    let root_items = model::scan_dir(&home).unwrap_or_else(|e| {
        eprintln!("failed to load node: {e}");
        std::process::exit(1);
    });
    let mut root = arena.insert(model::node_from_items(home, root_items));
    model::calc_size(&mut arena, root, &mut ts);

    let mut ui = Ui::new();
    if let Some(node) = arena.get(root) {
        let r = node.rect;
        ui.focus_to_rect(r, Point::new(600.0, 440.0));
    }

    let tasks = Tasks::new().unwrap_or_else(|e| {
        eprintln!("failed to init task system: {e}");
        std::process::exit(1);
    });
    let preview = Preview::new(&tasks);
    let mut ptex = PreviewTex { current: None, retired: Vec::new() };
    let mut extra_uploads: Vec<PendingUpload> = Vec::new();
    let mut frame_counter: u64 = 0;

    let start = Instant::now();
    let mut last_frame = Instant::now();
    let mut dirty = true;
    let mut animating = true;
    let mut blink_epoch = Instant::now();
    let mut last_caret_visible = true;

    while platform.running {
        queue.flush().ok();
        queue.dispatch_pending(&mut platform).expect("wayland dispatch");
        if let Some(guard) = queue.prepare_read() {
            let wl_fd = guard.connection_fd().as_raw_fd();
            let timeout: i32 = {
                let mut t: i64 = if dirty || animating {
                    0
                } else if children.is_empty() {
                    -1
                } else {
                    2000 // wake periodically to reap spawned children
                };
                let now = Instant::now();
                let consider = |deadline: Instant, t: &mut i64| {
                    let ms = deadline.saturating_duration_since(now).as_millis() as i64;
                    if *t < 0 || ms < *t {
                        *t = ms;
                    }
                };
                if let Some(d) = platform.next_repeat_deadline() {
                    consider(d, &mut t);
                }
                if ui.url.active {
                    // Wake for the next caret blink toggle.
                    let elapsed = now.duration_since(blink_epoch).as_millis() as u64;
                    let to_next = CARET_BLINK_MS - (elapsed % CARET_BLINK_MS);
                    consider(now + Duration::from_millis(to_next), &mut t);
                }
                t.clamp(-1, i32::MAX as i64) as i32
            };
            let mut fds = [
                libc::pollfd { fd: wl_fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: tasks.wake_read, events: libc::POLLIN, revents: 0 },
            ];
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as u64, timeout) };
            if ret > 0 && fds[0].revents & libc::POLLIN != 0 {
                guard.read().ok();
            } else {
                drop(guard);
            }
            queue.dispatch_pending(&mut platform).expect("wayland dispatch");
        }
        if !platform.running {
            break;
        }

        handlers::reap(&mut children);
        tasks.drain_wake();
        while let Ok(result) = tasks.rx.try_recv() {
            match result {
                TaskResult::PreviewDone { gen, image } => {
                    // Stale generations are dropped: the shown preview always
                    // matches the latest selection.
                    if gen == preview.current_gen() {
                        ptex.retire_current(frame_counter);
                        if let Some(img) = image {
                            match make_preview_texture(&gfx, &renderer, &img) {
                                Ok((tex, set)) => {
                                    extra_uploads.push(PendingUpload {
                                        texture_image: tex.image,
                                        bytes: img.rgba,
                                        x: 0,
                                        y: 0,
                                        width: img.width,
                                        height: img.height,
                                        initialized: false,
                                    });
                                    ptex.current = Some((tex, set, (img.width, img.height)));
                                }
                                Err(e) => eprintln!("preview texture failed: {e}"),
                            }
                        }
                    }
                }
                other => apply_task_result(other, &mut arena, &mut ui, &mut ts, &platform),
            }
            dirty = true;
        }

        if platform.resized || platform.scale_changed {
            platform.resized = false;
            platform.scale_changed = false;
            platform.apply_scale();
            ts.set_scale(platform.scale() as f32);
            if let Err(e) =
                gfx.recreate_swapchain(platform.physical_extent(), band_h(platform.scale()))
            {
                eprintln!("swapchain recreate failed: {e}");
                break;
            }
            // The offscreen targets were recreated; point the overlay's
            // descriptor sets at the new views.
            renderer.free_texture_set(&gfx.device, scene_set);
            renderer.free_texture_set(&gfx.device, blur_set);
            scene_set =
                renderer.register_texture(&gfx.device, &gfx.blur.scene).expect("scene set");
            blur_set =
                renderer.register_texture(&gfx.device, &gfx.blur.blur_b).expect("blur set");
            dirty = true;
        }

        let (lw, lh) = platform.logical_size;
        let window = Point::new(lw as f32, lh as f32);

        // Keyboard: repeats + URL bar editing.
        platform.process_key_repeats(Instant::now());
        let key_events: Vec<KeyEvent> = platform.key_events.drain(..).collect();
        if ui.url.active {
            for ev in &key_events {
                match handle_url_key(ev, &mut ui.url) {
                    UrlOutcome::None => {}
                    UrlOutcome::Edited => {
                        blink_epoch = Instant::now();
                        dirty = true;
                    }
                    UrlOutcome::Cancel => dirty = true,
                    UrlOutcome::Copy => {
                        if let Some(t) = ui.url.selected_text() {
                            clipboard_copy(t, &mut children);
                        }
                    }
                    UrlOutcome::Cut => {
                        if let Some(t) = ui.url.selected_text().map(str::to_owned) {
                            clipboard_copy(&t, &mut children);
                            ui.url.delete_selection();
                            blink_epoch = Instant::now();
                            dirty = true;
                        }
                    }
                    UrlOutcome::Paste => {
                        if let Some(t) = clipboard_paste() {
                            ui.url.insert(&t);
                            blink_epoch = Instant::now();
                            dirty = true;
                        }
                    }
                    UrlOutcome::Commit(text) => {
                        let path = PathBuf::from(expand_home(&text));
                        if path.is_dir()
                            && reroot(&mut arena, &mut ui, &mut ts, &mut root, path, window)
                        {
                            ui.url.cancel();
                            preview.cancel();
                            ptex.retire_current(frame_counter);
                        }
                        // Invalid path: stay in the editor so it can be fixed.
                        dirty = true;
                    }
                }
            }
        }

        // Click outside the URL bar drops editing (back to selection display).
        if platform.pointer_state.pressed && ui.url.active {
            let cur = Point::new(platform.pointer_state.x as f32, platform.pointer_state.y as f32);
            if !ui.url_bar_rect.contains(cur) {
                ui.url.cancel();
                dirty = true;
            }
        }

        let (action, input_dirty) = ui.process_input(&mut arena, &platform.pointer_state);
        dirty |= input_dirty;
        platform.end_input_frame();
        if let Some(action) = action {
            if matches!(action, Action::UrlBar) {
                // Focus the field (seeding it with the selected path) and
                // put the caret at the click position.
                if !ui.url.active {
                    let initial = ui
                        .selected_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.url.begin(initial, 0);
                }
                let local =
                    ui.last_pointer.x - (ui.url_bar_rect.min.x + ui::URL_PAD) + ui.url.scroll;
                ui.url.caret = ts.caret_index(&ui.url.text, local.max(0.0));
                ui.url.anchor = None;
                blink_epoch = Instant::now();
                dirty = true;
            } else {
                let clear_preview = handle_action(
                    action,
                    &mut arena,
                    &mut ui,
                    &tasks,
                    &preview,
                    &file_handlers,
                    &mut children,
                    root,
                    window,
                );
                if clear_preview {
                    ptex.retire_current(frame_counter);
                }
                dirty = true;
            }
        }

        let now = Instant::now();
        let dt = (now - last_frame).as_secs_f32().min(0.1);
        last_frame = now;
        if ui.step_camera(dt) {
            dirty = true;
        }

        let caret_visible = if ui.url.active {
            let visible =
                Instant::now().duration_since(blink_epoch).as_millis() as u64 / CARET_BLINK_MS % 2
                    == 0;
            if visible != last_caret_visible {
                last_caret_visible = visible;
                dirty = true;
            }
            visible
        } else {
            true
        };

        if dirty || animating {
            let scale = platform.scale() as f32;
            let mut canvas = DrawList::new(scale);
            let mut overlay = DrawList::new(scale);
            ts.begin_frame();
            let spin_angle = start.elapsed().as_secs_f32() * 12.0;
            let preview_dims = ptex.current.as_ref().map(|(_, _, d)| *d);
            let out = ui::build_frame(
                &mut ui,
                &mut arena,
                root,
                &mut ts,
                &mut canvas,
                &mut overlay,
                window,
                spin_angle,
                preview_dims,
                caret_visible,
            );
            animating = out.animating || ui.refocus;

            let mut uploads = std::mem::take(&mut ts.pending);
            uploads.append(&mut extra_uploads);
            let sets = TexSets {
                atlas: atlas_set,
                preview: ptex.current.as_ref().map(|(_, s, _)| *s),
                scene: Some(scene_set),
                blur: Some(blur_set),
            };
            let mut recorded = false;
            let mut offsets: Vec<u32> = Vec::new();
            let result = gfx.render_frame(|phase, device, cmd, extent, frame_idx| match phase {
                gfx::Phase::Upload => {
                    recorded = true;
                    offsets =
                        renderer.record_pre(device, cmd, frame_idx, &[&canvas, &overlay], &uploads)?;
                    Ok(())
                }
                gfx::Phase::Scene => {
                    renderer.record_pass(device, cmd, extent, frame_idx, &canvas, offsets[0], &sets);
                    Ok(())
                }
                gfx::Phase::Overlay => {
                    renderer
                        .record_pass(device, cmd, extent, frame_idx, &overlay, offsets[1], &sets);
                    Ok(())
                }
            });
            if recorded {
                frame_counter += 1;
                ptex.destroy_old(&gfx.device, &renderer, frame_counter);
            } else {
                // Never recorded (acquire failed): keep the uploads for the
                // retry, or cached glyphs/textures would point at
                // never-filled texels.
                extra_uploads = uploads;
            }
            match result {
                Ok(true) => dirty = false,
                Ok(false) => {
                    if let Err(e) =
                        gfx.recreate_swapchain(platform.physical_extent(), band_h(platform.scale()))
                    {
                        eprintln!("swapchain recreate failed: {e}");
                        break;
                    }
                    renderer.free_texture_set(&gfx.device, scene_set);
                    renderer.free_texture_set(&gfx.device, blur_set);
                    scene_set = renderer
                        .register_texture(&gfx.device, &gfx.blur.scene)
                        .expect("scene set");
                    blur_set = renderer
                        .register_texture(&gfx.device, &gfx.blur.blur_b)
                        .expect("blur set");
                }
                Err(e) => {
                    eprintln!("render failed: {e}");
                    break;
                }
            }
        }
    }

    unsafe {
        gfx.device.device_wait_idle().ok();
        upload::destroy_texture(&gfx.device, &ts.texture);
        if let Some((tex, _, _)) = &ptex.current {
            upload::destroy_texture(&gfx.device, tex);
        }
        for (_, tex, _) in &ptex.retired {
            upload::destroy_texture(&gfx.device, tex);
        }
    }
    renderer.destroy(&gfx.device);
}

enum UrlOutcome {
    None,
    /// Text or caret changed.
    Edited,
    Cancel,
    Copy,
    Cut,
    Paste,
    Commit(String),
}

fn handle_url_key(ev: &KeyEvent, url: &mut TextField) -> UrlOutcome {
    let sym = ev.keysym.raw();
    match sym {
        ks::KEY_Return | ks::KEY_KP_Enter => return UrlOutcome::Commit(url.text.clone()),
        ks::KEY_Escape => {
            url.cancel();
            return UrlOutcome::Cancel;
        }
        ks::KEY_Left => url.move_left(ev.ctrl, ev.shift),
        ks::KEY_Right => url.move_right(ev.ctrl, ev.shift),
        ks::KEY_Home => url.move_home(ev.shift),
        ks::KEY_End => url.move_end(ev.shift),
        ks::KEY_BackSpace => url.backspace(ev.ctrl),
        ks::KEY_Delete => url.delete(),
        _ if ev.ctrl => {
            return match sym {
                ks::KEY_a | ks::KEY_A => {
                    url.select_all();
                    UrlOutcome::Edited
                }
                ks::KEY_c | ks::KEY_C => UrlOutcome::Copy,
                ks::KEY_x | ks::KEY_X => UrlOutcome::Cut,
                ks::KEY_v | ks::KEY_V => UrlOutcome::Paste,
                _ => UrlOutcome::None,
            };
        }
        _ if !ev.utf8.is_empty() => url.insert(&ev.utf8),
        _ => return UrlOutcome::None,
    }
    UrlOutcome::Edited
}

fn expand_home(text: &str) -> String {
    let text = text.trim();
    if let Some(home) = std::env::var_os("HOME") {
        if text == "~" {
            return home.to_string_lossy().into_owned();
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    text.to_string()
}

/// Re-root the whole canvas at `path` (URL bar commit): the old tree is
/// closed and a fresh root scanned synchronously.
fn reroot(
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &mut TextSystem,
    root: &mut NodeId,
    path: PathBuf,
    window: Point,
) -> bool {
    match model::scan_dir(&path) {
        Ok(data) => {
            arena.close_recursive(*root);
            *root = arena.insert(model::node_from_items(path.clone(), data));
            model::calc_size(arena, *root, ts);
            ui.selection = None;
            ui.selected_path = Some(path);
            if let Some(n) = arena.get(*root) {
                let r = n.rect;
                ui.focus_to_rect(r, window);
            }
            true
        }
        Err(e) => {
            eprintln!("cannot open {}: {e}", path.display());
            false
        }
    }
}

/// Clipboard via wl-copy/wl-paste (same tools the toolbar copy button
/// uses); wl-copy lingers to serve the selection and gets reaped like any
/// other child.
fn clipboard_copy(text: &str, children: &mut Vec<std::process::Child>) {
    let child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).ok();
            }
            children.push(child);
        }
        Err(e) => eprintln!("wl-copy failed: {e}"),
    }
}

fn clipboard_paste() -> Option<String> {
    let out = Command::new("wl-paste").arg("--no-newline").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.retain(|c| !c.is_control());
    Some(s)
}

fn make_preview_texture(
    gfx: &gfx::Gfx,
    renderer: &Renderer2d,
    img: &preview::DecodedImage,
) -> Result<(Texture, vk::DescriptorSet), String> {
    let tex = upload::create_texture(
        &gfx.device,
        &gfx.mem_props,
        img.width,
        img.height,
        vk::Format::R8G8B8A8_UNORM,
    )?;
    match renderer.register_texture(&gfx.device, &tex) {
        Ok(set) => Ok((tex, set)),
        Err(e) => {
            unsafe { upload::destroy_texture(&gfx.device, &tex) };
            Err(e)
        }
    }
}

/// Returns true when the currently shown preview should be cleared (any row
/// click, like the C preview_destroy call).
#[allow(clippy::too_many_arguments)]
fn handle_action(
    action: Action,
    arena: &mut NodeArena,
    ui: &mut Ui,
    tasks: &Tasks,
    preview: &Preview,
    file_handlers: &[handlers::Handler],
    children: &mut Vec<std::process::Child>,
    root: NodeId,
    window: Point,
) -> bool {
    match action {
        Action::Row { node, item } => {
            let Some(n) = arena.get(node) else { return false };
            let Some(it) = n.items.get(item) else { return false };
            ui.selection = Some((node, item));
            let path = n.path.join(&it.name);
            ui.selected_path = Some(path.clone());
            let is_dir = it.is_dir;
            let can_scan = is_dir && it.child.is_none() && !it.scanning;
            // Any selection change invalidates in-flight preview decodes.
            if !is_dir && preview::previewable(&path) {
                preview.request(path.clone());
            } else {
                preview.cancel();
            }
            // Marking `scanning` before spawning both shows the spinner and
            // dedups double-clicks (the C version raced two threads here).
            if can_scan {
                if let Some(n) = arena.get_mut(node) {
                    n.items[item].scanning = true;
                }
                tasks.spawn_scan(node, item, path);
            }
            return true;
        }
        Action::CloseNode { node } => {
            arena.close_recursive(node);
            if let Some((sel_node, _)) = ui.selection {
                if arena.get(sel_node).is_none() {
                    ui.selection = None;
                    ui.selected_path = None;
                }
            }
        }
        Action::FocusRoot => {
            if let Some(n) = arena.get(root) {
                let r = n.rect;
                ui.focus_to_rect(r, window);
            }
        }
        Action::FocusSelection => {
            if let Some((node, item)) = ui.selection {
                if let Some(n) = arena.get(node) {
                    if let Some(it) = n.items.get(item) {
                        let r = it.rect.offset(n.rect.min);
                        ui.focus_to_rect(r, window);
                    }
                }
            }
        }
        Action::FocusParentItem => {
            // Focus the item row in the parent that owns the selection's
            // node (the C version's intent; its math added the wrong base).
            if let Some((node, _)) = ui.selection {
                let Some(n) = arena.get(node) else { return false };
                if let Some((pid, pidx)) = n.parent {
                    if let Some(p) = arena.get(pid) {
                        if let Some(pit) = p.items.get(pidx) {
                            let r = pit.rect.offset(p.rect.min);
                            ui.focus_to_rect(r, window);
                        }
                    }
                }
            }
        }
        Action::FocusNodeTop => {
            if let Some((node, _)) = ui.selection {
                if let Some(n) = arena.get(node) {
                    let r = n.rect;
                    ui.focus_to_rect(r, window);
                }
            }
        }
        Action::CopyPath => {
            // Arg-vector spawn: paths with spaces survive (the C
            // string_concat version broke them).
            if let Some(p) = &ui.selected_path {
                if let Err(e) =
                    handlers::spawn(OsStr::new("wl-copy"), &[p.as_os_str()], children)
                {
                    eprintln!("wl-copy failed: {e}");
                }
            }
        }
        Action::OpenTerminal => {
            // The C app passed the selected *file* to foot -D, which foot
            // rejects; use its directory.
            if let Some(p) = &ui.selected_path {
                let dir = if p.is_dir() { p.as_path() } else { p.parent().unwrap_or(p.as_path()) };
                if let Err(e) = handlers::spawn(
                    OsStr::new("foot"),
                    &[OsStr::new("-D"), dir.as_os_str()],
                    children,
                ) {
                    eprintln!("foot failed: {e}");
                }
            }
        }
        Action::OpenWith { node, item } => {
            let Some(n) = arena.get(node) else { return false };
            let Some(it) = n.items.get(item) else { return false };
            let path = n.path.join(&it.name);
            match handlers::find_handler(file_handlers, &path) {
                Some(h) => {
                    if let Err(e) = handlers::spawn_handler(h, &path, children) {
                        eprintln!("handler failed for {}: {e}", path.display());
                    }
                }
                None => eprintln!("no handler for {}", path.display()),
            }
        }
        // UrlBar is handled inline in the main loop (needs text/caret state).
        Action::NodeBody | Action::None | Action::UrlBar => {}
    }
    false
}

/// Apply a worker result on the main thread. The generational id check
/// makes results for since-closed nodes drop harmlessly (the exact race the
/// C app had).
fn apply_task_result(
    result: TaskResult,
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &mut TextSystem,
    platform: &platform::wayland::Platform,
) {
    match result {
        TaskResult::ScanDone { node, item, path, result } => {
            let Some(n) = arena.get_mut(node) else { return };
            let Some(it) = n.items.get_mut(item) else { return };
            if !it.scanning {
                return;
            }
            it.scanning = false;
            match result {
                Ok(data) => {
                    let mut child = model::node_from_items(path, data);
                    child.parent = Some((node, item));
                    let child_id = arena.insert(child);
                    if let Some(n) = arena.get_mut(node) {
                        n.items[item].child = Some(child_id);
                    }
                    model::calc_size(arena, child_id, ts);
                    if let Some(c) = arena.get(child_id) {
                        let r = c.rect;
                        let (lw, lh) = platform.logical_size;
                        ui.focus_to_rect(r, Point::new(lw as f32, lh as f32));
                    }
                }
                Err(e) => {
                    // C returned NULL silently; at least log it.
                    eprintln!("failed to open {}: {e}", path.display());
                }
            }
        }
        // PreviewDone is handled inline in the main loop (it needs gfx).
        TaskResult::PreviewDone { .. } => unreachable!(),
    }
}
