//! Text + icon atlas. fontdue rasterizes glyphs at 18 logical px times the
//! current output scale into a shelf-packed R8 atlas; icons (pre-rasterized
//! PNG alpha masks) and a procedural busy spinner live in the same atlas so
//! the whole UI minus the preview samples one texture. Measurements are
//! returned in logical pixels and are scale-independent (fontdue metrics are
//! linear in px size).

use std::collections::HashMap;

use ash::vk;

use crate::geom::Point;
use crate::gfx::renderer2d::{DrawList, Rgba};
use crate::gfx::upload::{self, PendingUpload, Texture};

pub const FONT_SIZE: f32 = 18.0; // logical px, matches Pango absolute size 18

const ATLAS_SIZE: u32 = 1024;
const PAD: u32 = 1;

static FONT_BYTES: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Home,
    Close,
    Selection,
    Top,
    Parent,
    Copy,
    Open,
    Terminal,
    Busy,
}

const ICON_PNGS: [(&[u8], Icon); 8] = [
    (include_bytes!("../assets/icons/home.png"), Icon::Home),
    (include_bytes!("../assets/icons/close.png"), Icon::Close),
    (include_bytes!("../assets/icons/selection.png"), Icon::Selection),
    (include_bytes!("../assets/icons/top.png"), Icon::Top),
    (include_bytes!("../assets/icons/parent.png"), Icon::Parent),
    (include_bytes!("../assets/icons/copy.png"), Icon::Copy),
    (include_bytes!("../assets/icons/open.png"), Icon::Open),
    (include_bytes!("../assets/icons/terminal.png"), Icon::Terminal),
];

#[derive(Clone, Copy)]
struct GlyphInfo {
    /// uv rect in the atlas (normalized)
    uv: [f32; 4],
    /// bitmap size in physical px
    w: f32,
    h: f32,
    /// offset from pen position (baseline, physical px, y-down)
    xmin: f32,
    ytop: f32,
    advance: f32,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

struct Packer {
    shelves: Vec<Shelf>,
    next_y: u32,
}

impl Packer {
    fn new() -> Packer {
        Packer { shelves: Vec::new(), next_y: PAD }
    }

    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 * PAD > ATLAS_SIZE {
            return None;
        }
        for shelf in &mut self.shelves {
            if h <= shelf.h && h + 8 >= shelf.h && shelf.x + w + PAD <= ATLAS_SIZE {
                let pos = (shelf.x, shelf.y);
                shelf.x += w + PAD;
                return Some(pos);
            }
        }
        if self.next_y + h + PAD > ATLAS_SIZE {
            return None;
        }
        let shelf = Shelf { y: self.next_y, h, x: PAD + w + PAD };
        self.next_y += h + PAD;
        let pos = (PAD, shelf.y);
        self.shelves.push(shelf);
        Some(pos)
    }
}

struct IconSource {
    icon: Icon,
    w: u32,
    h: u32,
    alpha: Vec<u8>,
}

pub struct TextSystem {
    font: fontdue::Font,
    pub texture: Texture,
    packer: Packer,
    glyphs: HashMap<char, Option<GlyphInfo>>,
    icon_sources: Vec<IconSource>,
    icon_uvs: HashMap<Icon, [f32; 4]>,
    pub pending: Vec<PendingUpload>,
    /// logical -> physical scale glyphs are currently rasterized at
    scale: f32,
    /// resets performed this frame (bounds atlas-full thrashing)
    resets: u32,
}

impl TextSystem {
    pub fn new(
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        scale: f32,
    ) -> Result<TextSystem, String> {
        let font = fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .map_err(|e| format!("load font: {e}"))?;
        let texture =
            upload::create_texture(device, mem_props, ATLAS_SIZE, ATLAS_SIZE, vk::Format::R8_UNORM)?;

        let mut icon_sources = Vec::new();
        for (bytes, icon) in ICON_PNGS {
            let img = image::load_from_memory(bytes)
                .map_err(|e| format!("decode icon {icon:?}: {e}"))?
                .to_rgba8();
            let (w, h) = img.dimensions();
            let alpha: Vec<u8> = img.pixels().map(|p| p.0[3]).collect();
            icon_sources.push(IconSource { icon, w, h, alpha });
        }
        icon_sources.push(spinner_source());

        let mut ts = TextSystem {
            font,
            texture,
            packer: Packer::new(),
            glyphs: HashMap::new(),
            icon_sources,
            icon_uvs: HashMap::new(),
            pending: Vec::new(),
            scale,
            resets: 0,
        };
        // The very first upload must cover the whole texture so the image
        // leaves UNDEFINED layout: zero it, then pack the icons.
        ts.pending.push(PendingUpload {
            texture_image: ts.texture.image,
            bytes: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            x: 0,
            y: 0,
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            initialized: false,
        });
        ts.pack_icons();
        Ok(ts)
    }

    /// Re-rasterize everything at a new output scale (window moved to a
    /// differently-scaled display).
    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < 1e-3 {
            return;
        }
        self.scale = scale;
        self.reset();
    }

    pub fn begin_frame(&mut self) {
        self.resets = 0;
    }

    fn reset(&mut self) {
        self.packer = Packer::new();
        self.glyphs.clear();
        self.icon_uvs.clear();
        self.pack_icons();
    }

    fn pack_icons(&mut self) {
        for i in 0..self.icon_sources.len() {
            let (w, h) = (self.icon_sources[i].w, self.icon_sources[i].h);
            if let Some((x, y)) = self.packer.pack(w, h) {
                let src = &self.icon_sources[i];
                self.pending.push(PendingUpload {
                    texture_image: self.texture.image,
                    bytes: src.alpha.clone(),
                    x,
                    y,
                    width: w,
                    height: h,
                    initialized: true,
                });
                self.icon_uvs.insert(src.icon, uv_rect(x, y, w, h));
            }
        }
    }

    pub fn icon_uv(&self, icon: Icon) -> [f32; 4] {
        self.icon_uvs.get(&icon).copied().unwrap_or([0.0; 4])
    }

    fn px(&self) -> f32 {
        FONT_SIZE * self.scale
    }

    /// Row height in logical px (replaces Pango text_size().y).
    pub fn line_height(&self) -> f32 {
        self.font
            .horizontal_line_metrics(FONT_SIZE)
            .map(|m| m.new_line_size)
            .unwrap_or(FONT_SIZE * 1.36)
    }

    fn ascent(&self) -> f32 {
        self.font.horizontal_line_metrics(FONT_SIZE).map(|m| m.ascent).unwrap_or(FONT_SIZE)
    }

    fn glyph(&mut self, ch: char) -> Option<GlyphInfo> {
        if let Some(g) = self.glyphs.get(&ch) {
            return *g;
        }
        let px = self.px();
        let (metrics, bitmap) = self.font.rasterize(ch, px);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        let pos = if w == 0 || h == 0 { Some((0, 0)) } else { self.packer.pack(w, h) };
        let pos = match pos {
            Some(p) => p,
            None => {
                // Atlas full: reset once per frame and retry; glyphs cached
                // by earlier draws re-rasterize on their next use.
                if self.resets >= 1 {
                    self.glyphs.insert(ch, None);
                    return None;
                }
                self.resets += 1;
                self.reset();
                match self.packer.pack(w, h) {
                    Some(p) => p,
                    None => {
                        self.glyphs.insert(ch, None);
                        return None;
                    }
                }
            }
        };
        if w > 0 && h > 0 {
            self.pending.push(PendingUpload {
                texture_image: self.texture.image,
                bytes: bitmap,
                x: pos.0,
                y: pos.1,
                width: w,
                height: h,
                initialized: true,
            });
        }
        let info = GlyphInfo {
            uv: uv_rect(pos.0, pos.1, w, h),
            w: w as f32,
            h: h as f32,
            xmin: metrics.xmin as f32,
            // top of bitmap relative to baseline, y-down screen coords
            ytop: -(metrics.height as f32 + metrics.ymin as f32),
            advance: metrics.advance_width,
        };
        self.glyphs.insert(ch, Some(info));
        Some(info)
    }

    /// Measure a string in logical px.
    pub fn measure(&mut self, s: &str) -> Point {
        let mut w = 0.0f32;
        for ch in s.chars() {
            if let Some(g) = self.glyph(ch) {
                w += g.advance / self.scale;
            }
        }
        Point::new(w, self.line_height())
    }

    /// Draw text with its top-left corner at `origin` (logical px), like the
    /// Pango draw_text2 did. Returns the advance width in logical px.
    pub fn draw(&mut self, list: &mut DrawList, origin: Point, s: &str, color: Rgba) -> f32 {
        let scale = self.scale;
        let baseline_y = (origin.y + self.ascent()) * scale;
        let mut pen_x = origin.x * scale;
        let start_x = pen_x;
        for ch in s.chars() {
            let Some(g) = self.glyph(ch) else { continue };
            if g.w > 0.0 && g.h > 0.0 {
                let x = (pen_x + g.xmin).round();
                let y = (baseline_y + g.ytop).round();
                list.glyph_quad_phys([x, y, x + g.w, y + g.h], g.uv, color);
            }
            pen_x += g.advance;
        }
        (pen_x - start_x) / scale
    }
}

fn uv_rect(x: u32, y: u32, w: u32, h: u32) -> [f32; 4] {
    let s = ATLAS_SIZE as f32;
    [x as f32 / s, y as f32 / s, (x + w) as f32 / s, (y + h) as f32 / s]
}

/// The C app referenced a busy.svg that never existed; generate the spinner
/// procedurally instead: a ring with an angular alpha fade and a gap.
fn spinner_source() -> IconSource {
    const S: u32 = 64;
    let mut alpha = vec![0u8; (S * S) as usize];
    let c = S as f32 / 2.0;
    let outer = 28.0;
    let inner = 20.0;
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let r = (dx * dx + dy * dy).sqrt();
            // 1px soft edge on both radii
            let ring = (outer - r).clamp(0.0, 1.0) * (r - inner).clamp(0.0, 1.0);
            if ring <= 0.0 {
                continue;
            }
            // angle 0..1, fading tail with a gap at the head
            let angle = dy.atan2(dx) / (2.0 * std::f32::consts::PI) + 0.5;
            let fade = (angle * 1.15).min(1.0) * if angle > 0.95 { 0.0 } else { 1.0 };
            alpha[(y * S + x) as usize] = (ring * fade * 255.0) as u8;
        }
    }
    IconSource { icon: Icon::Busy, w: S, h: S, alpha }
}
