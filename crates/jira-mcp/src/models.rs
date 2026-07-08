use std::collections::BTreeMap;

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolResponse {
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuthSetCredentialsArgs {
    pub profile: Option<String>,
    pub url: Option<String>,
    pub email: Option<String>,
    pub token: Option<String>,
    pub project: Option<String>,
    pub timeout_secs: Option<u64>,
    pub deployment: Option<String>,
    pub auth_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueListArgs {
    pub project_key: Option<String>,
    pub jql: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueKeyArgs {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueViewArgs {
    pub key: String,
    /// Авто-скачать image-вложения в локальный кэш и вернуть local_path. Default: true.
    pub download_images: Option<bool>,
    /// Скачать также не-image вложения. Default: false.
    pub download_all: Option<bool>,
    /// Каталог назначения. Default: <cache>/jira-commands/attachments/<KEY>/.
    pub attachment_dir: Option<String>,
    /// Сжимать картинки (downscale + JPEG). Default: true.
    pub compress: Option<bool>,
    /// Макс. ширина при сжатии. Default: 1280.
    pub max_width: Option<u32>,
    /// Качество JPEG 1..=100 при сжатии. Default: 75.
    pub quality: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentDownloadArgs {
    pub key: String,
    /// Только эти filenames/ids; пусто/None = все.
    pub filenames: Option<Vec<String>>,
    pub images_only: Option<bool>,
    pub attachment_dir: Option<String>,
    /// Сжимать картинки (downscale + JPEG). Default: true.
    pub compress: Option<bool>,
    /// Макс. ширина при сжатии. Default: 1280.
    pub max_width: Option<u32>,
    /// Качество JPEG 1..=100 при сжатии. Default: 75.
    pub quality: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueTypesListArgs {
    pub project_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueFieldsArgs {
    pub project_key: String,
    pub issue_type_id: Option<String>,
    pub required_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueCreateArgs {
    pub project_key: String,
    pub summary: String,
    pub issue_type: String,
    pub description: Option<String>,
    pub description_adf: Option<Value>,
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub labels: Option<Vec<String>>,
    pub components: Option<Vec<String>>,
    pub parent: Option<String>,
    pub fix_versions: Option<Vec<String>>,
    pub custom_fields: Option<BTreeMap<String, Value>>,
    /// Plugin fields (ECCF single/multi-select) that only accept an `update` set
    /// operation with a numeric option-id, e.g. `{"customfield_59170": "625"}`.
    pub set: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueUpdateArgs {
    pub key: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub description_adf: Option<Value>,
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub labels: Option<Vec<String>>,
    pub components: Option<Vec<String>>,
    pub parent: Option<String>,
    pub fix_versions: Option<Vec<String>>,
    pub custom_fields: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueDeleteArgs {
    pub key: String,
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueTransitionArgs {
    pub key: String,
    pub transition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AttachmentInput {
    Path {
        path: String,
    },
    Inline {
        filename: String,
        media_type: Option<String>,
        base64: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueAttachArgs {
    pub key: String,
    pub attachments: Vec<AttachmentInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommentAddArgs {
    pub key: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorklogAddArgs {
    pub key: String,
    pub time_spent: String,
    pub comment: Option<String>,
    pub started: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorklogDeleteArgs {
    pub key: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkTransitionArgs {
    pub jql: String,
    pub to: String,
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkUpdateArgs {
    pub jql: String,
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveArgs {
    pub jql: String,
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApiRequestArgs {
    pub method: String,
    pub path: String,
    pub query: Option<BTreeMap<String, Value>>,
    pub body: Option<Value>,
}
