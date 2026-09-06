use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use tokio::process::Command;

pub(super) struct Filter {
    file: File,
}

impl Filter {
    pub(super) fn new() -> anyhow::Result<Self> {
        let mut instructions = Vec::new();
        instructions.extend_from_slice(&instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            std::mem::offset_of!(libc::seccomp_data, arch) as u32,
        ));
        let architecture = if cfg!(all(target_arch = "aarch64", target_endian = "little")) {
            Ok(0xc00000b7)
        } else if cfg!(target_arch = "x86_64") {
            Ok(0xc000003e)
        } else {
            Err(anyhow::anyhow!(
                "Syscall isolation is unsupported on this architecture"
            ))
        }?;
        instructions.extend_from_slice(&instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            1,
            0,
            architecture,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_KILL_PROCESS,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            std::mem::offset_of!(libc::seccomp_data, nr) as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K,
            0,
            1,
            0x40000000,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_KILL_PROCESS,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            3,
            libc::SYS_clone as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            std::mem::offset_of!(libc::seccomp_data, args) as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K,
            0,
            1,
            (libc::CLONE_NEWCGROUP
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWNET
                | libc::CLONE_NEWNS
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWUSER
                | libc::CLONE_NEWUTS) as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            std::mem::offset_of!(libc::seccomp_data, nr) as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            1,
            libc::SYS_clone3 as u32,
        ));
        instructions.extend_from_slice(&instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
        ));
        let denied = vec![
            libc::SYS_socket,
            libc::SYS_linkat,
            libc::SYS_mknodat,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_io_uring_setup,
            libc::SYS_open_by_handle_at,
            libc::SYS_unshare,
            libc::SYS_setns,
        ];
        #[cfg(target_arch = "x86_64")]
        let denied = {
            let mut denied = denied;
            denied.extend([libc::SYS_link, libc::SYS_mknod]);
            denied
        };
        for syscall in denied {
            instructions.extend_from_slice(&instruction(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                0,
                1,
                syscall as u32,
            ));
            instructions.extend_from_slice(&instruction(
                libc::BPF_RET | libc::BPF_K,
                0,
                0,
                libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
            ));
        }
        instructions.extend_from_slice(&instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_ALLOW,
        ));
        let descriptor = unsafe {
            libc::memfd_create(
                c"joe-seccomp".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            let mut file = unsafe { File::from_raw_fd(descriptor) };
            file.write_all(&instructions)?;
            file.rewind()?;
            let sealed = unsafe {
                libc::fcntl(
                    file.as_raw_fd(),
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_WRITE
                        | libc::F_SEAL_GROW
                        | libc::F_SEAL_SHRINK
                        | libc::F_SEAL_SEAL,
                )
            };
            if sealed < 0 {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(Self { file })
            }
        }
    }

    pub(super) fn attach(&self, command: &mut Command) {
        let descriptor = self.file.as_raw_fd();
        command.arg("--seccomp").arg(descriptor.to_string());
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(descriptor, libc::F_SETFD, 0) < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn instruction(code: u32, yes: u8, no: u8, value: u32) -> [u8; 8] {
    let code = (code as u16).to_ne_bytes();
    let value = value.to_ne_bytes();
    [
        code[0], code[1], yes, no, value[0], value[1], value[2], value[3],
    ]
}
