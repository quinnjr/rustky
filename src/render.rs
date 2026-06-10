use std::sync::Arc;

use skia_rs::prelude::*;
use skia_rs_canvas::Surface;

use crate::styled::StyledLine;

pub struct Renderer {
    pub font: Font,
    pub font_size: f32,
    pub fg: Color,
    pub bg: Color,
    pub typeface: Arc<Typeface>,
}

pub fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    match bytes.len() {
        3 => Color::from_argb(255, bytes[0], bytes[1], bytes[2]),
        4 => Color::from_argb(bytes[3], bytes[0], bytes[1], bytes[2]),
        _ => Color::WHITE,
    }
}

impl Renderer {
    pub fn new(font_size: f32, fg_hex: &str, bg_hex: &str) -> Self {
        let font_data = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/DejaVuSansMono.ttf"
        ));
        let typeface =
            Arc::new(Typeface::from_data(font_data.to_vec()).expect("failed to load font"));
        let font = Font::new(typeface.clone(), font_size);
        Self {
            font,
            font_size,
            fg: parse_hex_color(fg_hex),
            bg: parse_hex_color(bg_hex),
            typeface,
        }
    }

    #[allow(dead_code)]
    pub fn render_lines(&self, lines: &[String], width: u32, height: u32) -> Vec<u8> {
        let w = width as i32;
        let h = height as i32;

        let mut surface = Surface::new_raster_n32_premul(w, h).expect("failed to create surface");

        {
            let mut canvas = surface.raster_canvas();
            canvas.clear(self.bg);

            let mut paint = Paint::default();
            paint.set_color(self.fg.into());
            paint.set_anti_alias(true);

            let line_height = self.font_size * 1.4;
            let padding_x = 8.0;
            let mut y = line_height;

            for line in lines {
                canvas.draw_string(line, padding_x, y, &self.font, &paint);
                y += line_height;
            }
        }

        surface.pixels().to_vec()
    }

    pub fn content_height(&self, lines: &[StyledLine]) -> f32 {
        let mut h = 0.0_f32;
        for line in lines {
            let fs = line.style.font_size.unwrap_or(self.font_size);
            h += fs * 1.4;
        }
        h
    }

    #[allow(dead_code)]
    pub fn render_styled_lines(
        &self,
        lines: &[StyledLine],
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        self.render_styled_lines_scroll(lines, width, height, 0.0)
    }

    pub fn render_styled_lines_scroll(
        &self,
        lines: &[StyledLine],
        width: u32,
        height: u32,
        scroll_offset: f32,
    ) -> Vec<u8> {
        let w = width as i32;
        let h = height as i32;

        let mut surface = Surface::new_raster_n32_premul(w, h).expect("failed to create surface");

        {
            let mut canvas = surface.raster_canvas();
            canvas.clear(self.bg);

            let padding_x = 8.0;
            let height_f = height as f32;
            let mut y = -scroll_offset;

            for line in lines {
                let eff_font_size = line.style.font_size.unwrap_or(self.font_size);
                let line_height = eff_font_size * 1.4;
                y += line_height;

                // Skip lines that are fully above or below the viewport
                if y < 0.0 {
                    continue;
                }
                if y - line_height > height_f {
                    break;
                }

                // Per-line background
                if let Some(ref bg_hex) = line.style.bg_color {
                    let bg_color = parse_hex_color(bg_hex);
                    let mut bg_paint = Paint::default();
                    bg_paint.set_color(bg_color.into());
                    canvas.draw_rect(
                        &Rect::from_xywh(0.0, y - line_height, width as f32, line_height),
                        &bg_paint,
                    );
                }

                // Per-line foreground color
                let fg_color = line
                    .style
                    .fg_color
                    .as_deref()
                    .map(parse_hex_color)
                    .unwrap_or(self.fg);

                let mut paint = Paint::default();
                paint.set_color(fg_color.into());
                paint.set_anti_alias(true);

                // Per-line font size: reuse default font or create a custom one
                if (eff_font_size - self.font_size).abs() < 0.01 {
                    canvas.draw_string(&line.text, padding_x, y, &self.font, &paint);
                } else {
                    let custom_font = Font::new(self.typeface.clone(), eff_font_size);
                    canvas.draw_string(&line.text, padding_x, y, &custom_font, &paint);
                }
            }
        }

        surface.pixels().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::styled::StyledLine;

    fn render_text(text: &str) -> Vec<u8> {
        let renderer = Renderer::new(13.0, "#ffffff", "#000000");
        let lines = vec![StyledLine::plain(text.to_string())];
        renderer.render_styled_lines_scroll(&lines, 100, 40, 0.0)
    }

    fn lit_pixels(pixels: &[u8]) -> usize {
        // background is opaque black; any non-zero RGB channel means a glyph pixel
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 0 || p[1] > 0 || p[2] > 0)
            .count()
    }

    #[test]
    fn space_renders_no_pixels() {
        assert_eq!(lit_pixels(&render_text(" ")), 0);
    }

    #[test]
    fn glyph_renders_sparse_coverage() {
        let lit = lit_pixels(&render_text("i"));
        assert!(lit > 0, "glyph 'i' should produce some pixels");
        // placeholder code fills a solid 6.5x13 box (~84 px); a real 'i' is far sparser
        assert!(lit < 40, "glyph 'i' too dense ({lit} px) — looks like a solid box");
    }
}
