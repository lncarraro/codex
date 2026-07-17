use super::*;
use codex_exec_server::LOCAL_FS;
use pretty_assertions::assert_eq;
use std::fs;
use tempfile::tempdir;

const PROJECT_CONFIG: &str = r#"
[file_encoding]
default = "utf-8"
preserve_existing = true
strict = true

[[file_encoding.rules]]
globs = ["src/*.java"]
encoding = "windows-1252"
"#;

fn setup_project() -> (tempfile::TempDir, PathUri) {
    let directory = tempdir().expect("temporary project");
    fs::create_dir_all(directory.path().join(".codex")).expect("create .codex");
    fs::create_dir_all(directory.path().join("src")).expect("create source directory");
    fs::write(directory.path().join(".codex/config.toml"), PROJECT_CONFIG)
        .expect("write project config");
    let cwd = PathUri::from_host_native_path(directory.path()).expect("absolute project path");
    (directory, cwd)
}

async fn apply(
    cwd: &PathUri,
    patch: &str,
) -> std::result::Result<AppliedPatchDelta, ApplyPatchFailure> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        patch,
        cwd,
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
}

#[tokio::test]
async fn update_preserves_windows_1252_bytes() {
    let (directory, cwd) = setup_project();
    let path = directory.path().join("src/Cliente.java");
    fs::write(&path, b"// Informa\xe7\xe3o\nclass Cliente {}\n").expect("write legacy source");

    apply(
        &cwd,
        r#"*** Begin Patch
*** Update File: src/Cliente.java
@@
-// Informação
+// Informação atualizada — João
 class Cliente {}
*** End Patch"#,
    )
    .await
    .expect("patch should succeed");

    assert_eq!(
        fs::read(path).expect("read updated source"),
        b"// Informa\xe7\xe3o atualizada \x97 Jo\xe3o\nclass Cliente {}\n"
    );
}

#[tokio::test]
async fn update_preserves_existing_non_ascii_utf8_over_legacy_rule() {
    let (directory, cwd) = setup_project();
    let path = directory.path().join("src/Cliente.java");
    fs::write(&path, "// Informação\nclass Cliente {}\n").expect("write UTF-8 source");

    apply(
        &cwd,
        r#"*** Begin Patch
*** Update File: src/Cliente.java
@@
-// Informação
+// Informação concluída ✅
 class Cliente {}
*** End Patch"#,
    )
    .await
    .expect("patch should succeed");

    assert_eq!(
        fs::read(path).expect("read updated source"),
        "// Informação concluída ✅\nclass Cliente {}\n".as_bytes()
    );
}

#[tokio::test]
async fn unrepresentable_character_does_not_modify_legacy_file() {
    let (directory, cwd) = setup_project();
    let path = directory.path().join("src/Cliente.java");
    let original = b"class Cliente {}\n";
    fs::write(&path, original).expect("write ASCII source");

    let result = apply(
        &cwd,
        r#"*** Begin Patch
*** Update File: src/Cliente.java
@@
+// concluído ✅
 class Cliente {}
*** End Patch"#,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(fs::read(path).expect("read unchanged source"), original);
}

#[tokio::test]
async fn configured_add_does_not_overwrite_unknown_existing_encoding() {
    let (directory, cwd) = setup_project();
    let path = directory.path().join("src/Novo.java");
    let original = [0x81, 0x8d, 0x8f];
    fs::write(&path, original).expect("write invalid Windows-1252 source");

    let result = apply(
        &cwd,
        r#"*** Begin Patch
*** Add File: src/Novo.java
+class Novo {}
*** End Patch"#,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(fs::read(path).expect("read unchanged source"), original);
}

#[tokio::test]
async fn new_matching_file_is_written_as_windows_1252() {
    let (directory, cwd) = setup_project();
    let path = directory.path().join("src/Novo.java");

    apply(
        &cwd,
        r#"*** Begin Patch
*** Add File: src/Novo.java
+// Informação — João
+class Novo {}
*** End Patch"#,
    )
    .await
    .expect("patch should succeed");

    assert_eq!(
        fs::read(path).expect("read new source"),
        b"// Informa\xe7\xe3o \x97 Jo\xe3o\nclass Novo {}\n"
    );
}
