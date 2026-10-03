// SPDX-License-Identifier: GPL-3.0-or-later
//! Painting one output's lock screen over its wallpaper.
//!
//! The caller copies the already-rendered (and, if configured, blurred) wallpaper into the buffer
//! first; this draws everything on top of it. The text that sits straight on the wallpaper --
//! clock, date, name, messages -- is white with a soft dark shadow, because the wallpaper can be
//! anything and white alone vanishes on a pale photograph. The field and the button are Motif
//! chrome in the configured palette: the arrangement is KDE's, the widgets are IRIX's.

use wlrix_ui::Rgb;
use wlrix_ui::canvas::{Canvas, Rect};
use wlrix_ui::motif::{self, Bevel};
use wlrix_ui::palette::Palette;
use wlrix_ui::text::{Face, Fonts};

use crate::avatar::Avatar;
use crate::ui::layout::Layout;

/// The text drawn straight on the wallpaper.
const TEXT: Rgb = Rgb::from_channels(0xff, 0xff, 0xff);
/// Its shadow, and how strong it is at full glyph coverage.
const SHADOW: Rgb = Rgb::from_channels(0x00, 0x00, 0x00);
const SHADOW_ALPHA: u32 = 0x90;

/// What a frame shows, apart from the wallpaper and the layout.
pub struct View<'a> {
    pub time: &'a str,
    pub date: &'a str,
    pub name: &'a str,
    /// How many characters have been typed, shown as that many dots.
    pub typed: usize,
    /// Whether the keyboard is on this output's lock surface, which earns the field its keyline.
    pub focused: bool,
    /// The visible half of the caret's blink.
    pub caret_on: bool,
    /// A check is running: the button stays in and the caret is hidden.
    pub checking: bool,
    pub message: Option<&'a str>,
    pub caps_lock: bool,
}

/// Draw everything over the wallpaper already in `canvas`.
pub fn paint(
    canvas: &mut Canvas,
    layout: &Layout,
    palette: &Palette,
    fonts: &mut Fonts,
    avatar: &mut Avatar,
    view: &View,
) {
    let unit = layout.unit;
    let shadow_offset = (2.0 * unit).round().max(1.0) as i32;

    centered(
        canvas,
        fonts,
        Face::Regular,
        layout.clock_px,
        layout.center_x,
        layout.clock_baseline,
        view.time,
        shadow_offset,
    );
    centered(
        canvas,
        fonts,
        Face::Regular,
        layout.date_px,
        layout.center_x,
        layout.date_baseline,
        view.date,
        shadow_offset,
    );

    avatar.draw(
        canvas,
        layout.center_x,
        layout.avatar_center_y,
        layout.avatar_diameter,
    );

    centered(
        canvas,
        fonts,
        Face::Regular,
        layout.name_px,
        layout.center_x,
        layout.name_baseline,
        view.name,
        (unit.round() as i32).max(1),
    );

    field(canvas, layout, palette, fonts, view);
    button(canvas, layout, palette, view.checking);

    // The message under the field, and the caps-lock warning under that when both apply.
    let mut baseline = layout.message_baseline;
    let line = fonts.line_height(Face::Regular, layout.message_px);
    for text in [view.message, view.caps_lock.then_some("Caps Lock is on")]
        .into_iter()
        .flatten()
    {
        centered(
            canvas,
            fonts,
            Face::Regular,
            layout.message_px,
            layout.center_x,
            baseline,
            text,
            (unit.round() as i32).max(1),
        );
        baseline += line;
    }
}

/// The password field: a sunken Motif well with a dot per typed character.
fn field(canvas: &mut Canvas, layout: &Layout, palette: &Palette, fonts: &mut Fonts, view: &View) {
    let rect = layout.field;
    let unit = layout.unit;
    let thickness = scaled(palette.metrics.shadow_thickness_text, unit);
    motif::panel(
        canvas,
        rect,
        palette.text_field_background,
        Bevel::sunken(
            palette.text_top_shadow,
            palette.text_bottom_shadow,
            thickness,
        ),
    );
    if view.focused {
        let keyline = (unit.round() as i32).max(1);
        for i in 0..keyline {
            canvas.stroke_rect(rect.inset(thickness + i), palette.outer_line);
        }
    }

    let inner = rect.inset(thickness + (unit.round() as i32).max(1));
    let left = inner.x + scaled(palette.metrics.text_margin_width, unit);
    let center_y = rect.y + rect.h / 2;

    if view.typed == 0 && !view.checking {
        let baseline = fonts.centered_baseline(Face::Italic, layout.field_px, rect);
        draw_text(
            canvas,
            fonts,
            Face::Italic,
            layout.field_px,
            left,
            baseline,
            "Password",
            palette.disabled_foreground,
            0,
        );
    }

    // Dots, clipped to the well: a long password shows as many as fit, and the caret stays at
    // the right-hand end rather than walking out of the field.
    let dot = (7.0 * unit).round().max(3.0) as i32;
    let pitch = (12.0 * unit).round().max(dot as f32 + 1.0) as i32;
    let room = ((inner.right() - left - pitch) / pitch).max(0) as usize;
    let shown = view.typed.min(room);
    for i in 0..shown {
        let cx = left + i as i32 * pitch + dot / 2;
        disc(canvas, cx, center_y, dot as f32 / 2.0, palette.foreground);
    }

    if view.focused && view.caret_on && !view.checking {
        let x = if shown == 0 {
            left
        } else {
            left + shown as i32 * pitch - (pitch - dot) / 2
        };
        let width = (unit.round() as i32).max(1);
        let height = (rect.h - 2 * thickness - (8.0 * unit) as i32).max(1);
        canvas.fill_rect(
            Rect::new(x, center_y - height / 2, width, height),
            palette.foreground,
        );
    }
}

/// The unlock button: a raised Motif face with a right-pointing chevron. It stays pressed while a
/// check is running, which is the whole of the "working" indication -- KDE spins, IRIX would not.
fn button(canvas: &mut Canvas, layout: &Layout, palette: &Palette, pressed: bool) {
    let rect = layout.button;
    let unit = layout.unit;
    let thickness = scaled(palette.metrics.shadow_thickness_default, unit);
    let (fill, bevel) = if pressed {
        (
            palette.armed,
            Bevel::sunken(
                palette.face_top_shadow,
                palette.face_bottom_shadow,
                thickness,
            ),
        )
    } else {
        (
            palette.face,
            Bevel::raised(
                palette.face_top_shadow,
                palette.face_bottom_shadow,
                thickness,
            ),
        )
    };
    motif::panel(canvas, rect, fill, bevel);

    let nudge = if pressed {
        (unit.round() as i32).max(1)
    } else {
        0
    };
    let size = (5.0 * unit).round().max(3.0) as i32;
    let stroke = (2.0 * unit).round().max(1.0) as i32;
    let cx = rect.x + rect.w / 2 + nudge;
    let cy = rect.y + rect.h / 2 + nudge;
    let x0 = cx - size / 2;
    for i in 0..=size {
        for t in 0..stroke {
            canvas.put(x0 + i + t, cy - size + i, palette.foreground);
            canvas.put(x0 + i + t, cy + size - i, palette.foreground);
        }
    }
}

/// A line of shadowed text centered on `center_x`.
#[allow(clippy::too_many_arguments)]
fn centered(
    canvas: &mut Canvas,
    fonts: &mut Fonts,
    face: Face,
    px: f32,
    center_x: i32,
    baseline: i32,
    text: &str,
    shadow: i32,
) {
    if text.is_empty() {
        return;
    }
    let width = fonts.width(face, px, text);
    draw_text(
        canvas,
        fonts,
        face,
        px,
        center_x - width / 2,
        baseline,
        text,
        TEXT,
        shadow,
    );
}

/// A line of text with its left edge at `x`, on `baseline`, with a soft shadow `shadow` pixels
/// down and right (none for zero).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    canvas: &mut Canvas,
    fonts: &mut Fonts,
    face: Face,
    px: f32,
    x: i32,
    baseline: i32,
    text: &str,
    color: Rgb,
    shadow: i32,
) {
    let Some(raster) = fonts.rasterize(face, px, text) else {
        return;
    };
    // The raster's top is the line's top, so the baseline is one ascent below it.
    let top = baseline - fonts.ascent(face, px);
    if shadow > 0 {
        for row in 0..raster.height {
            for col in 0..raster.width {
                let coverage = u32::from(raster.get(col, row)) * SHADOW_ALPHA / 255;
                canvas.blend(x + col + shadow, top + row + shadow, SHADOW, coverage as u8);
            }
        }
    }
    raster.draw(canvas, x, top, color);
}

/// A filled, antialiased disc.
fn disc(canvas: &mut Canvas, cx: i32, cy: i32, radius: f32, color: Rgb) {
    let extent = radius.ceil() as i32 + 1;
    for y in -extent..=extent {
        for x in -extent..=extent {
            let distance = ((x as f32).powi(2) + (y as f32).powi(2)).sqrt();
            let coverage = ((radius - distance + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            canvas.blend(cx + x, cy + y, color, coverage);
        }
    }
}

/// A palette metric scaled to the output, never thinner than a pixel.
fn scaled(metric: i32, unit: f32) -> i32 {
    ((metric as f32 * unit).round() as i32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlrix_ui::palette::DEFAULT;

    fn frame(view: &View) -> (Vec<u8>, Layout) {
        let (w, h) = (1920, 1080);
        let layout = Layout::compute(w, h, 1);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let mut canvas = Canvas::new(&mut pixels, w, h);
        canvas.clear(Rgb::from_channels(0x55, 0x55, 0x55));
        let mut fonts = Fonts::load().expect("system fonts");
        let mut avatar = Avatar::load(&[]);
        paint(&mut canvas, &layout, DEFAULT, &mut fonts, &mut avatar, view);
        (pixels, layout)
    }

    fn view(typed: usize, checking: bool) -> View<'static> {
        View {
            time: "14:20",
            date: "Friday, September 18, 2026",
            name: "Victor Fugazzotto",
            typed,
            focused: true,
            caret_on: false,
            checking,
            message: None,
            caps_lock: false,
        }
    }

    fn pixel(pixels: &[u8], x: i32, y: i32) -> Rgb {
        let i = ((y * 1920 + x) * 4) as usize;
        Rgb(u32::from_ne_bytes(pixels[i..i + 4].try_into().unwrap()))
    }

    #[test]
    fn the_field_is_a_sunken_well() {
        let (pixels, l) = frame(&view(0, false));
        // Sunken: the dark shadow is on the top edge.
        assert_eq!(
            pixel(&pixels, l.field.x + l.field.w / 2, l.field.y),
            DEFAULT.text_bottom_shadow
        );
    }

    #[test]
    fn typed_characters_show_as_dots_and_nothing_else() {
        let (empty, l) = frame(&view(0, false));
        let (three, _) = frame(&view(3, false));
        let cy = l.field.y + l.field.h / 2;
        // Inside the bevel and the focus keyline, which is drawn in the same black.
        let well = l.field.inset(DEFAULT.metrics.shadow_thickness_text + 2);
        let count = |pixels: &[u8]| {
            (well.x..well.right())
                .filter(|&x| pixel(pixels, x, cy) == DEFAULT.foreground)
                .count()
        };
        // The placeholder is not in the foreground color, so the empty field has none on the
        // center row; three dots put some there.
        assert_eq!(count(&empty), 0);
        assert!(count(&three) > 0);
    }

    #[test]
    fn the_button_stays_in_while_checking() {
        let (idle, l) = frame(&view(1, false));
        let (busy, _) = frame(&view(1, true));
        let top = (l.button.x + l.button.w / 2, l.button.y);
        assert_eq!(pixel(&idle, top.0, top.1), DEFAULT.face_top_shadow);
        assert_eq!(pixel(&busy, top.0, top.1), DEFAULT.face_bottom_shadow);
    }

    #[test]
    fn the_clock_is_drawn_in_white() {
        let (pixels, l) = frame(&view(0, false));
        let band = (l.clock_baseline - l.clock_px as i32)..l.clock_baseline;
        let lit = band
            .flat_map(|y| (0..1920).map(move |x| (x, y)))
            .any(|(x, y)| pixel(&pixels, x, y) == TEXT);
        assert!(lit);
    }
}
