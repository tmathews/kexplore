//! Immediate-mode UI: every frame `build_frame` emits both the draw list and
//! the hitbox list; `process_input` hit-tests the pointer against the
//! previous frame's hitboxes (same one-frame latency as the C app) and
//! returns a typed Action instead of the C trigger/type magic ints.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::geom::{Point, Rect};
use crate::gfx::renderer2d::{DrawList, Rgba, TexSlot};
use crate::model::{FileView, Node, NodeArena, NodeId, HEADER_H, ROW_ICON, ROW_ICON_GAP};
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
    FocusHome,
    FocusSelection,
    /// Go up one level: navigate to the parent of the current location.
    GoUp,
    CopyPath,
    OpenTerminal,
    Row { node: NodeId, item: usize },
    /// Click on a node's body/header: make it the active node.
    NodeBody { node: NodeId },
    CloseNode { node: NodeId },
    /// Toggle the node's path in the favorites set (header star button).
    ToggleFavorite { node: NodeId },
    /// Pin a transient node (file or directory) so it stays open.
    PinNode { node: NodeId },
    /// Press target for a node's corner resize handle.
    ResizeNode { node: NodeId },
    /// A row in the right-click context menu (acts on `ui.context_menu`).
    Menu(MenuItem),
}

/// The entries in the right-click context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Open,
    OpenTerminal,
    CopyFile,
    CopyPath,
}

/// The menu rows, in display order.
pub const MENU_ITEMS: [(MenuItem, &str); 4] = [
    (MenuItem::Open, "Open"),
    (MenuItem::OpenTerminal, "Open Terminal Here"),
    (MenuItem::CopyFile, "Copy File"),
    (MenuItem::CopyPath, "Copy File Path"),
];

/// An open right-click context menu, anchored at `pos` (screen px), acting on
/// the row it was opened over.
pub struct ContextMenu {
    pub pos: Point,
    pub node: NodeId,
    pub item: usize,
    pub path: PathBuf,
    pub is_dir: bool,
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
    /// Rubber-band selection: a left-drag started on empty canvas. `origin` is
    /// the fixed corner (screen px); the moving corner is the live cursor.
    Marquee { origin: Point },
}

/// What an in-progress touchpad 2-finger scroll gesture is doing. Locked at
/// the start of the gesture so panning across a scrollable node keeps panning.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollLock {
    None,
    Pan,
    Node(NodeId),
}

/// A finger-scroll gap longer than this (ms) starts a fresh gesture, so the
/// target is re-evaluated only after the fingers lift.
const SCROLL_GESTURE_GAP_MS: u128 = 150;

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
    /// The node the URL bar and amber route reflect: whatever you last opened
    /// or clicked. Replaces the old (node, row) selection — a file or directory
    /// is now always a whole node.
    pub active_node: Option<NodeId>,
    /// Nodes in the current marquee multi-selection. Dragging any one of them
    /// moves the whole set; a click on empty canvas clears it.
    pub selected_nodes: HashSet<NodeId>,
    /// Path of the active node, cached for the URL bar / clipboard / terminal.
    pub selected_path: Option<PathBuf>,
    /// Paths the user has starred (in-memory for now; #10 adds persistence).
    pub favorites: HashSet<PathBuf>,
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
    /// Locked target of the current touchpad scroll gesture, and the time of
    /// its last scroll event (a long gap ends the gesture).
    scroll_lock: ScrollLock,
    last_scroll: Option<std::time::Instant>,
    /// The open right-click context menu, if any, and its drawn rect (screen
    /// px, from the last frame) for click-away detection.
    pub context_menu: Option<ContextMenu>,
    menu_rect: Rect,
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
            active_node: None,
            selected_nodes: HashSet::new(),
            selected_path: None,
            favorites: HashSet::new(),
            hitboxes: Vec::new(),
            drag: DragState::None,
            url: TextField::new(),
            url_bar_rect: Rect::ZERO,
            last_pointer: Point::ZERO,
            hover: None,
            last_click: None,
            scroll_lock: ScrollLock::None,
            last_scroll: None,
            context_menu: None,
            menu_rect: Rect::ZERO,
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

    /// Make `node` the active node (what the URL bar and amber route reflect),
    /// caching its path. `None` clears the active node.
    pub fn set_active(&mut self, arena: &NodeArena, node: Option<NodeId>) {
        self.active_node = node;
        self.selected_path = node.and_then(|id| arena.get(id)).map(|n| n.path.clone());
    }

    /// Apply a pinch zoom `factor` immediately (the gesture is continuous, so
    /// it isn't smoothed), keeping the world point under `cursor` fixed.
    pub fn pinch_zoom(&mut self, cursor: Point, factor: f32) {
        self.refocus = false;
        let pivot_world = cursor.scale(1.0 / self.zoom).add(self.camera);
        let new_zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.zoom = new_zoom;
        self.zoom_target = new_zoom;
        self.camera = pivot_world.sub(cursor.scale(1.0 / new_zoom));
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

        // Right-click over a row opens the context menu there.
        if pointer.right_pressed {
            if let Some(Action::Row { node, item }) = hit.map(|h| h.action) {
                if let Some(n) = arena.get(node) {
                    if let Some(it) = n.items.get(item) {
                        self.context_menu = Some(ContextMenu {
                            pos: cursor,
                            node,
                            item,
                            path: n.path.join(&it.name),
                            is_dir: it.is_dir,
                        });
                        dirty = true;
                    }
                }
            }
        }

        // A left press with the menu open and the cursor outside it dismisses
        // the menu and is swallowed (it doesn't also act on the canvas).
        let mut swallow_press = false;
        if pointer.pressed && self.context_menu.is_some() && !self.menu_rect.contains(cursor) {
            self.context_menu = None;
            swallow_press = true;
            dirty = true;
        }

        // Touchpad pinch: zoom about the cursor, applied immediately (the
        // gesture is already continuous, so no extra smoothing).
        if pointer.pinch > 0.0 && (pointer.pinch - 1.0).abs() > 0.0001 {
            self.pinch_zoom(cursor, pointer.pinch as f32);
            dirty = true;
        }

        // Scroll. A mouse wheel scrolls an overflowing node under the cursor,
        // otherwise it zooms. A touchpad 2-finger scroll is one continuous
        // gesture whose target — pan the canvas, or scroll a node — is locked
        // at the start, so panning across a scrollable node keeps panning; you
        // only scroll a node when the gesture *begins* on it.
        let dx = pointer.scroll_dx as f32;
        let dy = pointer.scroll_dy as f32;
        if dx != 0.0 || dy != 0.0 {
            if pointer.scroll_finger {
                let now = std::time::Instant::now();
                let new_gesture = self.scroll_lock == ScrollLock::None
                    || self.last_scroll.map_or(true, |t| {
                        now.duration_since(t).as_millis() > SCROLL_GESTURE_GAP_MS
                    });
                if new_gesture {
                    let on_scrollable = hit.and_then(|h| h.drag.node()).filter(|id| {
                        arena.get(*id).is_some_and(|n| n.content_h - n.rect.height() > 0.5)
                    });
                    self.scroll_lock = match on_scrollable {
                        Some(id) => ScrollLock::Node(id),
                        None => ScrollLock::Pan,
                    };
                }
                self.last_scroll = Some(now);
                match self.scroll_lock {
                    ScrollLock::Node(id) => {
                        if let Some(node) = arena.get_mut(id) {
                            let max = (node.content_h - node.rect.height()).max(0.0);
                            node.scroll = (node.scroll + dy).clamp(0.0, max);
                        }
                    }
                    _ => {
                        // Pan: drag the canvas with the fingers (grab-style).
                        self.camera = self.camera.sub(Point::new(dx, dy).scale(1.0 / self.zoom));
                        self.refocus = false;
                    }
                }
                dirty = true;
            } else {
                let node_scrolled = dy != 0.0
                    && hit
                        .and_then(|h| h.drag.node())
                        .and_then(|id| {
                            arena.get_mut(id).and_then(|node| {
                                let max = (node.content_h - node.rect.height()).max(0.0);
                                (max > 0.0)
                                    .then(|| node.scroll = (node.scroll + dy * 3.0).clamp(0.0, max))
                            })
                        })
                        .is_some();
                if !node_scrolled {
                    // One wheel notch ~= 15 logical px; zoom in on scroll-up.
                    self.zoom_at(cursor, -dy / 15.0);
                }
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

        if pointer.pressed && !swallow_press {
            let kind = hit.map(|h| h.drag).unwrap_or(DragKind::None);
            self.drag = DragState::Pending { origin: cursor, kind };
        }

        if pointer.is_down {
            // Pointer delta in world units (screen delta / zoom).
            let delta = cursor.sub(self.last_pointer).scale(1.0 / self.zoom);
            if let DragState::Pending { origin, kind } = self.drag {
                if cursor.sub(origin).length() >= 1.0 {
                    // Left-drag on empty canvas rubber-bands a selection;
                    // panning is middle-button only.
                    self.drag = match kind {
                        DragKind::Node(id) => {
                            // Grabbing a node outside the current multi-selection
                            // starts a fresh single-node move, so clear the set.
                            if !self.selected_nodes.contains(&id) {
                                self.selected_nodes.clear();
                            }
                            DragState::Node(id)
                        }
                        DragKind::Resize(id) => DragState::Resize(id),
                        DragKind::None => DragState::Marquee { origin },
                    };
                }
            }
            match self.drag {
                DragState::Node(id) => {
                    // If the grabbed node is part of the multi-selection, move
                    // the whole set together; otherwise just this one.
                    if self.selected_nodes.len() > 1 && self.selected_nodes.contains(&id) {
                        for &nid in &self.selected_nodes {
                            if let Some(node) = arena.get_mut(nid) {
                                node.anim_to = None;
                                node.rect = node.rect.offset(delta);
                            }
                        }
                        dirty = true;
                    } else if let Some(node) = arena.get_mut(id) {
                        // Grabbing a node cancels any in-flight collision glide.
                        node.anim_to = None;
                        node.rect = node.rect.offset(delta);
                        dirty = true;
                    }
                }
                DragState::Marquee { .. } => {
                    // The band is drawn from `origin` + the live cursor; just
                    // keep redrawing as it grows.
                    dirty = true;
                }
                DragState::Resize(id) => {
                    // Resize from the corner, following the world-space cursor.
                    // File nodes resize freely (the image letterboxes); directory
                    // nodes resize vertically only — their width stays fit to the
                    // content so filenames never clip — and never taller than
                    // their content (no empty space below the last row).
                    if let Some(node) = arena.get_mut(id) {
                        let world_y = cursor.y / self.zoom + self.camera.y;
                        node.anim_to = None;
                        node.user_sized = true;
                        if node.file.is_some() {
                            let world_x = cursor.x / self.zoom + self.camera.x;
                            let new_w = (world_x - node.rect.min.x).max(PREVIEW_MIN_W);
                            let new_h = (world_y - node.rect.min.y).max(HEADER_H + 30.0);
                            node.rect.max = Point::new(node.rect.min.x + new_w, node.rect.min.y + new_h);
                        } else {
                            let max_h = node.content_h.max(HEADER_H + 30.0);
                            let new_h =
                                (world_y - node.rect.min.y).clamp(HEADER_H + 30.0, max_h);
                            node.rect.max.y = node.rect.min.y + new_h;
                        }
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
                    // A plain click on empty canvas drops the multi-selection.
                    self.last_click = None;
                    if !self.selected_nodes.is_empty() {
                        self.selected_nodes.clear();
                        dirty = true;
                    }
                }
            }
            // A group move leaves the nodes where dropped (snapping each against
            // the others would just make them fight); only a lone node snaps.
            let group_move = matches!(self.drag, DragState::Node(id)
                if self.selected_nodes.len() > 1 && self.selected_nodes.contains(&id));
            if let DragState::Node(id) | DragState::Resize(id) = self.drag {
              if !group_move {
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
            }
            if let DragState::Marquee { origin } = self.drag {
                // Select every node whose box overlaps the band. Corners are in
                // screen px; convert to world (world = screen/zoom + camera).
                let to_world = |p: Point| p.scale(1.0 / self.zoom).add(self.camera);
                let (a, b) = (to_world(origin), to_world(cursor));
                let band = Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                };
                self.selected_nodes.clear();
                for (nid, node) in arena.iter() {
                    if band.intersects(node.rect) {
                        self.selected_nodes.insert(nid);
                    }
                }
                dirty = true;
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
/// Hover highlight: the amber accent (reused from the old persistent selection
/// colour, which read nicely) rather than a plain white tint.
const HOVER_ROW_BG: Rgba = Rgba([255, 191, 64, 66]); // amber ~0.26
const HOVER_BUTTON_BG: Rgba = Rgba([255, 255, 255, 38]); // white 0.15
/// Zebra striping on alternate rows — fainter than the hover highlight so it
/// never competes with it.
const ALT_ROW_BG: Rgba = Rgba([255, 255, 255, 10]); // white ~0.04
/// Header bar: a faint tint over the box fill, a divider line under it, and
/// the gold used for a starred (favorited) node.
const HEADER_BG: Rgba = Rgba([255, 255, 255, 15]); // ~0.06 tint
const HEADER_DIV: Rgba = Rgba([255, 255, 255, 40]); // ~0.16 divider
const COLOR_FAVORITE: Rgba = Rgba([255, 205, 70, 255]);
const HEADER_ICON: f32 = 15.0;
const HEADER_BTN: f32 = 16.0;
const HEADER_PAD: f32 = 7.0;
const HEADER_BTN_GAP: f32 = 4.0;
/// Border for nodes on the chain from root to the current selection.
const COLOR_PATH_BORDER: Rgba = Rgba([255, 191, 64, 255]);
/// Multi-selection accent: a cyan distinct from the amber route colour, used
/// for the dashed border on marquee-selected nodes and the rubber-band itself.
const COLOR_MULTISELECT: Rgba = Rgba([90, 200, 255, 255]);
const COLOR_MARQUEE_FILL: Rgba = Rgba([90, 200, 255, 36]);

/// Two clicks on the same target within this window count as a double click.
const DOUBLE_CLICK_MS: u128 = 400;

/// Smallest width (logical px) an image-preview node can be resized to.
const PREVIEW_MIN_W: f32 = 100.0;
/// Drawn size (logical px) of the corner resize handle glyph.
const PREVIEW_HANDLE: f32 = 16.0;
/// The resize grab zone is larger than the drawn glyph so the corner is easy to
/// hit: it reaches this far in from the corner and a little past it outward.
const RESIZE_GRAB_IN: f32 = 28.0;
const RESIZE_GRAB_OUT: f32 = 7.0;

fn inflate(r: Rect, by: f32) -> Rect {
    Rect { min: Point::new(r.min.x - by, r.min.y - by), max: Point::new(r.max.x + by, r.max.y + by) }
}

/// Stroke a dashed rectangle border (screen px), walking each edge in
/// fixed-length dashes. Used for the marquee multi-selection indicator.
fn draw_dashed_rect(list: &mut DrawList, r: Rect, color: Rgba, width: f32) {
    const DASH: f32 = 6.0;
    const GAP: f32 = 4.0;
    let mut edge = |a: Point, b: Point| {
        let span = b.sub(a);
        let len = span.length();
        if len < 0.5 {
            return;
        }
        let dir = span.scale(1.0 / len);
        let mut t = 0.0;
        while t < len {
            let e = (t + DASH).min(len);
            list.line(a.add(dir.scale(t)), a.add(dir.scale(e)), width, color);
            t += DASH + GAP;
        }
    };
    let (tl, tr) = (Point::new(r.min.x, r.min.y), Point::new(r.max.x, r.min.y));
    let (br, bl) = (Point::new(r.max.x, r.max.y), Point::new(r.min.x, r.max.y));
    edge(tl, tr);
    edge(tr, br);
    edge(br, bl);
    edge(bl, tl);
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
    let path = path_nodes(arena, ui.active_node);
    // The current canvas root's path is the URL bar's default when nothing is
    // selected (there is always a node open).
    let root_path = arena.get(root).map(|n| n.path.clone());
    // Background grid first, then connectors, so every node box paints over
    // both — grid and lines pass *under* the nodes.
    draw_grid(canvas, view);
    draw_connectors(arena, root, canvas, view, cap, &path);
    draw_entries(ui, arena, root, ts, canvas, view, spin_angle, &path, &mut out);
    // The rubber-band draws on top of the nodes, in screen px (like the grid).
    if let DragState::Marquee { origin } = ui.drag {
        let cur = ui.last_pointer;
        let band = Rect {
            min: Point::new(origin.x.min(cur.x), origin.y.min(cur.y)),
            max: Point::new(origin.x.max(cur.x), origin.y.max(cur.y)),
        };
        canvas.rect(band, COLOR_MARQUEE_FILL, 0.0);
        canvas.rect_stroke(band, COLOR_MULTISELECT, 0.0, 1.0);
    }
    draw_navigation(ui, ts, overlay, window, caret_visible, root_path.as_deref());
    // The context menu draws last of all, on top of the toolbar.
    draw_context_menu(ui, ts, overlay, window);
    out
}

// Context-menu look: a frosted translucent panel with a soft shadow.
const MENU_BG: Rgba = Rgba([24, 24, 28, 205]); // ~0.80 dark
const MENU_BORDER: Rgba = Rgba([255, 255, 255, 40]);
const MENU_HOVER: Rgba = Rgba([255, 191, 64, 90]); // amber, like row hover
const MENU_RADIUS: f32 = 10.0;
const MENU_PAD_Y: f32 = 6.0; // vertical padding at top/bottom of the list
const MENU_ITEM_PAD_X: f32 = 14.0;
const MENU_ITEM_PAD_Y: f32 = 6.0; // padding above/below each label

/// Draw the open right-click context menu (frosted backdrop, soft shadow,
/// rounded corners) into the overlay, and push its item hitboxes. Records the
/// menu rect for next frame's click-away test.
fn draw_context_menu(ui: &mut Ui, ts: &mut TextSystem, list: &mut DrawList, window: Point) {
    let Some(menu) = &ui.context_menu else {
        ui.menu_rect = Rect::ZERO;
        return;
    };
    let lh = ts.line_height();
    let item_h = lh + 2.0 * MENU_ITEM_PAD_Y;
    let mut text_w = 0.0f32;
    for (_, label) in MENU_ITEMS {
        text_w = text_w.max(ts.measure(label).x);
    }
    let w = text_w + 2.0 * MENU_ITEM_PAD_X;
    let h = item_h * MENU_ITEMS.len() as f32 + 2.0 * MENU_PAD_Y;
    // Anchor at the cursor, clamped on-screen (and below the toolbar).
    let x = menu.pos.x.min(window.x - w - 4.0).max(4.0);
    let y = menu.pos.y.min(window.y - h - 4.0).max(TOOLBAR_H + 4.0);
    let rect = Rect::from_xywh(x, y, w, h);
    ui.menu_rect = rect;

    // Soft drop shadow: concentric faint rounded rects, offset down.
    const SHADOW_DY: f32 = 4.0;
    for i in (1..=5).rev() {
        let s = i as f32 * 2.0;
        let sr = Rect {
            min: Point::new(rect.min.x - s, rect.min.y - s + SHADOW_DY),
            max: Point::new(rect.max.x + s, rect.max.y + s + SHADOW_DY),
        };
        list.rect(sr, Rgba::new(0.0, 0.0, 0.0, 0.10), MENU_RADIUS + s);
    }
    // Blurred backdrop, inset by the corner radius so its rectangular quad stays
    // within the rounded shape (no sharp corners poking out).
    let inset = inflate(rect, -MENU_RADIUS);
    if inset.max.x > inset.min.x && inset.max.y > inset.min.y {
        let uv = [
            inset.min.x / window.x,
            inset.min.y / window.y,
            inset.max.x / window.x,
            inset.max.y / window.y,
        ];
        list.image_slot_uv(inset, TexSlot::Blur, uv);
    }
    // Frosted fill + border.
    list.rect(rect, MENU_BG, MENU_RADIUS);
    list.rect_stroke(rect, MENU_BORDER, MENU_RADIUS, 1.0);

    // A full-menu hitbox swallows clicks on padding (pushed before the items so
    // they still win the reverse hit-test).
    ui.hitboxes.push(Hitbox { area: rect, action: Action::None, drag: DragKind::None });
    for (i, (item, label)) in MENU_ITEMS.iter().enumerate() {
        let iy = y + MENU_PAD_Y + i as f32 * item_h;
        let irow = Rect::from_xywh(x + 4.0, iy, w - 8.0, item_h);
        let action = Action::Menu(*item);
        if ui.hover == Some(action) {
            list.rect(irow, MENU_HOVER, 6.0);
        }
        ts.draw(
            list,
            Point::new(x + MENU_ITEM_PAD_X, iy + MENU_ITEM_PAD_Y),
            label,
            Rgba::WHITE,
        );
        ui.hitboxes.push(Hitbox { area: irow, action, drag: DragKind::None });
    }
}

/// The route highlighted amber (borders + connector wires): the active node
/// and every ancestor up to the root. The URL bar shows the active node's path,
/// so the route is exactly the chain that path names.
fn path_nodes(arena: &NodeArena, active: Option<NodeId>) -> HashSet<NodeId> {
    let mut set = HashSet::new();
    let mut cur = active;
    while let Some(id) = cur {
        set.insert(id);
        cur = arena.get(id).and_then(|n| n.parent.map(|(p, _)| p));
    }
    set
}

/// Draw a node's header bar (the title strip at the top of its box): a subtle
/// tint and divider, the type icon on the left, and favorite/close buttons on
/// the right. Button hitboxes are pushed after the node-body one so they win
/// the reverse hit-test. All coordinates are world; mapped via `view`.
#[allow(clippy::too_many_arguments)]
fn draw_header(
    ui: &mut Ui,
    ts: &TextSystem,
    list: &mut DrawList,
    view: View,
    id: NodeId,
    node_rect: Rect,
    type_icon: Icon,
    primary: Option<(Icon, Action)>,
    favorited: bool,
) {
    let hdr = Rect { min: node_rect.min, max: Point::new(node_rect.max.x, node_rect.min.y + HEADER_H) };
    list.rect(view.w2s_rect(hdr), HEADER_BG, 5.0);
    let dy = node_rect.min.y + HEADER_H;
    let a = view.w2s(Point::new(node_rect.min.x, dy));
    let b = view.w2s(Point::new(node_rect.max.x, dy));
    list.line(a, b, 1.0, HEADER_DIV);
    // Type icon, left, vertically centered.
    let iy = node_rect.min.y + (HEADER_H - HEADER_ICON) * 0.5;
    let ir = view.w2s_rect(Rect::from_xywh(node_rect.min.x + HEADER_PAD, iy, HEADER_ICON, HEADER_ICON));
    list.glyph_quad(ir, ts.icon_uv(type_icon), Rgba::new(1.0, 1.0, 1.0, 0.85), 0.0);
    // Action buttons, right to left: the primary button (close/pin), then star.
    let by = node_rect.min.y + (HEADER_H - HEADER_BTN) * 0.5;
    let mut bx = node_rect.max.x - HEADER_PAD - HEADER_BTN;
    if let Some((icon, action)) = primary {
        let r = view.w2s_rect(Rect::from_xywh(bx, by, HEADER_BTN, HEADER_BTN));
        if ui.hover == Some(action) {
            list.rect(inflate(r, 2.0), HOVER_BUTTON_BG, 4.0);
        }
        list.glyph_quad(r, ts.icon_uv(icon), Rgba::WHITE, 0.0);
        ui.hitboxes.push(Hitbox { area: r, action, drag: DragKind::Node(id) });
        bx -= HEADER_BTN + HEADER_BTN_GAP;
    }
    let fr = view.w2s_rect(Rect::from_xywh(bx, by, HEADER_BTN, HEADER_BTN));
    let faction = Action::ToggleFavorite { node: id };
    if ui.hover == Some(faction) {
        list.rect(inflate(fr, 2.0), HOVER_BUTTON_BG, 4.0);
    }
    let star = if favorited { COLOR_FAVORITE } else { Rgba::new(1.0, 1.0, 1.0, 0.4) };
    list.glyph_quad(fr, ts.icon_uv(Icon::Star), star, 0.0);
    ui.hitboxes.push(Hitbox { area: fr, action: faction, drag: DragKind::Node(id) });
}

/// Faint background grid, in world space so it pans and zooms with the
/// canvas. Spacing steps by powers of two as you zoom so the on-screen density
/// stays roughly constant; every 4th line is a touch brighter.
fn draw_grid(list: &mut DrawList, view: View) {
    const BASE: f32 = 64.0; // world px between minor lines at 1:1
    const MIN_SCREEN: f32 = 22.0; // keep on-screen spacing at least this
    const MAJOR_EVERY: f32 = 4.0;
    let minor = Rgba::new(1.0, 1.0, 1.0, 0.035);
    let major = Rgba::new(1.0, 1.0, 1.0, 0.07);
    let mut spacing = BASE;
    while spacing * view.zoom < MIN_SCREEN {
        spacing *= 2.0;
    }
    let vis = view.visible();
    let win = view.window;
    let color_for = |n: f32| if (n % MAJOR_EVERY).abs() < 0.5 { major } else { minor };
    // Vertical lines span the full height; horizontal span the full width.
    let mut x = (vis.min.x / spacing).floor() * spacing;
    while x <= vis.max.x {
        let sx = (x - view.cam.x) * view.zoom;
        list.line(Point::new(sx, 0.0), Point::new(sx, win.y), 1.0, color_for((x / spacing).round()));
        x += spacing;
    }
    let mut y = (vis.min.y / spacing).floor() * spacing;
    while y <= vis.max.y {
        let sy = (y - view.cam.y) * view.zoom;
        list.line(Point::new(0.0, sy), Point::new(win.x, sy), 1.0, color_for((y / spacing).round()));
        y += spacing;
    }
}

/// A smooth cubic-bezier "wire" from `a` (parent edge) to `b` (child edge),
/// leaving and arriving horizontally. Sampled in world space and emitted as
/// short segments via `view`.
fn draw_curve(list: &mut DrawList, view: View, a: Point, b: Point, width: f32, color: Rgba) {
    const SEGMENTS: usize = 24;
    // Horizontal control-point reach: half the span, with a floor so short
    // hops still bow out a little.
    let cx = ((b.x - a.x).abs() * 0.5).max(30.0);
    let p1 = Point::new(a.x + cx, a.y);
    let p2 = Point::new(b.x - cx, b.y);
    let mut prev = view.w2s(a);
    for i in 1..=SEGMENTS {
        let t = i as f32 / SEGMENTS as f32;
        let u = 1.0 - t;
        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let p = Point::new(
            w0 * a.x + w1 * p1.x + w2 * p2.x + w3 * b.x,
            w0 * a.y + w1 * p1.y + w2 * p2.y + w3 * b.y,
        );
        let ps = view.w2s(p);
        list.line(prev, ps, width, color);
        prev = ps;
    }
}

/// Fit a directory box each frame: width always tracks the content; height fits
/// the content unless the user has resized it, in which case the user's height
/// is kept (capped to the content so no empty space shows below the last row).
/// Also re-clamps the scroll offset to the resulting box.
fn clamp_dir_box(node: &mut Node, cap: Point) {
    let box_w = node.content_w.min(cap.x);
    let box_h = if node.user_sized {
        node.rect.height().clamp(HEADER_H + 30.0, node.content_h.max(HEADER_H + 30.0))
    } else {
        node.content_h.min(cap.y)
    };
    node.rect.max = Point::new(node.rect.min.x + box_w, node.rect.min.y + box_h);
    node.scroll = node.scroll.clamp(0.0, (node.content_h - box_h).max(0.0));
}

/// Re-clamp every node and emit the parent→child connector wires. Kept
/// separate from `draw_entries` so all wires land in the draw list before any
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
        // Directory boxes fit their content; a user-resized box keeps its height
        // but still fits its width. File nodes are entirely user-sized.
        if node.file.is_none() {
            clamp_dir_box(node, cap);
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
                // Highlight the wire amber when it is an edge of the active
                // route (its child node is on the selection path).
                let color =
                    if path.contains(&child_id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
                let width = if path.contains(&child_id) { 4.0 } else { 3.0 };
                draw_curve(list, view, a, b, width, color);
            }
        }
        draw_connectors(arena, child_id, list, view, cap, path);
    }
}

/// Number of text lines in a file node's info panel, and its total world
/// height. Kept in step with `draw_file_info` and the node-creation sizing.
const FILE_INFO_LINES: f32 = 5.0;
const FILE_INFO_PAD: f32 = 6.0;
pub fn file_info_height(ts: &TextSystem) -> f32 {
    FILE_INFO_LINES * ts.line_height() + 2.0 * FILE_INFO_PAD
}

/// Draw a file node's metadata panel inside `body` (world coords): name, then
/// size + permissions, owner:group, and the modified / created dates.
fn draw_file_info(
    ts: &mut TextSystem,
    list: &mut DrawList,
    view: View,
    body: Rect,
    name: &str,
    fv: &FileView,
) {
    let z = view.zoom;
    let lh = ts.line_height();
    let clip = view.w2s_rect(body);
    let x = body.min.x;
    let dim = Rgba::new(1.0, 1.0, 1.0, 0.7);
    let lines = [
        (name.to_string(), Rgba::WHITE),
        (format!("{}   {}", fmt_size(fv.meta.size), fmt_mode(fv.meta.mode)), dim),
        (format!("{}:{}", fv.owner, fv.group), dim),
        (format!("modified {}", fv.modified), dim),
        (format!("created {}", fv.created), dim),
    ];
    let mut y = body.min.y + FILE_INFO_PAD;
    for (s, color) in &lines {
        ts.draw_clipped(list, view.w2s(Point::new(x, y)), s, *color, clip, z);
        y += lh;
    }
}

/// Fit a rect of the given aspect (w/h) centred inside `region`, letterboxed.
fn fit_rect(region: Rect, aspect: f32) -> Rect {
    let (rw, rh) = (region.width(), region.height());
    if rw <= 0.0 || rh <= 0.0 {
        return region;
    }
    let (w, h) = if rw / rh > aspect { (rh * aspect, rh) } else { (rw, rw / aspect) };
    let cx = region.min.x + rw * 0.5;
    let cy = region.min.y + rh * 0.5;
    Rect { min: Point::new(cx - w * 0.5, cy - h * 0.5), max: Point::new(cx + w * 0.5, cy + h * 0.5) }
}

/// Draw a node's bottom-right corner resize handle (two short edge lines) and
/// push its press hitbox. The hit area is larger than the drawn glyph so the
/// corner is easy to grab. Applies to every node — directory or file.
fn draw_resize_handle(ui: &mut Ui, list: &mut DrawList, view: View, node_rect: Rect, id: NodeId) {
    let hr = view.w2s_rect(Rect::from_xywh(
        node_rect.max.x - PREVIEW_HANDLE,
        node_rect.max.y - PREVIEW_HANDLE,
        PREVIEW_HANDLE,
        PREVIEW_HANDLE,
    ));
    let raction = Action::ResizeNode { node: id };
    let hc = if ui.hover == Some(raction) { Rgba::WHITE } else { Rgba::new(1.0, 1.0, 1.0, 0.6) };
    list.line(Point::new(hr.min.x, hr.max.y), Point::new(hr.max.x, hr.max.y), 2.0, hc);
    list.line(Point::new(hr.max.x, hr.min.y), Point::new(hr.max.x, hr.max.y), 2.0, hc);
    // Grab zone: reaches further in from the corner and a little past it.
    let grab = view.w2s_rect(Rect {
        min: Point::new(node_rect.max.x - RESIZE_GRAB_IN, node_rect.max.y - RESIZE_GRAB_IN),
        max: Point::new(node_rect.max.x + RESIZE_GRAB_OUT, node_rect.max.y + RESIZE_GRAB_OUT),
    });
    ui.hitboxes.push(Hitbox { area: grab, action: raction, drag: DragKind::Resize(id) });
}

/// Human-readable byte size, e.g. `1.2 MiB` (exact `B` under 1 KiB).
fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Unix permission bits as an `rwxr-xr-x` string.
fn fmt_mode(mode: u32) -> String {
    let tri = |b: u32| {
        format!(
            "{}{}{}",
            if b & 0b100 != 0 { "r" } else { "-" },
            if b & 0b010 != 0 { "w" } else { "-" },
            if b & 0b001 != 0 { "x" } else { "-" },
        )
    };
    format!("{}{}{}", tri((mode >> 6) & 7), tri((mode >> 3) & 7), tri(mode & 7))
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
    // 90% rule without a relayout. File nodes and user-resized boxes skip the
    // content-fit clamp (they keep whatever size the user gave them).
    let (node_rect, scroll, content_h, is_root, file_view, favorited, pinned, fname) = {
        let Some(node) = arena.get_mut(id) else { return };
        let file_view = node.file.clone();
        if file_view.is_none() {
            clamp_dir_box(node, cap);
        }
        let favorited = ui.favorites.contains(&node.path);
        let pinned = node.pinned;
        let fname =
            node.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (node.rect, node.scroll, node.content_h, node.parent.is_none(), file_view, favorited, pinned, fname)
    };

    // File node: header, the decoded image (if any) letterboxed above a metadata
    // panel that always sits at the bottom, plus a resize handle. No rows.
    if let Some(fv) = file_view {
        if visible.intersects(node_rect) {
            let screen = view.w2s_rect(node_rect);
            list.rect(screen, COLOR_BOX_FILL, 5.0);
            let info_h = file_info_height(ts);
            let info_top = (node_rect.max.y - info_h).max(node_rect.min.y + HEADER_H);
            if let Some(img) = fv.image {
                // Image fills the region between the header and the info panel,
                // preserving aspect (letterboxed).
                let region = Rect {
                    min: Point::new(node_rect.min.x + 2.0, node_rect.min.y + HEADER_H),
                    max: Point::new(node_rect.max.x - 2.0, info_top),
                };
                let aspect = (img.img_w.max(1) as f32) / (img.img_h.max(1) as f32);
                list.image_tex(view.w2s_rect(fit_rect(region, aspect)), img.tex);
            }
            let info = Rect {
                min: Point::new(node_rect.min.x + 6.0, info_top),
                max: Point::new(node_rect.max.x - 2.0, node_rect.max.y - 2.0),
            };
            draw_file_info(ts, list, view, info, &fname, &fv);
            let border = if path.contains(&id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
            list.rect_stroke(screen, border, 5.0, 3.0);
            if ui.selected_nodes.contains(&id) {
                draw_dashed_rect(list, inflate(screen, 4.0), COLOR_MULTISELECT, 2.0);
            }
            ui.hitboxes.push(Hitbox {
                area: screen,
                action: Action::NodeBody { node: id },
                drag: DragKind::Node(id),
            });
            // Unpinned nodes show a pin button (transient); pinned ones a close
            // button.
            let primary = if pinned {
                (Icon::Close, Action::CloseNode { node: id })
            } else {
                (Icon::Pin, Action::PinNode { node: id })
            };
            draw_header(ui, ts, list, view, id, node_rect, Icon::File, Some(primary), favorited);
            draw_resize_handle(ui, list, view, node_rect, id);
        }
        return;
    }

    if visible.intersects(node_rect) {
        let screen_rect = view.w2s_rect(node_rect);
        list.rect(screen_rect, COLOR_BOX_FILL, 5.0);
        let border = if path.contains(&id) { COLOR_PATH_BORDER } else { Rgba::WHITE };
        list.rect_stroke(screen_rect, border, 5.0, 3.0);
        if ui.selected_nodes.contains(&id) {
            draw_dashed_rect(list, inflate(screen_rect, 4.0), COLOR_MULTISELECT, 2.0);
        }
        ui.hitboxes.push(Hitbox { area: screen_rect, action: Action::NodeBody { node: id }, drag: DragKind::Node(id) });
        // Non-root directory nodes are pinnable like files: pin to keep, close
        // when pinned. The root is permanent and shows no button.
        let primary = (!is_root).then(|| {
            if pinned {
                (Icon::Close, Action::CloseNode { node: id })
            } else {
                (Icon::Pin, Action::PinNode { node: id })
            }
        });
        draw_header(ui, ts, list, view, id, node_rect, Icon::Folder, primary, favorited);
    }

    // Rows draw shifted by the scroll offset and clip to below the header.
    // Everything below is computed in world coords and mapped via `view`.
    let content_clip = Rect {
        min: Point::new(node_rect.min.x, node_rect.min.y + HEADER_H),
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
        let child = item.child;
        let is_dir = item.is_dir;
        let scanning = item.scanning;
        let display = item.display.clone();
        // Side attachments (open button, spinner) hang off the box edge in
        // world x when the row text is wider than the capped box.
        let side_x = rect.max.x.min(node_rect.max.x).max(node_rect.min.x);

        if in_view {
            let row_action = Action::Row { node: id, item: i };
            // Row backgrounds, painted bottom-up: zebra, then hover feedback.
            // There is no persistent per-row selection anymore — "where you are"
            // is shown by the active node's amber route, not a highlighted row.
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
            let color = Rgba::WHITE;
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

    // Scrollbar indicator on the right edge (in the rows area, below the
    // header) while content overflows.
    if scrolled && visible.intersects(node_rect) {
        let rows_view = (node_rect.height() - HEADER_H).max(1.0);
        let rows_total = (content_h - HEADER_H).max(1.0);
        let track_h = rows_view - 8.0;
        let thumb_h = (track_h * rows_view / rows_total).max(20.0);
        let max_scroll = content_h - node_rect.height();
        let t = if max_scroll > 0.0 { scroll / max_scroll } else { 0.0 };
        let thumb_y = node_rect.min.y + HEADER_H + 4.0 + t * (track_h - thumb_h);
        let thumb = Rect::from_xywh(node_rect.max.x - 6.0, thumb_y, 3.0, thumb_h);
        list.rect(view.w2s_rect(thumb), Rgba::new(1.0, 1.0, 1.0, 0.35), 1.5);
    }

    // Directory resize handle, drawn last so it wins the bottom-right corner
    // over any row beneath it.
    if visible.intersects(node_rect) {
        draw_resize_handle(ui, list, view, node_rect, id);
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
    // Composite the offscreen canvas, then the blurred band under the bar. The
    // blur now covers the whole scene, so sample just its top strip here.
    list.image_slot(Rect::from_xywh(0.0, 0.0, window.x, window.y), TexSlot::Scene);
    let band = Rect::from_xywh(0.0, 0.0, window.x, TOOLBAR_H);
    list.image_slot_uv(band, TexSlot::Blur, [0.0, 0.0, 1.0, (TOOLBAR_H / window.y).clamp(0.0, 1.0)]);
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
        (Icon::Home, Action::FocusHome),
        (Icon::Selection, Action::FocusSelection),
        (Icon::Parent, Action::GoUp),
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
