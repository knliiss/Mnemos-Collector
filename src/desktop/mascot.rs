use std::ptr::null_mut;
use std::sync::OnceLock;

use regex::Regex;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, HDC};
use windows_sys::Win32::UI::WindowsAndMessaging::CreateIcon;

#[derive(Debug, Clone, Copy)]
struct MascotRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
}

#[derive(Debug, Clone, Copy)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
}

struct IconSurface<'a> {
    pixels: &'a mut [u8],
    width: usize,
    height: usize,
}

impl IconSurface<'_> {
    fn write(&mut self, x: usize, y: usize, color: RgbColor) {
        let bottom_up_y = self.height - 1 - y;
        let index = (bottom_up_y * self.width + x) * 4;

        self.pixels[index] = color.blue;
        self.pixels[index + 1] = color.green;
        self.pixels[index + 2] = color.red;
        self.pixels[index + 3] = 0xff;
    }
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

pub unsafe fn create_icon(size: i32) -> *mut core::ffi::c_void {
    let size = size.max(16);
    let width = size as usize;
    let height = size as usize;
    let mut color_bits = vec![0_u8; width * height * 4];
    let mut surface = IconSurface {
        pixels: &mut color_bits,
        width,
        height,
    };

    fill_icon_background(&mut surface);
    rasterize_mascot(&mut surface);

    let mask_stride = width.div_ceil(16) * 2;
    let mask_bits = vec![0_u8; mask_stride * height];

    unsafe {
        CreateIcon(
            null_mut(),
            size,
            size,
            1,
            32,
            mask_bits.as_ptr(),
            color_bits.as_ptr(),
        )
    }
}

fn fill_icon_background(surface: &mut IconSurface<'_>) {
    let background = RgbColor {
        red: 0x05,
        green: 0x06,
        blue: 0x05,
    };

    for y in 0..surface.height {
        for x in 0..surface.width {
            surface.write(x, y, background);
        }
    }
}

fn rasterize_mascot(surface: &mut IconSurface<'_>) {
    let size = surface.width.min(surface.height);
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
        let color = mascot_color(mascot_rect.color);

        for y in top..bottom {
            for x in left..right {
                surface.write(x, y, color);
            }
        }
    }
}

fn mascot_color(color: u32) -> RgbColor {
    RgbColor {
        red: (color & 0xff) as u8,
        green: ((color >> 8) & 0xff) as u8,
        blue: ((color >> 16) & 0xff) as u8,
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
