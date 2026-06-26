use crate::acp::ContentBlock;
use crate::adapter::ChannelRef;
use crate::config::S3OffloadSettings;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tracing::{warn, Instrument};

pub const CAPABILITY_ID: &str = "discarded-file-offload";
const MAX_HINTS_PER_MESSAGE: usize = 10;

#[derive(Debug, Clone)]
pub struct DiscardedFileOffloader {
    settings: S3OffloadSettings,
    working_dir: String,
    per_session_working_dir: bool,
    client: Option<Arc<S3Client>>,
}

#[derive(Debug, Clone)]
pub struct OffloadResult {
    pub filename: String,
    pub working_path: String,
    pub object_key: String,
    pub uploaded: bool,
    pub error: Option<String>,
}

impl DiscardedFileOffloader {
    pub async fn new(
        settings: S3OffloadSettings,
        working_dir: String,
        per_session_working_dir: bool,
    ) -> Self {
        #[cfg(feature = "secrets-aws")]
        let client = Some(Arc::new(S3Client::new(&settings).await));
        #[cfg(not(feature = "secrets-aws"))]
        let client = None;

        Self {
            settings,
            working_dir,
            per_session_working_dir,
            client,
        }
    }

    pub async fn offload_bytes(
        &self,
        channel: &ChannelRef,
        filename: &str,
        bytes: Vec<u8>,
    ) -> OffloadResult {
        let logical_session_key = session_key(channel);
        let path = compute_paths(
            &self.settings,
            &self.working_dir,
            self.per_session_working_dir
                .then_some(logical_session_key.as_str()),
            filename,
        );
        let mut result = OffloadResult {
            filename: sanitize_filename(filename),
            working_path: path.working_path.clone(),
            object_key: path.object_key.clone(),
            uploaded: false,
            error: None,
        };

        let Some(client) = &self.client else {
            result.error = Some("S3 support is not compiled in".to_string());
            warn!(filename = result.filename, key = %result.object_key, "discarded file offload skipped");
            return result;
        };

        match client
            .put_with_collision_suffix(&self.settings.bucket, &path.object_key, bytes)
            .await
        {
            Ok(object_key) => {
                result.uploaded = true;
                result.object_key = object_key;
            }
            Err(e) => {
                let error = e.to_string();
                warn!(
                    filename = result.filename,
                    key = %result.object_key,
                    error = %error,
                    "discarded file offload failed; continuing dispatch"
                );
                result.error = Some(error);
            }
        }
        result
    }
}

pub fn append_hint_block(extra_blocks: &mut Vec<ContentBlock>, result: OffloadResult) {
    let existing = extra_blocks
        .iter()
        .filter(
            |block| matches!(block, ContentBlock::Text { text } if text.contains(CAPABILITY_ID)),
        )
        .count();
    if existing >= MAX_HINTS_PER_MESSAGE {
        return;
    }
    let text = if result.uploaded {
        format!(
            "[{CAPABILITY_ID}] stored discarded file `{}` at `{}` (object key: `{}`).",
            result.filename, result.working_path, result.object_key
        )
    } else {
        format!(
            "[{CAPABILITY_ID}] failed to store discarded file `{}` at `{}`; continuing without offload.",
            result.filename, result.working_path
        )
    };
    if !extra_blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { text: existing } if existing == &text))
    {
        extra_blocks.push(ContentBlock::Text { text });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComputedPaths {
    working_path: String,
    object_key: String,
}

fn compute_paths(
    settings: &S3OffloadSettings,
    working_dir: &str,
    session_key: Option<&str>,
    filename: &str,
) -> ComputedPaths {
    let filename = sanitize_filename(filename);
    let mut working_parts = split_clean_path(working_dir);
    if let Some(session_key) = session_key {
        working_parts.push(sanitize_path_component(session_key, "session"));
    }
    working_parts.push(filename.clone());
    let working_path = format!("/{}", working_parts.join("/"));

    let mut object_parts = split_clean_path(&settings.directory);
    if let Some(session_key) = session_key {
        object_parts.push(sanitize_path_component(session_key, "session"));
    }
    object_parts.push(filename.clone());

    ComputedPaths {
        working_path,
        object_key: object_parts.join("/"),
    }
}

fn session_key(channel: &ChannelRef) -> String {
    let thread_id = channel.thread_id.as_deref().unwrap_or(&channel.channel_id);
    format!("{}:{}", channel.platform, thread_id)
}

fn split_clean_path(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|part| {
            let trimmed = part.trim();
            !trimmed.is_empty() && trimmed != "." && trimmed != ".."
        })
        .map(|part| sanitize_path_component(part, "dir"))
        .collect()
}

fn sanitize_filename(filename: &str) -> String {
    sanitize_path_component(
        filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(filename)
            .trim(),
        "file",
    )
}

fn sanitize_path_component(component: &str, fallback: &str) -> String {
    let mut sanitized = String::with_capacity(component.len());
    for ch in component.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(feature = "secrets-aws")]
#[derive(Debug, Clone)]
struct S3Client {
    inner: aws_sdk_s3::Client,
}

#[cfg(feature = "secrets-aws")]
impl S3Client {
    async fn new(settings: &S3OffloadSettings) -> Self {
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(settings.region.clone()));
        if let (Some(access_key_id), Some(secret_access_key)) = (
            settings.access_key_id.as_deref(),
            settings.secret_access_key.as_deref(),
        ) {
            loader = loader.credentials_provider(aws_credential_types::Credentials::new(
                access_key_id,
                secret_access_key,
                settings.session_token.clone(),
                None,
                "openab-s3-config",
            ));
        }
        if let Some(endpoint_url) = &settings.endpoint_url {
            loader = loader.endpoint_url(endpoint_url);
        }
        let aws_cfg = loader.load().await;
        let mut s3_cfg = aws_sdk_s3::config::Builder::from(&aws_cfg);
        if settings.force_path_style {
            s3_cfg = s3_cfg.force_path_style(true);
        }
        Self {
            inner: aws_sdk_s3::Client::from_conf(s3_cfg.build()),
        }
    }

    async fn put_with_collision_suffix(
        &self,
        bucket: &str,
        object_key: &str,
        bytes: Vec<u8>,
    ) -> Result<String> {
        self.inner
            .put_object()
            .bucket(bucket)
            .key(object_key)
            .body(aws_sdk_s3::primitives::ByteStream::from(bytes))
            .send()
            .instrument(tracing::info_span!(
                "s3_put_discarded_file",
                bucket,
                key = %object_key
            ))
            .await
            .map_err(|err| {
                anyhow!(
                    "failed to put S3 object {bucket}/{object_key}: {}",
                    describe_sdk_error(&err)
                )
            })?;
        Ok(object_key.to_string())
    }
}

#[cfg(feature = "secrets-aws")]
fn describe_sdk_error<E, R>(err: &aws_sdk_s3::error::SdkError<E, R>) -> String
where
    aws_sdk_s3::error::SdkError<E, R>: std::error::Error,
{
    format!("{}", aws_sdk_s3::error::DisplayErrorContext(err))
}

#[cfg(not(feature = "secrets-aws"))]
#[derive(Debug, Clone)]
struct S3Client;

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> S3OffloadSettings {
        S3OffloadSettings {
            bucket: "bucket".into(),
            region: "us-east-1".into(),
            endpoint_url: None,
            force_path_style: true,
            directory: "tenant-a/discarded".into(),
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
        }
    }

    #[test]
    fn compute_key_without_per_session_working_dir() {
        let paths = compute_paths(&settings(), "/home/agent", None, "report.pdf");
        assert_eq!(paths.working_path, "/home/agent/report.pdf");
        assert_eq!(paths.object_key, "tenant-a/discarded/report.pdf");
    }

    #[test]
    fn compute_key_with_per_session_working_dir() {
        let paths = compute_paths(
            &settings(),
            "/home/agent",
            Some("discord:123/456"),
            "report.pdf",
        );
        assert_eq!(paths.working_path, "/home/agent/discord_123_456/report.pdf");
        assert_eq!(
            paths.object_key,
            "tenant-a/discarded/discord_123_456/report.pdf"
        );
    }

    #[test]
    fn filename_sanitization_prevents_path_traversal() {
        let paths = compute_paths(&settings(), "/home/agent", None, "../secret token.pdf");
        assert_eq!(paths.working_path, "/home/agent/secret_token.pdf");
        assert_eq!(paths.object_key, "tenant-a/discarded/secret_token.pdf");
        assert!(!paths.object_key.contains(".."));
    }

    #[test]
    fn hint_contains_capability_and_path() {
        let mut blocks = Vec::new();
        append_hint_block(
            &mut blocks,
            OffloadResult {
                filename: "a.bin".into(),
                working_path: "/home/agent/a.bin".into(),
                object_key: "discarded/home/agent/a.bin".into(),
                uploaded: true,
                error: None,
            },
        );
        assert!(matches!(
            &blocks[0],
            ContentBlock::Text { text }
                if text.contains(CAPABILITY_ID) && text.contains("/home/agent/a.bin")
        ));
    }
}
