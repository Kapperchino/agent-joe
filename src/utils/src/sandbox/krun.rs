use super::protocol::Configuration;
use anyhow::Context;
use std::{
    ffi::{CStr, CString},
    path::PathBuf,
};

struct Filesystem {
    tag: &'static CStr,
    path: PathBuf,
    read_only: bool,
}

struct KrunContext {
    id: u32,
}

impl KrunContext {
    fn new() -> anyhow::Result<Self> {
        let id = result("krun_create_ctx", krun::krun_create_ctx())? as u32;
        Ok(Self { id })
    }

    fn configure(&self, configuration: Configuration) -> anyhow::Result<()> {
        result(
            "krun_set_vm_config",
            krun::krun_set_vm_config(self.id, 2, 4096),
        )?;
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
        let arguments = [
            CString::new("/usr/local/libexec/joe-guest")?,
            CString::new(configuration.temporary_name.to_string())?,
        ];
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

pub(super) fn run() -> anyhow::Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let configuration: Configuration = match arguments.as_slice() {
        [configuration] => serde_json::from_slice(configuration.as_encoded_bytes())
            .context("Invalid libkrun configuration"),
        _ => Err(anyhow::anyhow!(
            "joe-sandbox requires one internal JSON configuration"
        )),
    }?;
    let _firmware = unsafe { libloading::Library::new(&configuration.firmware) }
        .context("Cannot load Joe's bundled libkrun firmware")?;
    let context = KrunContext::new()?;
    context.configure(configuration)?;
    context.start()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_a_context_releases_it_in_the_crate() {
        let context = KrunContext::new().unwrap();
        let id = context.id;
        assert_eq!(krun::krun_set_vm_config(id, 2, 4096), 0);
        drop(context);
        assert_eq!(krun::krun_set_vm_config(id, 2, 4096), -libc::ENOENT);
    }

    #[test]
    fn crate_errors_retain_the_operation_and_cause() {
        let context = KrunContext::new().unwrap();
        let error = result(
            "krun_set_vm_config",
            krun::krun_set_vm_config(context.id, 0, 4096),
        )
        .unwrap_err();
        assert!(error.to_string().contains("krun_set_vm_config"));
        assert!(
            error
                .to_string()
                .contains(&std::io::Error::from_raw_os_error(libc::EINVAL).to_string())
        );
    }
}
