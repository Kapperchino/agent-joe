use super::configuration::Configuration;
use anyhow::Context;
use std::{
    ffi::{CStr, CString},
    path::PathBuf,
};

const LINUX_RLIMIT_NOFILE: u32 = 7;
const OPEN_FILE_LIMIT: libc::rlim_t = 65536;

struct HostResources {
    vcpus: u8,
    memory_mib: u32,
}

impl HostResources {
    fn detect() -> anyhow::Result<Self> {
        Self::new(
            unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) },
            unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) },
            unsafe { libc::sysconf(libc::_SC_PAGESIZE) },
        )
        .context("Cannot size sandbox resources from the host")
    }

    fn new(
        cpu_count: libc::c_long,
        physical_pages: libc::c_long,
        page_size: libc::c_long,
    ) -> anyhow::Result<Self> {
        let vcpus =
            u8::try_from(cpu_count).context("Host CPU count does not fit libkrun's vCPU range")?;
        let physical_pages =
            u64::try_from(physical_pages).context("Cannot read the host's physical page count")?;
        let page_size = u64::try_from(page_size).context("Cannot read the host's page size")?;
        let memory_bytes = physical_pages
            .checked_mul(page_size)
            .context("Host physical memory size overflows a byte count")?;
        let memory_mib = u32::try_from(memory_bytes / 2 / (1024 * 1024))
            .context("Half of host physical memory does not fit libkrun's memory range")?;
        match vcpus > 0 && memory_mib > 0 {
            true => Ok(Self { vcpus, memory_mib }),
            false => Err(anyhow::anyhow!(
                "Host must report at least one CPU and 2 MiB of physical memory"
            )),
        }
    }
}

struct Filesystem {
    tag: &'static CStr,
    path: PathBuf,
    read_only: bool,
}

struct KrunContext {
    id: u32,
    init: Box<[u8]>,
}

impl KrunContext {
    fn new(init: Box<[u8]>) -> anyhow::Result<Self> {
        let id = result("krun_create_ctx", krun::krun_create_ctx())? as u32;
        Ok(Self { id, init })
    }

    fn configure(&self, configuration: Configuration) -> anyhow::Result<()> {
        let resources = HostResources::detect()?;
        result(
            "krun_set_vm_config",
            krun::krun_set_vm_config(self.id, resources.vcpus, resources.memory_mib),
        )?;
        let limits = [CString::new(format!(
            "{LINUX_RLIMIT_NOFILE}={OPEN_FILE_LIMIT}:{OPEN_FILE_LIMIT}"
        ))?];
        let limits = pointers(&limits);
        result("krun_set_rlimits", unsafe {
            krun::krun_set_rlimits(self.id, limits.as_ptr())
        })?;
        result(
            "krun_disable_implicit_vsock",
            krun::krun_disable_implicit_vsock(self.id),
        )?;
        result(
            "krun_disable_implicit_console",
            krun::krun_disable_implicit_console(self.id),
        )?;
        result("krun_add_virtio_console_default", unsafe {
            krun::krun_add_virtio_console_default(self.id, 0, 1, 2)
        })?;
        for mount in [
            Filesystem {
                tag: c"/dev/root",
                path: configuration.rootfs,
                read_only: true,
            },
            Filesystem {
                tag: c"joe-workspace",
                path: configuration.workspace,
                read_only: false,
            },
        ] {
            let path = CString::new(mount.path.as_os_str().as_encoded_bytes())?;
            result("krun_add_virtiofs3", unsafe {
                krun::krun_add_virtiofs3(
                    self.id,
                    mount.tag.as_ptr(),
                    path.as_ptr(),
                    0,
                    mount.read_only,
                )
            })?;
        }
        result("krun_fs_add_overlay_file", unsafe {
            krun::krun_fs_add_overlay_file(
                self.id,
                c"/dev/root".as_ptr(),
                c"init.krun".as_ptr(),
                self.init.as_ptr(),
                self.init.len(),
                0o755,
                true,
            )
        })?;
        let arguments = [CString::new("/usr/local/libexec/joe-guest")?];
        let arguments = pointers(&arguments);
        let environment = pointers(&[]);
        result("krun_set_exec", unsafe {
            krun::krun_set_exec(
                self.id,
                c"/bin/sh".as_ptr(),
                arguments.as_ptr(),
                environment.as_ptr(),
            )
        })?;
        Ok(())
    }

    fn start(&self) -> anyhow::Result<()> {
        result("krun_start_enter", krun::krun_start_enter(self.id))?;
        Err(anyhow::anyhow!(
            "libkrun returned without entering the guest"
        ))
    }
}

impl Drop for KrunContext {
    fn drop(&mut self) {
        krun::krun_free_ctx(self.id);
    }
}

fn pointers(strings: &[CString]) -> Vec<*const libc::c_char> {
    let mut pointers = vec![std::ptr::null(); 4096];
    pointers
        .iter_mut()
        .zip(strings)
        .for_each(|(pointer, value)| *pointer = value.as_ptr());
    pointers
}

fn result(operation: &str, status: i32) -> anyhow::Result<i32> {
    match status {
        0.. => Ok(status),
        _ => Err(anyhow::anyhow!(
            "{operation}: {}",
            std::io::Error::from_raw_os_error(-status)
        )),
    }
}

fn raise_open_file_limit() -> anyhow::Result<()> {
    let mut limits = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    match unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
    .context("Cannot read the sandbox launcher's open file limit")?;
    let desired = limits.rlim_max.min(OPEN_FILE_LIMIT);
    match limits.rlim_cur < desired {
        true => {
            limits.rlim_cur = desired;
            match unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limits) } {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
            .context("Cannot raise the sandbox launcher's open file limit")
        }
        false => Ok(()),
    }
}

pub(super) fn run() -> anyhow::Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let configuration: Configuration = match arguments.as_slice() {
        [configuration] => serde_json::from_slice(configuration.as_encoded_bytes())
            .context("Invalid libkrun configuration"),
        _ => Err(anyhow::anyhow!(
            "joe-sandbox requires one internal JSON configuration"
        )),
    }?;
    raise_open_file_limit()?;
    let _firmware = unsafe { libloading::Library::new(&configuration.firmware) }
        .context("Cannot load Joe's bundled libkrun firmware")?;
    let init =
        std::fs::read(&configuration.init).context("Cannot read Joe's guest init program")?;
    let context = KrunContext::new(init.into_boxed_slice())?;
    context.configure(configuration)?;
    context.start()
}

#[cfg(test)]
#[path = "../tests/unit/launcher/tests.rs"]
mod tests;
