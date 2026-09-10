use super::runtime::Runtime;
use super::*;
use std::ffi::OsString;
use std::path::Path;

struct Profile {
    rules: String,
    parameters: Vec<OsString>,
}

impl Profile {
    fn new(
        workspace: &WorkspacePolicy,
        executable: &Path,
        runtime: &Runtime,
        temporary: &TemporaryDirectory,
    ) -> anyhow::Result<Self> {
        let mut profile = Self {
                rules: r#"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target self))
(allow sysctl-read)
(allow file-read-metadata)
(allow file-read* (literal "/") (subpath "/usr/lib") (subpath "/System/Library") (subpath "/System/Cryptexes") (subpath "/System/Volumes/Preboot/Cryptexes") (subpath "/Library/Apple") (subpath "/private/var/db/dyld") (subpath "/private/preboot/Cryptexes") (literal "/dev/null") (literal "/dev/random") (literal "/dev/urandom"))
(allow file-write-data (literal "/dev/null"))
"#.into(),
                parameters: Vec::new(),
            };
        profile.path(
            "allow",
            "file-read* file-write*",
            "subpath",
            workspace.root(),
        );
        profile.path("allow", "file-read*", "literal", executable);
        for path in runtime.read_only_paths()? {
            profile.path("allow", "file-read*", "subpath", path);
        }
        let temporary = profile.parameter(temporary.path());
        profile.rules.push_str(&format!(
            r#"(deny file-link)
(deny file-read* file-write* (require-all
    (regex #"(^|/)[.]([tT][uU][rR][bB][oO]-[cC][oO][dD][eE])(/|$)")
    (require-not (subpath (param "{temporary}")))))
(deny file-write* (require-all
    (regex #"(^|/)[.]([gG][iI][tT])(/|$)")
    (require-not (subpath (param "{temporary}")))))
(deny file-write* (regex #"(^|/)[.]([aA][gG][eE][nN][tT][sS]|[cC][oO][dD][eE][xX])(/|$)"))
"#
        ));
        for root in workspace.read_only_roots() {
            profile.path("deny", "file-write*", "subpath", root);
        }
        Ok(profile)
    }

    fn path(&mut self, decision: &str, operations: &str, filter: &str, path: &Path) {
        let name = self.parameter(path);
        self.rules.push_str(&format!(
            "({decision} {operations} ({filter} (param \"{name}\")))\n"
        ));
    }

    fn parameter(&mut self, path: &Path) -> String {
        let name = format!("PATH_{}", self.parameters.len());
        let mut parameter = OsString::from(format!("{name}="));
        parameter.push(path);
        self.parameters.push(parameter);
        name
    }
}

pub(super) fn prepare(
    command: Command,
    workspace: &WorkspacePolicy,
    runtime: &Runtime,
    temporary: &TemporaryDirectory,
) -> anyhow::Result<Command> {
    let source = command.as_std();
    let profile = Profile::new(workspace, &runtime.helper, runtime, temporary)?;
    let mut isolated = Command::new("/usr/bin/sandbox-exec");
    isolated.env_clear().current_dir(workspace.root());
    for parameter in profile.parameters {
        isolated.arg("-D").arg(parameter);
    }
    isolated
        .args(["-p", &profile.rules])
        .arg(source.get_program())
        .args(source.get_args());
    Ok(isolated)
}
