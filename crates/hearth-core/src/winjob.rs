//! On Windows, every server process is placed in one Job Object created with
//! `KILL_ON_JOB_CLOSE`. If Hearth crashes or is killed from Task Manager, Windows closes the
//! job handle and terminates the servers too, so no orphaned `java.exe` keeps a world locked.
//! On other platforms this is a no-op (`kill_on_drop` covers normal exits).

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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

    pub fn adopt(child: &tokio::process::Child) {
        let (Some(job), Some(proc_handle)) = (job(), child.raw_handle()) else {
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
    pub fn adopt(_child: &tokio::process::Child) {}
}

pub(crate) use imp::adopt;
