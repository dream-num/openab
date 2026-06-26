## ADDED Requirements

### Requirement: Discarded file offload to S3
The system SHALL upload non-injectable uploaded files to S3 when an S3 configuration block is present and valid.

#### Scenario: S3 configured and non-injectable file encountered
- **WHEN** an incoming attachment is not injected into the prompt (for example unsupported file type, size cap exceeded, or media transform failure)
- **THEN** the system SHALL attempt to upload the original file bytes to the configured S3 directory.
- **THEN** the system SHALL emit a prompt text hint containing the computed storage path for that file.

#### Scenario: No S3 configured
- **WHEN** no valid `s3` configuration block is present
- **THEN** the system SHALL keep existing discard behavior and MUST NOT attempt S3 upload.
- **THEN** existing adapter warning behavior (if any) SHALL remain unchanged.

### Requirement: Per-session object key composition
The system SHALL compute the S3 object key under the configured S3 `directory` and include `session_working_dir` when enabled.

#### Scenario: per_session_working_dir enabled
- **WHEN** `agent.per_session_working_dir` is enabled and a discarded file is uploaded
- **THEN** the object key SHALL be `<directory>/<session_working_dir>/<file_name>`.
- **THEN** the prompt hint SHALL include the object key and the logical working path `<working_dir>/<session_working_dir>/<file_name>`.

#### Scenario: per_session_working_dir disabled
- **WHEN** `agent.per_session_working_dir` is disabled and a discarded file is uploaded
- **THEN** the object key SHALL be `<directory>/<file_name>`.
- **THEN** the prompt hint SHALL include the object key and the logical working path `<working_dir>/<file_name>`.

### Requirement: File path hint formatting in prompt
The system SHALL provide a machine-readable but concise text hint when a discarded file is offloaded.

#### Scenario: Prompt hint emitted after successful offload
- **WHEN** an offload succeeds
- **THEN** the hint SHALL include the capability identifier `discarded-file-offload` and the computed file path.
- **THEN** the hint SHALL be placed in arrival-event attachment blocks so it remains aligned with the triggering message.

### Requirement: Graceful handling on upload failure
The system SHALL not fail the whole turn when S3 upload fails.

#### Scenario: Upload failure
- **WHEN** S3 upload returns an error
- **THEN** the system SHALL continue prompt dispatch without blocking the user turn.
- **THEN** the system SHALL preserve existing user-facing warning behavior for unsupported files.
