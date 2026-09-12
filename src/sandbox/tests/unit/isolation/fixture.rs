use super::*;

pub(crate) fn command(
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
            helper: path,
        },
        #[cfg(target_os = "linux")]
        _filter: seccomp::Filter::new()?,
    })
}
