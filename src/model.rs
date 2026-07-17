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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    }
}

/// Port of node_calc_size: stack rows vertically with 5px padding, size the
/// box to the widest row, and position it to the right of the parent item.
/// The displayed box is capped to `max_size` (90% of the safe viewing area);
/// overflowing content scrolls inside the box.
pub fn calc_size(arena: &mut NodeArena, id: NodeId, ts: &mut TextSystem, max_size: Point) {
    const PADDING: f32 = 5.0;
    let Some(node) = arena.get(id) else { return };
    let lh = ts.line_height();
    let mut oy = PADDING;
    let mut max_w = 0.0f32;
    let mut rects = Vec::with_capacity(node.items.len());
    for item in &node.items {
        let w = ts.measure(&item.display).x;
        rects.push(Rect { min: Point::new(PADDING, oy), max: Point::new(PADDING + w, oy + lh) });
        oy += lh;
        max_w = max_w.max(w);
    }
    let origin = match node.parent {
        Some((pid, pidx)) => {
            let parent = arena.get(pid);
            match parent {
                Some(p) => {
                    let item_y = p.items.get(pidx).map(|it| it.rect.min.y).unwrap_or(0.0);
                    Point::new(p.rect.max.x + 20.0, p.rect.min.y + item_y)
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
    node.content_w = max_w + 2.0 * PADDING;
    node.content_h = oy + PADDING;
    node.scroll = 0.0;
    let box_w = node.content_w.min(max_size.x);
    let box_h = node.content_h.min(max_size.y);
    node.rect = Rect { min: origin, max: Point::new(origin.x + box_w, origin.y + box_h) };
}
