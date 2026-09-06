use super::SchemaVersion;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Owner {
    version: SchemaVersion,
    process: ProcessIdentity,
    pub token: String,
}

impl Owner {
    pub fn new(previous: Option<Self>, token: String) -> anyhow::Result<Self> {
        let state = previous.map(|owner| owner.process.state()).transpose()?;
        match state {
            Some(ProcessState::Running) => Err(anyhow::anyhow!(
                "Session is already open in another actor or process"
            )),
            None | Some(ProcessState::Exited) => Ok(Self {
                version: SchemaVersion,
                process: ProcessIdentity::current()?,
                token,
            }),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProcessIdentity {
    pid: u32,
    started: String,
}

enum ProcessState {
    Running,
    Exited,
}

impl ProcessIdentity {
    fn current() -> anyhow::Result<Self> {
        let pid = std::process::id();
        let started = process_start(pid)?
            .ok_or_else(|| anyhow::anyhow!("Cannot identify the current process"))?;
        Ok(Self { pid, started })
    }

    fn state(&self) -> anyhow::Result<ProcessState> {
        match process_start(self.pid)? {
            Some(started) if started == self.started => Ok(ProcessState::Running),
            _ => Ok(ProcessState::Exited),
        }
    }
}

#[cfg(target_os = "macos")]
fn process_start(pid: u32) -> anyhow::Result<Option<String>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of_val(&info) as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid.try_into()?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    match read {
        0 => match std::io::Error::last_os_error() {
            error if error.raw_os_error() == Some(libc::ESRCH) => Ok(None),
            error => Err(error.into()),
        },
        read if read == size => {
            let info = unsafe { info.assume_init() };
            match info.pbi_status {
                libc::SZOMB => Ok(None),
                _ => Ok(Some(format!(
                    "{}:{}:{}",
                    boot_id()?,
                    info.pbi_start_tvsec,
                    info.pbi_start_tvusec
                ))),
            }
        }
        _ => Err(anyhow::anyhow!("Incomplete process identity for PID {pid}")),
    }
}

#[cfg(target_os = "macos")]
fn boot_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 64];
    let mut size = bytes.len();
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    match result {
        0 => Ok(std::ffi::CStr::from_bytes_with_nul(&bytes[..size])?
            .to_str()?
            .to_owned()),
        _ => Err(std::io::Error::last_os_error().into()),
    }
}

#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let fields = stat
                .rsplit_once(')')
                .ok_or_else(|| anyhow::anyhow!("Invalid process identity for PID {pid}"))?
                .1;
            match fields.split_whitespace().next() {
                Some("Z" | "X") => Ok(None),
                _ => {
                    let started = fields
                        .split_whitespace()
                        .nth(19)
                        .ok_or_else(|| anyhow::anyhow!("Missing process start time for PID {pid}"))?
                        .parse::<u64>()?;
                    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
                    Ok(Some(format!("{}:{started}", boot.trim())))
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_start(_: u32) -> anyhow::Result<Option<String>> {
    Err(anyhow::anyhow!(
        "Session process ownership is unsupported on this platform"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_distinguishes_process_incarnations_and_rejects_unsupported_schema() {
        let owner = Owner::new(None, "first".into()).unwrap();
        assert!(Owner::new(Some(owner.clone()), "second".into()).is_err());
        let mut previous = owner.clone();
        previous.process.started = "previous boot or reused PID".into();
        let reclaimed = Owner::new(Some(previous), "second".into()).unwrap();
        assert_eq!(reclaimed.token, "second");
        assert!(reclaimed.process == owner.process);
        let mut previous = serde_json::to_value(owner).unwrap();
        previous["version"] = serde_json::json!(999);
        assert!(serde_json::from_value::<Owner>(previous).is_err());
    }
}
