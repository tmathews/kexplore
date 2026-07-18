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
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use ash::vk;
use geom::Point;
use gfx::renderer2d::{DrawList, Renderer2d, TexSets};
use gfx::upload::{self, PendingUpload, Texture};
use model::{NodeArena, NodeId};
use platform::wayland::KeyEvent;
use std::collections::HashSet;
use tasks::{TaskResult, Tasks};
use text::TextSystem;
use textfield::TextField;
use ui::{Action, Ui};
use wayland_client::Proxy;
use xkbcommon::xkb::keysyms as ks;

const CARET_BLINK_MS: u64 = 530;

struct PreviewSlot {
    tex: Texture,
    set: vk::DescriptorSet,
}

/// Owns every image-preview texture. Preview nodes reference a slot by its
/// opaque index; when a node is closed (its index no longer appears among the
/// arena's preview nodes) the texture is retired and destroyed after the last
/// in-flight frame that could still sample it.
struct PreviewTextures {
    slots: Vec<Option<PreviewSlot>>,
    free: Vec<u32>,
    retired: Vec<(u64, Texture, vk::DescriptorSet)>,
}

impl PreviewTextures {
    fn new() -> PreviewTextures {
        PreviewTextures { slots: Vec::new(), free: Vec::new(), retired: Vec::new() }
    }

    /// Register a texture, returning its opaque id (reusing a freed slot when
    /// possible).
    fn alloc(&mut self, tex: Texture, set: vk::DescriptorSet) -> u32 {
        let slot = PreviewSlot { tex, set };
        if let Some(id) = self.free.pop() {
            self.slots[id as usize] = Some(slot);
            id
        } else {
            self.slots.push(Some(slot));
            (self.slots.len() - 1) as u32
        }
    }

    /// Descriptor table indexed by texture id, for `TexSets::previews`. Empty
    /// or retired slots resolve to the atlas (never actually sampled).
    fn table(&self, atlas: vk::DescriptorSet) -> Vec<vk::DescriptorSet> {
        self.slots.iter().map(|s| s.as_ref().map(|s| s.set).unwrap_or(atlas)).collect()
    }

    /// Retire any slot whose id is no longer referenced by a live preview
    /// node; its texture drains the in-flight frames before destruction.
    fn retire_unused(&mut self, live: &HashSet<u32>, frame_counter: u64) {
        for i in 0..self.slots.len() as u32 {
            if self.slots[i as usize].is_some() && !live.contains(&i) {
                let s = self.slots[i as usize].take().unwrap();
                self.retired.push((frame_counter, s.tex, s.set));
                self.free.push(i);
            }
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

    unsafe fn destroy_all(&mut self, device: &ash::Device) {
        for slot in self.slots.drain(..).flatten() {
            upload::destroy_texture(device, &slot.tex);
        }
        for (_, tex, _) in &self.retired {
            upload::destroy_texture(device, tex);
        }
    }
}

fn main() {
    let platform::wayland::Init { conn, mut queue, mut platform } =
        platform::wayland::init("Kallos Explore", "kallos-explore", (1280, 720))
            .unwrap_or_else(|e| {
                eprintln!("failed to init wayland: {e}");
                std::process::exit(1);
            });

    let display_ptr = conn.backend().display_ptr() as *mut std::ffi::c_void;
    let surface_ptr = platform.surface.id().as_ptr() as *mut std::ffi::c_void;
    // The frosted blur now covers the whole scene (so panels — the toolbar and
    // the context menu — can blur anywhere), so its height is the full surface.
    let mut gfx = gfx::Gfx::new(
        display_ptr,
        surface_ptr,
        platform.physical_extent(),
        platform.physical_extent().1,
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

    // The canvas always roots at the filesystem root; the app opens by
    // navigating from there down to the user's home directory.
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let file_handlers = handlers::read_handlers(&home.join(".config/kallos/handlers"));
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut arena = NodeArena::new();
    let root_items = model::scan_dir(Path::new("/")).unwrap_or_else(|e| {
        eprintln!("failed to load root: {e}");
        std::process::exit(1);
    });
    let root = arena.insert(model::node_from_items(PathBuf::from("/"), root_items));
    // The root is permanent — always pinned, never pruned.
    if let Some(n) = arena.get_mut(root) {
        n.pinned = true;
    }
    let (lw, lh) = platform.logical_size;
    let window0 = Point::new((lw.max(1)) as f32, (lh.max(1)) as f32);
    model::calc_size(&mut arena, root, &mut ts, ui::node_max_size(window0));

    let mut ui = Ui::new();
    // Open the chain of nodes down to home and focus it; snap (no startup pan).
    navigate_to(&mut arena, &mut ui, &mut ts, root, &home, window0);
    // Pin the landing node (and its chain) so browsing elsewhere doesn't
    // collapse the home view the app opened to.
    if let Some(active) = ui.active_node {
        pin_node(&mut arena, active);
    }
    ui.camera = ui.camera_target;
    ui.refocus = false;

    let tasks = Tasks::new().unwrap_or_else(|e| {
        eprintln!("failed to init task system: {e}");
        std::process::exit(1);
    });
    let mut ptex = PreviewTextures::new();
    let mut extra_uploads: Vec<PendingUpload> = Vec::new();
    let mut frame_counter: u64 = 0;

    let qh = queue.handle();
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
                TaskResult::PreviewDone { node, item, path, image } => {
                    if let Some(n) = arena.get_mut(node) {
                        if let Some(it) = n.items.get_mut(item) {
                            it.preview_loading = false;
                        }
                    }
                    // Attach to the file node we opened for this row, but only if
                    // it's still the same file with no image yet (the user may
                    // have clicked away, closing the transient node).
                    let child = arena
                        .get(node)
                        .and_then(|n| n.items.get(item))
                        .and_then(|it| it.child);
                    let attach = child.and_then(|c| arena.get(c)).is_some_and(|c| {
                        c.path == path && c.file.as_ref().is_some_and(|f| f.image.is_none())
                    });
                    if let (true, Some(child_id), Some(img)) = (attach, child, image) {
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
                                let tex_id = ptex.alloc(tex, set);
                                let (lw, lh) = platform.logical_size;
                                let win = Point::new(lw as f32, lh as f32);
                                attach_image(
                                    &mut arena, &mut ui, &ts, child_id, tex_id, img.width,
                                    img.height, win,
                                );
                            }
                            Err(e) => eprintln!("preview texture failed: {e}"),
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
                gfx.recreate_swapchain(platform.physical_extent(), platform.physical_extent().1)
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
                            platform.set_clipboard(&qh, t.to_owned());
                        }
                    }
                    UrlOutcome::Cut => {
                        if let Some(t) = ui.url.selected_text().map(str::to_owned) {
                            platform.set_clipboard(&qh, t);
                            ui.url.delete_selection();
                            blink_epoch = Instant::now();
                            dirty = true;
                        }
                    }
                    UrlOutcome::Paste => {
                        if let Some(t) = platform.clipboard_text(&conn) {
                            ui.url.insert(&t);
                            blink_epoch = Instant::now();
                            dirty = true;
                        }
                    }
                    UrlOutcome::Commit(text) => {
                        let path = PathBuf::from(expand_home(&text));
                        // Navigate: open the chain of nodes from root to the
                        // path (reusing open ones) and jump the camera there.
                        if path.exists()
                            && navigate_to(&mut arena, &mut ui, &mut ts, root, &path, window)
                        {
                            ui.url.cancel();
                        }
                        // Invalid path: stay in the editor so it can be fixed.
                        dirty = true;
                    }
                }
            }
        } else {
            // Canvas keys: Space pins the active node (file or directory) so it
            // stays open, along with its ancestor chain.
            for ev in &key_events {
                if ev.keysym.raw() == ks::KEY_space {
                    if let Some(active) = ui.active_node {
                        pin_node(&mut arena, active);
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
        if let Some((action, double)) = action {
            if matches!(action, Action::UrlBar) {
                // Focus the field (seeding it with the selected path, or the
                // active root path when nothing is selected) and put the caret
                // at the click position.
                if !ui.url.active {
                    let initial = ui
                        .selected_path
                        .clone()
                        .or_else(|| arena.get(root).map(|n| n.path.clone()))
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
            } else if matches!(action, Action::FocusHome) {
                // Home button: navigate to $HOME (reopens its chain if any of
                // it was closed) and jump the camera there.
                navigate_to(&mut arena, &mut ui, &mut ts, root, &home, window);
                dirty = true;
            } else if matches!(action, Action::GoUp) {
                // Up button: navigate to the parent of the current location.
                if let Some(parent) =
                    ui.selected_path.as_deref().and_then(Path::parent).map(Path::to_path_buf)
                {
                    navigate_to(&mut arena, &mut ui, &mut ts, root, &parent, window);
                    dirty = true;
                }
            } else if matches!(action, Action::CopyPath) {
                // Copy the selected path onto the native clipboard (held until
                // another client takes the selection).
                if let Some(p) = ui.selected_path.as_ref().map(|p| p.to_string_lossy().into_owned()) {
                    platform.set_clipboard(&qh, p);
                }
            } else if let Action::Menu(item) = action {
                // Context-menu row. Take (and close) the menu, then act on its
                // target.
                if let Some(menu) = ui.context_menu.take() {
                    match item {
                        ui::MenuItem::Open => {
                            // Files open in their handler (double-click), dirs
                            // open/focus their node (single-click).
                            handle_action(
                                Action::Row { node: menu.node, item: menu.item },
                                !menu.is_dir,
                                &mut arena,
                                &mut ui,
                                &ts,
                                &tasks,
                                &file_handlers,
                                &mut children,
                                window,
                            );
                        }
                        ui::MenuItem::OpenTerminal => {
                            let dir = if menu.path.is_dir() {
                                menu.path.as_path()
                            } else {
                                menu.path.parent().unwrap_or(menu.path.as_path())
                            };
                            if let Err(e) = handlers::spawn(
                                OsStr::new("foot"),
                                &[OsStr::new("-D"), dir.as_os_str()],
                                &mut children,
                            ) {
                                eprintln!("foot failed: {e}");
                            }
                        }
                        ui::MenuItem::CopyFile => {
                            platform.set_clipboard_file(&qh, &menu.path);
                        }
                        ui::MenuItem::CopyPath => {
                            platform.set_clipboard(&qh, menu.path.to_string_lossy().into_owned());
                        }
                    }
                }
                dirty = true;
            } else {
                handle_action(
                    action,
                    double,
                    &mut arena,
                    &mut ui,
                    &ts,
                    &tasks,
                    &file_handlers,
                    &mut children,
                    window,
                );
                dirty = true;
            }
        }

        let now = Instant::now();
        let dt = (now - last_frame).as_secs_f32().min(0.1);
        last_frame = now;
        // Camera pan + zoom smoothing.
        let cam_moving = ui.step_camera(dt);
        if cam_moving {
            dirty = true;
        }
        // Collision glide: nodes slide to their resolved positions.
        let nodes_moving = model::step_nodes(&mut arena, dt);
        if nodes_moving {
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
            let out = ui::build_frame(
                &mut ui,
                &mut arena,
                root,
                &mut ts,
                &mut canvas,
                &mut overlay,
                window,
                spin_angle,
                caret_visible,
            );
            animating = out.animating || cam_moving || nodes_moving;

            let mut uploads = std::mem::take(&mut ts.pending);
            uploads.append(&mut extra_uploads);
            let prev_table = ptex.table(atlas_set);
            let sets = TexSets {
                atlas: atlas_set,
                previews: &prev_table,
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
                // Free textures of any file node that was closed.
                let live: HashSet<u32> = arena
                    .iter()
                    .filter_map(|(_, n)| n.file.as_ref().and_then(|f| f.image).map(|img| img.tex))
                    .collect();
                ptex.retire_unused(&live, frame_counter);
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
                        gfx.recreate_swapchain(platform.physical_extent(), platform.physical_extent().1)
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
        ptex.destroy_all(&gfx.device);
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

/// Open every directory node from the root (`/`) down to `target`, reusing
/// nodes that are already open, select the target, and jump the camera to it.
/// Preserves all other open nodes. Returns false if the target isn't reachable
/// (a path component is missing).
fn navigate_to(
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &mut TextSystem,
    root: NodeId,
    target: &Path,
    window: Point,
) -> bool {
    let Some(root_path) = arena.get(root).map(|n| n.path.clone()) else { return false };
    let Ok(rel) = target.strip_prefix(&root_path) else { return false };
    let comps: Vec<&OsStr> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Target is the root itself: focus and activate it.
    if comps.is_empty() {
        if let Some(n) = arena.get(root) {
            let r = n.rect;
            ui.focus_to_rect(r, window);
        }
        activate(arena, ui, root);
        return true;
    }

    let mut current = root;
    let mut cur_path = root_path;
    let last = comps.len() - 1;
    for (i, &comp) in comps.iter().enumerate() {
        let Some(node) = arena.get(current) else { return false };
        let Some(idx) = node.items.iter().position(|it| it.name.as_os_str() == comp) else {
            return false; // path component not present (renamed / removed)
        };
        let is_dir = node.items[idx].is_dir;
        cur_path = cur_path.join(comp);

        if is_dir {
            // Reuse the already-open child, or open it synchronously.
            let child = match arena.get(current).and_then(|n| n.items[idx].child) {
                Some(c) => c,
                None => match open_child_sync(arena, ts, current, idx, cur_path.clone(), window) {
                    Some(c) => c,
                    None => return false,
                },
            };
            if i == last {
                // Target directory: focus and activate its node.
                if let Some(n) = arena.get(child) {
                    let r = n.rect;
                    ui.focus_to_rect(r, window);
                }
                activate(arena, ui, child);
            } else {
                current = child;
            }
        } else {
            // Target file (must be the last component): open/focus its file node.
            let fnode = match arena.get(current).and_then(|n| n.items[idx].child) {
                Some(c) => Some(c),
                None => open_file_node(arena, ui, ts, current, idx, cur_path.clone(), window),
            };
            if let Some(fid) = fnode {
                if let Some(n) = arena.get(fid) {
                    let r = n.rect;
                    ui.focus_to_rect(r, window);
                }
                activate(arena, ui, fid);
            }
            break;
        }
    }
    true
}

/// Synchronously scan `path` and attach it as the open child of item `idx` in
/// `node`; returns the new node id, collision-resolved into a free spot.
fn open_child_sync(
    arena: &mut NodeArena,
    ts: &mut TextSystem,
    node: NodeId,
    idx: usize,
    path: PathBuf,
    window: Point,
) -> Option<NodeId> {
    let data = model::scan_dir(&path).ok()?;
    let mut child = model::node_from_items(path, data);
    child.parent = Some((node, idx));
    let child_id = arena.insert(child);
    if let Some(n) = arena.get_mut(node) {
        n.items[idx].child = Some(child_id);
    }
    model::calc_size(arena, child_id, ts, ui::node_max_size(window));
    let cand = arena.get(child_id).map(|n| n.rect)?;
    let obstacles: Vec<geom::Rect> =
        arena.iter().filter(|(id, _)| *id != child_id).map(|(_, n)| n.rect).collect();
    let target = model::resolve_collision(cand, &obstacles, false);
    if let Some(n) = arena.get_mut(child_id) {
        n.rect = target;
    }
    Some(child_id)
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

/// Handle a resolved click action (row/node/header buttons). `ts` is needed to
/// size freshly opened file nodes.
#[allow(clippy::too_many_arguments)]
fn handle_action(
    action: Action,
    double: bool,
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &TextSystem,
    tasks: &Tasks,
    file_handlers: &[handlers::Handler],
    children: &mut Vec<std::process::Child>,
    window: Point,
) {
    match action {
        Action::Row { node, item } => {
            let Some(n) = arena.get(node) else { return };
            let Some(it) = n.items.get(item) else { return };
            let path = n.path.join(&it.name);
            let is_dir = it.is_dir;
            let child = it.child;
            let scanning = it.scanning;

            if double {
                // Double-click a file opens it in its external handler.
                // Directories already open on single click, so double adds
                // nothing for them.
                if !is_dir {
                    match handlers::find_handler(file_handlers, &path) {
                        Some(h) => {
                            if let Err(e) = handlers::spawn_handler(h, &path, children) {
                                eprintln!("handler failed for {}: {e}", path.display());
                            }
                        }
                        None => eprintln!("no handler for {}", path.display()),
                    }
                }
                return;
            }

            // Single click opens (or focuses) the row's node and makes it
            // active; activating prunes every transient peek not on the route,
            // so the previous unpinned file/dir peek is dismissed.
            if let Some(c) = child {
                activate(arena, ui, c);
                if let Some(r) = arena.get(c).map(|n| n.rect) {
                    ui.ensure_visible(r, window);
                }
                return;
            }
            // Dismiss any transient peek off this node's route *before* placing
            // the newcomer, so its collision resolve isn't routed around a node
            // that is about to close (which pushed it needlessly far down).
            prune_transients(arena, Some(node));
            if is_dir {
                // Directory: scan asynchronously; ScanDone activates the child.
                if !scanning {
                    if let Some(n) = arena.get_mut(node) {
                        n.items[item].scanning = true;
                    }
                    // Keep the active node valid during the async scan.
                    ui.set_active(arena, Some(node));
                    tasks.spawn_scan(node, item, path);
                }
            } else {
                // File: open a fresh (unpinned) file node, make it active, and
                // decode an image if the type supports it. (Peeks were pruned
                // above, so the placement above is already collision-correct.)
                if let Some(child_id) =
                    open_file_node(arena, ui, ts, node, item, path.clone(), window)
                {
                    ui.set_active(arena, Some(child_id));
                    if preview::previewable(&path) {
                        if let Some(n) = arena.get_mut(node) {
                            n.items[item].preview_loading = true;
                        }
                        tasks.spawn_preview(node, item, path);
                    }
                }
            }
        }
        Action::NodeBody { node } => {
            // Clicking a node makes it active (URL bar + route follow it) and
            // dismisses any transient peek off its route.
            activate(arena, ui, node);
        }
        Action::CloseNode { node } => {
            let parent = arena.get(node).and_then(|n| n.parent.map(|(p, _)| p));
            arena.close_recursive(node);
            // If the active node was the one closed (or a now-dangling
            // descendant), fall back to the closed node's parent.
            if ui.active_node.map_or(false, |a| arena.get(a).is_none()) {
                ui.set_active(arena, parent);
            }
        }
        Action::ToggleFavorite { node } => {
            if let Some(n) = arena.get(node) {
                let p = n.path.clone();
                if !ui.favorites.remove(&p) {
                    ui.favorites.insert(p);
                }
            }
        }
        Action::PinNode { node } => {
            // Pin the node and its ancestor chain so it stays open.
            pin_node(arena, node);
        }
        Action::FocusSelection => {
            if let Some(active) = ui.active_node {
                if let Some(r) = arena.get(active).map(|n| n.rect) {
                    ui.focus_to_rect(r, window);
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
        // UrlBar, FocusHome, GoUp, CopyPath and Menu are handled inline in the
        // main loop (they need text/caret state, navigate_to, or the platform
        // clipboard); ResizeNode is press-driven.
        Action::None
        | Action::UrlBar
        | Action::FocusHome
        | Action::GoUp
        | Action::CopyPath
        | Action::Menu(_)
        | Action::ResizeNode { .. } => {}
    }
}

/// Pin `id` and its whole ancestor chain, so pinned nodes stay connected to the
/// root and pruning never orphans a pin.
fn pin_node(arena: &mut NodeArena, id: NodeId) {
    let mut cur = Some(id);
    while let Some(n) = cur {
        cur = match arena.get_mut(n) {
            Some(node) => {
                node.pinned = true;
                node.parent.map(|(p, _)| p)
            }
            None => None,
        };
    }
}

/// Close every unpinned node that is not on the active route (root -> active).
/// With the pin-ancestors invariant, this never orphans a pinned node.
fn prune_transients(arena: &mut NodeArena, active: Option<NodeId>) {
    let mut keep: HashSet<NodeId> = HashSet::new();
    let mut cur = active;
    while let Some(id) = cur {
        keep.insert(id);
        cur = arena.get(id).and_then(|n| n.parent.map(|(p, _)| p));
    }
    let close: Vec<NodeId> =
        arena.iter().filter(|(id, n)| !n.pinned && !keep.contains(id)).map(|(id, _)| id).collect();
    for id in close {
        arena.close_recursive(id);
    }
}

/// Make `node` active and dismiss every transient peek not on its route.
fn activate(arena: &mut NodeArena, ui: &mut Ui, node: NodeId) {
    ui.set_active(arena, Some(node));
    prune_transients(arena, Some(node));
}

/// Default file-node width; the height is the header plus the info panel (an
/// image, once decoded, grows the box between them).
const FILE_NODE_W: f32 = 300.0;

/// Synchronously create a transient file node for `path` (metadata only, no
/// image yet), attach it as the open child of item `idx` in `node`, place it
/// beside the row and collision-resolve it. None if the stat fails.
fn open_file_node(
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &TextSystem,
    node: NodeId,
    idx: usize,
    path: PathBuf,
    window: Point,
) -> Option<NodeId> {
    let meta = model::FileMeta::read(&path)?;
    let origin = {
        let p = arena.get(node)?;
        let item_y = p.items.get(idx).map(|it| it.rect.min.y).unwrap_or(0.0);
        Point::new(p.rect.max.x + 20.0, p.rect.min.y + item_y - p.scroll)
    };
    let h = model::HEADER_H + ui::file_info_height(ts);
    let rect = geom::Rect::from_xywh(origin.x, origin.y, FILE_NODE_W, h);
    let child_id = arena.insert(model::file_node(path, (node, idx), meta, None, rect, false));
    if let Some(n) = arena.get_mut(node) {
        n.items[idx].child = Some(child_id);
    }
    let obstacles: Vec<geom::Rect> =
        arena.iter().filter(|(id, _)| *id != child_id).map(|(_, n)| n.rect).collect();
    let target = model::resolve_collision(rect, &obstacles, false);
    if let Some(c) = arena.get_mut(child_id) {
        if target != rect {
            c.anim_to = Some(target.min);
        }
    }
    ui.ensure_visible(target, window);
    Some(child_id)
}

/// Attach a decoded image to an existing file node, growing the box so the
/// image (aspect-preserved) sits above the info panel, then re-resolve collisions.
fn attach_image(
    arena: &mut NodeArena,
    ui: &mut Ui,
    ts: &TextSystem,
    id: NodeId,
    tex: u32,
    img_w: u32,
    img_h: u32,
    window: Point,
) {
    let cap = ui::node_max_size(window);
    let info_h = ui::file_info_height(ts);
    let aspect = (img_w.max(1) as f32) / (img_h.max(1) as f32);
    // Box = header + aspect-preserved image + info panel; cap to the safe area.
    let mut w = 320.0_f32.min(cap.x);
    let mut box_h = model::HEADER_H + w / aspect + info_h;
    if box_h > cap.y {
        box_h = cap.y;
        let img_h = (box_h - model::HEADER_H - info_h).max(1.0);
        w = (img_h * aspect).min(cap.x);
    }
    let Some(n) = arena.get_mut(id) else { return };
    n.rect.max = Point::new(n.rect.min.x + w, n.rect.min.y + box_h);
    n.content_w = w;
    n.content_h = box_h;
    if let Some(f) = &mut n.file {
        f.image = Some(model::ImageTex { tex, img_w, img_h });
    }
    let rect = n.rect;
    let obstacles: Vec<geom::Rect> =
        arena.iter().filter(|(x, _)| *x != id).map(|(_, n)| n.rect).collect();
    let target = model::resolve_collision(rect, &obstacles, false);
    if target != rect {
        if let Some(n) = arena.get_mut(id) {
            n.anim_to = Some(target.min);
        }
    }
    ui.ensure_visible(target, window);
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
                    let (lw, lh) = platform.logical_size;
                    let window = Point::new(lw as f32, lh as f32);
                    model::calc_size(arena, child_id, ts, ui::node_max_size(window));
                    if let Some(cand) = arena.get(child_id).map(|c| c.rect) {
                        // Collision: slide the newcomer (down/right only) to
                        // the nearest free space so it never lands on top of
                        // an existing node.
                        let obstacles: Vec<geom::Rect> = arena
                            .iter()
                            .filter(|(id, _)| *id != child_id)
                            .map(|(_, n)| n.rect)
                            .collect();
                        let target = model::resolve_collision(cand, &obstacles, false);
                        if target != cand {
                            if let Some(c) = arena.get_mut(child_id) {
                                c.anim_to = Some(target.min);
                            }
                        }
                        // Calm auto-focus: only pan if the resolved resting
                        // place is outside the visible area.
                        ui.ensure_visible(target, window);
                    }
                    // Single-click-to-open triggered this scan; the freshly
                    // opened directory becomes active (pruning stale peeks).
                    activate(arena, ui, child_id);
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
