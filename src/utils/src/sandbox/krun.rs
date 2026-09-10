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

struct KrunContext<'a> {
    id: u32,
    library: &'a libloading::Library,
}

impl<'a> KrunContext<'a> {
    fn new(library: &'a libloading::Library) -> anyhow::Result<Self> {
        let create = unsafe { library.get::<unsafe extern "C" fn() -> i32>(b"krun_create_ctx\0")? };
        let id = result("krun_create_ctx", unsafe { create() })? as u32;
        Ok(Self { id, library })
    }

    fn configure(&self, configuration: Configuration) -> anyhow::Result<()> {
        let library = self.library;
        let vm = unsafe {
            library.get::<unsafe extern "C" fn(u32, u8, u32) -> i32>(b"krun_set_vm_config\0")?
        };
        result("krun_set_vm_config", unsafe { vm(self.id, 2, 4096) })?;
        self.call(b"krun_disable_implicit_vsock\0")?;
        self.call(b"krun_disable_implicit_console\0")?;
        let console = unsafe {
            library.get::<unsafe extern "C" fn(u32, i32, i32, i32) -> i32>(
                b"krun_add_virtio_console_default\0",
            )?
        };
        result("krun_add_virtio_console_default", unsafe {
            console(self.id, 0, 1, 2)
        })?;
        let filesystem = unsafe {
            library.get::<unsafe extern "C" fn(
                u32,
                *const libc::c_char,
                *const libc::c_char,
                u64,
                bool,
            ) -> i32>(b"krun_add_virtiofs3\0")?
        };
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
                filesystem(
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
        let exec = unsafe {
            library.get::<unsafe extern "C" fn(
                u32,
                *const libc::c_char,
                *const *const libc::c_char,
                *const *const libc::c_char,
            ) -> i32>(b"krun_set_exec\0")?
        };
        result("krun_set_exec", unsafe {
            exec(
                self.id,
                c"/bin/sh".as_ptr(),
                arguments.as_ptr(),
                environment.as_ptr(),
            )
        })?;
        Ok(())
    }

    fn call(&self, name: &[u8]) -> anyhow::Result<i32> {
        let function = unsafe { self.library.get::<unsafe extern "C" fn(u32) -> i32>(name)? };
        result(std::str::from_utf8(name)?.trim_end_matches('\0'), unsafe {
            function(self.id)
        })
    }
}

impl Drop for KrunContext<'_> {
    fn drop(&mut self) {
        let _ = self.call(b"krun_free_ctx\0");
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
    #[cfg(target_os = "macos")]
    let firmware_name = "libkrunfw.5.dylib";
    #[cfg(target_os = "linux")]
    let firmware_name = "libkrunfw.so.5";
    let firmware = configuration
        .library
        .parent()
        .context("libkrun requires a library directory")?
        .join(firmware_name);
    let _firmware = unsafe { libloading::Library::new(firmware) }
        .context("Cannot load Joe's bundled libkrun firmware")?;
    let library = unsafe { libloading::Library::new(&configuration.library) }
        .context("Cannot load libkrun 1.18.x and its libkrunfw dependency")?;
    let context = KrunContext::new(&library)?;
    context.configure(configuration)?;
    context.call(b"krun_start_enter\0")?;
    Err(anyhow::anyhow!(
        "libkrun returned without entering the guest"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_failures_prevent_boot_and_release_the_context() {
        let directory = std::env::temp_dir().join(format!("joe-krun-api-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("fixture.c");
        let library_path = directory.join("fixture.so");
        std::fs::write(&source, include_str!("krun_fixture.c")).unwrap();
        let compiled = std::process::Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(&source)
            .arg("-o")
            .arg(&library_path)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let library = unsafe { libloading::Library::new(&library_path).unwrap() };
        let fault = unsafe {
            library
                .get::<unsafe extern "C" fn(i32)>(b"joe_fault\0")
                .unwrap()
        };
        let state = unsafe {
            library
                .get::<unsafe extern "C" fn(i32) -> i32>(b"joe_state\0")
                .unwrap()
        };
        for failure in 0..=7 {
            unsafe { fault(failure) };
            let context = KrunContext::new(&library).unwrap();
            let result = context.configure(Configuration {
                library: library_path.clone(),
                rootfs: "/rootfs".into(),
                workspace: "/project".into(),
                temporary_name: uuid::Uuid::nil(),
            });
            match failure {
                0 => {
                    result.unwrap();
                    assert!(
                        context
                            .call(b"krun_start_enter\0")
                            .unwrap_err()
                            .to_string()
                            .contains("krun_start_enter")
                    );
                    assert_eq!(unsafe { state(1) }, 1);
                }
                _ => {
                    assert!(result.is_err());
                    assert_eq!(unsafe { state(1) }, 0);
                }
            }
            drop(context);
            assert_eq!(unsafe { state(2) }, 1);
            assert_eq!(unsafe { state(3) }, 0);
        }
        drop(library);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
