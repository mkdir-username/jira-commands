use std::time::Duration;

use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Client, Response, StatusCode,
};
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::{
    adf::markdown_to_adf,
    config::{JiraAuthType, JiraConfig, JiraDeployment},
    error::{JiraError, Result},
    model::{
        attachment::Attachment,
        comment::Comment,
        field::Field,
        issue::{
            CreateIssueRequest, CreateIssueRequestV2, Issue, RawIssue, RawSearchResponse,
            SearchResult, UpdateIssueRequest,
        },
        worklog::Worklog,
    },
};

const AGILE_BASE: &str = "/rest/agile/1.0";
const MAX_RETRIES: u32 = 3;

#[derive(Clone)]
pub struct JiraClient {
    http: Client,
    config: JiraConfig,
}

/// Параметры скачивания вложений задачи.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Пропускать не-image вложения.
    pub images_only: bool,
    /// Даунскейл + JPEG-перекодирование картинок (retina-PNG неуместен для тех-контекста).
    pub compress: bool,
    /// Максимальная ширина при compress (картинки уже неё не апскейлятся).
    pub max_width: u32,
    /// Качество JPEG (1..=100) при compress.
    pub quality: u8,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            images_only: false,
            compress: true,
            max_width: 1280,
            quality: 75,
        }
    }
}

/// Пере-кодировать картинку в JPEG, ЕСЛИ это оправдано и реально уменьшает размер.
/// Сжимаем только когда: шире max_width (нужен downscale) ИЛИ это PNG (lossless retina-скрин —
/// главный источник bloat'а). Уже-маленький JPEG ≤max_width оставляем как есть.
/// `Some(jpeg)` только если результат строго меньше оригинала (do-no-harm); иначе `None`.
fn maybe_compress(bytes: &[u8], max_width: u32, quality: u8) -> Option<Vec<u8>> {
    let format = image::guess_format(bytes).ok()?;
    let img = image::load_from_memory(bytes).ok()?;

    let too_wide = img.width() > max_width;
    let is_png = format == image::ImageFormat::Png;
    if !too_wide && !is_png {
        return None; // already-JPEG разумного размера — не трогаем
    }

    let img = if too_wide {
        img.resize(max_width, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&img.to_rgb8())
        .ok()?;

    (out.len() < bytes.len()).then_some(out)
}

impl JiraClient {
    pub fn new(config: JiraConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        Self { http, config }
    }

    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn platform_url(&self, path: &str) -> String {
        format!(
            "{}/rest/api/{}{}",
            self.config.base_url.trim_end_matches('/'),
            self.config.api_version,
            path
        )
    }

    #[allow(dead_code)]
    fn agile_url(&self, path: &str) -> String {
        format!(
            "{}{}{}",
            self.config.base_url.trim_end_matches('/'),
            AGILE_BASE,
            path
        )
    }

    fn auth_headers(&self) -> Result<HeaderMap> {
        let token = self.config.token.as_deref().ok_or_else(|| {
            JiraError::Auth("No token configured. Run `jirac auth login` first.".into())
        })?;

        let auth_value = match self.config.auth_type {
            JiraAuthType::CloudApiToken | JiraAuthType::DataCenterBasic => {
                let credentials = base64_encode(&format!("{}:{}", self.config.email, token));
                format!("Basic {credentials}")
            }
            JiraAuthType::DataCenterPat => format!("Bearer {token}"),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value)
                .map_err(|e| JiraError::Auth(format!("Invalid auth header: {e}")))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Ok(headers)
    }

    /// Auth headers without Content-Type — required for multipart uploads.
    fn auth_headers_no_content_type(&self) -> Result<HeaderMap> {
        let token = self.config.token.as_deref().ok_or_else(|| {
            JiraError::Auth("No token configured. Run `jirac auth login` first.".into())
        })?;

        let auth_value = match self.config.auth_type {
            JiraAuthType::CloudApiToken | JiraAuthType::DataCenterBasic => {
                let credentials = base64_encode(&format!("{}:{}", self.config.email, token));
                format!("Basic {credentials}")
            }
            JiraAuthType::DataCenterPat => format!("Bearer {token}"),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value)
                .map_err(|e| JiraError::Auth(format!("Invalid auth header: {e}")))?,
        );
        Ok(headers)
    }

    /// Field identifying a user in request bodies: `accountId` on Cloud, `name` on Data Center.
    fn user_ref_field(&self) -> &'static str {
        match self.config.deployment {
            JiraDeployment::DataCenter => "name",
            JiraDeployment::Cloud => "accountId",
        }
    }

    /// Get the current authenticated user's identifier (accountId on Cloud, name on DC).
    pub async fn get_myself(&self) -> Result<String> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/myself");
        let field = self.user_ref_field();

        let http = &self.http;
        let user: serde_json::Value = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        user.get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| JiraError::Api {
                status: 0,
                message: format!("Could not get {field} from /myself"),
            })
    }

    /// Resolve an assignee string to the user object Jira expects in `fields.assignee`.
    ///
    /// - `"me"` → current user via /myself
    /// - contains `@` → search by email, take the first match
    /// - anything else → used verbatim (accountId on Cloud, login on Data Center)
    ///
    /// Cloud yields `{"accountId": "..."}`, Data Center `{"name": "..."}` — DC has no accountId.
    async fn resolve_assignee_ref(&self, s: &str) -> Result<Value> {
        let field = self.user_ref_field();

        let id = if s == "me" {
            self.get_myself().await?
        } else if !s.contains('@') {
            s.to_string()
        } else {
            let users = self.search_users(s).await?;
            users
                .iter()
                .find(|u| {
                    u.get("emailAddress")
                        .and_then(|v| v.as_str())
                        .map(|e| e.eq_ignore_ascii_case(s))
                        .unwrap_or(false)
                })
                .or_else(|| users.first())
                .and_then(|u| u.get(field))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| JiraError::Api {
                    status: 0,
                    message: format!("User not found: {s}"),
                })?
        };

        Ok(json!({ field: id }))
    }

    /// Core request method with rate-limit retry logic.
    async fn request<T>(&self, builder_fn: impl Fn() -> reqwest::RequestBuilder) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let req = builder_fn();
            let response = req.send().await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);

                warn!("Rate limited. Retrying after {}s", retry_after);

                if attempt >= MAX_RETRIES {
                    return Err(JiraError::RateLimit { retry_after });
                }

                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            return handle_response(response).await;
        }
    }

    /// Core request method for responses with no body (204 No Content).
    async fn request_no_body(
        &self,
        builder_fn: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<()> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let req = builder_fn();
            let response = req.send().await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);

                warn!("Rate limited. Retrying after {}s", retry_after);

                if attempt >= MAX_RETRIES {
                    return Err(JiraError::RateLimit { retry_after });
                }

                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let status = response.status();
            if status.is_success() {
                return Ok(());
            }

            let body = response.text().await.unwrap_or_default();
            if status == StatusCode::NOT_FOUND {
                return Err(JiraError::NotFound(body));
            }
            return Err(JiraError::Api {
                status: status.as_u16(),
                message: body,
            });
        }
    }

    /// Core request для бинарных ответов (attachment download) с rate-limit retry.
    async fn request_bytes(
        &self,
        builder_fn: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<Vec<u8>> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let response = builder_fn().send().await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);

                warn!("Rate limited. Retrying after {}s", retry_after);

                if attempt >= MAX_RETRIES {
                    return Err(JiraError::RateLimit { retry_after });
                }

                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let status = response.status();
            if status == StatusCode::NOT_FOUND {
                return Err(JiraError::NotFound(
                    response.text().await.unwrap_or_default(),
                ));
            }
            if !status.is_success() {
                return Err(JiraError::Api {
                    status: status.as_u16(),
                    message: response.text().await.unwrap_or_default(),
                });
            }
            return Ok(response.bytes().await?.to_vec());
        }
    }

    /// Скачать произвольный авторизованный URL (полный absolute, напр. attachment.content).
    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let headers = self.auth_headers_no_content_type()?;
        let http = &self.http;
        self.request_bytes(|| http.get(url).headers(headers.clone()))
            .await
    }

    /// Скачать содержимое одного вложения по его `content` URL.
    pub async fn download_attachment(&self, attachment: &Attachment) -> Result<Vec<u8>> {
        if attachment.content.is_empty() {
            return Err(JiraError::NotFound(format!(
                "attachment {} has no content URL",
                attachment.id
            )));
        }
        self.fetch_bytes(&attachment.content).await
    }

    /// Скачать вложения задачи в каталог. Возвращает копии Attachment c заполненным local_path.
    /// opts.images_only=true пропускает не-image mime. При opts.compress картинки даунскейлятся
    /// до opts.max_width и пере-кодируются в JPEG (имя → `.jpg`) — retina-PNG неуместен для
    /// технического контекста. Идемпотентно: raw — по совпадению размера, сжатые — по наличию файла.
    pub async fn download_issue_attachments(
        &self,
        attachments: &[Attachment],
        dir: &std::path::Path,
        opts: &DownloadOptions,
    ) -> Result<Vec<Attachment>> {
        std::fs::create_dir_all(dir)?;
        let mut out = Vec::new();
        for att in attachments {
            if opts.images_only && !att.is_image() {
                continue;
            }
            // sanitize: имя из API недоверенное — берём только базовое имя (защита от path traversal `../`, `/`)
            let safe_name = std::path::Path::new(&att.filename)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("attachment-{}", att.id));

            let want_compress = opts.compress && att.is_image();
            let stem = std::path::Path::new(&safe_name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| safe_name.clone());
            let raw_target = dir.join(&safe_name);
            let jpg_target = dir.join(format!("{stem}.jpg"));

            // идемпотентность: файл (в любой из двух форм) уже на месте
            let existing = if want_compress && jpg_target.exists() {
                Some(jpg_target.clone())
            } else if raw_target.exists() {
                Some(raw_target.clone())
            } else {
                None
            };

            let final_path = if let Some(p) = existing {
                p
            } else {
                let bytes = self.download_attachment(att).await?;
                let (out_bytes, target) = match want_compress
                    .then(|| maybe_compress(&bytes, opts.max_width, opts.quality))
                    .flatten()
                {
                    Some(jpg) => (jpg, jpg_target),
                    None => (bytes, raw_target),
                };
                std::fs::write(&target, &out_bytes)?;
                target
            };

            let mut copy = att.clone();
            copy.local_path = Some(final_path.to_string_lossy().into_owned());
            out.push(copy);
        }
        Ok(out)
    }

    /// Multipart request with rate-limit retry (for attachment uploads).
    async fn request_multipart<T>(
        &self,
        builder_fn: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let req = builder_fn();
            let response = req.send().await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);

                warn!("Rate limited. Retrying after {}s", retry_after);

                if attempt >= MAX_RETRIES {
                    return Err(JiraError::RateLimit { retry_after });
                }

                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            return handle_response(response).await;
        }
    }

    /// Search issues using JQL with cursor-based pagination.
    pub async fn search_issues(
        &self,
        jql: &str,
        next_page_token: Option<&str>,
        max_results: Option<u32>,
    ) -> Result<SearchResult> {
        let headers = self.auth_headers()?;
        let url = if self.config.api_version >= 3 {
            self.platform_url("/search/jql")
        } else {
            self.platform_url("/search")
        };

        let mut body = json!({
            "jql": jql,
            "maxResults": max_results.unwrap_or(50),
            "fields": ["summary", "status", "assignee", "reporter", "priority",
                       "issuetype", "project", "created", "updated", "description"]
        });

        if self.config.api_version >= 3 {
            if let Some(token) = next_page_token {
                body["nextPageToken"] = json!(token);
            }
        } else if let Some(token) = next_page_token {
            if let Ok(start_at) = token.parse::<u64>() {
                body["startAt"] = json!(start_at);
            }
        }

        debug!("Searching JQL: {}", jql);

        let http = &self.http;
        let raw: RawSearchResponse = self
            .request(|| http.post(&url).headers(headers.clone()).json(&body))
            .await?;

        Ok(SearchResult {
            issues: raw.issues.into_iter().map(|r| r.into_issue()).collect(),
            next_page_token: raw.next_page_token,
            total: raw.total,
        })
    }

    /// Fetch a single issue by key.
    pub async fn get_issue(&self, key: &str) -> Result<Issue> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{key}"));

        let http = &self.http;
        let raw: RawIssue = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(raw.into_issue())
    }

    /// Create a new issue.
    pub async fn create_issue(&self, req: CreateIssueRequest) -> Result<Issue> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/issue");

        let description_adf = req.description.as_deref().map(markdown_to_adf);

        let mut fields = json!({
            "project": { "key": req.project_key },
            "summary": req.summary,
            "issuetype": { "name": req.issue_type }
        });

        if let Some(adf) = description_adf {
            fields["description"] = adf;
        }

        if let Some(assignee) = &req.assignee {
            fields["assignee"] = self.resolve_assignee_ref(assignee).await?;
        }

        if let Some(priority) = &req.priority {
            fields["priority"] = json!({ "name": priority });
        }

        let body = json!({ "fields": fields });

        #[derive(serde::Deserialize)]
        struct CreateResponse {
            key: String,
        }

        let http = &self.http;
        let resp: CreateResponse = self
            .request(|| http.post(&url).headers(headers.clone()).json(&body))
            .await?;

        // Fetch the full issue after creation
        self.get_issue(&resp.key).await
    }

    /// Update an existing issue.
    pub async fn update_issue(&self, key: &str, req: UpdateIssueRequest) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{key}"));

        let mut fields = json!({});

        if let Some(summary) = &req.summary {
            fields["summary"] = json!(summary);
        }
        if let Some(adf) = &req.description_adf {
            fields["description"] = adf.clone();
        } else if let Some(description) = &req.description {
            fields["description"] = markdown_to_adf(description);
        }
        if let Some(assignee) = &req.assignee {
            fields["assignee"] = self.resolve_assignee_ref(assignee).await?;
        }
        if let Some(priority) = &req.priority {
            fields["priority"] = json!({ "name": priority });
        }
        if let Some(labels) = &req.labels {
            fields["labels"] = json!(labels);
        }
        if let Some(components) = &req.components {
            fields["components"] = json!(components
                .iter()
                .map(|c| json!({"name": c}))
                .collect::<Vec<_>>());
        }
        if let Some(fix_versions) = &req.fix_versions {
            fields["fixVersions"] = json!(fix_versions
                .iter()
                .map(|v| json!({"name": v}))
                .collect::<Vec<_>>());
        }
        if let Some(parent) = &req.parent {
            fields["parent"] = json!({ "key": parent });
        }
        for (field_id, value) in &req.custom_fields {
            fields[field_id] = value.to_api_json();
        }

        let body = json!({ "fields": fields });

        let http = &self.http;
        self.request_no_body(|| http.put(&url).headers(headers.clone()).json(&body))
            .await
    }

    /// Delete an issue.
    pub async fn delete_issue(&self, key: &str) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{key}"));

        let http = &self.http;
        self.request_no_body(|| http.delete(&url).headers(headers.clone()))
            .await
    }

    /// Get fields available for a project (runtime field resolution — no hardcoding).
    pub async fn get_project_fields(&self, project_key: &str) -> Result<Vec<Field>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/createmeta/{project_key}/issuetypes"));

        #[derive(serde::Deserialize)]
        struct IssueTypeMeta {
            // Cloud returns `{issueTypes: [{fields: ...}]}`; DC returns a paginated
            // `{values: [...]}` whose entries carry no per-type `fields` — DC callers
            // must fetch fields per issue type via `get_fields_for_issue_type`.
            #[serde(rename = "issueTypes", alias = "values")]
            issue_types: Vec<IssueTypeDetail>,
        }

        #[derive(serde::Deserialize)]
        struct IssueTypeDetail {
            fields: Option<std::collections::HashMap<String, FieldMeta>>,
        }

        #[derive(serde::Deserialize)]
        struct FieldMeta {
            name: String,
            required: bool,
            schema: Option<Value>,
        }

        let http = &self.http;
        let meta: IssueTypeMeta = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        let mut fields: Vec<Field> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for it in meta.issue_types {
            if let Some(field_map) = it.fields {
                for (id, meta) in field_map {
                    if seen.insert(id.clone()) {
                        let field_type = meta
                            .schema
                            .as_ref()
                            .and_then(|s| s.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();

                        fields.push(Field {
                            id,
                            name: meta.name,
                            field_type,
                            required: meta.required,
                            schema: meta.schema,
                            allowed_values: None,
                        });
                    }
                }
            }
        }

        Ok(fields)
    }

    /// Get server info (used to detect Jira tier).
    pub async fn get_server_info(&self) -> Result<Value> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/serverInfo");

        let http = &self.http;
        self.request(|| http.get(&url).headers(headers.clone()))
            .await
    }

    /// Transition an issue to a new status.
    ///
    /// Backward-compat wrapper around [`Self::transition_issue_with_fields`] that
    /// sends only `{"transition": {"id": ...}}` without workflow fields. Status
    /// will change but resolution stays untouched (typically `Unresolved`),
    /// which is usually undesirable when transitioning to a `done`-category status.
    /// Prefer [`Self::transition_issue_with_fields`] when closing an issue.
    pub async fn transition_issue(&self, key: &str, transition_id: &str) -> Result<()> {
        self.transition_issue_with_fields(key, transition_id, None)
            .await
    }

    /// Transition an issue, optionally setting workflow fields (e.g. `resolution`).
    ///
    /// Sends `{"transition": {"id": id}, "fields": {...}}` when `fields` is `Some`
    /// and non-empty. Used to atomically apply a transition together with required
    /// workflow fields like `resolution`. Inspect each transition's `fields` map
    /// (returned by `GET /transitions`) to learn which fields are allowed/required.
    pub async fn transition_issue_with_fields(
        &self,
        key: &str,
        transition_id: &str,
        fields: Option<&serde_json::Map<String, Value>>,
    ) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{key}/transitions"));

        let mut body = json!({
            "transition": { "id": transition_id }
        });
        if let Some(f) = fields {
            if !f.is_empty() {
                body["fields"] = Value::Object(f.clone());
            }
        }

        let http = &self.http;
        self.request_no_body(|| http.post(&url).headers(headers.clone()).json(&body))
            .await
    }

    /// Get available transitions for an issue.
    pub async fn get_transitions(&self, key: &str) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{key}/transitions"));

        #[derive(serde::Deserialize)]
        struct TransitionsResponse {
            transitions: Vec<Value>,
        }

        let http = &self.http;
        let resp: TransitionsResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(resp.transitions)
    }

    /// Get available issue types for a project (id + name).
    pub async fn get_issue_types(&self, project_key: &str) -> Result<Vec<IssueType>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/createmeta/{project_key}/issuetypes"));

        #[derive(serde::Deserialize)]
        struct MetaResponse {
            // Cloud returns `{issueTypes: [...]}`; DC returns a paginated `{values: [...]}`.
            #[serde(rename = "issueTypes", alias = "values")]
            issue_types: Vec<IssueType>,
        }

        let http = &self.http;
        let resp: MetaResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(resp.issue_types)
    }

    /// Get fields for a specific issue type within a project (with allowed values).
    pub async fn get_fields_for_issue_type(
        &self,
        project_key: &str,
        issue_type_id: &str,
    ) -> Result<Vec<Field>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!(
            "/issue/createmeta/{project_key}/issuetypes/{issue_type_id}"
        ));

        #[derive(serde::Deserialize)]
        struct FieldMetaResponse {
            // Cloud returns `{fields: ...}`; DC returns a paginated `{values: [...]}`.
            #[serde(rename = "fields", alias = "values")]
            fields: FieldCollection,
        }

        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum FieldCollection {
            Map(std::collections::HashMap<String, FieldMetaMap>),
            List(Vec<FieldMetaEntry>),
        }

        #[derive(serde::Deserialize)]
        struct FieldMetaMap {
            name: String,
            required: bool,
            schema: Option<Value>,
            #[serde(rename = "allowedValues")]
            allowed_values: Option<Vec<Value>>,
        }

        #[derive(serde::Deserialize)]
        struct FieldMetaEntry {
            #[serde(rename = "fieldId")]
            field_id: Option<String>,
            key: Option<String>,
            name: String,
            required: bool,
            schema: Option<Value>,
            #[serde(rename = "allowedValues")]
            allowed_values: Option<Vec<Value>>,
        }

        let http = &self.http;
        let resp: FieldMetaResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        let fields = match resp.fields {
            FieldCollection::Map(fields) => fields
                .into_iter()
                .map(|(id, meta)| {
                    let field_type = meta
                        .schema
                        .as_ref()
                        .and_then(|s| s.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    Field {
                        id,
                        name: meta.name,
                        field_type,
                        required: meta.required,
                        schema: meta.schema,
                        allowed_values: meta.allowed_values,
                    }
                })
                .collect(),
            FieldCollection::List(fields) => fields
                .into_iter()
                .map(|meta| {
                    let field_type = meta
                        .schema
                        .as_ref()
                        .and_then(|s| s.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let id = meta.field_id.or(meta.key).unwrap_or_default();

                    Field {
                        id,
                        name: meta.name,
                        field_type,
                        required: meta.required,
                        schema: meta.schema,
                        allowed_values: meta.allowed_values,
                    }
                })
                .collect(),
        };

        Ok(fields)
    }

    /// Search Jira users by query string (for User field autocomplete).
    ///
    /// Data Center's `/user/search` takes `username`; Cloud takes `query`.
    pub async fn search_users(&self, query: &str) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/user/search");
        let param = match self.config.deployment {
            JiraDeployment::DataCenter => "username",
            JiraDeployment::Cloud => "query",
        };

        let http = &self.http;
        let users: Vec<Value> = self
            .request(|| {
                http.get(&url)
                    .headers(headers.clone())
                    .query(&[(param, query), ("maxResults", "20")])
            })
            .await?;

        Ok(users)
    }

    /// List components available within a project.
    pub async fn get_project_components(&self, project_key: &str) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/project/{project_key}/components"));

        let http = &self.http;
        let components: Vec<Value> = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(components)
    }

    /// List fix versions available within a project.
    pub async fn get_project_versions(&self, project_key: &str) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/project/{project_key}/versions"));

        let http = &self.http;
        let versions: Vec<Value> = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(versions)
    }

    /// Upload a file as an attachment to an issue.
    pub async fn upload_attachment(
        &self,
        issue_key: &str,
        file_path: &std::path::Path,
    ) -> Result<Vec<Attachment>> {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        let bytes = std::fs::read(file_path)?;
        let mime = mime_guess::from_path(file_path)
            .first_or_octet_stream()
            .to_string();

        self.upload_attachment_bytes(issue_key, &file_name, bytes, Some(&mime))
            .await
    }

    /// Upload an in-memory attachment to an issue.
    pub async fn upload_attachment_bytes(
        &self,
        issue_key: &str,
        file_name: &str,
        bytes: Vec<u8>,
        media_type: Option<&str>,
    ) -> Result<Vec<Attachment>> {
        use reqwest::{header::HeaderValue, multipart};

        let headers = self.auth_headers_no_content_type()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/attachments"));
        let mime = media_type
            .map(|value| value.to_string())
            .or_else(|| {
                mime_guess::from_path(file_name)
                    .first_raw()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let http = &self.http;
        let raw_attachments: Vec<Value> = self
            .request_multipart(|| {
                let part = multipart::Part::bytes(bytes.clone())
                    .file_name(file_name.to_string())
                    .mime_str(&mime)
                    .expect("invalid mime type");
                let form = multipart::Form::new().part("file", part);

                let mut req_headers = headers.clone();
                req_headers.insert("X-Atlassian-Token", HeaderValue::from_static("no-check"));

                http.post(&url).headers(req_headers).multipart(form)
            })
            .await?;

        Ok(raw_attachments
            .iter()
            .filter_map(Attachment::from_value)
            .collect())
    }

    /// Create a new issue with dynamic custom fields.
    pub async fn create_issue_v2(&self, req: CreateIssueRequestV2) -> Result<Issue> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/issue");

        let description_adf = req
            .description_adf
            .or_else(|| req.description.as_deref().map(markdown_to_adf));

        let mut fields = json!({
            "project": { "key": req.project_key },
            "summary": req.summary,
            "issuetype": { "name": req.issue_type }
        });

        if let Some(adf) = description_adf {
            fields["description"] = adf;
        }
        if let Some(assignee) = &req.assignee {
            fields["assignee"] = self.resolve_assignee_ref(assignee).await?;
        }
        if let Some(priority) = &req.priority {
            fields["priority"] = json!({ "name": priority });
        }
        if !req.labels.is_empty() {
            fields["labels"] = json!(req.labels);
        }
        if !req.components.is_empty() {
            fields["components"] = json!(req
                .components
                .iter()
                .map(|c| json!({"name": c}))
                .collect::<Vec<_>>());
        }
        if let Some(parent) = &req.parent {
            fields["parent"] = json!({ "key": parent });
        }
        if !req.fix_versions.is_empty() {
            fields["fixVersions"] = json!(req
                .fix_versions
                .iter()
                .map(|v| json!({"name": v}))
                .collect::<Vec<_>>());
        }
        for (field_id, value) in &req.custom_fields {
            fields[field_id] = value.to_api_json();
        }

        let mut body = json!({ "fields": fields });
        if !req.update_ops.is_empty() {
            body["update"] = Value::Object(req.update_ops.clone());
        }

        #[derive(serde::Deserialize)]
        struct CreateResponse {
            key: String,
        }

        let http = &self.http;
        let resp: CreateResponse = self
            .request(|| http.post(&url).headers(headers.clone()).json(&body))
            .await?;

        self.get_issue(&resp.key).await
    }

    // ── Comments ─────────────────────────────────────────────────────────────

    /// List all comments for an issue.
    pub async fn get_comments(&self, issue_key: &str) -> Result<Vec<Comment>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/comment"));

        #[derive(serde::Deserialize)]
        struct CommentResponse {
            comments: Vec<Value>,
        }

        let http = &self.http;
        let resp: CommentResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(resp
            .comments
            .iter()
            .filter_map(|v| Comment::from_value(v, issue_key))
            .collect())
    }

    /// Add a comment to an issue.
    pub async fn add_comment(&self, issue_key: &str, body: &str) -> Result<Comment> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/comment"));

        let payload = json!({
            "body": markdown_to_adf(body)
        });

        let http = &self.http;
        let raw: Value = self
            .request(|| http.post(&url).headers(headers.clone()).json(&payload))
            .await?;

        Comment::from_value(&raw, issue_key).ok_or_else(|| JiraError::Api {
            status: 0,
            message: "Failed to parse comment".into(),
        })
    }

    // ── Worklog ──────────────────────────────────────────────────────────────

    /// List all worklogs for an issue.
    pub async fn get_worklogs(&self, issue_key: &str) -> Result<Vec<Worklog>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/worklog"));

        #[derive(serde::Deserialize)]
        struct WorklogResponse {
            worklogs: Vec<Value>,
        }

        let http = &self.http;
        let resp: WorklogResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(resp
            .worklogs
            .iter()
            .filter_map(|v| Worklog::from_value(v, issue_key))
            .collect())
    }

    /// Add a worklog entry to an issue.
    /// `time_spent` uses Jira format: "2h 30m", "1d", "45m"
    /// `started` is optional ISO 8601 timestamp; defaults to now if None.
    pub async fn add_worklog(
        &self,
        issue_key: &str,
        time_spent: &str,
        comment: Option<&str>,
        started: Option<&str>,
    ) -> Result<Worklog> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/worklog"));

        // Jira requires started in "2006-01-02T15:04:05.000+0000" format
        let started_str = started
            .map(|s| s.to_string())
            .unwrap_or_else(current_jira_timestamp);

        let mut body = json!({
            "timeSpent": time_spent,
            "started": started_str,
        });

        if let Some(c) = comment {
            body["comment"] = markdown_to_adf(c);
        }

        let http = &self.http;
        let raw: Value = self
            .request(|| http.post(&url).headers(headers.clone()).json(&body))
            .await?;

        Worklog::from_value(&raw, issue_key).ok_or_else(|| JiraError::Api {
            status: 0,
            message: "Failed to parse worklog".into(),
        })
    }

    /// Delete a worklog entry.
    pub async fn delete_worklog(&self, issue_key: &str, worklog_id: &str) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/worklog/{worklog_id}"));

        let http = &self.http;
        self.request_no_body(|| http.delete(&url).headers(headers.clone()))
            .await
    }

    /// Delete a comment from an issue.
    pub async fn delete_comment(&self, issue_key: &str, comment_id: &str) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/comment/{comment_id}"));

        let http = &self.http;
        self.request_no_body(|| http.delete(&url).headers(headers.clone()))
            .await
    }

    /// Delete an attachment by ID.
    pub async fn delete_attachment(&self, attachment_id: &str) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/attachment/{attachment_id}"));

        let http = &self.http;
        self.request_no_body(|| http.delete(&url).headers(headers.clone()))
            .await
    }

    /// List remote links on an issue.
    pub async fn get_remote_links(&self, issue_key: &str) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/remotelink"));

        let http = &self.http;
        self.request(|| http.get(&url).headers(headers.clone()))
            .await
    }

    /// Add a remote link to an issue.
    pub async fn add_remote_link(
        &self,
        issue_key: &str,
        url_str: &str,
        title: &str,
    ) -> Result<Value> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/remotelink"));

        let payload = json!({
            "object": {
                "url": url_str,
                "title": title,
            }
        });

        let http = &self.http;
        self.request(|| http.post(&url).headers(headers.clone()).json(&payload))
            .await
    }

    /// Delete a remote link from an issue.
    pub async fn delete_remote_link(&self, issue_key: &str, link_id: &str) -> Result<()> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/issue/{issue_key}/remotelink/{link_id}"));

        let http = &self.http;
        self.request_no_body(|| http.delete(&url).headers(headers.clone()))
            .await
    }

    // ── Bulk ops ─────────────────────────────────────────────────────────────

    /// Fetch ALL issues matching a JQL query using cursor-based pagination.
    /// Respects the Atlassian safeguard: max 500 pages.
    pub async fn get_all_issues(&self, jql: &str) -> Result<Vec<Issue>> {
        let mut all_issues = Vec::new();
        let mut next_page_token: Option<String> = None;
        let mut iterations = 0u32;
        const MAX_ITERATIONS: u32 = 500;

        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                break;
            }

            let result = self
                .search_issues(jql, next_page_token.as_deref(), Some(100))
                .await?;

            all_issues.extend(result.issues);

            match result.next_page_token {
                Some(token) => next_page_token = Some(token),
                None => break,
            }
        }

        Ok(all_issues)
    }

    /// Archive a batch of issues by key. Jira accepts up to 1000 per request.
    pub async fn archive_issues(&self, issue_keys: &[String]) -> Result<()> {
        if issue_keys.is_empty() {
            return Ok(());
        }
        let headers = self.auth_headers()?;
        let url = self.platform_url("/issue/archive");

        // Batch in chunks of 1000
        for chunk in issue_keys.chunks(1000) {
            let body = json!({ "issueIdsOrKeys": chunk });
            let http = &self.http;
            // Archive returns 200 with a body — use request() not request_no_body()
            let _: Value = self
                .request(|| http.put(&url).headers(headers.clone()).json(&body))
                .await?;
        }

        Ok(())
    }

    // ── Raw API passthrough ───────────────────────────────────────────────────

    /// Execute an arbitrary Jira REST API call and return the raw JSON response.
    /// Returns `None` for 204 No Content responses (success with no body).
    /// `path` should start with `/rest/...`
    pub async fn raw_request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        let headers = self.auth_headers()?;
        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), path);

        let http = &self.http;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let req = match method.to_uppercase().as_str() {
                "GET" => http.get(&url),
                "POST" => http.post(&url),
                "PUT" => http.put(&url),
                "DELETE" => http.delete(&url),
                "PATCH" => http.patch(&url),
                _ => http.get(&url),
            };
            let req = req.headers(headers.clone());
            let req = if let Some(b) = &body {
                req.json(b)
            } else {
                req
            };

            let response = req.send().await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);
                warn!("Rate limited. Retrying after {}s", retry_after);
                if attempt >= MAX_RETRIES {
                    return Err(JiraError::RateLimit { retry_after });
                }
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let status = response.status();

            // 204 No Content — success with empty body
            if status == StatusCode::NO_CONTENT {
                return Ok(None);
            }

            if status.is_success() {
                let value: Value = response.json().await?;
                return Ok(Some(value));
            }

            let body_text = response.text().await.unwrap_or_default();
            return Err(match status {
                StatusCode::NOT_FOUND => JiraError::NotFound(body_text),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    JiraError::Auth(format!("HTTP {status}: {body_text}"))
                }
                _ => JiraError::Api {
                    status: status.as_u16(),
                    message: body_text,
                },
            });
        }
    }

    // ── Plans API (Jira Premium) ──────────────────────────────────────────────

    /// Check if this Jira instance is Premium tier.
    pub async fn is_premium(&self) -> bool {
        match self.get_server_info().await {
            Ok(info) => {
                let license = info
                    .get("deploymentType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // "Cloud" with advanced features, or check licenseInfo
                let _ = license;
                // Simplest heuristic: try to call the plans endpoint
                let headers = match self.auth_headers() {
                    Ok(h) => h,
                    Err(_) => return false,
                };
                let url = self.platform_url("/plans/plan");
                let http = &self.http;
                matches!(
                    http.get(&url).headers(headers).send().await,
                    Ok(r) if r.status().is_success()
                )
            }
            Err(_) => false,
        }
    }

    /// List Jira Plans (requires Jira Premium / Advanced Roadmaps).
    pub async fn get_plans(&self) -> Result<Vec<Value>> {
        let headers = self.auth_headers()?;
        let url = self.platform_url("/plans/plan");

        #[derive(serde::Deserialize)]
        struct PlansResponse {
            values: Vec<Value>,
        }

        let http = &self.http;
        let resp: PlansResponse = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        Ok(resp.values)
    }

    /// Resolve a project key (e.g. `PAYDAY`) to its numeric id (e.g. `91001`).
    pub async fn get_project_id(&self, project_key: &str) -> Result<String> {
        let headers = self.auth_headers()?;
        let url = self.platform_url(&format!("/project/{project_key}"));

        let http = &self.http;
        let project: Value = self
            .request(|| http.get(&url).headers(headers.clone()))
            .await?;

        project
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| JiraError::NotFound(format!("project {project_key} has no id")))
    }

    /// Fetch selectable options for an ECCF (Extended Context Custom Field) select
    /// field, scoped by project and issue type. Returns each option's numeric id —
    /// the value ECCF fields expect via an `update` set operation.
    pub async fn eccf_select_options(
        &self,
        field_id: &str,
        project_id: &str,
        issue_type_id: &str,
    ) -> Result<Vec<EccfOption>> {
        let headers = self.auth_headers()?;
        let url = format!(
            "{}/rest/eccf/1.0/context/select/options",
            self.config.base_url.trim_end_matches('/')
        );
        // Context param types are Gson `@SerializedName` numbers: PROJECT="1", ISSUE_TYPE="2".
        let params = format!(
            r#"[{{"type":"1","valueIds":[{project_id}]}},{{"type":"2","valueIds":[{issue_type_id}]}}]"#
        );

        let http = &self.http;
        let options: Vec<EccfOption> = self
            .request(|| {
                http.get(&url)
                    .headers(headers.clone())
                    .query(&[("fieldId", field_id), ("params", params.as_str())])
            })
            .await?;

        Ok(options)
    }
}

/// Issue type metadata (id + name) returned by createmeta.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct IssueType {
    pub id: String,
    pub name: String,
}

/// A single selectable option of an ECCF select field.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EccfOption {
    pub id: i64,
    pub title: String,
    #[serde(default, rename = "isDisabled")]
    pub is_disabled: bool,
    #[serde(default, rename = "isRequired")]
    pub is_required: bool,
}

async fn handle_response<T>(response: Response) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();

    if status.is_success() {
        // 204/205: no body — callers expecting a body should use request_no_body().
        // Defensive: try to deserialize from null (works for Value and Option<T>).
        if status == StatusCode::NO_CONTENT || status == StatusCode::RESET_CONTENT {
            return serde_json::from_value(serde_json::Value::Null).map_err(|_| JiraError::Api {
                status: status.as_u16(),
                message: "Unexpected empty response body".into(),
            });
        }
        let value: T = response.json().await?;
        return Ok(value);
    }

    let body = response.text().await.unwrap_or_default();

    match status {
        StatusCode::NOT_FOUND => Err(JiraError::NotFound(body)),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Err(JiraError::Auth(format!("HTTP {status}: {body}")))
        }
        _ => Err(JiraError::Api {
            status: status.as_u16(),
            message: body,
        }),
    }
}

/// Returns current UTC time in Jira worklog format: "2006-01-02T15:04:05.000+0000"
fn current_jira_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Manual conversion: secs since epoch → date/time components
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    // Days since epoch
    let days = secs / 86400;
    // Simplified: use a rough date calculation
    // For worklog "started", accuracy to the day is sufficient
    let year_approx = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000+0000",
        year_approx,
        month.min(12),
        day.min(28),
        h,
        m,
        s
    )
}

fn base64_encode(input: &str) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut result = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() {
            bytes[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() {
            bytes[i + 2] as u32
        } else {
            0
        };

        let _ = write!(result, "{}", CHARS[((b0 >> 2) & 0x3F) as usize] as char);
        let _ = write!(
            result,
            "{}",
            CHARS[(((b0 & 0x3) << 4) | ((b1 >> 4) & 0xF)) as usize] as char
        );
        if i + 1 < bytes.len() {
            let _ = write!(
                result,
                "{}",
                CHARS[(((b1 & 0xF) << 2) | ((b2 >> 6) & 0x3)) as usize] as char
            );
        } else {
            result.push('=');
        }
        if i + 2 < bytes.len() {
            let _ = write!(result, "{}", CHARS[(b2 & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        i += 3;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{JiraAuthType, JiraDeployment};
    use wiremock::{
        matchers::{body_json, header, method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    fn dc_test_client(base_url: String) -> JiraClient {
        JiraClient::new(JiraConfig {
            profile_name: Some("dc-test".into()),
            base_url,
            email: String::new(),
            token: Some("dc-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::DataCenter,
            auth_type: JiraAuthType::DataCenterPat,
            api_version: 2,
        })
    }

    #[tokio::test]
    async fn request_bytes_returns_raw_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blob"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());
        let url = format!("{}/blob", server.uri());
        let bytes = client.fetch_bytes(&url).await.unwrap();
        assert_eq!(bytes, vec![1u8, 2, 3]);
    }

    #[tokio::test]
    async fn download_attachment_fetches_content_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/secure/attachment/42/img.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNGDATA".to_vec()))
            .mount(&server)
            .await;

        let att = Attachment {
            id: "42".into(),
            filename: "img.png".into(),
            size: 7,
            mime_type: "image/png".into(),
            content: format!("{}/secure/attachment/42/img.png", server.uri()),
            created: String::new(),
            author: None,
            local_path: None,
        };
        let client = dc_test_client(server.uri());
        let bytes = client.download_attachment(&att).await.unwrap();
        assert_eq!(bytes, b"PNGDATA");
    }

    #[tokio::test]
    async fn download_issue_attachments_saves_images_to_dir() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/secure/attachment/1/a.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"DATA".to_vec()))
            .mount(&server)
            .await;

        let mk = |id: &str, name: &str, mime: &str| Attachment {
            id: id.into(),
            filename: name.into(),
            size: 4,
            mime_type: mime.into(),
            content: format!("{}/secure/attachment/{id}/{name}", server.uri()),
            created: String::new(),
            author: None,
            local_path: None,
        };
        let atts = vec![
            mk("1", "a.png", "image/png"),
            mk("2", "b.pdf", "application/pdf"),
        ];
        let dir = tempfile::tempdir().unwrap();
        let client = dc_test_client(server.uri());

        // compress=false → проверяем чистую filter+to-dir логику без перекодирования
        let opts = DownloadOptions {
            images_only: true,
            compress: false,
            ..DownloadOptions::default()
        };
        let saved = client
            .download_issue_attachments(&atts, dir.path(), &opts)
            .await
            .unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved[0].local_path.as_ref().unwrap().ends_with("a.png"));
        assert!(dir.path().join("a.png").exists());
        assert!(!dir.path().join("b.pdf").exists());
    }

    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        let buf = image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x % 256) as u8, 0, 0]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(buf)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn sample_jpeg(w: u32, h: u32) -> Vec<u8> {
        let buf = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&buf)
            .unwrap();
        out
    }

    #[test]
    fn maybe_compress_downscales_wide_png_to_jpeg() {
        let png = sample_png(3000, 100);
        let jpg = maybe_compress(&png, 1280, 75).expect("compressed");
        let decoded = image::load_from_memory(&jpg).expect("decode jpg");
        assert!(decoded.width() <= 1280, "width {} > 1280", decoded.width());
        assert_eq!(image::guess_format(&jpg).unwrap(), image::ImageFormat::Jpeg);
        assert!(
            jpg.len() < png.len(),
            "jpg {} !< png {}",
            jpg.len(),
            png.len()
        );
    }

    #[test]
    fn maybe_compress_skips_small_jpeg() {
        // уже-JPEG в пределах max_width — не трогаем (иначе бы раздули)
        let jpg = sample_jpeg(400, 300);
        assert!(maybe_compress(&jpg, 1280, 75).is_none());
    }

    #[test]
    fn maybe_compress_returns_none_on_garbage() {
        assert!(maybe_compress(b"not-an-image", 1280, 75).is_none());
    }

    #[tokio::test]
    async fn data_center_pat_uses_bearer_and_api_v2() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/rest/api/2/serverInfo"))
            .and(header("authorization", "Bearer dc-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "deploymentType": "Data Center",
                "version": "10.0.0"
            })))
            .mount(&server)
            .await;

        let client = JiraClient::new(JiraConfig {
            profile_name: Some("dc-main".into()),
            base_url: server.uri(),
            email: String::new(),
            token: Some("dc-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::DataCenter,
            auth_type: JiraAuthType::DataCenterPat,
            api_version: 2,
        });

        let info = client.get_server_info().await.expect("server info");
        assert_eq!(info["deploymentType"], Value::String("Data Center".into()));
    }

    #[tokio::test]
    async fn cloud_auth_uses_basic_and_api_v3() {
        let server = MockServer::start().await;
        let expected = format!("Basic {}", base64_encode("dev@example.com:cloud-token"));

        Mock::given(method("GET"))
            .and(path("/rest/api/3/serverInfo"))
            .and(header("authorization", expected.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "deploymentType": "Cloud",
                "version": "1001.0.0"
            })))
            .mount(&server)
            .await;

        let client = JiraClient::new(JiraConfig {
            profile_name: Some("cloud-main".into()),
            base_url: server.uri(),
            email: "dev@example.com".into(),
            token: Some("cloud-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::Cloud,
            auth_type: JiraAuthType::CloudApiToken,
            api_version: 3,
        });

        let info = client.get_server_info().await.expect("server info");
        assert_eq!(info["deploymentType"], Value::String("Cloud".into()));
    }

    #[tokio::test]
    async fn get_fields_for_issue_type_supports_map_response() {
        let server = MockServer::start().await;
        let expected = format!("Basic {}", base64_encode("dev@example.com:cloud-token"));

        Mock::given(method("GET"))
            .and(path("/rest/api/3/issue/createmeta/TEST/issuetypes/10001"))
            .and(header("authorization", expected.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "fields": {
                    "summary": {
                        "name": "Summary",
                        "required": true,
                        "schema": { "type": "string" }
                    }
                }
            })))
            .mount(&server)
            .await;

        let client = JiraClient::new(JiraConfig {
            profile_name: Some("cloud-main".into()),
            base_url: server.uri(),
            email: "dev@example.com".into(),
            token: Some("cloud-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::Cloud,
            auth_type: JiraAuthType::CloudApiToken,
            api_version: 3,
        });

        let fields = client
            .get_fields_for_issue_type("TEST", "10001")
            .await
            .expect("map response should parse");

        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].id, "summary");
        assert_eq!(fields[0].name, "Summary");
        assert!(fields[0].required);
        assert_eq!(fields[0].field_type, "string");
    }

    #[tokio::test]
    async fn get_fields_for_issue_type_supports_list_response() {
        let server = MockServer::start().await;
        let expected = format!("Basic {}", base64_encode("dev@example.com:cloud-token"));

        Mock::given(method("GET"))
            .and(path("/rest/api/3/issue/createmeta/TEST/issuetypes/10002"))
            .and(header("authorization", expected.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "fields": [
                    {
                        "fieldId": "customfield_10553",
                        "key": "customfield_10553",
                        "name": "Labels (OSS)",
                        "required": true,
                        "schema": {
                            "custom": "com.atlassian.jira.plugin.system.customfieldtypes:labels",
                            "items": "string",
                            "type": "array"
                        },
                        "allowedValues": []
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = JiraClient::new(JiraConfig {
            profile_name: Some("cloud-main".into()),
            base_url: server.uri(),
            email: "dev@example.com".into(),
            token: Some("cloud-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::Cloud,
            auth_type: JiraAuthType::CloudApiToken,
            api_version: 3,
        });

        let fields = client
            .get_fields_for_issue_type("TEST", "10002")
            .await
            .expect("list response should parse");

        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].id, "customfield_10553");
        assert_eq!(fields[0].name, "Labels (OSS)");
        assert!(fields[0].required);
        assert_eq!(fields[0].field_type, "array");
    }

    #[tokio::test]
    async fn get_issue_types_supports_dc_values_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/2/issue/createmeta/PAYDAY/issuetypes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "isLast": true,
                "maxResults": 50,
                "startAt": 0,
                "total": 1,
                "values": [
                    { "id": "10300", "name": "Development", "subtask": true }
                ]
            })))
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());
        let types = client
            .get_issue_types("PAYDAY")
            .await
            .expect("DC paginated values response should parse");

        assert_eq!(types.len(), 1);
        assert_eq!(types[0].id, "10300");
        assert_eq!(types[0].name, "Development");
    }

    #[tokio::test]
    async fn get_fields_for_issue_type_supports_dc_values_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/2/issue/createmeta/PAYDAY/issuetypes/10300"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "isLast": true,
                "maxResults": 50,
                "startAt": 0,
                "total": 1,
                "values": [
                    {
                        "fieldId": "customfield_59170",
                        "name": "Delivery component",
                        "required": true,
                        "schema": {
                            "type": "option",
                            "custom": "ru.alfabank.atlassian.jira.eccf:eccf-single-select-type"
                        }
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());
        let fields = client
            .get_fields_for_issue_type("PAYDAY", "10300")
            .await
            .expect("DC paginated values response should parse");

        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].id, "customfield_59170");
        assert_eq!(fields[0].name, "Delivery component");
        assert!(fields[0].required);
    }

    #[tokio::test]
    async fn eccf_select_options_parses_option_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/eccf/1.0/context/select/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": 625, "title": "JS", "isDisabled": false, "isRequired": false }
            ])))
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());
        let opts = client
            .eccf_select_options("59170", "91001", "10300")
            .await
            .expect("eccf options should parse");

        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].id, 625);
        assert_eq!(opts[0].title, "JS");
    }

    #[tokio::test]
    async fn transition_with_fields_sends_resolution_in_body() {
        let server = MockServer::start().await;
        let expected_body = json!({
            "transition": {"id": "321"},
            "fields": {"resolution": {"name": "Done"}}
        });

        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue/PAYDAY-1/transitions"))
            .and(header("authorization", "Bearer dc-token"))
            .and(body_json(&expected_body))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());

        let mut fields = serde_json::Map::new();
        fields.insert("resolution".into(), json!({"name": "Done"}));

        client
            .transition_issue_with_fields("PAYDAY-1", "321", Some(&fields))
            .await
            .expect("transition should succeed");
    }

    #[tokio::test]
    async fn transition_without_fields_omits_fields_key() {
        let server = MockServer::start().await;
        let expected_body = json!({"transition": {"id": "321"}});

        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue/PAYDAY-1/transitions"))
            .and(header("authorization", "Bearer dc-token"))
            .and(body_json(&expected_body))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = dc_test_client(server.uri());

        client
            .transition_issue("PAYDAY-1", "321")
            .await
            .expect("legacy transition should succeed");
    }

    fn cloud_test_client(base_url: String) -> JiraClient {
        JiraClient::new(JiraConfig {
            profile_name: Some("cloud-test".into()),
            base_url,
            email: "me@example.com".into(),
            token: Some("cloud-token".into()),
            project: None,
            timeout_secs: 30,
            deployment: JiraDeployment::Cloud,
            auth_type: JiraAuthType::CloudApiToken,
            api_version: 3,
        })
    }

    async fn mock_created_issue(server: &MockServer, api_version: u8, key: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/rest/api/{api_version}/issue/{key}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "1",
                "key": key,
                "fields": {"summary": "s", "status": {"name": "Open"}}
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn dc_assignee_me_resolves_to_name_object() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/rest/api/2/myself"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "U_M25ZM",
                "key": "JIRAUSER155790",
                "displayName": "Черба Денис Сергеевич"
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue"))
            .and(body_json(json!({
                "fields": {
                    "project": {"key": "PAYDAY"},
                    "summary": "sub",
                    "issuetype": {"name": "Development"},
                    "assignee": {"name": "U_M25ZM"},
                    "parent": {"key": "PAYDAY-1831"}
                },
                "update": {"customfield_59170": [{"set": "625"}]}
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key": "PAYDAY-1855"})))
            .expect(1)
            .mount(&server)
            .await;

        mock_created_issue(&server, 2, "PAYDAY-1855").await;

        let mut update_ops = serde_json::Map::new();
        update_ops.insert("customfield_59170".into(), json!([{"set": "625"}]));

        let client = dc_test_client(server.uri());
        let issue = client
            .create_issue_v2(CreateIssueRequestV2 {
                project_key: "PAYDAY".into(),
                summary: "sub".into(),
                issue_type: "Development".into(),
                assignee: Some("me".into()),
                parent: Some("PAYDAY-1831".into()),
                update_ops,
                ..Default::default()
            })
            .await
            .expect("DC create with assignee=me should succeed");

        assert_eq!(issue.key, "PAYDAY-1855");
    }

    #[tokio::test]
    async fn cloud_assignee_me_keeps_account_id_object() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/rest/api/3/myself"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"accountId": "5b10a2844c"})),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .and(body_json(json!({
                "fields": {
                    "project": {"key": "PROJ"},
                    "summary": "task",
                    "issuetype": {"name": "Task"},
                    "assignee": {"accountId": "5b10a2844c"}
                }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key": "PROJ-1"})))
            .expect(1)
            .mount(&server)
            .await;

        mock_created_issue(&server, 3, "PROJ-1").await;

        let client = cloud_test_client(server.uri());
        let issue = client
            .create_issue_v2(CreateIssueRequestV2 {
                project_key: "PROJ".into(),
                summary: "task".into(),
                issue_type: "Task".into(),
                assignee: Some("me".into()),
                ..Default::default()
            })
            .await
            .expect("Cloud create with assignee=me should succeed");

        assert_eq!(issue.key, "PROJ-1");
    }

    #[tokio::test]
    async fn dc_assignee_email_searches_by_username_param() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/rest/api/2/user/search"))
            .and(query_param("username", "MSTsareva@alfabank.ru"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "name": "U_M26RC",
                "key": "JIRAUSER158012",
                "emailAddress": "MSTsareva@alfabank.ru"
            }])))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue"))
            .and(body_json(json!({
                "fields": {
                    "project": {"key": "PAYDAY"},
                    "summary": "sub",
                    "issuetype": {"name": "Development"},
                    "assignee": {"name": "U_M26RC"}
                }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key": "PAYDAY-1900"})))
            .expect(1)
            .mount(&server)
            .await;

        mock_created_issue(&server, 2, "PAYDAY-1900").await;

        let client = dc_test_client(server.uri());
        client
            .create_issue_v2(CreateIssueRequestV2 {
                project_key: "PAYDAY".into(),
                summary: "sub".into(),
                issue_type: "Development".into(),
                assignee: Some("MSTsareva@alfabank.ru".into()),
                ..Default::default()
            })
            .await
            .expect("DC create with email assignee should succeed");
    }
}
