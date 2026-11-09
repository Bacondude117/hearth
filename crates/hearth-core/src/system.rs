//! Host facts the UI needs (memory slider bounds).

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
        (ok != 0).then(|| s.ullTotalPhys / (1024 * 1024))
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
