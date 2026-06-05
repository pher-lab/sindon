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
}
