use super::{
    artifact::{ArchivePath, Artifact, Checksum},
    directory::PrivateDirectory,
    download::{Downloads, Installation},
    platform::Platform,
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("joe-provision-{}", uuid::Uuid::new_v4()));
        PrivateDirectory::new(path.clone()).unwrap();
        Self {
            path: path.canonicalize().unwrap(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn incomplete_installations_are_replaced_and_completed_installations_are_reused() {
    let fixture = Fixture::new();
    let installation = Installation::new(fixture.path.clone(), &|| Ok(())).unwrap();
    assert!(
        installation
            .prepare("runtime", |staging| {
                fs::write(staging.join("interrupted"), "partial")?;
                Err(anyhow::anyhow!("cancelled"))
            })
            .is_err()
    );
    assert!(!fixture.path.join("runtime").exists());
    let completed = installation
        .prepare("runtime", |staging| {
            assert!(!staging.join("interrupted").exists());
            fs::write(staging.join("ready"), "complete")?;
            Ok(())
        })
        .unwrap();
    assert!(completed.join("complete").is_file());
    let reused = installation
        .prepare("runtime", |_| panic!("completed installation was rebuilt"))
        .unwrap();
    assert_eq!(completed, reused);
}

#[test]
fn waiting_for_another_installer_can_be_cancelled() {
    let fixture = Fixture::new();
    let _installation = Installation::new(fixture.path.clone(), &|| Ok(())).unwrap();
    let attempts = std::cell::Cell::new(0);
    let result = Installation::new(fixture.path.clone(), &|| {
        attempts.set(attempts.get() + 1);
        match attempts.get() {
            1 => Ok(()),
            _ => Err(anyhow::anyhow!("cancelled")),
        }
    });
    assert!(result.err().unwrap().to_string().contains("cancelled"));
    assert_eq!(attempts.get(), 2);
}

#[test]
fn concurrent_installers_publish_once() {
    let fixture = Fixture::new();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let threads = (0..4)
        .map(|_| {
            let path = fixture.path.clone();
            let count = count.clone();
            std::thread::spawn(move || {
                let installation = Installation::new(path, &|| Ok(())).unwrap();
                installation
                    .prepare("runtime", |_| {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    })
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        assert!(thread.join().unwrap().join("complete").is_file());
    }
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn only_verified_downloads_are_reused_offline() {
    let fixture = Fixture::new();
    let downloads = Downloads::new(fixture.path.clone(), &|| Ok(())).unwrap();
    let contents = b"pinned archive";
    let digest = format!("{:x}", Sha256::digest(contents));
    fs::write(fixture.path.join(&digest), contents).unwrap();
    let path = downloads
        .get(
            &Artifact::new("https://127.0.0.1:1/unavailable", &digest).unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), contents);
    fs::write(fixture.path.join(&digest), "corrupted").unwrap();
    assert!(
        downloads
            .get(
                &Artifact::new("https://127.0.0.1:1/unavailable", &digest).unwrap(),
                None
            )
            .is_err()
    );
    assert!(Artifact::new("http://127.0.0.1:1/unavailable", &digest).is_err());
    assert!(Checksum::new("../escape").is_err());
}

#[test]
fn caches_reject_shared_directories_and_symlinks() {
    let fixture = Fixture::new();
    let shared = fixture.path.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Installation::new(shared, &|| Ok(())).is_err());
    let link = fixture.path.join("link");
    std::os::unix::fs::symlink(&fixture.path, &link).unwrap();
    assert!(Installation::new(link, &|| Ok(())).is_err());
}

#[test]
fn archives_cannot_write_through_symlinks_outside_the_destination() {
    let fixture = Fixture::new();
    let outside = fixture.path.join("outside");
    fs::create_dir(&outside).unwrap();
    let archive = fixture.path.join("archive.tar.gz");
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        flate2::Compression::default(),
    );
    let mut builder = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    builder
        .append_link(&mut header, "escape", &outside)
        .unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "escape/secret", b"oops".as_slice())
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    let downloads = Downloads::new(fixture.path.join("downloads"), &|| Ok(())).unwrap();
    assert!(
        downloads
            .unpack(&archive, &fixture.path.join("rootfs"))
            .is_err()
    );
    assert!(!outside.join("secret").exists());
}

#[test]
fn artifact_paths_and_platforms_reject_unsupported_inputs() {
    for path in ["/absolute", "../escape", "safe/../../escape"] {
        assert!(ArchivePath::new(path.into()).is_err());
    }
    assert!(ArchivePath::new("usr/local/bin/cargo".into()).is_ok());
    assert!(Platform::new("macos", "x86_64").is_err());
    assert!(Platform::new("linux", "riscv64").is_err());
    assert!(Platform::new("windows", "x86_64").is_err());
    assert!(Platform::new("linux", "aarch64").is_ok());
    assert!(Platform::new("linux", "x86_64").is_ok());
    assert!(Platform::new("macos", "aarch64").is_ok());
}
