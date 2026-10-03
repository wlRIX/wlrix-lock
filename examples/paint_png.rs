// SPDX-License-Identifier: GPL-3.0-or-later
//! Paint one lock-screen frame to a PNG, without a compositor and without locking anything.
//!
//! The drawing is most of what there is to look at, and looking at it through a real lock means
//! typing a password to get the screen back. This renders exactly what one output would show --
//! the same config lookup, wallpaper, blur, layout and widgets -- into a file that can be put
//! next to the reference picture.
//!
//!     cargo run --example paint_png -- out.png [width height [scale]] [--config lock.toml]
//!         [--typed N] [--message TEXT] [--caps]
//!
//! A development aid, not part of the locker.

use std::path::PathBuf;

use wlrix_lock::avatar::Avatar;
use wlrix_lock::config::Config;
use wlrix_lock::ui::draw::{self, View};
use wlrix_lock::ui::layout::Layout;
use wlrix_lock::user::User;
use wlrix_lock::{blur, clock};
use wlrix_ui::canvas::Canvas;
use wlrix_ui::text::Fonts;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut config_path = None;
    let mut typed = 0;
    let mut message = None;
    let mut caps_lock = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            "--typed" => typed = args.next().and_then(|n| n.parse().ok()).unwrap_or(0),
            "--message" => message = args.next(),
            "--caps" => caps_lock = true,
            _ => positional.push(arg),
        }
    }
    let out = positional
        .first()
        .ok_or("usage: paint_png out.png [w h [scale]]")?;
    let number = |i: usize, default: i32| {
        positional
            .get(i)
            .and_then(|n| n.parse().ok())
            .unwrap_or(default)
    };
    let (logical_w, logical_h, scale) = (number(1, 1920), number(2, 1080), number(3, 1).max(1));
    let (width, height) = (logical_w * scale, logical_h * scale);

    clock::init_locale();
    let config = Config::load(config_path.as_deref());
    let (palette, _) = wlrix_ui::palette::resolve(config.appearance.palette.as_deref());
    let user = User::current()?;
    let mut fonts = Fonts::load()?;
    let mut avatar = Avatar::load(&user.avatar_candidates());
    let mut pictures = wlrix_bg::decode::Pictures::new();

    let wallpaper = config.background.resolve(None);
    let picture = wallpaper.image.as_deref().and_then(|p| pictures.load(p));
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    wlrix_bg::render::render(&mut pixels, width, height, &wallpaper, picture);
    blur::blur(
        &mut pixels,
        width as usize,
        height as usize,
        (config.blur as i32 * scale) as usize,
    );

    let tm = clock::now();
    let time = clock::format(&tm, config.time_format());
    let date = match config.date_format() {
        Some(format) => clock::format(&tm, format),
        None => clock::format(&tm, clock::long_date_format()),
    };
    let view = View {
        time: &time,
        date: &date,
        name: &user.display_name,
        typed,
        focused: true,
        caret_on: true,
        checking: false,
        message: message.as_deref(),
        caps_lock,
    };
    let layout = Layout::compute(width, height, scale);
    let mut canvas = Canvas::new(&mut pixels, width, height);
    draw::paint(
        &mut canvas,
        &layout,
        palette,
        &mut fonts,
        &mut avatar,
        &view,
    );

    // Xrgb8888, native-endian, to RGB.
    let rgb: Vec<u8> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| {
            let v = u32::from_ne_bytes(*px);
            [(v >> 16) as u8, (v >> 8) as u8, v as u8]
        })
        .collect();
    image::RgbImage::from_raw(width as u32, height as u32, rgb)
        .ok_or("buffer size mismatch")?
        .save(out)
        .map_err(|err| format!("could not write {out}: {err}"))?;
    println!("wrote {out} ({width}x{height})");
    Ok(())
}
