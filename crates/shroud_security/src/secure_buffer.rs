use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// A byte buffer that guarantees zeroization of its contents on drop.
///
/// Used for raw sensitive data: encryption keys, tokens, binary secrets.
/// Like `SecureString`, access is closure-based to make exposure explicit.
///
/// Does NOT implement `Clone`, `Deref`, or `AsRef<[u8]>`.
pub struct SecureBuffer {
    inner: Vec<u8>,
}

impl Drop for SecureBuffer {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl ZeroizeOnDrop for SecureBuffer {}

impl SecureBuffer {
    /// Create a new `SecureBuffer` from a byte slice.
    pub fn new(data: &[u8]) -> Self {
        Self {
            inner: data.to_vec(),
        }
    }

    /// Create a buffer pre-filled with zeros.
    pub fn zeroed(len: usize) -> Self {
        Self {
            inner: vec![0u8; len],
        }
    }

    /// Create an empty buffer with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Vec::with_capacity(capacity),
        }
    }

    /// Access the buffer contents through a closure.
    pub fn expose<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.inner)
    }

    /// Mutably access the buffer contents through a closure.
    pub fn expose_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Vec<u8>) -> R,
    {
        f(&mut self.inner)
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Zeroize and clear the buffer.
    pub fn clear(&mut self) {
        self.inner.zeroize();
    }

    /// Zeroize and replace with new data.
    pub fn replace(&mut self, new_data: &[u8]) {
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
    fn expose_mut_access() {
        let mut buf = SecureBuffer::zeroed(4);
        buf.expose_mut(|data| {
            data[0] = 0xDE;
            data[1] = 0xAD;
        });
        buf.expose(|data| {
            assert_eq!(data[0], 0xDE);
            assert_eq!(data[1], 0xAD);
        });
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
        let mut buf = SecureBuffer::new(b"old_data");
        buf.replace(b"new_data");
        buf.expose(|data| assert_eq!(data, b"new_data"));
    }
}
