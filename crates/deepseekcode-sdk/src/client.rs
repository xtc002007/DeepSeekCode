use deepseekcode_schema::event::EventEnvelope;
use deepseekcode_schema::permission::PermissionStatus;
use deepseekcode_schema::provider::ProviderInfo;
use deepseekcode_schema::session::{CreateSessionRequest, PromptRequest, Session};
use deepseekcode_types::{MessageId, Result, SessionId};
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::pin::Pin;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum EventStreamItem {
    Event(EventEnvelope),
    Gap { head: i64, limit: i64 },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct FsTreePage {
    pub entries: Vec<FsEntry>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b'\\' | b':' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Clone)]
pub struct DeepSeekCodeClient {
    http: Client,
    base_url: String,
    directory: String,
    auth: Option<(String, String)>,
}

impl DeepSeekCodeClient {
    pub fn new(base_url: impl Into<String>, directory: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            directory: directory.into(),
            auth: None,
        }
    }

    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.auth = Some((username.into(), password.into()));
        self
    }

    pub async fn health(&self) -> Result<()> {
        let _: serde_json::Value = self.get("/api/v1/health").await?;
        Ok(())
    }

    pub async fn create_session(&self, req: CreateSessionRequest) -> Result<Session> {
        self.post("/api/v1/session", &req).await
    }

    pub async fn prompt(&self, req: PromptRequest) -> Result<MessageId> {
        let path = format!("/api/v1/session/{}/prompt", req.session_id);
        let resp: serde_json::Value = self.post(&path, &req).await?;
        let id = resp["message_id"].as_str().ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::Other("missing message_id".into())
        })?;
        MessageId::from_str(id)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))
    }

    pub async fn interrupt(&self, session_id: SessionId) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/interrupt");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.get("/api/v1/session").await
    }

    pub async fn get_session(&self, session_id: SessionId) -> Result<Session> {
        let path = format!("/api/v1/session/{session_id}");
        self.get(&path).await
    }

    pub async fn list_active_sessions(&self) -> Result<Vec<SessionId>> {
        let ids: Vec<String> = self.get("/api/v1/session/active").await?;
        ids.into_iter()
            .map(|s| {
                SessionId::from_str(&s)
                    .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))
            })
            .collect()
    }

    pub async fn list_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        let path = format!("/api/v1/session/{session_id}/messages");
        self.get(&path).await
    }

    pub async fn list_timeline(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let path = format!("/api/v1/session/{session_id}/timeline?since_seq={since_seq}");
        self.get(&path).await
    }

    pub async fn list_activity(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<deepseekcode_schema::activity::ActivityTree> {
        let path = format!("/api/v1/session/{session_id}/activity?since_seq={since_seq}");
        self.get(&path).await
    }

    pub async fn session_todo(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::todo::TodoItem>> {
        let path = format!("/api/v1/session/{session_id}/todo");
        self.get(&path).await
    }

    pub async fn session_todo_update(
        &self,
        session_id: SessionId,
        todos: &[deepseekcode_schema::todo::TodoItem],
    ) -> Result<Vec<deepseekcode_schema::todo::TodoItem>> {
        let path = format!("/api/v1/session/{session_id}/todo");
        self.post(&path, &serde_json::json!({ "todos": todos }))
            .await
    }

    pub async fn list_session_events(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<Vec<EventEnvelope>> {
        let path = format!("/api/v1/session/{session_id}/events?since_seq={since_seq}");
        self.get(&path).await
    }

    pub async fn submit_question_answer(&self, session_id: SessionId, answer: &str) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/question/answer");
        let _: serde_json::Value = self
            .post(&path, &serde_json::json!({ "answer": answer }))
            .await?;
        Ok(())
    }

    pub async fn list_session_plans(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::plan::SessionPlanSummary>> {
        let path = format!("/api/v1/session/{session_id}/plans");
        self.get(&path).await
    }

    pub async fn dismiss_plan_implement_prompt(
        &self,
        session_id: SessionId,
        plan_id: &str,
    ) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/plans/{plan_id}/dismiss");
        let _: serde_json::Value = self.post(&path, &serde_json::json!({})).await?;
        Ok(())
    }

    pub async fn latest_event_seq(&self, session_id: SessionId) -> Result<i64> {
        let path = format!("/api/v1/session/{session_id}/events/head");
        let value: serde_json::Value = self.get(&path).await?;
        Ok(value["seq"].as_i64().unwrap_or(0))
    }

    pub async fn fork_session(
        &self,
        session_id: SessionId,
        until_message_id: Option<MessageId>,
    ) -> Result<Session> {
        let path = format!("/api/v1/session/{session_id}/fork");
        self.post(
            &path,
            &serde_json::json!({ "until_message_id": until_message_id }),
        )
        .await
    }

    pub async fn export_session(&self, session_id: SessionId) -> Result<serde_json::Value> {
        self.export_session_with_sanitize(session_id, true).await
    }

    pub async fn export_session_with_sanitize(
        &self,
        session_id: SessionId,
        sanitize: bool,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/session/{session_id}/export?sanitize={}",
            if sanitize { "true" } else { "false" }
        );
        self.get(&path).await
    }

    pub async fn revert_get_staged(
        &self,
        session_id: SessionId,
    ) -> Result<Option<deepseekcode_schema::session::RevertState>> {
        let path = format!("/api/v1/session/{session_id}/revert");
        let resp: serde_json::Value = self.get(&path).await?;
        if resp.is_null() {
            return Ok(None);
        }
        serde_json::from_value(resp)
            .map(Some)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))
    }

    pub async fn revert_stage(
        &self,
        session_id: SessionId,
        message_id: MessageId,
    ) -> Result<deepseekcode_schema::session::RevertState> {
        let path = format!("/api/v1/session/{session_id}/revert/stage");
        let body = deepseekcode_schema::session::RevertStageRequest { message_id };
        self.post(&path, &body).await
    }

    pub async fn revert_clear(&self, session_id: SessionId) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/revert/clear");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn revert_commit(
        &self,
        session_id: SessionId,
    ) -> Result<deepseekcode_schema::session::RevertState> {
        let path = format!("/api/v1/session/{session_id}/revert/commit");
        self.post(&path, &serde_json::json!({})).await
    }

    pub async fn list_projects(&self) -> Result<Vec<deepseekcode_schema::project::Project>> {
        self.get("/api/v1/projects").await
    }

    pub async fn create_project(
        &self,
        path: &str,
    ) -> Result<deepseekcode_schema::project::Project> {
        let body = deepseekcode_schema::project::CreateProjectRequest {
            path: path.to_string(),
        };
        self.post("/api/v1/projects", &body).await
    }

    pub async fn list_project_directories(&self) -> Result<Vec<serde_json::Value>> {
        self.get("/api/v1/project/directories").await
    }

    pub async fn add_project_directory(
        &self,
        directory_path: &str,
        strategy: Option<&str>,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "directory_path": directory_path,
            "strategy": strategy,
        });
        self.post("/api/v1/project/directories", &body).await
    }

    pub async fn get_location(&self) -> Result<serde_json::Value> {
        self.get("/api/v1/location").await
    }

    pub async fn list_worktrees(&self) -> Result<Vec<serde_json::Value>> {
        self.get("/api/v1/worktree").await
    }

    pub async fn create_worktree(&self, branch: Option<&str>) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "branch": branch });
        self.post("/api/v1/worktree", &body).await
    }

    pub async fn remove_worktree(&self, path: &str) -> Result<()> {
        let query = format!("?path={}", urlencoding_encode(path));
        let mut req = self
            .http
            .delete(format!("{}/api/v1/worktree{query}", self.base_url,));
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(text));
        }
        Ok(())
    }

    pub async fn prune_worktrees(&self) -> Result<()> {
        let _: serde_json::Value = self.post_empty("/api/v1/worktree/prune").await?;
        Ok(())
    }

    pub async fn move_session(&self, session_id: SessionId, directory: &str) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/move");
        let body = serde_json::json!({ "directory": directory });
        let _: serde_json::Value = self.post(&path, &body).await?;
        Ok(())
    }

    pub async fn list_providers(&self) -> Result<Vec<ProviderInfo>> {
        self.get("/api/v1/provider").await
    }

    pub async fn connect_provider(
        &self,
        provider_id: &str,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Result<()> {
        let path = format!("/api/v1/provider/{provider_id}/connect");
        let body = deepseekcode_schema::provider::ConnectProviderRequest { api_key, base_url };
        let _: serde_json::Value = self.post(&path, &body).await?;
        Ok(())
    }

    pub async fn disconnect_provider(&self, provider_id: &str) -> Result<()> {
        let path = format!("/api/v1/provider/{provider_id}/disconnect");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn list_pending_policies(
        &self,
    ) -> Result<Vec<deepseekcode_schema::policy::PolicyRequest>> {
        self.get("/api/v1/policy/pending").await
    }

    pub async fn reply_policy(
        &self,
        policy_id: &str,
        reply: deepseekcode_schema::policy::PolicyReply,
    ) -> Result<()> {
        let path = format!("/api/v1/policy/{policy_id}/reply");
        let _: serde_json::Value = self.post(&path, &reply).await?;
        Ok(())
    }

    pub async fn list_policy_grants(
        &self,
    ) -> Result<Vec<deepseekcode_schema::policy::PolicyGrant>> {
        self.get("/api/v1/policy/grants").await
    }

    pub async fn delete_policy_grant(&self, grant_id: &str) -> Result<()> {
        let path = format!("/api/v1/policy/grants/{grant_id}");
        let _: serde_json::Value = self.delete(&path).await?;
        Ok(())
    }

    pub async fn update_policy_config(
        &self,
        policy: &deepseekcode_config::PolicyConfig,
    ) -> Result<()> {
        self.update_policy_settings(policy, None).await
    }

    pub async fn update_policy_settings(
        &self,
        policy: &deepseekcode_config::PolicyConfig,
        policy_mode: Option<deepseekcode_config::PolicyMode>,
    ) -> Result<()> {
        let mut body = serde_json::json!({ "policy": policy });
        if let Some(mode) = policy_mode {
            body["policy_mode"] = serde_json::json!(mode);
        }
        let _: serde_json::Value = self.put("/api/v1/policy/config", &body).await?;
        Ok(())
    }

    pub async fn list_pending_permissions(
        &self,
    ) -> Result<Vec<deepseekcode_schema::permission::PermissionRequest>> {
        self.get("/api/v1/permission/pending").await
    }

    pub async fn respond_permission(
        &self,
        permission_id: &str,
        status: PermissionStatus,
    ) -> Result<()> {
        let path = format!("/api/v1/permission/{permission_id}/respond");
        let body = deepseekcode_schema::permission::PermissionResponse { status };
        let _: serde_json::Value = self.post(&path, &body).await?;
        Ok(())
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn directory(&self) -> &str {
        &self.directory
    }

    pub fn auth_credentials(&self) -> Option<(String, String)> {
        self.auth.clone()
    }

    pub fn ws_url(&self, path: &str) -> String {
        let scheme = if self.base_url.starts_with("https://") {
            "wss://"
        } else {
            "ws://"
        };
        let host = self
            .base_url
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        format!("{scheme}{host}{path}")
    }

    pub async fn update_model(
        &self,
        session_id: SessionId,
        agent: Option<&str>,
        model_id: &str,
        provider_id: &str,
    ) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/model");
        let body = serde_json::json!({
            "agent": agent,
            "model_id": model_id,
            "provider_id": provider_id,
        });
        let mut req = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory)
            .json(&body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    format!("update_model failed: {status}")
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn update_session(&self, session_id: SessionId, title: Option<&str>) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}");
        let body = serde_json::json!({ "title": title });
        let mut req = self
            .http
            .patch(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory)
            .json(&body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    "update_session failed".into()
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn delete_session(&self, session_id: SessionId) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}");
        let mut req = self
            .http
            .delete(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    "delete_session failed".into()
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn create_pty(&self, rows: Option<u16>, cols: Option<u16>) -> Result<String> {
        let body = serde_json::json!({
            "rows": rows,
            "cols": cols,
        });
        let resp: serde_json::Value = self.post("/api/v1/pty", &body).await?;
        resp["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| deepseekcode_types::DeepSeekCodeError::Other("missing pty id".into()))
    }

    pub async fn pty_ticket(&self, pty_id: &str) -> Result<(String, u64)> {
        let path = format!("/api/v1/pty/{pty_id}/ticket");
        let mut req = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory);
        if self.auth.is_some() {
            req = req.header("x-deepseekcode-ticket", "1");
        }
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let resp = self.handle_response::<serde_json::Value>(resp).await?;
        let ticket = resp["ticket"]
            .as_str()
            .ok_or_else(|| {
                deepseekcode_types::DeepSeekCodeError::Other("missing pty ticket".into())
            })?
            .to_string();
        let expires_in = resp["expires_in"].as_u64().unwrap_or(60);
        Ok((ticket, expires_in))
    }

    pub async fn fs_tree(
        &self,
        path: Option<&str>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<Vec<FsEntry>> {
        Ok(self.fs_tree_page(path, offset, limit).await?.entries)
    }

    pub async fn fs_tree_page(
        &self,
        path: Option<&str>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<FsTreePage> {
        let mut parts = Vec::new();
        if let Some(p) = path {
            parts.push(format!("path={}", urlencoding_encode(p)));
        }
        if let Some(offset) = offset {
            parts.push(format!("offset={offset}"));
        }
        if let Some(limit) = limit {
            parts.push(format!("limit={limit}"));
        }
        let query = if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        };
        let resp: serde_json::Value = self.get(&format!("/api/v1/fs/tree{query}")).await?;
        let entries = resp["entries"].as_array().cloned().unwrap_or_default();
        Ok(FsTreePage {
            entries: entries
                .into_iter()
                .filter_map(|e| {
                    Some(FsEntry {
                        name: e["name"].as_str()?.to_string(),
                        path: e["path"].as_str()?.to_string(),
                        is_dir: e["is_dir"].as_bool()?,
                    })
                })
                .collect(),
            total: resp["total"].as_u64().unwrap_or(0) as usize,
            offset: resp["offset"].as_u64().unwrap_or(0) as usize,
            limit: resp["limit"].as_u64().unwrap_or(0) as usize,
        })
    }

    pub async fn fs_write(&self, session_id: SessionId, path: &str, content: &str) -> Result<()> {
        let body = serde_json::json!({
            "path": path,
            "content": content,
        });
        let mut req = self
            .http
            .put(format!("{}{}", self.base_url, "/api/v1/fs/file"))
            .header("x-deepseekcode-directory", &self.directory)
            .header("x-deepseekcode-session-id", session_id.to_string())
            .json(&body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    "fs_write failed".into()
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn fs_file(&self, path: &str) -> Result<String> {
        let query = format!("?path={}", urlencoding_encode(path));
        let resp: serde_json::Value = self.get(&format!("/api/v1/fs/file{query}")).await?;
        resp["content"].as_str().map(String::from).ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::Other("missing file content".into())
        })
    }

    pub async fn fs_diff(
        &self,
        path: &str,
        original: Option<&str>,
        snapshot_id: Option<&str>,
    ) -> Result<String> {
        let mut query = format!("?path={}", urlencoding_encode(path));
        if let Some(orig) = original {
            query.push_str(&format!("&original={}", urlencoding_encode(orig)));
        }
        if let Some(snap) = snapshot_id {
            query.push_str(&format!("&snapshot_id={}", urlencoding_encode(snap)));
        }
        let resp: serde_json::Value = self.get(&format!("/api/v1/fs/diff{query}")).await?;
        Ok(resp["hunks"].as_str().unwrap_or("").to_string())
    }

    pub async fn resize_pty(&self, pty_id: &str, rows: u16, cols: u16) -> Result<()> {
        let path = format!("/api/v1/pty/{pty_id}/resize");
        let body = serde_json::json!({ "rows": rows, "cols": cols });
        let mut req = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory)
            .json(&body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    "resize_pty failed".into()
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn destroy_pty(&self, pty_id: &str) -> Result<()> {
        let path = format!("/api/v1/pty/{pty_id}");
        let mut req = self
            .http
            .delete(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                if text.is_empty() {
                    "destroy_pty failed".into()
                } else {
                    text
                },
            ));
        }
        Ok(())
    }

    pub async fn compact_session(&self, session_id: SessionId) -> Result<()> {
        let path = format!("/api/v1/session/{session_id}/compact");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn list_mcp(&self) -> Result<Vec<serde_json::Value>> {
        self.get("/api/v1/mcp").await
    }

    pub async fn connect_mcp(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/mcp/{name}/connect");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn disconnect_mcp(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/mcp/{name}/disconnect");
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillInfo>> {
        self.get("/api/v1/skills").await
    }

    pub async fn list_plugins(&self) -> Result<Vec<PluginInfo>> {
        self.get("/api/v1/plugins").await
    }

    pub async fn event_stream(
        &self,
        since_seq: i64,
        session_id: Option<SessionId>,
    ) -> Result<Pin<Box<dyn Stream<Item = EventStreamItem> + Send>>> {
        let url = self.event_url(since_seq, session_id);
        let mut req = self.http.get(&url);
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let response = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let stream = response.bytes_stream().eventsource();
        let mapped = stream.filter_map(move |event| async move {
            match event {
                Ok(ev) => {
                    let data = ev.data;
                    if data.is_empty() {
                        return None;
                    }
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                            if matches!(error, "gap_too_large" | "gap") {
                                let head = value
                                    .get("head")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(since_seq);
                                let limit =
                                    value.get("limit").and_then(|v| v.as_i64()).unwrap_or(0);
                                return Some(EventStreamItem::Gap { head, limit });
                            }
                        }
                    }
                    serde_json::from_str::<EventEnvelope>(&data)
                        .ok()
                        .map(EventStreamItem::Event)
                }
                Err(_) => None,
            }
        });
        Ok(Box::pin(mapped))
    }

    pub fn event_url(&self, since_seq: i64, session_id: Option<SessionId>) -> String {
        match session_id {
            Some(id) => format!(
                "{}/api/v1/event?since_seq={since_seq}&session_id={id}",
                self.base_url
            ),
            None => format!("{}/api/v1/event?since_seq={since_seq}", self.base_url),
        }
    }

    pub async fn export_diagnostics(&self, run_id: Option<&str>) -> Result<Vec<u8>> {
        let path = match run_id {
            Some(id) => format!("/api/v1/diagnostics/export?run_id={id}"),
            None => "/api/v1/diagnostics/export".to_string(),
        };
        self.get_bytes(&path).await
    }

    async fn handle_response<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp
                .json()
                .await
                .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()));
        }
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(body);
        Err(match status.as_u16() {
            404 => deepseekcode_types::DeepSeekCodeError::NotFound(message),
            400 => deepseekcode_types::DeepSeekCodeError::InvalidInput(message),
            403 => deepseekcode_types::DeepSeekCodeError::PermissionDenied(message),
            _ => deepseekcode_types::DeepSeekCodeError::Other(format!("HTTP {status}: {message}")),
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let mut req = self.http.get(format!("{}{}", self.base_url, path));
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        self.handle_response(resp).await
    }

    async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let mut req = self.http.get(format!("{}{}", self.base_url, path));
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                "diagnostics export failed: {}",
                resp.status()
            )));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))
    }

    async fn post<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let mut req = self.http.post(format!("{}{}", self.base_url, path));
        req = req
            .header("x-deepseekcode-directory", &self.directory)
            .json(body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        self.handle_response(resp).await
    }

    async fn post_empty(&self, path: &str) -> Result<serde_json::Value> {
        let mut req = self.http.post(format!("{}{}", self.base_url, path));
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let body = resp
            .text()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if body.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&body)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))
    }

    async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let mut req = self.http.delete(format!("{}{}", self.base_url, path));
        req = req.header("x-deepseekcode-directory", &self.directory);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        self.handle_response(resp).await
    }

    async fn put<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let mut req = self
            .http
            .put(format!("{}{}", self.base_url, path))
            .header("x-deepseekcode-directory", &self.directory)
            .json(body);
        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        self.handle_response(resp).await
    }
}
