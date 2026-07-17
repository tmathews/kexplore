//! Immediate-mode UI: every frame `build_frame` emits both the draw list and
//! the hitbox list; `process_input` hit-tests the pointer against the
//! previous frame's hitboxes (same one-frame latency as the C app) and
//! returns a typed Action instead of the C trigger/type magic ints.

use std::path::PathBuf;

use crate::geom::{Point, Rect};
use crate::gfx::renderer2d::{DrawList, Rgba, TexSlot};
use crate::model::{NodeArena, NodeId};
use crate::platform::wayland::PointerState;
use crate::text::{Icon, TextSystem};
use crate::textfield::TextField;

pub const TOOLBAR_H: f32 = 62.0;
/// Horizontal inset from the URL bar border to its text.
pub const URL_PAD: f32 = 10.0;

/// The largest box a node may occupy: 90% of the safe viewing area (the
/// window minus the toolbar band).
pub fn node_max_size(window: Point) -> Point {
    Point::new(window.x * 0.9, (window.y - TOOLBAR_H).max(1.0) * 0.9)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Dead surface that swallows clicks (toolbar background).
    None,
    /// Click in the URL bar: begin/continue editing, position the caret.
    UrlBar,
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
    pub url: TextField,
    /// URL bar rect from the last built frame (screen/logical px), for
    /// caret positioning and click-away detection.
    pub url_bar_rect: Rect,
    pub last_pointer: Point,
    /// What the pointer is over (from the previous frame's hitboxes).
    pub hover: Option<Action>,
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
            url: TextField::new(),
            url_bar_rect: Rect::ZERO,
            last_pointer: Point::ZERO,
            hover: None,
        }
    }

    /// Port of focus_to_rect, including its odd `cam_half - 100` clamp for
    /// rects taller than the view — but centering within the visible region
    /// below the toolbar instead of the whole window, so focused content
    /// no longer lands under the bar.
    pub fn focus_to_rect(&mut self, rect: Rect, window: Point) {
        let avail_h = (window.y - TOOLBAR_H).max(1.0);
        let cam_half = Point::new(window.x * 0.5, avail_h * 0.5);
        let mut frame_half = Point::new(rect.width() * 0.5, rect.height() * 0.5);
        if frame_half.y > cam_half.y {
            frame_half.y = cam_half.y - 100.0;
        }
        self.camera_target =
            rect.min.sub(Point::new(cam_half.x, TOOLBAR_H + cam_half.y)).add(frame_half);
        self.refocus = true;
    }

    /// Pan the camera just enough that `rect` (a freshly opened node) sits
    /// inside the safe area below the toolbar, with a margin; no-op when it
    /// is already fully visible. Explicit focus actions still use
    /// focus_to_rect's full centering — this is the calm variant for
    /// automatic movement.
    pub fn ensure_visible(&mut self, rect: Rect, window: Point) {
        const MARGIN: f32 = 12.0;
        // Include the close button hanging 25px above the box.
        let rect = Rect { min: Point::new(rect.min.x, rect.min.y - 30.0), max: rect.max };
        // If a lerp is already in flight, judge visibility against where the
        // camera is headed, not where it currently is.
        let base = if self.refocus { self.camera_target } else { self.camera };
        let vmin = base.add(Point::new(MARGIN, TOOLBAR_H + MARGIN));
        let vmax = base.add(Point::new(window.x - MARGIN, window.y - MARGIN));
        let axis = |min: f32, max: f32, vmin: f32, vmax: f32| -> f32 {
            if max - min > vmax - vmin || min < vmin {
                min - vmin // doesn't fit, or sticks out top/left: align start
            } else if max > vmax {
                max - vmax
            } else {
                0.0
            }
        };
        let dx = axis(rect.min.x, rect.max.x, vmin.x, vmax.x);
        let dy = axis(rect.min.y, rect.max.y, vmin.y, vmax.y);
        if dx != 0.0 || dy != 0.0 {
            self.camera_target = base.add(Point::new(dx, dy));
            self.refocus = true;
        }
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

        // Hover feedback: track what's under the pointer (suppressed while
        // dragging so highlights don't chase the drag).
        let hover = if matches!(self.drag, DragState::None | DragState::Pending { .. }) {
            hit.map(|h| h.action)
        } else {
            None
        };
        if hover != self.hover {
            self.hover = hover;
            dirty = true;
        }

        // Mouse wheel over a node scrolls its content.
        let wheel = pointer.scroll_delta as f32;
        if wheel != 0.0 {
            if let Some(id) = hit.and_then(|h| h.drag) {
                if let Some(node) = arena.get_mut(id) {
                    let max_scroll = (node.content_h - node.rect.height()).max(0.0);
                    if max_scroll > 0.0 {
                        node.scroll = (node.scroll + wheel * 3.0).clamp(0.0, max_scroll);
                        dirty = true;
                    }
                }
            }
        }

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
const HOVER_ROW_BG: Rgba = Rgba([255, 255, 255, 26]); // white 0.10
const HOVER_BUTTON_BG: Rgba = Rgba([255, 255, 255, 38]); // white 0.15

fn inflate(r: Rect, by: f32) -> Rect {
    Rect { min: Point::new(r.min.x - by, r.min.y - by), max: Point::new(r.max.x + by, r.max.y + by) }
}

pub struct FrameOut {
    /// keep rendering continuously (spinners visible)
    pub animating: bool,
}

/// Build the whole frame: the canvas list (nodes + preview) renders into the
/// offscreen scene texture; the overlay list (scene composite + blurred band
/// + toolbar) renders into the swapchain. Fills `ui.hitboxes` for next
/// iteration's input.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    ui: &mut Ui,
    arena: &mut NodeArena,
    root: NodeId,
    ts: &mut TextSystem,
    canvas: &mut DrawList,
    overlay: &mut DrawList,
    window: Point,
    spin_angle: f32,
    preview: Option<(u32, u32)>,
    caret_visible: bool,
) -> FrameOut {
    ui.hitboxes.clear();
    let mut out = FrameOut { animating: false };
    let camera = Rect { min: ui.camera, max: ui.camera.add(window) };
    draw_entries(ui, arena, root, ts, canvas, camera, spin_angle, &mut out);
    draw_preview(canvas, window, preview);
    draw_navigation(ui, ts, overlay, window, caret_visible);
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
    let cap = node_max_size(camera.size());

    // Re-clamp the box and scroll every frame so window resizes keep the
    // 90% rule without a relayout.
    let (node_rect, scroll, content_h, is_root) = {
        let Some(node) = arena.get_mut(id) else { return };
        let box_w = node.content_w.min(cap.x);
        let box_h = node.content_h.min(cap.y);
        node.rect.max = Point::new(node.rect.min.x + box_w, node.rect.min.y + box_h);
        node.scroll = node.scroll.clamp(0.0, (node.content_h - box_h).max(0.0));
        (node.rect, node.scroll, node.content_h, node.parent.is_none())
    };

    if camera.intersects(node_rect) {
        let screen_rect = node_rect.offset(off);
        list.rect(screen_rect, COLOR_BOX_FILL, 5.0);
        list.rect_stroke(screen_rect, Rgba::WHITE, 5.0, 3.0);
        ui.hitboxes.push(Hitbox { area: screen_rect, action: Action::NodeBody, drag: Some(id) });
        if !is_root {
            // Close button: node.min + (2, -25), 20x20, like draw_entries.
            let r = Rect::from_xywh(node_rect.min.x + 2.0, node_rect.min.y - 25.0, 20.0, 20.0)
                .offset(off);
            let action = Action::CloseNode { node: id };
            if ui.hover == Some(action) {
                list.rect(inflate(r, 3.0), HOVER_BUTTON_BG, 5.0);
            }
            list.glyph_quad(r, ts.icon_uv(Icon::Close), Rgba::WHITE, 0.0);
            ui.hitboxes.push(Hitbox { area: r, action, drag: Some(id) });
        }
    }

    // Rows draw shifted by the scroll offset and clip to the box interior.
    let content_clip = Rect {
        min: Point::new(node_rect.min.x, node_rect.min.y + 2.0),
        max: Point::new(node_rect.max.x, node_rect.max.y - 2.0),
    };
    let scrolled = content_h > node_rect.height() + 0.5;

    let item_count = arena.get(id).map(|n| n.items.len()).unwrap_or(0);
    for i in 0..item_count {
        let Some(node) = arena.get(id) else { return };
        let item = &node.items[i];
        let rect = item.rect.offset(node_rect.min).offset(Point::new(0.0, -scroll)); // world
        let screen_rect = rect.offset(off);
        // The interactive row spans the node's inner width, not just the
        // text extent.
        let band = Rect {
            min: Point::new(node_rect.min.x + 2.0, rect.min.y),
            max: Point::new(node_rect.max.x - 2.0, rect.max.y),
        };
        let row_in_box = rect.max.y > content_clip.min.y && rect.min.y < content_clip.max.y;
        let in_view = camera.intersects(band) && row_in_box;
        let selected = ui.selection == Some((id, i));
        let child = item.child;
        let is_dir = item.is_dir;
        let scanning = item.scanning;
        let display = item.display.clone();
        // Side attachments (open button, spinner) hang off the box edge when
        // the row text is wider than the capped box.
        let side_x = rect.max.x.min(node_rect.max.x).max(node_rect.min.x) - camera.min.x;

        if in_view {
            let row_action = Action::Row { node: id, item: i };
            if ui.hover == Some(row_action) {
                if let Some(bg) = band.intersect(content_clip) {
                    list.rect(bg.offset(off), HOVER_ROW_BG, 3.0);
                }
            }
            let color = if selected {
                COLOR_ROW_SELECTED
            } else if child.is_some() {
                COLOR_ROW_OPEN
            } else {
                Rgba::WHITE
            };
            ts.draw_clipped(list, screen_rect.min, &display, color, content_clip.offset(off));
            if let Some(hb) = band.offset(off).intersect(content_clip.offset(off)) {
                ui.hitboxes.push(Hitbox { area: hb, action: row_action, drag: Some(id) });
            }
            if selected && !is_dir {
                let r = Rect::from_xywh(side_x + 10.0, screen_rect.min.y + 4.0, 20.0, 20.0);
                let action = Action::OpenWith { node: id, item: i };
                if ui.hover == Some(action) {
                    list.rect(inflate(r, 3.0), HOVER_BUTTON_BG, 5.0);
                }
                list.glyph_quad(r, ts.icon_uv(Icon::Open), Rgba::WHITE, 0.0);
                ui.hitboxes.push(Hitbox { area: r, action, drag: Some(id) });
            }
        }

        if scanning {
            // Busy spinner beside the row (the C app intended this but its
            // busy.svg never existed).
            if in_view {
                let r = Rect::from_xywh(side_x + 10.0, screen_rect.min.y + 2.0, 16.0, 16.0);
                list.glyph_quad(r, ts.icon_uv(Icon::Busy), Rgba::WHITE, spin_angle);
            }
            out.animating = true;
        } else if let Some(child_id) = child {
            draw_entries(ui, arena, child_id, ts, list, camera, spin_angle, out);
            if let Some(child_node) = arena.get(child_id) {
                // Anchor at the row's center, clamped to the box edge when
                // the row is scrolled out of view.
                let anchor_y = (rect.min.y + rect.height() / 2.0)
                    .clamp(node_rect.min.y + 5.0, node_rect.max.y - 5.0);
                list.line(
                    Point::new(node_rect.max.x, anchor_y).add(off),
                    Point::new(child_node.rect.min.x, child_node.rect.min.y + 5.0).add(off),
                    3.0,
                    Rgba::WHITE,
                );
            }
        }
    }

    // Scrollbar indicator on the right edge while content overflows.
    if scrolled && camera.intersects(node_rect) {
        let box_h = node_rect.height();
        let track_h = box_h - 8.0;
        let thumb_h = (track_h * box_h / content_h).max(20.0);
        let max_scroll = content_h - box_h;
        let t = if max_scroll > 0.0 { scroll / max_scroll } else { 0.0 };
        let thumb_y = node_rect.min.y + 4.0 + t * (track_h - thumb_h);
        let thumb = Rect::from_xywh(node_rect.max.x - 6.0, thumb_y, 3.0, thumb_h).offset(off);
        list.rect(thumb, Rgba::new(1.0, 1.0, 1.0, 0.35), 1.5);
    }
}

/// Port of draw_navigation: scene composite, frosted toolbar bar, six icon
/// buttons, URL bar.
fn draw_navigation(
    ui: &mut Ui,
    ts: &mut TextSystem,
    list: &mut DrawList,
    window: Point,
    caret_visible: bool,
) {
    // Composite the offscreen canvas, then the blurred band under the bar.
    list.image_slot(Rect::from_xywh(0.0, 0.0, window.x, window.y), TexSlot::Scene);
    let band = Rect::from_xywh(0.0, 0.0, window.x, TOOLBAR_H);
    list.image_slot(band, TexSlot::Blur);
    // Lighter tint than the C 0.9 so the frosted backdrop reads through.
    list.solid(band, Rgba::new(0.0, 0.0, 0.0, 0.55));
    list.line(
        Point::new(0.0, TOOLBAR_H),
        Point::new(window.x, TOOLBAR_H),
        1.0,
        Rgba::new(1.0, 1.0, 1.0, 0.3),
    );
    // The bar swallows clicks: rows hidden beneath it can't be clicked or
    // dragged. Pushed before the buttons so they still win (reverse scan).
    ui.hitboxes.push(Hitbox { area: band, action: Action::None, drag: None });

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
        if ui.hover == Some(action) {
            list.rect(inflate(r, 5.0), HOVER_BUTTON_BG, 5.0);
        }
        list.glyph_quad(r, ts.icon_uv(icon), Rgba::WHITE, 0.0);
        ui.hitboxes.push(Hitbox { area: r, action, drag: None });
        ox += size + padding;
    }

    // URL bar: y 16, height 31, to width-20 (draw_navigation's abwh quirk).
    let url_bar = Rect::from_xywh(ox, 16.0, window.x - ox - padding, 31.0);
    ui.url_bar_rect = url_bar;
    ui.hitboxes.push(Hitbox { area: url_bar, action: Action::UrlBar, drag: None });
    let focused = ui.url.active;
    list.rect_stroke(url_bar, Rgba::WHITE, 5.0, if focused { 2.0 } else { 1.0 });

    let clip = Rect {
        min: Point::new(url_bar.min.x + 5.0, url_bar.min.y),
        max: Point::new(url_bar.max.x - 5.0, url_bar.max.y),
    };
    let text_y = url_bar.min.y + 2.0;

    if focused {
        // Keep the caret in view within the field.
        let avail = (url_bar.width() - 2.0 * URL_PAD).max(10.0);
        let caret_x = ts.caret_x(&ui.url.text, ui.url.caret);
        if caret_x - ui.url.scroll > avail {
            ui.url.scroll = caret_x - avail;
        }
        if caret_x < ui.url.scroll {
            ui.url.scroll = caret_x;
        }
        let text_w = ts.measure(&ui.url.text).x;
        ui.url.scroll = ui.url.scroll.clamp(0.0, (text_w - avail).max(0.0));

        let origin = Point::new(url_bar.min.x + URL_PAD - ui.url.scroll, text_y);
        let lh = ts.line_height();
        if let Some((s, e)) = ui.url.selection() {
            let x0 = origin.x + ts.caret_x(&ui.url.text, s);
            let x1 = origin.x + ts.caret_x(&ui.url.text, e);
            let sel = Rect { min: Point::new(x0, text_y), max: Point::new(x1, text_y + lh) };
            if let Some(r) = sel.intersect(clip) {
                list.rect(r, Rgba::new(0.35, 0.55, 1.0, 0.45), 0.0);
            }
        }
        ts.draw_clipped(list, origin, &ui.url.text, Rgba::WHITE, clip);
        if caret_visible {
            let x = origin.x + caret_x;
            let caret =
                Rect { min: Point::new(x, text_y + 1.0), max: Point::new(x + 1.5, text_y + lh - 1.0) };
            if let Some(r) = caret.intersect(clip) {
                list.rect(r, Rgba::WHITE, 0.0);
            }
        }
    } else {
        let text = match &ui.selected_path {
            Some(p) => p.to_string_lossy().into_owned(),
            None => "No selection...".to_string(),
        };
        ts.draw_clipped(list, Point::new(url_bar.min.x + URL_PAD, text_y), &text, Rgba::WHITE, clip);
    }
}
