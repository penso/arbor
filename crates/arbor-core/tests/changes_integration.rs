#![allow(clippy::expect_used)]

use {
    arbor_core::changes::{self, ChangeKind},
    std::{fs, path::Path},
};

#[test]
fn reports_modified_and_untracked_files() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let repo_path = temp_dir.path().join("repo");

    let repo = git2::Repository::init(&repo_path).expect("repo should be initialized");
    setup_git2_config(&repo);

    fs::write(repo_path.join("tracked.txt"), "hello\n").expect("tracked file should be written");
    create_initial_commit(&repo, "initial commit");

    fs::write(repo_path.join("tracked.txt"), "hello from arbor\n")
        .expect("tracked file should be modified");
    fs::write(repo_path.join("untracked.txt"), "new file\n").expect("untracked file should exist");

    let changes = changes::changed_files(&repo_path).expect("gix status should succeed");

    assert!(changes.iter().any(|change| {
        change.path.as_path() == Path::new("tracked.txt")
            && change.kind == ChangeKind::Modified
            && (change.additions > 0 || change.deletions > 0)
    }));
    assert!(changes.iter().any(|change| {
        change.path.as_path() == Path::new("untracked.txt")
            && change.kind == ChangeKind::Added
            && change.additions > 0
            && change.deletions == 0
    }));
}

#[test]
fn reports_line_level_diff_summary() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let repo_path = temp_dir.path().join("repo");

    let repo = git2::Repository::init(&repo_path).expect("repo should be initialized");
    setup_git2_config(&repo);

    fs::write(repo_path.join("tracked.txt"), "line-a\nline-b\n")
        .expect("tracked file should be written");
    create_initial_commit(&repo, "initial commit");

    fs::write(repo_path.join("tracked.txt"), "line-a\nline-c\nline-d\n")
        .expect("tracked file should be modified");
    fs::write(repo_path.join("untracked.txt"), "first\nsecond\n")
        .expect("untracked file should be written");

    let summary = changes::diff_line_summary(&repo_path).expect("diff summary should succeed");

    assert!(
        summary.additions >= 4,
        "expected additions >= 4, got {}",
        summary.additions
    );
    assert!(
        summary.deletions >= 1,
        "expected deletions >= 1, got {}",
        summary.deletions
    );
}

fn setup_git2_config(repo: &git2::Repository) {
    let mut config = repo.config().expect("config should be accessible");
    config
        .set_str("user.email", "tests@example.com")
        .expect("email should be set");
    config
        .set_str("user.name", "Arbor Tests")
        .expect("name should be set");
}

fn create_initial_commit(repo: &git2::Repository, message: &str) {
    let mut index = repo.index().expect("index should be accessible");
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .expect("files should be added");
    index.write().expect("index should be written");
    let tree_oid = index.write_tree().expect("tree should be written");
    let tree = repo.find_tree(tree_oid).expect("tree should be found");
    let sig = repo.signature().expect("signature should be created");

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
        .expect("commit should be created");
}
