use arbor_core::{
    changes::{count_lines, diff_line_stats},
    worktree::short_branch,
};

fn main() {
    divan::main();
}

// --- count_lines benchmarks ---

#[divan::bench]
fn count_lines_empty() -> usize {
    count_lines(b"")
}

#[divan::bench]
fn count_lines_single_line() -> usize {
    count_lines(b"hello world\n")
}

#[divan::bench]
fn count_lines_no_trailing_newline() -> usize {
    count_lines(b"hello world")
}

#[divan::bench]
fn count_lines_100_lines(bencher: divan::Bencher) {
    let input: Vec<u8> = (0..100)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| count_lines(&input))
}

#[divan::bench]
fn count_lines_10000_lines(bencher: divan::Bencher) {
    let input: Vec<u8> = (0..10_000)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| count_lines(&input))
}

// --- diff_line_stats benchmarks ---

#[divan::bench]
fn diff_line_stats_identical(bencher: divan::Bencher) {
    let content: Vec<u8> = (0..100)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| diff_line_stats(&content, &content))
}

#[divan::bench]
fn diff_line_stats_small_change(bencher: divan::Bencher) {
    let old: Vec<u8> = (0..100)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    let mut new_lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    new_lines[50] = "modified line 50".to_string();
    let new: Vec<u8> = new_lines.join("\n").into_bytes();
    bencher.bench(|| diff_line_stats(&old, &new))
}

#[divan::bench]
fn diff_line_stats_complete_rewrite(bencher: divan::Bencher) {
    let old: Vec<u8> = (0..100)
        .flat_map(|i| format!("old line {i}\n").into_bytes())
        .collect();
    let new: Vec<u8> = (0..100)
        .flat_map(|i| format!("new line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| diff_line_stats(&old, &new))
}

#[divan::bench]
fn diff_line_stats_added_file(bencher: divan::Bencher) {
    let new: Vec<u8> = (0..100)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| diff_line_stats(b"", &new))
}

#[divan::bench]
fn diff_line_stats_removed_file(bencher: divan::Bencher) {
    let old: Vec<u8> = (0..100)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    bencher.bench(|| diff_line_stats(&old, b""))
}

// --- short_branch benchmarks ---

#[divan::bench]
fn short_branch_with_prefix() -> String {
    short_branch("refs/heads/main")
}

#[divan::bench]
fn short_branch_without_prefix() -> String {
    short_branch("main")
}

#[divan::bench]
fn short_branch_nested() -> String {
    short_branch("refs/heads/feature/my-feature")
}
