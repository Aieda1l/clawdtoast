//! Software card renderer: rounded card, sprite, status dot, two lines of text.

use crate::core::Card;
use crate::protocol::Kind;
use fontdue::{Font, FontSettings};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub const CARD_W: f64 = 320.0;
pub const CARD_H: f64 = 96.0;

const SPRITE_PNG: &[u8] = include_bytes!("../assets/clawd.png");
const CODEX_SPRITE_PNG: &[u8] = include_bytes!("../assets/openai-spin.png");

// macOS and Linux system fonts. Windows fonts are resolved from the Windows directory below.
// Helvetica.ttc is skipped: fontdue maps U+00B7 to the wrong glyph in it.
const FONT_CANDIDATES: &[(&str, usize)] = &[
    ("/System/Library/Fonts/Supplemental/Arial.ttf", 0),
    ("/Library/Fonts/Arial.ttf", 0),
    // Linux: Debian/Ubuntu, Fedora, Arch layouts.
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", 0),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf", 0),
    ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
];
const BOLD_CANDIDATES: &[(&str, usize)] = &[
    ("/System/Library/Fonts/Supplemental/Arial Bold.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", 0),
    ("/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf", 0),
    ("/usr/share/fonts/google-noto/NotoSans-Bold.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/noto/NotoSans-Bold.ttf", 0),
];

/// Asks fontconfig for a sans font when none of the well-known paths exist (Linux only).
fn fontconfig_font(style: &str) -> Option<Font> {
    if cfg!(not(target_os = "linux")) {
        return None;
    }
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", &format!("sans:{style}")])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let data = std::fs::read(path.trim()).ok()?;
    Font::from_bytes(data, FontSettings::default()).ok()
}

/// A sheet of square frames laid out left to right; frame size is the image height.
pub struct Sprite {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

pub struct Assets {
    sprite: Option<Sprite>,
    codex_sprite: Option<Sprite>,
    regular: Option<Font>,
    bold: Option<Font>,
}

fn load_font(candidates: &[(&str, usize)]) -> Option<Font> {
    candidates.iter().find_map(|(path, idx)| {
        let data = std::fs::read(path).ok()?;
        let settings = FontSettings { collection_index: *idx as u32, ..Default::default() };
        Font::from_bytes(data, settings).ok()
    })
}

#[cfg(target_os = "windows")]
fn windows_font(bold: bool) -> Option<Font> {
    let root = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot"))?;
    let dir = std::path::PathBuf::from(root).join("Fonts");
    let names: &[&str] = if bold {
        &["segoeuib.ttf", "arialbd.ttf", "tahomabd.ttf", "verdanab.ttf"]
    } else {
        &["segoeui.ttf", "arial.ttf", "tahoma.ttf", "verdana.ttf"]
    };
    names.iter().find_map(|name| {
        let data = std::fs::read(dir.join(name)).ok()?;
        Font::from_bytes(data, FontSettings::default()).ok()
    })
}

#[cfg(not(target_os = "windows"))]
fn windows_font(_bold: bool) -> Option<Font> {
    None
}

fn load_sprite(png_bytes: &[u8]) -> Option<Sprite> {
    let mut reader = png::Decoder::new(png_bytes).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight).then(|| Sprite {
        w: info.width as usize,
        h: info.height as usize,
        rgba: buf[..info.buffer_size()].to_vec(),
    })
}

impl Sprite {
    pub fn frames(&self) -> usize {
        (self.w / self.h).max(1)
    }
}

impl Assets {
    pub fn frames(&self) -> usize {
        self.sprite.as_ref().map_or(1, Sprite::frames)
    }

    fn sprite_for(&self, card: &Card) -> Option<&Sprite> {
        if card.agent == "codex" {
            self.codex_sprite.as_ref().or(self.sprite.as_ref())
        } else {
            self.sprite.as_ref()
        }
    }

    pub fn load() -> Assets {
        let regular = windows_font(false)
            .or_else(|| load_font(FONT_CANDIDATES))
            .or_else(|| fontconfig_font("regular"));
        let bold = windows_font(true)
            .or_else(|| load_font(BOLD_CANDIDATES))
            .or_else(|| fontconfig_font("bold"))
            .or_else(|| windows_font(false))
            .or_else(|| load_font(FONT_CANDIDATES));
        if regular.is_none() && bold.is_none() {
            crate::log::line("renderer: no system fonts found; notification text cannot render");
        }
        Assets { sprite: load_sprite(SPRITE_PNG), codex_sprite: load_sprite(CODEX_SPRITE_PNG), regular, bold }
    }
}

fn dot_color(kind: Kind) -> Color {
    match kind {
        Kind::Done => Color::from_rgba8(0x5B, 0xC2, 0x6B, 255),
        Kind::Permission => Color::from_rgba8(0xF2, 0xA9, 0x3B, 255),
        Kind::Idle => Color::from_rgba8(0x6C, 0xA8, 0xF0, 255),
        _ => Color::from_rgba8(0xB0, 0xB0, 0xB0, 255),
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Draws `s` with its baseline at `y`; returns the advance. Coverage is blended over the pixmap.
fn draw_text(pm: &mut Pixmap, font: &Font, s: &str, px: f32, x: f32, y: f32, rgb: (u8, u8, u8)) -> f32 {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut pen = x;
    for ch in s.chars() {
        let (m, bitmap) = font.rasterize(ch, px);
        let gx = pen.round() as i32 + m.xmin;
        let gy = y.round() as i32 - m.height as i32 - m.ymin;
        for row in 0..m.height {
            for col in 0..m.width {
                let cov = bitmap[row * m.width + col] as u32;
                let (tx, ty) = (gx + col as i32, gy + row as i32);
                if cov == 0 || tx < 0 || ty < 0 || tx >= pw || ty >= ph {
                    continue;
                }
                let i = ((ty * pw + tx) * 4) as usize;
                // Source is opaque text colour scaled by coverage (premultiplied).
                let inv = 255 - cov;
                data[i] = ((rgb.0 as u32 * cov + data[i] as u32 * inv) / 255) as u8;
                data[i + 1] = ((rgb.1 as u32 * cov + data[i + 1] as u32 * inv) / 255) as u8;
                data[i + 2] = ((rgb.2 as u32 * cov + data[i + 2] as u32 * inv) / 255) as u8;
                data[i + 3] = (cov + data[i + 3] as u32 * inv / 255).min(255) as u8;
            }
        }
        pen += m.advance_width;
    }
    pen - x
}

fn text_width(font: &Font, s: &str, px: f32) -> f32 {
    s.chars().map(|c| font.metrics(c, px).advance_width).sum()
}

/// Greedy word wrap into at most `max_lines` lines, ellipsizing the last if text is cut off.
fn wrap(font: &Font, s: &str, px: f32, max_w: f32, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        match lines.last_mut() {
            Some(cur) if text_width(font, &format!("{cur} {word}"), px) <= max_w => {
                cur.push(' ');
                cur.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    let cut = lines.len() > max_lines;
    lines.truncate(max_lines);
    let last = lines.len().saturating_sub(1);
    for (i, l) in lines.iter_mut().enumerate() {
        // The last line gets an ellipsis if text was cut; any line wider than the card is hard-cut.
        let needs = (cut && i == last) || text_width(font, l, px) > max_w;
        if needs {
            while !l.is_empty() && text_width(font, &format!("{l}…"), px) > max_w {
                l.pop();
            }
            l.push('…');
        }
    }
    lines
}

/// Renders a card at `scale`, showing sprite sheet frame `frame` (wraps). into premultiplied 0xAARRGGBB pixels (`w * h` of them).
pub fn render(card: &Card, assets: &Assets, frame: usize, w: u32, h: u32, scale: f64) -> Vec<u32> {
    let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let s = scale as f32;
    let (fw, fh) = (w as f32, h as f32);

    if let Some(path) = rounded_rect(0.5 * s, 0.5 * s, fw - s, fh - s, 14.0 * s) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(0x26, 0x24, 0x22, 0xF5));
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        paint.set_color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x26));
        let stroke = Stroke { width: s, ..Default::default() };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    if let Some(sp) = assets.sprite_for(card) {
        // Whole-number upscale keeps the pixel art crisp.
        let k = ((2.0 * scale).round() as usize).max(1);
        let (ox, oy) = ((16.0 * s) as usize, ((CARD_H as f32 * s) as usize).saturating_sub(sp.h * k) / 2);
        let frame_off = (frame % sp.frames()) * sp.h;
        let (pw, ph) = (pm.width() as usize, pm.height() as usize);
        let data = pm.data_mut();
        for y in 0..sp.h * k {
            for x in 0..sp.h * k {
                let si = ((y / k) * sp.w + frame_off + x / k) * 4;
                let a = sp.rgba[si + 3] as u32;
                let (dx, dy) = (ox + x, oy + y);
                if a == 0 || dx >= pw || dy >= ph {
                    continue;
                }
                let di = (dy * pw + dx) * 4;
                for c in 0..3 {
                    data[di + c] = (sp.rgba[si + c] as u32 * a / 255) as u8;
                }
                data[di + 3] = a as u8;
            }
        }
    }

    let text_x = (16.0 + 64.0 + 16.0) * s;
    let max_w = (CARD_W as f32 - 16.0 - 80.0 - 14.0) * s;
    let dot_r = 4.5 * s;
    let title_base = 34.0 * s;
    if let Some(circle) = {
        let mut pb = PathBuilder::new();
        pb.push_circle(text_x + dot_r, title_base - 5.0 * s, dot_r);
        pb.finish()
    } {
        let mut paint = Paint::default();
        paint.set_color(dot_color(card.kind));
        paint.anti_alias = true;
        pm.fill_path(&circle, &paint, FillRule::Winding, Transform::identity(), None);
    }

    let reg = assets.regular.as_ref().or(assets.bold.as_ref());
    let bold = assets.bold.as_ref().or(reg);
    if let (Some(bold), Some(reg)) = (bold, reg) {
        let tx = text_x + dot_r * 2.0 + 7.0 * s;
        let mut title = card.title();
        let tw = max_w - (tx - text_x);
        while text_width(bold, &title, 14.0 * s) > tw && title.pop().is_some() {}
        draw_text(&mut pm, bold, &title, 14.0 * s, tx, title_base, (0xF5, 0xF1, 0xEC));
        for (i, line) in wrap(reg, &card.body, 12.5 * s, max_w, 2).iter().enumerate() {
            let y = (54.0 + 17.0 * i as f32) * s;
            draw_text(&mut pm, reg, line, 12.5 * s, text_x, y, (0xC9, 0xC3, 0xBC));
        }
    }

    pm.data()
        .chunks_exact(4)
        .map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
        .collect()
}

/// Composites premultiplied 0xAARRGGBB pixels onto the card's solid colour, for X11 screens with
/// no compositor, where a window cannot be transparent.
pub fn flatten(pixels: &mut [u32]) {
    const BG: [u32; 3] = [0x26, 0x24, 0x22];
    for p in pixels {
        let inv = 255 - (*p >> 24);
        let ch = |shift: u32, bg: u32| (((*p >> shift) & 0xFF) + bg * inv / 255).min(255);
        *p = 0xFF00_0000 | ch(16, BG[0]) << 16 | ch(8, BG[1]) << 8 | ch(0, BG[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::{fade, flatten};

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_fonts_render_notification_body() {
        use super::{render, Assets, CARD_H, CARD_W};
        use crate::core::Card;
        use crate::protocol::Kind;

        let assets = Assets::load();
        assert!(assets.regular.is_some(), "Windows regular font should load");
        assert!(assets.bold.is_some(), "Windows bold font should load");

        let mut card = Card {
            session_id: "test".into(), kind: Kind::Done, project: "project".into(),
            body: String::new(), badge: None, ts: 0, pid: 0, app: String::new(), agent: String::new(),
        };
        let without_body = render(&card, &assets, 0, CARD_W as u32, CARD_H as u32, 1.0);
        card.body = "Finished the Codex task".into();
        let with_body = render(&card, &assets, 0, CARD_W as u32, CARD_H as u32, 1.0);

        // Restrict to the body area so sprite, title, and status dot cannot satisfy the test.
        let body_changed = (42..89).any(|y| {
            (96..310).any(|x| {
                let pos = y * CARD_W as usize + x;
                without_body[pos] != with_body[pos]
            })
        });
        assert!(body_changed, "Notification body must affect rendered pixels");
    }

    #[test]
    fn codex_cards_use_the_openai_sprite_sheet() {
        use super::{render, Assets, CARD_H, CARD_W};
        use crate::core::Card;
        use crate::protocol::Kind;

        let assets = Assets::load();
        let sheet = assets.codex_sprite.as_ref().expect("codex sprite embedded");
        assert_eq!((sheet.w, sheet.h, sheet.frames()), (128, 32, 4));

        let mut card = Card {
            session_id: "t".into(), kind: Kind::Done, project: "p".into(),
            body: String::new(), badge: None, ts: 0, pid: 0, app: String::new(), agent: String::new(),
        };
        let draw = |c: &Card, f| render(c, &assets, f, CARD_W as u32, CARD_H as u32, 1.0);
        let claude = draw(&card, 0);
        card.agent = "codex".into();
        let codex: Vec<Vec<u32>> = (0..4).map(|f| draw(&card, f)).collect();
        assert_ne!(claude, codex[0], "codex card must not use the Clawd sprite");
        for f in 1..4 {
            assert_ne!(codex[0], codex[f], "frame {f} must differ so the logo spins");
        }
    }

    #[test]
    fn fade_scales_every_premultiplied_channel() {
        let mut px = [0xFF80_4020, 0x1111_1111];
        fade(&mut px, 1.0);
        assert_eq!(px[0], 0xFF80_4020);
        fade(&mut px, 0.5);
        assert_eq!(px[0], 0x7F40_2010);
        fade(&mut px, 0.0);
        assert_eq!(px, [0, 0]);
    }

    #[test]
    fn flatten_fills_transparent_with_card_colour_and_keeps_opaque() {
        let mut px = [0x0000_0000, 0xFFAA_BBCC];
        flatten(&mut px);
        assert_eq!(px, [0xFF26_2422, 0xFFAA_BBCC]);
    }
}

/// Scales premultiplied 0xAARRGGBB pixels by `alpha` (0..=1), for compositors with no window opacity.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn fade(pixels: &mut [u32], alpha: f64) {
    let a = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
    for p in pixels {
        let ch = |shift: u32| (((*p >> shift) & 0xFF) * a >> 8).min(255);
        *p = ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
    }
}
