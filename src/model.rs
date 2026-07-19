//! The file-tree model: a generational arena of directory nodes. The arena
//! lives only on the main thread; worker threads exchange plain data
//! (PathBuf in, Vec<ItemData> out), so the tree needs no locks. Generational
//! ids make stale references (drag targets, selection, in-flight scan
//! results) mechanically safe: `get` returns None after a node is closed,
//! even if the slot is reused.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::geom::{Point, Rect};
use crate::text::TextSystem;
use crate::textfield::TextField;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    index: u32,
    gen: u32,
}

pub struct Node {
    pub path: PathBuf,
    pub items: Vec<Item>,
    /// Owning node and the index of the item we hang off of.
    pub parent: Option<(NodeId, usize)>,
    /// Canvas position of the displayed box (size capped to the viewport
    /// limit); Rect::ZERO means not laid out yet.
    pub rect: Rect,
    /// Full (uncapped) content size from layout.
    pub content_w: f32,
    pub content_h: f32,
    /// Vertical scroll offset when content_h exceeds the box height.
    pub scroll: f32,
    /// When set, the box glides its min corner toward this point (collision
    /// resolve after an open or a drag-release); cleared on arrival.
    pub anim_to: Option<Point>,
    /// Pinned nodes stay open; unpinned ("transient") nodes are peeks that close
    /// once the active route moves off them (files *and* directories). The root
    /// is always pinned. Pinning a node pins its whole ancestor chain.
    pub pinned: bool,
    /// True once the user has manually resized the box, which turns off the
    /// automatic content-fit clamp for directory nodes.
    pub user_sized: bool,
    /// What this node *is* — which decides how it lays out and draws.
    pub body: NodeBody,
}

/// The kind of a node. Directory nodes render `items` as rows and fit their box
/// to that content; file nodes render a metadata panel (plus an image once one
/// is decoded); draft nodes are the unsaved "new file" box — a name being typed,
/// anchored to a directory.
pub enum NodeBody {
    Dir,
    File(FileView),
    Draft(Draft),
}

/// A file that does not exist yet: the name being edited, and the directory it
/// would land in. `dir` re-binds as the active route moves, so it is *not* a
/// `Node::parent` link — no item row backs a draft.
pub struct Draft {
    pub dir: NodeId,
    pub name: TextField,
}

impl NodeBody {
    pub fn is_dir(&self) -> bool {
        matches!(self, NodeBody::Dir)
    }
    pub fn file(&self) -> Option<&FileView> {
        match self {
            NodeBody::File(f) => Some(f),
            _ => None,
        }
    }
    pub fn file_mut(&mut self) -> Option<&mut FileView> {
        match self {
            NodeBody::File(f) => Some(f),
            _ => None,
        }
    }
    pub fn draft(&self) -> Option<&Draft> {
        match self {
            NodeBody::Draft(d) => Some(d),
            _ => None,
        }
    }
    pub fn draft_mut(&mut self) -> Option<&mut Draft> {
        match self {
            NodeBody::Draft(d) => Some(d),
            _ => None,
        }
    }
}

/// A file node's contents: stat metadata, an optional decoded image, and the
/// info-panel strings resolved once at creation (owner/group names, dates).
#[derive(Clone)]
pub struct FileView {
    pub meta: FileMeta,
    /// Present once an image preview has been decoded for a displayable type.
    pub image: Option<ImageTex>,
    pub owner: String,
    pub group: String,
    pub modified: String,
    pub created: String,
}

impl FileView {
    /// Build the presentation for `meta`, resolving owner/group names and
    /// formatting the modified/created timestamps.
    pub fn new(meta: FileMeta, image: Option<ImageTex>) -> FileView {
        FileView {
            image,
            owner: user_name(meta.uid),
            group: group_name(meta.gid),
            modified: fmt_time(meta.mtime),
            created: fmt_time(meta.ctime),
            meta,
        }
    }
}

/// A decoded image bound to a file node.
#[derive(Clone, Copy)]
pub struct ImageTex {
    /// Opaque texture id, indexing the descriptor table owned by main.rs.
    pub tex: u32,
    /// Intrinsic image size in pixels (for aspect-locked resize).
    pub img_w: u32,
    pub img_h: u32,
}

/// File stat metadata shown in a file node's info panel.
#[derive(Clone, Copy)]
pub struct FileMeta {
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// Modification / change time as seconds since the Unix epoch.
    pub mtime: i64,
    pub ctime: i64,
}

impl FileMeta {
    /// Read metadata for `path` (following symlinks). None if the stat fails.
    pub fn read(path: &Path) -> Option<FileMeta> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some(FileMeta {
            size: m.size(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            mtime: m.mtime(),
            ctime: m.ctime(),
        })
    }
}

/// Resolve a uid to its login name (falling back to the numeric id).
fn user_name(uid: u32) -> String {
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = [0 as libc::c_char; 1024];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut out);
        if rc == 0 && !out.is_null() && !pwd.pw_name.is_null() {
            std::ffi::CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned()
        } else {
            uid.to_string()
        }
    }
}

/// Resolve a gid to its group name (falling back to the numeric id).
fn group_name(gid: u32) -> String {
    unsafe {
        let mut grp: libc::group = std::mem::zeroed();
        let mut buf = [0 as libc::c_char; 1024];
        let mut out: *mut libc::group = std::ptr::null_mut();
        let rc = libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut out);
        if rc == 0 && !out.is_null() && !grp.gr_name.is_null() {
            std::ffi::CStr::from_ptr(grp.gr_name).to_string_lossy().into_owned()
        } else {
            gid.to_string()
        }
    }
}

/// Format a Unix timestamp as local `YYYY-MM-DD HH:MM` (empty on failure).
fn fmt_time(epoch: i64) -> String {
    unsafe {
        let t = epoch as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0 as libc::c_char; 64];
        let fmt = c"%Y-%m-%d %H:%M";
        let n = libc::strftime(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), &tm);
        if n == 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

pub struct Item {
    pub name: OsString,
    /// Lossy display form, cached for measuring/drawing. Never turn this
    /// back into a path — use `name`.
    pub display: String,
    pub is_dir: bool,
    /// Row rect relative to the node's min corner.
    pub rect: Rect,
    pub child: Option<NodeId>,
    /// A directory scan is in flight for this item (spinner + click dedup).
    pub scanning: bool,
    /// An image-preview decode is in flight for this item (spinner + dedup).
    pub preview_loading: bool,
}

/// Plain data produced by a directory scan (safe to send from workers).
pub struct ItemData {
    pub name: OsString,
    pub is_dir: bool,
}

struct Slot {
    gen: u32,
    node: Option<Node>,
}

pub struct NodeArena {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

impl NodeArena {
    pub fn new() -> NodeArena {
        NodeArena { slots: Vec::new(), free: Vec::new() }
    }

    pub fn insert(&mut self, node: Node) -> NodeId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.node = Some(node);
            NodeId { index, gen: slot.gen }
        } else {
            self.slots.push(Slot { gen: 0, node: Some(node) });
            NodeId { index: self.slots.len() as u32 - 1, gen: 0 }
        }
    }

    /// Iterate every live node with its id (order is arbitrary).
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            slot.node.as_ref().map(|n| (NodeId { index: i as u32, gen: slot.gen }, n))
        })
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.gen != id.gen {
            return None;
        }
        slot.node.as_ref()
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.gen != id.gen {
            return None;
        }
        slot.node.as_mut()
    }

    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.gen != id.gen {
            return None;
        }
        let node = slot.node.take();
        if node.is_some() {
            slot.gen = slot.gen.wrapping_add(1);
            self.free.push(id.index);
        }
        node
    }

    /// Close a node and its whole subtree, unlinking it from its parent item.
    pub fn close_recursive(&mut self, id: NodeId) {
        let Some(node) = self.get(id) else { return };
        if let Some((pid, pidx)) = node.parent {
            if let Some(parent) = self.get_mut(pid) {
                if let Some(item) = parent.items.get_mut(pidx) {
                    item.child = None;
                }
            }
        }
        self.close_subtree(id);
    }

    fn close_subtree(&mut self, id: NodeId) {
        let Some(node) = self.remove(id) else { return };
        for item in node.items {
            if let Some(child) = item.child {
                self.close_subtree(child);
            }
        }
    }
}

/// Read a directory into sorted plain item data. Skips only `.`/`..`
/// (dotfiles are shown), sorts byte-wise like the C strcmp qsort.
pub fn scan_dir(path: &Path) -> std::io::Result<Vec<ItemData>> {
    let mut items = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        // file_type() uses d_type and falls back to lstat on DT_UNKNOWN
        // (fixing the C app's misclassification on such filesystems);
        // symlinks are not followed, matching d_type == DT_DIR.
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push(ItemData { name: entry.file_name(), is_dir });
    }
    items.sort_unstable_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    Ok(items)
}

pub fn node_from_items(path: PathBuf, data: Vec<ItemData>) -> Node {
    let items = data
        .into_iter()
        .map(|d| Item {
            display: d.name.to_string_lossy().into_owned(),
            name: d.name,
            is_dir: d.is_dir,
            rect: Rect::ZERO,
            child: None,
            scanning: false,
            preview_loading: false,
        })
        .collect();
    Node {
        path,
        items,
        parent: None,
        rect: Rect::ZERO,
        content_w: 0.0,
        content_h: 0.0,
        scroll: 0.0,
        anim_to: None,
        pinned: false,
        user_sized: false,
        body: NodeBody::Dir,
    }
}

/// Build a file node (no rows) sized to `rect`, attached to a file item via
/// `parent`. Holds the file's metadata and an optional decoded image.
pub fn file_node(
    path: PathBuf,
    parent: (NodeId, usize),
    meta: FileMeta,
    image: Option<ImageTex>,
    rect: Rect,
    pinned: bool,
) -> Node {
    Node {
        path,
        items: Vec::new(),
        parent: Some(parent),
        rect,
        content_w: rect.width(),
        content_h: rect.height(),
        scroll: 0.0,
        anim_to: None,
        pinned,
        user_sized: false,
        body: NodeBody::File(FileView::new(meta, image)),
    }
}

/// Build a draft ("new file") node sized to `rect`, anchored to directory
/// `dir`. It has no `parent` link and is born pinned: a draft is never a peek,
/// so `prune_transients` must not collect it. `path` is the directory it
/// currently targets; the filename lives in the `TextField`.
pub fn draft_node(dir: NodeId, path: PathBuf, name: String, rect: Rect) -> Node {
    let mut field = TextField::new();
    let caret = name.len();
    field.begin(name, caret);
    Node {
        path,
        items: Vec::new(),
        parent: None,
        rect,
        content_w: rect.width(),
        content_h: rect.height(),
        scroll: 0.0,
        anim_to: None,
        pinned: true,
        user_sized: false,
        body: NodeBody::Draft(Draft { dir, name: field }),
    }
}

/// Logical size of the file-type icon at the start of each row, and the gap
/// between it and the row text. Used by `calc_size` (width reservation) and
/// `ui::draw_entries` (icon + text placement); keep them in step.
pub const ROW_ICON: f32 = 15.0;
pub const ROW_ICON_GAP: f32 = 5.0;

/// Height of the node header bar (title strip at the top of every box) and the
/// minimum box width needed to fit its type icon plus the action buttons.
pub const HEADER_H: f32 = 24.0;
pub const HEADER_MIN_W: f32 = 82.0;

/// Port of node_calc_size: stack rows vertically with 5px padding, size the
/// box to the widest row, and position it to the right of the parent item.
/// The displayed box is capped to `max_size` (90% of the safe viewing area);
/// overflowing content scrolls inside the box.
pub fn calc_size(arena: &mut NodeArena, id: NodeId, ts: &mut TextSystem, max_size: Point) {
    const PADDING: f32 = 5.0;
    let Some(node) = arena.get(id) else { return };
    let lh = ts.line_height();
    // Room at the start of each row for the file-type icon (see ROW_ICON /
    // ROW_ICON_GAP in ui.rs; kept in sync here so the box is wide enough).
    let icon_advance = ROW_ICON + ROW_ICON_GAP;
    // Rows begin below the header bar (a fixed strip at the top of the box).
    let mut oy = HEADER_H;
    let mut max_w = 0.0f32;
    let mut rects = Vec::with_capacity(node.items.len());
    for item in &node.items {
        let w = icon_advance + ts.measure(&item.display).x;
        rects.push(Rect { min: Point::new(PADDING, oy), max: Point::new(PADDING + w, oy + lh) });
        oy += lh;
        max_w = max_w.max(w);
    }
    let origin = match node.parent {
        Some((pid, pidx)) => {
            let parent = arena.get(pid);
            match parent {
                Some(p) => {
                    // The item's row y is content-relative; subtract the
                    // parent's scroll so the child spawns beside the row as
                    // displayed, not where it would sit unscrolled.
                    let item_y = p.items.get(pidx).map(|it| it.rect.min.y).unwrap_or(0.0);
                    Point::new(p.rect.max.x + 20.0, p.rect.min.y + item_y - p.scroll)
                }
                None => Point::ZERO,
            }
        }
        None => Point::ZERO,
    };
    let Some(node) = arena.get_mut(id) else { return };
    for (item, r) in node.items.iter_mut().zip(rects) {
        item.rect = r;
    }
    node.content_w = (max_w + 2.0 * PADDING).max(HEADER_MIN_W);
    node.content_h = oy + PADDING;
    node.scroll = 0.0;
    let box_w = node.content_w.min(max_size.x);
    let box_h = node.content_h.min(max_size.y);
    node.rect = Rect { min: origin, max: Point::new(origin.x + box_w, origin.y + box_h) };
}

/// Empty space kept between node boxes when collision-resolving.
const NODE_GAP: f32 = 12.0;

/// Strict overlap: touching edges (and the NODE_GAP margin) do not count, so
/// a box pushed exactly NODE_GAP clear of an obstacle reads as separated and
/// the resolver terminates.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
}

/// Slide `cand` straight down until it clears every obstacle. Monotonic in y
/// (each step drops below the lowest obstacle it currently hits), so it
/// always converges — there is always free space below the lowest node.
fn slide_down(cand: Rect, obstacles: &[Rect]) -> Rect {
    let mut r = cand;
    for _ in 0..256 {
        let mut new_top = r.min.y;
        for o in obstacles {
            if overlaps(r, *o) {
                new_top = new_top.max(o.max.y + NODE_GAP);
            }
        }
        if new_top == r.min.y {
            return r;
        }
        r = r.offset(Point::new(0.0, new_top - r.min.y));
    }
    r
}

/// Find a non-overlapping position for `cand` given the other nodes'
/// `obstacles`.
///
/// Freshly opened nodes (`allow_up_left = false`) simply slide down past
/// whatever they land on, so a new box stacks below its neighbors and never
/// jumps above or before its parent.
///
/// Dropped nodes (`allow_up_left = true`) snap to the nearest free spot: we
/// probe the four edge-aligned positions around each obstacle (keeping the
/// unblocked coordinate) and pick the collision-free one closest to where the
/// node was dropped, falling back to a slide-down if the area is too crowded.
pub fn resolve_collision(cand: Rect, obstacles: &[Rect], allow_up_left: bool) -> Rect {
    if obstacles.iter().all(|o| !overlaps(cand, *o)) {
        return cand;
    }
    if !allow_up_left {
        return slide_down(cand, obstacles);
    }
    let (w, h) = (cand.width(), cand.height());
    let mut best: Option<Rect> = None;
    let mut consider = |min: Point| {
        let r = Rect { min, max: Point::new(min.x + w, min.y + h) };
        if obstacles.iter().all(|o| !overlaps(r, *o)) {
            let d = min.sub(cand.min).length();
            if best.map_or(true, |b| d < b.min.sub(cand.min).length()) {
                best = Some(r);
            }
        }
    };
    for o in obstacles {
        consider(Point::new(cand.min.x, o.max.y + NODE_GAP)); // below
        consider(Point::new(cand.min.x, o.min.y - NODE_GAP - h)); // above
        consider(Point::new(o.max.x + NODE_GAP, cand.min.y)); // right
        consider(Point::new(o.min.x - NODE_GAP - w, cand.min.y)); // left
    }
    best.unwrap_or_else(|| slide_down(cand, obstacles))
}

#[cfg(test)]
mod tests {
    use super::{overlaps, resolve_collision, NODE_GAP};
    use crate::geom::{Point, Rect};

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_xywh(x, y, w, h)
    }

    #[test]
    fn no_obstacles_is_identity() {
        let c = r(0.0, 0.0, 100.0, 80.0);
        assert_eq!(resolve_collision(c, &[], false), c);
        assert_eq!(resolve_collision(c, &[], true), c);
    }

    #[test]
    fn open_slides_newcomer_clear_without_going_up_or_left() {
        // Fully overlapping; down/right-only search must not move up or left.
        let obstacle = r(0.0, 0.0, 100.0, 100.0);
        let cand = r(0.0, 0.0, 100.0, 100.0);
        let out = resolve_collision(cand, &[obstacle], false);
        assert!(!overlaps(out, obstacle));
        assert!(out.min.x >= cand.min.x - 0.01);
        assert!(out.min.y >= cand.min.y - 0.01);
    }

    #[test]
    fn drag_may_push_up_or_left_when_that_is_nearest() {
        // Candidate sits near the obstacle's top-left corner, so the shortest
        // escape is up or left — only allowed with allow_up_left.
        let obstacle = r(0.0, 0.0, 100.0, 100.0);
        let cand = r(10.0, 10.0, 20.0, 20.0);
        let out = resolve_collision(cand, &[obstacle], true);
        assert!(!overlaps(out, obstacle));
        assert!(out.min.x < cand.min.x || out.min.y < cand.min.y);
    }

    #[test]
    fn keeps_at_least_the_gap() {
        let obstacle = r(0.0, 0.0, 100.0, 100.0);
        let cand = r(50.0, 50.0, 100.0, 100.0);
        let out = resolve_collision(cand, &[obstacle], false);
        assert!(!overlaps(out, obstacle));
        // Cleared on some axis by at least the gap.
        let clears_down = out.min.y >= obstacle.max.y + NODE_GAP - 0.01;
        let clears_right = out.min.x >= obstacle.max.x + NODE_GAP - 0.01;
        assert!(clears_down || clears_right);
    }

    #[test]
    fn resolves_against_multiple_obstacles() {
        let obstacles = [
            r(0.0, 0.0, 100.0, 100.0),
            r(0.0, 120.0, 100.0, 100.0),
            r(120.0, 0.0, 100.0, 100.0),
        ];
        let cand = r(10.0, 10.0, 90.0, 90.0);
        let out = resolve_collision(cand, &obstacles, true);
        for o in &obstacles {
            assert!(!overlaps(out, *o), "still overlaps {:?}", o);
        }
    }

    #[test]
    fn overlaps_is_strict_at_edges() {
        // Edge-touching boxes are not overlapping (so a gap-cleared box ends
        // the search).
        let a = r(0.0, 0.0, 10.0, 10.0);
        let b = r(10.0, 0.0, 10.0, 10.0);
        assert!(!overlaps(a, b));
        let c = r(5.0, 5.0, 10.0, 10.0);
        assert!(overlaps(a, c));
        let _ = Point::ZERO;
    }
}

/// Advance any node gliding toward its `anim_to` target; returns true while
/// at least one is still moving so the caller keeps the frame loop awake.
/// Uses the same frame-rate-independent lerp as the camera.
pub fn step_nodes(arena: &mut NodeArena, dt: f32) -> bool {
    const RATE: f32 = 10.5;
    let t = 1.0 - (-RATE * dt).exp();
    let mut moving = false;
    for slot in &mut arena.slots {
        let Some(node) = &mut slot.node else { continue };
        let Some(target) = node.anim_to else { continue };
        let next = node.rect.min.lerp(target, t);
        if target.sub(next).length() < 0.5 {
            node.rect = node.rect.offset(target.sub(node.rect.min));
            node.anim_to = None;
        } else {
            node.rect = node.rect.offset(next.sub(node.rect.min));
            moving = true;
        }
    }
    moving
}
