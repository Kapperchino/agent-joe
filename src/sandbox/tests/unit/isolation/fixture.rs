use super::*;

pub fn command(
    command: Command,
    workspace: &dyn Workspace,
) -> anyhow::Result<IsolatedCommand> {
    let path = workspace.root().to_path_buf();
    Ok(IsolatedCommand {
        command,
        temporary: TemporaryDirectory::new(workspace)?,
        runtime: runtime::Runtime {
            rootfs: path.clone(),
            firmware: path.clone(),
            init: path.clone(),
            cache: cache::BuildCache::new(path.parent().unwrap().join("cache"), &path)?,
            helper: path,
        },
        #[cfg(target_os = "linux")]
        _filter: seccomp::Filter::new()?,
    })
}
