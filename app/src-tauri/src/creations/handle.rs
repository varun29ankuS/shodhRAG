//! What Shodh reads of a creation folder is decided on the open file, not on
//! its path.
//!
//! The folder is written by commands Shodh does not run itself (on macOS,
//! unasked, in a sandbox that cannot read the home folder). A path checked
//! and then opened can be swapped in between: a folder of it replaced by a
//! link to a file the sandbox hides. So every read here:
//!
//! - opens without following a final link (`O_NOFOLLOW`; on Windows the link
//!   itself, `FILE_FLAG_OPEN_REPARSE_POINT`), and without blocking on a FIFO;
//! - asks the **open handle** where it really is (`F_GETPATH` on macOS,
//!   `/proc/self/fd` elsewhere on Unix, `GetFinalPathNameByHandleW` on
//!   Windows), which no later swap can change, and refuses it unless that is
//!   inside the creation folder;
//! - refuses a file with more than one name (a hard link can give a file
//!   from elsewhere a name inside the folder) and anything but a regular file.
//!
//! Folders are told apart by their identity on disk ([`identity`]): a
//! creation's record keeps its folder's, so a folder of the same name made
//! later is never taken for it.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, what.to_string())
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\host\x` → `\\host\x`.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Open `path` to read, never through a final link and never waiting on a
/// FIFO.
#[cfg(unix)]
fn open_raw(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

#[cfg(windows)]
fn open_raw(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_raw(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Where the open `file` really is.
#[cfg(target_vendor = "apple")]
fn real_path(file: &File) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let path = rustix::fs::getpath(file)?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path.as_bytes())))
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn real_path(file: &File) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(windows)]
fn real_path(file: &File) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let handle = HANDLE(file.as_raw_handle());
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: the handle is open for the lifetime of `file`, and the
        // buffer is writable for its whole length.
        let written =
            unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, FILE_NAME_NORMALIZED) }
                as usize;
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written < buffer.len() {
            buffer.truncate(written);
            return Ok(plain(PathBuf::from(std::ffi::OsString::from_wide(&buffer))));
        }
        buffer.resize(written + 1, 0);
    }
}

#[cfg(not(any(unix, windows)))]
fn real_path(_file: &File) -> io::Result<PathBuf> {
    Err(refused("this system cannot say where an open file is"))
}

/// How many names the open `file` has, and whether it is a link (a reparse
/// point on Windows).
#[cfg(unix)]
fn names_and_link(file: &File) -> io::Result<(u64, bool)> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    Ok((meta.nlink(), meta.file_type().is_symlink()))
}

#[cfg(windows)]
fn names_and_link(file: &File) -> io::Result<(u64, bool)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for the lifetime of `file`; `info` is a
    // valid out pointer.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok((
        u64::from(info.nNumberOfLinks),
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0,
    ))
}

#[cfg(not(any(unix, windows)))]
fn names_and_link(_file: &File) -> io::Result<(u64, bool)> {
    Ok((1, false))
}

/// Whether `path` lies inside `root` (both resolved; without case on Windows).
pub fn within(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    for part in root.components() {
        let Some(next) = parts.next() else {
            return false;
        };
        let same = if cfg!(windows) {
            part.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&next.as_os_str().to_string_lossy())
        } else {
            part == next
        };
        if !same {
            return false;
        }
    }
    true
}

/// Open `path` to read, as a regular file with one name whose real place,
/// read from the open handle, is inside `root` (resolved). Its size comes with
/// it.
pub fn open_inside(root: &Path, path: &Path) -> io::Result<(File, u64)> {
    let file = open_raw(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(refused("it is not a file"));
    }
    let real = plain(real_path(&file)?);
    if !within(&real, root) {
        return Err(refused("the file it opens is outside the creation folder"));
    }
    let (names, link) = names_and_link(&file)?;
    if link {
        return Err(refused("it is a link"));
    }
    if names > 1 {
        return Err(refused(
            "the file is a hard link, which may be a file from outside the folder",
        ));
    }
    Ok((file, meta.len()))
}

/// The identity of the folder at `path` on its disk (device and inode, or
/// volume and file index): a folder made later under the same name has
/// another. The folder itself is not followed if it is a link.
#[cfg(unix)]
pub fn identity(path: &Path) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(refused("it is not a folder"));
    }
    Ok(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
pub fn identity(path: &Path) -> io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    let folder = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for the lifetime of `folder`; `info` is a
    // valid out pointer.
    unsafe { GetFileInformationByHandle(HANDLE(folder.as_raw_handle()), &mut info) }
        .map_err(|e| io::Error::other(e.to_string()))?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Err(refused("it is not a folder"));
    }
    Ok(format!(
        "{}:{}:{}",
        info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn identity(path: &Path) -> io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(refused("it is not a folder"));
    }
    Err(refused("this system cannot tell folders apart"))
}
