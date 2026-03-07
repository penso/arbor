#![allow(clippy::expect_used)]

use {arbor_core::worktree, std::fs};

#[test]
fn lists_real_git_worktrees() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let repo_path = temp_dir.path().join("repo");
    let feature_path = temp_dir.path().join("feature-worktree");

    // Initialize a git repo with an initial commit using git2.
    let repo = git2::Repository::init(&repo_path).expect("repo should be initialized");
    setup_git2_config(&repo);

    fs::write(repo_path.join("README.md"), "# Arbor\n").expect("test file should be written");
    create_initial_commit(&repo, "initial commit");

    // Create a linked worktree with a new branch.
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .expect("HEAD should resolve");
    let branch = repo
        .branch("feature", &head_commit, false)
        .expect("branch should be created");

    let mut opts = git2::WorktreeAddOptions::new();
    let branch_ref = branch.into_reference();
    opts.reference(Some(&branch_ref));
    repo.worktree("feature-worktree", &feature_path, Some(&opts))
        .expect("worktree should be added");

    let worktrees = worktree::list(&repo_path).expect("worktree list should succeed");
    let repo_path = fs::canonicalize(repo_path).expect("repo path should resolve");
    let feature_path = fs::canonicalize(feature_path).expect("feature path should resolve");

    assert_eq!(worktrees.len(), 2);
    assert!(
        worktrees
            .iter()
            .any(
                |entry| fs::canonicalize(&entry.path).ok().as_deref() == Some(&repo_path)
                    && entry.branch.as_deref() == Some("refs/heads/main")
            )
    );
    assert!(
        worktrees
            .iter()
            .any(
                |entry| fs::canonicalize(&entry.path).ok().as_deref() == Some(&feature_path)
                    && entry.branch.as_deref() == Some("refs/heads/feature")
            )
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
    config
        .set_str("init.defaultBranch", "main")
        .expect("default branch should be set");
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
