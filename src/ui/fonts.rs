//! UI fonts: FA's TrueType fonts from the VFS (`/fonts/*.ttf`), by the
//! family names FAF's UI asks for. Metrics feed the text controls' layout
//! LazyVars (`TextAdvance`, `FontAscent`, `FontDescent`); the renderer
//! rasterises glyphs from the same fonts.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ab_glyph::{Font, FontArc, PxScale};

use crate::engine::vfs::Vfs;

/// The file for a family name (unknown families fall back to Arial, as
/// does "Arial Gras": `probe_ui_controls` measures it as plain Arial).
pub fn file_for(family: &str) -> &'static str {
    match family.to_ascii_lowercase().as_str() {
        "arial bold" | "arialbd" => "/fonts/arialbd.ttf",
        "arial black" => "/fonts/ariblk.ttf",
        "arial italic" => "/fonts/ariali.ttf",
        "arial bold italic" => "/fonts/arialbi.ttf",
        "arial narrow" => "/fonts/arialn.ttf",
        "arial narrow bold" => "/fonts/arialnb.ttf",
        "arial rounded mt bold" | "arial rounded" => "/fonts/arlrdbd.ttf",
        "butterbelly" => "/fonts/butterbe.ttf",
        "vdub" => "/fonts/vdub.ttf",
        "wintermute" => "/fonts/wintermu.ttf",
        "zeroes three" | "zeroes 3" => "/fonts/zeroes_3.ttf",
        _ => "/fonts/arial.ttf",
    }
}

/// A point size's em in whole pixels: moho draws a size-n font with an em
/// of n pixels (`probe_ui_controls` text: Arial 14 measures as a 14-pixel
/// em). Size 0 (or less) draws at 20 pixels: `SetFont('Arial', 0)` measures
/// ascent 19 and "Hello World" 101, which only a 20-pixel em gives.
pub fn ppem(pointsize: f32) -> u16 {
    if pointsize.is_nan() || pointsize <= 0.0 {
        return 20;
    }
    pointsize.round().clamp(1.0, 512.0) as u16
}

/// ab_glyph's scale for an em of `ppem` pixels (its scale is the ascent to
/// descent height, not the em).
pub fn em_scale(font: &FontArc, ppem: u16) -> PxScale {
    let upem = font.units_per_em().unwrap_or(2048.0);
    PxScale::from(f32::from(ppem) * font.height_unscaled() / upem)
}

/// What a line of one font at one size needs to be drawn: the face, its
/// scale, and its whole-pixel ascent.
pub struct Line {
    pub font: FontArc,
    pub scale: PxScale,
    pub ppem: u16,
    pub ascent: f32,
}

/// Loaded fonts by file (lookups only).
pub struct Fonts {
    vfs: Arc<RwLock<Vfs>>,
    loaded: HashMap<&'static str, Option<Face>>,
}

/// A font file and the tables its whole-pixel metrics come from.
struct Face {
    font: FontArc,
    data: Arc<[u8]>,
    upem: f32,
    win: (f32, f32),
    /// GDI's external leading in font units: the line gap beyond what the
    /// Windows ascent and descent already cover.
    leading: f32,
    /// `hdmx`: pixel size -> each glyph's advance at that size.
    hdmx: HashMap<u16, Vec<u8>>,
    /// `VDMX` (1:1 ratio): pixel size -> (ascent, descent), whole pixels.
    vdmx: HashMap<u16, (i16, i16)>,
    hinters: HashMap<u16, Option<skrifa::outline::HintingInstance>>,
    advances: HashMap<(u16, u16), f32>,
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

impl Face {
    fn new(bytes: Vec<u8>) -> Option<Self> {
        use skrifa::raw::types::Tag;
        let data: Arc<[u8]> = bytes.into();
        let font = FontArc::try_from_vec(data.to_vec()).map_err(|e| tracing::warn!("font failed to load: {e}")).ok()?;
        let r = skrifa::FontRef::new(&data).ok()?;
        let table = |tag: &[u8; 4]| r.table_data(Tag::new(tag)).map(|t| t.as_bytes().to_vec()).unwrap_or_default();
        let (head, hhea, os2) = (table(b"head"), table(b"hhea"), table(b"OS/2"));
        let upem = f32::from(be16(&head, 18).filter(|&u| u > 0).unwrap_or(2048));
        let i16_at = |d: &[u8], at| be16(d, at).map_or(0.0, |v| f32::from(v as i16));
        let (asc, desc, gap) = (i16_at(&hhea, 4), i16_at(&hhea, 6), i16_at(&hhea, 8));
        let win = match (be16(&os2, 74), be16(&os2, 76)) {
            (Some(a), Some(d)) => (f32::from(a), f32::from(d)),
            _ => (asc, -desc),
        };
        let leading = (gap - ((win.0 + win.1) - (asc - desc))).max(0.0);
        let mut hdmx = HashMap::new();
        let h = table(b"hdmx");
        if let (Some(n), Some(size)) = (be16(&h, 2), h.get(4..8).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)) {
            let glyphs = font.glyph_count();
            for i in 0..usize::from(n) {
                let at = 8 + i * size;
                if let Some(rec) = h.get(at..at + 2 + glyphs) {
                    hdmx.insert(u16::from(rec[0]), rec[2..].to_vec());
                }
            }
        }
        let mut vdmx = HashMap::new();
        let v = table(b"VDMX");
        if let Some(ratios) = be16(&v, 4) {
            // The first ratio covering a square pixel (or any ratio).
            let pick = (0..usize::from(ratios)).find(|&i| match v.get(6 + i * 4..10 + i * 4) {
                Some(&[_, x, y0, y1]) => x == 0 || (y0 <= x && x <= y1),
                _ => false,
            });
            if let Some(off) = pick.and_then(|i| be16(&v, 6 + usize::from(ratios) * 4 + i * 2)) {
                let off = usize::from(off);
                for k in 0..usize::from(be16(&v, off).unwrap_or(0)) {
                    let at = off + 4 + k * 6;
                    if let (Some(size), Some(max), Some(min)) = (be16(&v, at), be16(&v, at + 2), be16(&v, at + 4)) {
                        vdmx.insert(size, (max as i16, min as i16));
                    }
                }
            }
        }
        Some(Face { font, data, upem, win, leading, hdmx, vdmx, hinters: HashMap::new(), advances: HashMap::new() })
    }

    /// Whole-pixel ascent and descent (positive) at `ppem`: the `VDMX`
    /// table's, else the Windows ascent and descent rounded, as GDI.
    fn vertical(&self, ppem: u16) -> (f32, f32) {
        match self.vdmx.get(&ppem) {
            Some(&(max, min)) => (f32::from(max), -f32::from(min)),
            None => {
                let k = f32::from(ppem) / self.upem;
                ((self.win.0 * k).round(), (self.win.1 * k).round())
            }
        }
    }

    /// A glyph's whole-pixel advance at `ppem`: the `hdmx` table's, else the
    /// advance after full (black-and-white) TrueType hinting, else the
    /// linear one rounded. Full hinting is what matches moho at the sizes
    /// Arial has no `hdmx` record for (14, 18; `probe_ui_controls`); the
    /// lighter smooth hinting is a pixel off on 'M' at 14.
    fn advance(&mut self, ppem: u16, glyph: ab_glyph::GlyphId) -> f32 {
        if let Some(&w) = self.hdmx.get(&ppem).and_then(|w| w.get(usize::from(glyph.0))) {
            return f32::from(w);
        }
        if let Some(&w) = self.advances.get(&(ppem, glyph.0)) {
            return w;
        }
        let linear = (self.font.h_advance_unscaled(glyph) * f32::from(ppem) / self.upem).round();
        let w = self.hinted_advance(ppem, glyph.0).unwrap_or(linear);
        self.advances.insert((ppem, glyph.0), w);
        w
    }

    fn hinted_advance(&mut self, ppem: u16, glyph: u16) -> Option<f32> {
        use skrifa::MetadataProvider;
        use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, Target, pen::NullPen};
        let font = skrifa::FontRef::new(&self.data).ok()?;
        let outlines = font.outline_glyphs();
        let hinter = self
            .hinters
            .entry(ppem)
            .or_insert_with(|| {
                let options = HintingOptions { engine: Engine::Interpreter, target: Target::Mono };
                HintingInstance::new(&outlines, skrifa::instance::Size::new(f32::from(ppem)), skrifa::instance::LocationRef::default(), options).ok()
            })
            .as_ref()?;
        let g = outlines.get(skrifa::GlyphId::new(u32::from(glyph)))?;
        let m = g.draw(DrawSettings::hinted(hinter, false), &mut NullPen).ok()?;
        m.advance_width.map(f32::round)
    }
}

/// One line's metrics at a size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub advance: f32,
    /// Extra space between lines (GDI's external leading).
    pub leading: f32,
}

impl Fonts {
    pub fn new(vfs: Arc<RwLock<Vfs>>) -> Self {
        Self { vfs, loaded: HashMap::new() }
    }

    pub fn font(&mut self, family: &str) -> Option<FontArc> {
        self.font_file(file_for(family))
    }

    /// The font in VFS file `file` (one of [`file_for`]'s).
    pub fn font_file(&mut self, file: &'static str) -> Option<FontArc> {
        self.face(file).map(|f| f.font.clone())
    }

    fn face(&mut self, file: &'static str) -> Option<&mut Face> {
        let vfs = &self.vfs;
        self.loaded
            .entry(file)
            .or_insert_with(|| {
                let bytes = vfs.read().ok()?.read(file).ok()?;
                let face = Face::new(bytes);
                if face.is_none() {
                    tracing::warn!(file, "font failed to load");
                }
                face
            })
            .as_mut()
    }

    /// How to draw a line of `family` at `pointsize` (see [`Line`]).
    pub fn line(&mut self, family: &str, pointsize: f32) -> Option<Line> {
        let ppem = ppem(pointsize);
        let face = self.face(file_for(family))?;
        Some(Line { scale: em_scale(&face.font, ppem), ppem, ascent: face.vertical(ppem).0, font: face.font.clone() })
    }

    /// A glyph's whole-pixel advance in `family` at `ppem` (moho lays text
    /// out by these, with no kerning: `GetStringAdvance('AV')` is A's plus V's).
    pub fn advance(&mut self, family: &str, ppem: u16, glyph: ab_glyph::GlyphId) -> f32 {
        self.face(file_for(family)).map_or(0.0, |f| f.advance(ppem, glyph))
    }

    /// Ascent, descent (positive), leading and the advance of `text`, in
    /// whole pixels, as moho measures (`probe_ui_controls` text).
    pub fn measure(&mut self, family: &str, pointsize: f32, text: &str) -> Metrics {
        let ppem = ppem(pointsize);
        let Some(face) = self.face(file_for(family)) else { return Metrics { ascent: pointsize, descent: pointsize * 0.25, advance: text.chars().count() as f32 * pointsize * 0.5, leading: 0.0 } };
        let mut advance = 0.0;
        // Control characters take no room (`'a\nb'` measures as `'ab'`).
        for c in text.chars().filter(|c| !c.is_control()) {
            let g = face.font.glyph_id(c);
            advance += face.advance(ppem, g);
        }
        let (ascent, descent) = face.vertical(ppem);
        Metrics { ascent, descent, advance, leading: (face.leading * f32::from(ppem) / face.upem).round() }
    }
}
