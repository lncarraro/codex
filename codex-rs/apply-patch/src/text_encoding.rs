use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_text_encoding::DecodedText;
use codex_text_encoding::EncodingPolicy;
use codex_text_encoding::FileEncodingConfig;
use codex_utils_path_uri::PathConvention;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use std::io;

const PROJECT_CONFIG_PATH: &str = ".codex/config.toml";

#[derive(Debug, Default, Deserialize)]
struct ProjectConfigFile {
    #[serde(default)]
    file_encoding: Option<FileEncodingConfig>,
}

pub(crate) struct ProjectEncodingPolicy {
    root: PathUri,
    policy: EncodingPolicy,
    configured: bool,
}

impl ProjectEncodingPolicy {
    pub(crate) async fn load(
        search_from: &PathUri,
        fs: &dyn ExecutorFileSystem,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> io::Result<Self> {
        for root in search_from.ancestors() {
            let config_path = root
                .join(PROJECT_CONFIG_PATH)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            let bytes = match fs.read_file(&config_path, sandbox).await {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let contents = String::from_utf8(bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} must be UTF-8: {error}",
                        config_path.inferred_native_path_string()
                    ),
                )
            })?;
            let config = toml::from_str::<ProjectConfigFile>(&contents).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "failed to parse {}: {error}",
                        config_path.inferred_native_path_string()
                    ),
                )
            })?;
            let Some(file_encoding) = config.file_encoding else {
                continue;
            };
            let policy = EncodingPolicy::compile(file_encoding).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "invalid file_encoding configuration in {}: {error}",
                        config_path.inferred_native_path_string()
                    ),
                )
            })?;
            return Ok(Self {
                root,
                policy,
                configured: true,
            });
        }

        Ok(Self {
            root: search_from.clone(),
            policy: EncodingPolicy::default(),
            configured: false,
        })
    }

    pub(crate) async fn read_text(
        &self,
        path: &PathUri,
        fs: &dyn ExecutorFileSystem,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> io::Result<DecodedText> {
        let bytes = fs.read_file(path, sandbox).await?;
        self.decode(path, &bytes)
    }

    pub(crate) fn decode(&self, path: &PathUri, bytes: &[u8]) -> io::Result<DecodedText> {
        self.policy
            .decode_existing(&self.policy_path(path), bytes)
            .map_err(|error| encoding_error(path, error))
    }

    pub(crate) fn encode_new(&self, path: &PathUri, content: &str) -> io::Result<Vec<u8>> {
        self.policy
            .encode_new(&self.policy_path(path), content)
            .map_err(|error| encoding_error(path, error))
    }

    pub(crate) fn encode_existing(
        &self,
        path: &PathUri,
        decoded: &DecodedText,
        content: &str,
    ) -> io::Result<Vec<u8>> {
        self.policy
            .encode_existing(decoded, content)
            .map_err(|error| encoding_error(path, error))
    }

    pub(crate) async fn encode_add(
        &self,
        path: &PathUri,
        content: &str,
        fs: &dyn ExecutorFileSystem,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> io::Result<Vec<u8>> {
        match fs.read_file(path, sandbox).await {
            Ok(bytes) => match self.decode(path, &bytes) {
                Ok(decoded) => self.encode_existing(path, &decoded, content),
                Err(_) if !self.configured => self.encode_new(path, content),
                Err(error) => Err(error),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.encode_new(path, content),
            Err(error) => Err(error),
        }
    }

    fn policy_path(&self, path: &PathUri) -> String {
        relative_path(&self.root, path).unwrap_or_else(|| path.inferred_native_path_string())
    }
}

fn encoding_error(path: &PathUri, error: impl std::error::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "failed to convert text file {}: {error}",
            path.inferred_native_path_string()
        ),
    )
}

fn relative_path(root: &PathUri, path: &PathUri) -> Option<String> {
    let root_url = root.to_url();
    let path_url = path.to_url();
    if root_url.scheme() != path_url.scheme() || root_url.host_str() != path_url.host_str() {
        return None;
    }
    let convention = root.infer_path_convention()?;
    if path.infer_path_convention()? != convention {
        return None;
    }

    let root_path = root.inferred_native_path_string();
    let target_path = path.inferred_native_path_string();
    let root_segments = non_empty_segments(convention, &root_path);
    let target_segments = non_empty_segments(convention, &target_path);
    if root_segments.len() > target_segments.len() {
        return None;
    }
    let prefix_matches = root_segments
        .iter()
        .zip(&target_segments)
        .all(|(left, right)| path_segment_eq(convention, left, right));
    if !prefix_matches {
        return None;
    }

    Some(target_segments[root_segments.len()..].join("/"))
}

fn non_empty_segments(convention: PathConvention, path: &str) -> Vec<&str> {
    convention
        .path_segments(path)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn path_segment_eq(convention: PathConvention, left: &str, right: &str) -> bool {
    match convention {
        PathConvention::Posix => left == right,
        PathConvention::Windows => left.eq_ignore_ascii_case(right),
    }
}

#[cfg(test)]
#[path = "text_encoding_tests.rs"]
mod tests;
