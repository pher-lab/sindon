use subtle::ConstantTimeEq;

/// Compare two byte slices in constant time.
/// Returns true if they are equal, false otherwise.
/// Execution time does not depend on the position of the first difference.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_slices() {
        assert!(ct_eq(b"hello", b"hello"));
    }

    #[test]
    fn different_slices() {
        assert!(!ct_eq(b"hello", b"world"));
    }

    #[test]
    fn different_lengths() {
        assert!(!ct_eq(b"short", b"longer"));
    }

    #[test]
    fn empty_slices() {
        assert!(ct_eq(b"", b""));
    }
}
