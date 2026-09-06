use super::*;

pub struct PrivateStorage {
    directory: File,
    path: PathBuf,
    workspace_identity: String,
}

struct StorageName<'a>(&'a str);

impl<'a> StorageName<'a> {
    fn new(value: &'a str) -> anyhow::Result<Self> {
        let valid = !value.is_empty()
            && value.len() <= 100
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
        valid
            .then_some(Self(value))
            .ok_or_else(|| anyhow::anyhow!("Invalid storage name"))
    }
}

fn private_directory(parent: &File, name: &str) -> anyhow::Result<File> {
    let name = StorageName::new(name)?;
    match fs::mkdirat(parent, name.0, Mode::from_raw_mode(0o700)) {
        Ok(()) => parent.sync_all()?,
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => Err(io_error(error))?,
    }
    let directory =
        File::from(fs::openat(parent, name.0, directory_flags(), Mode::empty()).map_err(io_error)?);
    fs::fchmod(&directory, Mode::from_raw_mode(0o700)).map_err(io_error)?;
    Ok(directory)
}

impl WorkspacePolicy {
    pub fn workspace_identity(&self) -> anyhow::Result<String> {
        let resolved = self.resolve(&self.base, Access::Read)?;
        resolved.root.validate_identity()?;
        let root = resolved.directory(&resolved.relative, Parents::Existing)?;
        let identity = fs::fstat(&root).map_err(io_error)?;
        Ok(format!(
            "{}:{}:{}",
            self.base.display(),
            identity.st_dev,
            identity.st_ino
        ))
    }

    pub fn session_storage(&self, namespace: &str) -> anyhow::Result<PrivateStorage> {
        let namespace = StorageName::new(namespace)?;
        let resolved = self.resolve(&self.base, Access::Write)?;
        resolved.root.validate_identity()?;
        let root = resolved.directory(&resolved.relative, Parents::Existing)?;
        let directory = private_directory(&root, crate::utils::CONFIG_DIR_NAME)?;
        let directory = private_directory(&directory, namespace.0)?;
        let storage = PrivateStorage {
            directory,
            path: self
                .base
                .join(crate::utils::CONFIG_DIR_NAME)
                .join(namespace.0),
            workspace_identity: self.workspace_identity()?,
        };
        for name in ["data.mdb", "lock.mdb"] {
            storage.prepare_database_file(name)?;
        }
        storage.directory.sync_all()?;
        Ok(storage)
    }
}

impl PrivateStorage {
    fn prepare_database_file(&self, filename: &str) -> anyhow::Result<()> {
        let filename = StorageName::new(filename)?;
        match fs::openat(
            &self.directory,
            filename.0,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(_) | Err(rustix::io::Errno::EXIST) => Ok(()),
            Err(error) => Err(io_error(error)),
        }?;
        let stat =
            fs::statat(&self.directory, filename.0, AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
        OrdinaryFileMetadata::new(stat, Path::new(filename.0))?;
        fs::chmodat(
            &self.directory,
            filename.0,
            Mode::from_raw_mode(0o600),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(io_error)
    }

    pub fn workspace_identity(&self) -> &str {
        &self.workspace_identity
    }

    pub fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
