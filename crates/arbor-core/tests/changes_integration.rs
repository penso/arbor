#![allow(clippy::expect_used)]

use {
    arbor_core::changes::{self, ChangeKind},
    std::{fs, path::Path, process::Command},
};

#[test]
fn reports_modified_and_untracked_files() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let repo_path = temp_dir.path().join("repo");

    fs::create_dir_all(&repo_path).expect("repo dir should be created");
    run_git(&repo_path, &["init", "--initial-branch=main"]);
    run_git(&repo_path, &["config", "user.email", "tests@example.com"]);
    run_git(&repo_path, &["config", "user.name", "Arbor Tests"]);

    fs::write(repo_path.join("tracked.txt"), "hello\n").expect("tracked file should be written");
    run_git(&repo_path, &["add", "tracked.txt"]);
    run_git(&repo_path, &["commit", "-m", "initial commit"]);

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

    fs::create_dir_all(&repo_path).expect("repo dir should be created");
    run_git(&repo_path, &["init", "--initial-branch=main"]);
    run_git(&repo_path, &["config", "user.email", "tests@example.com"]);
    run_git(&repo_path, &["config", "user.name", "Arbor Tests"]);

    fs::write(repo_path.join("tracked.txt"), "line-a\nline-b\n")
        .expect("tracked file should be written");
    run_git(&repo_path, &["add", "tracked.txt"]);
    run_git(&repo_path, &["commit", "-m", "initial commit"]);

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
