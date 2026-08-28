use std::sync::OnceLock;

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use regex::Regex;

#[derive(Debug, Clone, Copy)]
struct MascotRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: Color32,
}

static MASCOT_RECTS: OnceLock<Vec<MascotRect>> = OnceLock::new();

pub fn draw(painter: &egui::Painter, rect: Rect) {
    let scale = rect.width() / 160.0;

    for mascot_rect in mascot_rects() {
        let left = rect.left() + mascot_rect.x as f32 * scale;
        let top = rect.top() + mascot_rect.y as f32 * scale;
        let width = mascot_rect.width as f32 * scale;
        let height = mascot_rect.height as f32 * scale;
        let scaled = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height));

        painter.rect_filled(scaled, 0.0, mascot_rect.color);
    }
}

pub fn icon(size: usize) -> egui::IconData {
    let size = size.max(16);
    let mut rgba = vec![0_u8; size * size * 4];
    let background = [0x05, 0x06, 0x05, 0xff];

    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&background);
    }

    rasterize_mascot(&mut rgba, size);

    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

fn rasterize_mascot(rgba: &mut [u8], size: usize) {
    let scale = size as f32 / 160.0;

    for mascot_rect in mascot_rects() {
        let left = (mascot_rect.x as f32 * scale).round().max(0.0) as usize;
        let top = (mascot_rect.y as f32 * scale).round().max(0.0) as usize;
        let right = ((mascot_rect.x + mascot_rect.width) as f32 * scale)
            .round()
            .clamp(0.0, size as f32) as usize;
        let bottom = ((mascot_rect.y + mascot_rect.height) as f32 * scale)
            .round()
            .clamp(0.0, size as f32) as usize;
        let color = mascot_rect.color;

        for y in top..bottom {
            for x in left..right {
                let index = (y * size + x) * 4;

                rgba[index] = color.r();
                rgba[index + 1] = color.g();
                rgba[index + 2] = color.b();
                rgba[index + 3] = 0xff;
            }
        }
    }
}

fn mascot_rects() -> &'static Vec<MascotRect> {
    MASCOT_RECTS.get_or_init(|| {
        let source = include_str!("../../assets/mnemos-mascot-cat.svg");
        let source = source
            .split("<g class=\"mnemos-cat-eyelids\">")
            .next()
            .unwrap_or(source);
        let pattern = Regex::new(
            r##"<rect x="(?P<x>\d+)" y="(?P<y>\d+)" width="(?P<width>\d+)" height="(?P<height>\d+)" fill="#(?P<color>[0-9A-Fa-f]{6})""##,
        )
        .expect("valid mascot rectangle regex");

        pattern
            .captures_iter(source)
            .filter_map(|captures| {
                let color = u32::from_str_radix(captures.name("color")?.as_str(), 16).ok()?;

                Some(MascotRect {
                    x: captures.name("x")?.as_str().parse().ok()?,
                    y: captures.name("y")?.as_str().parse().ok()?,
                    width: captures.name("width")?.as_str().parse().ok()?,
                    height: captures.name("height")?.as_str().parse().ok()?,
                    color: Color32::from_rgb(
                        ((color >> 16) & 0xff) as u8,
                        ((color >> 8) & 0xff) as u8,
                        (color & 0xff) as u8,
                    ),
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mascot_uses_the_same_svg_rectangles_as_windows() {
        let rectangles = mascot_rects();

        assert!(!rectangles.is_empty());
        assert_eq!(rectangles.first().unwrap().x, 48);
        assert_eq!(rectangles.first().unwrap().y, 112);
    }

    #[test]
    fn icon_has_expected_rgba_size() {
        let icon = icon(32);

        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
    }
}
