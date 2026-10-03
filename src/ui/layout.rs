// SPDX-License-Identifier: GPL-3.0-or-later
//! Where everything goes on one output.
//!
//! The arrangement is KDE's lock screen: the clock high up, the date under it, the round picture
//! in the middle of the screen with the name beneath, and the password field and its button under
//! that. Measured from the reference at 1920x1080 and written here in those pixels, then scaled by
//! the output's *height* and centered horizontally -- the greeter's rule, so a wide or a tall
//! monitor gets the same column rather than a stretched one.
//!
//! Everything is in buffer pixels: logical size times the output's integer scale. The caller
//! hit-tests in surface coordinates and multiplies by the scale first.

use wlrix_ui::canvas::Rect;

/// The height the reference was measured at.
const REFERENCE_HEIGHT: f32 = 1080.0;
/// How small and how large the column may get against the reference, so a 768-line laptop panel
/// still fits the field and a 2160-line monitor at scale 1 does not get a comically huge clock.
const MIN_FACTOR: f32 = 0.7;
const MAX_FACTOR: f32 = 2.0;

/// One output's layout, in buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Buffer pixels per reference pixel.
    pub unit: f32,
    pub center_x: i32,
    pub clock_baseline: i32,
    pub clock_px: f32,
    pub date_baseline: i32,
    pub date_px: f32,
    pub avatar_center_y: i32,
    pub avatar_diameter: i32,
    pub name_baseline: i32,
    pub name_px: f32,
    pub field: Rect,
    pub field_px: f32,
    pub button: Rect,
    pub message_baseline: i32,
    pub message_px: f32,
}

impl Layout {
    /// The layout for a buffer of `width` x `height` pixels at integer `scale`.
    pub fn compute(width: i32, height: i32, scale: i32) -> Self {
        let scale = scale.max(1);
        let logical_height = height as f32 / scale as f32;
        let factor = (logical_height / REFERENCE_HEIGHT).clamp(MIN_FACTOR, MAX_FACTOR);
        let unit = factor * scale as f32;
        let at = |reference: f32| (reference * unit).round() as i32;

        // The column is laid out around the vertical center rather than from the top, so a
        // screen taller than the reference keeps the picture in the middle, as KDE does.
        let middle = height / 2;
        let from_middle = |reference_y: f32| middle + at(reference_y - REFERENCE_HEIGHT / 2.0);

        let center_x = width / 2;
        // Field 248 wide, a 7 gap, a 33 square button: 288 in all, centered.
        let field_w = at(248.0);
        let field_h = at(33.0);
        let gap = at(7.0);
        let total = field_w + gap + field_h;
        let field_x = center_x - total / 2;
        let field_y = from_middle(602.0);

        let clock_baseline = from_middle(212.0);
        let date_baseline = from_middle(284.0);

        Self {
            unit,
            center_x,
            clock_baseline,
            clock_px: 104.0 * unit,
            date_baseline,
            date_px: 32.0 * unit,
            avatar_center_y: from_middle(467.0),
            avatar_diameter: at(124.0),
            name_baseline: from_middle(572.0),
            name_px: 21.0 * unit,
            field: Rect::new(field_x, field_y, field_w, field_h),
            field_px: 15.0 * unit,
            button: Rect::new(field_x + field_w + gap, field_y, field_h, field_h),
            message_baseline: from_middle(668.0),
            message_px: 15.0 * unit,
        }
    }

    /// Whether a buffer-pixel point is on the unlock button.
    pub fn on_button(&self, x: i32, y: i32) -> bool {
        self.button.contains(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_size_lands_where_the_reference_was_measured() {
        let l = Layout::compute(1920, 1080, 1);
        assert_eq!(l.unit, 1.0);
        assert_eq!(l.field, Rect::new(816, 602, 248, 33));
        assert_eq!(l.button, Rect::new(1071, 602, 33, 33));
        assert_eq!(l.avatar_center_y, 467);
        assert_eq!(l.avatar_diameter, 124);
    }

    #[test]
    fn scale_two_is_the_same_layout_at_twice_the_pixels() {
        let one = Layout::compute(1920, 1080, 1);
        let two = Layout::compute(3840, 2160, 2);
        assert_eq!(two.field.x, one.field.x * 2);
        assert_eq!(two.field.w, one.field.w * 2);
        assert_eq!(two.avatar_center_y, one.avatar_center_y * 2);
    }

    #[test]
    fn a_wide_screen_keeps_the_column_centered_and_unstretched() {
        let normal = Layout::compute(1920, 1080, 1);
        let wide = Layout::compute(3440, 1080, 1);
        assert_eq!(wide.field.w, normal.field.w);
        assert_eq!(wide.avatar_diameter, normal.avatar_diameter);
        // The field and button together are centered on the screen, as on the reference.
        let left_margin = wide.field.x;
        let right_margin = 3440 - wide.button.right();
        assert!((left_margin - right_margin).abs() <= 1);
    }

    #[test]
    fn a_small_panel_still_fits_everything_on_screen() {
        let l = Layout::compute(1366, 768, 1);
        assert!(l.clock_baseline - l.clock_px as i32 >= 0);
        assert!(l.message_baseline < 768);
        assert!(l.field.x >= 0 && l.button.right() <= 1366);
    }

    #[test]
    fn only_the_button_is_a_target() {
        let l = Layout::compute(1920, 1080, 1);
        assert!(l.on_button(1080, 610));
        assert!(!l.on_button(900, 610));
        assert!(!l.on_button(0, 0));
    }
}
