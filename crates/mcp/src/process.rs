//! Who a pid is: the process's start time and executable, which a pid that
//! macOS gave to a new process doesn't share, and when this Mac booted.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// One process, as opposed to its pid, which macOS reuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Microseconds since 1970 when the process started.
    pub start_us: u64,
    pub executable: String,
}

/// The identity of the process that has `pid` now; `None` when none runs
/// or macOS won't say.
#[cfg(target_os = "macos")]
pub fn identity(pid: i64) -> Option<Identity> {
    let pid = i32::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: proc_pidpath writes at most `path.len()` bytes.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if length <= 0 {
        return None;
    }
    Some(Identity {
        start_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        executable: String::from_utf8_lossy(&path[..length as usize]).into_owned(),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn identity(_pid: i64) -> Option<Identity> {
    None
}

/// When this Mac last booted.
#[cfg(target_os = "macos")]
pub fn boot_time() -> Option<SystemTime> {
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut boot: libc::timeval = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::timeval>();
    // SAFETY: sysctl writes at most `size` bytes into `boot`.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut boot as *mut libc::timeval).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && boot.tv_sec > 0).then(|| {
        SystemTime::UNIX_EPOCH
            + std::time::Duration::new(boot.tv_sec as u64, boot.tv_usec as u32 * 1000)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn boot_time() -> Option<SystemTime> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_an_identity_after_boot() {
        let me = identity(std::process::id() as i64).expect("this process");
        assert_eq!(
            me.executable,
            std::env::current_exe()
                .unwrap()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        let boot = boot_time().expect("boot time");
        let started = SystemTime::UNIX_EPOCH + std::time::Duration::from_micros(me.start_us);
        assert!(boot < started && started <= SystemTime::now());
        assert_eq!(identity(0), None);
        assert_eq!(identity(i64::from(i32::MAX)), None);
    }
}
