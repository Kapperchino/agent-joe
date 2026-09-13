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
        if storage.read_file("generations.json")?.is_none() {
            ["data.mdb", "lock.mdb"]
                .into_iter()
                .try_for_each(|name| storage.open_file(name).map(drop))?;
        }
        storage.directory.sync_all()?;
        Ok(storage)
    }
}

impl PrivateStorage {
    pub fn open_file(&self, filename: &str) -> anyhow::Result<File> {
        let filename = StorageName::new(filename)?;
        let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        let file = match fs::openat(
            &self.directory,
            filename.0,
            flags | OFlags::CREATE | OFlags::EXCL,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(file) => Ok(file),
            Err(rustix::io::Errno::EXIST) => {
                fs::openat(&self.directory, filename.0, flags, Mode::empty())
            }
            Err(error) => Err(error),
        }
        .map_err(io_error)?;
        OrdinaryFileMetadata::new(fs::fstat(&file).map_err(io_error)?, Path::new(filename.0))?;
        fs::fchmod(&file, Mode::from_raw_mode(0o600)).map_err(io_error)?;
        Ok(File::from(file))
    }

    pub fn read_file(&self, filename: &str) -> anyhow::Result<Option<File>> {
        let filename = StorageName::new(filename)?;
        let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        match fs::openat(&self.directory, filename.0, flags, Mode::empty()) {
            Ok(file) => {
                OrdinaryFileMetadata::new(
                    fs::fstat(&file).map_err(io_error)?,
                    Path::new(filename.0),
                )?;
                Ok(Some(File::from(file)))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    pub fn child(&self, name: &str) -> anyhow::Result<Self> {
        Ok(Self {
            directory: private_directory(&self.directory, StorageName::new(name)?.0)?,
            path: self.path.join(name),
            workspace_identity: self.workspace_identity.clone(),
        })
    }

    pub fn publish_file(&self, temporary: &str, name: &str) -> anyhow::Result<()> {
        let temporary = StorageName::new(temporary)?;
        let name = StorageName::new(name)?;
        fs::renameat(&self.directory, temporary.0, &self.directory, name.0).map_err(io_error)?;
        self.directory.sync_all().map_err(Into::into)
    }

    pub fn replace_file(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let temporary = format!("{}.tmp", StorageName::new(name)?.0);
        let mut file = self.open_file(&temporary)?;
        file.set_len(0)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.publish_file(&temporary, name)
    }

    pub fn remove_file(&self, name: &str) -> anyhow::Result<()> {
        let name = StorageName::new(name)?;
        match fs::unlinkat(&self.directory, name.0, AtFlags::empty()) {
            Ok(()) => self.directory.sync_all().map_err(Into::into),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    pub fn remove_child(&self, name: &str) -> anyhow::Result<()> {
        let name = StorageName::new(name)?;
        match fs::unlinkat(&self.directory, name.0, AtFlags::REMOVEDIR) {
            Ok(()) => self.directory.sync_all().map_err(Into::into),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    pub fn sync(&self) -> anyhow::Result<()> {
        self.directory.sync_all().map_err(Into::into)
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
