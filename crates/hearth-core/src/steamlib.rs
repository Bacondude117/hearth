//! Finding things the player has installed on this PC: Steam libraries and the Documents
//! folder. Used to locate games Hearth runs from the player's own copy (Factorio) and their
//! mods (tModLoader).

use std::path::{Path, PathBuf};

/// `"path"  "D:\\SteamLibrary"` lines from Steam's `libraryfolders.vdf`.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("\"path\"")?.trim();
            let v = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(v.replace("\\\\", "\\")))
        })
        .collect()
}

/// Every Steam library folder on this PC (the Steam folder itself first).
pub fn libraries() -> Vec<PathBuf> {
    let Some(steam) = platform::steam_dir() else {
        return vec![];
    };
    let mut libs = vec![steam.clone()];
    if let Ok(text) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for p in library_paths(&text) {
            if !libs.iter().any(|l| same_path(l, &p)) {
                libs.push(p);
            }
        }
    }
    libs
}

/// `<library>/steamapps/common/<folder>` for the first library that has it.
pub fn app_dir(common_folder: &str) -> Option<PathBuf> {
    libraries()
        .into_iter()
        .map(|l| l.join("steamapps").join("common").join(common_folder))
        .find(|d| d.is_dir())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The user's Documents folder (it may be redirected, e.g. to OneDrive).
pub fn documents_dir() -> Option<PathBuf> {
    platform::documents_dir()
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    pub fn documents_dir() -> Option<PathBuf> {
        let mut out: *mut u16 = std::ptr::null_mut();
        // SAFETY: SHGetKnownFolderPath allocates `out` on success; it's freed below.
        let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut out) };
        if hr != 0 || out.is_null() {
            return None;
        }
        // SAFETY: `out` is a valid NUL-terminated wide string until freed.
        let path = unsafe {
            let len = (0..).take_while(|&i| *out.add(i) != 0).count();
            let p = std::ffi::OsString::from_wide(std::slice::from_raw_parts(out, len));
            CoTaskMemFree(out as *const _);
            p
        };
        Some(PathBuf::from(path))
    }

    pub fn steam_dir() -> Option<PathBuf> {
        let key = wide("Software\\Valve\\Steam");
        let name = wide("SteamPath");
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buffers are valid for the sizes given.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut size,
            )
        };
        if rc != 0 {
            let fallback = PathBuf::from(r"C:\Program Files (x86)\Steam");
            return fallback.is_dir().then_some(fallback);
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let s = String::from_utf16_lossy(&buf[..len]).replace('/', "\\");
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::PathBuf;

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    pub fn documents_dir() -> Option<PathBuf> {
        home().map(|h| h.join("Documents"))
    }

    pub fn steam_dir() -> Option<PathBuf> {
        home().map(|h| h.join(".steam").join("steam")).filter(|p| p.is_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steam_library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            library_paths(vdf),
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
    }
}
