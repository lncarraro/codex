use super::*;
use pretty_assertions::assert_eq;

#[test]
fn computes_posix_workspace_relative_paths() {
    let root = PathUri::parse("file:///workspace/project").expect("valid root");
    let path = PathUri::parse("file:///workspace/project/src/Cliente.java").expect("valid path");

    assert_eq!(
        relative_path(&root, &path).as_deref(),
        Some("src/Cliente.java")
    );
}

#[test]
fn compares_windows_paths_case_insensitively() {
    let left =
        PathUri::parse("file:///C:/Workspace/Project/.codex/config.toml").expect("valid left path");
    let right = PathUri::parse("file:///c:/workspace/project/.CODEX/CONFIG.TOML")
        .expect("valid right path");

    assert!(paths_equal(&left, &right));
}

#[test]
fn computes_windows_workspace_relative_paths_case_insensitively() {
    let root = PathUri::parse("file:///C:/Workspace/Project").expect("valid root");
    let path = PathUri::parse("file:///c:/workspace/project/src/Cliente.java").expect("valid path");

    assert_eq!(
        relative_path(&root, &path).as_deref(),
        Some("src/Cliente.java")
    );
}
