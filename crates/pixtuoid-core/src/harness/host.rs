//! What the capture rig asks of the host: a known folder no sandbox variable
//! moves, a directory no other local user can write into, whether this process
//! owns a path, and whether a registry key exists.

use std::path::Path;
#[cfg(any(windows, test))]
use std::path::PathBuf;

/// `s` as the NUL-terminated wide string a Win32 call takes.
#[cfg(windows)]
pub(crate) fn nul_terminated(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(Some(0)).collect()
}

/// A Windows known folder, resolved through the shell rather than through an
/// environment variable a sandbox redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownFolder {
    /// `FOLDERID_ProgramData`.
    ProgramData,
    /// `FOLDERID_Profile`: the user's home as Rust's `dirs` and the CLIs built
    /// on it resolve `~`, whatever `USERPROFILE` says.
    Profile,
}

impl KnownFolder {
    /// Every known folder a `system_config` row may name.
    pub const ALL: [Self; 2] = [Self::ProgramData, Self::Profile];

    /// How a `system_config` row spells it.
    pub const fn placeholder(self) -> &'static str {
        match self {
            Self::ProgramData => "{FOLDERID_ProgramData}",
            Self::Profile => "{FOLDERID_Profile}",
        }
    }
}

/// `folder`'s path, which "does not include a trailing backslash"
/// (learn.microsoft.com, SHGetKnownFolderPath).
///
/// # Errors
///
/// If the shell has no path for it.
#[cfg(windows)]
pub fn known_folder(folder: KnownFolder) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Profile, FOLDERID_ProgramData, SHGetKnownFolderPath,
    };

    let id = match folder {
        KnownFolder::ProgramData => &FOLDERID_ProgramData,
        KnownFolder::Profile => &FOLDERID_Profile,
    };
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: `id` is a live GUID, a null token asks for the current user, and
    // `raw` is a live out-pointer.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
    let path = (hr >= 0 && !raw.is_null()).then(|| {
        // SAFETY: on success `raw` is the NUL-terminated path the call allocated.
        let wide = unsafe {
            std::slice::from_raw_parts(raw, (0..).take_while(|&i| *raw.add(i) != 0).count())
        };
        PathBuf::from(std::ffi::OsString::from_wide(wide))
    });
    // SAFETY: the caller frees `raw` "whether SHGetKnownFolderPath succeeds or
    // not"; freeing null is a no-op.
    unsafe { CoTaskMemFree(raw.cast()) };
    path.ok_or_else(|| std::io::Error::from_raw_os_error(hr))
}

/// Owner, SYSTEM and Administrators in full, Users read and execute, nothing
/// inherited: a parent like ProgramData would otherwise pass every local user
/// its create right. Users must keep read: codex's unelevated sandbox token
/// cannot enter a workspace without it.
#[cfg(windows)]
const PRIVATE_DIR: windows_sys::core::PCWSTR =
    windows_sys::w!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)(A;OICI;0x1200a9;;;BU)");

/// Create `path`, one level, where no other local user can write, as one
/// call: refuses one that already exists.
///
/// # Errors
///
/// `AlreadyExists` for an existing entry, or the create's own failure.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

        let sd = crate::security_descriptor::SecurityDescriptor::from_sddl(PRIVATE_DIR)?;
        let wide = nul_terminated(path.as_os_str());
        // SAFETY: `wide` is NUL-terminated, and `sd` outlives the call, which
        // copies the descriptor onto the new directory.
        if unsafe { CreateDirectoryW(wide.as_ptr(), sd.attributes()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
}

/// Whether this process owns the entry at `path`, a link judged as itself and
/// never by its target: a planted link to a directory of ours is still not
/// ours. False whenever that cannot be read.
pub fn owned_by_us(path: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_owned(path).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path)
            .map(|m| m.uid() == rustix::process::getuid().as_raw())
            .unwrap_or(false)
    }
}

/// The entry's owner against the token's default owner, not its user: an
/// elevated administrator's new objects are owned by Administrators.
#[cfg(windows)]
fn windows_owned(path: &Path) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        TOKEN_OWNER, TOKEN_QUERY, TokenOwner,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let wide = nul_terminated(path.as_os_str());
    // SAFETY: `wide` is NUL-terminated; the handle is closed below. The
    // reparse-point flag opens a link itself, never its target.
    let file = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return None;
    }
    let owned = (|| {
        let mut owner: PSID = std::ptr::null_mut();
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `file` was opened with READ_CONTROL; the out-pointers are
        // live, and those this call doesn't fill are null, as documented optional.
        let rc = unsafe {
            GetSecurityInfo(
                file,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut psd,
            )
        };
        if rc != 0 {
            return None;
        }
        let mut token = std::ptr::null_mut();
        // SAFETY: the pseudo-handle needs no close; `token` is a live out-pointer.
        let matches =
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                None
            } else {
                let mut len = 0u32;
                // SAFETY: a null buffer of length 0 is the documented size probe.
                unsafe {
                    GetTokenInformation(token, TokenOwner, std::ptr::null_mut(), 0, &mut len);
                }
                // `u64`-backed so the TOKEN_OWNER's SID pointer is aligned.
                let mut buf = vec![0u64; (len as usize).div_ceil(8)];
                // SAFETY: `token` is open and `buf` spans at least `len` bytes.
                let read = unsafe {
                    GetTokenInformation(token, TokenOwner, buf.as_mut_ptr().cast(), len, &mut len)
                };
                let matches = (read != 0).then(|| {
                    // SAFETY: a successful read filled `buf` with a TOKEN_OWNER whose
                    // SID points into `buf`, which outlives this borrow.
                    let ours = unsafe { (*buf.as_ptr().cast::<TOKEN_OWNER>()).Owner };
                    // SAFETY: both SIDs are valid for the call.
                    unsafe { EqualSid(owner, ours) != 0 }
                });
                // SAFETY: opened above, closed once.
                unsafe { CloseHandle(token) };
                matches
            };
        // SAFETY: LocalAlloc'd by GetSecurityInfo; `owner` pointed into it and is
        // not read again.
        unsafe { LocalFree(psd) };
        matches
    })();
    // SAFETY: opened above, closed once.
    unsafe { CloseHandle(file) };
    owned
}

/// Whether `key` (`HKLM\…` or `HKCU\…`, the 64-bit view) exists. One the
/// process may not read counts as existing: a CLI under the same token could
/// not read it either, but the run would not be shown to be hermetic.
#[cfg(windows)]
pub fn registry_key_exists(key: &str) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };

    let (hive, sub) = match key.split_once('\\') {
        Some(("HKLM", sub)) => (HKEY_LOCAL_MACHINE, sub),
        Some(("HKCU", sub)) => (HKEY_CURRENT_USER, sub),
        _ => return false,
    };
    let wide = nul_terminated(std::ffi::OsStr::new(sub));
    let mut opened: HKEY = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and `opened` a live out-pointer.
    let rc = unsafe { RegOpenKeyExW(hive, wide.as_ptr(), 0, KEY_READ, &mut opened) };
    if rc == ERROR_SUCCESS {
        // SAFETY: opened above, closed once.
        unsafe { RegCloseKey(opened) };
    }
    rc != ERROR_FILE_NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_dir_is_made_once_and_never_reused() {
        let d = tempfile::tempdir().expect("tempdir");
        let dir = d.path().join("base");
        create_private_dir(&dir).expect("create");
        assert!(owned_by_us(&dir));
        let again = create_private_dir(&dir).expect_err("it exists");
        assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
    }

    /// Judged on the link itself: our link to a directory that is not ours is
    /// ours, and following it would have read the target's owner instead.
    #[test]
    fn ownership_is_judged_on_the_entry_itself_never_its_target() {
        let d = tempfile::tempdir().expect("tempdir");
        assert!(owned_by_us(d.path()));
        assert!(!owned_by_us(&d.path().join("missing")));
        let link = d.path().join("link");
        #[cfg(unix)]
        let target = PathBuf::from("/usr");
        #[cfg(windows)]
        let target = system32();
        assert!(!owned_by_us(&target), "a system directory is not ours");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(windows)]
        {
            // A junction: unlike a symlink, it needs no privilege.
            let made = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .status()
                .expect("mklink");
            assert!(made.success());
        }
        assert!(owned_by_us(&link), "the link is ours, its target is not");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_dir_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().expect("tempdir");
        let dir = d.path().join("base");
        create_private_dir(&dir).expect("create");
        let mode = std::fs::metadata(&dir).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// The DACL read back off the directory: a create that dropped the
    /// descriptor would read ProgramData's inherited ACEs here.
    #[cfg(windows)]
    #[test]
    fn a_private_dir_holds_exactly_its_protected_dacl() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

        let d = tempfile::tempdir().expect("tempdir");
        let dir = d.path().join("base");
        create_private_dir(&dir).expect("create");
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&dir)
            .expect("open the directory");
        let held = crate::security_descriptor::tests_support::dacl_sddl(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
        );
        let meant = crate::security_descriptor::SecurityDescriptor::from_sddl(PRIVATE_DIR)
            .expect("sddl")
            .dacl();
        assert_eq!(held, meant);
        assert!(held.starts_with("D:P"), "protected: {held}");
    }

    #[cfg(windows)]
    fn system32() -> PathBuf {
        let root = std::env::var_os("SystemRoot").expect("SystemRoot");
        PathBuf::from(root).join("System32")
    }

    #[cfg(windows)]
    #[test]
    fn program_data_is_read_from_the_shell_not_its_variable() {
        let real = known_folder(KnownFolder::ProgramData).expect("ProgramData");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("ProgramData", Some(elsewhere.path()), || {
            assert_eq!(known_folder(KnownFolder::ProgramData).expect("again"), real);
        });
    }

    #[cfg(windows)]
    #[test]
    fn a_registry_key_exists_only_while_it_does() {
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_VOLATILE, RegCloseKey, RegCreateKeyExW,
            RegDeleteKeyW,
        };

        let sub = format!(r"Software\pixtuoid-test-{}", std::process::id());
        let key = format!(r"HKCU\{sub}");
        assert!(!registry_key_exists(&key));
        let wide = nul_terminated(std::ffi::OsStr::new(&sub));
        let mut made: HKEY = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated; every other pointer is a live
        // out-pointer or documented optional null.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut made,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, ERROR_SUCCESS);
        // SAFETY: created above, closed once.
        unsafe { RegCloseKey(made) };
        assert!(registry_key_exists(&key));
        // SAFETY: `wide` is NUL-terminated.
        let deleted = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide.as_ptr()) };
        assert_eq!(deleted, ERROR_SUCCESS);
        assert!(!registry_key_exists(&key));
        assert!(!registry_key_exists(r"HKXX\Software"), "an unknown hive");
    }
}
