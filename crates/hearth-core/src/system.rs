//! Host facts the UI needs (memory slider bounds).

/// Windows process-creation flags for everything Hearth launches (servers, installers, agents):
/// no console window, and explicitly *normal* priority. Without the priority flag, children
/// would inherit Hearth's own below-normal priority (see `lower_own_priority`).
#[cfg(windows)]
pub(crate) const CHILD_PROCESS_FLAGS: u32 = 0x0800_0000 /* CREATE_NO_WINDOW */ | 0x0000_0020 /* NORMAL_PRIORITY_CLASS */;

/// Runs Hearth's own UI/manager at below-normal CPU priority so game servers and games get the
/// CPU first. Servers are started at normal priority regardless. No-op off Windows.
pub fn lower_own_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS};
        if SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) == 0 {
            tracing::warn!("could not lower Hearth's priority: {}", std::io::Error::last_os_error());
        }
    }
}

/// Total physical memory in MB, if it can be determined.
pub fn total_memory_mb() -> Option<u64> {
    imp::total_memory_mb()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn total_memory_mb() -> Option<u64> {
        let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
        (ok != 0).then_some(s.ullTotalPhys / (1024 * 1024))
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn total_memory_mb() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_some_memory() {
        assert!(super::total_memory_mb().unwrap_or(1) > 0);
    }
}
