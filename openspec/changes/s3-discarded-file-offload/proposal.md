## Why

OpenAB 目前对无法直接转交给 ACP 的“舍弃文件”（非文本、非音频、非图片）没有统一落盘策略，常见于附件超限或不受支持类型，缺少可追踪和可复现的持久化位置。新增基于 S3 的回收目录后，可将这类文件集中外部化保存，避免因内存/响应限制导致的信息丢失。

当用户启用 `agent.per_session_working_dir` 时，将按会话目录细分上传，可恢复会话上下文中的文件位置关系，便于排障与审计；在 prompt 中回报文件路径则让模型能继续引用这些文件。

## What Changes

- 新增一组 `s3` 配置项：启用/禁用开关、目标存储目录前缀（`bucket/prefix` 风格）以及会话子目录是否启用。
- 对当前“被舍弃文件”分支增加统一处理：当检测到文件未纳入 prompt（例如不支持的 MIME/大小限制场景）时，改为上传到 S3 目标目录。
- 当 `agent.per_session_working_dir` 为 true 时，上传对象 key 使用：`<directory>/<session_working_dir>/<file_name>`。
- 当会话工作目录未启用时，上传对象 key 使用：`<directory>/<file_name>`。
- 在 prompt 中回报的逻辑文件路径仍使用：`<working_dir>/<session_working_dir>/<file_name>` 或 `<working_dir>/<file_name>`。
- 在 prompt 的附件说明块中补充“文件已外部化存储路径”并指向上述路径。
- 提供配置缺省行为：无 S3 配置时保持当前弃置行为，确保现有部署无破坏性变更。

## Capabilities

### New Capabilities

- `discarded-file-offload`: 将当前会话中无法直接注入模型的附件按规则持久化到 S3，并将访问路径写入 prompt，支持 `agent.per_session_working_dir` 下沉。

### Modified Capabilities

- 无（本次只新增能力，不修改既有规范）

## Impact

- 影响配置层：新增 `s3` 配置块解析与校验。
- 影响附件处理路径（Discord/Slack/gateway）中的“unsupported file”处理。
- 影响 adapter → dispatch → ACP prompt 内容构建：增加可复核的外部文件位置信息块。
- 影响运行时依赖：新增/复用 AWS S3 客户端与凭据读取能力。
