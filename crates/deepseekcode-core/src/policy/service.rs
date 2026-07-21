//! Policy service: evaluate rules, prompt users, persist project grants.

use crate::policy::defaults::agent_default_ruleset;
use crate::policy::evaluate::{evaluate_patterns, merge_rulesets};
use crate::policy::patterns::{
    extract_patterns, extract_patterns_for_workspace, remember_patterns,
    remember_patterns_for_workspace, tool_scope,
};
use deepseekcode_config::{
    merge_rulesets as merge_config_rulesets, ruleset_from_config, ruleset_from_permissions,
    DeepSeekCodeConfig, PolicyMode,
};
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::permission::{PermissionRequest, PermissionStatus};
use deepseekcode_schema::policy::{
    PolicyDecision, PolicyGrant, PolicyReply, PolicyReplyKind, PolicyRequest, PolicyRequestStatus,
    PolicyRuleset,
};
use deepseekcode_storage::{EventStore, PolicyStore};
use deepseekcode_types::{DeepSeekCodeError, PermissionId, Result, SessionId};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};

const POLICY_TIMEOUT: Duration = Duration::from_secs(300);

pub enum PolicyOutcome {
    Permitted,
    Rejected { message: Option<String> },
    Blocked { reason: String },
}

struct PendingEntry {
    request: PolicyRequest,
    tx: oneshot::Sender<PolicyOutcome>,
}

pub struct PolicyService {
    global_rules: RwLock<PolicyRuleset>,
    agent_rules: RwLock<HashMap<String, PolicyRuleset>>,
    session_grants: RwLock<HashMap<SessionId, PolicyRuleset>>,
    project_grants: RwLock<PolicyRuleset>,
    pending: RwLock<HashMap<PermissionId, PendingEntry>>,
    policy_mode: RwLock<PolicyMode>,
    store: Option<PolicyStore>,
    project_dir: String,
    events: Option<Arc<EventStore>>,
}

impl PolicyService {
    pub fn new(
        config: &DeepSeekCodeConfig,
        project_dir: impl Into<String>,
        store: Option<PolicyStore>,
    ) -> Self {
        let global_rules = build_global_ruleset(config);
        let mut agent_rules = HashMap::new();
        for (name, agent_cfg) in &config.agents {
            if let Some(policy) = &agent_cfg.policy {
                agent_rules.insert(name.clone(), ruleset_from_config(policy));
            }
        }
        Self {
            global_rules: RwLock::new(global_rules),
            agent_rules: RwLock::new(agent_rules),
            session_grants: RwLock::new(HashMap::new()),
            project_grants: RwLock::new(Vec::new()),
            pending: RwLock::new(HashMap::new()),
            policy_mode: RwLock::new(effective_policy_mode(config)),
            store,
            project_dir: project_dir.into(),
            events: None,
        }
    }

    fn is_auto_mode(&self) -> impl std::future::Future<Output = bool> + '_ {
        async move { *self.policy_mode.read().await == PolicyMode::Auto }
    }

    pub fn with_events(mut self, events: Arc<EventStore>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn load_from_db(&self) -> Result<()> {
        if let Some(store) = &self.store {
            let grants = store.load_grants(&self.project_dir).await?;
            *self.project_grants.write().await = grants
                .iter()
                .map(|g| deepseekcode_schema::policy::PolicyRule {
                    scope: g.scope.clone(),
                    pattern: g.pattern.clone(),
                    action: deepseekcode_schema::policy::PolicyAction::Permit,
                })
                .collect();
        }
        Ok(())
    }

    pub async fn reload_policy(&self, config: &DeepSeekCodeConfig) -> Result<()> {
        *self.global_rules.write().await = build_global_ruleset(config);
        *self.policy_mode.write().await = effective_policy_mode(config);
        let mut agent_rules = HashMap::new();
        for (name, agent_cfg) in &config.agents {
            if let Some(policy) = &agent_cfg.policy {
                agent_rules.insert(name.clone(), ruleset_from_config(policy));
            }
        }
        *self.agent_rules.write().await = agent_rules;
        Ok(())
    }

    async fn effective_ruleset(&self, session_id: SessionId, agent: &AgentKind) -> PolicyRuleset {
        let agent_name = format!("{agent:?}").to_lowercase();
        let agent_cfg = self.agent_rules.read().await.get(&agent_name).cloned();
        let session = self
            .session_grants
            .read()
            .await
            .get(&session_id)
            .cloned()
            .unwrap_or_default();
        merge_rulesets([
            agent_default_ruleset(agent),
            self.global_rules.read().await.clone(),
            agent_cfg.unwrap_or_default(),
            session,
            self.project_grants.read().await.clone(),
        ])
    }

    pub async fn evaluate(
        &self,
        session_id: SessionId,
        agent: &AgentKind,
        scope: &str,
        patterns: &[String],
    ) -> PolicyDecision {
        let ruleset = self.effective_ruleset(session_id, agent).await;
        evaluate_patterns(scope, patterns, &ruleset)
    }

    pub async fn ensure_permitted(
        &self,
        session_id: SessionId,
        agent: &AgentKind,
        tool_name: &str,
        input: serde_json::Value,
        working_dir: &str,
    ) -> Result<PolicyOutcome> {
        let scope = tool_scope(tool_name).to_string();
        let patterns = extract_patterns_for_workspace(tool_name, &input, Some(working_dir));

        // outside_workspace guard — path tools should fail back to the model, not prompt the user.
        let outside =
            crate::policy::patterns::outside_workspace_paths(working_dir, &input, tool_name);
        if !outside.is_empty() {
            if is_path_scoped_tool(tool_name) {
                return Ok(PolicyOutcome::Blocked {
                    reason: format!(
                        "path outside project workspace (root: {working_dir}): {}. \
                         Use a relative path under the project root.",
                        outside.join(", ")
                    ),
                });
            }
            let ow_patterns: Vec<String> = outside.iter().cloned().collect();
            match self
                .evaluate(session_id, agent, "outside_workspace", &ow_patterns)
                .await
            {
                PolicyDecision::Block => {
                    return Ok(PolicyOutcome::Blocked {
                        reason: format!("access outside workspace blocked: {}", outside.join(", ")),
                    });
                }
                PolicyDecision::Prompt => {
                    if self.is_auto_mode().await {
                        self.emit_auto_permitted(
                            session_id,
                            "outside_workspace",
                            tool_name,
                            &ow_patterns,
                            input.clone(),
                        )
                        .await?;
                        return Ok(PolicyOutcome::Permitted);
                    }
                    let outcome = self
                        .prompt(
                            session_id,
                            agent,
                            "outside_workspace",
                            tool_name,
                            &ow_patterns,
                            &remember_patterns_for_workspace(tool_name, &input, Some(working_dir)),
                            format!("Allow access outside workspace for '{tool_name}'?"),
                            input.clone(),
                        )
                        .await?;
                    return Ok(outcome);
                }
                PolicyDecision::Permit => {}
            }
        }

        match self.evaluate(session_id, agent, &scope, &patterns).await {
            PolicyDecision::Permit => Ok(PolicyOutcome::Permitted),
            PolicyDecision::Block => Ok(PolicyOutcome::Blocked {
                reason: format!("tool '{tool_name}' blocked by policy"),
            }),
            PolicyDecision::Prompt => {
                if self.is_auto_mode().await {
                    self.emit_auto_permitted(session_id, &scope, tool_name, &patterns, input)
                        .await?;
                    Ok(PolicyOutcome::Permitted)
                } else {
                    self.prompt(
                        session_id,
                        agent,
                        &scope,
                        tool_name,
                        &patterns,
                        &remember_patterns_for_workspace(tool_name, &input, Some(working_dir)),
                        format!("Allow '{tool_name}' to run?"),
                        input,
                    )
                    .await
                }
            }
        }
    }

    async fn emit_auto_permitted(
        &self,
        session_id: SessionId,
        scope: &str,
        tool_name: &str,
        patterns: &[String],
        input: serde_json::Value,
    ) -> Result<()> {
        tracing::info!(
            target: deepseekcode_telemetry::AUDIT,
            session_id = %session_id,
            tool = tool_name,
            scope,
            "policy auto-permitted (YOLO mode)"
        );
        let Some(events) = &self.events else {
            return Ok(());
        };
        let payload = json!({
            "policy_id": PermissionId::new().to_string(),
            "session_id": session_id.to_string(),
            "scope": scope,
            "tool_name": tool_name,
            "patterns": patterns,
            "reply": "auto",
            "input": input,
        });
        events
            .append(EventKind::PolicyResolved, Some(session_id), payload)
            .await?;
        Ok(())
    }

    async fn prompt(
        &self,
        session_id: SessionId,
        agent: &AgentKind,
        scope: &str,
        tool_name: &str,
        patterns: &[String],
        remember: &[String],
        description: String,
        input: serde_json::Value,
    ) -> Result<PolicyOutcome> {
        let _ = agent;
        tracing::info!(
            target: deepseekcode_telemetry::AUDIT,
            session_id = %session_id,
            tool = tool_name,
            scope,
            "policy prompt requested"
        );

        let req = PolicyRequest {
            id: PermissionId::new(),
            session_id,
            scope: scope.to_string(),
            tool_name: tool_name.to_string(),
            patterns: patterns.to_vec(),
            remember_patterns: remember.to_vec(),
            description,
            input: input.clone(),
            status: PolicyRequestStatus::Pending,
            created_at: chrono::Utc::now(),
        };

        let (tx, rx) = oneshot::channel();
        self.pending.write().await.insert(
            req.id,
            PendingEntry {
                request: req.clone(),
                tx,
            },
        );

        self.emit_prompted(&req).await?;

        let outcome = match tokio::time::timeout(POLICY_TIMEOUT, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                self.finalize_pending_timeout(
                    &req,
                    &format!("policy channel closed for {tool_name}"),
                )
                .await?;
                PolicyOutcome::Rejected {
                    message: Some(format!("policy channel closed for {tool_name}")),
                }
            }
            Err(_) => {
                self.finalize_pending_timeout(&req, &format!("timeout waiting for {tool_name}"))
                    .await?;
                PolicyOutcome::Rejected {
                    message: Some(format!("timeout waiting for {tool_name}")),
                }
            }
        };

        Ok(outcome)
    }

    /// Remove a timed-out or abandoned pending entry and emit PolicyResolved(rejected).
    async fn finalize_pending_timeout(&self, req: &PolicyRequest, message: &str) -> Result<()> {
        self.pending.write().await.remove(&req.id);
        self.emit_resolved(
            req,
            &PolicyReply {
                reply: PolicyReplyKind::Reject,
                message: Some(message.to_string()),
            },
        )
        .await
    }

    async fn emit_prompted(&self, req: &PolicyRequest) -> Result<()> {
        let Some(events) = &self.events else {
            return Ok(());
        };
        let payload = json!({
            "policy_id": req.id.to_string(),
            "session_id": req.session_id.to_string(),
            "scope": req.scope,
            "tool_name": req.tool_name,
            "patterns": req.patterns,
            "remember_patterns": req.remember_patterns,
            "description": req.description,
            "input": req.input,
            // legacy alias fields
            "permission_id": req.id.to_string(),
        });
        events
            .append(EventKind::PolicyPrompted, Some(req.session_id), payload)
            .await?;
        Ok(())
    }

    pub async fn list_pending(&self) -> Vec<PolicyRequest> {
        self.pending
            .read()
            .await
            .values()
            .map(|e| e.request.clone())
            .collect()
    }

    pub async fn list_grants(&self) -> Result<Vec<PolicyGrant>> {
        if let Some(store) = &self.store {
            store.load_grants(&self.project_dir).await
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn delete_grant(&self, id: &str) -> Result<()> {
        if let Some(store) = &self.store {
            store.delete_grant(id).await?;
            self.load_from_db().await?;
        }
        Ok(())
    }

    pub async fn reply(&self, id: PermissionId, reply: PolicyReply) -> Result<()> {
        let entry = self.pending.write().await.remove(&id);
        let Some(entry) = entry else {
            return Err(DeepSeekCodeError::NotFound(format!(
                "policy request {id} not found"
            )));
        };
        let req = entry.request;

        self.emit_resolved(&req, &reply).await?;

        let outcome = match reply.reply {
            PolicyReplyKind::Once => {
                self.save_session_grant(&req).await?;
                PolicyOutcome::Permitted
            }
            PolicyReplyKind::Project => {
                self.save_project_grants(&req).await?;
                PolicyOutcome::Permitted
            }
            PolicyReplyKind::Reject => PolicyOutcome::Rejected {
                message: reply.message,
            },
        };

        let _ = entry.tx.send(outcome);
        Ok(())
    }

    async fn save_session_grant(&self, req: &PolicyRequest) -> Result<()> {
        let patterns = if req.remember_patterns.is_empty() {
            req.patterns.clone()
        } else {
            req.remember_patterns.clone()
        };
        let mut grants = self.session_grants.write().await;
        let session_rules = grants.entry(req.session_id).or_default();
        for pattern in patterns {
            session_rules.push(deepseekcode_schema::policy::PolicyRule {
                scope: req.scope.clone(),
                pattern,
                action: deepseekcode_schema::policy::PolicyAction::Permit,
            });
        }
        Ok(())
    }

    async fn save_project_grants(&self, req: &PolicyRequest) -> Result<()> {
        let patterns = if req.remember_patterns.is_empty() {
            req.patterns.clone()
        } else {
            req.remember_patterns.clone()
        };
        if let Some(store) = &self.store {
            for pattern in &patterns {
                store
                    .save_grant(&self.project_dir, &req.scope, pattern)
                    .await?;
            }
        }
        let mut grants = self.project_grants.write().await;
        for pattern in &patterns {
            grants.push(deepseekcode_schema::policy::PolicyRule {
                scope: req.scope.clone(),
                pattern: pattern.clone(),
                action: deepseekcode_schema::policy::PolicyAction::Permit,
            });
        }
        Ok(())
    }

    async fn emit_resolved(&self, req: &PolicyRequest, reply: &PolicyReply) -> Result<()> {
        let Some(events) = &self.events else {
            return Ok(());
        };
        let status = match reply.reply {
            PolicyReplyKind::Once => "permitted",
            PolicyReplyKind::Project => "project",
            PolicyReplyKind::Reject => "rejected",
        };
        let payload = json!({
            "policy_id": req.id.to_string(),
            "session_id": req.session_id.to_string(),
            "scope": req.scope,
            "tool_name": req.tool_name,
            "reply": format!("{:?}", reply.reply).to_lowercase(),
            "message": reply.message,
            "permission_id": req.id.to_string(),
            "status": status,
        });
        events
            .append(EventKind::PolicyResolved, Some(req.session_id), payload)
            .await?;
        Ok(())
    }

    // --- legacy compatibility ---

    pub async fn ensure_allowed(
        &self,
        session_id: SessionId,
        tool_name: &str,
        agent: &AgentKind,
        input: serde_json::Value,
    ) -> Result<()> {
        match self
            .ensure_permitted(session_id, agent, tool_name, input, &self.project_dir)
            .await?
        {
            PolicyOutcome::Permitted => Ok(()),
            PolicyOutcome::Blocked { reason } => Err(DeepSeekCodeError::PermissionDenied(reason)),
            PolicyOutcome::Rejected { message } => Err(DeepSeekCodeError::PermissionDenied(
                message.unwrap_or_else(|| tool_name.to_string()),
            )),
        }
    }

    pub async fn list_pending_legacy(&self) -> Vec<PermissionRequest> {
        self.list_pending()
            .await
            .into_iter()
            .map(policy_to_permission_request)
            .collect()
    }

    pub async fn respond_legacy(&self, id: PermissionId, status: PermissionStatus) -> Result<()> {
        let reply = match status {
            PermissionStatus::Allowed => PolicyReply {
                reply: PolicyReplyKind::Once,
                message: None,
            },
            PermissionStatus::Always => PolicyReply {
                reply: PolicyReplyKind::Project,
                message: None,
            },
            PermissionStatus::Denied | PermissionStatus::Pending => PolicyReply {
                reply: PolicyReplyKind::Reject,
                message: None,
            },
        };
        self.reply(id, reply).await
    }
}

fn is_path_scoped_tool(tool_name: &str) -> bool {
    matches!(
        tool_scope(tool_name).as_str(),
        "read" | "edit" | "list" | "glob" | "grep"
    )
}

fn build_global_ruleset(config: &DeepSeekCodeConfig) -> PolicyRuleset {
    merge_config_rulesets([
        ruleset_from_config(&config.policy),
        ruleset_from_permissions(&config.permissions),
    ])
}

fn effective_policy_mode(config: &DeepSeekCodeConfig) -> PolicyMode {
    config.policy_mode.unwrap_or_default()
}

fn policy_to_permission_request(req: PolicyRequest) -> PermissionRequest {
    PermissionRequest {
        id: req.id,
        tool_name: req.tool_name,
        description: req.description,
        input: req.input,
        status: PermissionStatus::Pending,
        created_at: req.created_at,
    }
}

/// Backward-compatible alias used across the codebase.
pub type PermissionService = PolicyService;

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_config::{PolicyMode, PolicyScopeConfig};
    use deepseekcode_schema::agent::AgentKind;
    use deepseekcode_schema::policy::{PolicyAction, PolicyReply, PolicyReplyKind};

    #[tokio::test]
    async fn auto_mode_permits_without_pending() {
        let mut config = DeepSeekCodeConfig::default();
        config.policy_mode = Some(PolicyMode::Auto);
        config.policy.insert(
            "bash".into(),
            PolicyScopeConfig::Simple(PolicyAction::Prompt),
        );
        let svc = PolicyService::new(&config, ".", None);
        let session_id = SessionId::new();
        let outcome = svc
            .ensure_permitted(
                session_id,
                &AgentKind::Build,
                "bash",
                serde_json::json!({ "command": "echo hi" }),
                ".",
            )
            .await
            .unwrap();
        assert!(matches!(outcome, PolicyOutcome::Permitted));
        assert!(svc.list_pending().await.is_empty());
    }

    #[tokio::test]
    async fn auto_mode_still_blocks() {
        let mut config = DeepSeekCodeConfig::default();
        config.policy_mode = Some(PolicyMode::Auto);
        config.policy.insert(
            "bash".into(),
            PolicyScopeConfig::Simple(PolicyAction::Block),
        );
        let svc = PolicyService::new(&config, ".", None);
        let session_id = SessionId::new();
        let outcome = svc
            .ensure_permitted(
                session_id,
                &AgentKind::Build,
                "bash",
                serde_json::json!({ "command": "echo hi" }),
                ".",
            )
            .await
            .unwrap();
        assert!(matches!(outcome, PolicyOutcome::Blocked { .. }));
    }

    #[tokio::test]
    async fn once_grant_permits_same_pattern_in_session() {
        use std::sync::Arc;
        let mut config = DeepSeekCodeConfig::default();
        config.policy.insert(
            "bash".into(),
            PolicyScopeConfig::Simple(PolicyAction::Prompt),
        );
        let svc = Arc::new(PolicyService::new(&config, ".", None));
        let session_id = SessionId::new();

        let svc_bg = Arc::clone(&svc);
        let first = tokio::spawn(async move {
            svc_bg
                .ensure_permitted(
                    session_id,
                    &AgentKind::Build,
                    "bash",
                    serde_json::json!({ "command": "echo once" }),
                    ".",
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let pending = svc.list_pending().await;
        assert_eq!(pending.len(), 1);
        svc.reply(
            pending[0].id,
            PolicyReply {
                reply: PolicyReplyKind::Once,
                message: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            first.await.unwrap().unwrap(),
            PolicyOutcome::Permitted
        ));

        let outcome = svc
            .ensure_permitted(
                session_id,
                &AgentKind::Build,
                "bash",
                serde_json::json!({ "command": "echo once" }),
                ".",
            )
            .await
            .unwrap();
        assert!(matches!(outcome, PolicyOutcome::Permitted));
        assert!(svc.list_pending().await.is_empty());
    }

    #[tokio::test]
    async fn reject_does_not_drain_other_pending_requests() {
        use std::sync::Arc;
        let mut config = DeepSeekCodeConfig::default();
        config.policy.insert(
            "bash".into(),
            PolicyScopeConfig::Simple(PolicyAction::Prompt),
        );
        config.policy.insert(
            "read".into(),
            PolicyScopeConfig::Simple(PolicyAction::Prompt),
        );
        let svc = Arc::new(PolicyService::new(&config, ".", None));
        let session_a = SessionId::new();
        let session_b = SessionId::new();

        let svc_a = Arc::clone(&svc);
        let _a = tokio::spawn(async move {
            svc_a
                .ensure_permitted(
                    session_a,
                    &AgentKind::Build,
                    "bash",
                    serde_json::json!({ "command": "echo a" }),
                    ".",
                )
                .await
        });
        let svc_b = Arc::clone(&svc);
        let _b = tokio::spawn(async move {
            svc_b
                .ensure_permitted(
                    session_b,
                    &AgentKind::Build,
                    "read",
                    serde_json::json!({ "path": "README.md" }),
                    ".",
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(svc.list_pending().await.len(), 2);

        let first = svc.list_pending().await[0].id;
        svc.reply(
            first,
            PolicyReply {
                reply: PolicyReplyKind::Reject,
                message: Some("no".into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(svc.list_pending().await.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn prompt_timeout_clears_pending() {
        use std::sync::Arc;
        let mut config = DeepSeekCodeConfig::default();
        config.policy.insert(
            "bash".into(),
            PolicyScopeConfig::Simple(PolicyAction::Prompt),
        );
        let svc = Arc::new(PolicyService::new(&config, ".", None));
        let session_id = SessionId::new();

        let timeout_task = tokio::spawn({
            let svc = Arc::clone(&svc);
            async move {
                svc.ensure_permitted(
                    session_id,
                    &AgentKind::Build,
                    "bash",
                    serde_json::json!({ "command": "echo timeout" }),
                    ".",
                )
                .await
            }
        });
        tokio::time::advance(super::POLICY_TIMEOUT + std::time::Duration::from_secs(1)).await;
        assert!(matches!(
            timeout_task.await.unwrap().unwrap(),
            PolicyOutcome::Rejected { .. }
        ));
        assert!(svc.list_pending().await.is_empty());
    }

    #[tokio::test]
    async fn path_tool_outside_workspace_blocks_without_prompt() {
        let config = DeepSeekCodeConfig::default();
        let svc = PolicyService::new(&config, "/project", None);
        let session_id = SessionId::new();
        let outcome = svc
            .ensure_permitted(
                session_id,
                &AgentKind::Build,
                "list",
                serde_json::json!({ "path": "C:\\outside\\other-project" }),
                "/project",
            )
            .await
            .unwrap();
        assert!(matches!(outcome, PolicyOutcome::Blocked { .. }));
        assert!(svc.list_pending().await.is_empty());
    }
}

impl Default for PolicyService {
    fn default() -> Self {
        Self::new(&DeepSeekCodeConfig::default(), ".", None)
    }
}
