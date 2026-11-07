//! Real host resource observations for the Host system.info surface.
//!
//! This module reports only values obtained from the current operating
//! system. Unsupported platforms return an explicit error instead of a
//! synthetic success payload.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMemorySnapshot {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

pub fn sample_host_hostname() -> Result<String, String> {
    #[cfg(unix)]
    {
        let mut buffer = [0_u8; 256];
        if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
            return Err("host hostname sampling failed".into());
        }
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
        let hostname = String::from_utf8(buffer[..end].to_vec())
            .map_err(|_| "host hostname was not valid UTF-8".to_string())?;
        if hostname.trim().is_empty() {
            return Err("host hostname was empty".into());
        }
        return Ok(hostname);
    }

    #[cfg(windows)]
    {
        let mut buffer = [0_u16; 256];
        let mut length = buffer.len() as u32;
        if unsafe { GetComputerNameW(buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err("host hostname sampling failed".into());
        }
        let hostname = String::from_utf16(&buffer[..length as usize])
            .map_err(|_| "host hostname was not valid UTF-16".to_string())?;
        if hostname.trim().is_empty() {
            return Err("host hostname was empty".into());
        }
        return Ok(hostname);
    }

    #[cfg(not(any(unix, windows)))]
    {
        Err("host hostname sampling is unsupported on this platform".into())
    }
}

pub fn sample_host_os_release() -> Result<String, String> {
    #[cfg(unix)]
    {
        let mut value = std::mem::MaybeUninit::<libc::utsname>::zeroed();
        if unsafe { libc::uname(value.as_mut_ptr()) } != 0 {
            return Err("host OS release sampling failed".into());
        }
        let value = unsafe { value.assume_init() };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                value.release.as_ptr().cast::<u8>(),
                value.release.len(),
            )
        };
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
        let release = String::from_utf8(bytes[..end].to_vec())
            .map_err(|_| "host OS release was not valid UTF-8".to_string())?;
        if release.trim().is_empty() {
            return Err("host OS release was empty".into());
        }
        return Ok(release);
    }

    #[cfg(windows)]
    {
        let mut value = WindowsOsVersionInfo {
            size: std::mem::size_of::<WindowsOsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        };
        let result = unsafe { RtlGetVersion(&mut value) };
        if result != 0 {
            return Err(format!("host OS release sampling failed: status {result}"));
        }
        return Ok(format!("{}.{}.{}", value.major, value.minor, value.build));
    }

    #[cfg(not(any(unix, windows)))]
    {
        Err("host OS release sampling is unsupported on this platform".into())
    }
}

pub fn sample_host_memory() -> Result<HostMemorySnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        return linux_memory();
    }

    #[cfg(target_os = "macos")]
    {
        return macos_memory();
    }

    #[cfg(windows)]
    {
        return windows_memory();
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err("host memory sampling is unsupported on this platform".into())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sysconf_pages(name: libc::c_int, label: &str) -> Result<u64, String> {
    let value = unsafe { libc::sysconf(name) };
    if value <= 0 {
        return Err(format!("host {label} sampling failed"));
    }
    u64::try_from(value).map_err(|_| format!("host {label} value was negative"))
}

#[cfg(target_os = "linux")]
fn linux_memory() -> Result<HostMemorySnapshot, String> {
    let contents = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("host memory sampling failed: {error}"))?;
    let value_kib = |key: &str| {
        contents
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.trim().strip_suffix(" kB"))
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| format!("host memory field {key} was unavailable"))
    };
    Ok(HostMemorySnapshot {
        total_bytes: value_kib("MemTotal:")?.saturating_mul(1024),
        available_bytes: value_kib("MemAvailable:")?.saturating_mul(1024),
    })
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn macos_memory() -> Result<HostMemorySnapshot, String> {
    use std::ffi::CString;
    let name = CString::new("hw.memsize").expect("static sysctl name");
    let mut total_bytes = 0_u64;
    let mut length = std::mem::size_of::<u64>();
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut total_bytes as *mut u64 as *mut libc::c_void,
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || total_bytes == 0 {
        return Err("host physical memory sampling failed".into());
    }

    let page_size = sysconf_pages(libc::_SC_PAGESIZE, "page size")?;
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64_data_t>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let result = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr() as *mut libc::integer_t,
            &mut count,
        )
    };
    if result != 0 {
        return Err("host available memory sampling failed".into());
    }
    let stats = unsafe { stats.assume_init() };
    let available_pages = u64::from(stats.free_count)
        .saturating_add(u64::from(stats.inactive_count))
        .saturating_add(u64::from(stats.speculative_count));
    Ok(HostMemorySnapshot {
        total_bytes,
        available_bytes: available_pages.saturating_mul(page_size),
    })
}

#[cfg(windows)]
#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_physical: u64,
    available_physical: u64,
    total_page_file: u64,
    available_page_file: u64,
    total_virtual: u64,
    available_virtual: u64,
    available_extended_virtual: u64,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsOsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

#[cfg(windows)]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut WindowsOsVersionInfo) -> i32;
    fn GetComputerNameW(buffer: *mut u16, size: *mut u32) -> i32;
}

#[cfg(windows)]
unsafe extern "system" {
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
}

#[cfg(windows)]
fn windows_memory() -> Result<HostMemorySnapshot, String> {
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };
    let success = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    if !success || status.total_physical == 0 {
        return Err("host memory sampling failed".into());
    }
    Ok(HostMemorySnapshot {
        total_bytes: status.total_physical,
        available_bytes: status.available_physical,
    })
}

#[cfg(test)]
mod tests {
    use super::{sample_host_hostname, sample_host_memory, sample_host_os_release};

    #[test]
    fn reports_positive_total_and_bounded_available_memory() {
        let memory = sample_host_memory().expect("host memory is available");
        assert!(memory.total_bytes > 0);
        assert!(memory.available_bytes <= memory.total_bytes);
    }

    #[test]
    fn reports_non_empty_os_release() {
        assert!(!sample_host_os_release().expect("host OS release is available").is_empty());
    }

    #[test]
    fn reports_non_empty_hostname() {
        assert!(!sample_host_hostname().expect("host hostname is available").is_empty());
    }
}
