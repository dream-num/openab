## Context

Current message-processing keeps prompt injection constrained to text, image, and optional STT-transcribed audio blocks. Files that are not injected (unsupported MIME/types, size caps exceeded, etc.) are effectively dropped with only best-effort warnings in some adapters. There is no durable, uniform post-drop handling, and paths referenced by the model are unavailable.

The requested change introduces a new `s3` configuration block so all discarded files can be uploaded to a configurable directory, with optional per-session path partitioning using the existing `agent.per_session_working_dir` capability.

## Goals / Non-Goals

**Goals:**
- Define and implement a configuration-driven path for discarded-file offloading.
- Upload discarded files to the configured S3 directory whenever S3 is configured.
- If `agent.per_session_working_dir` is enabled, append the per-session working directory segment under the configured S3 `directory`.
- Append explicit file-location hints to prompt text so the model can reason about where discarded files were stored.
- Preserve backward compatibility when S3 is not configured.

**Non-Goals:**
- Do not change existing model behavior for files that are currently supported (text/audio/image).
- Do not build UI for browsing S3 files in this change.
- Do not perform automatic OCR/text extraction for non-text files.

## Decisions

### Decision 1: Introduce `s3` config block and treat configured-but-invalid credentials as a runtime no-op with warning
**Alternative A**: Add only a flat global S3 URL and always attempt uploads.
**Alternative B**: Add a typed config block (`bucket`, `directory`, optional endpoint/region/credentials source) with explicit enable condition.

**Choice**: **B**. A typed block is safer and matches existing config style, supports future extension, and makes the feature opt-in by presence of the block.

### Decision 2: Upload only when file is already discarded from prompt path
**Alternative A**: Upload all attachments, regardless of prompt readiness, then always inject location.
**Alternative B**: Upload only files that were otherwise discarded/unusable by prompt injection logic.

**Choice**: **B**. This minimizes S3 traffic, preserves current behavior/performance, and aligns with requirement to handle “all discarded files”.

### Decision 3: Path composition rule in storage and prompt
**Alternative A**: Store raw upload URL in prompt only.
**Alternative B**: Store object keys under the configured `directory`, while reporting the logical working-path (`working_dir/session_working_dir/file`) in the prompt.

**Choice**: **B**. The object key should be controlled by the storage directory, while the prompt still reports `working_dir + session_working_dir(if enable) + file` so agents can reference the file through mounted storage paths.

### Decision 4: Upload implementation location
**Alternative A**: Implement S3 writes separately in each adapter.
**Alternative B**: Centralize discard handling in shared media/dispatch adapter path.

**Choice**: **B**. Centralization ensures identical behavior across Discord/Slack/Gateway and avoids duplicated branching and inconsistent path semantics.

### Decision 5: Failure handling and prompt semantics
**Alternative A**: Fail turn when S3 upload fails.
**Alternative B**: Keep prompt flow and emit explicit warning metadata in attachment text.

**Choice**: **B**. This avoids regressions where one failed upload blocks conversation handling, while still exposing visibility.

## Risks / Trade-offs

- [Risk] S3 credential or network failure prevents upload → [Mitigation] degrade gracefully: keep existing warning + continue turn, optionally add retry/queue in a later change.
- [Risk] Path collisions for same filename under shared directories → [Mitigation] include filename sanitization plus optional deterministic suffix if upload key already exists.
- [Risk] Leaking secrets via prompt text → [Mitigation] only persist key path, never credentials or response URLs; redact sensitive config fields in logs.
- [Risk] Increased prompt token budget due to file metadata lines → [Mitigation] keep path message concise and avoid duplicate entries for already referenced files.

## Migration Plan

1. 在无 `s3` 配置时部署不变：不会上传任何额外文件。
2. 引入配置字段后，重启服务；验证至少一条不支持文件可被落盘上传。
3. 对开启 `agent.per_session_working_dir` 的场景验证路径包含会话级目录。
4. 回滚策略：移除 `s3` 配置块或将其置空即可使系统恢复旧行为，无需额外数据库或持久数据迁移。

## Open Questions

- 是否需要支持私有对象存储兼容端点（如 MinIO）或仅支持 AWS S3 兼容 API？
- 对文件名冲突的策略（覆盖/追加后缀/时间戳）是否有偏好？
- 是否需要将上传后的对象 key 同步返回给用户（例如私有链接）还是仅在 prompt/日志中返回逻辑路径。
