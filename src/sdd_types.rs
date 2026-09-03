use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) const PROTOCOL_VERSION: &str = "wiki.sdd/v1";

#[derive(Debug)]
pub enum SddError {
    Invalid(String),
    Io(String),
    Yaml(String),
}

impl std::fmt::Display for SddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Io(message) | Self::Yaml(message) => {
                f.write_str(message)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub phase: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequired {
    pub kind: String,
    pub path: String,
    pub instructions: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub files: Vec<String>,
    pub requirement_refs: Vec<String>,
    pub acceptance_refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveSummary {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationSummary {
    pub result: String,
    pub checks: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SddErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SddResponse {
    pub schema_version: &'static str,
    pub ok: bool,
    pub command: String,
    pub change: Option<ChangeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<ChangeSummary>>,
    pub action_required: Option<ActionRequired>,
    pub task: Option<TaskSummary>,
    pub verification: Option<VerificationSummary>,
    pub moved: Option<MoveSummary>,
    pub artifacts: BTreeMap<String, String>,
    pub next: Vec<String>,
    pub error: Option<SddErrorPayload>,
}

impl SddResponse {
    pub(crate) fn success(command: &str) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            ok: true,
            command: command.to_string(),
            change: None,
            changes: None,
            action_required: None,
            task: None,
            verification: None,
            moved: None,
            artifacts: BTreeMap::new(),
            next: Vec::new(),
            error: None,
        }
    }

    pub(crate) fn error(command: &str, code: &str, message: impl Into<String>) -> Self {
        let mut response = Self::success(command);
        response.ok = false;
        response.error = Some(SddErrorPayload {
            code: code.to_string(),
            message: message.into(),
        });
        response
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ChangeMeta {
    pub(crate) sdd_version: u32,
    pub(crate) change_id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) phase: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(default)]
    pub(crate) verified_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerificationInput {
    #[serde(rename = "schemaVersion")]
    pub(crate) _schema_version: String,
    pub(crate) result: String,
    #[serde(default)]
    pub(crate) tasks: Vec<TaskVerificationInput>,
    #[serde(default)]
    pub(crate) checks: Vec<CheckInput>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskVerificationInput {
    pub(crate) id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckInput {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) evidence_refs: Vec<String>,
    #[serde(default)]
    pub(crate) requirement_refs: Vec<String>,
    #[serde(default)]
    pub(crate) acceptance_refs: Vec<String>,
    #[serde(default)]
    pub(crate) task_refs: Vec<String>,
}
