// SPDX-License-Identifier: GPL-3.0-or-later
//! The wlRIX screen locker, as a library.
//!
//! The binary (`main.rs`) is a thin shell over this, so the config, the clock, the passwd lookup,
//! the avatar mask, the blur, the layout and the PAM conversation can be exercised by tests
//! without a compositor or a PAM stack. Only [`ui`] needs a Wayland connection, and only
//! [`pam::authenticate`] a real PAM service.

pub mod auth;
pub mod avatar;
pub mod blur;
pub mod clock;
pub mod config;
pub mod pam;
pub mod secret;
pub mod ui;
pub mod user;
pub mod xdg;
