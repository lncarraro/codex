use super::*;
use pretty_assertions::assert_eq;

fn windows_java_policy() -> EncodingPolicy {
    EncodingPolicy::compile(FileEncodingConfig {
        rules: vec![FileEncodingRule {
            globs: vec!["src/**/*.java".to_string()],
            encoding: TextEncoding::Windows1252,
        }],
        ..Default::default()
    })
    .expect("valid policy")
}

#[test]
fn defaults_new_files_to_utf8() {
    let policy = EncodingPolicy::default();
    let bytes = policy
        .encode_new("src/main.rs", "Informação ✅\n")
        .expect("UTF-8 supports Unicode");

    assert_eq!(bytes, "Informação ✅\n".as_bytes());
}

#[test]
fn rejects_disabled_existing_encoding_preservation() {
    let error = EncodingPolicy::compile(FileEncodingConfig {
        preserve_existing: false,
        ..Default::default()
    })
    .expect_err("existing encodings must always be preserved");

    assert!(matches!(
        error,
        TextEncodingError::ExistingEncodingPreservationRequired
    ));
}

#[test]
fn last_matching_rule_wins() {
    let policy = EncodingPolicy::compile(FileEncodingConfig {
        rules: vec![
            FileEncodingRule {
                globs: vec!["src/**/*.java".to_string()],
                encoding: TextEncoding::Windows1252,
            },
            FileEncodingRule {
                globs: vec!["src/modern/**/*.java".to_string()],
                encoding: TextEncoding::Utf8,
            },
        ],
        ..Default::default()
    })
    .expect("valid policy");

    assert_eq!(
        policy.encoding_for_path("src/legacy/Cliente.java"),
        TextEncoding::Windows1252
    );
    assert_eq!(
        policy.encoding_for_path("src/modern/Cliente.java"),
        TextEncoding::Utf8
    );
}

#[test]
fn windows_1252_round_trips_accents_and_typographic_punctuation() {
    let policy = windows_java_policy();
    let original = b"Informa\xe7\xe3o \x97 Jo\xe3o\n";
    let decoded = policy
        .decode_existing("src/legacy/Cliente.java", original)
        .expect("valid Windows-1252");

    assert_eq!(decoded.content, "Informação — João\n");
    assert_eq!(decoded.encoding, TextEncoding::Windows1252);
    assert_eq!(
        policy
            .encode_existing(&decoded, "Informação — José\n")
            .expect("representable text"),
        b"Informa\xe7\xe3o \x97 Jos\xe9\n"
    );
}

#[test]
fn windows_1252_rejects_undefined_bytes() {
    let error = windows_java_policy()
        .decode_existing("src/legacy/Cliente.java", &[0x81, 0x8D, 0x8F, 0x90, 0x9D])
        .expect_err("undefined Windows-1252 bytes must be rejected");

    assert!(matches!(error, TextEncodingError::InvalidWindows1252));
}

#[test]
fn windows_1252_rejects_undefined_control_characters() {
    let error = windows_java_policy()
        .encode_new(
            "src/legacy/Cliente.java",
            "\u{0081}\u{008D}\u{008F}\u{0090}\u{009D}",
        )
        .expect_err("undefined Windows-1252 controls must be rejected");

    assert!(matches!(
        error,
        TextEncodingError::UnrepresentableCharacter {
            encoding: TextEncoding::Windows1252,
            character: '\u{0081}',
            byte_index: 0,
        }
    ));
}

#[test]
fn iso_8859_1_round_trips_portuguese_accents() {
    let policy = EncodingPolicy::compile(FileEncodingConfig {
        default: TextEncoding::Iso8859_1,
        ..Default::default()
    })
    .expect("valid policy");
    let decoded = policy
        .decode_existing("legado.txt", b"Informa\xe7\xe3o de Jo\xe3o\n")
        .expect("valid ISO-8859-1");

    assert_eq!(decoded.content, "Informação de João\n");
    assert_eq!(
        policy
            .encode_existing(&decoded, "Informação de José\n")
            .expect("representable text"),
        b"Informa\xe7\xe3o de Jos\xe9\n"
    );
}

#[test]
fn rejects_unrepresentable_legacy_characters() {
    let policy = windows_java_policy();
    let decoded = policy
        .decode_existing("src/legacy/Cliente.java", b"class Cliente {}\n")
        .expect("ASCII is valid");

    let error = policy
        .encode_existing(&decoded, "// concluído ✅\nclass Cliente {}\n")
        .expect_err("emoji is not representable");

    assert!(matches!(
        error,
        TextEncodingError::UnrepresentableCharacter {
            encoding: TextEncoding::Windows1252,
            character: '✅',
            ..
        }
    ));
}

#[test]
fn preserves_existing_non_ascii_utf8_over_legacy_rule() {
    let policy = windows_java_policy();
    let decoded = policy
        .decode_existing(
            "src/legacy/Cliente.java",
            "// Informação moderna\n".as_bytes(),
        )
        .expect("valid UTF-8");

    assert_eq!(decoded.encoding, TextEncoding::Utf8);
    assert_eq!(
        policy
            .encode_existing(&decoded, "// Informação moderna ✅\n")
            .expect("UTF-8 remains UTF-8"),
        "// Informação moderna ✅\n".as_bytes()
    );
}

#[test]
fn ascii_existing_file_uses_path_rule_for_future_writes() {
    let policy = windows_java_policy();
    let decoded = policy
        .decode_existing("src/legacy/Cliente.java", b"class Cliente {}\n")
        .expect("ASCII is valid");

    assert_eq!(decoded.encoding, TextEncoding::Windows1252);
    assert_eq!(
        policy
            .encode_existing(&decoded, "// João\nclass Cliente {}\n")
            .expect("accent is representable"),
        b"// Jo\xe3o\nclass Cliente {}\n"
    );
}

#[test]
fn preserves_utf8_bom() {
    let policy = EncodingPolicy::default();
    let decoded = policy
        .decode_existing("README.md", b"\xEF\xBB\xBFInforma\xc3\xa7\xc3\xa3o\n")
        .expect("valid UTF-8 BOM file");

    assert!(decoded.has_utf8_bom);
    assert_eq!(
        policy
            .encode_existing(&decoded, "Informação atualizada\n")
            .expect("valid UTF-8"),
        b"\xEF\xBB\xBFInforma\xc3\xa7\xc3\xa3o atualizada\n"
    );
}

#[test]
fn invalid_utf8_requires_a_legacy_rule_or_default() {
    let error = EncodingPolicy::default()
        .decode_existing("Cliente.java", b"Jo\xe3o\n")
        .expect_err("default UTF-8 must not guess a legacy encoding");

    assert!(matches!(error, TextEncodingError::InvalidUtf8(_)));
}
