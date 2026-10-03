// SPDX-License-Identifier: GPL-3.0-or-later
//! The PAM check, on its own thread.
//!
//! PAM sleeps for a second or more after a wrong password, and longer still with `pam_faillock`
//! counting. Doing that on the thread that draws would freeze the lock screen mid-attempt -- the
//! caret would stop, the clock would stop, and the person at the keyboard would reasonably
//! conclude the machine had hung. So the check runs here, and the screen keeps painting.
//!
//! Replies come back over a `calloop` channel, which the Wayland event loop already waits on.
//! The same shape as `wlrix-greeter/src/worker.rs`, for the same reason.

use std::sync::mpsc;
use std::thread;

use calloop::channel::{Channel, Sender, channel};

use crate::pam::{self, Failure};
use crate::secret::Secret;

/// The PAM service the stack is read from: `/etc/pam.d/wlrix-lock`.
pub const SERVICE: &str = "wlrix-lock";

/// What the check said.
#[derive(Debug)]
pub enum Reply {
    /// A message from the PAM stack, to show while the check goes on.
    Notice(String),
    /// The password was right: unlock.
    Unlocked,
    /// It was not, or the check could not be made.
    Failed(Failure),
}

/// The drawing side's end of the worker.
pub struct Handle {
    requests: mpsc::Sender<Secret>,
}

impl Handle {
    /// Check `password`. The answer arrives on the reply channel.
    ///
    /// The secret moves to the worker and is cleared there when the attempt is done, so the
    /// password exists in exactly one place at a time.
    pub fn check(&self, password: Secret) -> bool {
        self.requests.send(password).is_ok()
    }
}

/// Start the worker for `user`.
pub fn spawn(user: String) -> Result<(Handle, Channel<Reply>), String> {
    let (requests, incoming) = mpsc::channel::<Secret>();
    let (replies, channel) = channel::<Reply>();
    thread::Builder::new()
        .name("pam".into())
        .spawn(move || run(&user, &incoming, &replies))
        .map_err(|err| format!("could not start the authentication thread: {err}"))?;
    Ok((Handle { requests }, channel))
}

/// The worker's life: one attempt per password sent, until the drawing side goes away.
fn run(user: &str, incoming: &mpsc::Receiver<Secret>, replies: &Sender<Reply>) {
    while let Ok(password) = incoming.recv() {
        let mut notice = |text: String| {
            let _ = replies.send(Reply::Notice(text));
        };
        let result = pam::authenticate(SERVICE, user, &password, &mut notice);
        // Cleared now rather than whenever the loop comes round again.
        drop(password);
        let reply = match result {
            Ok(()) => Reply::Unlocked,
            Err(failure) => Reply::Failed(failure),
        };
        if replies.send(reply).is_err() {
            return;
        }
    }
}
