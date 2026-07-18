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
const PROJECT_CONFIG_DIRECTORY: &str = ".codex";
const PROJECT_CONFIG_FILE: &str = "config.toml";

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
        Self::load_for_patch(search_from, &[], fs, sandbox).await
    }

    pub(crate) async fn load_for_patch(
        search_from: &PathUri,
        patch_paths: &[PathUri],
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
            let config = match parse_project_config(&config_path, &contents) {
                Ok(config) => config,
                Err(error) => {
                    return Self::repair_policy_or_error(root, config_path, patch_paths, error);
                }
            };
            let Some(file_encoding) = config.file_encoding else {
                continue;
            };
            let policy = match compile_file_encoding(&config_path, file_encoding) {
                Ok(policy) => policy,
                Err(error) => {
                    return Self::repair_policy_or_error(root, config_path, patch_paths, error);
                }
            };
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

    pub(crate) fn validate_project_config(path: &PathUri, content: &str) -> io::Result<()> {
        if !is_project_config_path(path) {
            return Ok(());
        }

        let config = parse_project_config(path, content)?;
        if let Some(file_encoding) = config.file_encoding {
            compile_file_encoding(path, file_encoding)?;
        }
        Ok(())
    }

    fn repair_policy_or_error(
        root: PathUri,
        config_path: PathUri,
        patch_paths: &[PathUri],
        error: io::Error,
    ) -> io::Result<Self> {
        if !patch_paths.is_empty()
            && patch_paths
                .iter()
                .all(|path| paths_equal(path, &config_path))
        {
            return Ok(Self {
                root,
                policy: EncodingPolicy::default(),
                configured: false,
            });
        }

        Err(error)
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

fn parse_project_config(path: &PathUri, content: &str) -> io::Result<ProjectConfigFile> {
    toml::from_str::<ProjectConfigFile>(content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse {}: {error}",
                path.inferred_native_path_string()
            ),
        )
    })
}

fn compile_file_encoding(path: &PathUri, config: FileEncodingConfig) -> io::Result<EncodingPolicy> {
    EncodingPolicy::compile(config).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid file_encoding configuration in {}: {error}",
                path.inferred_native_path_string()
            ),
        )
    })
}

fn is_project_config_path(path: &PathUri) -> bool {
    let Some(convention) = path.infer_path_convention() else {
        return false;
    };
    let path_text = path.inferred_native_path_string();
    let segments = non_empty_segments(convention, &path_text);
    let [.., directory, file] = segments.as_slice() else {
        return false;
    };

    path_segment_eq(convention, directory, PROJECT_CONFIG_DIRECTORY)
        && path_segment_eq(convention, file, PROJECT_CONFIG_FILE)
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

fn paths_equal(left: &PathUri, right: &PathUri) -> bool {
    let left_url = left.to_url();
    let right_url = right.to_url();
    if left_url.scheme() != right_url.scheme() || left_url.host_str() != right_url.host_str() {
        return false;
    }
    let Some(convention) = left.infer_path_convention() else {
        return false;
    };
    if right.infer_path_convention() != Some(convention) {
        return false;
    }

    let left_path = left.inferred_native_path_string();
    let right_path = right.inferred_native_path_string();
    let left_segments = non_empty_segments(convention, &left_path);
    let right_segments = non_empty_segments(convention, &right_path);
    left_segments.len() == right_segments.len()
        && left_segments
            .iter()
            .zip(right_segments)
            .all(|(left, right)| path_segment_eq(convention, left, right))
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
