//! Immediate-mode UI: every frame `build_frame` emits both the draw list and
//! the hitbox list; `process_input` hit-tests the pointer against the
//! previous frame's hitboxes (same one-frame latency as the C app) and
//! returns a typed Action instead of the C trigger/type magic ints.

use std::path::PathBuf;

use crate::geom::{Point, Rect};
use crate::gfx::renderer2d::{DrawList, Rgba};
use crate::model::{NodeArena, NodeId};
use crate::platform::wayland::PointerState;
use crate::text::{Icon, TextSystem};

#[derive(Clone, Copy, Debug)]
pub enum Action {
    FocusRoot,
    FocusSelection,
    FocusParentItem,
    FocusNodeTop,
    CopyPath,
    OpenTerminal,
    Row { node: NodeId, item: usize },
    OpenWith { node: NodeId, item: usize },
    NodeBody,
    CloseNode { node: NodeId },
}

#[derive(Clone, Copy)]
pub struct Hitbox {
    pub area: Rect, // screen space (logical px)
    pub action: Action,
    /// Node to move when a drag starts on this box; None pans the camera.
    pub drag: Option<NodeId>,
}

pub enum DragState {
    None,
    /// Button is down but movement hasn't crossed the threshold yet.
    Pending { origin: Point, node: Option<NodeId> },
    Camera,
    Node(NodeId),
}

pub struct Ui {
    pub camera: Point, // camera.min in world coords
    pub camera_target: Point,
    pub refocus: bool,
    pub selection: Option<(NodeId, usize)>,
    pub selected_path: Option<PathBuf>,
    pub hitboxes: Vec<Hitbox>,
    pub drag: DragState,
    last_pointer: Point,
}

/// Camera lerp: C used a fixed 0.16 per frame at ~60Hz; this is the
/// frame-rate independent equivalent.
const LERP_RATE: f32 = 10.5;

impl Ui {
    pub fn new() -> Ui {
        Ui {
            camera: Point::ZERO,
            camera_target: Point::ZERO,
            refocus: false,
            selection: None,
            selected_path: None,
            hitboxes: Vec::new(),
            drag: DragState::None,
            last_pointer: Point::ZERO,
        }
    }

    /// Port of focus_to_rect, including its odd `cam_half - 100` clamp for
    /// rects taller than the view.
    pub fn focus_to_rect(&mut self, rect: Rect, window: Point) {
        let cam_half = Point::new(window.x * 0.5, window.y * 0.5);
        let mut frame_half = Point::new(rect.width() * 0.5, rect.height() * 0.5);
        if frame_half.y > cam_half.y {
            frame_half.y = cam_half.y - 100.0;
        }
        self.camera_target = rect.min.sub(cam_half).add(frame_half);
        self.refocus = true;
    }

    /// Advance the camera lerp; returns true while animating.
    pub fn step_camera(&mut self, dt: f32) -> bool {
        if !self.refocus {
            return false;
        }
        let t = 1.0 - (-LERP_RATE * dt).exp();
        self.camera = self.camera.lerp(self.camera_target, t);
        if self.camera_target.sub(self.camera).length() < 0.5 {
            self.camera = self.camera_target;
            self.refocus = false;
        }
        true
    }

    /// Hit-test, drag handling, and click dispatch. Returns the clicked
    /// action (if any) and whether the frame is dirty from input.
    pub fn process_input(
        &mut self,
        arena: &mut NodeArena,
        pointer: &PointerState,
    ) -> (Option<Action>, bool) {
        let mut dirty = false;
        let cursor = Point::new(pointer.x as f32, pointer.y as f32);
        let hit = self
            .hitboxes
            .iter()
            .rev()
            .find(|hb| hb.area.contains(cursor))
            .copied();

        if pointer.pressed {
            self.drag = DragState::Pending { origin: cursor, node: hit.and_then(|h| h.drag) };
        }

        if pointer.is_down {
            let delta = cursor.sub(self.last_pointer);
            if let DragState::Pending { origin, node } = self.drag {
                if cursor.sub(origin).length() >= 1.0 {
                    self.drag = match node {
                        Some(id) => DragState::Node(id),
                        None => DragState::Camera,
                    };
                }
            }
            match self.drag {
                DragState::Camera => {
                    self.camera = self.camera.sub(delta);
                    self.refocus = false;
                    dirty = true;
                }
                DragState::Node(id) => {
                    if let Some(node) = arena.get_mut(id) {
                        node.rect = node.rect.offset(delta);
                        dirty = true;
                    }
                }
                _ => {}
            }
        }
        self.last_pointer = cursor;

        let mut action = None;
        if pointer.released {
            if let DragState::Pending { .. } = self.drag {
                action = hit.map(|h| h.action);
            }
            if !matches!(self.drag, DragState::None) {
                self.drag = DragState::None;
                dirty = true;
            }
        }
        (action, dirty)
    }
}

/// Colors from draw.c.
const COLOR_BOX_FILL: Rgba = Rgba([0, 0, 0, 128]); // rgba(0,0,0,0.5)
const COLOR_ROW_SELECTED: Rgba = Rgba([255, 0, 0, 255]);
const COLOR_ROW_OPEN: Rgba = Rgba([0, 255, 0, 255]);

pub struct FrameOut {
    /// keep rendering continuously (spinners visible)
    pub animating: bool,
}

/// Build the whole frame: node tree, preview placeholder, toolbar. Fills
/// `ui.hitboxes` for next iteration's input.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    ui: &mut Ui,
    arena: &mut NodeArena,
    root: NodeId,
    ts: &mut TextSystem,
    list: &mut DrawList,
    window: Point,
    spin_angle: f32,
    preview: Option<(u32, u32)>,
) -> FrameOut {
    ui.hitboxes.clear();
    let mut out = FrameOut { animating: false };
    let camera = Rect { min: ui.camera, max: ui.camera.add(window) };
    draw_entries(ui, arena, root, ts, list, camera, spin_angle, &mut out);
    draw_preview(list, window, preview);
    draw_navigation(ui, ts, list, window);
    out
}

/// Port of draw_preview: scaled to 400 wide (upscaling small images too,
/// like the C app), anchored bottom-right with a 10px margin and a 1px
/// white border, drawn under the toolbar.
fn draw_preview(list: &mut DrawList, window: Point, preview: Option<(u32, u32)>) {
    let Some((w, h)) = preview else { return };
    if w == 0 {
        return;
    }
    let scale = 400.0 / w as f32;
    let (pw, ph) = (400.0, h as f32 * scale);
    let r = Rect::from_xywh(window.x - 10.0 - pw, window.y - 10.0 - ph, pw, ph);
    list.image(r);
    list.rect_stroke(r, Rgba::WHITE, 0.0, 1.0);
}

fn draw_entries(
    ui: &mut Ui,
    arena: &mut NodeArena,
    id: NodeId,
    ts: &mut TextSystem,
    list: &mut DrawList,
    camera: Rect,
    spin_angle: f32,
    out: &mut FrameOut,
) {
    let off = Point::new(-camera.min.x, -camera.min.y);
    let Some(node) = arena.get(id) else { return };
    let node_rect = node.rect;
    let is_root = node.parent.is_none();

    if camera.intersects(node_rect) {
        let screen_rect = node_rect.offset(off);
        list.rect(screen_rect, COLOR_BOX_FILL, 5.0);
        list.rect_stroke(screen_rect, Rgba::WHITE, 5.0, 3.0);
        ui.hitboxes.push(Hitbox { area: screen_rect, action: Action::NodeBody, drag: Some(id) });
        if !is_root {
            // Close button: node.min + (2, -25), 20x20, like draw_entries.
            let r = Rect::from_xywh(node_rect.min.x + 2.0, node_rect.min.y - 25.0, 20.0, 20.0)
                .offset(off);
            list.glyph_quad(r, ts.icon_uv(Icon::Close), Rgba::WHITE, 0.0);
            ui.hitboxes.push(Hitbox {
                area: r,
                action: Action::CloseNode { node: id },
                drag: Some(id),
            });
        }
    }

    let item_count = arena.get(id).map(|n| n.items.len()).unwrap_or(0);
    for i in 0..item_count {
        let Some(node) = arena.get(id) else { return };
        let item = &node.items[i];
        let rect = item.rect.offset(node_rect.min); // world
        let screen_rect = rect.offset(off);
        let in_view = camera.intersects(rect);
        let selected = ui.selection == Some((id, i));
        let child = item.child;
        let is_dir = item.is_dir;
        let scanning = item.scanning;
        let display = item.display.clone();

        if in_view {
            let color = if selected {
                COLOR_ROW_SELECTED
            } else if child.is_some() {
                COLOR_ROW_OPEN
            } else {
                Rgba::WHITE
            };
            ts.draw(list, screen_rect.min, &display, color);
            ui.hitboxes.push(Hitbox {
                area: screen_rect,
                action: Action::Row { node: id, item: i },
                drag: Some(id),
            });
            if selected && !is_dir {
                let r = Rect::from_xywh(screen_rect.max.x + 10.0, screen_rect.min.y + 4.0, 20.0, 20.0);
                list.glyph_quad(r, ts.icon_uv(Icon::Open), Rgba::WHITE, 0.0);
                ui.hitboxes.push(Hitbox {
                    area: r,
                    action: Action::OpenWith { node: id, item: i },
                    drag: Some(id),
                });
            }
        }

        if scanning {
            // Busy spinner beside the row (the C app intended this but its
            // busy.svg never existed).
            let r = Rect::from_xywh(screen_rect.max.x + 10.0, screen_rect.min.y + 2.0, 16.0, 16.0);
            list.glyph_quad(r, ts.icon_uv(Icon::Busy), Rgba::WHITE, spin_angle);
            out.animating = true;
        } else if let Some(child_id) = child {
            draw_entries(ui, arena, child_id, ts, list, camera, spin_angle, out);
            if let Some(child_node) = arena.get(child_id) {
                list.line(
                    Point::new(node_rect.max.x, rect.min.y + rect.height() / 2.0).add(off),
                    Point::new(child_node.rect.min.x, child_node.rect.min.y + 5.0).add(off),
                    3.0,
                    Rgba::WHITE,
                );
            }
        }
    }
}

/// Port of draw_navigation: toolbar bar, six icon buttons, URL bar.
fn draw_navigation(ui: &mut Ui, ts: &mut TextSystem, list: &mut DrawList, window: Point) {
    list.solid(Rect::from_xywh(0.0, 0.0, window.x, 62.0), Rgba::new(0.0, 0.0, 0.0, 0.9));
    list.line(Point::new(0.0, 62.0), Point::new(window.x, 62.0), 1.0, Rgba::new(1.0, 1.0, 1.0, 0.3));

    let mut ox = 20.0;
    let oy = 20.0;
    let size = 22.0;
    let padding = 20.0;
    let buttons = [
        (Icon::Home, Action::FocusRoot),
        (Icon::Selection, Action::FocusSelection),
        (Icon::Parent, Action::FocusParentItem),
        (Icon::Top, Action::FocusNodeTop),
        (Icon::Copy, Action::CopyPath),
        (Icon::Terminal, Action::OpenTerminal),
    ];
    for (icon, action) in buttons {
        let r = Rect::from_xywh(ox, oy, size, size);
        list.glyph_quad(r, ts.icon_uv(icon), Rgba::WHITE, 0.0);
        ui.hitboxes.push(Hitbox { area: r, action, drag: None });
        ox += size + padding;
    }

    // URL bar: y 16, height 31, to width-20 (draw_navigation's abwh quirk).
    let url_bar = Rect::from_xywh(ox, 16.0, window.x - ox - padding, 31.0);
    list.rect_stroke(url_bar, Rgba::WHITE, 5.0, 1.0);
    let text = match &ui.selected_path {
        Some(p) => p.to_string_lossy().into_owned(),
        None => "No selection...".to_string(),
    };
    ts.draw(list, Point::new(url_bar.min.x + 10.0, url_bar.min.y + 2.0), &text, Rgba::WHITE);
}
