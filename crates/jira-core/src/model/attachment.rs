use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub size: u64,
    #[serde(rename = "mimeType", default)]
    pub mime_type: String,
    /// Download URL
    pub content: String,
    #[serde(default)]
    pub created: String,
    /// Display name of the uploader
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Локальный путь после скачивания (заполняется download-методами, не из API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// Parse from the raw Jira attachment JSON object.
    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Attachment {
            id: v.get("id")?.as_str()?.to_string(),
            filename: v.get("filename")?.as_str()?.to_string(),
            size: v.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
            mime_type: v
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("application/octet-stream")
                .to_string(),
            content: v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            created: v
                .get("created")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            author: v
                .get("author")
                .and_then(|a| a.get("displayName"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string()),
            local_path: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(mime: &str) -> Attachment {
        Attachment {
            id: "1".into(),
            filename: "f".into(),
            size: 0,
            mime_type: mime.into(),
            content: String::new(),
            created: String::new(),
            author: None,
            local_path: None,
        }
    }

    #[test]
    fn is_image_detects_image_mimes() {
        assert!(att("image/png").is_image());
        assert!(att("image/jpeg").is_image());
        assert!(!att("application/pdf").is_image());
        assert!(!att("application/octet-stream").is_image());
    }
}
