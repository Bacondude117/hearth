//! On Windows, every server process is placed in one Job Object created with
//! `KILL_ON_JOB_CLOSE`. If Hearth crashes or is killed from Task Manager, Windows closes the
//! job handle and terminates the servers too, so no orphaned `java.exe` keeps a world locked.
//! On other platforms this is a no-op (`kill_on_drop` covers normal exits).
//!
//! A server with a memory limit gets its own job instead, with the same kill-on-close plus
//! `JOB_OBJECT_LIMIT_JOB_MEMORY`, which caps the committed memory of the server and any
//! processes it starts. .NET servers (Terraria, tModLoader) also size their heap to it.

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    // HANDLE is a raw pointer (not Send/Sync); store it as an integer.
    static JOB: OnceLock<usize> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        let h = *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return 0;
            }
            // Intentionally never closed: it must live as long as the app.
            job as usize
        });
        (h != 0).then_some(h as HANDLE)
    }

    /// A server's own job. Closing it (when the server is gone) is harmless; if Hearth dies,
    /// Windows closes it and the server is terminated.
    pub struct JobGuard(usize);

    impl Drop for JobGuard {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed exactly once.
            unsafe { CloseHandle(self.0 as HANDLE) };
        }
    }

    /// Puts the server in a job of its own that limits its memory to `limit_mb`.
    pub fn adopt_limited(proc_handle: Option<usize>, limit_mb: u32) -> Option<JobGuard> {
        let proc_handle = proc_handle?;
        // SAFETY: plain Win32 calls with valid, zero-initialized structures.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let guard = JobGuard(job as usize);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = limit_mb as usize * 1024 * 1024;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, proc_handle as HANDLE) == 0 {
                tracing::warn!("memory limit not applied: {}", std::io::Error::last_os_error());
                return None;
            }
            Some(guard)
        }
    }

    pub fn adopt(proc_handle: Option<usize>) {
        let (Some(job), Some(proc_handle)) = (job(), proc_handle) else {
            tracing::warn!("could not attach server process to job object");
            return;
        };
        let ok = unsafe { AssignProcessToJobObject(job, proc_handle as HANDLE) };
        if ok == 0 {
            tracing::warn!("AssignProcessToJobObject failed: {}", std::io::Error::last_os_error());
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct JobGuard;

    pub fn adopt(_proc_handle: Option<usize>) {}

    pub fn adopt_limited(_proc_handle: Option<usize>, _limit_mb: u32) -> Option<JobGuard> {
        None
    }
}

pub(crate) use imp::{adopt, adopt_limited, JobGuard};
