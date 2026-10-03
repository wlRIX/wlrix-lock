// SPDX-License-Identifier: GPL-3.0-or-later
//! The password as it is typed.
//!
//! A `String` would leave copies of the password all over the heap: every push past capacity
//! moves the bytes and frees the old block without clearing it, and a backspace only moves the
//! length. Neither is a vulnerability by itself -- this process's memory belongs to the user it
//! protects -- but a core dump, a swapped page or a later bug that reads freed memory is exactly
//! where a password should not turn up. So this buffer grows by hand, clears what it leaves
//! behind, and clears itself when dropped.
//!
//! No `Debug` derive, no `Display`, no `Clone`: a password cannot reach a log line by accident.

/// Room for any password a person types, so the buffer never has to move in ordinary use.
const INITIAL_CAPACITY: usize = 256;

/// A password being typed, or being handed to PAM.
pub struct Secret {
    bytes: Vec<u8>,
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(INITIAL_CAPACITY),
        }
    }

    /// Append typed text.
    pub fn push_str(&mut self, text: &str) {
        let needed = self.bytes.len() + text.len();
        if needed > self.bytes.capacity() {
            // Move by hand, so the old block is cleared before it is freed rather than after
            // somebody else has been handed it.
            let mut grown = Vec::with_capacity(needed.max(self.bytes.capacity() * 2));
            grown.extend_from_slice(&self.bytes);
            wipe(&mut self.bytes);
            self.bytes = grown;
        }
        self.bytes.extend_from_slice(text.as_bytes());
    }

    /// Remove the last character, clearing its bytes.
    pub fn pop(&mut self) {
        let Ok(text) = std::str::from_utf8(&self.bytes) else {
            // Only ever filled from `&str`, so this cannot happen; clear everything if it does.
            self.clear();
            return;
        };
        let Some((start, _)) = text.char_indices().next_back() else {
            return;
        };
        for byte in &mut self.bytes[start..] {
            // SAFETY: a valid, aligned, exclusive reference to a byte.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        self.bytes.truncate(start);
    }

    /// Forget the password, clearing its bytes.
    pub fn clear(&mut self) {
        wipe(&mut self.bytes);
        self.bytes.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// How many characters, for drawing that many bullets. Never logged.
    pub fn chars(&self) -> usize {
        std::str::from_utf8(&self.bytes)
            .map(|text| text.chars().count())
            .unwrap_or(0)
    }

    /// The bytes, for the PAM conversation to copy out.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Hand the password over and leave this one empty, without copying it.
    pub fn take(&mut self) -> Secret {
        std::mem::take(self)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.bytes);
    }
}

/// Zero every byte of `bytes`, including the spare capacity beyond its length.
///
/// Volatile writes followed by a compiler fence, so the stores cannot be dropped as dead because
/// the memory is about to be freed -- which is precisely what an optimizer is allowed to assume
/// about an ordinary store before a `free`.
fn wipe(bytes: &mut Vec<u8>) {
    let capacity = bytes.capacity();
    let base = bytes.as_mut_ptr();
    for i in 0..capacity {
        // SAFETY: `i < capacity`, so the pointer is within the allocation; spare capacity is
        // allocated memory and writing it is fine even though it is not part of the slice.
        unsafe { std::ptr::write_volatile(base.add(i), 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Zero a C string PAM handed back or that we built, up to its terminator.
///
/// # Safety
///
/// `ptr` must be null or point at a writable NUL-terminated string.
pub unsafe fn wipe_c_string(ptr: *mut libc::c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: per the contract, the string is writable and terminated.
    unsafe {
        let len = libc::strlen(ptr);
        for i in 0..len {
            std::ptr::write_volatile(ptr.add(i), 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_backspacing_work_by_character() {
        let mut secret = Secret::new();
        secret.push_str("pa");
        secret.push_str("ß");
        secret.push_str("語");
        assert_eq!(secret.chars(), 4);
        secret.pop();
        assert_eq!(secret.as_bytes(), "paß".as_bytes());
        secret.pop();
        secret.pop();
        secret.pop();
        assert!(secret.is_empty());
        // Backspace on an empty field is a no-op, not a panic.
        secret.pop();
        assert!(secret.is_empty());
    }

    #[test]
    fn a_backspace_clears_the_bytes_it_removes() {
        let mut secret = Secret::new();
        secret.push_str("ab語");
        let base = secret.bytes.as_ptr();
        secret.pop();
        // The three bytes of 語 are still allocated, and must now be zero.
        // SAFETY: within capacity; the buffer has not moved.
        let tail = unsafe { std::slice::from_raw_parts(base.add(2), 3) };
        assert_eq!(tail, &[0, 0, 0]);
    }

    #[test]
    fn growing_past_capacity_keeps_the_contents() {
        let mut secret = Secret::new();
        let long = "x".repeat(INITIAL_CAPACITY + 10);
        secret.push_str("ab");
        secret.push_str(&long);
        assert_eq!(secret.chars(), INITIAL_CAPACITY + 12);
        assert!(secret.as_bytes().starts_with(b"abxx"));
    }

    #[test]
    fn take_leaves_an_empty_field() {
        let mut secret = Secret::new();
        secret.push_str("hunter2");
        let taken = secret.take();
        assert!(secret.is_empty());
        assert_eq!(taken.as_bytes(), b"hunter2");
    }
}
