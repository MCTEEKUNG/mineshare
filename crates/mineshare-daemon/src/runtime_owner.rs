//! Cross-process ownership for the bridge runtime.
//!
//! The GUI embeds the daemon and the headless binary can run the same
//! runtime.  A process-local single-instance guard cannot stop those two
//! different executables from running simultaneously, so the runtime itself
//! owns the seam and refuses a second owner.

use anyhow::Result;

/// Held for the complete lifetime of `runtime::run`.
///
/// On Windows the named kernel object is scoped to the interactive logon
/// session (`Local\`).  Windows removes it automatically after the last
/// process handle closes, including after a crash.
pub struct RuntimeOwner {
    #[cfg(target_os = "windows")]
    handle: usize,
}

impl RuntimeOwner {
    pub fn acquire() -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            Self::acquire_named("Local\\MineShareRuntimeOwner")
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(Self {})
        }
    }

    #[cfg(target_os = "windows")]
    fn acquire_named(name: &str) -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            use anyhow::Context;
            use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
            use windows_sys::Win32::System::Threading::CreateMutexW;

            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `name` is NUL-terminated and lives through the call.
            // The returned handle is owned by `RuntimeOwner` and closed in Drop.
            let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error())
                    .context("create MineShare runtime ownership mutex");
            }

            // GetLastError must be read immediately after CreateMutexW.  A valid
            // handle plus ERROR_ALREADY_EXISTS means another executable already
            // owns (or is starting) the runtime in this user session.
            let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            if already_exists {
                // SAFETY: `handle` is a valid handle returned above.
                unsafe {
                    CloseHandle(handle);
                }
                anyhow::bail!(
                    "another MineShare runtime is already active in this Windows session"
                );
            }

            Ok(Self {
                handle: handle as usize,
            })
        }

        #[cfg(not(target_os = "windows"))]
        {
            // The currently deployed pair is Windows. Linux keeps its existing
            // systemd ownership until the cross-platform IPC host lands.
            Ok(Self {})
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        // SAFETY: this module is the sole owner of the non-null handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.handle as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::RuntimeOwner;

    #[test]
    fn only_one_runtime_owner_can_exist_per_session() {
        // Exercise the same kernel-object path without colliding with a real
        // MineShare installation running while the developer executes tests.
        let name = format!("Local\\MineShareRuntimeOwnerTest-{}", uuid::Uuid::new_v4());
        let first = RuntimeOwner::acquire_named(&name).expect("first owner");
        let second = RuntimeOwner::acquire_named(&name);
        assert!(second.is_err(), "a duplicate runtime owner was accepted");
        drop(first);
        RuntimeOwner::acquire_named(&name)
            .expect("ownership should recover after the handle closes");
    }
}
