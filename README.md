# kexplore (Rust + Vulkan)

Rewrite of the C/Cairo kexplore: a graph-based file explorer for Wayland.
Directories open as draggable node boxes on an infinite pannable canvas,
connected by lines. Same look and behavior as the C app, with the known bugs
fixed and HiDPI support added.

## Building

```sh
cargo build --release
./target/release/kexplore
```

Runtime requirements: a Wayland compositor, a Vulkan 1.3 driver (any recent
Mesa), `libwayland-client`, `libxkbcommon`. Everything else (font, icons,
shaders) is embedded in the binary — no install step or data directory
needed.

## Usage

Mouse only, like the C app:

- **URL bar** — click to edit: type a directory path and press Enter to
  re-root the canvas there (`~` expands). Caret, arrow keys (Ctrl for word
  jumps), Shift-selection, Home/End, Ctrl+A/C/X/V (clipboard via
  `wl-copy`/`wl-paste`); Esc or clicking away cancels back to showing the
  selection. Long paths scroll and clip inside the field.
- **Click a directory row** — selects it (the URL bar shows its path and
  the nodes forming that route get an amber border). **Double-click** opens
  it as a new node (scanned on a worker thread; spinner shows while it
  loads). A node box never exceeds 90% of the viewing area below the
  toolbar; bigger directories scroll inside the box with the mouse wheel
  (scrollbar shows the position). The camera only pans when a new node
  spawns outside the view, and then just enough to bring it in; use the
  toolbar focus buttons for explicit centering. Connector lines between a
  parent row and its child node always pass beneath the node boxes.
- **Click a file row** — select it; PNG/JPEG/WebP get a preview bottom-right;
  an open button appears next to the row for files with a configured handler.
- **Drag a node** — move it. **Drag empty canvas** — pan the camera.
- **Close button** above a node closes its whole subtree.
- Toolbar: focus home / focus selection / focus parent / focus top of node /
  copy path (`wl-copy`) / open terminal (`foot -D <dir>`). The bar floats
  over the canvas with a frosted-glass blur of whatever scrolls beneath it;
  it swallows clicks, and camera focus targets the area below it.

## Handlers

`~/.config/kallos/handlers`, one rule per line, same format as the C app:

```
ext, ext: command with {FILE}
```

e.g. `png, jpg, webp: imv {FILE}` or `mp4, mkv: mpv --loop {FILE}`.
Extension match is case-insensitive; the **last** matching line wins.
The command is split on whitespace, a leading `~` expands to `$HOME`, and
`{FILE}` is replaced with the selected path (spaces in paths are safe — no
shell, no globbing, no `$VAR` expansion).

## HiDPI

The app obeys the compositor's scale, including fractional scaling
(`wp-fractional-scale-v1` + `wp-viewporter`; falls back to integer
`preferred_buffer_scale`, then `wl_output` scale). Layout is in logical
pixels; the swapchain renders at physical pixels and glyphs are rasterized
at the effective scale, so text stays sharp at 1.5x/2x and re-sharpens when
the window moves between differently-scaled outputs.

## Development notes

- **Shaders**: GLSL sources and compiled SPIR-V both live in `shaders/` and
  the `.spv` files are embedded with `include_bytes!`. After editing GLSL,
  rebuild them manually (no build-time dependency on a compiler):

  ```sh
  for s in ui.vert ui.frag blur.vert blur.frag; do
      glslc shaders/$s -o shaders/$s.spv
  done
  ```

- **Icons**: `assets/icons/*.png` are pre-rasterized from the SVG sources in
  `data/` (which remain the source of truth). To regenerate:

  ```sh
  for i in home close selection top parent copy open terminal; do
      rsvg-convert -w 64 -h 64 data/$i.svg > assets/icons/$i.png
  done
  ```

  The busy spinner is generated procedurally at startup (the C app
  referenced a `busy.svg` that never existed).

- **Font**: `assets/NotoSans-Regular.ttf` (OFL) is embedded; text rendering
  is fontdue into a shelf-packed R8 atlas. No shaping/fallback — filenames
  in scripts the font doesn't cover render as tofu.

- **Validation layers** are enabled automatically in debug builds when
  `VK_LAYER_KHRONOS_validation` is installed.

- `KEXPLORE_SCAN_DELAY_MS=<ms>` artificially slows directory scans (debug
  aid for the spinner and close-during-scan paths).

## Differences from the C version

Fixed:
- Data races: fire-and-forget threads mutating shared state → workers now
  exchange plain data over channels; the tree lives only on the main thread
  with generational ids, so stale results (e.g. a scan finishing after its
  node was closed) drop harmlessly.
- Zombie processes: spawned children are reaped.
- Rapid preview clicking showing the wrong image: generation-counter
  cancellation; the shown preview always matches the latest selection.
- Busy spinner now actually renders (per-item, while its scan runs).
- Paths with spaces work in handlers, copy-path, and open-terminal
  (arg-vector spawns instead of string concatenation + wordexp).
- `foot -D` gets the selected file's *directory* (the C app passed the file).
- Decoration mode request actually asks for server-side decorations.
- Idle CPU is ~0% (damage-driven rendering; the C app redrew every frame).
- `DT_UNKNOWN` directory entries are classified via stat fallback.
- Decode bombs: preview inputs capped (64 MB file, 64 MP image, decoded
  image downscaled to ≤2048px before upload).

Cut (relative to C):
- SVG file previews (kept toolbar SVGs as pre-rasterized PNGs; resvg was a
  disproportionate dependency for one preview format).
- Canvas keyboard navigation (h/j/k/l from the C TODO list) — the URL bar
  is keyboard-driven, but node traversal is still mouse-only.
