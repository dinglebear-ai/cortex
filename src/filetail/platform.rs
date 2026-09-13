//! Platform-specific primitives for file-tail ingest.
//!
//! The file-tail security and rotation model is Unix-centric: it uses the
//! `(st_dev, st_ino)` pair to detect log rotation/truncation and opens files
//! with `O_NOFOLLOW` to defeat symlink-swap TOCTOU attacks. Those concepts have
//! no direct Windows equivalent, so this module isolates the two
//! platform-specific operations behind a stable, cross-platform surface.
//!
//! Identity comes from the opened handle on both Unix and Windows, so saved
//! checkpoints cannot match a different file merely because its length grew.

use std::fs::{File, OpenOptions};
use std::path::Path;

pub(crate) fn file_identity(file: &File) -> std::io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct FileInfo {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut FileInfo,
            ) -> i32;
        }
        let mut info = FileInfo::default();
        // SAFETY: the borrowed File keeps its handle open; info has the C
        // BY_HANDLE_FILE_INFORMATION layout and is writable for this call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((
            u64::from(info.volume),
            (u64::from(info.index_high) << 32) | u64::from(info.index_low),
        ))
    }
}

/// Open `path` read-only without following a terminal symlink / reparse point.
pub(crate) fn open_read_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT — open the reparse point itself rather
        // than following it, mirroring O_NOFOLLOW's symlink-swap protection.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}
