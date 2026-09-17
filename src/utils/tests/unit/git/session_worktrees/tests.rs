use super::*;
use std::path::Path;

struct Fixture {
    root: PathBuf,
    workspace: WorkspacePolicy,
    repo: git2::Repository,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("joe-session-worktree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = git2::Repository::init(&root).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let workspace = WorkspacePolicy::workspace(root.clone()).unwrap();
        let fixture = Self {
            root,
            workspace,
            repo,
        };
        fixture.commit("file.txt", "base\n");
        fixture
    }

    fn commit(&self, path: &str, text: &str) -> Oid {
        std::fs::write(self.root.join(path), text).unwrap();
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let signature = Signature::now("Fixture", "fixture@example.com").unwrap();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "fixture",
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }

    fn session(&self) -> SessionWorktree {
        SessionWorktree::create(&self.workspace, &uuid::Uuid::new_v4().to_string(), None)
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn merge_ignores_missing_unrelated_worktrees() {
    let fixture = Fixture::new();
    let missing = fixture.session();
    std::fs::write(missing.path.join("file.txt"), "unmerged work\n").unwrap();
    let retained = missing.proposal(&fixture.workspace).unwrap().unwrap();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    std::fs::remove_dir_all(&missing.path).unwrap();

    assert!(matches!(
        session.merge(&fixture.workspace, &approved).unwrap(),
        MergeOutcome::Merged { .. }
    ));
    assert_eq!(
        fixture
            .repo
            .refname_to_id("refs/heads/main")
            .unwrap()
            .to_string(),
        approved
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "session\n"
    );
    assert_eq!(
        fixture
            .repo
            .refname_to_id(&format!("refs/heads/{}", missing.branch()))
            .unwrap()
            .to_string(),
        retained
    );
    assert!(fixture.repo.find_worktree(missing.id()).is_ok());
}

#[test]
fn cleanup_ignores_missing_unrelated_worktrees() {
    let fixture = Fixture::new();
    let missing = fixture.session();
    let reference = format!("refs/heads/{}", missing.branch());
    let retained = fixture.repo.refname_to_id(&reference).unwrap();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    std::fs::remove_dir_all(&missing.path).unwrap();

    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_worktree(session.id()).is_err());
    assert!(
        fixture
            .repo
            .find_reference(&format!("refs/heads/{}", session.branch()))
            .is_err()
    );
    assert_eq!(fixture.repo.refname_to_id(&reference).unwrap(), retained);
    assert!(fixture.repo.find_worktree(missing.id()).is_ok());
}

#[test]
fn missing_locked_worktrees_still_block_merge_and_cleanup() {
    let fixture = Fixture::new();
    let missing = fixture.session();
    let worktree = fixture.repo.find_worktree(missing.id()).unwrap();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let main = fixture.repo.refname_to_id("refs/heads/main").unwrap();
    worktree.lock(Some("temporarily unavailable")).unwrap();
    std::fs::remove_dir_all(&missing.path).unwrap();

    assert!(session.merge(&fixture.workspace, &approved).is_err());
    assert_eq!(fixture.repo.refname_to_id("refs/heads/main").unwrap(), main);
    worktree.unlock().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    worktree.lock(Some("temporarily unavailable")).unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    worktree.unlock().unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_worktree(missing.id()).is_ok());
}

#[test]
fn invalid_existing_worktrees_still_block_merge_and_cleanup() {
    let fixture = Fixture::new();
    let other = fixture.session();
    let gitfile = other.path.join(".git");
    let metadata = std::fs::read(&gitfile).unwrap();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let main = fixture.repo.refname_to_id("refs/heads/main").unwrap();
    std::fs::write(&gitfile, "invalid Git metadata\n").unwrap();

    assert!(session.merge(&fixture.workspace, &approved).is_err());
    assert_eq!(fixture.repo.refname_to_id("refs/heads/main").unwrap(), main);
    std::fs::write(&gitfile, &metadata).unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    std::fs::write(&gitfile, "invalid Git metadata\n").unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    std::fs::write(&gitfile, &metadata).unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(other.workspace(&fixture.workspace).is_ok());
}

#[test]
fn merge_refuses_main_checked_out_in_another_worktree() {
    let fixture = Fixture::new();
    let other = fixture.session();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let main = fixture.repo.refname_to_id("refs/heads/main").unwrap();
    let linked = git2::Repository::open(&other.path).unwrap();
    std::fs::write(linked.path().join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let error = session.merge(&fixture.workspace, &approved).err().unwrap();
    assert!(
        format!("{error:#}").contains("main is checked out in another worktree"),
        "{error:#}"
    );
    assert_eq!(fixture.repo.refname_to_id("refs/heads/main").unwrap(), main);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "base\n"
    );
    assert!(session.workspace(&fixture.workspace).is_ok());
}

#[test]
fn fast_forward_commit_summarizes_added_updated_and_removed_files() {
    let fixture = Fixture::new();
    fixture.commit("removed.txt", "remove me\n");
    let session = fixture.session();
    std::fs::write(session.path.join("added.txt"), "new\n").unwrap();
    std::fs::write(session.path.join("file.txt"), "updated\n").unwrap();
    std::fs::remove_file(session.path.join("removed.txt")).unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    assert_eq!(
        session.proposal(&fixture.workspace).unwrap().unwrap(),
        approved
    );
    session.merge(&fixture.workspace, &approved).unwrap();
    let commit = fixture.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.id().to_string(), approved);
    assert_eq!(commit.parent_count(), 1);
    assert_eq!(
        commit.message().unwrap(),
        "Add added.txt; update file.txt; remove removed.txt"
    );
}

#[test]
fn divergent_merge_summarizes_all_session_changes_but_not_main_only_changes() {
    let fixture = Fixture::new();
    fixture.commit("removed.txt", "remove me\n");
    let session = fixture.session();
    std::fs::write(session.path.join("added.txt"), "new\n").unwrap();
    session.proposal(&fixture.workspace).unwrap().unwrap();
    std::fs::write(session.path.join("file.txt"), "updated\n").unwrap();
    std::fs::remove_file(session.path.join("removed.txt")).unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let target = fixture.commit("main-only.txt", "main work\n");
    session.merge(&fixture.workspace, &approved).unwrap();
    let commit = fixture.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.parent_count(), 2);
    assert_eq!(commit.parent_id(0).unwrap(), target);
    assert_eq!(commit.parent_id(1).unwrap().to_string(), approved);
    assert_eq!(
        commit.message().unwrap(),
        "Add added.txt; update file.txt; remove removed.txt"
    );
}

#[test]
fn fallback_commit_summaries_use_brief_file_counts() {
    let fixture = Fixture::new();
    fixture.commit("removed.txt", "remove me\n");
    let session = fixture.session();
    for number in 0..10 {
        std::fs::write(session.path.join(format!("added-{number}.txt")), "new\n").unwrap();
    }
    std::fs::write(session.path.join("file.txt"), "updated\n").unwrap();
    std::fs::remove_file(session.path.join("removed.txt")).unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    let commit = fixture.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        commit.message().unwrap(),
        "Add 10 files; update 1 file; remove 1 file"
    );
}

#[test]
fn commit_summaries_handle_unicode_long_paths_and_binary_changes() {
    for name in ["résumé.txt".to_owned(), format!("{}.bin", "長".repeat(75))] {
        let fixture = Fixture::new();
        let session = fixture.session();
        std::fs::write(session.path.join(&name), [0, 1, 2, 255]).unwrap();
        let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
        let commit = fixture
            .repo
            .find_commit(Oid::from_str(&approved).unwrap())
            .unwrap();
        let expected = match name.chars().count() {
            ..=68 => format!("Add {name}"),
            _ => "Add 1 file".to_owned(),
        };
        assert_eq!(commit.message().unwrap(), expected);
        assert!(commit.message().unwrap().chars().count() <= 72);
    }
}

#[cfg(unix)]
#[test]
fn commit_summaries_escape_control_characters_in_paths() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("new\nfile.txt"), "new\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let commit = fixture
        .repo
        .find_commit(Oid::from_str(&approved).unwrap())
        .unwrap();
    assert_eq!(commit.message().unwrap(), "Add new\\nfile.txt");
}

#[test]
fn commit_messages_require_brief_plain_text_subjects() {
    for text in [
        "",
        "  ",
        "Fix\nmore",
        "Fix\0value",
        "```Fix```",
        "\"Fix\"",
        &"x".repeat(73),
    ] {
        assert!(CommitMessage::new(text).is_err(), "{text:?}");
    }
    assert_eq!(
        CommitMessage::new("  Raise retry limit to five\n")
            .unwrap()
            .as_str(),
        "Raise retry limit to five"
    );
    assert!(CommitMessage::new(&"長".repeat(72)).is_ok());
}

#[test]
fn descriptive_messages_survive_fast_forward_and_divergent_merges() {
    for diverged in [false, true] {
        let fixture = Fixture::new();
        let session = fixture.session();
        std::fs::write(
            session.path.join("retry.txt"),
            "Retry failed requests five times\n",
        )
        .unwrap();
        session.proposal(&fixture.workspace).unwrap().unwrap();
        std::fs::write(session.path.join("file.txt"), "retries = 5\n").unwrap();
        let original = session.proposal(&fixture.workspace).unwrap().unwrap();
        if diverged {
            fixture.commit("main-only.txt", "Unrelated main work\n");
        }
        let diff = session
            .proposal_diff(&fixture.workspace, &original)
            .unwrap();
        assert!(diff.contains("+retries = 5"));
        assert!(diff.contains("+Retry failed requests five times"));
        assert!(!diff.contains("main-only.txt"));
        let child = git2::Repository::open(&session.path).unwrap();
        let index = std::fs::read(child.path().join("index")).unwrap();
        let main_index = std::fs::read(fixture.repo.path().join("index")).unwrap();
        let main = fixture.repo.refname_to_id("HEAD").unwrap();
        let subject =
            CommitMessage::new("Raise retry limit to five and document retry behavior").unwrap();
        let described = session
            .describe_proposal(&fixture.workspace, &original, &subject)
            .unwrap();
        let before = fixture
            .repo
            .find_commit(Oid::from_str(&original).unwrap())
            .unwrap();
        let after = fixture
            .repo
            .find_commit(Oid::from_str(&described).unwrap())
            .unwrap();
        assert_eq!(before.tree_id(), after.tree_id());
        assert_eq!(
            before.parent_ids().collect::<Vec<_>>(),
            after.parent_ids().collect::<Vec<_>>()
        );
        assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), main);
        assert_eq!(std::fs::read(child.path().join("index")).unwrap(), index);
        assert_eq!(
            std::fs::read(fixture.repo.path().join("index")).unwrap(),
            main_index
        );
        assert_eq!(
            session.proposal(&fixture.workspace).unwrap().unwrap(),
            described
        );
        session.merge(&fixture.workspace, &described).unwrap();
        let merged = fixture.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(merged.message().unwrap(), subject.as_str());
        assert_eq!(merged.parent_count(), if diverged { 2 } else { 1 });
        session.cleanup(&fixture.workspace, &described).unwrap();
        assert!(!session.path.exists());
    }
}

#[test]
fn describing_a_stale_proposal_preserves_newer_changes_and_references() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "retries = 5\n").unwrap();
    let original = session.proposal(&fixture.workspace).unwrap().unwrap();
    let subject = CommitMessage::new("Raise retry limit to five").unwrap();
    std::fs::write(session.path.join("file.txt"), "retries = 7\n").unwrap();
    let child = git2::Repository::open(&session.path).unwrap();
    assert!(
        session
            .describe_proposal(&fixture.workspace, &original, &subject)
            .is_err()
    );
    assert_eq!(child.refname_to_id("HEAD").unwrap().to_string(), original);
    let newer = session.proposal(&fixture.workspace).unwrap().unwrap();
    assert!(
        session
            .describe_proposal(&fixture.workspace, &original, &subject)
            .is_err()
    );
    assert_eq!(child.refname_to_id("HEAD").unwrap().to_string(), newer);
    assert_eq!(
        std::fs::read_to_string(session.path.join("file.txt")).unwrap(),
        "retries = 7\n"
    );
}

#[test]
fn proposal_diffs_exclude_changes_already_in_main() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "shared change\n").unwrap();
    std::fs::write(session.path.join("session.txt"), "new session behavior\n").unwrap();
    let commit = session.proposal(&fixture.workspace).unwrap().unwrap();
    fixture.commit("file.txt", "shared change\n");
    let diff = session.proposal_diff(&fixture.workspace, &commit).unwrap();
    assert!(diff.contains("+new session behavior"));
    assert!(!diff.contains("file.txt"));
    assert!(!diff.contains("shared change"));
}

#[test]
fn conflicted_proposal_diffs_describe_only_the_session_changes() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session behavior\n").unwrap();
    let commit = session.proposal(&fixture.workspace).unwrap().unwrap();
    fixture.commit("file.txt", "main behavior\n");
    fixture.commit("main-only.txt", "unrelated work\n");
    let diff = session.proposal_diff(&fixture.workspace, &commit).unwrap();
    assert!(diff.contains("-base"));
    assert!(diff.contains("+session behavior"));
    assert!(!diff.contains("main behavior"));
    assert!(!diff.contains("main-only.txt"));
}

#[test]
fn integrated_proposals_cannot_be_rewritten() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session behavior\n").unwrap();
    let commit = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &commit).unwrap();
    assert!(
        session
            .proposal_diff(&fixture.workspace, &commit)
            .unwrap()
            .is_empty()
    );
    let message = CommitMessage::new("Change session behavior").unwrap();
    assert!(
        session
            .describe_proposal(&fixture.workspace, &commit, &message)
            .is_err()
    );
    assert_eq!(
        fixture.repo.refname_to_id("HEAD").unwrap().to_string(),
        commit
    );
    assert_eq!(
        git2::Repository::open(&session.path)
            .unwrap()
            .refname_to_id("HEAD")
            .unwrap()
            .to_string(),
        commit
    );
}

#[test]
fn sessions_are_isolated_and_proposals_do_not_merge_without_approval() {
    let fixture = Fixture::new();
    let first = fixture.session();
    let second = fixture.session();
    let base = fixture.repo.refname_to_id("HEAD").unwrap();
    std::fs::write(first.path.join("file.txt"), "first\n").unwrap();
    std::fs::write(first.path.join("added.txt"), "new\n").unwrap();
    let commit = first.proposal(&fixture.workspace).unwrap().unwrap();
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(
        std::fs::read_to_string(second.path.join("file.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "base\n"
    );
    assert!(matches!(
        first.merge(&fixture.workspace, &commit).unwrap(),
        MergeOutcome::Merged { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("added.txt")).unwrap(),
        "new\n"
    );
    assert!(
        GitRepository::required(&fixture.workspace)
            .unwrap()
            .status(&fixture.workspace)
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(first.proposal(&fixture.workspace).unwrap().is_none());
    assert!(first.workspace(&fixture.workspace).is_ok());
    first.cleanup(&fixture.workspace, &commit).unwrap();
    assert!(!first.path.exists());
    assert!(fixture.repo.find_worktree(&first.id).is_err());
    assert!(
        fixture
            .repo
            .find_branch(&first.branch(), git2::BranchType::Local)
            .is_err()
    );
    assert!(second.workspace(&fixture.workspace).is_ok());
    assert_eq!(fixture.repo.worktrees().unwrap().len(), 1);
}

#[test]
fn concurrent_sessions_merge_with_two_parents_and_preserve_both_changes() {
    let fixture = Fixture::new();
    let first = fixture.session();
    let second = fixture.session();
    std::fs::write(first.path.join("first.txt"), "first\n").unwrap();
    std::fs::remove_file(second.path.join("file.txt")).unwrap();
    let first_commit = first.proposal(&fixture.workspace).unwrap().unwrap();
    let second_commit = second.proposal(&fixture.workspace).unwrap().unwrap();
    first.merge(&fixture.workspace, &first_commit).unwrap();
    second.merge(&fixture.workspace, &second_commit).unwrap();
    assert_eq!(
        fixture
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        2
    );
    assert!(fixture.root.join("first.txt").exists());
    assert!(!fixture.root.join("file.txt").exists());
    first.cleanup(&fixture.workspace, &first_commit).unwrap();
    second.cleanup(&fixture.workspace, &second_commit).unwrap();
    assert!(!first.path.exists());
    assert!(!second.path.exists());
    assert_eq!(fixture.repo.worktrees().unwrap().len(), 0);
}

#[test]
fn conflicts_and_dirty_main_retain_work_and_allow_retry() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let commit = session.proposal(&fixture.workspace).unwrap().unwrap();
    std::fs::write(fixture.root.join("file.txt"), "user\n").unwrap();
    let index = std::fs::read(fixture.repo.path().join("index")).unwrap();
    let base = fixture.repo.refname_to_id("HEAD").unwrap();
    assert!(session.merge(&fixture.workspace, &commit).is_err());
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(
        std::fs::read(fixture.repo.path().join("index")).unwrap(),
        index
    );
    let target = fixture.commit("file.txt", "user\n");
    assert!(session.merge(&fixture.workspace, &commit).is_err());
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), target);
    assert!(session.cleanup(&fixture.workspace, &commit).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    assert_eq!(
        std::fs::read_to_string(session.path.join("file.txt")).unwrap(),
        "session\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "user\n"
    );
    std::fs::write(session.path.join("file.txt"), "user\n").unwrap();
    let retry = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &retry).unwrap();
    assert_eq!(
        fixture
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Merge session changes already present in main"
    );
}

#[test]
fn fork_copies_pending_edits_and_stale_approval_cannot_merge_new_changes() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "fork base\n").unwrap();
    let fork = SessionWorktree::create(
        &fixture.workspace,
        &uuid::Uuid::new_v4().to_string(),
        Some(&session),
    )
    .unwrap()
    .unwrap();
    assert_ne!(fork.path, session.path);
    assert_eq!(
        std::fs::read_to_string(fork.path.join("file.txt")).unwrap(),
        "fork base\n"
    );
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    let target = fixture.repo.refname_to_id("HEAD").unwrap();
    std::fs::write(session.path.join("file.txt"), "later edits\n").unwrap();
    assert!(session.merge(&fixture.workspace, &approved).is_err());
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), target);
    assert_eq!(
        std::fs::read_to_string(fork.path.join("file.txt")).unwrap(),
        "fork base\n"
    );
}

#[test]
fn main_is_required_and_saved_identity_cannot_redirect_workspace_access() {
    let fixture = Fixture::new();
    let mut session = fixture.session();
    session.path = fixture.root.clone();
    assert!(session.workspace(&fixture.workspace).is_err());
    fixture
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .rename("master", false)
        .unwrap();
    assert!(
        SessionWorktree::create(&fixture.workspace, &uuid::Uuid::new_v4().to_string(), None)
            .is_err()
    );
}

#[test]
fn merging_cannot_overwrite_ignored_files_in_main() {
    let fixture = Fixture::new();
    let main = fixture.commit(".gitignore", "local.txt\n");
    std::fs::write(fixture.root.join("local.txt"), "private local data\n").unwrap();
    let session = fixture.session();
    std::fs::remove_file(session.path.join(".gitignore")).unwrap();
    std::fs::write(session.path.join("local.txt"), "session data\n").unwrap();
    let commit = session.proposal(&fixture.workspace).unwrap().unwrap();
    let index = std::fs::read(fixture.repo.path().join("index")).unwrap();
    assert!(session.merge(&fixture.workspace, &commit).is_err());
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), main);
    assert_eq!(
        std::fs::read(fixture.repo.path().join("index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("local.txt")).unwrap(),
        "private local data\n"
    );
}

#[test]
fn conflicts_are_resolved_in_the_session_and_record_both_parents_before_merging() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    fixture.commit("file.txt", "main\n");
    let target = fixture.commit("incoming.txt", "incoming\n");
    let error = session.merge(&fixture.workspace, &approved).err().unwrap();
    let conflict = error.downcast_ref::<MergeConflict>().unwrap();
    assert!(
        session
            .finish_resolution(&fixture.workspace, conflict)
            .is_err()
    );
    session
        .prepare_resolution(&fixture.workspace, conflict)
        .unwrap();
    let markers = std::fs::read_to_string(session.path.join("file.txt")).unwrap();
    assert!(markers.contains("<<<<<<< Joe session"));
    assert!(markers.contains(">>>>>>> main"));
    assert!(markers.contains("session\n"));
    assert!(markers.contains("main\n"));
    assert_eq!(
        std::fs::read_to_string(session.path.join("incoming.txt")).unwrap(),
        "incoming\n"
    );
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), target);
    assert!(
        session
            .finish_resolution(&fixture.workspace, conflict)
            .is_err()
    );
    std::fs::write(session.path.join("file.txt"), "combined\n").unwrap();
    let resolved = session
        .finish_resolution(&fixture.workspace, conflict)
        .unwrap();
    let commit = fixture
        .repo
        .find_commit(Oid::from_str(&resolved).unwrap())
        .unwrap();
    assert_eq!(commit.parent_count(), 2);
    assert_eq!(commit.parent_id(0).unwrap().to_string(), approved);
    assert_eq!(commit.parent_id(1).unwrap(), target);
    assert_eq!(commit.message().unwrap(), "Update file.txt");
    fixture.commit("later.txt", "later main work\n");
    session.merge(&fixture.workspace, &resolved).unwrap();
    assert_eq!(
        fixture
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Update file.txt"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "combined\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("incoming.txt")).unwrap(),
        "incoming\n"
    );
    assert!(fixture.root.join("later.txt").exists());
    session.cleanup(&fixture.workspace, &resolved).unwrap();
    assert!(!session.path.exists());
    assert!(
        fixture
            .repo
            .find_branch(&session.branch(), git2::BranchType::Local)
            .is_err()
    );
}

#[test]
fn failed_resolution_preparation_cannot_discard_main_changes() {
    let fixture = Fixture::new();
    fixture.commit(".gitignore", "local.txt\n");
    let session = fixture.session();
    std::fs::write(session.path.join("local.txt"), "private local data\n").unwrap();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    fixture.commit("file.txt", "main\n");
    fixture.commit(".gitignore", "");
    let target = fixture.commit("local.txt", "main data\n");
    let error = session.merge(&fixture.workspace, &approved).err().unwrap();
    let conflict = error.downcast_ref::<MergeConflict>().unwrap();
    assert!(
        session
            .prepare_resolution(&fixture.workspace, conflict)
            .is_err()
    );
    assert!(
        session
            .finish_resolution(&fixture.workspace, conflict)
            .is_err()
    );
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), target);
    assert_eq!(
        std::fs::read_to_string(session.path.join("local.txt")).unwrap(),
        "private local data\n"
    );
}

#[test]
fn cleanup_of_an_already_integrated_session_preserves_source_changes_and_index() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "merged\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    fixture.commit("later.txt", "later main commit\n");
    assert!(matches!(
        session.merge(&fixture.workspace, &approved).unwrap(),
        MergeOutcome::Unchanged
    ));
    let head = fixture.repo.refname_to_id("HEAD").unwrap();
    std::fs::write(fixture.root.join("file.txt"), "staged\n").unwrap();
    let mut index = fixture.repo.index().unwrap();
    index.add_path(Path::new("file.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(fixture.root.join("file.txt"), "unstaged\n").unwrap();
    let index = std::fs::read(fixture.repo.path().join("index")).unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_worktree(&session.id).is_err());
    assert!(
        fixture
            .repo
            .find_branch(&session.branch(), git2::BranchType::Local)
            .is_err()
    );
    assert_eq!(fixture.repo.refname_to_id("HEAD").unwrap(), head);
    assert_eq!(
        std::fs::read(fixture.repo.path().join("index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "unstaged\n"
    );
}

#[test]
fn cleanup_refuses_unmerged_and_post_merge_commits() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    session.merge(&fixture.workspace, &approved).unwrap();
    std::fs::write(session.path.join("file.txt"), "later work\n").unwrap();
    let later = session.proposal(&fixture.workspace).unwrap().unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert!(session.cleanup(&fixture.workspace, &later).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    assert_eq!(
        fixture
            .repo
            .refname_to_id(&format!("refs/heads/{}", session.branch()))
            .unwrap()
            .to_string(),
        later
    );
}

#[test]
fn cleanup_preserves_uncommitted_source_files() {
    for path in ["file.txt", "untracked.txt", "target/notes.txt"] {
        let fixture = Fixture::new();
        let session = fixture.session();
        std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
        let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
        session.merge(&fixture.workspace, &approved).unwrap();
        let local = session.path.join(path);
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "preserve local data\n").unwrap();
        assert!(
            session.cleanup(&fixture.workspace, &approved).is_err(),
            "{path}"
        );
        assert_eq!(
            std::fs::read_to_string(local).unwrap(),
            "preserve local data\n"
        );
        assert!(session.workspace(&fixture.workspace).is_ok());
        assert!(
            fixture
                .repo
                .find_branch(&session.branch(), git2::BranchType::Local)
                .is_ok()
        );
    }
}

#[test]
fn cleanup_removes_the_entire_session_directory_including_large_build_caches() {
    for ignore in [None, Some("/target\n/ignored.txt\n")] {
        let fixture = Fixture::new();
        if let Some(ignore) = ignore {
            fixture.commit(".gitignore", ignore);
        }
        let session = fixture.session();
        for path in [
            "target/.joe/linux/build/debug/build.bin",
            "target/.joe/linux/cargo/registry/src/dependency.bin",
            "target/.joe/tmp/leftover/output.bin",
            ".turbo-code/private.bin",
        ] {
            let path = session.path.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(path)
                .unwrap()
                .set_len(128 * 1024 * 1024)
                .unwrap();
        }
        if ignore.is_some() {
            std::fs::write(session.path.join("ignored.txt"), "discard with session\n").unwrap();
            std::fs::write(session.path.join("target/host-build.bin"), "build output\n").unwrap();
        }
        std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
        let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
        session.merge(&fixture.workspace, &approved).unwrap();
        session.cleanup(&fixture.workspace, &approved).unwrap();
        assert!(!session.path.exists());
        assert!(fixture.repo.find_worktree(&session.id).is_err());
        assert!(
            fixture
                .repo
                .find_branch(&session.branch(), git2::BranchType::Local)
                .is_err()
        );
        assert_eq!(
            fixture.repo.refname_to_id("HEAD").unwrap().to_string(),
            approved
        );
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
            "session\n"
        );
        assert!(!fixture.root.join("target").exists());
        assert!(!fixture.root.join(".turbo-code").exists());
    }
}

#[cfg(unix)]
#[test]
fn cleanup_removes_cache_links_without_following_them() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let session = fixture.session();
    let cache = session.path.join("target/.joe/linux/cargo/registry");
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(&outside.root, cache.join("index")).unwrap();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert_eq!(
        std::fs::read_to_string(outside.root.join("file.txt")).unwrap(),
        "base\n"
    );
    assert!(outside.repo.head().is_ok());
}

#[test]
fn cleanup_reports_oversized_files_and_allows_retry() {
    for path in ["file.txt", "untracked.bin", "target/notes.bin"] {
        let fixture = Fixture::new();
        let session = fixture.session();
        std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
        let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
        session.merge(&fixture.workspace, &approved).unwrap();
        let child = git2::Repository::open(&session.path).unwrap();
        let index = std::fs::read(child.path().join("index")).unwrap();
        let local = session.path.join(path);
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "preserve local data\n").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&local)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        let error = session
            .cleanup(&fixture.workspace, &approved)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Cleanup conflict"), "{error}");
        assert!(error.contains(path), "{error}");
        assert!(!error.contains("read limit"), "{error}");
        assert_eq!(
            std::fs::metadata(&local).unwrap().len(),
            16 * 1024 * 1024 + 1
        );
        let mut prefix = [0; b"preserve local data\n".len()];
        std::io::Read::read_exact(&mut std::fs::File::open(&local).unwrap(), &mut prefix).unwrap();
        assert_eq!(&prefix, b"preserve local data\n");
        assert_eq!(std::fs::read(child.path().join("index")).unwrap(), index);
        assert!(session.workspace(&fixture.workspace).is_ok());
        assert_eq!(
            fixture
                .repo
                .refname_to_id(&format!("refs/heads/{}", session.branch()))
                .unwrap()
                .to_string(),
            approved
        );
        match path {
            "file.txt" => std::fs::write(&local, "session\n").unwrap(),
            _ => std::fs::remove_file(&local).unwrap(),
        }
        session.cleanup(&fixture.workspace, &approved).unwrap();
        assert!(!session.path.exists());
        assert!(
            fixture
                .repo
                .find_branch(&session.branch(), git2::BranchType::Local)
                .is_err()
        );
    }
}

#[test]
fn cleanup_preserves_same_size_edits_and_missing_files() {
    for content in [Some("changed\n"), None] {
        let fixture = Fixture::new();
        let session = fixture.session();
        std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
        let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
        session.merge(&fixture.workspace, &approved).unwrap();
        let local = session.path.join("file.txt");
        match content {
            Some(content) => std::fs::write(&local, content).unwrap(),
            None => std::fs::remove_file(&local).unwrap(),
        }
        assert!(session.cleanup(&fixture.workspace, &approved).is_err());
        assert!(session.workspace(&fixture.workspace).is_ok());
        assert_eq!(std::fs::read_to_string(&local).ok().as_deref(), content);
        std::fs::write(&local, "session\n").unwrap();
        session.cleanup(&fixture.workspace, &approved).unwrap();
        assert!(!session.path.exists());
    }
}

#[test]
fn cleanup_preserves_index_only_changes() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    let child = git2::Repository::open(&session.path).unwrap();
    std::fs::write(session.path.join("file.txt"), "staged\n").unwrap();
    let mut index = child.index().unwrap();
    index.add_path(Path::new("file.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let before = std::fs::read(child.path().join("index")).unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert_eq!(std::fs::read(child.path().join("index")).unwrap(), before);
    assert!(session.workspace(&fixture.workspace).is_ok());
}

#[test]
fn cleanup_preserves_locked_worktrees_and_allows_retry() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    let child = git2::Repository::open(&session.path).unwrap();
    let lock = child.path().join("locked");
    std::fs::write(&lock, "keep this worktree\n").unwrap();
    assert!(session.cleanup(&fixture.workspace, &approved).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    std::fs::remove_file(lock).unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(
        fixture
            .repo
            .find_branch(&session.branch(), git2::BranchType::Local)
            .is_err()
    );
}

#[test]
fn prune_recovers_removed_worktrees_without_deleting_branches_checked_out_elsewhere() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "unmerged\n").unwrap();
    session.proposal(&fixture.workspace).unwrap().unwrap();
    let other = fixture.session();
    let linked = git2::Repository::open(&other.path).unwrap();
    let reference = format!("refs/heads/{}", session.branch());
    let mut options = git2::WorktreePruneOptions::new();
    options.valid(true).working_tree(true);
    fixture
        .repo
        .find_worktree(session.id())
        .unwrap()
        .prune(Some(&mut options))
        .unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_reference(&reference).is_ok());
    std::fs::write(linked.path().join("HEAD"), format!("ref: {reference}\n")).unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(fixture.repo.find_reference(&reference).is_ok());
    linked
        .set_head(&format!("refs/heads/{}", other.branch()))
        .unwrap();
    fixture.repo.set_head(&reference).unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(fixture.repo.find_reference(&reference).is_ok());
    fixture.repo.set_head("refs/heads/main").unwrap();
    std::fs::create_dir(&session.path).unwrap();
    std::fs::write(session.path.join("private.txt"), "replacement directory\n").unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(session.path.join("private.txt").exists());
    std::fs::remove_dir_all(&session.path).unwrap();
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Pruned
    );
    assert!(fixture.repo.find_reference(&reference).is_err());
    assert!(other.workspace(&fixture.workspace).is_ok());
}

#[test]
fn prune_discards_unmerged_commits_and_all_files_without_changing_main_or_its_index() {
    let fixture = Fixture::new();
    fixture.commit(".gitignore", "ignored.bin\n/target\n");
    let session = fixture.session();
    let other = fixture.session();
    std::fs::write(session.path.join("file.txt"), "unmerged\n").unwrap();
    session.proposal(&fixture.workspace).unwrap().unwrap();
    std::fs::write(session.path.join("file.txt"), "unstaged\n").unwrap();
    std::fs::write(session.path.join("untracked.txt"), "untracked\n").unwrap();
    std::fs::File::create(session.path.join("ignored.bin"))
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    std::fs::create_dir_all(session.path.join("target/cache")).unwrap();
    std::fs::write(session.path.join("target/cache/build.bin"), "cache\n").unwrap();
    std::fs::write(fixture.root.join("file.txt"), "staged main\n").unwrap();
    let mut index = fixture.repo.index().unwrap();
    index.add_path(Path::new("file.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(fixture.root.join("file.txt"), "unstaged main\n").unwrap();
    let before_index = std::fs::read(fixture.repo.path().join("index")).unwrap();
    let main = fixture.repo.refname_to_id("refs/heads/main").unwrap();
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Pruned
    );
    assert!(!session.path.exists());
    assert!(fixture.repo.find_worktree(&session.id).is_err());
    assert!(
        fixture
            .repo
            .find_branch(&session.branch(), git2::BranchType::Local)
            .is_err()
    );
    assert_eq!(fixture.repo.refname_to_id("refs/heads/main").unwrap(), main);
    assert_eq!(
        std::fs::read(fixture.repo.path().join("index")).unwrap(),
        before_index
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("file.txt")).unwrap(),
        "unstaged main\n"
    );
    assert!(other.workspace(&fixture.workspace).is_ok());
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Pruned
    );
}

#[test]
fn prune_keeps_commits_already_merged_into_main() {
    let fixture = Fixture::new();
    let session = fixture.session();
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Merged
    );
    std::fs::write(session.path.join("file.txt"), "merged\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    fixture.commit("other.txt", "later main commit\n");
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Merged
    );
    assert!(session.workspace(&fixture.workspace).is_ok());
    std::fs::write(session.path.join("file.txt"), "unmerged local work\n").unwrap();
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Pruned
    );
}

#[test]
fn prune_discards_local_changes_without_unmerged_commits() {
    for change in ["unstaged", "staged", "untracked"] {
        let fixture = Fixture::new();
        let session = fixture.session();
        match change {
            "untracked" => std::fs::write(session.path.join("new.txt"), "local\n").unwrap(),
            "staged" => {
                let child = git2::Repository::open(&session.path).unwrap();
                std::fs::write(session.path.join("file.txt"), "index only\n").unwrap();
                let mut index = child.index().unwrap();
                index.add_path(Path::new("file.txt")).unwrap();
                index.write().unwrap();
                std::fs::write(session.path.join("file.txt"), "base\n").unwrap();
            }
            _ => std::fs::write(session.path.join("file.txt"), "local\n").unwrap(),
        }
        assert_eq!(
            session.prune(&fixture.workspace).unwrap(),
            PruneOutcome::Pruned,
            "{change}"
        );
    }
}

#[test]
fn prune_retains_locked_worktrees_and_changed_identities() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "unmerged\n").unwrap();
    let child = git2::Repository::open(&session.path).unwrap();
    let lock = child.path().join("locked");
    std::fs::write(&lock, "keep\n").unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    std::fs::remove_file(lock).unwrap();
    let mut altered = session.clone();
    altered.path = fixture.root.clone();
    assert!(altered.prune(&fixture.workspace).is_err());
    altered = session.clone();
    altered.target = session.branch();
    assert!(altered.prune(&fixture.workspace).is_err());
    child
        .set_head_detached(child.refname_to_id("HEAD").unwrap())
        .unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(session.path.join("file.txt").exists());
    child
        .set_head(&format!("refs/heads/{}", session.branch()))
        .unwrap();
    assert_eq!(
        session.prune(&fixture.workspace).unwrap(),
        PruneOutcome::Pruned
    );
}

#[test]
fn prune_requires_original_root_and_whole_project_write_access() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "local\n").unwrap();
    let child = session.workspace(&fixture.workspace).unwrap();
    assert!(session.prune(&child).is_err());
    let readonly = fixture
        .workspace
        .restricted(
            &[PathBuf::from(".")],
            crate::workspace::RootAccess::ReadOnly,
        )
        .unwrap();
    assert!(session.prune(&readonly).is_err());
    let restricted = fixture
        .workspace
        .restricted(
            &[PathBuf::from("file.txt")],
            crate::workspace::RootAccess::ReadWrite,
        )
        .unwrap();
    assert!(session.prune(&restricted).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
}

#[test]
fn prune_retains_a_branch_checked_out_elsewhere() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "unmerged\n").unwrap();
    let reference = format!("refs/heads/{}", session.branch());
    std::fs::write(
        fixture.repo.path().join("HEAD"),
        format!("ref: {reference}\n"),
    )
    .unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    fixture.repo.set_head("refs/heads/main").unwrap();
    let other = fixture.session();
    let linked = git2::Repository::open(&other.path).unwrap();
    std::fs::write(linked.path().join("HEAD"), format!("ref: {reference}\n")).unwrap();
    assert!(session.prune(&fixture.workspace).is_err());
    assert!(session.workspace(&fixture.workspace).is_ok());
    assert!(other.path.exists());
}

#[test]
fn cleanup_preserves_a_session_branch_checked_out_in_the_source_repository() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    let reference = format!("refs/heads/{}", session.branch());
    std::fs::write(
        fixture.repo.path().join("HEAD"),
        format!("ref: {reference}\n"),
    )
    .unwrap();
    let error = session.cleanup(&fixture.workspace, &approved).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checked out in the source repository")
    );
    assert!(session.workspace(&fixture.workspace).is_ok());
    assert_eq!(
        fixture.repo.refname_to_id(&reference).unwrap().to_string(),
        approved
    );
    fixture.repo.set_head("refs/heads/main").unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_reference(&reference).is_err());
}

#[test]
fn cleanup_preserves_a_session_branch_checked_out_in_another_worktree() {
    let fixture = Fixture::new();
    let session = fixture.session();
    std::fs::write(session.path.join("file.txt"), "session\n").unwrap();
    let approved = session.proposal(&fixture.workspace).unwrap().unwrap();
    session.merge(&fixture.workspace, &approved).unwrap();
    let other = fixture.session();
    let linked = git2::Repository::open(&other.path).unwrap();
    let reference = format!("refs/heads/{}", session.branch());
    std::fs::write(linked.path().join("HEAD"), format!("ref: {reference}\n")).unwrap();
    let error = session.cleanup(&fixture.workspace, &approved).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checked out in another worktree")
    );
    assert!(session.workspace(&fixture.workspace).is_ok());
    assert_eq!(
        fixture.repo.refname_to_id(&reference).unwrap().to_string(),
        approved
    );
    linked
        .set_head(&format!("refs/heads/{}", other.branch()))
        .unwrap();
    session.cleanup(&fixture.workspace, &approved).unwrap();
    assert!(!session.path.exists());
    assert!(fixture.repo.find_reference(&reference).is_err());
    assert!(other.workspace(&fixture.workspace).is_ok());
}
