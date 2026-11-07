//! Confined atomic write for the existing ExecService Write resource.
use std::path::Path;

#[cfg(unix)]
pub fn write(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let relative=target.strip_prefix(root).map_err(|_|"write outside workspace")?;
    let mut parts=relative.components().collect::<Vec<_>>();
    let name=parts.pop().ok_or("write target missing")?;
    let std::path::Component::Normal(name)=name else {return Err("invalid write name".into());};
    let name=CString::new(name.as_bytes()).map_err(|_|"invalid write name")?;
    let mut directory=File::open(root).map_err(|_|"workspace root unavailable")?;
    for part in parts {
        let std::path::Component::Normal(part)=part else {return Err("invalid write parent".into());};
        let part=CString::new(part.as_bytes()).map_err(|_|"invalid write parent")?;
        let fd=unsafe {libc::openat(directory.as_raw_fd(),part.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
        if fd<0 {return Err("write parent unavailable or symlink".into());}
        directory=unsafe {File::from_raw_fd(fd)};
    }
    let mut info=std::mem::MaybeUninit::<libc::stat>::uninit();
    let existing=unsafe {libc::fstatat(directory.as_raw_fd(),name.as_ptr(),info.as_mut_ptr(),libc::AT_SYMLINK_NOFOLLOW)};
    if existing==0 {
        let info=unsafe {info.assume_init()};
        if info.st_mode & libc::S_IFMT != libc::S_IFREG {return Err("write target is not a regular file".into());}
    } else if std::io::Error::last_os_error().raw_os_error()!=Some(libc::ENOENT) {
        return Err("write target metadata unavailable".into());
    }
    let temporary=CString::new(format!(".fabushi-write-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst))).map_err(|_|"invalid staging name")?;
    let fd=unsafe {libc::openat(directory.as_raw_fd(),temporary.as_ptr(),libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};
    if fd<0 {return Err("write staging unavailable".into());}
    let mut staged=unsafe {File::from_raw_fd(fd)};
    let result=(|| {
        staged.write_all(bytes).map_err(|_|"write staging failed")?;
        staged.sync_all().map_err(|_|"write sync failed")?;
        let renamed=unsafe {libc::renameat(directory.as_raw_fd(),temporary.as_ptr(),directory.as_raw_fd(),name.as_ptr())};
        if renamed!=0 {return Err("write atomic replacement failed");}
        directory.sync_all().map_err(|_|"write directory sync failed")?;
        Ok(())
    })();
    if result.is_err() {unsafe {libc::unlinkat(directory.as_raw_fd(),temporary.as_ptr(),0);}}
    result.map_err(str::to_string)
}

#[cfg(not(unix))]
pub fn write(_root: &Path, _target: &Path, _bytes: &[u8]) -> Result<(), String> {
    Err("secure workspace writes unsupported on this platform".into())
}
