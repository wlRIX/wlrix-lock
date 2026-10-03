// SPDX-License-Identifier: GPL-3.0-or-later
//! The round account picture over the name.
//!
//! The picture is the user's own -- `~/.face.icon`, `~/.face` or AccountsService's copy; see
//! [`crate::user::User::avatar_candidates`] -- cropped to a circle with an antialiased edge and a
//! thin light ring around it. Without one, a drawn head-and-shoulders glyph stands in, which is
//! what the reference design shows for an account that never chose a picture.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use image::imageops::FilterType;
use wlrix_ui::Rgb;
use wlrix_ui::canvas::Canvas;

/// The largest account picture decoded, per axis. Account pictures are a few hundred pixels; a
/// header claiming more than this is refused before anything is allocated.
const MAX_DIMENSION: u32 = 4096;

/// The ring around the circle, and the strokes of the stand-in glyph.
const RING: Rgb = Rgb::from_channels(0xf0, 0xf0, 0xf0);
/// The stand-in glyph's disc: a dark translucent gray, so the wallpaper tints it.
const DISC: Rgb = Rgb::from_channels(0x31, 0x36, 0x3b);
const DISC_ALPHA: u8 = 0xd8;

/// The account picture, decoded once and rescaled when the size it is drawn at changes.
pub struct Avatar {
    source: Option<RgbaImage>,
    /// The picture cropped and scaled to a square of this many pixels.
    scaled: Option<(u32, RgbaImage)>,
}

impl Avatar {
    /// The first of `candidates` that decodes, or the stand-in if none does.
    pub fn load(candidates: &[PathBuf]) -> Self {
        let source = candidates.iter().find_map(|path| match decode(path) {
            Ok(picture) => {
                eprintln!("wlrix-lock: account picture {}", path.display());
                Some(picture)
            }
            Err(None) => None,
            Err(Some(why)) => {
                eprintln!("wlrix-lock: {why}");
                None
            }
        });
        Self {
            source,
            scaled: None,
        }
    }

    /// Draw the avatar as a circle of `diameter` pixels centered on `(cx, cy)`.
    pub fn draw(&mut self, canvas: &mut Canvas, cx: i32, cy: i32, diameter: i32) {
        if diameter <= 0 {
            return;
        }
        let radius = diameter as f32 / 2.0;
        let left = cx - diameter / 2;
        let top = cy - diameter / 2;
        // Pixel centers, relative to the circle's center.
        let offset = |i: i32, origin: i32, center: i32| (origin + i) as f32 + 0.5 - center as f32;

        match self.picture(diameter as u32) {
            Some(picture) => {
                for y in 0..diameter {
                    for x in 0..diameter {
                        let mask = circle(offset(x, left, cx), offset(y, top, cy), radius);
                        if mask == 0 {
                            continue;
                        }
                        let [r, g, b, a] = picture.get_pixel(x as u32, y as u32).0;
                        let coverage = (u32::from(a) * u32::from(mask) / 255) as u8;
                        canvas.blend(left + x, top + y, Rgb::from_channels(r, g, b), coverage);
                    }
                }
            }
            None => {
                for y in 0..diameter {
                    for x in 0..diameter {
                        let (dx, dy) = (offset(x, left, cx), offset(y, top, cy));
                        let disc = circle(dx, dy, radius);
                        let coverage = (u32::from(disc) * u32::from(DISC_ALPHA) / 255) as u8;
                        canvas.blend(left + x, top + y, DISC, coverage);
                        let glyph = person(dx / radius, dy / radius, radius).min(disc);
                        canvas.blend(left + x, top + y, RING, glyph);
                    }
                }
            }
        }

        // The ring, straddling the edge so it hides the antialiasing seam.
        let stroke = (diameter as f32 / 60.0).max(1.5);
        let extent = diameter / 2 + stroke.ceil() as i32 + 1;
        for y in -extent..=extent {
            for x in -extent..=extent {
                let (dx, dy) = (x as f32 + 0.5, y as f32 + 0.5);
                let coverage = ring(dx, dy, radius, stroke);
                canvas.blend(cx + x, cy + y, RING, coverage);
            }
        }
    }

    /// The picture scaled to cover a `size` square, cached for that size.
    fn picture(&mut self, size: u32) -> Option<&RgbaImage> {
        let source = self.source.as_ref()?;
        if self.scaled.as_ref().is_none_or(|(at, _)| *at != size) {
            self.scaled = Some((size, cover(source, size)));
        }
        self.scaled.as_ref().map(|(_, picture)| picture)
    }
}

/// Decode one candidate. `Err(None)` is a file that simply is not there, which is the ordinary
/// case for two of the three candidates and not worth a log line.
fn decode(path: &Path) -> Result<RgbaImage, Option<String>> {
    if !path.is_file() {
        return Err(None);
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|err| Some(format!("could not open {}: {err}", path.display())))?
        .with_guessed_format()
        .map_err(|err| Some(format!("could not read {}: {err}", path.display())))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    reader.limits(limits);
    reader
        .decode()
        .map(|picture| picture.to_rgba8())
        .map_err(|err| Some(format!("could not decode {}: {err}", path.display())))
}

/// `source` center-cropped to a square and scaled to `size`.
fn cover(source: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = source.dimensions();
    let side = w.min(h).max(1);
    let cropped = image::imageops::crop_imm(source, (w - side) / 2, (h - side) / 2, side, side);
    image::imageops::resize(&*cropped, size, size, FilterType::Lanczos3)
}

/// Coverage of the pixel at `(dx, dy)` from the center by a disc of `radius`, antialiased over
/// one pixel.
fn circle(dx: f32, dy: f32, radius: f32) -> u8 {
    let distance = (dx * dx + dy * dy).sqrt();
    to_coverage(radius - distance + 0.5)
}

/// Coverage by a ring of `stroke` width centered on `radius`.
fn ring(dx: f32, dy: f32, radius: f32, stroke: f32) -> u8 {
    let distance = (dx * dx + dy * dy).sqrt();
    to_coverage(stroke / 2.0 - (distance - radius).abs() + 0.5)
}

/// The stand-in glyph -- a head and a pair of shoulders, outlined -- at a point given in units of
/// the disc's radius. `radius` is the disc's size in pixels, to keep the stroke constant.
fn person(u: f32, v: f32, radius: f32) -> u8 {
    let stroke = 0.075;
    let px = 1.0 / radius;
    // Head: a ring above center.
    let head = {
        let d = (u * u + (v + 0.28) * (v + 0.28)).sqrt();
        stroke / 2.0 - (d - 0.27).abs()
    };
    // Shoulders: the upper half of a wide ring below center, clipped by the disc itself.
    let shoulders = if v >= 0.28 - stroke {
        let d = (u * u + (v - 0.98) * (v - 0.98)).sqrt();
        stroke / 2.0 - (d - 0.62).abs()
    } else {
        f32::MIN
    };
    to_coverage(head.max(shoulders) / px + 0.5)
}

fn to_coverage(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_circle_is_solid_inside_and_empty_outside() {
        assert_eq!(circle(0.0, 0.0, 10.0), 255);
        assert_eq!(circle(20.0, 0.0, 10.0), 0);
        // Exactly on the edge is half covered: that is the antialiasing.
        let edge = circle(10.0, 0.0, 10.0);
        assert!((120..=135).contains(&edge), "{edge}");
    }

    #[test]
    fn cover_crops_to_the_middle_square() {
        // A 4x2 picture, left half red and right half blue, with a green middle: the square
        // crop keeps the middle two columns.
        let mut source = RgbaImage::new(4, 2);
        for y in 0..2 {
            source.put_pixel(0, y, image::Rgba([255, 0, 0, 255]));
            source.put_pixel(1, y, image::Rgba([0, 255, 0, 255]));
            source.put_pixel(2, y, image::Rgba([0, 255, 0, 255]));
            source.put_pixel(3, y, image::Rgba([0, 0, 255, 255]));
        }
        let square = cover(&source, 2);
        assert_eq!(square.dimensions(), (2, 2));
        assert!(square.pixels().all(|px| px.0[1] > 200 && px.0[0] < 50));
    }

    #[test]
    fn a_missing_file_is_quiet_and_not_an_error() {
        assert_eq!(decode(Path::new("/nonexistent/.face")), Err(None));
    }

    #[test]
    fn the_stand_in_draws_something_inside_the_disc() {
        let mut pixels = vec![0u8; 64 * 64 * 4];
        let mut canvas = Canvas::new(&mut pixels, 64, 64);
        let mut avatar = Avatar {
            source: None,
            scaled: None,
        };
        avatar.draw(&mut canvas, 32, 32, 60);
        // The head ring crosses the vertical center line above the middle.
        let lit = (0..32).any(|y| canvas.get(32, y) == RING);
        assert!(lit, "the head should be drawn in the ring color somewhere");
        // A corner is outside the circle and its ring.
        assert_eq!(canvas.get(0, 0), Rgb(0));
    }
}
