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
fn computes_windows_workspace_relative_paths_case_insensitively() {
    let root = PathUri::parse("file:///C:/Workspace/Project").expect("valid root");
    let path = PathUri::parse("file:///c:/workspace/project/src/Cliente.java").expect("valid path");

    assert_eq!(
        relative_path(&root, &path).as_deref(),
        Some("src/Cliente.java")
    );
}
