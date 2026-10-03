// SPDX-License-Identifier: GPL-3.0-or-later
//! Blurring the wallpaper behind the lock screen, when asked to.
//!
//! Three passes of a box blur in each direction, which is within a few percent of a Gaussian and
//! costs the same whatever the radius: each pass is a running sum, one add and one subtract per
//! pixel. It runs once per output per configure -- the wallpaper does not move -- so even at 4K
//! it is a one-off, not a per-frame cost.
//!
//! Edges clamp: the pixels beyond the border are taken to be copies of the border, so the edge of
//! the screen does not darken toward a black that is not there.

/// How many box passes approximate the Gaussian.
const PASSES: usize = 3;

/// Blur an `Xrgb8888` buffer of `width` x `height` in place, with a box of `radius` pixels.
///
/// A radius of zero, or a buffer too small to hold `width * height` pixels, is left alone.
pub fn blur(pixels: &mut [u8], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return;
    }
    let mut channels: Vec<[u32; 3]> = pixels[..width * height * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| {
            let value = u32::from_ne_bytes(*px);
            [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff]
        })
        .collect();

    let longest = width.max(height);
    let mut line = vec![[0u32; 3]; longest];
    for _ in 0..PASSES {
        for y in 0..height {
            let row = &mut channels[y * width..(y + 1) * width];
            line[..width].copy_from_slice(row);
            box_pass(&line[..width], row, radius);
        }
        let mut column = vec![[0u32; 3]; height];
        for x in 0..width {
            for y in 0..height {
                line[y] = channels[y * width + x];
            }
            box_pass(&line[..height], &mut column, radius);
            for y in 0..height {
                channels[y * width + x] = column[y];
            }
        }
    }

    for (px, [r, g, b]) in pixels[..width * height * 4]
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(channels)
    {
        *px = ((r << 16) | (g << 8) | b).to_ne_bytes();
    }
}

/// One box pass along a line: each output is the mean of the `2 * radius + 1` inputs around it.
fn box_pass(input: &[[u32; 3]], output: &mut [[u32; 3]], radius: usize) {
    let len = input.len();
    let at = |i: isize| input[i.clamp(0, len as isize - 1) as usize];
    let window = (2 * radius + 1) as u32;
    let r = radius as isize;

    let mut sum = [0u32; 3];
    for i in -r..=r {
        let px = at(i);
        for c in 0..3 {
            sum[c] += px[c];
        }
    }
    for (i, out) in output.iter_mut().enumerate().take(len) {
        for c in 0..3 {
            // Rounded rather than truncated, or every pass would shave the image a little darker.
            out[c] = (sum[c] + window / 2) / window;
        }
        let leaving = at(i as isize - r);
        let entering = at(i as isize + r + 1);
        for c in 0..3 {
            sum[c] = sum[c] + entering[c] - leaving[c];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    fn read(pixels: &[u8]) -> Vec<u32> {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| u32::from_ne_bytes(*px))
            .collect()
    }

    #[test]
    fn a_flat_color_is_unchanged() {
        // Clamped edges and rounded means: a solid screen must come out exactly as it went in,
        // or `blur` over the plain `#555555` default would visibly shift the gray.
        let mut pixels = buffer(&[0x0055_5555; 64]);
        blur(&mut pixels, 8, 8, 3);
        assert!(read(&pixels).iter().all(|&px| px == 0x0055_5555));
    }

    #[test]
    fn zero_radius_leaves_the_picture_alone() {
        let original: Vec<u32> = (0..16).map(|i| i * 0x0001_0101).collect();
        let mut pixels = buffer(&original);
        blur(&mut pixels, 4, 4, 0);
        assert_eq!(read(&pixels), original);
    }

    #[test]
    fn a_hard_edge_is_softened() {
        // Left half black, right half white.
        let row = [
            0,
            0,
            0,
            0,
            0x00ff_ffff,
            0x00ff_ffff,
            0x00ff_ffff,
            0x00ff_ffff,
        ];
        let mut pixels = buffer(&row.repeat(4));
        blur(&mut pixels, 8, 4, 1);
        let out = read(&pixels);
        let gray = |px: u32| px & 0xff;
        // Monotonic across the edge, with something in between.
        assert!(gray(out[3]) > 0 && gray(out[3]) < 0xff);
        assert!(gray(out[4]) > gray(out[3]));
        assert_eq!(gray(out[0]), out[0] >> 16 & 0xff, "channels stay together");
    }
}
