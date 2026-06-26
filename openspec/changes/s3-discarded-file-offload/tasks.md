## 1. Setup

- [x] 1.1 在 `src/config.rs` 增加 `S3Config` 配置块，支持 `enabled`（可选默认 false）、`bucket`、`region`、`endpoint_url`（可选）、`directory` 等字段，更新 `Config` 聚合结构。
- [x] 1.2 定义 S3 配置缺省与校验逻辑：仅在检测到有效 `s3` 配置时启用该能力；无配置保持当前舍弃行为不变。
- [x] 1.3 更新 `docs/config-reference.md`（如存在）和变更说明，记录新增配置块与参数语义。
- [x] 1.4 若未引入 S3 SDK 依赖，新增 `aws-sdk-s3` 依赖和必要 feature（与现有 aws 依赖策略保持一致）。

## 2. Offload Path and Storage Layer

- [x] 2.1 新增/扩展共享模块（如 `src/media.rs` 或新增 `src/s3_offload.rs`）封装“上传到 S3 + 构建对象 key”能力。
- [x] 2.2 实现 key 计算函数：
  - 无 `per_session_working_dir`：`<directory>/<file_name>`。
  - 有 `per_session_working_dir`：`<directory>/<session_working_dir>/<file_name>`。
- [x] 2.3 增加文件名/路径清理（防止路径遍历）并处理 key 冲突策略（如追加唯一后缀）。
- [x] 2.4 实现上传失败降级策略：记录告警但不阻塞 `dispatch`；仍保留现有 fallback 警告语义。

## 3. Prompt Integration

- [x] 3.1 为“舍弃文件”新增统一元数据提示文本格式（含标识 `discarded-file-offload` 与对象路径）。
- [x] 3.2 在统一附件打包入口（如 `AdapterRouter::pack_arrival_event` 前置/前后处理）或各适配器共享流程中，将提示文本注入到 `extra_blocks`，保持与现有文本顺序一致。
- [x] 3.3 确保提示文本在多文件与批量发送情况下不会重复、不会过载 token（必要时去重/截断策略）。

## 4. Adapter Wiring

- [x] 4.1 修改 Discord 适配器舍弃分支：当图片处理返回 `NotAnImage`/失败且触发 S3 时，执行上传并追加提示 block。
- [x] 4.2 修改 Slack 适配器舍弃分支：统一“未处理文件”路径，成功/失败结果都要可见于提示文本。
- [x] 4.3 修改 Gateway 适配器舍弃分支：支持本地 `path/data` 文件来源的丢弃文件同样走离线对象上传。
- [x] 4.4 对齐现有行为：无 S3 配置时不影响当前 warning/跳过逻辑；有配置时提示从 URL/警告切换为 offload 信息。

## 5. Validation

- [x] 5.1 增加/更新单元测试：验证 key 计算、路径拼接（含 per_session_working_dir 开关）和 prompt 提示文本场景。
- [ ] 5.2 增加端到端测试：覆盖无 S3 配置、S3 成功上传、S3 上传失败三类行为。
- [x] 5.3 运行 `cargo fmt`、`cargo clippy -- -D warnings`、`cargo test` 及本变更相关手工验证用例。
