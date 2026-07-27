/// Security level for a widget or data context.
/// Higher levels inherit all protections from lower levels.
/// When propagating through a widget tree, the effective level
/// is `max(parent_effective, child_declared)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SecurityLevel {
    /// No special handling. Standard rendering and memory.
    #[default]
    Normal = 0,
    /// Zeroize on drop. Secure text buffer. No glyph caching.
    Sensitive = 1,
    /// Sensitive + mlock, guard pages, secure atlas.
    Protected = 2,
    /// Protected + screen capture prevention, IME bypass, full isolation.
    Maximum = 3,
}

impl SecurityLevel {
    /// Returns the stricter of two security levels.
    pub fn merge(self, other: SecurityLevel) -> SecurityLevel {
        std::cmp::max(self, other)
    }

    /// Returns true if this level requires zeroization of data.
    pub fn requires_zeroize(self) -> bool {
        self >= SecurityLevel::Sensitive
    }

    /// Returns true if this level requires memory locking (mlock).
    pub fn requires_mlock(self) -> bool {
        self >= SecurityLevel::Protected
    }

    /// Returns true if this level requires display protection.
    pub fn requires_display_protection(self) -> bool {
        self >= SecurityLevel::Maximum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering() {
        assert!(SecurityLevel::Normal < SecurityLevel::Sensitive);
        assert!(SecurityLevel::Sensitive < SecurityLevel::Protected);
        assert!(SecurityLevel::Protected < SecurityLevel::Maximum);
    }

    #[test]
    fn merge_takes_stricter() {
        assert_eq!(
            SecurityLevel::Normal.merge(SecurityLevel::Protected),
            SecurityLevel::Protected
        );
        assert_eq!(
            SecurityLevel::Maximum.merge(SecurityLevel::Sensitive),
            SecurityLevel::Maximum
        );
    }

    #[test]
    fn tier_checks() {
        assert!(!SecurityLevel::Normal.requires_zeroize());
        assert!(SecurityLevel::Sensitive.requires_zeroize());
        assert!(SecurityLevel::Protected.requires_mlock());
        assert!(!SecurityLevel::Sensitive.requires_mlock());
        assert!(SecurityLevel::Maximum.requires_display_protection());
        assert!(!SecurityLevel::Protected.requires_display_protection());
    }
}
