// SPDX-License-Identifier: GPL-3.0-or-later
//! Checking a password with PAM.
//!
//! Hand-declared rather than through a crate -- see `Cargo.toml` for why. It is five functions
//! and one callback, and the callback is the part that matters: it is where the password leaves
//! this program, so it is written here, where it can be read.
//!
//! Only `auth` and `account`. A locker re-proves who is at the keyboard; it opens no session and
//! changes no credentials, so `pam_open_session` and friends have no business here. The one
//! courtesy extended is `PAM_REFRESH_CRED` afterwards, which renews a Kerberos ticket that
//! expired while the screen was locked -- what `kscreenlocker` does, and failure is ignored.
//!
//! Runs unprivileged. `pam_unix` checks the caller's own password through its setuid helper
//! `unix_chkpwd`, which is how every unprivileged locker works.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use crate::secret::{Secret, wipe_c_string};

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_CONV_ERR: c_int = 19;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

const PAM_REFRESH_CRED: c_int = 0x0010;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: ConvFn,
    appdata_ptr: *mut c_void,
}

/// An opaque `pam_handle_t`.
#[repr(C)]
struct PamHandle {
    _private: [u8; 0],
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_setcred(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut PamHandle, pam_status: c_int) -> c_int;
    fn pam_strerror(pamh: *mut PamHandle, errnum: c_int) -> *const c_char;
}

/// Why an attempt did not unlock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The password was wrong. Deliberately carries no detail.
    Denied,
    /// Anything else, with PAM's own description: an expired account, a broken stack, a missing
    /// service file. Worth showing, because the person at the keyboard cannot fix it by typing
    /// more carefully and needs to know that.
    Error(String),
}

/// What the conversation callback needs: the answer, and somewhere to put PAM's notices.
struct Conversation<'a> {
    password: &'a Secret,
    notice: &'a mut dyn FnMut(String),
}

/// Check `password` for `user` against the PAM service `service`.
///
/// `notice` receives each informational or error message the stack sends while it works --
/// `pam_faillock`'s "the account is locked" is the one that matters -- so it can be shown.
/// Blocks, for a second or more after a wrong password; call it from a worker thread.
pub fn authenticate(
    service: &str,
    user: &str,
    password: &Secret,
    notice: &mut dyn FnMut(String),
) -> Result<(), Failure> {
    let service = CString::new(service).map_err(|_| Failure::Error("bad service name".into()))?;
    let user = CString::new(user).map_err(|_| Failure::Error("bad user name".into()))?;

    let mut conversation = Conversation { password, notice };
    let conv = PamConv {
        conv: converse,
        appdata_ptr: (&mut conversation as *mut Conversation).cast(),
    };

    let mut handle: *mut PamHandle = std::ptr::null_mut();
    // SAFETY: the strings and `conv` outlive the handle, which is ended before this returns;
    // `conversation` is only reached through `appdata_ptr` while PAM calls are in progress.
    let status = unsafe { pam_start(service.as_ptr(), user.as_ptr(), &conv, &mut handle) };
    if status != PAM_SUCCESS || handle.is_null() {
        return Err(Failure::Error(describe(std::ptr::null_mut(), status)));
    }

    // SAFETY: `handle` is a live handle from `pam_start`, ended exactly once below.
    unsafe {
        let mut status = pam_authenticate(handle, 0);
        if status == PAM_SUCCESS {
            status = pam_acct_mgmt(handle, 0);
        }
        let result = match status {
            PAM_SUCCESS => {
                pam_setcred(handle, PAM_REFRESH_CRED);
                Ok(())
            }
            PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_MAXTRIES | PAM_PERM_DENIED => {
                Err(Failure::Denied)
            }
            other => Err(Failure::Error(describe(handle, other))),
        };
        pam_end(handle, status);
        result
    }
}

/// PAM's description of a status.
fn describe(handle: *mut PamHandle, status: c_int) -> String {
    // SAFETY: `pam_strerror` accepts any handle, including null, and returns a static string.
    let text = unsafe { pam_strerror(handle, status) };
    if text.is_null() {
        return format!("PAM error {status}");
    }
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// The conversation function PAM calls back into.
///
/// Answers every prompt with the password -- echoed or not, since a stack asking for an echoed
/// answer during a re-authentication is asking for a token, and the field is the only answer
/// there is -- and hands every message to [`Conversation::notice`].
///
/// The response array and each answer are allocated with the C allocator, because PAM frees them
/// with `free`. On any failure everything allocated so far is cleared and freed here and
/// `PAM_CONV_ERR` returned, so no copy of the password is left for nobody to free.
extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
        return PAM_CONV_ERR;
    }
    let count = num_msg as usize;
    // SAFETY: `appdata_ptr` is the `Conversation` `authenticate` set, alive for the whole call.
    let conversation = unsafe { &mut *(appdata_ptr as *mut Conversation) };

    // SAFETY: calloc of `count` responses; null is checked.
    let responses =
        unsafe { libc::calloc(count, std::mem::size_of::<PamResponse>()) } as *mut PamResponse;
    if responses.is_null() {
        return PAM_BUF_ERR;
    }

    for i in 0..count {
        // Linux-PAM passes an array of pointers to messages, which is what `msg[i]` reads.
        // SAFETY: PAM promises `num_msg` entries.
        let message = unsafe { *msg.add(i) };
        if message.is_null() {
            // SAFETY: `responses` holds `count` entries, any filled so far by us.
            unsafe { free_responses(responses, count) };
            return PAM_CONV_ERR;
        }
        // SAFETY: a non-null message from PAM.
        let (style, text) = unsafe { ((*message).msg_style, (*message).msg) };
        match style {
            PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                let Some(answer) = c_copy(conversation.password.as_bytes()) else {
                    // SAFETY: as above.
                    unsafe { free_responses(responses, count) };
                    return PAM_CONV_ERR;
                };
                // SAFETY: `i < count`.
                unsafe { (*responses.add(i)).resp = answer };
            }
            PAM_ERROR_MSG | PAM_TEXT_INFO => {
                if !text.is_null() {
                    // SAFETY: PAM's message text is NUL-terminated.
                    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
                    let text = text.trim();
                    if !text.is_empty() {
                        (conversation.notice)(text.to_string());
                    }
                }
            }
            _ => {
                // SAFETY: as above.
                unsafe { free_responses(responses, count) };
                return PAM_CONV_ERR;
            }
        }
    }

    // SAFETY: PAM gave us somewhere to put the array, and now owns it.
    unsafe { *resp = responses };
    PAM_SUCCESS
}

/// The password as a `malloc`ed C string, or `None` if it cannot be one.
fn c_copy(bytes: &[u8]) -> Option<*mut c_char> {
    // A NUL inside would silently truncate the password PAM sees. Typed text cannot contain one,
    // but refusing is cheaper than reasoning about what a truncated password would match.
    if bytes.contains(&0) {
        return None;
    }
    // SAFETY: allocate len+1 bytes, copy, terminate; null is checked.
    unsafe {
        let ptr = libc::malloc(bytes.len() + 1) as *mut u8;
        if ptr.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        *ptr.add(bytes.len()) = 0;
        Some(ptr.cast())
    }
}

/// Clear and free a response array of `count` entries and every answer in it.
///
/// # Safety
///
/// `responses` must come from `calloc` with `count` entries, each `resp` null or `malloc`ed.
unsafe fn free_responses(responses: *mut PamResponse, count: usize) {
    for i in 0..count {
        // SAFETY: per the contract.
        unsafe {
            let answer = (*responses.add(i)).resp;
            wipe_c_string(answer);
            libc::free(answer.cast());
        }
    }
    // SAFETY: per the contract.
    unsafe { libc::free(responses.cast()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the conversation function over `messages` the way PAM would.
    fn run(
        messages: &[(c_int, &str)],
        password: &str,
    ) -> (c_int, Vec<Option<String>>, Vec<String>) {
        let texts: Vec<CString> = messages
            .iter()
            .map(|(_, text)| CString::new(*text).unwrap())
            .collect();
        let structs: Vec<PamMessage> = messages
            .iter()
            .zip(&texts)
            .map(|((style, _), text)| PamMessage {
                msg_style: *style,
                msg: text.as_ptr(),
            })
            .collect();
        let mut pointers: Vec<*const PamMessage> =
            structs.iter().map(|m| m as *const PamMessage).collect();

        let mut secret = Secret::new();
        secret.push_str(password);
        let mut notices = Vec::new();
        let mut push = |text: String| notices.push(text);
        let mut conversation = Conversation {
            password: &secret,
            notice: &mut push,
        };
        let mut resp: *mut PamResponse = std::ptr::null_mut();
        let status = converse(
            messages.len() as c_int,
            pointers.as_mut_ptr(),
            &mut resp,
            (&mut conversation as *mut Conversation).cast(),
        );
        let mut answers = Vec::new();
        if status == PAM_SUCCESS {
            for i in 0..messages.len() {
                // SAFETY: the array PAM would now own; freed here in its place.
                let answer = unsafe { (*resp.add(i)).resp };
                answers.push(if answer.is_null() {
                    None
                } else {
                    Some(
                        unsafe { CStr::from_ptr(answer) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                });
            }
            unsafe { free_responses(resp, messages.len()) };
        }
        (status, answers, notices)
    }

    #[test]
    fn a_password_prompt_is_answered_with_the_password() {
        let (status, answers, notices) = run(&[(PAM_PROMPT_ECHO_OFF, "Password: ")], "hunter2");
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(answers, vec![Some("hunter2".to_string())]);
        assert!(notices.is_empty());
    }

    #[test]
    fn messages_are_passed_on_and_not_answered() {
        let (status, answers, notices) = run(
            &[
                (
                    PAM_TEXT_INFO,
                    "The account is locked due to 3 failed logins.",
                ),
                (PAM_PROMPT_ECHO_OFF, "Password: "),
            ],
            "pw",
        );
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(answers, vec![None, Some("pw".to_string())]);
        assert_eq!(
            notices,
            vec!["The account is locked due to 3 failed logins.".to_string()]
        );
    }

    #[test]
    fn an_unknown_message_style_fails_the_conversation() {
        let (status, _, _) = run(&[(PAM_PROMPT_ECHO_OFF, "Password: "), (99, "?")], "pw");
        assert_eq!(status, PAM_CONV_ERR);
    }
}
