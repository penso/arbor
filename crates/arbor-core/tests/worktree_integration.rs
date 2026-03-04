#![allow(clippy::expect_used)]

use std::{fs, path::Path, process::Command};

use arbor_core::worktree;

#[test]
fn lists_real_git_worktrees() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let repo_path = temp_dir.path().join("repo");
    let feature_path = temp_dir.path().join("feature-worktree");

    fs::create_dir_all(&repo_path).expect("repo dir should be created");

    run_git(&repo_path, &["init", "--initial-branch=main"]);
    run_git(&repo_path, &["config", "user.email", "tests@example.com"]);
    run_git(&repo_path, &["config", "user.name", "Arbor Tests"]);

    fs::write(repo_path.join("README.md"), "# Arbor\n").expect("test file should be written");
    run_git(&repo_path, &["add", "README.md"]);
    run_git(&repo_path, &["commit", "-m", "initial commit"]);

    run_git(&repo_path, &[
        "worktree",
        "add",
        "-b",
        "feature",
        feature_path
            .to_str()
            .expect("feature path should be valid UTF-8"),
    ]);

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

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git command should execute");

    if output.status.success() {
        return;
    }

    panic!(
        "git command failed: git {}\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
