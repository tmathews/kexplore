# kexplore — TODO / session handoff

Canvas-based Wayland file explorer. Rust + Vulkan (ash), rewritten from the
original C/Cairo app. Working tree is clean; everything below is committed on
`main`.

## Where things live

- `src/main.rs` — app struct, main loop, input dispatch, `handle_action`,
  clipboard/spawn helpers, preview texture lifecycle.
- `src/ui.rs` — immediate-mode frame build (`build_frame`, `draw_entries`,
  `draw_connectors`, `draw_navigation`), hit-testing, `Action` enum, camera,
  drag/hover/double-click state, colors & layout constants.
- `src/model.rs` — generational-arena node tree, `scan_dir`, `calc_size`
  (layout + 90% cap), `NodeId`.
- `src/gfx/` — `mod.rs` (Vulkan device + render loop with Upload/Scene/Overlay
  phases), `renderer2d.rs` (batcher, `DrawList`, `TexSlot`, `TexSets`),
  `blur.rs` (toolbar backdrop), `swapchain.rs`, `upload.rs`.
- `src/text.rs` — fontdue glyph atlas, `draw_clipped`, `caret_x`/`caret_index`.
- `src/textfield.rs` — URL bar edit state machine.
- `src/platform/wayland.rs` — hand-rolled Wayland client, pointer + keyboard
  (xkbcommon), scroll, fractional scale.
- `src/tasks.rs` / `src/preview.rs` — worker threads (dir scan, image decode).
- `src/handlers.rs` — open-with config + process spawning.

## Done

Rewrite (commits fbfa972, 2163d2b): full C→Rust/Vulkan port; C sources
deleted, crate promoted to repo root. ash + raw wayland-client + fontdue +
`image` crate. Fixes over the C app: scan/preview races, zombie children,
missing busy spinner, space-unsafe spawns. Font/icons/shaders embedded, no
install step. HiDPI incl. fractional scaling (a hard requirement — the C app
ignoring scale was a pain point).

UX items (newest first):
- **Lines under nodes, URL-path border, double-click** (85051e3) — connector
  lines render beneath all boxes (`draw_connectors` pre-pass); nodes on the
  URL route get an amber border; **directories open on double-click, single
  click only selects** (files still preview on single click).
- **Full-width row hitboxes + hover feedback** (4e91523) — rows clickable
  across the node's inner width; hover highlight on rows and icon buttons.
- **Calm auto-focus** (ad28c04) — opening a node only pans if it spawned out
  of view, and just enough. Startup/re-root/focus-buttons still fully center.
- **Node child spawn fix** (5bfb176) — child of a row in a scrolled list
  spawned far below; now subtracts parent scroll.
- **90% cap + node scrolling** (e414e9c) — node box never exceeds 90% of the
  area below the toolbar; overflow scrolls inside via mouse wheel + scrollbar.
- **Editable URL bar** (1bb351f) — click to edit, Enter re-roots at typed dir
  (`~` expands), Esc/click-away cancels; caret/arrows/word-jumps/selection/
  Home-End/Ctrl+A/C/X/V, clip + horizontal scroll. Added keyboard input
  (xkbcommon) with repeat.
- **Frosted-glass toolbar** (8b05f87) — bar floats over canvas with a real
  backdrop blur of the band behind it; swallows clicks; focus targets the
  area below it.

## Remaining (todo)

Priority note: **UX-5 (collision) is the natural next item** — double-click-
to-open plus no collision makes it easy to stack nodes on top of each other.

- **UX-5 — node collision**: nodes never overlap. Opening a node pushes others
  out of the way to find free space; dragging a node over another
  blocks/resolves to an acceptable non-overlapping position. (Design: pick a
  push direction / packing rule; decide whether push is animated.)
- **UX-9 — row file-type icons**: simple directory vs file icon at the start
  of each row. Start with two icons; extend to more types later. (Icons are
  pre-rasterized PNGs packed into the glyph atlas — see `src/text.rs` Icon
  enum + `assets/icons/`, regen via rsvg-convert; rows shift right to make
  room, watch `calc_size` width and row text origin.)
- **UX-11 — alternating row backgrounds**: rows alternate none/coloured/none
  with a faint tint. (Draw in `draw_entries`; keep subtle enough not to fight
  the hover highlight `HOVER_ROW_BG`.)
- **UX-10 — previews as resizable nodes**: multiple previews attached to known
  file types; previews behave like nodes on the canvas and are resizable.
  (Biggest item — currently a single preview draws bottom-right. Likely needs
  a preview to become a node-like entity with its own rect/drag/resize, its
  own TexSlot/descriptor, and per-type handling.)
- **Wheel-over-canvas behavior** — currently wheel only scrolls a hovered
  node; wheel over empty canvas does nothing. **Needs user spec** (zoom?
  canvas pan?). Original C TODOs mentioned zoom + middle-mouse pan.

## Open questions for the user

- UX-10 scope: what file types get previews, and what does "resizable" mean
  exactly (free resize, or snap to aspect)?
- Wheel-over-canvas: zoom vs pan?

## Testing (GUI)

The user's compositor (`kwm`) has no screencopy, so verify by nesting `niri`:
- `niri -c <scratchpad>/niri-config/config.kdl` → serves `WAYLAND_DISPLAY=wayland-1`.
- Screenshot: `WAYLAND_DISPLAY=wayland-1 grim out.png`.
- Inject input with the scratchpad scripts: `wlclick.py` (pointer, incl.
  `wheel:N` and `down up` sequences for double-click) and `wlkbd.py`
  (`text:...`, `key:NAME[+ctrl][+shift]`) over `zwlr_virtual_pointer` /
  `zwp_virtual_keyboard`. **Always kill the nested niri + app when done** so
  the user can test.
- `KEXPLORE_SCAN_DELAY_MS=<ms>` slows scans (spinner / close-during-scan).
- Build/run: `cargo build [--release]`; toolchain is in `~/.cargo` (rustup).

Convention this project follows: after each item, `cargo build --release`,
verify in niri with a screenshot, then commit with a
`Co-Authored-By: Claude` trailer.
