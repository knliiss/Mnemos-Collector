use std::sync::OnceLock;

use eframe::egui::{self, Color32, Rect};
use regex::Regex;

const SOURCE_SIZE: f32 = 160.0;

#[derive(Debug, Clone, Copy)]
struct MascotRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
}

static MASCOT_RECTS: OnceLock<Vec<MascotRect>> = OnceLock::new();

pub(super) fn paint(ui: &egui::Ui, rect: Rect) {
    let size = rect.width().min(rect.height());
    let scale = size / SOURCE_SIZE;
    let left_offset = rect.left() + (rect.width() - size) / 2.0;
    let top_offset = rect.top() + (rect.height() - size) / 2.0;

    for mascot_rect in mascot_rects() {
        let left = left_offset + (mascot_rect.x as f32 * scale).round();
        let top = top_offset + (mascot_rect.y as f32 * scale).round();
        let width = (mascot_rect.width as f32 * scale).round();
        let height = (mascot_rect.height as f32 * scale).round();
        let target = Rect::from_min_size(
            egui::pos2(left, top),
            egui::vec2(width, height),
        );

        ui.painter()
            .rect_filled(target, 0, color32(mascot_rect.color));
    }
}

pub(super) fn icon() -> egui::IconData {
    const SIZE: usize = 32;

    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    let background = [0x05, 0x06, 0x05, 0xff];

    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&background);
    }

    let scale = SIZE as f32 / SOURCE_SIZE;

    for mascot_rect in mascot_rects() {
        let left = (mascot_rect.x as f32 * scale).round().max(0.0) as usize;
        let top = (mascot_rect.y as f32 * scale).round().max(0.0) as usize;
        let right = ((mascot_rect.x + mascot_rect.width) as f32 * scale)
            .round()
            .clamp(0.0, SIZE as f32) as usize;
        let bottom = ((mascot_rect.y + mascot_rect.height) as f32 * scale)
            .round()
            .clamp(0.0, SIZE as f32) as usize;
        let color = rgba_color(mascot_rect.color);

        for y in top..bottom {
            for x in left..right {
                let index = (y * SIZE + x) * 4;
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

fn mascot_rects() -> &'static [MascotRect] {
    MASCOT_RECTS
        .get_or_init(|| {
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
                    Some(MascotRect {
                        x: captures.name("x")?.as_str().parse().ok()?,
                        y: captures.name("y")?.as_str().parse().ok()?,
                        width: captures.name("width")?.as_str().parse().ok()?,
                        height: captures.name("height")?.as_str().parse().ok()?,
                        color: u32::from_str_radix(captures.name("color")?.as_str(), 16).ok()?,
                    })
                })
                .collect()
        })
        .as_slice()
}

fn color32(color: u32) -> Color32 {
    Color32::from_rgb(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

fn rgba_color(color: u32) -> [u8; 4] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        0xff,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_pixel_art_mascot_from_shared_svg() {
        let rects = mascot_rects();

        assert!(!rects.is_empty());
        assert!(rects.iter().all(|rect| rect.width > 0 && rect.height > 0));
    }

    #[test]
    fn portable_icon_uses_expected_dimensions() {
        let icon = icon();

        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
    }
}
