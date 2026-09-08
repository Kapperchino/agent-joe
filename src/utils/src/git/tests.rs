use super::*;
use crate::changes::ChangeTracker;

pub(crate) struct Fixture {
    pub root: PathBuf,
    pub workspace: WorkspacePolicy,
    pub repo: Repository,
}

impl Fixture {
    pub fn new() -> Self {
        initialize().unwrap();
        let root = std::env::temp_dir().join(format!("joe-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = Repository::init(&root).unwrap();
        let workspace = WorkspacePolicy::workspace(root.clone()).unwrap();
        Self {
            root,
            workspace,
            repo,
        }
    }

    pub fn write(&self, path: &str, content: &str) {
        self.workspace.write(Path::new(path), content).unwrap();
    }

    pub fn stage(&self, path: &str) {
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
    }

    pub fn commit(&self) -> git2::Oid {
        let mut index = self.repo.index().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("Joe test", "joe@example.invalid").unwrap();
        let parents = self
            .repo
            .head()
            .ok()
            .map(|head| head.peel_to_commit().unwrap())
            .into_iter()
            .collect::<Vec<_>>();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "fixture commit",
                &tree,
                &parents.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn git_reports_dirty_index_tree_untracked_rename_delete_and_literal_paths() {
    let fixture = Fixture::new();
    for path in [
        "dirty file.txt",
        "rename me.txt",
        "deleted.txt",
        "[literal].txt",
    ] {
        fixture.write(path, "base\n");
        fixture.stage(path);
    }
    let base = fixture.commit();
    fixture.write("dirty file.txt", "staged\n");
    fixture.stage("dirty file.txt");
    fixture.write("dirty file.txt", "working\n");
    std::fs::rename(
        fixture.root.join("rename me.txt"),
        fixture.root.join("renamed file.txt"),
    )
    .unwrap();
    let mut index = fixture.repo.index().unwrap();
    index.remove_path(Path::new("rename me.txt")).unwrap();
    index.add_path(Path::new("renamed file.txt")).unwrap();
    index.write().unwrap();
    std::fs::remove_file(fixture.root.join("deleted.txt")).unwrap();
    fixture.write("new\tfile.txt", "untracked\n");
    fixture.write("[literal].txt", "literal change\n");
    std::fs::create_dir(fixture.root.join(".turbo-code")).unwrap();
    std::fs::write(fixture.root.join(".turbo-code/secret"), "never expose this").unwrap();
    let git = GitRepository::required(&fixture.workspace).unwrap();
    let status = git.status(&fixture.workspace).unwrap();
    assert!(
        status
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("dirty file.txt")
                && entry.index == GitChange::Modified
                && entry.worktree == GitChange::Modified)
    );
    assert!(
        status
            .entries
            .iter()
            .any(|entry| entry.previous_path.as_deref() == Some(Path::new("rename me.txt")))
    );
    assert!(status.entries.iter().any(
        |entry| entry.path == Path::new("deleted.txt") && entry.worktree == GitChange::Deleted
    ));
    assert!(
        status
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("new\tfile.txt"))
    );
    let staged = git
        .diff(&fixture.workspace, DiffTarget::Staged, None)
        .unwrap();
    let unstaged = git
        .diff(&fixture.workspace, DiffTarget::Unstaged, None)
        .unwrap();
    assert!(staged.contains("+staged"));
    assert!(unstaged.contains("-staged\n+working"));
    assert!(unstaged.contains("+untracked"));
    assert!(!unstaged.contains("never expose"));
    let literal = git
        .diff(
            &fixture.workspace,
            DiffTarget::Head,
            Some(Path::new("[literal].txt")),
        )
        .unwrap();
    assert!(literal.contains("+literal change"));
    assert!(!literal.contains("dirty file"));
    let shown = GitRepository::execute(
        &fixture.workspace,
        GitOperation::Show {
            revision: Revision::new(&base.to_string()).unwrap(),
            path: Some("dirty file.txt".into()),
        },
    )
    .unwrap();
    assert!(matches!(shown, GitResult::Show { content, .. } if content == "base\n"));
    assert!(
        matches!(GitRepository::execute(&fixture.workspace, GitOperation::Log { revision: Revision::new("HEAD").unwrap(), limit: LogLimit::new(20).unwrap() }).unwrap(), GitResult::Log(commits) if commits.len() == 1)
    );
    let before = std::fs::read(fixture.root.join(".git/index")).unwrap();
    git.status(&fixture.workspace).unwrap();
    git.diff(&fixture.workspace, DiffTarget::Head, None)
        .unwrap();
    assert_eq!(
        before,
        std::fs::read(fixture.root.join(".git/index")).unwrap()
    );
}

#[test]
fn revisions_paths_and_external_object_stores_are_guarded() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.stage("file");
    fixture.commit();
    for revision in [
        "--output=/tmp/leak",
        "HEAD:file",
        "HEAD@{1}",
        "HEAD..main",
        "refs/../outside",
        "",
    ] {
        assert!(Revision::new(revision).is_err());
        assert!(serde_json::from_value::<Revision>(serde_json::json!(revision)).is_err());
    }
    for path in ["../outside", ".git/config", ".turbo-code/secret"] {
        assert!(
            GitRepository::execute(
                &fixture.workspace,
                GitOperation::Show {
                    revision: Revision::new("HEAD").unwrap(),
                    path: Some(path.into())
                }
            )
            .is_err()
        );
    }
    std::fs::write(
        fixture.root.join(".git/objects/info/alternates"),
        "/private/tmp/foreign-objects\n",
    )
    .unwrap();
    assert!(GitRepository::required(&fixture.workspace).is_err());
}

#[test]
fn repository_helpers_never_execute_and_worktrees_preserve_source_changes() {
    use worktrees::{DirtySource, ManagedWorktree, WorktreeOperation, WorktreeState};
    let fixture = Fixture::new();
    fixture.write("file.txt", "base\n");
    fixture.stage("file.txt");
    fixture.commit();
    let sentinel = fixture.root.join("helper-ran");
    let helper = format!("touch {}", sentinel.display());
    let mut config = fixture.repo.config().unwrap();
    for key in [
        "core.pager",
        "core.fsmonitor",
        "diff.external",
        "diff.evil.textconv",
        "filter.evil.clean",
        "filter.evil.smudge",
    ] {
        config.set_str(key, &helper).unwrap();
    }
    fixture.write(".gitattributes", "*.txt diff=evil filter=evil\n");
    fixture.stage(".gitattributes");
    fixture.commit();
    let hook = fixture.root.join(".git/hooks/post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\n{helper}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fixture.write("file.txt", "user change\n");
    fixture.stage("file.txt");
    let source_index = std::fs::read(fixture.root.join(".git/index")).unwrap();
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let create = |dirty| WorktreeOperation::Create {
        base: Revision::new("HEAD").unwrap(),
        dirty,
    };
    assert!(
        ManagedWorktree::execute(&fixture.workspace, &tracker, create(DirtySource::Reject))
            .is_err()
    );
    let record =
        ManagedWorktree::execute(&fixture.workspace, &tracker, create(DirtySource::BaseOnly))
            .unwrap()
            .remove(0);
    assert!(record.dirty_source);
    let base_commit = fixture.repo.head().unwrap().peel_to_commit().unwrap();
    fixture
        .repo
        .branch("user-branch", &base_commit, false)
        .unwrap();
    let saved = tracker.snapshot().unwrap().worktrees;
    let mut changed = saved.clone();
    changed[0].branch = "user-branch".into();
    tracker.update_worktrees(changed).unwrap();
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Remove {
                id: record.id.clone()
            }
        )
        .is_err()
    );
    assert!(record.path.exists());
    assert!(
        fixture
            .repo
            .find_branch("user-branch", git2::BranchType::Local)
            .is_ok()
    );
    tracker.update_worktrees(saved).unwrap();
    assert_eq!(
        std::fs::read_to_string(record.path.join("file.txt")).unwrap(),
        "base\n"
    );
    assert!(!sentinel.exists());
    std::fs::write(record.path.join("file.txt"), "isolated change\n").unwrap();
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Integrate {
                id: record.id.clone()
            }
        )
        .is_err()
    );
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Remove {
                id: record.id.clone()
            }
        )
        .is_err()
    );
    assert_eq!(
        source_index,
        std::fs::read(fixture.root.join(".git/index")).unwrap()
    );
    assert_eq!(
        fixture.workspace.read(Path::new("file.txt")).unwrap(),
        "user change\n"
    );
    std::fs::write(record.path.join("file.txt"), "base\n").unwrap();
    let removed = ManagedWorktree::execute(
        &fixture.workspace,
        &tracker,
        WorktreeOperation::Remove { id: record.id },
    )
    .unwrap();
    assert!(matches!(removed[0].state, WorktreeState::Removed));
    assert!(!record.path.exists());
    assert!(!sentinel.exists());
}

#[test]
fn worktree_integration_is_guarded_journaled_and_undoable() {
    use worktrees::{DirtySource, ManagedWorktree, WorktreeOperation};
    let fixture = Fixture::new();
    fixture.write("original.txt", "base\n");
    fixture.stage("original.txt");
    fixture.commit();
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let record = ManagedWorktree::execute(
        &fixture.workspace,
        &tracker,
        WorktreeOperation::Create {
            base: Revision::new("HEAD").unwrap(),
            dirty: DirtySource::Reject,
        },
    )
    .unwrap()
    .remove(0);
    std::fs::rename(
        record.path.join("original.txt"),
        record.path.join("renamed file.txt"),
    )
    .unwrap();
    std::fs::write(record.path.join("new file"), "new\n").unwrap();
    ManagedWorktree::execute(
        &fixture.workspace,
        &tracker,
        WorktreeOperation::Integrate {
            id: record.id.clone(),
        },
    )
    .unwrap();
    assert!(!fixture.root.join("original.txt").exists());
    assert_eq!(
        fixture.workspace.read(Path::new("new file")).unwrap(),
        "new\n"
    );
    let edit = tracker
        .snapshot()
        .unwrap()
        .records
        .last()
        .unwrap()
        .id
        .clone();
    std::fs::write(record.path.join("ignored.tmp"), "retain this").unwrap();
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Remove {
                id: record.id.clone()
            }
        )
        .is_err()
    );
    std::fs::remove_file(record.path.join("ignored.tmp")).unwrap();
    ManagedWorktree::execute(
        &fixture.workspace,
        &tracker,
        WorktreeOperation::Remove { id: record.id },
    )
    .unwrap();
    tracker.undo(&fixture.workspace, &edit).unwrap();
    assert_eq!(
        fixture.workspace.read(Path::new("original.txt")).unwrap(),
        "base\n"
    );
    assert!(!fixture.root.join("new file").exists());
    assert!(!fixture.root.join("renamed file.txt").exists());
}

#[cfg(unix)]
#[test]
fn replaced_worktree_directories_and_config_includes_cannot_expand_access() {
    use worktrees::{DirtySource, ManagedWorktree, WorktreeOperation};
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.stage("file");
    fixture.commit();
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let record = ManagedWorktree::execute(
        &fixture.workspace,
        &tracker,
        WorktreeOperation::Create {
            base: Revision::new("HEAD").unwrap(),
            dirty: DirtySource::Reject,
        },
    )
    .unwrap()
    .remove(0);
    let outside = Fixture::new();
    outside.write("file", "outside\n");
    std::fs::rename(&record.path, fixture.root.join("retained-worktree")).unwrap();
    std::os::unix::fs::symlink(&outside.root, &record.path).unwrap();
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Integrate {
                id: record.id.clone()
            }
        )
        .is_err()
    );
    assert!(
        ManagedWorktree::execute(
            &fixture.workspace,
            &tracker,
            WorktreeOperation::Remove { id: record.id }
        )
        .is_err()
    );
    assert_eq!(
        outside.workspace.read(Path::new("file")).unwrap(),
        "outside\n"
    );
    fixture
        .repo
        .config()
        .unwrap()
        .set_str("include.path", outside.root.join("file").to_str().unwrap())
        .unwrap();
    let error = GitRepository::required(&fixture.workspace).err().unwrap();
    assert!(error.to_string().contains("includes"));
}
