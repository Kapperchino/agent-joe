use super::provision::platform::{Architecture, Platform};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::AsRawFd;
use tokio::process::Command;

pub(super) struct Filter {
    file: File,
}

impl Filter {
    pub(super) fn new() -> anyhow::Result<Self> {
        let architecture = match (
            Platform::current()?.architecture(),
            cfg!(target_endian = "little"),
        ) {
            (Architecture::Arm64, true) => Ok(0xc00000b7),
            (Architecture::Amd64, true) => Ok(0xc000003e),
            _ => Err(anyhow::anyhow!(
                "Syscall isolation is unsupported on this architecture"
            )),
        }?;
        let denied = [
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
            #[cfg(target_arch = "x86_64")]
            libc::SYS_link,
            #[cfg(target_arch = "x86_64")]
            libc::SYS_mknod,
        ];
        let instructions: Vec<u8> = [
            Instruction {
                code: libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                on_match: 0,
                on_mismatch: 0,
                value: std::mem::offset_of!(libc::seccomp_data, arch) as u32,
            },
            Instruction {
                code: libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                on_match: 1,
                on_mismatch: 0,
                value: architecture,
            },
            Instruction {
                code: libc::BPF_RET | libc::BPF_K,
                on_match: 0,
                on_mismatch: 0,
                value: libc::SECCOMP_RET_KILL_PROCESS,
            },
            Instruction {
                code: libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                on_match: 0,
                on_mismatch: 0,
                value: std::mem::offset_of!(libc::seccomp_data, nr) as u32,
            },
            Instruction {
                code: libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K,
                on_match: 0,
                on_mismatch: 1,
                value: 0x40000000,
            },
            Instruction {
                code: libc::BPF_RET | libc::BPF_K,
                on_match: 0,
                on_mismatch: 0,
                value: libc::SECCOMP_RET_KILL_PROCESS,
            },
            Instruction {
                code: libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                on_match: 0,
                on_mismatch: 3,
                value: libc::SYS_clone as u32,
            },
            Instruction {
                code: libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                on_match: 0,
                on_mismatch: 0,
                value: std::mem::offset_of!(libc::seccomp_data, args) as u32,
            },
            Instruction {
                code: libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K,
                on_match: 0,
                on_mismatch: 1,
                value: (libc::CLONE_NEWCGROUP
                    | libc::CLONE_NEWIPC
                    | libc::CLONE_NEWNET
                    | libc::CLONE_NEWNS
                    | libc::CLONE_NEWPID
                    | libc::CLONE_NEWUSER
                    | libc::CLONE_NEWUTS) as u32,
            },
            Instruction {
                code: libc::BPF_RET | libc::BPF_K,
                on_match: 0,
                on_mismatch: 0,
                value: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
            },
            Instruction {
                code: libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                on_match: 0,
                on_mismatch: 0,
                value: std::mem::offset_of!(libc::seccomp_data, nr) as u32,
            },
            Instruction {
                code: libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                on_match: 0,
                on_mismatch: 1,
                value: libc::SYS_clone3 as u32,
            },
            Instruction {
                code: libc::BPF_RET | libc::BPF_K,
                on_match: 0,
                on_mismatch: 0,
                value: libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
            },
        ]
        .into_iter()
        .chain(denied.into_iter().flat_map(|syscall| {
            [
                Instruction {
                    code: libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                    on_match: 0,
                    on_mismatch: 1,
                    value: syscall as u32,
                },
                Instruction {
                    code: libc::BPF_RET | libc::BPF_K,
                    on_match: 0,
                    on_mismatch: 0,
                    value: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                },
            ]
        }))
        .chain([Instruction {
            code: libc::BPF_RET | libc::BPF_K,
            on_match: 0,
            on_mismatch: 0,
            value: libc::SECCOMP_RET_ALLOW,
        }])
        .flat_map(Instruction::bytes)
        .collect();
        let mut file = File::from(memfd_create(
            c"joe-seccomp",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?);
        file.write_all(&instructions)?;
        file.rewind()?;
        fcntl_add_seals(
            &file,
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
        )?;
        Ok(Self { file })
    }

    pub(super) fn attach(&self, command: &mut Command) {
        let descriptor = self.file.as_raw_fd();
        command.arg("--seccomp").arg(descriptor.to_string());
        unsafe {
            command.pre_exec(move || match libc::fcntl(descriptor, libc::F_SETFD, 0) {
                0.. => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            });
        }
    }
}

struct Instruction {
    code: u32,
    on_match: u8,
    on_mismatch: u8,
    value: u32,
}

impl Instruction {
    fn bytes(self) -> [u8; 8] {
        let code = (self.code as u16).to_ne_bytes();
        let value = self.value.to_ne_bytes();
        [
            code[0],
            code[1],
            self.on_match,
            self.on_mismatch,
            value[0],
            value[1],
            value[2],
            value[3],
        ]
    }
}
