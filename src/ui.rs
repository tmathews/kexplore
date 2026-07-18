//! Immediate-mode UI: every frame `build_frame` emits both the draw list and
//! the hitbox list; `process_input` hit-tests the pointer against the
//! previous frame's hitboxes (same one-frame latency as the C app) and
//! returns a typed Action instead of the C trigger/type magic ints.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::geom::{Point, Rect};
use crate::gfx::renderer2d::{DrawList, Rgba, TexSlot};
use crate::model::{NodeArena, NodeId, ROW_ICON, ROW_ICON_GAP};
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
    /// Press target for a preview node's corner resize handle.
    ResizePreview { node: NodeId },
}

/// What a press-drag on a hitbox does once it crosses the move threshold.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Pan the camera.
    None,
    /// Move this node.
    Node(NodeId),
    /// Aspect-locked resize of this preview node from its corner.
    Resize(NodeId),
}

impl DragKind {
    /// The node this drag concerns, if any (used for wheel-scroll targeting).
    fn node(self) -> Option<NodeId> {
        match self {
            DragKind::Node(id) | DragKind::Resize(id) => Some(id),
            DragKind::None => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Hitbox {
    pub area: Rect, // screen space (logical px)
    pub action: Action,
    /// What a drag starting on this box does.
    pub drag: DragKind,
}

pub enum DragState {
    None,
    /// Button is down but movement hasn't crossed the threshold yet.
    Pending { origin: Point, kind: DragKind },
    Node(NodeId),
    Resize(NodeId),
}

pub struct Ui {
    pub camera: Point, // world point shown at the content origin (screen 0,0)
    pub camera_target: Point,
    pub refocus: bool,
    /// Canvas zoom: screen = (world - camera) * zoom. 1.0 is 1:1.
    pub zoom: f32,
    zoom_target: f32,
    /// World point kept fixed under the cursor during a smooth zoom, and its
    /// screen position; the camera is derived from these each zoom step.
    zoom_pivot_world: Point,
    zoom_pivot_screen: Point,
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
    /// Previous click for double-click detection.
    last_click: Option<(Action, std::time::Instant)>,
}

/// The world↔screen transform for canvas content. Screen/logical px; the
/// DrawList applies HiDPI scale on top. Toolbar chrome bypasses this.
#[derive(Clone, Copy)]
pub struct View {
    pub cam: Point,
    pub zoom: f32,
    pub window: Point,
}

impl View {
    pub fn w2s(&self, p: Point) -> Point {
        p.sub(self.cam).scale(self.zoom)
    }
    pub fn w2s_rect(&self, r: Rect) -> Rect {
        Rect { min: self.w2s(r.min), max: self.w2s(r.max) }
    }
    /// World region currently visible on screen (for culling).
    pub fn visible(&self) -> Rect {
        Rect { min: self.cam, max: self.cam.add(self.window.scale(1.0 / self.zoom)) }
    }
}

/// Camera lerp: C used a fixed 0.16 per frame at ~60Hz; this is the
/// frame-rate independent equivalent.
const LERP_RATE: f32 = 10.5;

/// Zoom limits and how much one wheel notch multiplies the zoom target.
/// Zoom is overview-only (out to 1:1): capping at 1.0 keeps canvas text
/// always downscaled from the atlas, so it never blurs. (Zooming in past 1:1
/// would need the atlas re-rasterized at the effective scale.)
const ZOOM_MIN: f32 = 0.15;
const ZOOM_MAX: f32 = 1.0;
const ZOOM_PER_NOTCH: f32 = 1.15;
/// Zoom smoothing rate (frame-rate independent, a touch snappier than pan).
const ZOOM_LERP_RATE: f32 = 16.0;

impl Ui {
    pub fn new() -> Ui {
        Ui {
            camera: Point::ZERO,
            camera_target: Point::ZERO,
            refocus: false,
            zoom: 1.0,
            zoom_target: 1.0,
            zoom_pivot_world: Point::ZERO,
            zoom_pivot_screen: Point::ZERO,
            selection: None,
            selected_path: None,
            hitboxes: Vec::new(),
            drag: DragState::None,
            url: TextField::new(),
            url_bar_rect: Rect::ZERO,
            last_pointer: Point::ZERO,
            hover: None,
            last_click: None,
        }
    }

    /// Center `rect` (world) in the safe area below the toolbar at the current
    /// zoom; if it's taller than the view, align its top ~100px below the bar.
    /// Screen offsets are converted to world by dividing by zoom.
    pub fn focus_to_rect(&mut self, rect: Rect, window: Point) {
        let z = self.zoom;
        let avail_h = (window.y - TOOLBAR_H).max(1.0);
        let cam_x = rect.center().x - (window.x * 0.5) / z;
        let cam_y = if rect.height() * z > avail_h {
            rect.min.y - (TOOLBAR_H + 100.0) / z
        } else {
            rect.center().y - (TOOLBAR_H + avail_h * 0.5) / z
        };
        self.camera_target = Point::new(cam_x, cam_y);
        self.refocus = true;
    }

    /// Pan just enough that `rect` (a freshly opened node) sits inside the safe
    /// area below the toolbar, with a margin; no-op when already fully visible.
    /// The visible region is `window / zoom` in world units.
    pub fn ensure_visible(&mut self, rect: Rect, window: Point) {
        const MARGIN: f32 = 12.0;
        let z = self.zoom;
        // Include the close button hanging above the box.
        let rect = Rect { min: Point::new(rect.min.x, rect.min.y - 30.0), max: rect.max };
        // If a lerp is already in flight, judge against where it is headed.
        let base = if self.refocus { self.camera_target } else { self.camera };
        // Safe-area bounds in world units (screen offsets scaled by 1/zoom).
        let vmin = base.add(Point::new(MARGIN, TOOLBAR_H + MARGIN).scale(1.0 / z));
        let vmax = base.add(Point::new(window.x - MARGIN, window.y - MARGIN).scale(1.0 / z));
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

    /// Queue a smooth zoom by `notches` wheel steps, keeping the world point
    /// under `cursor` (screen px) fixed as the zoom animates.
    pub fn zoom_at(&mut self, cursor: Point, notches: f32) {
        self.refocus = false; // zoom drives the camera now, not a focus lerp
        self.zoom_pivot_screen = cursor;
        self.zoom_pivot_world = cursor.scale(1.0 / self.zoom).add(self.camera);
        self.zoom_target = (self.zoom_target * ZOOM_PER_NOTCH.powf(notches)).clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// Advance the camera pan lerp and the zoom lerp; returns true while either
    /// is still animating.
    pub fn step_camera(&mut self, dt: f32) -> bool {
        let mut animating = false;
        if (self.zoom - self.zoom_target).abs() > 0.0005 {
            let t = 1.0 - (-ZOOM_LERP_RATE * dt).exp();
            self.zoom += (self.zoom_target - self.zoom) * t;
            if (self.zoom - self.zoom_target).abs() < 0.001 {
                self.zoom = self.zoom_target;
            }
            // Keep the pivot's world point under its original screen position.
            self.camera = self.zoom_pivot_world.sub(self.zoom_pivot_screen.scale(1.0 / self.zoom));
            animating = true;
        }
        if self.refocus {
            let t = 1.0 - (-LERP_RATE * dt).exp();
            self.camera = self.camera.lerp(self.camera_target, t);
            if self.camera_target.sub(self.camera).length() < 0.5 {
                self.camera = self.camera_target;
                self.refocus = false;
            }
            animating = true;
        }
        animating
    }

    /// Hit-test, drag handling, and click dispatch. Returns the clicked
    /// action with a double-click flag (if any) and whether the frame is
    /// dirty from input.
    pub fn process_input(
        &mut self,
        arena: &mut NodeArena,
        pointer: &PointerState,
    ) -> (Option<(Action, bool)>, bool) {
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

        // Mouse wheel: scroll a hovered node that has overflow, otherwise zoom
        // the canvas about the cursor (smoothed).
        let wheel = pointer.scroll_delta as f32;
        if wheel != 0.0 {
            let scrolled = hit.and_then(|h| h.drag.node()).and_then(|id| {
                arena.get_mut(id).and_then(|node| {
                    let max_scroll = (node.content_h - node.rect.height()).max(0.0);
                    (max_scroll > 0.0).then(|| {
                        node.scroll = (node.scroll + wheel * 3.0).clamp(0.0, max_scroll);
                    })
                })
            });
            if scrolled.is_some() {
                dirty = true;
            } else {
                // One wheel notch ~= 15 logical px; zoom in on scroll-up.
                self.zoom_at(cursor, -wheel / 15.0);
                dirty = true;
            }
        }

        // Middle-button drag pans the canvas (world delta = screen delta / zoom).
        if pointer.middle_down {
            if !pointer.middle_pressed {
                let delta = cursor.sub(self.last_pointer);
                if delta.x != 0.0 || delta.y != 0.0 {
                    self.camera = self.camera.sub(delta.scale(1.0 / self.zoom));
                    self.refocus = false;
                    dirty = true;
                }
            }
        }

        if pointer.pressed {
            let kind = hit.map(|h| h.drag).unwrap_or(DragKind::None);
            self.drag = DragState::Pending { origin: cursor, kind };
        }

        if pointer.is_down {
            // Pointer delta in world units (screen delta / zoom).
            let delta = cursor.sub(self.last_pointer).scale(1.0 / self.zoom);
            if let DragState::Pending { origin, kind } = self.drag {
                if cursor.sub(origin).length() >= 1.0 {
                    // Left-drag on empty canvas is inert (reserved for later);
                    // panning is middle-button only.
                    self.drag = match kind {
                        DragKind::Node(id) => DragState::Node(id),
                        DragKind::Resize(id) => DragState::Resize(id),
                        DragKind::None => DragState::None,
                    };
                }
            }
            match self.drag {
                DragState::Node(id) => {
                    if let Some(node) = arena.get_mut(id) {
                        // Grabbing a node cancels any in-flight collision glide.
                        node.anim_to = None;
                        node.rect = node.rect.offset(delta);
                        dirty = true;
                    }
                }
                DragState::Resize(id) => {
                    // Aspect-locked resize from the corner: width follows the
                    // world-space cursor, height keeps the image's aspect.
                    if let Some(node) = arena.get_mut(id) {
                        if let Some(pv) = node.preview {
                            let aspect = (pv.img_w.max(1) as f32) / (pv.img_h.max(1) as f32);
                            let world_x = cursor.x / self.zoom + self.camera.x;
                            let new_w = (world_x - node.rect.min.x).max(PREVIEW_MIN_W);
                            let new_h = new_w / aspect;
                            node.anim_to = None;
                            node.rect.max =
                                Point::new(node.rect.min.x + new_w, node.rect.min.y + new_h);
                            node.content_w = new_w;
                            node.content_h = new_h;
                            dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }
        self.last_pointer = cursor;

        let mut action = None;
        if pointer.released {
            if let DragState::Pending { .. } = self.drag {
                if let Some(h) = hit {
                    let now = std::time::Instant::now();
                    let double = self
                        .last_click
                        .take()
                        .is_some_and(|(a, t)| a == h.action && now.duration_since(t).as_millis() < DOUBLE_CLICK_MS);
                    // A consumed double doesn't seed the next one (a triple
                    // click is not two doubles).
                    if !double {
                        self.last_click = Some((h.action, now));
                    }
                    action = Some((h.action, double));
                } else {
                    self.last_click = None;
                }
            }
            if let DragState::Node(id) | DragState::Resize(id) = self.drag {
                // Snap-on-release: if the dropped/resized node overlaps others,
                // glide it to the nearest free spot (search all four directions).
                if let Some(cand) = arena.get(id).map(|n| n.rect) {
                    let obstacles: Vec<Rect> = arena
                        .iter()
                        .filter(|(other, _)| *other != id)
                        .map(|(_, n)| n.rect)
                        .collect();
                    let target = crate::model::resolve_collision(cand, &obstacles, true);
                    if target != cand {
                        if let Some(node) = arena.get_mut(id) {
                            node.anim_to = Some(target.min);
                        }
                    }
                }
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
/// Zebra striping on alternate rows — fainter than the hover highlight so it
/// never competes with it.
const ALT_ROW_BG: Rgba = Rgba([255, 255, 255, 10]); // white ~0.04
/// Border for nodes on the chain from root to the current selection.
const COLOR_PATH_BORDER: Rgba = Rgba([255, 191, 64, 255]);

/// Two clicks on the same target within this window count as a double click.
const DOUBLE_CLICK_MS: u128 = 400;

/// Smallest width (logical px) an image-preview node can be resized to.
const PREVIEW_MIN_W: f32 = 100.0;
/// Size (logical px) of the corner resize handle on a preview node.
const PREVIEW_HANDLE: f32 = 16.0;

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
    caret_visible: bool,
) -> FrameOut {
    ui.hitboxes.clear();
    let mut out = FrameOut { animating: false };
    let view = View { cam: ui.camera, zoom: ui.zoom, window };
    // The 90% box cap is in world units (zoom-independent), based on window.
    let cap = node_max_size(window);
    let path = path_nodes(arena, ui.selection);
    // The current canvas root's path is the URL bar's default when nothing is
    // selected (there is always a node open).
    let root_path = arena.get(root).map(|n| n.path.clone());
    // Connectors are emitted first so every node box paints over them — the
    // lines pass *under* the nodes rather than across their content.
    draw_connectors(arena, root, canvas, view, cap, &path);
    draw_entries(ui, arena, root, ts, canvas, view, spin_angle, &path, &mut out);
    draw_navigation(ui, ts, overlay, window, caret_visible, root_path.as_deref());
    out
}

/// The set of nodes forming the route shown in the URL bar: the node holding
/// the selection, all of its ancestors, and the opened child of the selected
/// item (whose path equals the URL when the selection is an open directory).
fn path_nodes(arena: &NodeArena, selection: Option<(NodeId, usize)>) -> HashSet<NodeId> {
    let mut set = HashSet::new();
    let Some((node, item)) = selection else { return set };
    if let Some(n) = arena.get(node) {
        if let Some(child) = n.items.get(item).and_then(|it| it.child) {
            set.insert(child);
        }
    }
    let mut cur = Some(node);
    while let Some(id) = cur {
        set.insert(id);
        cur = arena.get(id).and_then(|n| n.parent.map(|(p, _)| p));
    }
    set
}

/// Re-clamp every node and emit the parent→child connector lines. Kept
/// separate from `draw_entries` so all lines land in the draw list before any
/// box, guaranteeing they render beneath the nodes.
fn draw_connectors(
    arena: &mut NodeArena,
    id: NodeId,
    list: &mut DrawList,
    view: View,
    cap: Point,
    path: &HashSet<NodeId>,
) {
    let (node_rect, scroll) = {
        let Some(node) = arena.get_mut(id) else { return };
        // Preview boxes are user-sized; only directory boxes get re-clamped.
        if node.preview.is_none() {
            let box_w = node.content_w.min(cap.x);
            let box_h = node.content_h.min(cap.y);
            node.rect.max = Point::new(node.rect.min.x + box_w, node.rect.min.y + box_h);
            node.scroll = node.scroll.clamp(0.0, (node.content_h - box_h).max(0.0));
        }
        (node.rect, node.scroll)
    };
    let visible = view.visible();
    let item_count = arena.get(id).map(|n| n.items.len()).unwrap_or(0);
    for i in 0..item_count {
        let Some(node) = arena.get(id) else { return };
        let item = &node.items[i];
        let Some(child_id) = item.child else { continue };
        let item_rect = item.rect;
        let rect = item_rect.offset(node_rect.min).offset(Point::new(0.0, -scroll));
        // Anchor at the row center, clamped to the box edge when the row is
        // scrolled out of view. All world coords; mapped to screen at emit.
        let anchor_y =
            (rect.min.y + rect.height() / 2.0).clamp(node_rect.min.y + 5.0, node_rect.max.y - 5.0);
        if let Some(child_node) = arena.get(child_id) {
            let a = Point::new(node_rect.max.x, anchor_y);
            let b = Point::new(child_node.rect.min.x, child_node.rect.min.y + 5.0);
            let seg = Rect {
                min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                max: Point::new(a.x.max(b.x), a.y.max(b.y)),
            };
            if visible.intersects(seg) {
                // Highlight the line amber when it is an edge of the active
                // route (its child node is on the selection path).
                let color =
                    if path.contains(&child_id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
                let width = if path.contains(&child_id) { 4.0 } else { 3.0 };
                list.line(view.w2s(a), view.w2s(b), width, color);
            }
        }
        draw_connectors(arena, child_id, list, view, cap, path);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_entries(
    ui: &mut Ui,
    arena: &mut NodeArena,
    id: NodeId,
    ts: &mut TextSystem,
    list: &mut DrawList,
    view: View,
    spin_angle: f32,
    path: &HashSet<NodeId>,
    out: &mut FrameOut,
) {
    let cap = node_max_size(view.window);
    let visible = view.visible();
    let z = view.zoom;

    // Re-clamp the box and scroll every frame so window resizes keep the
    // 90% rule without a relayout. Preview boxes are user-sized and skip this.
    let (node_rect, scroll, content_h, is_root, preview_tex) = {
        let Some(node) = arena.get_mut(id) else { return };
        let preview_tex = node.preview.map(|p| p.tex);
        if preview_tex.is_none() {
            let box_w = node.content_w.min(cap.x);
            let box_h = node.content_h.min(cap.y);
            node.rect.max = Point::new(node.rect.min.x + box_w, node.rect.min.y + box_h);
            node.scroll = node.scroll.clamp(0.0, (node.content_h - box_h).max(0.0));
        }
        (node.rect, node.scroll, node.content_h, node.parent.is_none(), preview_tex)
    };

    // World-space close button rect for a non-root node (hangs above top-left).
    let close_world =
        Rect::from_xywh(node_rect.min.x + 2.0, node_rect.min.y - 25.0, 20.0, 20.0);

    // Preview node: draw the image, a close button, and a corner resize
    // handle, then stop — no rows, no scroll, no children.
    if let Some(tex) = preview_tex {
        if visible.intersects(node_rect) {
            let screen = view.w2s_rect(node_rect);
            list.rect(screen, COLOR_BOX_FILL, 3.0); // shows through transparency
            let img = Rect {
                min: Point::new(screen.min.x + 2.0, screen.min.y + 2.0),
                max: Point::new(screen.max.x - 2.0, screen.max.y - 2.0),
            };
            list.image_tex(img, tex);
            let border = if path.contains(&id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
            list.rect_stroke(screen, border, 3.0, 2.0);
            ui.hitboxes.push(Hitbox { area: screen, action: Action::NodeBody, drag: DragKind::Node(id) });
            // Close button.
            let cr = view.w2s_rect(close_world);
            let caction = Action::CloseNode { node: id };
            if ui.hover == Some(caction) {
                list.rect(inflate(cr, 3.0), HOVER_BUTTON_BG, 5.0);
            }
            list.glyph_quad(cr, ts.icon_uv(Icon::Close), Rgba::WHITE, 0.0);
            ui.hitboxes.push(Hitbox { area: cr, action: caction, drag: DragKind::Node(id) });
            // Corner resize handle (bottom-right): two short edge lines.
            let hr = view.w2s_rect(Rect::from_xywh(
                node_rect.max.x - PREVIEW_HANDLE,
                node_rect.max.y - PREVIEW_HANDLE,
                PREVIEW_HANDLE,
                PREVIEW_HANDLE,
            ));
            let raction = Action::ResizePreview { node: id };
            let hc = if ui.hover == Some(raction) { Rgba::WHITE } else { Rgba::new(1.0, 1.0, 1.0, 0.6) };
            list.line(Point::new(hr.min.x, hr.max.y), Point::new(hr.max.x, hr.max.y), 2.0, hc);
            list.line(Point::new(hr.max.x, hr.min.y), Point::new(hr.max.x, hr.max.y), 2.0, hc);
            ui.hitboxes.push(Hitbox { area: hr, action: raction, drag: DragKind::Resize(id) });
        }
        return;
    }

    if visible.intersects(node_rect) {
        let screen_rect = view.w2s_rect(node_rect);
        list.rect(screen_rect, COLOR_BOX_FILL, 5.0);
        let border = if path.contains(&id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
        list.rect_stroke(screen_rect, border, 5.0, 3.0);
        ui.hitboxes.push(Hitbox { area: screen_rect, action: Action::NodeBody, drag: DragKind::Node(id) });
        if !is_root {
            let r = view.w2s_rect(close_world);
            let action = Action::CloseNode { node: id };
            if ui.hover == Some(action) {
                list.rect(inflate(r, 3.0), HOVER_BUTTON_BG, 5.0);
            }
            list.glyph_quad(r, ts.icon_uv(Icon::Close), Rgba::WHITE, 0.0);
            ui.hitboxes.push(Hitbox { area: r, action, drag: DragKind::Node(id) });
        }
    }

    // Rows draw shifted by the scroll offset and clip to the box interior.
    // Everything below is computed in world coords and mapped via `view`.
    let content_clip = Rect {
        min: Point::new(node_rect.min.x, node_rect.min.y + 2.0),
        max: Point::new(node_rect.max.x, node_rect.max.y - 2.0),
    };
    let clip_screen = view.w2s_rect(content_clip);
    let scrolled = content_h > node_rect.height() + 0.5;

    let item_count = arena.get(id).map(|n| n.items.len()).unwrap_or(0);
    for i in 0..item_count {
        let Some(node) = arena.get(id) else { return };
        let item = &node.items[i];
        let rect = item.rect.offset(node_rect.min).offset(Point::new(0.0, -scroll)); // world
        // The interactive row spans the node's inner width, not just the text.
        let band = Rect {
            min: Point::new(node_rect.min.x + 2.0, rect.min.y),
            max: Point::new(node_rect.max.x - 2.0, rect.max.y),
        };
        let row_in_box = rect.max.y > content_clip.min.y && rect.min.y < content_clip.max.y;
        let in_view = visible.intersects(band) && row_in_box;
        let selected = ui.selection == Some((id, i));
        let child = item.child;
        let is_dir = item.is_dir;
        let scanning = item.scanning;
        let display = item.display.clone();
        // Side attachments (open button, spinner) hang off the box edge in
        // world x when the row text is wider than the capped box.
        let side_x = rect.max.x.min(node_rect.max.x).max(node_rect.min.x);

        if in_view {
            let row_action = Action::Row { node: id, item: i };
            // Zebra striping on odd rows (#15), under the hover highlight.
            if i % 2 == 1 {
                if let Some(bg) = band.intersect(content_clip) {
                    list.rect(view.w2s_rect(bg), ALT_ROW_BG, 0.0);
                }
            }
            if ui.hover == Some(row_action) {
                if let Some(bg) = band.intersect(content_clip) {
                    list.rect(view.w2s_rect(bg), HOVER_ROW_BG, 3.0);
                }
            }
            let color = if selected {
                COLOR_ROW_SELECTED
            } else if child.is_some() {
                COLOR_ROW_OPEN
            } else {
                Rgba::WHITE
            };
            // File-type icon (#14): folder for directories, page for files.
            // Only drawn when the row sits fully inside the box so it never
            // pokes past a scrolled edge (text uses draw_clipped for that).
            if rect.min.y >= content_clip.min.y - 0.5 && rect.max.y <= content_clip.max.y + 0.5 {
                let icon = if is_dir { Icon::Folder } else { Icon::File };
                let iw = Rect::from_xywh(
                    rect.min.x,
                    rect.min.y + (rect.height() - ROW_ICON) * 0.5,
                    ROW_ICON,
                    ROW_ICON,
                );
                list.glyph_quad(view.w2s_rect(iw), ts.icon_uv(icon), color, 0.0);
            }
            let text_origin = view.w2s(Point::new(rect.min.x + ROW_ICON + ROW_ICON_GAP, rect.min.y));
            ts.draw_clipped(list, text_origin, &display, color, clip_screen, z);
            if let Some(hb) = band.intersect(content_clip) {
                ui.hitboxes.push(Hitbox {
                    area: view.w2s_rect(hb),
                    action: row_action,
                    drag: DragKind::Node(id),
                });
            }
            if selected && !is_dir {
                let r = view.w2s_rect(Rect::from_xywh(side_x + 10.0, rect.min.y + 4.0, 20.0, 20.0));
                let action = Action::OpenWith { node: id, item: i };
                if ui.hover == Some(action) {
                    list.rect(inflate(r, 3.0), HOVER_BUTTON_BG, 5.0);
                }
                list.glyph_quad(r, ts.icon_uv(Icon::Open), Rgba::WHITE, 0.0);
                ui.hitboxes.push(Hitbox { area: r, action, drag: DragKind::Node(id) });
            }
        }

        if scanning {
            // Busy spinner beside the row (the C app intended this but its
            // busy.svg never existed).
            if in_view {
                let r = view.w2s_rect(Rect::from_xywh(side_x + 10.0, rect.min.y + 2.0, 16.0, 16.0));
                list.glyph_quad(r, ts.icon_uv(Icon::Busy), Rgba::WHITE, spin_angle);
            }
            out.animating = true;
        } else if let Some(child_id) = child {
            // Connector lines were emitted by draw_connectors (under all
            // boxes); here we only recurse to draw the child subtree.
            draw_entries(ui, arena, child_id, ts, list, view, spin_angle, path, out);
        }
    }

    // Scrollbar indicator on the right edge while content overflows.
    if scrolled && visible.intersects(node_rect) {
        let box_h = node_rect.height();
        let track_h = box_h - 8.0;
        let thumb_h = (track_h * box_h / content_h).max(20.0);
        let max_scroll = content_h - box_h;
        let t = if max_scroll > 0.0 { scroll / max_scroll } else { 0.0 };
        let thumb_y = node_rect.min.y + 4.0 + t * (track_h - thumb_h);
        let thumb = Rect::from_xywh(node_rect.max.x - 6.0, thumb_y, 3.0, thumb_h);
        list.rect(view.w2s_rect(thumb), Rgba::new(1.0, 1.0, 1.0, 0.35), 1.5);
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
    active_path: Option<&Path>,
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
    ui.hitboxes.push(Hitbox { area: band, action: Action::None, drag: DragKind::None });

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
        ui.hitboxes.push(Hitbox { area: r, action, drag: DragKind::None });
        ox += size + padding;
    }

    // URL bar: y 16, height 31, to width-20 (draw_navigation's abwh quirk).
    let url_bar = Rect::from_xywh(ox, 16.0, window.x - ox - padding, 31.0);
    ui.url_bar_rect = url_bar;
    ui.hitboxes.push(Hitbox { area: url_bar, action: Action::UrlBar, drag: DragKind::None });
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
        ts.draw_clipped(list, origin, &ui.url.text, Rgba::WHITE, clip, 1.0);
        if caret_visible {
            let x = origin.x + caret_x;
            let caret =
                Rect { min: Point::new(x, text_y + 1.0), max: Point::new(x + 1.5, text_y + lh - 1.0) };
            if let Some(r) = caret.intersect(clip) {
                list.rect(r, Rgba::WHITE, 0.0);
            }
        }
    } else {
        // Show the selected path, or fall back to the active (root) node's
        // path so the bar always reflects where you are.
        let text = match ui.selected_path.as_deref().or(active_path) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => "No selection...".to_string(),
        };
        ts.draw_clipped(list, Point::new(url_bar.min.x + URL_PAD, text_y), &text, Rgba::WHITE, clip, 1.0);
    }
}
