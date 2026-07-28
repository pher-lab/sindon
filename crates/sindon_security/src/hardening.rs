//! Process hardening — always-on security measures applied at startup.
//!
//! These are defensive measures that reduce the attack surface of the process.
//! They are applied once at startup and cannot be reversed.

/// Disable core dumps for this process.
/// This prevents sensitive data from being written to disk on crash.
///
/// - Linux: `prctl(PR_SET_DUMPABLE, 0)`
/// - macOS: `setrlimit(RLIMIT_CORE, 0)`
/// - Windows: Sets error mode to prevent crash dumps
pub fn disable_core_dumps() -> Result<(), HardeningError> {
    platform::disable_core_dumps()
}

/// Block debugger attach for this process.
///
/// - Linux: `prctl(PR_SET_DUMPABLE, 0)` — blocks ptrace from non-root.
///   Idempotent with [`disable_core_dumps`].
/// - macOS: `ptrace(PT_DENY_ATTACH, 0, 0, 0)` — rejects any subsequent
///   `PT_ATTACH`.
/// - Windows: no-op. True anti-debugger APIs (`HideFromDebugger`,
///   `CheckRemoteDebuggerPresent` polling) are grey-zone and intentionally
///   skipped; exploit-mitigation hardening goes through
///   [`enable_exploit_mitigation`] instead.
pub fn enable_ptrace_protection() -> Result<(), HardeningError> {
    platform::enable_ptrace_protection()
}

/// Apply OS-level exploit mitigations.
///
/// - Windows: `SetProcessMitigationPolicy` with
///   `ProcessExtensionPointDisablePolicy` — blocks legacy AppInit DLLs,
///   global IME hooks, and similar extension-point DLL injection vectors.
///   **This also disables the extension-DLL path that CJK IMEs load
///   through**, so it is opt-in rather than always-on — see
///   [`enable_image_load_hardening`] for the IME-safe alternative.
/// - Linux / macOS: no-op. Reserved for future seccomp / sandbox hooks;
///   kept in the public surface so apps can call it unconditionally.
pub fn enable_exploit_mitigation() -> Result<(), HardeningError> {
    platform::enable_exploit_mitigation()
}

/// Apply IME-safe DLL image-load hardening.
///
/// Unlike [`enable_exploit_mitigation`] — which disables legacy extension
/// points and, as a side effect, breaks the plumbing CJK IMEs rely on —
/// this policy only constrains *where* DLLs may be loaded from. It never
/// touches the extension-point / IME path, so it is safe to apply by
/// default even for apps that accept Japanese / Chinese / Korean input.
///
/// - Windows: `SetProcessMitigationPolicy` with `ProcessImageLoadPolicy`,
///   setting `NoRemoteImages` (reject DLLs from UNC / remote shares),
///   `NoLowMandatoryLabelImages` (reject DLLs written by a low-integrity
///   process), and `PreferSystem32Images` (search System32 ahead of the
///   application directory, blunting DLL search-order hijacking). Takes
///   effect for images mapped *after* this call, which is the injection
///   window we care about — trusted DLLs already mapped at startup are
///   unaffected.
/// - Linux / macOS: no-op. Reserved for future loader-hardening hooks;
///   kept in the public surface so apps can call it unconditionally.
pub fn enable_image_load_hardening() -> Result<(), HardeningError> {
    platform::enable_image_load_hardening()
}

/// Turn on Code Integrity Guard in **audit** mode — a diagnostic, not a
/// defence.
///
/// Enforcing CIG (`MicrosoftSignedOnly`) would reject every non-Microsoft
/// DLL the process tries to map, which on a wgpu + IME app is a large and
/// hard-to-predict set: third-party IME text services, GPU vendor ICDs,
/// overlays, AV shims. Audit mode applies the *same* signing check and
/// logs what it would have rejected, without rejecting anything — so a
/// run under audit answers "what would enforcing cost us?" empirically.
///
/// - Windows: `SetProcessMitigationPolicy` with `ProcessSignaturePolicy`,
///   setting only `AuditMicrosoftSignedOnly` (bit 3 of the
///   `PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY` bitfield — the
///   enforcing `MicrosoftSignedOnly` is bit 0 and is deliberately *not*
///   set here). Findings land in the
///   `Microsoft-Windows-CodeIntegrity/Operational` event log. The policy
///   is read back afterwards, so an `Ok` means the bit actually stuck
///   rather than merely that the call returned success.
/// - Linux / macOS: no-op. There is no equivalent loader signing check
///   to audit; kept in the public surface so callers can invoke it
///   unconditionally.
///
/// # Audit under-reports — silence is not a clearance
///
/// Measured on Windows 11: an audit run logged nothing at all, and
/// enforcement of the same workload then blocked a DLL (an NVIDIA overlay)
/// that had been mapped during the audit run. The read-back above rules out
/// a failed call, so the silence is the audit channel itself missing loads,
/// not the policy failing to apply.
///
/// So "audit found nothing, therefore enforcing is safe here" does not
/// follow. Feasibility can only be established by an enforcing control run
/// — which is why [`enable_signature_enforcement`] exists at all.
pub fn enable_signature_audit() -> Result<(), HardeningError> {
    platform::enable_signature_audit()
}

/// Enforce Code Integrity Guard — reject every non-Microsoft-signed DLL
/// mapped from here on.
///
/// **Experimental, and not wired into [`App`](../../sindon_app) config.**
/// Two properties make this a poor default, both measured rather than
/// assumed:
///
/// - It only governs images mapped *after* the call. Injection that
///   happens at process creation (overlay and capture hooks, for one) is
///   already mapped by the time an in-process call can run, so the DLLs
///   most worth rejecting are exactly the ones out of reach.
/// - Anything legitimately non-Microsoft that the process needs later —
///   third-party IME text services being the case that matters here — is
///   rejected with it, **and that rejection is silent**. When a third-party
///   IME's text service was blocked in testing, no CodeIntegrity event was
///   written at all: TSF gives up on the COM activation without a trace.
///   Typing degrades to "letters appear, conversion does nothing", and an
///   app that turned this on has no way to connect that bug report to the
///   cause.
///
/// Run [`enable_signature_audit`] first on the workload in question and
/// read the CodeIntegrity log before considering this — while remembering
/// that an empty log does not clear the policy (see that function's docs).
pub fn enable_signature_enforcement() -> Result<(), HardeningError> {
    platform::enable_signature_enforcement()
}

/// Install a panic hook that attempts to zeroize sensitive memory before aborting.
/// The provided closure is called before the default panic handler runs.
pub fn install_panic_hook(pre_panic: impl Fn() + Send + Sync + 'static) {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Run the security cleanup first
        pre_panic();
        // Then call the original handler
        original(info);
    }));
}

#[derive(Debug)]
pub enum HardeningError {
    /// The operation is not supported on this platform.
    Unsupported,
    /// The OS call failed.
    OsError(String),
}

impl std::fmt::Display for HardeningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HardeningError::Unsupported => write!(f, "operation not supported on this platform"),
            HardeningError::OsError(msg) => write!(f, "OS error: {}", msg),
        }
    }
}

impl std::error::Error for HardeningError {}

#[cfg(target_os = "linux")]
mod platform {
    use super::HardeningError;

    pub fn disable_core_dumps() -> Result<(), HardeningError> {
        // prctl(PR_SET_DUMPABLE, 0) — also prevents ptrace attachment
        let ret = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
        if ret == 0 {
            Ok(())
        } else {
            Err(HardeningError::OsError(format!(
                "prctl(PR_SET_DUMPABLE, 0) failed: {}",
                std::io::Error::last_os_error()
            )))
        }
    }

    pub fn enable_ptrace_protection() -> Result<(), HardeningError> {
        // Same syscall as disable_core_dumps; idempotent. Exposed separately
        // so apps can pick either defence without implying the other.
        disable_core_dumps()
    }

    pub fn enable_exploit_mitigation() -> Result<(), HardeningError> {
        // No-op on Linux today. Reserved for future seccomp-bpf filters.
        Ok(())
    }

    pub fn enable_image_load_hardening() -> Result<(), HardeningError> {
        // No-op on Linux today. Loader hardening here would live in a
        // future seccomp / namespace layer.
        Ok(())
    }

    pub fn enable_signature_audit() -> Result<(), HardeningError> {
        // No-op on Linux: no loader-level signing check to audit.
        Ok(())
    }

    pub fn enable_signature_enforcement() -> Result<(), HardeningError> {
        // No-op on Linux: no loader-level signing check to enforce.
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::HardeningError;

    pub fn disable_core_dumps() -> Result<(), HardeningError> {
        let rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &rlim) };
        if ret == 0 {
            Ok(())
        } else {
            Err(HardeningError::OsError(format!(
                "setrlimit(RLIMIT_CORE, 0) failed: {}",
                std::io::Error::last_os_error()
            )))
        }
    }

    pub fn enable_ptrace_protection() -> Result<(), HardeningError> {
        // PT_DENY_ATTACH = 31 on Darwin. Not exported by the libc crate,
        // so we hard-code it (stable since Mac OS X 10.4).
        const PT_DENY_ATTACH: libc::c_int = 31;
        let ret = unsafe { libc::ptrace(PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0) };
        if ret == 0 {
            Ok(())
        } else {
            Err(HardeningError::OsError(format!(
                "ptrace(PT_DENY_ATTACH) failed: {}",
                std::io::Error::last_os_error()
            )))
        }
    }

    pub fn enable_exploit_mitigation() -> Result<(), HardeningError> {
        // No-op on macOS today. Reserved for future sandbox_init hook.
        Ok(())
    }

    pub fn enable_image_load_hardening() -> Result<(), HardeningError> {
        // No-op on macOS today. Reserved for a future dyld-environment /
        // library-validation hook.
        Ok(())
    }

    pub fn enable_signature_audit() -> Result<(), HardeningError> {
        // No-op on macOS: library validation is a codesign entitlement on
        // the bundle, not a runtime-auditable policy.
        Ok(())
    }

    pub fn enable_signature_enforcement() -> Result<(), HardeningError> {
        // No-op on macOS: the equivalent (library validation) is a
        // codesign entitlement, applied at build time.
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::HardeningError;

    pub fn disable_core_dumps() -> Result<(), HardeningError> {
        use windows::Win32::System::Diagnostics::Debug::SEM_FAILCRITICALERRORS;
        use windows::Win32::System::Diagnostics::Debug::SEM_NOGPFAULTERRORBOX;
        use windows::Win32::System::Diagnostics::Debug::SetErrorMode;

        unsafe {
            SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS);
        }
        Ok(())
    }

    pub fn enable_ptrace_protection() -> Result<(), HardeningError> {
        // Windows has no ptrace. Anti-debugger APIs on Windows
        // (NtSetInformationThread/HideFromDebugger, CheckRemoteDebuggerPresent
        // polling) are grey-zone and intentionally skipped — exploit-mitigation
        // hardening lives in `enable_exploit_mitigation` instead.
        Ok(())
    }

    pub fn enable_exploit_mitigation() -> Result<(), HardeningError> {
        use windows::Win32::System::Threading::{
            ProcessExtensionPointDisablePolicy, SetProcessMitigationPolicy,
        };

        // Extension-point policy is a single DWORD; bit 0 =
        // DisableExtensionPoints, remaining bits reserved. Passing a raw u32
        // avoids wrestling with the anonymous-union bitfield binding, which
        // has shifted shape across `windows` crate versions.
        let flags: u32 = 1;
        let result = unsafe {
            SetProcessMitigationPolicy(
                ProcessExtensionPointDisablePolicy,
                &flags as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>(),
            )
        };

        result.map_err(|e| {
            HardeningError::OsError(format!(
                "SetProcessMitigationPolicy(ProcessExtensionPointDisablePolicy) failed: {}",
                e
            ))
        })
    }

    pub fn enable_image_load_hardening() -> Result<(), HardeningError> {
        use windows::Win32::System::Threading::{
            ProcessImageLoadPolicy, SetProcessMitigationPolicy,
        };

        // PROCESS_MITIGATION_IMAGE_LOAD_POLICY is a single DWORD bitfield:
        //   bit 0 = NoRemoteImages
        //   bit 1 = NoLowMandatoryLabelImages
        //   bit 2 = PreferSystem32Images
        // We set all three. As with the extension-point policy above we pass
        // a raw u32 to dodge the anonymous-union bitfield binding, whose shape
        // has shifted across `windows` crate versions.
        let flags: u32 = 0b111;
        let result = unsafe {
            SetProcessMitigationPolicy(
                ProcessImageLoadPolicy,
                &flags as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>(),
            )
        };

        result.map_err(|e| {
            HardeningError::OsError(format!(
                "SetProcessMitigationPolicy(ProcessImageLoadPolicy) failed: {}",
                e
            ))
        })
    }

    /// Bit 3 of `PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY`. The bitfield
    /// is `MicrosoftSignedOnly:1, StoreSignedOnly:1, MitigationOptIn:1,
    /// AuditMicrosoftSignedOnly:1, AuditStoreSignedOnly:1` — so bit 0 is
    /// the enforcing variant we are deliberately avoiding.
    const AUDIT_MICROSOFT_SIGNED_ONLY: u32 = 1 << 3;

    /// Bit 0 — the enforcing variant. Every non-Microsoft-signed image
    /// mapped after this is applied is rejected outright.
    const MICROSOFT_SIGNED_ONLY: u32 = 1;

    pub fn enable_signature_audit() -> Result<(), HardeningError> {
        set_signature_policy(AUDIT_MICROSOFT_SIGNED_ONLY, "audit")
    }

    pub fn enable_signature_enforcement() -> Result<(), HardeningError> {
        set_signature_policy(MICROSOFT_SIGNED_ONLY, "enforce")
    }

    fn set_signature_policy(flags: u32, mode: &str) -> Result<(), HardeningError> {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, GetProcessMitigationPolicy, ProcessSignaturePolicy,
            SetProcessMitigationPolicy,
        };

        // Same raw-u32 trick as the two policies above: the generated
        // binding for this struct is an opaque `_bitfield`, so naming the
        // bits buys nothing over a documented constant.
        unsafe {
            SetProcessMitigationPolicy(
                ProcessSignaturePolicy,
                &flags as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>(),
            )
        }
        .map_err(|e| {
            HardeningError::OsError(format!(
                "SetProcessMitigationPolicy(ProcessSignaturePolicy, {}) failed: {}",
                mode, e
            ))
        })?;

        // Read back rather than trusting the return value. Not all
        // mitigation policies can be set after process creation, and a
        // policy that silently failed to apply would make an empty
        // CodeIntegrity log look like "nothing would be blocked" when it
        // actually means "nothing was ever checked".
        let mut applied: u32 = 0;
        unsafe {
            GetProcessMitigationPolicy(
                GetCurrentProcess(),
                ProcessSignaturePolicy,
                &mut applied as *mut u32 as *mut core::ffi::c_void,
                std::mem::size_of::<u32>(),
            )
        }
        .map_err(|e| {
            HardeningError::OsError(format!(
                "GetProcessMitigationPolicy(ProcessSignaturePolicy) failed: {}",
                e
            ))
        })?;

        if applied & flags == 0 {
            return Err(HardeningError::OsError(format!(
                "ProcessSignaturePolicy {} bit did not stick (flags = {:#x}); \
                 the policy is likely creation-time only on this build",
                mode, applied
            )));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::HardeningError;

    pub fn disable_core_dumps() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }

    pub fn enable_ptrace_protection() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }

    pub fn enable_exploit_mitigation() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }

    pub fn enable_image_load_hardening() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }

    pub fn enable_signature_audit() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }

    pub fn enable_signature_enforcement() -> Result<(), HardeningError> {
        Err(HardeningError::Unsupported)
    }
}
