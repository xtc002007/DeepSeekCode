//! Policy service: evaluate rules, prompt users, persist project grants.

use crate::policy::defaults::agent_default_ruleset;
use crate::policy::evaluate::{evaluate_patterns, merge_rulesets};
use crate::policy::patterns::{
    extract_patterns_for_workspace, remember_patterns_for_workspace, tool_scope,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyWaitErr {
    Cancelled,
    TimedOut,
    Closed,
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
    /// Per-session cancel tokens registered by the runner so `interrupt`
    /// aborts pending `prompt` waits (P0-4). Absent entry = no cancellation.
    cancels: RwLock<HashMap<SessionId, Arc<crate::tools::services::CancelToken>>>,
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
            cancels: RwLock::new(HashMap::new()),
        }
    }

    async fn is_auto_mode(&self) -> bool {
        *self.policy_mode.read().await == PolicyMode::Auto
    }

    /// Register (or replace) the cancel token for a session. Called by the
    /// runner on each drain start.
    pub async fn register_cancel(
        &self,
        session_id: SessionId,
        token: Arc<crate::tools::services::CancelToken>,
    ) {
        self.cancels.write().await.insert(session_id, token);
    }

    /// Cancel all pending prompts for a session (called by `interrupt`).
    /// Removes pending entries and wakes their waiters with rejection.
    pub async fn cancel_pending(&self, session_id: SessionId) {
        let mut pending = self.pending.write().await;
        let ids: Vec<PermissionId> = pending
            .iter()
            .filter(|(_, e)| e.request.session_id == session_id)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(entry) = pending.remove(&id) {
                let _ = entry.tx.send(PolicyOutcome::Rejected {
                    message: Some("interrupted".into()),
                });
            }
        }
    }

    async fn cancel_for(
        &self,
        session_id: SessionId,
    ) -> Option<Arc<crate::tools::services::CancelToken>> {
        self.cancels.read().await.get(&session_id).cloned()
    }

    /// Wait for a policy reply, a cancel, or a timeout.
    /// Returns `Ok(PolicyOutcome)` on reply, `Err(Cancelled)` on interrupt,
    /// `Err(TimedOut)` on timeout, `Err(Closed)` when the sender drops.
    async fn await_policy_reply(
        &self,
        rx: oneshot::Receiver<PolicyOutcome>,
        cancel: Option<Arc<crate::tools::services::CancelToken>>,
    ) -> std::result::Result<PolicyOutcome, PolicyWaitErr> {
        use std::result::Result::{Err, Ok};
        if let Some(token) = cancel {
            tokio::select! {
                biased;
                _ = token.cancelled() => Err(PolicyWaitErr::Cancelled),
                res = rx => match res {
                    Ok(outcome) => Ok(outcome),
                    Err(_) => Err(PolicyWaitErr::Closed),
                },
                _ = tokio::time::sleep(POLICY_TIMEOUT) => Err(PolicyWaitErr::TimedOut),
            }
        } else {
            match tokio::time::timeout(POLICY_TIMEOUT, rx).await {
                Ok(Ok(outcome)) => Ok(outcome),
                Ok(Err(_)) => Err(PolicyWaitErr::Closed),
                Err(_) => Err(PolicyWaitErr::TimedOut),
            }
        }
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
                    // Expand `~` consistently with config file rules so
                    // `~/.ssh=block`-style grants actually match absolutes.
                    pattern: expand_home_for_grant(&g.pattern),
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
        // Hot-reload must reflect DB deletes: re-read project grants and
        // drop stale in-memory session grants/pending (engine.reload_catalog
        // contract). Previously memory survived `DELETE /grants/{id}`.
        self.load_from_db().await?;
        self.session_grants.write().await.clear();
        // Pending prompts reference old rulesets; keep them (user must still
        // reply) but they will re-evaluate on reply path. Do not auto-clear.
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
            let ow_patterns: Vec<String> = outside.to_vec();
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

        // P0-8 symmetric secondary checks: `write` honors explicit `write`
        // rules; `apply_patch` honors its own `apply_patch` scope (no longer
        // mapped to `edit`); `edit` honors explicit `edit` via primary scope
        // already. Any Block wins.
        if matches!(tool_name, "write" | "apply_patch") {
            let secondary_scope = if tool_name == "apply_patch" {
                "apply_patch"
            } else {
                "write"
            };
            let secondary_patterns =
                extract_patterns_for_workspace(tool_name, &input, Some(working_dir));
            if matches!(
                self.evaluate(session_id, agent, secondary_scope, &secondary_patterns)
                    .await,
                PolicyDecision::Block
            ) {
                return Ok(PolicyOutcome::Blocked {
                    reason: format!(
                        "tool '{tool_name}' blocked by policy ({secondary_scope} scope)"
                    ),
                });
            }
        }
        match self.evaluate(session_id, agent, &scope, &patterns).await {
            PolicyDecision::Permit => Ok(PolicyOutcome::Permitted),
            PolicyDecision::Block => Ok(PolicyOutcome::Blocked {
                reason: format!("tool '{tool_name}' blocked by policy"),
            }),
            PolicyDecision::Prompt => {
                // loop_guard is never YOLO-permitted: auto mode must not
                // silently disable the 3x repetition guard.
                if scope == "loop_guard" {
                    return self
                        .prompt(
                            session_id,
                            agent,
                            &scope,
                            tool_name,
                            &patterns,
                            &remember_patterns_for_workspace(tool_name, &input, Some(working_dir)),
                            format!("Repeated tool '{tool_name}' detected 3x. Allow again?"),
                            input,
                        )
                        .await;
                }
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
        // Audit chain must not break in auto mode: emit prompted+resolved
        // with a legal `once` reply and `auto:true` marker (schema has no
        // `auto` reply kind; previously `"auto"` was schema-illegal).
        let policy_id = PermissionId::new();
        let prompted = json!({
            "policy_id": policy_id.to_string(),
            "session_id": session_id.to_string(),
            "scope": scope,
            "tool_name": tool_name,
            "patterns": patterns,
            "remember_patterns": patterns,
            "description": format!("Auto-permitted '{tool_name}' (policy_mode=auto)"),
            "input": input,
            "permission_id": policy_id.to_string(),
            "auto": true,
        });
        events
            .append(
                EventKind::PolicyPrompted,
                Some(session_id),
                prompted.clone(),
            )
            .await?;
        // Legacy compat alias.
        events
            .append(EventKind::PermissionRequested, Some(session_id), prompted)
            .await?;
        let resolved = json!({
            "policy_id": policy_id.to_string(),
            "session_id": session_id.to_string(),
            "scope": scope,
            "tool_name": tool_name,
            "patterns": patterns,
            "reply": "once",
            "auto": true,
            "message": serde_json::Value::Null,
            "permission_id": policy_id.to_string(),
            "status": "permitted",
        });
        events
            .append(
                EventKind::PolicyResolved,
                Some(session_id),
                resolved.clone(),
            )
            .await?;
        events
            .append(EventKind::PermissionResolved, Some(session_id), resolved)
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
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
        // Interrupt-aware: capture the session cancel token so `interrupt`
        // aborts the 300s wait instead of blocking the drain (P0-4).
        let cancel = self.cancel_for(session_id).await;
        self.pending.write().await.insert(
            req.id,
            PendingEntry {
                request: req.clone(),
                tx,
            },
        );

        self.emit_prompted(&req).await?;

        // Capture cancel state for post-timeout classification.
        let cancel_flag = cancel.clone();
        let outcome = match self.await_policy_reply(rx, cancel).await {
            Ok(outcome) => outcome,
            Err(PolicyWaitErr::Closed) => {
                self.finalize_pending_timeout(
                    &req,
                    &format!("policy channel closed for {tool_name}"),
                )
                .await?;
                PolicyOutcome::Rejected {
                    message: Some(format!("policy channel closed for {tool_name}")),
                }
            }
            Err(PolicyWaitErr::Cancelled) => {
                // Clean the pending map without emitting a second resolve if
                // `cancel_pending` already consumed the entry.
                let still_pending = self.pending.write().await.remove(&req.id).is_some();
                if still_pending {
                    self.emit_resolved(
                        &req,
                        &PolicyReply {
                            reply: PolicyReplyKind::Reject,
                            message: Some("interrupted".into()),
                        },
                    )
                    .await?;
                }
                return Err(DeepSeekCodeError::SessionInterrupted);
            }
            Err(PolicyWaitErr::TimedOut) => {
                let interrupted = cancel_flag
                    .as_ref()
                    .map(|t| t.is_cancelled())
                    .unwrap_or(false);
                if interrupted {
                    let still_pending = self.pending.write().await.remove(&req.id).is_some();
                    if still_pending {
                        self.emit_resolved(
                            &req,
                            &PolicyReply {
                                reply: PolicyReplyKind::Reject,
                                message: Some("interrupted".into()),
                            },
                        )
                        .await?;
                    }
                    return Err(DeepSeekCodeError::SessionInterrupted);
                }
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
            .append(
                EventKind::PolicyPrompted,
                Some(req.session_id),
                payload.clone(),
            )
            .await?;
        // Compat alias: legacy subscribers listen for permission.requested.
        events
            .append(
                EventKind::PermissionRequested,
                Some(req.session_id),
                payload,
            )
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
        // `project` means durable SQLite persistence. Without a store the
        // grant would vanish on restart: fail loudly instead of keeping a
        // memory-only illusion.
        let Some(store) = &self.store else {
            return Err(DeepSeekCodeError::Storage(
                "project grant requires a policy store (SQLite); configure storage first".into(),
            ));
        };
        for pattern in &patterns {
            store
                .save_grant(&self.project_dir, &req.scope, pattern)
                .await?;
        }
        // Re-read from DB so UNIQUE/UPSERT semantics (not in-memory push)
        // are the source of truth.
        self.load_from_db().await?;
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
            .append(
                EventKind::PolicyResolved,
                Some(req.session_id),
                payload.clone(),
            )
            .await?;
        events
            .append(EventKind::PermissionResolved, Some(req.session_id), payload)
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
        // `Pending` is a query state, never a decision: reject would turn a
        // status poll into a denial. Return InvalidInput instead.
        if matches!(status, PermissionStatus::Pending) {
            return Err(DeepSeekCodeError::InvalidInput(
                "cannot respond with pending status; use allowed/always/denied".into(),
            ));
        }
        let reply = match status {
            PermissionStatus::Allowed => PolicyReply {
                reply: PolicyReplyKind::Once,
                message: None,
            },
            PermissionStatus::Always => PolicyReply {
                reply: PolicyReplyKind::Project,
                message: None,
            },
            PermissionStatus::Denied => PolicyReply {
                reply: PolicyReplyKind::Reject,
                message: None,
            },
            PermissionStatus::Pending => unreachable!(),
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
    // Explicit `policy` always wins over legacy `permissions`: legacy first,
    // explicit last (last-match-wins). Previously reversed, so legacy
    // silently overwrote explicit rules.
    merge_config_rulesets([
        ruleset_from_permissions(&config.permissions),
        ruleset_from_config(&config.policy),
    ])
}

fn expand_home_for_grant(pattern: &str) -> String {
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    pattern.to_string()
}

fn effective_policy_mode(config: &DeepSeekCodeConfig) -> PolicyMode {
    config.policy_mode.unwrap_or_default()
}

fn policy_to_permission_request(req: PolicyRequest) -> PermissionRequest {
    PermissionRequest {
        id: req.id,
        tool_name: req.tool_name.clone(),
        // Preserve scope/patterns/session so legacy UI can decide.
        description: format!(
            "{} | scope={} patterns=[{}] session={} | {}",
            req.description,
            req.scope,
            req.patterns.join(", "),
            req.session_id,
            req.tool_name,
        ),
        input: {
            let mut v = req.input.clone();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("scope".into(), serde_json::Value::String(req.scope.clone()));
                obj.insert(
                    "patterns".into(),
                    serde_json::Value::Array(
                        req.patterns
                            .iter()
                            .map(|p| serde_json::Value::String(p.clone()))
                            .collect(),
                    ),
                );
                obj.insert(
                    "remember_patterns".into(),
                    serde_json::Value::Array(
                        req.remember_patterns
                            .iter()
                            .map(|p| serde_json::Value::String(p.clone()))
                            .collect(),
                    ),
                );
                obj.insert(
                    "session_id".into(),
                    serde_json::Value::String(req.session_id.to_string()),
                );
            }
            v
        },
        status: PermissionStatus::Pending,
        created_at: req.created_at,
    }
}

/// Backward-compatible alias used across the codebase.
pub type PermissionService = PolicyService;

impl Default for PolicyService {
    fn default() -> Self {
        Self::new(&DeepSeekCodeConfig::default(), ".", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_config::{PolicyMode, PolicyScopeConfig};
    use deepseekcode_schema::agent::AgentKind;
    use deepseekcode_schema::policy::{PolicyAction, PolicyReply, PolicyReplyKind};

    #[tokio::test]
    async fn auto_mode_permits_without_pending() {
        let mut config = DeepSeekCodeConfig {
            policy_mode: Some(PolicyMode::Auto),
            ..Default::default()
        };
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
        let mut config = DeepSeekCodeConfig {
            policy_mode: Some(PolicyMode::Auto),
            ..Default::default()
        };
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
