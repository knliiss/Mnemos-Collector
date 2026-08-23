use std::sync::OnceLock;

use regex::Regex;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, HDC};

#[derive(Debug, Clone, Copy)]
struct MascotRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
}

static MASCOT_RECTS: OnceLock<Vec<MascotRect>> = OnceLock::new();

pub unsafe fn draw(hdc: HDC, x: i32, y: i32, width: i32) {
    let scale = width as f32 / 160.0;

    for mascot_rect in mascot_rects() {
        let left = x + (mascot_rect.x as f32 * scale).round() as i32;
        let top = y + (mascot_rect.y as f32 * scale).round() as i32;
        let right = left + (mascot_rect.width as f32 * scale).round() as i32;
        let bottom = top + (mascot_rect.height as f32 * scale).round() as i32;
        let rect = RECT {
            left,
            top,
            right,
            bottom,
        };
        let brush = unsafe { CreateSolidBrush(mascot_rect.color) };

        unsafe {
            FillRect(hdc, &rect, brush);
            DeleteObject(brush);
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
                let red = (color >> 16) & 0xff;
                let green = (color >> 8) & 0xff;
                let blue = color & 0xff;

                Some(MascotRect {
                    x: captures.name("x")?.as_str().parse().ok()?,
                    y: captures.name("y")?.as_str().parse().ok()?,
                    width: captures.name("width")?.as_str().parse().ok()?,
                    height: captures.name("height")?.as_str().parse().ok()?,
                    color: red | (green << 8) | (blue << 16),
                })
            })
            .collect()
    })
}
