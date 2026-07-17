//! Strict decoding and encoding for editable project text files.
//!
//! Codex keeps text as Unicode internally while this crate preserves the byte
//! encoding selected for each file at the filesystem boundary.

use encoding_rs::WINDOWS_1252;
use globset::Glob;
use globset::GlobBuilder;
use globset::GlobSet;
use globset::GlobSetBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use thiserror::Error;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// A text encoding supported for editable project files.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum TextEncoding {
    /// Unicode UTF-8.
    #[default]
    #[serde(rename = "utf-8", alias = "utf8")]
    Utf8,
    /// ISO-8859-1 (Latin-1), interpreted strictly as Unicode U+0000..U+00FF.
    #[serde(rename = "iso-8859-1", alias = "latin1", alias = "latin-1")]
    Iso8859_1,
    /// Microsoft Windows code page 1252. The `ansi` alias is accepted for
    /// compatibility with projects that use that informal label.
    #[serde(rename = "windows-1252", alias = "cp1252", alias = "ansi")]
    Windows1252,
}

impl fmt::Display for TextEncoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Utf8 => "UTF-8",
            Self::Iso8859_1 => "ISO-8859-1",
            Self::Windows1252 => "Windows-1252",
        })
    }
}

/// One path-based encoding rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FileEncodingRule {
    /// Workspace-relative glob patterns. When multiple rules match, the last
    /// matching rule wins.
    pub globs: Vec<String>,
    /// Encoding selected for matching paths.
    pub encoding: TextEncoding,
}

/// Project text encoding configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FileEncodingConfig {
    /// Encoding used when no rule matches. Defaults to UTF-8.
    #[serde(default)]
    pub default: TextEncoding,
    /// Preserve a recognizable encoding for existing non-ASCII files.
    #[serde(default = "default_true")]
    pub preserve_existing: bool,
    /// Reject unrepresentable characters rather than replacing them.
    /// Only strict mode is supported.
    #[serde(default = "default_true")]
    pub strict: bool,
    /// Ordered path rules. The last matching rule wins.
    #[serde(default)]
    pub rules: Vec<FileEncodingRule>,
}

impl Default for FileEncodingConfig {
    fn default() -> Self {
        Self {
            default: TextEncoding::Utf8,
            preserve_existing: true,
            strict: true,
            rules: Vec::new(),
        }
    }
}

const fn default_true() -> bool {
    true
}

/// Text decoded for editing together with the metadata required to write it
/// without changing its byte encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedText {
    /// Unicode text exposed to the editing pipeline.
    pub content: String,
    /// Encoding to use when the edited text is written.
    pub encoding: TextEncoding,
    /// Whether an existing UTF-8 byte-order mark must be retained.
    pub has_utf8_bom: bool,
}

#[derive(Debug)]
struct CompiledRule {
    globs: GlobSet,
    encoding: TextEncoding,
}

/// Compiled project policy used to decode and encode editable text files.
#[derive(Debug)]
pub struct EncodingPolicy {
    default: TextEncoding,
    preserve_existing: bool,
    rules: Vec<CompiledRule>,
}

impl Default for EncodingPolicy {
    fn default() -> Self {
        Self {
            default: TextEncoding::Utf8,
            preserve_existing: true,
            rules: Vec::new(),
        }
    }
}

impl EncodingPolicy {
    /// Compiles path globs and validates a project configuration.
    pub fn compile(config: FileEncodingConfig) -> Result<Self, TextEncodingError> {
        if !config.strict {
            return Err(TextEncodingError::LossyModeUnsupported);
        }

        let rules = config
            .rules
            .into_iter()
            .map(CompiledRule::compile)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            default: config.default,
            preserve_existing: config.preserve_existing,
            rules,
        })
    }

    /// Returns the encoding selected by the default and ordered glob rules.
    pub fn encoding_for_path(&self, path: &str) -> TextEncoding {
        let normalized_path = path.replace('\\', "/");
        self.rules
            .iter()
            .filter(|rule| rule.globs.is_match(&normalized_path))
            .map(|rule| rule.encoding)
            .next_back()
            .unwrap_or(self.default)
    }

    /// Decodes an existing file and records the encoding to preserve on write.
    ///
    /// Valid non-ASCII UTF-8 and UTF-8 with BOM are recognizable and preserved.
    /// ASCII files are ambiguous, so their write encoding comes from path rules.
    /// A non-UTF-8 file must have a matching legacy rule or legacy default.
    pub fn decode_existing(
        &self,
        path: &str,
        bytes: &[u8],
    ) -> Result<DecodedText, TextEncodingError> {
        if let Some(without_bom) = bytes.strip_prefix(UTF8_BOM) {
            let content = decode_utf8(without_bom)?;
            return Ok(DecodedText {
                content,
                encoding: TextEncoding::Utf8,
                has_utf8_bom: true,
            });
        }

        let configured = self.encoding_for_path(path);
        if !self.preserve_existing {
            return decode_as(bytes, configured, false);
        }

        match std::str::from_utf8(bytes) {
            Ok(content) => Ok(DecodedText {
                content: content.to_string(),
                encoding: if content.is_ascii() {
                    configured
                } else {
                    TextEncoding::Utf8
                },
                has_utf8_bom: false,
            }),
            Err(_) => decode_as(bytes, configured, false),
        }
    }

    /// Encodes a newly created file according to its path rule or the default.
    pub fn encode_new(&self, path: &str, content: &str) -> Result<Vec<u8>, TextEncodingError> {
        encode_text(content, self.encoding_for_path(path), false)
    }

    /// Encodes edited text using metadata returned by [`Self::decode_existing`].
    pub fn encode_existing(
        &self,
        decoded: &DecodedText,
        content: &str,
    ) -> Result<Vec<u8>, TextEncodingError> {
        encode_text(content, decoded.encoding, decoded.has_utf8_bom)
    }
}

impl CompiledRule {
    fn compile(rule: FileEncodingRule) -> Result<Self, TextEncodingError> {
        if rule.globs.is_empty() {
            return Err(TextEncodingError::EmptyRule);
        }

        let mut builder = GlobSetBuilder::new();
        for pattern in rule.globs {
            let glob = build_glob(&pattern)
                .map_err(|source| TextEncodingError::InvalidGlob { pattern, source })?;
            builder.add(glob);
        }
        let globs = builder.build().map_err(TextEncodingError::BuildGlobSet)?;
        Ok(Self {
            globs,
            encoding: rule.encoding,
        })
    }
}

fn build_glob(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

fn decode_as(
    bytes: &[u8],
    encoding: TextEncoding,
    has_utf8_bom: bool,
) -> Result<DecodedText, TextEncodingError> {
    let content = match encoding {
        TextEncoding::Utf8 => decode_utf8(bytes)?,
        TextEncoding::Iso8859_1 => bytes.iter().copied().map(char::from).collect(),
        TextEncoding::Windows1252 => {
            if bytes.iter().copied().any(is_undefined_windows_1252_byte) {
                return Err(TextEncodingError::InvalidWindows1252);
            }
            let (decoded, had_errors) = WINDOWS_1252.decode_without_bom_handling(bytes);
            if had_errors {
                return Err(TextEncodingError::InvalidWindows1252);
            }
            decoded.into_owned()
        }
    };
    Ok(DecodedText {
        content,
        encoding,
        has_utf8_bom,
    })
}

fn decode_utf8(bytes: &[u8]) -> Result<String, TextEncodingError> {
    String::from_utf8(bytes.to_vec()).map_err(TextEncodingError::InvalidUtf8)
}

fn encode_text(
    content: &str,
    encoding: TextEncoding,
    has_utf8_bom: bool,
) -> Result<Vec<u8>, TextEncodingError> {
    match encoding {
        TextEncoding::Utf8 => {
            let mut bytes = Vec::with_capacity(content.len() + usize::from(has_utf8_bom) * 3);
            if has_utf8_bom {
                bytes.extend_from_slice(UTF8_BOM);
            }
            bytes.extend_from_slice(content.as_bytes());
            Ok(bytes)
        }
        TextEncoding::Iso8859_1 => content
            .char_indices()
            .map(|(byte_index, character)| {
                u8::try_from(character as u32).map_err(|_| {
                    TextEncodingError::UnrepresentableCharacter {
                        encoding,
                        character,
                        byte_index,
                    }
                })
            })
            .collect(),
        TextEncoding::Windows1252 => {
            if let Some((byte_index, character)) = first_unrepresentable_windows_1252(content) {
                return Err(TextEncodingError::UnrepresentableCharacter {
                    encoding,
                    character,
                    byte_index,
                });
            }
            let (encoded, _, had_errors) = WINDOWS_1252.encode(content);
            if had_errors {
                return Err(TextEncodingError::UnrepresentableCharacter {
                    encoding,
                    character: '\u{FFFD}',
                    byte_index: 0,
                });
            }
            Ok(encoded.into_owned())
        }
    }
}

fn first_unrepresentable_windows_1252(content: &str) -> Option<(usize, char)> {
    content.char_indices().find(|(_, character)| {
        if is_undefined_windows_1252_character(*character) {
            return true;
        }
        let value = character.to_string();
        let (_, _, had_errors) = WINDOWS_1252.encode(&value);
        had_errors
    })
}

const fn is_undefined_windows_1252_byte(byte: u8) -> bool {
    matches!(byte, 0x81 | 0x8D | 0x8F | 0x90 | 0x9D)
}

const fn is_undefined_windows_1252_character(character: char) -> bool {
    matches!(
        character,
        '\u{0081}' | '\u{008D}' | '\u{008F}' | '\u{0090}' | '\u{009D}'
    )
}

/// Error returned before an unsafe or lossy text conversion can be committed.
#[derive(Debug, Error)]
pub enum TextEncodingError {
    /// The file is not valid UTF-8 under the selected policy.
    #[error("file is not valid UTF-8: {0}")]
    InvalidUtf8(#[source] std::string::FromUtf8Error),
    /// The file contains bytes undefined by Windows-1252.
    #[error("file contains bytes that are undefined in Windows-1252")]
    InvalidWindows1252,
    /// An edited character cannot be represented by the target encoding.
    #[error(
        "character {character:?} at UTF-8 byte offset {byte_index} is not representable in {encoding}"
    )]
    UnrepresentableCharacter {
        /// Encoding selected for the file.
        encoding: TextEncoding,
        /// First character that cannot be represented.
        character: char,
        /// UTF-8 byte offset of that character in the edited string.
        byte_index: usize,
    },
    /// A configured path glob is invalid.
    #[error("invalid file encoding glob {pattern:?}: {source}")]
    InvalidGlob {
        /// Invalid glob pattern.
        pattern: String,
        /// Glob parser error.
        #[source]
        source: globset::Error,
    },
    /// Compiling a set of otherwise valid globs failed.
    #[error("failed to compile file encoding globs: {0}")]
    BuildGlobSet(#[source] globset::Error),
    /// Rules must contain at least one glob.
    #[error("file encoding rules must contain at least one glob")]
    EmptyRule,
    /// Lossy replacement would risk silent source corruption.
    #[error("file_encoding.strict=false is not supported; conversions must be strict")]
    LossyModeUnsupported,
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
