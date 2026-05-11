use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// A byte buffer that guarantees zeroization of its contents on drop.
///
/// Used for raw sensitive data: encryption keys, tokens, binary secrets.
/// Like `SecureString`, access is closure-based to make exposure explicit.
///
/// Does NOT implement `Clone`, `Deref`, or `AsRef<[u8]>`.
///
/// # Capacity is fixed at construction
///
/// To prevent realloc residue (a freed heap buffer keeping the previous
/// secret bytes after `Vec` amortized growth), the buffer is sized at
/// construction and never grows. Any mutator that would exceed `capacity`
/// panics. [`expose_bytes_mut`](Self::expose_bytes_mut) yields a
/// fixed-length `&mut [u8]` so in-place edits (decryption, RNG fill) stay
/// safe without exposing growth APIs.
pub struct SecureBuffer {
    inner: Vec<u8>,
    capacity: usize,
}

impl Drop for SecureBuffer {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl ZeroizeOnDrop for SecureBuffer {}

impl SecureBuffer {
    /// Create a new `SecureBuffer` from a byte slice.
    /// The buffer is sized exactly to fit `data` and cannot grow.
    pub fn new(data: &[u8]) -> Self {
        let capacity = data.len();
        let mut inner = Vec::with_capacity(capacity);
        inner.extend_from_slice(data);
        Self { inner, capacity }
    }

    /// Create a buffer pre-filled with zeros. `len` becomes both the
    /// initial length and the maximum capacity.
    pub fn zeroed(len: usize) -> Self {
        Self {
            inner: vec![0u8; len],
            capacity: len,
        }
    }

    /// Create an empty buffer with the given capacity in bytes.
    ///
    /// The buffer never grows beyond `capacity`; pushing past it panics.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// Access the buffer contents through a closure.
    pub fn expose<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.inner)
    }

    /// Mutably access the buffer contents as a fixed-length slice.
    ///
    /// The slice covers the current `len()` bytes; callers can edit
    /// individual bytes in place (e.g. for decryption-in-place or RNG
    /// fill) but cannot grow or reallocate the buffer.
    pub fn expose_bytes_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.inner)
    }

    /// Append a byte.
    ///
    /// # Panics
    /// If `self.len() == self.capacity()`.
    pub fn push(&mut self, byte: u8) {
        assert!(
            self.inner.len() < self.capacity,
            "SecureBuffer::push would exceed capacity ({})",
            self.capacity
        );
        self.inner.push(byte);
    }

    /// Append a byte slice.
    ///
    /// # Panics
    /// If `self.len() + data.len() > self.capacity()`.
    pub fn push_slice(&mut self, data: &[u8]) {
        let needed = self.inner.len() + data.len();
        assert!(
            needed <= self.capacity,
            "SecureBuffer::push_slice would exceed capacity ({} > {})",
            needed,
            self.capacity
        );
        self.inner.extend_from_slice(data);
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Maximum byte length this buffer can hold. Fixed at construction.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Zeroize and clear the buffer. Capacity is preserved.
    pub fn clear(&mut self) {
        self.inner.zeroize();
    }

    /// Zeroize and replace with new data.
    ///
    /// # Panics
    /// If `new_data.len() > self.capacity()`.
    pub fn replace(&mut self, new_data: &[u8]) {
        assert!(
            new_data.len() <= self.capacity,
            "SecureBuffer::replace would exceed capacity ({} > {})",
            new_data.len(),
            self.capacity
        );
        self.inner.zeroize();
        self.inner.extend_from_slice(new_data);
    }
}

impl Zeroize for SecureBuffer {
    fn zeroize(&mut self) {
        self.inner.zeroize();
    }
}

impl fmt::Debug for SecureBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecureBuffer([REDACTED; {} bytes])", self.inner.len())
    }
}

impl PartialEq for SecureBuffer {
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq;
        self.inner.ct_eq(&other.inner).into()
    }
}

impl Eq for SecureBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_operations() {
        let buf = SecureBuffer::new(b"secret_key_12345");
        assert_eq!(buf.len(), 16);
        assert!(!buf.is_empty());
        buf.expose(|data| assert_eq!(data, b"secret_key_12345"));
    }

    #[test]
    fn zeroed_buffer() {
        let buf = SecureBuffer::zeroed(32);
        assert_eq!(buf.len(), 32);
        buf.expose(|data| assert!(data.iter().all(|&b| b == 0)));
    }

    #[test]
    fn expose_bytes_mut_in_place_edit() {
        let mut buf = SecureBuffer::zeroed(4);
        buf.expose_bytes_mut(|data| {
            data[0] = 0xDE;
            data[1] = 0xAD;
        });
        buf.expose(|data| {
            assert_eq!(data[0], 0xDE);
            assert_eq!(data[1], 0xAD);
        });
    }

    #[test]
    fn push_within_capacity() {
        let mut buf = SecureBuffer::with_capacity(4);
        buf.push(1);
        buf.push(2);
        buf.push_slice(&[3, 4]);
        buf.expose(|data| assert_eq!(data, &[1, 2, 3, 4]));
    }

    #[test]
    #[should_panic(expected = "SecureBuffer::push would exceed capacity")]
    fn push_panics_on_overflow() {
        let mut buf = SecureBuffer::with_capacity(2);
        buf.push(1);
        buf.push(2);
        buf.push(3);
    }

    #[test]
    #[should_panic(expected = "SecureBuffer::push_slice would exceed capacity")]
    fn push_slice_panics_on_overflow() {
        let mut buf = SecureBuffer::with_capacity(2);
        buf.push_slice(&[1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "SecureBuffer::replace would exceed capacity")]
    fn replace_panics_on_overflow() {
        let mut buf = SecureBuffer::with_capacity(2);
        buf.replace(&[1, 2, 3]);
    }

    #[test]
    fn debug_is_redacted() {
        let buf = SecureBuffer::new(b"top_secret");
        let debug = format!("{:?}", buf);
        assert!(debug.contains("REDACTED"));
        assert!(debug.contains("10 bytes"));
        assert!(!debug.contains("top_secret"));
    }

    #[test]
    fn equality() {
        let a = SecureBuffer::new(b"same");
        let b = SecureBuffer::new(b"same");
        let c = SecureBuffer::new(b"diff");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn replace_clears_old() {
        let mut buf = SecureBuffer::with_capacity(16);
        buf.push_slice(b"old_data");
        buf.replace(b"new_data");
        buf.expose(|data| assert_eq!(data, b"new_data"));
    }

    #[test]
    fn capacity_invariant_no_realloc() {
        let mut buf = SecureBuffer::with_capacity(32);
        let initial_ptr = buf.expose(|d| d.as_ptr());
        buf.push_slice(b"hello world");
        buf.push(b'!');
        buf.replace(b"new_value");
        assert_eq!(buf.expose(|d| d.as_ptr()), initial_ptr);
    }

    #[test]
    fn clear_preserves_capacity() {
        let mut buf = SecureBuffer::with_capacity(16);
        buf.push_slice(b"hello");
        buf.clear();
        assert_eq!(buf.capacity(), 16);
        buf.push_slice(b"world");
        buf.expose(|d| assert_eq!(d, b"world"));
    }
}
