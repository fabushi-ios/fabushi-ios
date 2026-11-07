use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

pub const LOCAL_EXEC_FILE_SANDBOX_ARG: &str = "--local-exec-file-sandbox";
pub const LOCAL_EXEC_FILE_ROOT_ENV: &str = "FABUSHI_LOCAL_EXEC_FILE_ROOT";

fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn request_string<'a>(request: &'a Value, key: &str) -> Result<&'a str, String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("local-exec native file request requires string {key}"))
}

fn request_u64(request: &Value, key: &str) -> Result<u64, String> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("local-exec native file request requires integer {key}"))
}

fn request_bool(request: &Value, key: &str, default: bool) -> bool {
    request.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn optional_string(request: &Value, key: &str) -> Option<String> {
    request.get(key).and_then(Value::as_str).map(str::to_string)
}

fn relative_file_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err("local-exec native file path must be a non-empty relative path".into());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("local-exec native file path traversal was refused".into());
            }
        }
    }
    Ok(path.to_path_buf())
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!("{digest:x}")
}

fn stage_name(transfer_id: &str) -> String {
    let digest = Sha256::digest(transfer_id.as_bytes());
    format!(".fabushi-upload-{digest:x}.part")
}

fn current_version(root: &Path, relative: &Path, max_bytes: u64) -> Result<Option<String>, String> {
    match platform::read_file(root, relative, max_bytes).map_err(error_message)? {
        Some(bytes) => Ok(Some(sha256_hex(&bytes))),
        None => Ok(None),
    }
}

fn require_version(
    current: Option<&str>,
    expected: Option<&str>,
    overwrite: bool,
) -> Result<(), String> {
    if !overwrite && current.is_some() {
        return Err("File already exists and overwrite=false".into());
    }
    if let Some(expected) = expected {
        if current != Some(expected) {
            return Err("File version conflict: expectedVersion does not match the current file".into());
        }
    }
    Ok(())
}

pub fn handle_native_file_request(root: &Path, request: &Value) -> Result<Value, String> {
    let operation = request_string(request, "op")?;
    match operation {
        "version" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let max_bytes = request_u64(request, "maxBytes")?;
            let version = current_version(root, &relative, max_bytes)?;
            Ok(json!({ "version": version }))
        }
        "read" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let max_bytes = request_u64(request, "maxBytes")?;
            let bytes = platform::read_file(root, &relative, max_bytes)
                .map_err(error_message)?
                .ok_or_else(|| "File does not exist".to_string())?;
            let digest = sha256_hex(&bytes);
            Ok(json!({
                "bytesBase64": BASE64.encode(&bytes),
                "sha256": digest,
                "version": digest,
                "size": bytes.len(),
            }))
        }
        "write-atomic" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let max_bytes = request_u64(request, "maxBytes")?;
            let bytes = BASE64
                .decode(request_string(request, "bytesBase64")?)
                .map_err(|error| format!("invalid upload base64: {error}"))?;
            if bytes.len() as u64 > max_bytes {
                return Err(format!(
                    "File exceeds local-exec maximum: {} > {} bytes",
                    bytes.len(),
                    max_bytes
                ));
            }
            let digest = sha256_hex(&bytes);
            if let Some(expected_digest) = request.get("sha256").and_then(Value::as_str) {
                if !expected_digest.eq_ignore_ascii_case(&digest) {
                    return Err("Upload SHA-256 did not match the declared digest".into());
                }
            }
            let current = current_version(root, &relative, max_bytes)?;
            let overwrite = request_bool(request, "overwrite", true);
            let expected = request.get("expectedVersion").and_then(Value::as_str);
            require_version(current.as_deref(), expected, overwrite)?;
            let headroom = request_u64(request, "headroomBytes")?;
            let required = (bytes.len() as u64)
                .checked_add(headroom)
                .ok_or_else(|| "atomic file space requirement overflowed".to_string())?;
            platform::ensure_space(root, &relative, required).map_err(error_message)?;
            platform::atomic_write(root, &relative, &bytes, overwrite).map_err(error_message)?;
            Ok(json!({ "sha256": digest, "version": digest, "size": bytes.len() }))
        }
        "upload-start" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let transfer_id = request_string(request, "transferId")?;
            let total_bytes = request_u64(request, "totalBytes")?;
            let max_bytes = request_u64(request, "maxBytes")?;
            if total_bytes > max_bytes {
                return Err(format!(
                    "File exceeds local-exec maximum: {total_bytes} > {max_bytes} bytes"
                ));
            }
            let current = current_version(root, &relative, max_bytes)?;
            let overwrite = request_bool(request, "overwrite", true);
            let expected = request.get("expectedVersion").and_then(Value::as_str);
            require_version(current.as_deref(), expected, overwrite)?;
            let headroom = request_u64(request, "headroomBytes")?;
            let required = total_bytes
                .checked_add(headroom)
                .ok_or_else(|| "atomic file space requirement overflowed".to_string())?;
            platform::ensure_space(root, &relative, required).map_err(error_message)?;
            platform::create_stage(root, &relative, OsStr::new(&stage_name(transfer_id)))
                .map_err(error_message)?;
            Ok(json!({ "startingVersion": current }))
        }
        "upload-chunk" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let transfer_id = request_string(request, "transferId")?;
            let offset = request_u64(request, "offset")?;
            let bytes = BASE64
                .decode(request_string(request, "bytesBase64")?)
                .map_err(|error| format!("invalid upload chunk base64: {error}"))?;
            let size = platform::write_stage(
                root,
                &relative,
                OsStr::new(&stage_name(transfer_id)),
                offset,
                &bytes,
            )
            .map_err(error_message)?;
            Ok(json!({ "size": size }))
        }
        "upload-commit" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let transfer_id = request_string(request, "transferId")?;
            let total_bytes = request_u64(request, "totalBytes")?;
            let max_bytes = request_u64(request, "maxBytes")?;
            let expected_digest = request_string(request, "sha256")?.to_ascii_lowercase();
            let stage = stage_name(transfer_id);
            let bytes = platform::read_stage(root, &relative, OsStr::new(&stage), max_bytes)
                .map_err(error_message)?;
            if bytes.len() as u64 != total_bytes {
                return Err("Upload commit refused because not all declared bytes were received".into());
            }
            let digest = sha256_hex(&bytes);
            if digest != expected_digest {
                return Err("Upload SHA-256 did not match the declared digest".into());
            }
            let current = current_version(root, &relative, max_bytes)?;
            let starting = optional_string(request, "startingVersion");
            if current != starting {
                return Err("File version changed during chunk transfer".into());
            }
            let overwrite = request_bool(request, "overwrite", true);
            if !overwrite && current.is_some() {
                return Err("File already exists and overwrite=false".into());
            }
            platform::commit_stage(root, &relative, OsStr::new(&stage), overwrite)
                .map_err(error_message)?;
            Ok(json!({ "sha256": digest, "version": digest, "size": bytes.len() }))
        }
        "upload-abort" => {
            let relative = relative_file_path(request_string(request, "path")?)?;
            let transfer_id = request_string(request, "transferId")?;
            platform::remove_stage(root, &relative, OsStr::new(&stage_name(transfer_id)))
                .map_err(error_message)?;
            Ok(json!({ "aborted": true }))
        }
        other => Err(format!("unsupported local-exec native file operation: {other}")),
    }
}

pub fn execute_production_native_file_request_from_env() -> i32 {
    let root = match std::env::var_os(LOCAL_EXEC_FILE_ROOT_ENV).filter(|value| !value.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => {
            eprintln!("{LOCAL_EXEC_FILE_ROOT_ENV} is required");
            return 2;
        }
    };
    let mut input = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut input) {
        eprintln!("failed to read local-exec native file request: {error}");
        return 2;
    }
    let request: Value = match serde_json::from_str(&input) {
        Ok(value) => value,
        Err(error) => {
            println!("{}", json!({ "ok": false, "error": format!("invalid request JSON: {error}") }));
            return 0;
        }
    };
    let response = match handle_native_file_request(&root, &request) {
        Ok(value) => json!({ "ok": true, "value": value }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    println!("{response}");
    0
}

#[cfg(unix)]
mod platform {
    use libc::{self, c_int};
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path};

    fn cstring(value: &OsStr) -> io::Result<CString> {
        CString::new(value.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
    }

    fn open_root(root: &Path) -> io::Result<OwnedFd> {
        let root = cstring(root.as_os_str())?;
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let stats = fstat(owned.as_raw_fd())?;
        if (stats.st_mode & libc::S_IFMT) != libc::S_IFDIR {
            return Err(io::Error::other("authorized local-exec root is not a directory"));
        }
        Ok(owned)
    }

    fn fstat(fd: c_int) -> io::Result<libc::stat> {
        let mut stats = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(fd, &mut stats) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(stats)
    }

    fn dup_fd(fd: c_int) -> io::Result<OwnedFd> {
        let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicated < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
    }

    fn normal_components(relative: &Path) -> io::Result<Vec<&OsStr>> {
        let mut components = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(value) => components.push(value),
                Component::CurDir => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "local-exec native path traversal was refused",
                    ));
                }
            }
        }
        if components.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local-exec native file path is empty",
            ));
        }
        Ok(components)
    }

    fn open_parent(root: &Path, relative: &Path) -> io::Result<(OwnedFd, CString, u64)> {
        let root_fd = open_root(root)?;
        let root_stats = fstat(root_fd.as_raw_fd())?;
        let root_dev = root_stats.st_dev as u64;
        let components = normal_components(relative)?;
        let mut directory = dup_fd(root_fd.as_raw_fd())?;
        for component in &components[..components.len() - 1] {
            let name = cstring(component)?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let next = unsafe { OwnedFd::from_raw_fd(fd) };
            let stats = fstat(next.as_raw_fd())?;
            if (stats.st_mode & libc::S_IFMT) != libc::S_IFDIR {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local-exec path component is not a directory",
                ));
            }
            if stats.st_dev as u64 != root_dev {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local-exec path crosses a filesystem or mount boundary",
                ));
            }
            directory = next;
        }
        let leaf = cstring(components[components.len() - 1])?;
        Ok((directory, leaf, root_dev))
    }

    fn open_regular(
        parent: &OwnedFd,
        name: &CString,
        root_dev: u64,
        flags: c_int,
    ) -> io::Result<Option<File>> {
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(error);
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let stats = fstat(owned.as_raw_fd())?;
        if (stats.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local-exec native file target is not a regular file",
            ));
        }
        if stats.st_dev as u64 != root_dev {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local-exec file crosses a filesystem or mount boundary",
            ));
        }
        Ok(Some(File::from(owned)))
    }

    fn bounded_read(mut file: File, max_bytes: u64) -> io::Result<Vec<u8>> {
        let size = file.metadata()?.len();
        if size > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("File exceeds local-exec maximum: {size} > {max_bytes} bytes"),
            ));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File grew beyond the local-exec maximum while reading",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn read_file(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let (parent, name, root_dev) = open_parent(root, relative)?;
        match open_regular(&parent, &name, root_dev, libc::O_RDONLY)? {
            Some(file) => bounded_read(file, max_bytes).map(Some),
            None => Ok(None),
        }
    }

    pub(super) fn ensure_space(root: &Path, relative: &Path, required: u64) -> io::Result<()> {
        let (parent, _, _) = open_parent(root, relative)?;
        let mut stats = unsafe { std::mem::zeroed::<libc::statvfs>() };
        if unsafe { libc::fstatvfs(parent.as_raw_fd(), &mut stats) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let available = (stats.f_bavail as u128) * (stats.f_frsize as u128);
        if available < required as u128 {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!(
                    "Insufficient disk space for atomic file commit: need at least {required} bytes free, have {available}"
                ),
            ));
        }
        Ok(())
    }

    fn unique_temp_name() -> CString {
        let id = uuid::Uuid::new_v4().simple().to_string();
        CString::new(format!(".fabushi-atomic-{id}.tmp")).expect("generated temp name has no NUL")
    }

    pub(super) fn atomic_write(
        root: &Path,
        relative: &Path,
        data: &[u8],
        overwrite: bool,
    ) -> io::Result<()> {
        let (parent, leaf, _) = open_parent(root, relative)?;
        let temp = unique_temp_name();
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temp.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| -> io::Result<()> {
            file.write_all(data)?;
            file.sync_all()?;
            let commit_result = if overwrite {
                unsafe {
                    libc::renameat(
                        parent.as_raw_fd(),
                        temp.as_ptr(),
                        parent.as_raw_fd(),
                        leaf.as_ptr(),
                    )
                }
            } else {
                let linked = unsafe {
                    libc::linkat(
                        parent.as_raw_fd(),
                        temp.as_ptr(),
                        parent.as_raw_fd(),
                        leaf.as_ptr(),
                        0,
                    )
                };
                if linked == 0 {
                    unsafe { libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0) }
                } else {
                    linked
                }
            };
            if commit_result != 0 {
                return Err(io::Error::last_os_error());
            }
            if unsafe { libc::fsync(parent.as_raw_fd()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0);
            }
        }
        result
    }

    pub(super) fn create_stage(root: &Path, relative: &Path, stage: &OsStr) -> io::Result<()> {
        let (parent, _, _) = open_parent(root, relative)?;
        let stage = cstring(stage)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                stage.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        file.sync_all()
    }

    pub(super) fn write_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        offset: u64,
        data: &[u8],
    ) -> io::Result<u64> {
        let (parent, _, root_dev) = open_parent(root, relative)?;
        let stage = cstring(stage)?;
        let mut file = open_regular(&parent, &stage, root_dev, libc::O_RDWR)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upload staging file is missing"))?;
        let current = file.metadata()?.len();
        if current != offset {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Upload chunk offset is stale or out of order",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(file.metadata()?.len())
    }

    pub(super) fn read_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        max_bytes: u64,
    ) -> io::Result<Vec<u8>> {
        let (parent, _, root_dev) = open_parent(root, relative)?;
        let stage = cstring(stage)?;
        let file = open_regular(&parent, &stage, root_dev, libc::O_RDONLY)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upload staging file is missing"))?;
        bounded_read(file, max_bytes)
    }

    pub(super) fn commit_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        overwrite: bool,
    ) -> io::Result<()> {
        let (parent, leaf, _) = open_parent(root, relative)?;
        let stage = cstring(stage)?;
        let commit_result = if overwrite {
            unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    stage.as_ptr(),
                    parent.as_raw_fd(),
                    leaf.as_ptr(),
                )
            }
        } else {
            let linked = unsafe {
                libc::linkat(
                    parent.as_raw_fd(),
                    stage.as_ptr(),
                    parent.as_raw_fd(),
                    leaf.as_ptr(),
                    0,
                )
            };
            if linked == 0 {
                unsafe { libc::unlinkat(parent.as_raw_fd(), stage.as_ptr(), 0) }
            } else {
                linked
            }
        };
        if commit_result != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fsync(parent.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn remove_stage(root: &Path, relative: &Path, stage: &OsStr) -> io::Result<()> {
        let (parent, _, _) = open_parent(root, relative)?;
        let stage = cstring(stage)?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), stage.as_ptr(), 0) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ENOENT) {
                return Err(error);
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::{OsStr, c_void};
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::path::{Component, Path};
    use std::ptr::{null, null_mut};

    type Handle = *mut c_void;
    type NtStatus = i32;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const FILE_READ_DATA: u32 = 0x0001;
    const FILE_WRITE_DATA: u32 = 0x0002;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const DELETE: u32 = 0x0001_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_OPEN: u32 = 0x1;
    const FILE_CREATE: u32 = 0x2;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_TAG_INFO_CLASS: u32 = 9;
    const FILE_RENAME_INFORMATION_CLASS: u32 = 10;
    const FILE_DISPOSITION_INFO_EX_CLASS: u32 = 21;
    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
    const FILE_DISPOSITION_FLAG_DELETE: u32 = 0x1;
    const FILE_DISPOSITION_FLAG_POSIX_SEMANTICS: u32 = 0x2;
    const FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE: u32 = 0x10;
    const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xC0000034u32 as i32;
    const STATUS_OBJECT_PATH_NOT_FOUND: NtStatus = 0xC000003Au32 as i32;
    const STATUS_NO_SUCH_FILE: NtStatus = 0xC000000Fu32 as i32;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: Handle,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    union IoStatusValue {
        status: NtStatus,
        pointer: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        value: IoStatusValue,
        information: usize,
    }

    #[repr(C)]
    struct FileAttributeTagInfo {
        file_attributes: u32,
        reparse_tag: u32,
    }

    #[repr(C)]
    struct FileRenameInfo {
        flags: u32,
        root_directory: Handle,
        file_name_length: u32,
        file_name: [u16; 1],
    }

    #[repr(C)]
    struct FileDispositionInfoEx {
        flags: u32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            file_handle: *mut Handle,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *mut i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut c_void,
            ea_length: u32,
        ) -> NtStatus;
        fn NtSetInformationFile(
            file_handle: Handle,
            io_status_block: *mut IoStatusBlock,
            file_information: *mut c_void,
            length: u32,
            file_information_class: u32,
        ) -> NtStatus;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetFileInformationByHandleEx(
            handle: Handle,
            info_class: u32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
        fn SetFileInformationByHandle(
            handle: Handle,
            info_class: u32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_bytes: *mut u64,
            total_free_bytes: *mut u64,
        ) -> i32;
    }

    struct WinHandle(Handle);

    impl Drop for WinHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    impl WinHandle {
        fn raw(&self) -> Handle {
            self.0
        }

        fn into_file(self) -> File {
            let raw = self.0;
            std::mem::forget(self);
            unsafe { File::from_raw_handle(raw as RawHandle) }
        }
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn normal_components(relative: &Path) -> io::Result<Vec<&OsStr>> {
        let mut components = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(value) => components.push(value),
                Component::CurDir => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "local-exec native path traversal was refused",
                    ));
                }
            }
        }
        if components.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local-exec native file path is empty",
            ));
        }
        Ok(components)
    }

    fn attributes(handle: Handle) -> io::Result<FileAttributeTagInfo> {
        let mut info = FileAttributeTagInfo {
            file_attributes: 0,
            reparse_tag: 0,
        };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FILE_ATTRIBUTE_TAG_INFO_CLASS,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<FileAttributeTagInfo>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    fn validate_handle(handle: Handle, directory: bool) -> io::Result<()> {
        let info = attributes(handle)?;
        if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local-exec path traverses a Windows reparse point",
            ));
        }
        let is_directory = info.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if is_directory != directory {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                if directory {
                    "local-exec path component is not a directory"
                } else {
                    "local-exec native file target is not a regular file"
                },
            ));
        }
        Ok(())
    }

    fn open_root(root: &Path) -> io::Result<WinHandle> {
        let path = wide_null(root.as_os_str());
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let owned = WinHandle(handle);
        validate_handle(owned.raw(), true)?;
        Ok(owned)
    }

    fn nt_open(
        parent: Handle,
        name: &OsStr,
        desired_access: u32,
        disposition: u32,
        options: u32,
        directory: bool,
    ) -> io::Result<Option<WinHandle>> {
        let mut wide: Vec<u16> = name.encode_wide().collect();
        let byte_len = wide
            .len()
            .checked_mul(2)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path component is too long"))?;
        let mut unicode = UnicodeString {
            length: byte_len,
            maximum_length: byte_len,
            buffer: wide.as_mut_ptr(),
        };
        let mut attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: parent,
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: null_mut(),
            security_quality_of_service: null_mut(),
        };
        let mut iosb = IoStatusBlock {
            value: IoStatusValue { status: 0 },
            information: 0,
        };
        let mut handle: Handle = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access | SYNCHRONIZE,
                &mut attrs,
                &mut iosb,
                null_mut(),
                FILE_ATTRIBUTE_NORMAL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                disposition,
                options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                null_mut(),
                0,
            )
        };
        if status < 0 {
            if status == STATUS_OBJECT_NAME_NOT_FOUND
                || status == STATUS_OBJECT_PATH_NOT_FOUND
                || status == STATUS_NO_SUCH_FILE
            {
                return Ok(None);
            }
            return Err(io::Error::other(format!(
                "NtCreateFile failed with NTSTATUS 0x{:08x}",
                status as u32
            )));
        }
        let owned = WinHandle(handle);
        validate_handle(owned.raw(), directory)?;
        Ok(Some(owned))
    }

    fn open_parent(root: &Path, relative: &Path) -> io::Result<(WinHandle, Vec<u16>)> {
        let components = normal_components(relative)?;
        let mut directory = open_root(root)?;
        for component in &components[..components.len() - 1] {
            directory = nt_open(
                directory.raw(),
                component,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                FILE_OPEN,
                FILE_DIRECTORY_FILE,
                true,
            )?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "path component is missing"))?;
        }
        let leaf = components[components.len() - 1].encode_wide().collect::<Vec<_>>();
        Ok((directory, leaf))
    }

    fn leaf_os(wide: &[u16]) -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(wide)
    }

    fn open_regular(
        parent: &WinHandle,
        leaf: &[u16],
        desired_access: u32,
    ) -> io::Result<Option<File>> {
        let name = leaf_os(leaf);
        nt_open(
            parent.raw(),
            &name,
            desired_access | FILE_READ_ATTRIBUTES,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
            false,
        )
        .map(|value| value.map(WinHandle::into_file))
    }

    fn bounded_read(mut file: File, max_bytes: u64) -> io::Result<Vec<u8>> {
        let size = file.metadata()?.len();
        if size > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("File exceeds local-exec maximum: {size} > {max_bytes} bytes"),
            ));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File grew beyond the local-exec maximum while reading",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn read_file(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let (parent, leaf) = open_parent(root, relative)?;
        match open_regular(&parent, &leaf, FILE_READ_DATA)? {
            Some(file) => bounded_read(file, max_bytes).map(Some),
            None => Ok(None),
        }
    }

    pub(super) fn ensure_space(root: &Path, _relative: &Path, required: u64) -> io::Result<()> {
        let path = wide_null(root.as_os_str());
        let mut available = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if available < required {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!(
                    "Insufficient disk space for atomic file commit: need at least {required} bytes free, have {available}"
                ),
            ));
        }
        Ok(())
    }

    fn create_relative_file(parent: &WinHandle, name: &OsStr) -> io::Result<File> {
        nt_open(
            parent.raw(),
            name,
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
            false,
        )?
        .ok_or_else(|| io::Error::other("NtCreateFile unexpectedly returned no staging handle"))
        .map(WinHandle::into_file)
    }

    fn rename_file(file: &File, parent: &WinHandle, leaf: &[u16], replace: bool) -> io::Result<()> {
        let offset = std::mem::offset_of!(FileRenameInfo, file_name);
        let name_bytes = leaf
            .len()
            .checked_mul(2)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
        let bytes = offset + name_bytes;
        let words = (bytes + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>();
        let mut storage = vec![0usize; words];
        let info = storage.as_mut_ptr() as *mut FileRenameInfo;
        unsafe {
            (*info).flags = if replace {
                FILE_RENAME_FLAG_REPLACE_IF_EXISTS
            } else {
                0
            };
            (*info).root_directory = parent.raw();
            (*info).file_name_length = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                leaf.as_ptr() as *const u8,
                (info as *mut u8).add(offset),
                name_bytes,
            );
        }
        use std::os::windows::io::AsRawHandle;
        let mut iosb = IoStatusBlock {
            value: IoStatusValue { status: 0 },
            information: 0,
        };
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle() as Handle,
                &mut iosb,
                info as *mut c_void,
                bytes as u32,
                FILE_RENAME_INFORMATION_CLASS,
            )
        };
        if status < 0 {
            return Err(io::Error::other(format!(
                "NtSetInformationFile(FileRenameInformation) failed with NTSTATUS 0x{:08x}",
                status as u32
            )));
        }
        Ok(())
    }

    fn delete_file(file: &File) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        let info = FileDispositionInfoEx {
            flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as Handle,
                FILE_DISPOSITION_INFO_EX_CLASS,
                &info as *const _ as *const c_void,
                std::mem::size_of::<FileDispositionInfoEx>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn atomic_write(
        root: &Path,
        relative: &Path,
        data: &[u8],
        overwrite: bool,
    ) -> io::Result<()> {
        let (parent, leaf) = open_parent(root, relative)?;
        let temp_name = format!(".fabushi-atomic-{}.tmp", uuid::Uuid::new_v4().simple());
        let mut file = create_relative_file(&parent, OsStr::new(&temp_name))?;
        let result = (|| -> io::Result<()> {
            file.write_all(data)?;
            file.sync_all()?;
            rename_file(&file, &parent, &leaf, overwrite)
        })();
        if result.is_err() {
            let _ = delete_file(&file);
        }
        result
    }

    pub(super) fn create_stage(root: &Path, relative: &Path, stage: &OsStr) -> io::Result<()> {
        let (parent, _) = open_parent(root, relative)?;
        let file = create_relative_file(&parent, stage)?;
        file.sync_all()
    }

    pub(super) fn write_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        offset: u64,
        data: &[u8],
    ) -> io::Result<u64> {
        let (parent, _) = open_parent(root, relative)?;
        let stage_wide = stage.encode_wide().collect::<Vec<_>>();
        let mut file = open_regular(&parent, &stage_wide, FILE_READ_DATA | FILE_WRITE_DATA)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upload staging file is missing"))?;
        let current = file.metadata()?.len();
        if current != offset {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Upload chunk offset is stale or out of order",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(file.metadata()?.len())
    }

    pub(super) fn read_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        max_bytes: u64,
    ) -> io::Result<Vec<u8>> {
        let (parent, _) = open_parent(root, relative)?;
        let stage_wide = stage.encode_wide().collect::<Vec<_>>();
        let file = open_regular(&parent, &stage_wide, FILE_READ_DATA)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upload staging file is missing"))?;
        bounded_read(file, max_bytes)
    }

    pub(super) fn commit_stage(
        root: &Path,
        relative: &Path,
        stage: &OsStr,
        overwrite: bool,
    ) -> io::Result<()> {
        let (parent, leaf) = open_parent(root, relative)?;
        let stage_handle = nt_open(
            parent.raw(),
            stage,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
            false,
        )?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upload staging file is missing"))?;
        let file = stage_handle.into_file();
        rename_file(&file, &parent, &leaf, overwrite)
    }

    pub(super) fn remove_stage(root: &Path, relative: &Path, stage: &OsStr) -> io::Result<()> {
        let (parent, _) = open_parent(root, relative)?;
        let Some(handle) = nt_open(
            parent.raw(),
            stage,
            FILE_READ_ATTRIBUTES | DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
            false,
        )? else {
            return Ok(());
        };
        let file = handle.into_file();
        delete_file(&file)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::path::Path;

    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native local-exec file sandbox is unsupported on this platform",
        ))
    }

    pub(super) fn read_file(_: &Path, _: &Path, _: u64) -> io::Result<Option<Vec<u8>>> { unsupported() }
    pub(super) fn ensure_space(_: &Path, _: &Path, _: u64) -> io::Result<()> { unsupported() }
    pub(super) fn atomic_write(_: &Path, _: &Path, _: &[u8], _: bool) -> io::Result<()> { unsupported() }
    pub(super) fn create_stage(_: &Path, _: &Path, _: &OsStr) -> io::Result<()> { unsupported() }
    pub(super) fn write_stage(_: &Path, _: &Path, _: &OsStr, _: u64, _: &[u8]) -> io::Result<u64> { unsupported() }
    pub(super) fn read_stage(_: &Path, _: &Path, _: &OsStr, _: u64) -> io::Result<Vec<u8>> { unsupported() }
    pub(super) fn commit_stage(_: &Path, _: &Path, _: &OsStr, _: bool) -> io::Result<()> { unsupported() }
    pub(super) fn remove_stage(_: &Path, _: &Path, _: &OsStr) -> io::Result<()> { unsupported() }
}
