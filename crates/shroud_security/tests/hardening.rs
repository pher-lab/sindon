//! Smoke tests for process hardening.
//!
//! These can only assert that the platform hooks return `Ok` on the
//! current host — verifying that ptrace is actually blocked or that
//! `SetProcessMitigationPolicy` took effect requires a forked child with
//! an attached debugger, which isn't worth the test-harness complexity.
//! End-to-end validation lives outside the test suite (manual `gdb` /
//! `lldb` attach, WinDbg).
//!
//! We still guard Unix calls behind a serial mutex: `prctl` /
//! `ptrace(PT_DENY_ATTACH)` are process-global and one-way, so parallel
//! test threads calling them race each other harmlessly but unnecessarily.

use shroud_security::hardening::{
    disable_core_dumps, enable_exploit_mitigation, enable_image_load_hardening,
    enable_ptrace_protection, enable_signature_audit,
};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn hardening_hooks_return_ok_on_host() {
    // Order matters on Linux: disable_core_dumps already sets
    // PR_SET_DUMPABLE=0, so ptrace_protection is a no-op confirming the
    // state. On macOS PT_DENY_ATTACH is process-once but still returns 0
    // on first call in the test process.
    disable_core_dumps().expect("disable_core_dumps failed on host");
    enable_ptrace_protection().expect("enable_ptrace_protection failed on host");
    enable_exploit_mitigation().expect("enable_exploit_mitigation failed on host");
    // IME-safe DLL image-load policy. One-way like the others; applying it
    // to the test process only constrains subsequent loads, which is benign
    // for a local, system-IL test binary.
    enable_image_load_hardening().expect("enable_image_load_hardening failed on host");
}

/// Audit-mode Code Integrity Guard. Separate from the smoke test above
/// because this one carries real information: it fails if
/// `ProcessSignaturePolicy` turns out to be creation-time only, which is
/// exactly the question a CIG feasibility study has to answer before any
/// enforcing variant can be considered. Applying it to the test process
/// blocks nothing — audit mode only writes to the CodeIntegrity log.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn signature_audit_policy_applies_at_runtime() {
    enable_signature_audit().expect("enable_signature_audit failed on host");
}
