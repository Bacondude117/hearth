//! Starting a server with a console of its own (hidden) instead of Hearth's pipes.
//!
//! Some Windows servers read their console directly (Source's `srcds -console` calls
//! `GetNumberOfConsoleInputEvents` and quits when stdin is a pipe). Rust's process API always
//! hands the child explicit std handles, so these are started with `CreateProcessW` and no
//! `STARTF_USESTDHANDLES`: with `CREATE_NO_WINDOW` the child gets a real, invisible console.
//! Such servers are talked to another way (RCON) and their output is read from a log file.

/// Joins arguments into a Windows command line, quoted the way the MSVC runtime parses it.
pub fn command_line(program: &str, args: &[String]) -> String {
    let mut out = String::new();
    quote(program, &mut out);
    for a in args {
        out.push(' ');
        quote(a, &mut out);
    }
    out
}

fn quote(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote must be doubled.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

#[cfg(windows)]
pub use imp::ConsoleChild;

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleWindow};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
        INFINITE, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE};

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// A process started with its own hidden console. Closed (not killed) on drop; the job
    /// object it's placed in kills it if Hearth goes away.
    pub struct ConsoleChild {
        handle: usize,
    }

    impl Drop for ConsoleChild {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateProcessW and is closed exactly once.
            unsafe { CloseHandle(self.handle as HANDLE) };
        }
    }

    /// `KEY=VALUE\0...\0\0` with `extra` overriding the inherited variables.
    fn env_block(extra: &[(String, String)]) -> Vec<u16> {
        let mut vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| !extra.iter().any(|(e, _)| e.eq_ignore_ascii_case(k)))
            .collect();
        vars.extend(extra.iter().cloned());
        vars.sort_by_key(|(k, _)| k.to_uppercase());
        let mut block: Vec<u16> = vec![];
        for (k, v) in vars {
            block.extend(format!("{k}={v}").encode_utf16());
            block.push(0);
        }
        block.push(0);
        block
    }

    /// Only one console can be attached to Hearth at a time.
    static CONSOLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hides the console window the server opens for itself, if Windows shows it anyway. Checks
    /// for a minute (the window appears once the server has loaded), by briefly attaching to
    /// the server's console to find its window.
    fn hide_console_window(pid: u32) {
        std::thread::spawn(move || {
            let mut hidden_checks = 0;
            for _ in 0..120 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _g = CONSOLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: Hearth has no console of its own; it attaches, reads the window handle
                // and detaches straight away.
                let found = unsafe {
                    if AttachConsole(pid) == 0 {
                        // Not created yet, or the server has exited.
                        continue;
                    }
                    let hwnd = GetConsoleWindow();
                    let ok = !hwnd.is_null();
                    if ok && IsWindowVisible(hwnd) != 0 {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                    FreeConsole();
                    ok
                };
                if found {
                    hidden_checks += 1;
                    if hidden_checks >= 10 {
                        return;
                    }
                }
            }
        });
    }

    impl ConsoleChild {
        pub fn spawn(program: &Path, args: &[String], cwd: &Path, env: &[(String, String)]) -> std::io::Result<Self> {
            let app = wide(program.as_os_str());
            let mut cmdline: Vec<u16> = super::command_line(&program.to_string_lossy(), args)
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let dir = wide(cwd.as_os_str());
            let env = (!env.is_empty()).then(|| env_block(env));
            // SAFETY: zeroed STARTUPINFOW/PROCESS_INFORMATION are valid initial values.
            let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            // srcds is a GUI program that opens its console itself (so CREATE_NO_WINDOW doesn't
            // apply); its first window takes this show state.
            si.dwFlags = STARTF_USESHOWWINDOW;
            si.wShowWindow = SW_HIDE as u16;
            let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: every pointer is valid for the call; buffers are NUL-terminated.
            let ok = unsafe {
                CreateProcessW(
                    app.as_ptr(),
                    cmdline.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0, // inherit no handles: the child uses its own console
                    crate::system::CHILD_PROCESS_FLAGS | CREATE_UNICODE_ENVIRONMENT,
                    env.as_ref().map_or(std::ptr::null(), |e| e.as_ptr() as *const _),
                    dir.as_ptr(),
                    &si,
                    &mut pi,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: the thread handle isn't needed.
            unsafe { CloseHandle(pi.hThread) };
            hide_console_window(pi.dwProcessId);
            Ok(Self {
                handle: pi.hProcess as usize,
            })
        }

        pub fn raw_handle(&self) -> usize {
            self.handle
        }

        /// Waits for exit on a blocking thread; returns the exit code.
        pub async fn wait(&self) -> Option<i32> {
            let h = self.handle;
            tokio::task::spawn_blocking(move || {
                // SAFETY: the handle stays open while `self` (owned by the waiting task) lives.
                unsafe {
                    if WaitForSingleObject(h as HANDLE, INFINITE) != WAIT_OBJECT_0 {
                        return None;
                    }
                    let mut code = 0u32;
                    (GetExitCodeProcess(h as HANDLE, &mut code) != 0).then_some(code as i32)
                }
            })
            .await
            .ok()
            .flatten()
        }

        pub fn kill(&self) {
            // SAFETY: terminating our own child process.
            unsafe { TerminateProcess(self.handle as HANDLE, 1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_the_msvc_runtime() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            command_line(r"C:\Servers\srcds.exe", &a(&["-console", "+map", "c1m1_hotel", "coop"])),
            r"C:\Servers\srcds.exe -console +map c1m1_hotel coop"
        );
        assert_eq!(
            command_line(r"C:\Program Files\x\srcds.exe", &a(&["a b", ""])),
            r#""C:\Program Files\x\srcds.exe" "a b" """#
        );
        assert_eq!(command_line("x", &a(&[r#"say "hi""#])), r#"x "say \"hi\"""#);
        assert_eq!(
            command_line("x", &a(&[r"C:\dir with space\"])),
            r#"x "C:\dir with space\\""#
        );
        assert_eq!(
            command_line("x", &a(&[r"a\\b"])),
            r"x a\\b",
            "no quotes: backslashes literal"
        );
    }
}
