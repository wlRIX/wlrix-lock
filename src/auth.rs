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
//!
//! # The fingerprint
//!
//! A second thread runs the `wlrix-lock-fingerprint` stack over and over, beside the password
//! one, so a finger on the reader unlocks without anyone having to ask for it first -- KDE's
//! arrangement. pam_fprintd gives up after its timeout or a few mismatches, and loses the reader
//! across a suspend, so a finished attempt is simply started again. One that fails at once
//! (no reader, nothing enrolled, no fprintd) waits a while longer each time before the next, so
//! a machine without a reader is asked every few minutes rather than in a loop.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use calloop::channel::{Channel, Sender, channel};

use crate::pam::{self, Failure};
use crate::secret::Secret;

/// The PAM service the stack is read from: `/etc/pam.d/wlrix-lock`.
pub const SERVICE: &str = "wlrix-lock";

/// The fingerprint stack, run beside [`SERVICE`]: `/etc/pam.d/wlrix-lock-fingerprint`.
pub const FINGER_SERVICE: &str = "wlrix-lock-fingerprint";

/// An attempt that lasted at least this long was a real one -- a timeout or mismatches -- and is
/// started again straight away.
const REAL_ATTEMPT: Duration = Duration::from_secs(5);
/// The pause after one that failed at once, doubled each time up to the longest.
const FIRST_PAUSE: Duration = Duration::from_secs(10);
const LONGEST_PAUSE: Duration = Duration::from_secs(300);
/// Between two real attempts: long enough to read a "no match".
const BETWEEN_ATTEMPTS: Duration = Duration::from_secs(1);

/// What the check said.
#[derive(Debug)]
pub enum Reply {
    /// A message from the PAM stack, to show while the check goes on.
    Notice(String),
    /// The password was right: unlock.
    Unlocked,
    /// It was not, or the check could not be made.
    Failed(Failure),
    /// What the fingerprint stack says -- "Place your finger...", "no match" -- or `None` when
    /// it has stopped listening for now.
    Finger(Option<String>),
}

/// The drawing side's end of the worker.
pub struct Handle {
    requests: mpsc::Sender<Secret>,
    replies: Sender<Reply>,
    user: String,
    finger_started: bool,
}

impl Handle {
    /// Check `password`. The answer arrives on the reply channel.
    ///
    /// The secret moves to the worker and is cleared there when the attempt is done, so the
    /// password exists in exactly one place at a time.
    pub fn check(&self, password: Secret) -> bool {
        self.requests.send(password).is_ok()
    }

    /// Start listening for a finger, once; see the module docs. Does nothing when the machine
    /// has no fingerprint stack installed, or when its ordinary stack already asks for a finger
    /// itself -- then two conversations would fight over the reader.
    pub fn start_fingerprint(&mut self) {
        if self.finger_started {
            return;
        }
        self.finger_started = true;
        if !pam_service_exists(FINGER_SERVICE) {
            return;
        }
        if base_stack_has_fprintd() {
            eprintln!(
                "wlrix-lock: the system's auth stack already asks for a fingerprint; \
                 not asking a second time"
            );
            return;
        }
        let user = self.user.clone();
        let replies = self.replies.clone();
        let spawned = thread::Builder::new()
            .name("fingerprint".into())
            .spawn(move || listen_for_finger(&user, &replies));
        if let Err(err) = spawned {
            eprintln!("wlrix-lock: could not start the fingerprint thread: {err}");
        }
    }
}

/// Start the worker for `user`.
pub fn spawn(user: String) -> Result<(Handle, Channel<Reply>), String> {
    let (requests, incoming) = mpsc::channel::<Secret>();
    let (replies, channel) = channel::<Reply>();
    thread::Builder::new()
        .name("pam".into())
        .spawn({
            let user = user.clone();
            let replies = replies.clone();
            move || run(&user, &incoming, &replies)
        })
        .map_err(|err| format!("could not start the authentication thread: {err}"))?;
    Ok((
        Handle {
            requests,
            replies,
            user,
            finger_started: false,
        },
        channel,
    ))
}

/// The worker's life: one attempt per password sent, until the drawing side goes away.
fn run(user: &str, incoming: &mpsc::Receiver<Secret>, replies: &Sender<Reply>) {
    while let Ok(password) = incoming.recv() {
        let mut notice = |text: String| {
            let _ = replies.send(Reply::Notice(text));
        };
        let result = pam::authenticate(SERVICE, user, Some(&password), &mut notice);
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

/// The fingerprint thread's life: attempt after attempt until one succeeds or the drawing side
/// goes away.
fn listen_for_finger(user: &str, replies: &Sender<Reply>) {
    let mut pause = FIRST_PAUSE;
    loop {
        let started = Instant::now();
        let mut notice = |text: String| {
            let _ = replies.send(Reply::Finger(Some(text)));
        };
        let result = pam::authenticate(FINGER_SERVICE, user, None, &mut notice);
        let wait = match result {
            Ok(()) => {
                let _ = replies.send(Reply::Unlocked);
                return;
            }
            Err(failure) => {
                if let Failure::Error(why) = &failure {
                    eprintln!("wlrix-lock: fingerprint: {why}");
                }
                next_pause(started.elapsed(), &mut pause)
            }
        };
        // A long pause is a quiet one: whatever the reader last said no longer applies.
        if wait > BETWEEN_ATTEMPTS && replies.send(Reply::Finger(None)).is_err() {
            return;
        }
        thread::sleep(wait);
    }
}

/// How long to wait before the next fingerprint attempt, given how long the last one took, and
/// the backoff for attempts that fail at once.
fn next_pause(took: Duration, pause: &mut Duration) -> Duration {
    if took >= REAL_ATTEMPT {
        *pause = FIRST_PAUSE;
        return BETWEEN_ATTEMPTS;
    }
    let wait = *pause;
    *pause = (*pause * 2).min(LONGEST_PAUSE);
    wait
}

/// Whether PAM has a stack by this name, in `/etc/pam.d` or the vendor directory. Without one PAM
/// falls back to `other`, which is never what is meant.
fn pam_service_exists(name: &str) -> bool {
    ["/etc/pam.d", "/usr/lib/pam.d"]
        .iter()
        .any(|dir| Path::new(dir).join(name).is_file())
}

/// Whether the shared auth stack the password check includes already runs pam_fprintd -- Debian's
/// `pam-auth-update` puts it in `common-auth` when libpam-fprintd is installed.
fn base_stack_has_fprintd() -> bool {
    ["system-auth", "common-auth", SERVICE].iter().any(|name| {
        ["/etc/pam.d", "/usr/lib/pam.d"].iter().any(|dir| {
            std::fs::read_to_string(Path::new(dir).join(name))
                .is_ok_and(|text| mentions_fprintd(&text))
        })
    })
}

/// Whether a PAM file has a line, not commented out, that loads pam_fprintd.
fn mentions_fprintd(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default();
        line.contains("pam_fprintd")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_attempts_follow_each_other_and_quick_failures_back_off() {
        let mut pause = FIRST_PAUSE;
        assert_eq!(
            next_pause(Duration::from_secs(30), &mut pause),
            BETWEEN_ATTEMPTS
        );
        let waits: Vec<_> = (0..8)
            .map(|_| next_pause(Duration::from_millis(50), &mut pause))
            .collect();
        assert_eq!(waits[0], FIRST_PAUSE);
        assert_eq!(waits[1], FIRST_PAUSE * 2);
        assert_eq!(*waits.last().unwrap(), LONGEST_PAUSE);
        // A real attempt again resets the backoff.
        next_pause(Duration::from_secs(30), &mut pause);
        assert_eq!(next_pause(Duration::ZERO, &mut pause), FIRST_PAUSE);
    }

    #[test]
    fn a_commented_out_fprintd_line_does_not_count() {
        assert!(mentions_fprintd("auth sufficient pam_fprintd.so\nauth include system-auth\n"));
        assert!(!mentions_fprintd("#auth sufficient pam_fprintd.so\nauth include system-auth\n"));
        assert!(!mentions_fprintd("auth include system-auth # not pam_fprintd\n"));
    }
}
