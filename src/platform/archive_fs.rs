//! Filesystem space and no-clobber commit primitives for archive extraction.
use std::{io, path::Path};

#[derive(Debug, Clone, Copy)]
pub struct DiskSpace {
    pub available: u64,
    pub allocation_unit: u64,
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            path: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
        fn GetVolumePathNameW(path: *const u16, root: *mut u16, length: u32) -> i32;
        fn GetDiskFreeSpaceW(
            root: *const u16,
            sectors: *mut u32,
            bytes: *mut u32,
            free: *mut u32,
            total: *mut u32,
        ) -> i32;
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"));
        }
        value.push(0);
        Ok(value)
    }
    pub fn disk_space(path: &Path) -> io::Result<DiskSpace> {
        let path = wide(path)?;
        let mut available = 0;
        let mut root = vec![0u16; 32768];
        let (mut sectors, mut bytes, mut free, mut total) = (0, 0, 0, 0);
        // SAFETY: NUL-terminated path and appropriately sized writable outputs;
        // all buffers remain live and no pointer is retained by these APIs.
        unsafe {
            if GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
                return Err(io::Error::last_os_error());
            }
            if GetDiskFreeSpaceW(
                root.as_ptr(),
                &mut sectors,
                &mut bytes,
                &mut free,
                &mut total,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        let allocation_unit = u64::from(sectors) * u64::from(bytes);
        if allocation_unit == 0 {
            return Err(io::Error::other("filesystem allocation unit unavailable"));
        }
        Ok(DiskSpace {
            available,
            allocation_unit,
        })
    }
    pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        let from = wide(from)?;
        let to = wide(to)?;
        // SAFETY: Both paths are live NUL-terminated strings. Flags=0 omits
        // MOVEFILE_REPLACE_EXISTING and cross-volume copying deliberately.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
#[cfg(unix)]
mod native {
    use super::*;
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    fn path_c(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
    }
    // statvfs integer widths vary across Unix targets.
    #[allow(clippy::unnecessary_cast)]
    pub fn disk_space(path: &Path) -> io::Result<DiskSpace> {
        let path = path_c(path)?;
        let mut data = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: Path is a live C string; statvfs initializes data on success.
        if unsafe { libc::statvfs(path.as_ptr(), data.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: statvfs returned success above.
        let data = unsafe { data.assume_init() };
        let unit = data.f_frsize as u64;
        let available = (data.f_bavail as u64)
            .checked_mul(unit)
            .ok_or_else(|| io::Error::other("filesystem space overflow"))?;
        if unit == 0 {
            return Err(io::Error::other("filesystem allocation unit unavailable"));
        }
        Ok(DiskSpace {
            available,
            allocation_unit: unit,
        })
    }
    pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let from = path_c(from)?;
            let to = path_c(to)?;
            // SAFETY: Live C strings, paths relative to the process directory;
            // RENAME_NOREPLACE atomically refuses any pre-existing destination.
            if unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            let from = path_c(from)?;
            let to = path_c(to)?;
            // SAFETY: Live C strings; RENAME_EXCL refuses an existing target.
            if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (from, to);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic no-replace directory commit is not implemented on this platform",
            ))
        }
    }
}
#[cfg(any(windows, unix))]
pub use native::{disk_space, rename_no_replace};
#[cfg(not(any(windows, unix)))]
pub fn disk_space(_: &Path) -> io::Result<DiskSpace> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem space query unavailable",
    ))
}
#[cfg(not(any(windows, unix)))]
pub fn rename_no_replace(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace commit unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn space_query_and_atomic_directory_commit_refuse_existing_targets() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".sandbox-test/reliability-20261102/fs-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let root = tempfile::tempdir_in(&parent).unwrap();
        assert!(disk_space(root.path()).unwrap().allocation_unit > 0);
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(source.join("marker"), b"original").unwrap();
        assert!(rename_no_replace(&source, &target).is_err());
        assert_eq!(std::fs::read(source.join("marker")).unwrap(), b"original");
        std::fs::remove_dir(&target).unwrap();
        rename_no_replace(&source, &target).unwrap();
        assert_eq!(std::fs::read(target.join("marker")).unwrap(), b"original");
    }
}
