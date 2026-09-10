Joe prepares its sandbox automatically. Its launcher depends directly on the
pinned `libkrun` 1.18.0 crate, imported as `krun`, and the application bundles
libkrunfw 5.5.0. On first use it downloads the pinned Rust 1.95
Linux guest, installs rustfmt and Clippy, prepares crates.io dependencies,
and signs the launcher on macOS. Cargo resolves missing lockfiles and changed
dependencies inside the guest, preserving existing dependency versions. Subsequent commands reuse the private cache.
There are no sandbox environment variables, Docker builds or installation steps.

Both Linux (KVM, x86-64 or ARM64) and Apple Silicon macOS
(Hypervisor.framework) use the same launcher, VM configuration, guest image
preparation and command protocol. Each command gets a fresh Linux microVM with
two CPUs, 4 GiB RAM, a read-only root filesystem and the project at `/workspace`.
Build artifacts live in `target/.joe/linux/build`. Cargo builds Linux programs
on both hosts. Guest commands run offline; Joe downloads registry data on the
host without executing project code there.

Native components and guest images have pinned SHA-256 checksums. OCI layers are authenticated by the
pinned image manifest; crate archives are checked against `Cargo.lock`. A private
cache lock serializes preparation, and completed installations are published by
rename. Interrupted installations are rebuilt on the next attempt. The cache
lives in the operating system's user cache directory, outside the workspace.

The launcher disables implicit vsock, including Transparent Socket Impersonation,
and configures no network devices or port mappings. Separate virtio-console
ports carry stdout and stderr. Cancellation, timeout and output overflow kill
the VMM and every guest process, including detached sessions.

Libkrun's virtiofs backend requires host filesystem isolation. Bubblewrap and
seccomp on Linux, and Seatbelt on macOS, protect workspace metadata, private
session files, read-only roots and files outside the workspace. Joe bundles
Bubblewrap on Linux. The host policies expose runtime libraries; build tools
run inside the guest. Virtualization and the host's isolation facilities must
be permitted by the operating system; unavailable isolation fails closed.

The Cargo build automatically compiles and bundles the launcher and firmware for
its host platform. Cargo resolves the VMM through the launcher's lockfile. Joe
prepares the crate's Linux init executable and ARM firmware build inputs, and
loads the bundled libkrunfw before creating a VM. This build fetches pinned upstream sources and
uses the normal Rust and C build toolchains. Application users need no compiler
or separately installed sandbox helper. The launcher shares Joe's protocol and
sandbox configuration code across Linux and macOS.
