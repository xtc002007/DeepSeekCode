#[path = "activity_click_router.rs"]
pub(crate) mod activity_click_router;

use deepseekcode_schema::activity::ActivityTree;
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::plan::SessionPlanSummary;
use deepseekcode_schema::policy::{PolicyReply, PolicyReplyKind};
use deepseekcode_schema::provider::ProviderInfo;
use deepseekcode_schema::session::{
    CreateSessionRequest, PromptDelivery, PromptRequest, RevertFileEntry,
};
use deepseekcode_schema::todo::TodoItem;
use deepseekcode_sdk::{client::DeepSeekCodeClient, EmbeddedEngine, EventStreamItem};
use deepseekcode_types::{
    MessageId, SessionId, CONFIG_FILE, DIAGNOSTICS_ARCHIVE_PREFIX, PRODUCT_NAME,
    SESSION_EXPORT_PREFIX,
};
use futures::{SinkExt, StreamExt};
use gpui::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::composer::{
    ComposerView, PromptAttachment, PromptInput, PromptInputEvent, PromptSubmit,
    SessionTitleCancel, SessionTitleSave,
};
use crate::context_usage::ContextUsageState;
use crate::editor::EditorState;
use crate::file_tree::FileTreeState;
use crate::home::{HomeEvent, HomeView, RecentProject};
use crate::layout::{LayoutState, ProjectNavItem, RightPanelTab};
use crate::logic::{
    activity_visible_row_index, agent_selector_for_kind, ancestor_group_ids_for_leaf,
    classify_git_porcelain, estimate_tokens, extract_tool_path, find_todo_leaf_id_for_index,
    first_user_message_text, git_unified_diff, git_worktree_file_content,
    implement_prompt_from_plan_path, pick_latest_by_timestamp, revert_files_preview,
    sessions_for_project, toggle_activity_group_expansion, token_usage_from_payload,
    truncate_preview, ACTIVITY_ROW_HEIGHT_PX,
};
use crate::multi_window::{engine_client_for, shared_engine, WindowManager};
use crate::panel_resize::PanelResizeEdge;
use crate::permission_dock::{PermissionDock, PolicyPromptState};
use crate::plan_implement_dock::{PlanImplementDock, PlanImplementPromptState};
use crate::plan_session_strip::PlanSessionStrip;
use crate::preferences::{load_preferences, save_preferences, AppPreferences, ThemeId};
use crate::revert::{RevertAction, RevertDock, RevertDockState};
use crate::right_panel::RightPanelView;
use crate::session_activity::{
    push_session_event, truncate_session_events_on_compaction, SessionActivityStore,
};
use crate::session_header::SessionHeaderView;
use crate::settings::{
    PolicyRuleRow, SettingsCatalog, SettingsSection, SettingsView, POLICY_SCOPES,
};
use crate::sidebar::SidebarView;
use crate::terminal::TerminalTab;
use crate::titlebar::TitlebarView;
use deepseekcode_config::PolicyMode;
use deepseekcode_schema::policy::PolicyAction;

async fn lock_sdk_client(
    slot: &Arc<Mutex<Option<DeepSeekCodeClient>>>,
) -> Option<DeepSeekCodeClient> {
    slot.lock().await.clone()
}

async fn stop_all_project_sessions(client: &DeepSeekCodeClient) {
    if let Ok(active) = client.list_active_sessions().await {
        for session_id in active {
            let _ = client.interrupt(session_id).await;
        }
    }
    if let Ok(sessions) = client.list_sessions().await {
        for session in sessions {
            let _ = client.interrupt(session.id).await;
        }
    }
}

async fn prefetch_session_prompts(
    client: &DeepSeekCodeClient,
    sessions: &[deepseekcode_schema::session::Session],
    prompts: &mut HashMap<SessionId, String>,
) {
    const PREFETCH_LIMIT: usize = 8;
    let mut prefetched = 0usize;
    for session in sessions.iter().rev() {
        if prefetched >= PREFETCH_LIMIT {
            break;
        }
        if session
            .title
            .as_ref()
            .is_some_and(|title| !title.trim().is_empty())
        {
            continue;
        }
        if prompts.contains_key(&session.id) {
            continue;
        }
        prefetched += 1;
        if let Ok(messages) = client.list_messages(session.id).await {
            if let Some(text) = first_user_message_text(&messages) {
                prompts.insert(session.id, text);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct QuestionPromptState {
    pub question: String,
    pub options: Vec<String>,
    pub allow_custom: bool,
    pub custom_mode: bool,
}

#[derive(Clone, Debug)]
pub struct ApiKeyDialogState {
    pub provider_id: String,
    pub provider_name: String,
}

pub struct WorkspaceView {
    project_dir: Option<PathBuf>,
    home_view: Entity<HomeView>,
    engine: Arc<Mutex<Option<EmbeddedEngine>>>,
    client: Arc<Mutex<Option<DeepSeekCodeClient>>>,
    session_id: Option<SessionId>,
    activity: crate::session_activity::SessionActivityStore,
    providers: Vec<ProviderInfo>,
    pending_policies: VecDeque<PolicyPromptState>,
    policy_reject_expanded: bool,
    policy_patterns_expanded: bool,
    policy_rule_rows: Vec<PolicyRuleRow>,
    policy_auto_mode: bool,
    selected_policy_row: Option<usize>,
    pending_question: Option<QuestionPromptState>,
    prompt_input: Entity<PromptInput>,
    api_key_input: Entity<PromptInput>,
    base_url_input: Entity<PromptInput>,
    server_url_input: Entity<PromptInput>,
    server_password_input: Entity<PromptInput>,
    worktree_branch_input: Entity<PromptInput>,
    selected_agent: SharedString,
    selected_model: SharedString,
    selected_provider: Option<String>,
    api_key_dialog: Option<ApiKeyDialogState>,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_catalog: SettingsCatalog,
    window_manager: WindowManager,
    file_tree: FileTreeState,
    pub(crate) editor: EditorState,
    editor_tabs: Vec<String>,
    active_editor_tab: usize,
    editor_read_only: bool,
    file_editor_input: Entity<crate::file_editor_input::FileEditorInput>,
    session_title_input: Entity<PromptInput>,
    session_title_editing: bool,
    revert_staged_files: Vec<RevertFileEntry>,
    composer_error: Option<String>,
    permission_allow_input: Entity<PromptInput>,
    permission_deny_input: Entity<PromptInput>,
    question_input: Entity<PromptInput>,
    panel_resize_edge: Option<PanelResizeEdge>,
    panel_resize_start_x: f32,
    panel_resize_start_width: f32,
    terminal_pty_size: Option<(u16, u16)>,
    terminals: Vec<TerminalTab>,
    active_terminal: usize,
    next_terminal_id: usize,
    terminal_focus: FocusHandle,
    pub(crate) session_busy: bool,
    session_status: SharedString,
    session_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    terminal_scroll: ScrollHandle,
    session_context_menu: Option<SessionId>,
    project_context_menu: Option<String>,
    popover_anchor: Option<Point<Pixels>>,
    session_follow_tail: bool,
    pending_scroll_to_tail: bool,
    pending_scroll_to_activity_leaf: Option<String>,
    settings_scroll: ScrollHandle,
    focus: FocusHandle,
    context_usage: ContextUsageState,
    revert_dock: RevertDockState,
    project_nav: Vec<ProjectNavItem>,
    active_session_ids: HashSet<SessionId>,
    expanded_projects: HashSet<String>,
    /// Per-project visible session count in the sidebar (default page size is 5).
    session_nav_visible: HashMap<String, usize>,
    project_sessions_loaded: HashSet<String>,
    project_sessions_loading: HashSet<String>,
    project_nav_refresh_generation: Arc<AtomicU64>,
    layout: LayoutState,
    session_menu_open: bool,
    session_first_prompts: HashMap<SessionId, String>,
    health_ok: bool,
    composer_dropdown: Option<SharedString>,
    api_key_error: Option<String>,
    prompt_resume: bool,
    selected_message_id: Option<MessageId>,
    export_sanitize: bool,
    prompt_delivery: PromptDelivery,
    theme: ThemeId,
    server_url: Option<String>,
    server_password: Option<String>,
    attachments: Vec<PromptAttachment>,
    event_stream_generation: Arc<AtomicU64>,
    bootstrap_generation: Arc<AtomicU64>,
    hydrate_generation: Arc<AtomicU64>,
    child_projection_generation: Arc<AtomicU64>,
    last_sse_notify_at: Option<std::time::Instant>,
    session_todos: Vec<TodoItem>,
    git_staged: Vec<crate::logic::GitStatusEntry>,
    git_unstaged: Vec<crate::logic::GitStatusEntry>,
    git_untracked: Vec<crate::logic::GitStatusEntry>,
    git_diff_summary: Option<String>,
    git_status_message: Option<String>,
    git_log: Vec<String>,
    selected_detail_key: Option<String>,
    highlighted_todo_leaf_id: Option<String>,
    highlighted_todo_index: Option<usize>,
    composer_todo_expanded: bool,
    bash_output_detail_ref: Option<String>,
    pending_new_session: bool,
    pending_switch_session: Option<SessionId>,
    session_plans: Vec<SessionPlanSummary>,
    plan_picker_expanded: bool,
}

impl WorkspaceView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let prompt_input = cx.new(PromptInput::new);
        cx.subscribe(&prompt_input, |this, _, event, cx| {
            if let PromptInputEvent::PasteImage { bytes, format } = event {
                this.attach_clipboard_image(bytes.clone(), *format, cx);
            }
        })
        .detach();
        let api_key_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("sk-…", cx);
            input.set_masked(true, cx);
            input
        });
        let base_url_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("https://api.example.com/v1 (optional)", cx);
            input
        });
        let server_url_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("http://127.0.0.1:4096 (remote server URL)", cx);
            input
        });
        let server_password_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("Server password (optional)", cx);
            input.set_masked(true, cx);
            input
        });
        let worktree_branch_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("feature/my-branch (optional)", cx);
            input
        });
        let file_editor_input = cx.new(crate::file_editor_input::FileEditorInput::new);
        let session_title_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("Session title", cx);
            input.set_key_context("SessionTitleInput", cx);
            input
        });
        let permission_allow_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("allow rules (one per line)", cx);
            input
        });
        let permission_deny_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("Optional feedback when denying…", cx);
            input
        });
        let question_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_placeholder("Your answer…", cx);
            input
        });
        let home_view = cx.new(HomeView::new);
        cx.subscribe(&home_view, |this, _, event, cx| match event {
            HomeEvent::PickFolder => this.pick_project_folder(cx),
            HomeEvent::OpenProject(path) => {
                this.open_recent_project(PathBuf::from(path.clone()), cx);
            }
        })
        .detach();

        let prefs = load_preferences();
        let server_url_pref = prefs.server_url.clone();
        let server_password_pref = prefs.server_password.clone();
        let mut view = Self {
            project_dir: None,
            home_view,
            engine: shared_engine(),
            client: Arc::new(Mutex::new(None)),
            session_id: None,
            activity: crate::session_activity::SessionActivityStore {
                ignore_stale_run_events: true,
                ..Default::default()
            },
            providers: Vec::new(),
            pending_policies: VecDeque::new(),
            policy_reject_expanded: false,
            policy_patterns_expanded: false,
            policy_rule_rows: Vec::new(),
            policy_auto_mode: false,
            selected_policy_row: None,
            pending_question: None,
            prompt_input,
            api_key_input,
            base_url_input,
            server_url_input,
            server_password_input,
            worktree_branch_input,
            selected_agent: prefs.selected_agent.into(),
            selected_model: prefs.selected_model.into(),
            selected_provider: prefs.selected_provider,
            api_key_dialog: None,
            settings_open: false,
            settings_section: SettingsSection::General,
            settings_catalog: SettingsCatalog::default(),
            window_manager: WindowManager::new(),
            file_tree: FileTreeState::default(),
            editor: EditorState::default(),
            editor_tabs: Vec::new(),
            active_editor_tab: 0,
            editor_read_only: true,
            file_editor_input,
            session_title_input,
            session_title_editing: false,
            revert_staged_files: Vec::new(),
            composer_error: None,
            permission_allow_input,
            permission_deny_input,
            question_input,
            panel_resize_edge: None,
            panel_resize_start_x: 0.,
            panel_resize_start_width: 0.,
            terminal_pty_size: None,
            terminals: Vec::new(),
            active_terminal: 0,
            next_terminal_id: 1,
            terminal_focus: cx.focus_handle(),
            session_busy: false,
            session_status: "Ready".into(),
            session_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            terminal_scroll: ScrollHandle::new(),
            session_context_menu: None,
            project_context_menu: None,
            popover_anchor: None,
            session_follow_tail: true,
            pending_scroll_to_tail: false,
            pending_scroll_to_activity_leaf: None,
            settings_scroll: ScrollHandle::new(),
            focus,
            context_usage: ContextUsageState::default(),
            revert_dock: RevertDockState::default(),
            project_nav: Vec::new(),
            active_session_ids: HashSet::new(),
            expanded_projects: HashSet::new(),
            session_nav_visible: HashMap::new(),
            project_sessions_loaded: HashSet::new(),
            project_sessions_loading: HashSet::new(),
            project_nav_refresh_generation: Arc::new(AtomicU64::new(0)),
            layout: LayoutState::default(),
            session_menu_open: false,
            session_first_prompts: HashMap::new(),
            health_ok: true,
            composer_dropdown: None,
            api_key_error: None,
            prompt_resume: true,
            selected_message_id: None,
            export_sanitize: true,
            prompt_delivery: prefs.prompt_delivery,
            theme: prefs.theme,
            server_url: prefs.server_url,
            server_password: prefs.server_password,
            attachments: Vec::new(),
            event_stream_generation: Arc::new(AtomicU64::new(0)),
            bootstrap_generation: Arc::new(AtomicU64::new(0)),
            hydrate_generation: Arc::new(AtomicU64::new(0)),
            child_projection_generation: Arc::new(AtomicU64::new(0)),
            last_sse_notify_at: None,
            session_todos: Vec::new(),
            git_staged: Vec::new(),
            git_unstaged: Vec::new(),
            git_untracked: Vec::new(),
            git_diff_summary: None,
            git_status_message: None,
            git_log: Vec::new(),
            selected_detail_key: None,
            highlighted_todo_leaf_id: None,
            highlighted_todo_index: None,
            composer_todo_expanded: false,
            bash_output_detail_ref: None,
            pending_new_session: false,
            pending_switch_session: None,
            session_plans: Vec::new(),
            plan_picker_expanded: false,
        };
        if let Some(url) = server_url_pref {
            view.server_url_input.update(cx, |input, cx| {
                input.content = url.into();
                cx.notify();
            });
        }
        if let Some(pass) = server_password_pref {
            view.server_password_input.update(cx, |input, cx| {
                input.content = pass.into();
                cx.notify();
            });
        }
        view.preload_home_projects(cx);
        view
    }

    fn preload_home_projects(&mut self, cx: &mut Context<Self>) {
        self.home_view.update(cx, |home, _| {
            home.refresh_recent();
            home.set_loading(false);
        });
    }

    pub fn pick_project_folder(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            let path = rfd::FileDialog::new().pick_folder();
            if let Some(path) = path {
                let _ = view.update(cx, |this, cx| {
                    this.open_project(path, cx);
                });
            }
        })
        .detach();
    }

    pub fn open_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            path = %path.display(),
            "open_project"
        );
        crate::home::record_recent_project(path.clone());
        self.home_view.update(cx, |home, _| {
            home.add_recent(path.clone());
            home.refresh_recent();
        });
        self.reset_session_workspace_state();
        self.session_nav_visible
            .insert(path.display().to_string(), 5);
        self.expanded_projects
            .insert(project_path_key(&path.display().to_string()));
        self.project_dir = Some(path.clone());
        self.refresh_git_status_async(cx);
        self.prepare_project_context(&path, cx);
        self.editor = EditorState::default();
        self.bootstrap_project(path.clone(), cx);
        cx.notify();
    }

    fn prepare_project_context(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Some(message) = Self::ensure_agents_md(path) {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                path = %path.display(),
                "project context prepared"
            );
            cx.notify();
        }
    }

    fn reset_session_workspace_state(&mut self) {
        let pending_new = self.pending_new_session;
        let pending_switch = self.pending_switch_session;
        self.hydrate_generation.fetch_add(1, Ordering::SeqCst);
        self.event_stream_generation.fetch_add(1, Ordering::SeqCst);
        self.activity.timeline_render_generation =
            self.activity.timeline_render_generation.wrapping_add(1);
        self.session_id = None;
        self.activity.since_seq = 0;
        self.selected_message_id = None;
        self.session_follow_tail = true;
        self.context_usage = ContextUsageState::default();
        self.active_session_ids.clear();
        self.activity.session_events_enabled = false;
        self.activity.ignore_stale_run_events = true;
        self.pending_new_session = false;
        self.pending_switch_session = None;
        self.set_session_busy(false, "Ready");
        self.pending_new_session = pending_new;
        self.pending_switch_session = pending_switch;
    }

    pub(crate) fn toggle_context_usage_detail(&mut self, cx: &mut Context<Self>) {
        self.context_usage.expanded = !self.context_usage.expanded;
        cx.notify();
    }

    pub(crate) fn close_revert_dock(&mut self, cx: &mut Context<Self>) {
        self.revert_dock.open = false;
        cx.notify();
    }

    pub(crate) fn open_revert_dock(&mut self, cx: &mut Context<Self>) {
        self.revert_dock.open = true;
        self.refresh_revert_staged(cx);
        cx.notify();
    }

    fn apply_revert_state(&mut self, state: &deepseekcode_schema::session::RevertState) {
        self.revert_dock.staged_message_id = Some(state.staged_message_id.to_string());
        self.revert_staged_files = state.files.clone();
        let paths: Vec<String> = state.files.iter().map(|f| f.path.clone()).collect();
        self.file_tree.changed_paths = paths.iter().cloned().collect();
        self.revert_dock.diff_preview = revert_files_preview(&paths);
    }

    fn refresh_revert_staged(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let staged = client.revert_get_staged(session_id).await.ok().flatten();
            let _ = view.update(cx, |this, cx| {
                if let Some(state) = staged {
                    this.apply_revert_state(&state);
                } else {
                    this.revert_dock.staged_message_id = None;
                    this.revert_dock.diff_preview.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn handle_revert_action(&mut self, action: RevertAction, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let message_id = match action {
            RevertAction::Stage => {
                let Some(id) = self.selected_message_id else {
                    cx.notify();
                    return;
                };
                id
            }
            _ => MessageId::new(),
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let result = match action {
                RevertAction::Stage => client
                    .revert_stage(session_id, message_id)
                    .await
                    .map(|state| Some(state)),
                RevertAction::Clear => client
                    .revert_clear(session_id)
                    .await
                    .map(|_| None),
                RevertAction::Commit => client
                    .revert_commit(session_id)
                    .await
                    .map(Some),
            };
            match result {
                Ok(Some(state)) => {
                    let _ = view.update(cx, |this, cx| {
                        this.apply_revert_state(&state);
                        this.revert_dock.open = true;
                        if action == RevertAction::Commit {
                        }
                        cx.notify();
                    });
                    if action == RevertAction::Commit {
                        let _ = view.update(cx, |this, cx| {
                            this.hydrate_session_from_api(cx, true);
                        });
                    }
                }
                Ok(None) => {
                    let _ = view.update(cx, |this, cx| {
                        this.revert_dock.staged_message_id = None;
                        this.revert_dock.diff_preview.clear();
                        this.revert_dock.open = true;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!(target: deepseekcode_telemetry::DESKTOP, error = %e, "revert action failed");
                }
            }
        })
        .detach();
    }

    pub(crate) fn fork_current_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let until_message_id = self.selected_message_id;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            if let Ok(session) = client.fork_session(session_id, until_message_id).await {
                let _ = view.update(cx, |this, cx| {
                    this.session_id = Some(session.id);
                    this.activity.since_seq = 0;
                    this.hydrate_session_from_api(cx, true);
                    cx.notify();
                });
                let _ = view.update(cx, |this, cx| {
                    this.refresh_project_nav_from_client(cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn export_current_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let sanitize = self.export_sanitize;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |_view, _cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let export_result = client
                .export_session_with_sanitize(session_id, sanitize)
                .await;
            if let Ok(data) = export_result {
                let default_name = format!("{SESSION_EXPORT_PREFIX}-{session_id}.json");
                let dest = rfd::FileDialog::new()
                    .set_file_name(&default_name)
                    .save_file();
                let Some(path) = dest else {
                    return;
                };
                if let Ok(json) = serde_json::to_string_pretty(&data) {
                    if std::fs::write(&path, json).is_ok() {
                        tracing::info!(
                            target: deepseekcode_telemetry::DESKTOP,
                            path = %path.display(),
                            sanitize = sanitize,
                            "session exported"
                        );
                        rfd::MessageDialog::new()
                            .set_title("Session exported")
                            .set_description(format!("Saved to {}", path.display()))
                            .show();
                    }
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.project_dir.clone() else {
            return;
        };
        self.create_session_for_project(path, cx);
    }

    pub(crate) fn new_session_for_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.project_dir.as_ref() == Some(&path) {
            self.create_session_for_project(path, cx);
            return;
        }
        self.pending_new_session = true;
        self.open_recent_project(path, cx);
    }

    fn create_session_for_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (agent, model_id, provider_id) = self.current_session_model();
        let engine_slot = Arc::clone(&self.engine);
        let client_slot = Arc::clone(&self.client);
        let path_str = path.to_string_lossy().to_string();
        cx.spawn(async move |view, cx| {
            let client = {
                let slot_client = lock_sdk_client(&client_slot).await;
                if slot_client
                    .as_ref()
                    .is_some_and(|c| c.directory() == path_str)
                {
                    slot_client
                } else {
                    let guard = engine_slot.lock().await;
                    guard
                        .as_ref()
                        .map(|embedded| engine_client_for(embedded, path_str.clone()))
                }
            };
            let Some(client) = client else {
                return;
            };
            if let Ok(session) = client
                .create_session(CreateSessionRequest {
                    id: None,
                    directory: path_str.clone(),
                    agent: Some(agent),
                    model_id: Some(model_id),
                    provider_id: Some(provider_id),
                })
                .await
            {
                let _ = view.update(cx, |this, cx| {
                    if this.project_dir.as_ref().map(|p| p.display().to_string()) != Some(path_str)
                    {
                        this.refresh_project_nav_from_client(cx);
                        return;
                    }
                    this.hydrate_generation.fetch_add(1, Ordering::SeqCst);
                    this.session_id = Some(session.id);
                    this.activity.since_seq = 0;
                    this.context_usage = ContextUsageState::default();
                    this.sync_context_window();
                    this.set_session_busy(false, "Ready");
                    this.hydrate_session_from_api(cx, true);
                    this.refresh_project_nav_from_client(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn switch_session_by_id(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if self.session_id == Some(session_id) {
            return;
        }
        self.ensure_session_visible_in_nav(session_id);
        self.hydrate_generation.fetch_add(1, Ordering::SeqCst);
        self.session_id = Some(session_id);
        self.sync_agent_from_nav(session_id);
        self.session_plans.clear();
        self.plan_picker_expanded = false;
        self.activity.session_events.clear();
        self.activity.session_messages.clear();
        self.activity.child_session_projections.clear();
        self.activity.activity_tree = ActivityTree::default();
        self.activity.detail_store = crate::activity_timeline::DetailStore::default();
        self.activity.expanded_activity_groups.clear();
        self.activity.collapsed_activity_groups.clear();
        self.activity.expanded_activity_leaves.clear();
        self.highlighted_todo_leaf_id = None;
        self.highlighted_todo_index = None;
        self.bash_output_detail_ref = None;
        self.session_todos.clear();
        self.composer_todo_expanded = false;
        self.selected_detail_key = None;
        self.activity.since_seq = 0;
        self.selected_message_id = None;
        self.context_usage = ContextUsageState::default();
        self.sync_context_window();
        self.set_session_busy(false, "Loading session…");
        self.schedule_session_loading_repaint(cx);
        self.hydrate_session_from_api(cx, true);
        self.refresh_revert_staged(cx);
        self.refresh_session_plans(cx);
        self.session_menu_open = false;
        cx.notify();
    }

    pub(crate) fn toggle_sidebar_collapsed(&mut self, cx: &mut Context<Self>) {
        self.layout.sidebar_collapsed = !self.layout.sidebar_collapsed;
        cx.notify();
    }

    pub(crate) fn toggle_project_expanded(&mut self, path: String, cx: &mut Context<Self>) {
        let canonical = canonical_project_path(&path);
        let key = project_path_key(&canonical);
        let was_expanded = self
            .expanded_projects
            .iter()
            .any(|existing| project_paths_equal(existing, &canonical));
        self.expanded_projects
            .retain(|existing| !project_paths_equal(existing, &canonical));
        if was_expanded {
            cx.notify();
            return;
        }
        self.expanded_projects.insert(key.clone());
        // Allow refetch when user expands — empty results must not be cached forever.
        self.project_sessions_loaded.remove(&key);
        cx.notify();
        self.ensure_project_sessions_loaded(&key, &canonical, cx);
    }

    fn normalize_expanded_project_keys(&mut self) {
        let normalized = self
            .expanded_projects
            .iter()
            .map(|path| project_path_key(path))
            .collect::<HashSet<_>>();
        self.expanded_projects = normalized;
    }

    fn ensure_project_sessions_loaded(
        &mut self,
        path_key: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let project_path = canonical_project_path(project_path);
        let path_key = project_path_key(&project_path);
        if self.project_sessions_loading.contains(&path_key) {
            return;
        }
        if self.project_sessions_loaded.contains(&path_key) {
            return;
        }
        if self
            .project_nav
            .iter()
            .find(|item| project_paths_equal(&item.path, &project_path))
            .is_some_and(|item| !item.sessions.is_empty())
        {
            self.project_sessions_loaded.insert(path_key);
            return;
        }
        self.project_sessions_loading.insert(path_key.clone());
        let path_key = path_key.clone();
        let project_path = project_path.clone();
        let client_slot = Arc::clone(&self.client);
        let engine_slot = Arc::clone(&self.engine);
        let first_prompts = self.session_first_prompts.clone();
        cx.spawn(async move |view, cx| {
            let engine_addr = {
                let guard = engine_slot.lock().await;
                guard.as_ref().map(|embedded| embedded.addr.clone())
            };
            let Some(engine_addr) = engine_addr else {
                let _ = view.update(cx, |this, cx| {
                    this.project_sessions_loading.remove(&path_key);
                    cx.notify();
                });
                return;
            };
            let base = lock_sdk_client(&client_slot).await;
            let auth = base.as_ref().and_then(|client| client.auth_credentials());
            let mut scoped =
                DeepSeekCodeClient::new(format!("http://{engine_addr}"), project_path.clone());
            if let Some((user, pass)) = auth {
                scoped = scoped.with_auth(user, pass);
            }
            let sessions = scoped.list_sessions().await.unwrap_or_default();
            let active = if let Some(ref client) = base {
                client
                    .list_active_sessions()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            let items = sessions_for_project(&sessions, &active, &first_prompts);
            let _ = view.update(cx, |this, cx| {
                this.project_sessions_loading.remove(&path_key);
                if items.is_empty() {
                    this.project_sessions_loaded.remove(&path_key);
                } else {
                    this.project_sessions_loaded.insert(path_key.clone());
                }
                this.merge_project_sessions(&project_path, items, active);
                cx.notify();
            });
        })
        .detach();
    }

    fn merge_project_sessions(
        &mut self,
        project_path: &str,
        sessions: Vec<crate::layout::SessionNavItem>,
        active: HashSet<SessionId>,
    ) {
        let project_path = canonical_project_path(project_path);
        for session in &sessions {
            if !session.title.is_empty() {
                self.session_first_prompts
                    .insert(session.id, session.title.clone());
            }
        }
        if let Some(item) = self
            .project_nav
            .iter_mut()
            .find(|item| project_paths_equal(&item.path, &project_path))
        {
            item.sessions = sessions;
            item.path = project_path.clone();
        } else {
            self.project_nav.push(ProjectNavItem {
                path: project_path.clone(),
                sessions,
            });
        }
        if !self
            .project_nav
            .iter()
            .find(|item| project_paths_equal(&item.path, &project_path))
            .is_some_and(|item| item.sessions.is_empty())
        {
            self.project_sessions_loaded
                .insert(project_path_key(&project_path));
        }
        self.active_session_ids.extend(active);
        self.project_nav = dedupe_project_nav(self.project_nav.clone());
    }

    fn paths_needing_session_refresh(&self) -> HashSet<String> {
        let mut paths = self.expanded_projects.clone();
        if let Some(dir) = &self.project_dir {
            paths.insert(project_path_key(&dir.display().to_string()));
        }
        paths
    }

    pub(crate) fn activate_project_session(
        &mut self,
        project_path: PathBuf,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        self.expanded_projects
            .insert(project_path_key(&project_path.display().to_string()));
        if self.project_dir.as_ref() == Some(&project_path) {
            self.switch_session_by_id(session_id, cx);
            return;
        }
        self.pending_switch_session = Some(session_id);
        self.open_recent_project(project_path, cx);
    }

    pub(crate) fn set_right_panel_tab(&mut self, tab: RightPanelTab, cx: &mut Context<Self>) {
        self.layout.right_panel_tab = tab;
        match tab {
            RightPanelTab::Terminal => {
                if self.terminals.get(self.active_terminal).is_some_and(|tab| {
                    tab.bash_detail_key.is_none()
                        && !tab.state.connected
                        && !tab.state.pty_connecting
                }) {
                    self.connect_terminal_tab(self.active_terminal, cx);
                }
            }
            RightPanelTab::Git => {
                self.refresh_git_status_async(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn toggle_session_menu_at(&mut self, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        let next = !self.session_menu_open;
        self.session_menu_open = next;
        self.composer_dropdown = None;
        self.project_context_menu = None;
        self.session_context_menu = None;
        self.popover_anchor = if next { Some(anchor) } else { None };
        cx.notify();
    }

    pub(crate) fn toggle_session_menu(&mut self, cx: &mut Context<Self>) {
        self.session_menu_open = !self.session_menu_open;
        cx.notify();
    }

    pub(crate) fn close_session_menu(&mut self, cx: &mut Context<Self>) {
        self.session_menu_open = false;
        if self.project_context_menu.is_none() && self.session_context_menu.is_none() {
            self.popover_anchor = None;
        }
        cx.notify();
    }

    pub(crate) fn open_project_context_menu(
        &mut self,
        path: String,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.project_context_menu = Some(path);
        self.session_context_menu = None;
        self.session_menu_open = false;
        self.composer_dropdown = None;
        self.popover_anchor = Some(anchor);
        cx.notify();
    }

    pub(crate) fn close_project_context_menu(&mut self, cx: &mut Context<Self>) {
        self.project_context_menu = None;
        self.popover_anchor = None;
        cx.notify();
    }

    pub(crate) fn close_popover(&mut self, cx: &mut Context<Self>) {
        let changed = self.session_context_menu.is_some()
            || self.project_context_menu.is_some()
            || self.session_menu_open
            || self.composer_dropdown.is_some()
            || self.popover_anchor.is_some();
        self.session_context_menu = None;
        self.project_context_menu = None;
        self.session_menu_open = false;
        self.composer_dropdown = None;
        self.popover_anchor = None;
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn open_session_context_menu(
        &mut self,
        session_id: SessionId,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.composer_dropdown = None;
        self.session_context_menu = Some(session_id);
        self.popover_anchor = Some(anchor);
        cx.notify();
    }

    pub(crate) fn close_session_context_menu(&mut self, cx: &mut Context<Self>) {
        self.close_popover(cx);
    }

    pub(crate) fn delete_session_by_id(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        self.close_session_context_menu(cx);
        let deleting_active = self.session_id == Some(session_id);
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client.delete_session(session_id).await;
            }
            let _ = view.update(cx, |this, cx| {
                if deleting_active {
                    this.session_id = None;
                    this.activity.since_seq = 0;
                    this.selected_message_id = None;
                    this.session_busy = false;
                    this.session_status = "Ready".into();
                }
                this.session_first_prompts.remove(&session_id);
                cx.notify();
            });
            let _ = view.update(cx, |this, cx| {
                this.refresh_project_nav_from_client(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn current_session_title(&self) -> String {
        let Some(session_id) = self.session_id else {
            return "Session".into();
        };
        for nav in &self.project_nav {
            if let Some(item) = nav.sessions.iter().find(|s| s.id == session_id) {
                return item.title.clone();
            }
        }
        self.session_first_prompts
            .get(&session_id)
            .cloned()
            .unwrap_or_else(|| "新会话".into())
    }

    fn update_session_title_in_nav(&mut self, session_id: SessionId, title: String) {
        for nav in &mut self.project_nav {
            if let Some(item) = nav.sessions.iter_mut().find(|s| s.id == session_id) {
                item.title = title;
                break;
            }
        }
    }

    pub(crate) fn remove_project_from_sidebar(&mut self, path: String, cx: &mut Context<Self>) {
        let key = project_path_key(&path);
        self.expanded_projects.remove(&key);
        self.project_sessions_loaded.remove(&key);
        self.project_sessions_loading.remove(&key);
        self.session_nav_visible.remove(&path);
        self.project_nav
            .retain(|item| project_path_key(&item.path) != key);
        self.home_view
            .update(cx, |home, _| home.remove_recent(&path));
        if self
            .project_dir
            .as_ref()
            .is_some_and(|dir| project_path_key(&dir.display().to_string()) == key)
        {
            self.project_dir = None;
            self.session_id = None;
            self.activity.session_events.clear();
            self.activity.session_messages.clear();
            self.activity.activity_tree = ActivityTree::default();
            self.set_session_busy(false, "Ready");
        }
        cx.notify();
    }

    fn set_project_nav(&mut self, nav: Vec<ProjectNavItem>, active_ids: HashSet<SessionId>) {
        let nav = dedupe_project_nav(nav);
        for item in &nav {
            for session in &item.sessions {
                if !session.title.is_empty() {
                    self.session_first_prompts
                        .insert(session.id, session.title.clone());
                }
            }
        }
        self.project_nav = nav;
        self.active_session_ids = active_ids;
        self.normalize_expanded_project_keys();
        for item in &self.project_nav {
            if !item.sessions.is_empty() {
                self.project_sessions_loaded
                    .insert(project_path_key(&item.path));
            }
        }
    }

    pub(crate) fn refresh_project_nav_from_client(&mut self, cx: &mut Context<Self>) {
        let generation = self
            .project_nav_refresh_generation
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let refresh_generation = Arc::clone(&self.project_nav_refresh_generation);
        let client_slot = Arc::clone(&self.client);
        let engine_slot = Arc::clone(&self.engine);
        let current_dir = self.project_dir.as_ref().map(|p| p.display().to_string());
        let first_prompts = self.session_first_prompts.clone();
        let paths_to_load = self.paths_needing_session_refresh();
        let preserved_sessions: HashMap<String, Vec<crate::layout::SessionNavItem>> = self
            .project_nav
            .iter()
            .fold(HashMap::new(), |mut map, item| {
                let key = project_path_key(&item.path);
                let entry = map.entry(key).or_default();
                for session in &item.sessions {
                    if !entry.iter().any(|s| s.id == session.id) {
                        entry.push(session.clone());
                    }
                }
                map
            });
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            project = current_dir.as_deref().unwrap_or("(none)"),
            load_sessions_for = paths_to_load.len(),
            "refresh_project_nav started"
        );
        let started = std::time::Instant::now();
        cx.spawn(async move |view, cx| {
            let engine_addr = {
                let guard = engine_slot.lock().await;
                guard.as_ref().map(|embedded| embedded.addr.clone())
            };
            let Some(engine_addr) = engine_addr else {
                return;
            };
            let base = lock_sdk_client(&client_slot).await;
            let projects = if let Some(ref client) = base {
                client.list_projects().await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let active = if let Some(ref client) = base {
                client
                    .list_active_sessions()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            let auth = base.as_ref().and_then(|client| client.auth_credentials());
            let prompts = first_prompts;

            let mut nav: Vec<ProjectNavItem> = Vec::new();
            let mut seen_projects = HashSet::new();
            for project in &projects {
                let path = canonical_project_path(&project.path);
                let key = project_path_key(&path);
                if !seen_projects.insert(key) {
                    continue;
                }
                let sessions = preserved_sessions
                    .get(&project_path_key(&path))
                    .cloned()
                    .unwrap_or_default();
                nav.push(ProjectNavItem { path, sessions });
            }
            if nav.is_empty() {
                if let Some(dir) = current_dir.clone() {
                    let path = canonical_project_path(&dir);
                    let sessions = preserved_sessions
                        .get(&project_path_key(&path))
                        .cloned()
                        .unwrap_or_default();
                    nav.push(ProjectNavItem { path, sessions });
                }
            }

            let _ = view.update(cx, |this, cx| {
                if refresh_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                this.set_project_nav(nav.clone(), active.clone());
                cx.notify();
            });

            let mut paths_to_fetch: Vec<String> = nav
                .iter()
                .filter(|item| {
                    paths_to_load.contains(&project_path_key(&item.path))
                        && item.sessions.is_empty()
                })
                .map(|item| item.path.clone())
                .collect();
            if nav.is_empty() {
                if let Some(dir) = current_dir {
                    if paths_to_load.contains(&project_path_key(&dir)) {
                        paths_to_fetch.push(dir);
                    }
                }
            }

            for project_path in paths_to_fetch {
                if refresh_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let project_path = canonical_project_path(&project_path);
                let mut scoped =
                    DeepSeekCodeClient::new(format!("http://{engine_addr}"), project_path.clone());
                if let Some((user, pass)) = auth.clone() {
                    scoped = scoped.with_auth(user, pass);
                }
                let sessions = scoped.list_sessions().await.unwrap_or_default();
                let items = sessions_for_project(&sessions, &active, &prompts);
                let path_key = project_path_key(&project_path);
                let _ = view.update(cx, |this, cx| {
                    if refresh_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if items.is_empty() {
                        this.project_sessions_loaded.remove(&path_key);
                    } else {
                        this.project_sessions_loaded.insert(path_key);
                    }
                    this.merge_project_sessions(&project_path, items, active.clone());
                    cx.notify();
                });
            }

            let active_count = active.len();
            let _ = view.update(cx, |this, cx| {
                if refresh_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                tracing::info!(
                    target: deepseekcode_telemetry::DESKTOP,
                    project_count = this.project_nav.len(),
                    active_sessions = active_count,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "refresh_project_nav finished"
                );
                let server_projects: Vec<RecentProject> = this
                    .project_nav
                    .iter()
                    .map(|item| RecentProject {
                        path: item.path.clone(),
                        last_opened: String::new(),
                    })
                    .collect();
                let home = this.home_view.clone();
                cx.defer(move |cx| {
                    home.update(cx, |home, _| {
                        home.set_server_projects(server_projects);
                    });
                });
            });
        })
        .detach();
    }

    fn start_health_poll(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| loop {
            let ok = {
                let client = lock_sdk_client(&client_slot).await;
                match client {
                    Some(c) => c.health().await.is_ok(),
                    None => false,
                }
            };
            let _ = view.update(cx, |this, cx| {
                this.health_ok = ok;
                cx.notify();
            });
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        })
        .detach();
    }

    fn schedule_session_loading_repaint(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| loop {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let continue_anim = view
                .update(cx, |this, cx| {
                    if this.session_status.as_ref() == "Loading session…" {
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !continue_anim {
                break;
            }
        })
        .detach();
    }

    fn hydrate_session_from_api(&mut self, cx: &mut Context<Self>, restart_listener: bool) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let generation = self
            .hydrate_generation
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let hydrate_generation = Arc::clone(&self.hydrate_generation);
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let project_dir = view
                .update(cx, |this, _| this.project_dir.clone())
                .ok()
                .flatten();

            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    session_id = %session_id,
                    "hydrate skipped: sdk client unavailable"
                );
                let _ = view.update(cx, |this, cx| {
                    this.set_session_busy(false, "Ready");
                    cx.notify();
                });
                return;
            };
            let messages = client.list_messages(session_id).await.unwrap_or_default();
            let session_meta = client.get_session(session_id).await.ok();
            let latest_event_seq = client.latest_event_seq(session_id).await.unwrap_or(0);
            let resume_seq = latest_event_seq;

            let activity = client
                .list_activity(session_id, 0)
                .await
                .unwrap_or_default();
            let session_events = client
                .list_session_events(session_id, 0)
                .await
                .unwrap_or_default();
            let todos = client.session_todo(session_id).await.unwrap_or_default();

            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                session_id = %session_id,
                message_count = messages.len(),
                event_count = session_events.len(),
                latest_event_seq,
                resume_seq,
                "hydrating session activity from api"
            );

            let detail_store = crate::activity_timeline::rebuild_detail_store(&session_events);
            let activity_tree = activity;
            let child_ids = task_child_session_ids(&session_events);
            let expanded_group = last_provider_turn_group_id(&activity_tree);
            let git_status = if let Some(dir) = project_dir {
                tokio::task::spawn_blocking(move || collect_git_status(&dir))
                    .await
                    .ok()
            } else {
                None
            };

            let applied = view.update(cx, |this, cx| {
                if hydrate_generation.load(Ordering::SeqCst) != generation {
                    return false;
                }
                this.activity.hydrate_from_api(
                    session_events,
                    messages,
                    activity_tree,
                    detail_store,
                    resume_seq,
                );
                if let Some(group_id) = expanded_group {
                    this.activity.expanded_activity_groups.insert(group_id);
                }
                this.session_todos = todos;
                if let Some(session) = session_meta.as_ref() {
                    this.selected_agent = agent_selector_for_kind(session.agent).into();
                }
                this.rebuild_session_plans_from_events();
                if let Some(status) = git_status {
                    this.apply_git_status(status);
                }
                this.active_session_ids.remove(&session_id);
                this.sync_context_usage_from_events();
                this.set_session_busy(false, "Ready");
                this.pending_scroll_to_tail = true;
                cx.notify();
                true
            });
            if !applied.unwrap_or(false) {
                return;
            }
            for child_id in child_ids {
                let _ = view.update(cx, |this, cx| {
                    this.refresh_child_session_projection(child_id, cx);
                });
            }
            if restart_listener {
                let _ = view.update(cx, |this, cx| {
                    this.start_event_listener(cx);
                    this.sync_pending_permissions(cx);
                });
            }
        })
        .detach();
    }

    fn sync_context_window(&mut self) {
        let window = self
            .providers
            .iter()
            .flat_map(|p| p.models.iter())
            .find(|m| {
                m.name == self.selected_model.as_ref() || m.id == self.selected_model.as_ref()
            })
            .and_then(|m| m.context_window)
            .unwrap_or(128_000);
        self.context_usage.context_window = window;
    }

    fn estimate_tokens(text: &str) -> u32 {
        estimate_tokens(text)
    }

    fn apply_token_usage(&mut self, input: u32, output: u32, reasoning: u32) {
        if input > 0 {
            self.context_usage.input_tokens = input;
            self.context_usage.total_tokens = input;
        }
        if output > 0 {
            self.context_usage.output_tokens = output;
        }
        if reasoning > 0 {
            self.context_usage.reasoning_tokens = reasoning;
        }
    }

    fn sync_context_usage_from_events(&mut self) {
        if let Some((input, output, reasoning)) =
            crate::logic::latest_context_usage_from_events(&self.activity.session_events)
        {
            self.apply_token_usage(input, output, reasoning);
        }
    }

    pub fn with_project(project_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(cx);
        view.project_dir = Some(project_dir.clone());
        if let Some(message) = Self::ensure_agents_md(&project_dir) {}
        view.bootstrap_project(project_dir, cx);
        view
    }

    fn ensure_agents_md(project_dir: &std::path::Path) -> Option<String> {
        const TEMPLATE: &str = "# Project\n\n\
Describe this project so the AI assistant has context.\n\n\
## Structure\n\n\
- Note important directories and conventions here.\n";

        let agents = project_dir.join("AGENTS.md");
        if agents.exists() {
            return None;
        }
        match std::fs::write(&agents, TEMPLATE) {
            Ok(()) => Some(
                "Created AGENTS.md — edit it to describe your project, then send your prompt."
                    .into(),
            ),
            Err(e) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    path = %agents.display(),
                    error = %e,
                    "failed to create AGENTS.md"
                );
                Some(format!(
                    "Missing AGENTS.md: add AGENTS.md to the project root (or instructions in \
                     {CONFIG_FILE} for {PRODUCT_NAME}) before prompting."
                ))
            }
        }
    }

    async fn bootstrap_embedded_client(
        engine_slot: &Arc<Mutex<Option<EmbeddedEngine>>>,
        client_slot: &Arc<Mutex<Option<DeepSeekCodeClient>>>,
        path_str: &str,
    ) -> Option<DeepSeekCodeClient> {
        {
            let mut guard = engine_slot.lock().await;
            if guard.is_none() {
                match EmbeddedEngine::start(path_str.to_string()).await {
                    Ok(embedded) => {
                        *guard = Some(embedded);
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            error = %e,
                            "failed to start embedded engine"
                        );
                        return None;
                    }
                }
            }
        }
        let guard = engine_slot.lock().await;
        let embedded = guard.as_ref()?;
        let client = engine_client_for(embedded, path_str.to_string());
        *client_slot.lock().await = Some(client.clone());
        Some(client)
    }

    fn bootstrap_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let bootstrap_gen = self.bootstrap_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let bootstrap_generation = Arc::clone(&self.bootstrap_generation);
        let engine_slot = Arc::clone(&self.engine);
        let client_slot = Arc::clone(&self.client);
        let path_str = canonical_project_path(&path);
        let remote_url = self
            .server_url
            .clone()
            .filter(|url| !url.contains("127.0.0.1") && !url.contains("localhost"));
        let remote_password = self.server_password.clone();
        cx.spawn(async move |view, cx| {
            let client = if let Some(url) = remote_url {
                let mut remote = DeepSeekCodeClient::new(url, path_str.clone());
                if let Some(pass) = remote_password {
                    remote = remote.with_auth("deepseekcode", pass);
                }
                if remote.health().await.is_ok() {
                    *client_slot.lock().await = Some(remote.clone());
                    Some(remote)
                } else {
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                        "remote server unreachable, falling back to embedded engine"
                    );
                    Self::bootstrap_embedded_client(&engine_slot, &client_slot, &path_str).await
                }
            } else {
                Self::bootstrap_embedded_client(&engine_slot, &client_slot, &path_str).await
            };
            let Some(client) = client else {
                return;
            };
            let _ = client.create_project(&path_str).await;

            let (pending_new, pending_switch) = view
                .update(cx, |this, _| {
                    (this.pending_new_session, this.pending_switch_session.take())
                })
                .unwrap_or((false, None));

            let providers = client.list_providers().await.unwrap_or_default();
            let session_model = std::sync::Mutex::new(None);
            let _ = view.update(cx, |this, cx| {
                this.providers = providers.clone();
                this.init_defaults_from_providers(&providers);
                this.sync_context_window();
                *session_model.lock().unwrap() = Some(this.current_session_model());
                cx.notify();
            });
            let (agent, model_id, provider_id) = session_model
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| (AgentKind::Build, "gpt-4o-mini".into(), "openai".into()));

            let existing_sessions = if pending_switch.is_some() || pending_new {
                Vec::new()
            } else {
                client.list_sessions().await.unwrap_or_default()
            };
            let active_ids = client
                .list_active_sessions()
                .await
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>();
            if !existing_sessions.is_empty() {
                let items = sessions_for_project(&existing_sessions, &active_ids, &HashMap::new());
                let path_for_nav = path_str.clone();
                let _ = view.update(cx, |this, cx| {
                    this.merge_project_sessions(&path_for_nav, items, active_ids.clone());
                    cx.notify();
                });
            }
            let session_id = if let Some(session_id) = pending_switch {
                Some(session_id)
            } else if pending_new {
                None
            } else if existing_sessions.is_empty() {
                match client
                    .create_session(CreateSessionRequest {
                        id: None,
                        directory: path_str.clone(),
                        agent: Some(agent),
                        model_id: Some(model_id),
                        provider_id: Some(provider_id),
                    })
                    .await
                {
                    Ok(session) => Some(session.id),
                    Err(_) => {
                        let retry = client.list_sessions().await.unwrap_or_default();
                        pick_latest_by_timestamp(retry.into_iter().map(|s| {
                            let ts = s.updated_at.to_rfc3339();
                            (s, ts)
                        }))
                        .map(|s| s.id)
                    }
                }
            } else {
                pick_latest_by_timestamp(existing_sessions.into_iter().map(|s| {
                    let ts = s.updated_at.to_rfc3339();
                    (s, ts)
                }))
                .map(|s| s.id)
            };
            if bootstrap_generation.load(Ordering::SeqCst) != bootstrap_gen {
                return;
            }
            if pending_new {
                let _ = view.update(cx, |this, cx| {
                    this.pending_new_session = false;
                    this.create_session_for_project(PathBuf::from(path_str.clone()), cx);
                });
            } else {
                let _ = view.update(cx, |this, cx| {
                    this.session_id = session_id;
                });
                if session_id.is_some() {
                    let _ = view.update(cx, |this, cx| {
                        this.hydrate_session_from_api(cx, true);
                        this.refresh_revert_staged(cx);
                    });
                } else {
                    let _ = view.update(cx, |this, cx| {
                        this.start_event_listener(cx);
                    });
                }
            }
            let _ = view.update(cx, |this, cx| {
                this.terminals.clear();
                this.active_terminal = 0;
                this.next_terminal_id = 1;
                this.load_file_tree_root(cx);
                this.start_health_poll(cx);
                this.refresh_project_nav_from_client(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_home_projects(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let home_entity = self.home_view.clone();
        let _ = cx.spawn(async move |_view, cx| {
            let client = client_slot.lock().await.clone();
            let Some(client) = client else {
                let _ = home_entity.update(cx, |home, _| home.set_loading(false));
                return;
            };
            if let Ok(projects) = client.list_projects().await {
                let _ = home_entity.update(cx, |home, _| {
                    home.set_server_projects(crate::home::projects_from_api(projects));
                });
            } else {
                let _ = home_entity.update(cx, |home, _| home.set_loading(false));
            }
        });
    }

    fn init_defaults_from_providers(&mut self, providers: &[ProviderInfo]) {
        let selected = self.selected_model.to_string();

        // Keep provider aligned with the selected model when it exists in the catalog.
        if let Some(model) = providers
            .iter()
            .flat_map(|provider| provider.models.iter())
            .find(|model| model.name == selected || model.id == selected)
        {
            self.selected_model = model.name.clone().into();
            self.selected_provider = Some(model.provider_id.clone());
            self.persist_preferences();
            return;
        }

        if let Some(provider) = providers.iter().find(|p| p.connected) {
            if let Some(model) = provider.models.first() {
                self.selected_model = model.name.clone().into();
                self.selected_provider = Some(provider.id.clone());
            } else {
                self.selected_provider = Some(provider.id.clone());
            }
        } else if self.selected_provider.is_none() {
            if let Some(provider) = providers.first() {
                self.selected_provider = Some(provider.id.clone());
                if let Some(model) = provider.models.first() {
                    self.selected_model = model.name.clone().into();
                }
            }
        }
        self.persist_preferences();
    }

    fn normalize_model_selection(&mut self) {
        let (model_id, provider_id) = self.resolve_selected_model();
        for provider in &self.providers {
            for model in &provider.models {
                if model.id == model_id || model.name == model_id {
                    self.selected_model = model.name.clone().into();
                    self.selected_provider = Some(provider_id.clone());
                    return;
                }
            }
        }
        self.selected_provider = Some(provider_id);
    }

    fn resolve_selected_model(&self) -> (String, String) {
        let selected = self.selected_model.to_string();
        for provider in &self.providers {
            for model in &provider.models {
                if model.name == selected || model.id == selected {
                    return (model.id.clone(), model.provider_id.clone());
                }
            }
        }
        let provider_id = self
            .selected_provider
            .clone()
            .or_else(|| {
                self.providers
                    .iter()
                    .find(|provider| provider.connected)
                    .map(|provider| provider.id.clone())
            })
            .unwrap_or_else(|| "openai".into());
        (selected, provider_id)
    }

    fn load_file_tree_root(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir.clone() else {
            return;
        };
        let dir_str = dir.to_string_lossy().to_string();
        self.file_tree.loading.push(dir_str.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let page = if let Some(client) = lock_sdk_client(&client_slot).await {
                client
                    .fs_tree_page(Some(&dir_str), None, Some(500))
                    .await
                    .ok()
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                this.file_tree.loading.retain(|p| p != &dir_str);
                if let Some(page) = page {
                    this.file_tree.totals.insert(dir_str.clone(), page.total);
                    this.file_tree
                        .offsets
                        .insert(dir_str.clone(), page.offset + page.entries.len());
                    this.file_tree.entries.insert(dir_str, page.entries);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.file_tree.is_expanded(path) {
            self.file_tree.expanded.retain(|p| p != path);
        } else {
            self.file_tree.expanded.push(path.to_string());
            if !self.file_tree.entries.contains_key(path) {
                self.load_directory(path, cx);
            }
        }
        cx.notify();
    }

    fn load_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.to_string();
        self.file_tree.loading.push(path.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let page = if let Some(client) = lock_sdk_client(&client_slot).await {
                client.fs_tree_page(Some(&path), None, Some(500)).await.ok()
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                this.file_tree.loading.retain(|p| p != &path);
                if let Some(page) = page {
                    this.file_tree.totals.insert(path.clone(), page.total);
                    this.file_tree
                        .offsets
                        .insert(path.clone(), page.offset + page.entries.len());
                    this.file_tree.entries.insert(path, page.entries);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn load_more_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.to_string();
        let offset = self
            .file_tree
            .offsets
            .get(&path)
            .copied()
            .or_else(|| {
                self.file_tree
                    .entries
                    .get(&path)
                    .map(|entries| entries.len())
            })
            .unwrap_or(0);
        self.file_tree.loading.push(path.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let page = if let Some(client) = lock_sdk_client(&client_slot).await {
                client
                    .fs_tree_page(Some(&path), Some(offset), Some(500))
                    .await
                    .ok()
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                this.file_tree.loading.retain(|p| p != &path);
                if let Some(page) = page {
                    this.file_tree.totals.insert(path.clone(), page.total);
                    this.file_tree
                        .offsets
                        .insert(path.clone(), page.offset + page.entries.len());
                    let existing = this.file_tree.entries.entry(path).or_default();
                    existing.extend(page.entries);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.editor.scroll_to_line = None;
        self.editor.diff_stats = None;
        let path = path.to_string();
        let snapshot_id = self
            .revert_staged_files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.snapshot_id.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let result = if let Some(client) = lock_sdk_client(&client_slot).await {
                let content = client.fs_file(&path).await.ok();
                let diff = match client.fs_diff(&path, None, snapshot_id.as_deref()).await {
                    Ok(hunks) if !hunks.is_empty() => Some(hunks),
                    _ => content
                        .as_ref()
                        .map(|c| crate::editor::compute_local_diff("", c)),
                };
                Some((content, diff))
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                if let Some((content, diff)) = result {
                    if content.is_none() {
                        tracing::warn!(
                            target: deepseekcode_telemetry::DESKTOP,
                            path = %path,
                            "failed to open file"
                        );
                    }
                    let text = content.unwrap_or_default();
                    this.editor.path = Some(path.clone());
                    this.editor.set_content(text.clone());
                    this.editor.diff_hunks = diff;
                    let file_input = this.file_editor_input.clone();
                    let editor_text = text.clone();
                    cx.defer(move |cx| {
                        file_input.update(cx, |input, cx| {
                            input.set_text(editor_text, cx);
                        });
                    });
                    if !this.editor_tabs.iter().any(|p| p == &path) {
                        this.editor_tabs.push(path.clone());
                    }
                    this.active_editor_tab = this
                        .editor_tabs
                        .iter()
                        .position(|p| p == &path)
                        .unwrap_or(0);
                    this.layout.right_panel_tab = RightPanelTab::Editor;
                    this.editor_read_only = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_recent_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = PathBuf::from(canonical_project_path(&path));
        crate::home::record_recent_project(path.clone());
        self.home_view.update(cx, |home, _| {
            home.add_recent(path.clone());
            home.refresh_recent();
        });
        self.reset_session_workspace_state();
        self.session_nav_visible
            .insert(path.display().to_string(), 5);
        self.expanded_projects
            .insert(project_path_key(&path.display().to_string()));
        self.project_dir = Some(path.clone());
        self.refresh_git_status_async(cx);
        self.prepare_project_context(&path, cx);
        self.editor = EditorState::default();
        self.bootstrap_project(path, cx);
    }

    pub(crate) fn show_more_project_sessions(&mut self, path: String, cx: &mut Context<Self>) {
        const PAGE: usize = 5;
        let next = self.session_nav_visible.get(&path).copied().unwrap_or(PAGE) + PAGE;
        self.session_nav_visible.insert(path, next);
        cx.notify();
    }

    fn ensure_session_visible_in_nav(&mut self, session_id: SessionId) {
        let Some(project_path) = self.project_dir.as_ref() else {
            return;
        };
        let path = canonical_project_path(project_path);
        let Some(nav) = self
            .project_nav
            .iter()
            .find(|n| project_paths_equal(&n.path, &path))
        else {
            return;
        };
        let Some(index) = nav.sessions.iter().position(|s| s.id == session_id) else {
            return;
        };
        const PAGE: usize = 5;
        let need = index + 1;
        let current = self.session_nav_visible.get(&path).copied().unwrap_or(PAGE);
        if need > current {
            self.session_nav_visible.insert(path, need);
        }
    }

    pub(crate) fn add_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let title = format!("Shell {}", self.terminals.len() + 1);
        self.terminals.push(TerminalTab::new(id, title));
        self.active_terminal = self.terminals.len().saturating_sub(1);
        self.connect_terminal_tab(self.active_terminal, cx);
        cx.notify();
    }

    pub(crate) fn switch_terminal_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.terminals.len() {
            self.active_terminal = index;
            let needs_connect = self.terminals.get(index).is_some_and(|tab| {
                tab.bash_detail_key.is_none() && !tab.state.connected && !tab.state.pty_connecting
            });
            if needs_connect {
                self.connect_terminal_tab(index, cx);
            }
            cx.notify();
        }
    }

    fn mark_terminal_connect_failed(&mut self, tab_id: usize, message: impl Into<SharedString>) {
        if let Some(tab) = self.terminals.iter_mut().find(|t| t.id == tab_id) {
            tab.state.pty_connecting = false;
            tab.state.connected = false;
            tab.state.input_tx = None;
            tab.state.pty_id = None;
            tab.state.connect_error = Some(message.into());
        }
    }

    fn connect_terminal_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.terminals.get(tab_index) else {
            return;
        };
        if tab.bash_detail_key.is_some() {
            return;
        }
        if tab.state.connected || tab.state.pty_connecting {
            return;
        }
        let buffer = Arc::clone(&tab.buffer);
        let grid = Arc::clone(&tab.grid);
        let tab_id = tab.id;
        if let Some(tab) = self.terminals.get_mut(tab_index) {
            tab.state.input_tx = None;
            tab.state.pty_id = None;
            tab.state.connect_error = None;
        }
        let client_slot = Arc::clone(&self.client);
        if let Some(tab) = self.terminals.get_mut(tab_index) {
            tab.state.pty_connecting = true;
            tab.state.connect_error = None;
        }
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(tab) = self.terminals.get_mut(tab_index) {
            tab.state.input_tx = Some(input_tx.clone());
        }
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    tab_id,
                    reason = "embedded client not ready",
                    "pty connection failed"
                );
                let _ = view.update(cx, |this, cx| {
                    this.mark_terminal_connect_failed(tab_id, "embedded client not ready");
                    cx.notify();
                });
                return;
            };
            let pty_id = match client.create_pty(Some(24), Some(80)).await {
                Ok(id) => id,
                Err(e) => {
                    let reason = format!("create PTY failed: {e}");
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                        tab_id,
                        reason = %reason,
                        "pty connection failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.mark_terminal_connect_failed(tab_id, reason);
                        cx.notify();
                    });
                    return;
                }
            };
            grid.lock().await.reset();
            let mut ws_path = format!("/api/v1/pty/{pty_id}/connect");
            if let Ok((ticket, _)) = client.pty_ticket(&pty_id).await {
                ws_path.push_str(&format!("?ticket={ticket}"));
            }
            let ws_url = client.ws_url(&ws_path);

            let _ = view.update(cx, |this, cx| {
                if let Some(tab) = this.terminals.iter_mut().find(|t| t.id == tab_id) {
                    tab.state.pty_id = Some(pty_id);
                    tab.state.connected = false;
                }
                cx.notify();
            });

            let ws = match connect_async(&ws_url).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    let reason = format!("WebSocket connect failed ({ws_url}): {e}");
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                        tab_id,
                        reason = %reason,
                        "pty connection failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.mark_terminal_connect_failed(tab_id, reason);
                        cx.notify();
                    });
                    return;
                }
            };

            let _ = view.update(cx, |this, cx| {
                if let Some(tab) = this.terminals.iter_mut().find(|t| t.id == tab_id) {
                    tab.state.connected = true;
                    tab.state.pty_connecting = false;
                    tab.state.connect_error = None;
                }
                cx.notify();
            });

            let (mut write, mut read) = ws.split();
            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Binary(data))) => {
                                grid.lock().await.write(&data);
                                let text = grid.lock().await.render_text();
                                *buffer.lock().await = text;
                            }
                            Some(Ok(Message::Text(text))) => {
                                grid.lock().await.write(text.as_bytes());
                                let rendered = grid.lock().await.render_text();
                                *buffer.lock().await = rendered;
                            }
                            _ => break,
                        }
                        let _ = view.update(cx, |this, cx| {
                            this.refresh_terminal_output_for_tab(tab_id);
                            cx.notify();
                        });
                    }
                    input = input_rx.recv() => {
                        match input {
                            Some(data) => {
                                if write.send(Message::Binary(data.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }

            let _ = view.update(cx, |this, cx| {
                if let Some(tab) = this.terminals.iter_mut().find(|t| t.id == tab_id) {
                    tab.state.connected = false;
                    tab.state.pty_connecting = false;
                    tab.state.input_tx = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_terminal_output_for_tab(&mut self, tab_id: usize) {
        let output = self
            .terminals
            .iter()
            .find(|t| t.id == tab_id)
            .and_then(|tab| tab.buffer.try_lock().ok().map(|guard| guard.clone()));
        if let (Some(tab), Some(text)) =
            (self.terminals.iter_mut().find(|t| t.id == tab_id), output)
        {
            tab.state.output = text.into();
        }
    }

    fn refresh_terminal_output(&mut self) {
        let tab_id = self
            .terminals
            .get(self.active_terminal)
            .map(|tab| tab.id)
            .unwrap_or(0);
        self.refresh_terminal_output_for_tab(tab_id);
    }

    pub(crate) fn send_terminal_input(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let idx = self.active_terminal;
        let Some(tab) = self.terminals.get(idx) else {
            return;
        };
        let Some(input_tx) = tab.state.input_tx.clone() else {
            return;
        };
        let buffer = Arc::clone(&tab.buffer);
        let data = keystroke_to_bytes(keystroke);
        if data.is_empty() {
            return;
        }
        let echo = String::from_utf8_lossy(&data).to_string();
        if let Ok(mut guard) = buffer.try_lock() {
            guard.push_str(&echo);
            let text = guard.clone();
            if let Some(active) = self.terminals.get_mut(idx) {
                active.state.output = text.into();
            }
        }
        cx.spawn(async move |_view, _cx| {
            let _ = input_tx.send(data);
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let branch_text = self.worktree_branch_input.read(cx).text();
        let branch = if branch_text.is_empty() {
            None
        } else {
            Some(branch_text)
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let result = client.create_worktree(branch.as_deref()).await;
            let message = match &result {
                Ok(entry) => format!(
                    "Created worktree at {}",
                    entry.get("path").and_then(|v| v.as_str()).unwrap_or("?")
                ),
                Err(e) => format!("Failed to create worktree: {e}"),
            };
            let worktrees = client.list_worktrees().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.worktrees = worktrees;
                this.settings_catalog.worktree_message = message;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_worktree_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.open_recent_project(PathBuf::from(path), cx);
    }

    fn start_event_listener(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let since = self.activity.since_seq;
        let filter_session = self.session_id;
        let generation = self.event_stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let generation_slot = Arc::clone(&self.event_stream_generation);
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            since_seq = since,
            session_id = ?filter_session,
            generation,
            session_events_enabled = self.activity.session_events_enabled,
            ignore_stale_run_events = self.activity.ignore_stale_run_events,
            "starting event listener"
        );
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let mut since_seq = since;
            loop {
                if generation_slot.load(Ordering::SeqCst) != generation {
                    break;
                }
                match client.event_stream(since_seq, filter_session).await {
                    Ok(mut stream) => {
                        while let Some(item) = stream.next().await {
                            if generation_slot.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            match item {
                                EventStreamItem::Gap { head, .. } => {
                                    tracing::warn!(
                                        target: deepseekcode_telemetry::DESKTOP,
                                        since_seq = since_seq,
                                        head = head,
                                        "sse gap detected, backfilling via REST"
                                    );
                                    if let Some(session_id) = filter_session {
                                        if let Ok(events) =
                                            client.list_session_events(session_id, since_seq).await
                                        {
                                            for event in events {
                                                since_seq = event.seq;
                                                let _ = view.update(cx, |this, cx| {
                                                    this.apply_event(event, cx);
                                                    cx.notify();
                                                });
                                            }
                                            let _ = view.update(cx, |this, cx| {
                                                this.rebuild_activity_tree("sse_gap_backfill");
                                                cx.notify();
                                            });
                                        }
                                    }
                                    since_seq = head;
                                    break;
                                }
                                EventStreamItem::Event(event) => {
                                    since_seq = event.seq;
                                    let kind = event.kind;
                                    let _ = view.update(cx, |this, cx| {
                                        this.apply_event(event, cx);
                                        if kind == EventKind::MessageDelta
                                            || kind == EventKind::ToolProgress
                                        {
                                            let now = std::time::Instant::now();
                                            if let Some(last) = this.last_sse_notify_at {
                                                if now.duration_since(last)
                                                    < std::time::Duration::from_millis(16)
                                                {
                                                    return;
                                                }
                                            }
                                            this.last_sse_notify_at = Some(now);
                                        }
                                        cx.notify();
                                    });
                                }
                            }
                        }
                        tracing::warn!(
                            target: deepseekcode_telemetry::DESKTOP,
                            since_seq = since_seq,
                            "event stream disconnected, reconnecting"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            error = %e,
                            "failed to connect event stream, retrying"
                        );
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        })
        .detach();
        self.sync_pending_permissions(cx);
    }

    fn sync_pending_permissions(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let session_id = self.session_id;
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            if let Ok(pending) = client.list_pending_policies().await {
                let _ = view.update(cx, |this, cx| {
                    if this.session_id != session_id {
                        return;
                    }
                    this.pending_policies.clear();
                    let total = pending.len();
                    for (index, req) in pending.into_iter().enumerate() {
                        let detail = format_tool_input(&req.tool_name, &req.input);
                        this.push_policy_prompt(PolicyPromptState {
                            id: req.id.to_string(),
                            scope: req.scope,
                            tool: req.tool_name.clone(),
                            detail,
                            patterns: req.patterns,
                            queue_index: index,
                            queue_total: total,
                        });
                    }
                    if !this.pending_policies.is_empty() {
                        this.set_session_busy(true, "Waiting for policy approval…");
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn push_policy_prompt(&mut self, prompt: PolicyPromptState) {
        if self.pending_policies.iter().any(|p| p.id == prompt.id) {
            return;
        }
        self.pending_policies.push_back(prompt);
        self.policy_reject_expanded = false;
    }

    fn remove_policy_prompt(&mut self, id: &str) {
        self.pending_policies.retain(|p| p.id != id);
        if self.pending_policies.is_empty() {
            self.policy_reject_expanded = false;
            self.policy_patterns_expanded = false;
            if self.pending_question.is_none() {
                self.set_session_busy(false, "Ready");
            }
        }
    }

    pub(crate) fn focus_policy_prompt(&mut self, id: &str) {
        if let Some(pos) = self.pending_policies.iter().position(|p| p.id == id) {
            if pos > 0 {
                let prompt = self.pending_policies.remove(pos).unwrap();
                self.pending_policies.push_front(prompt);
            }
        }
    }

    pub(crate) fn toggle_policy_patterns_expanded(&mut self, cx: &mut Context<Self>) {
        self.policy_patterns_expanded = !self.policy_patterns_expanded;
        cx.notify();
    }

    fn clear_policy_reject_input(&mut self, cx: &mut Context<Self>) {
        self.permission_deny_input.update(cx, |input, cx| {
            input.set_text(String::new(), cx);
        });
    }

    pub(crate) fn composer_blocked(&self) -> bool {
        !self.pending_policies.is_empty() || self.pending_question.is_some()
    }

    fn policy_prompt_from_payload(&self, event: &EventEnvelope) -> PolicyPromptState {
        let tool = event.payload["tool_name"]
            .as_str()
            .unwrap_or("tool")
            .to_string();
        let id = event.payload["policy_id"]
            .as_str()
            .or_else(|| event.payload["permission_id"].as_str())
            .unwrap_or("")
            .to_string();
        let scope = event.payload["scope"].as_str().unwrap_or(&tool).to_string();
        let input = event.payload.get("input").cloned().unwrap_or_default();
        let detail = format_tool_input(&tool, &input);
        let patterns = event.payload["patterns"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        PolicyPromptState {
            id,
            scope,
            tool: tool.clone(),
            detail,
            patterns,
            queue_index: 0,
            queue_total: 1,
        }
    }

    fn handle_policy_prompt_event(&mut self, event: &EventEnvelope) {
        let mut prompt = self.policy_prompt_from_payload(event);
        let total = self.pending_policies.len() + 1;
        prompt.queue_total = total;
        prompt.queue_index = self.pending_policies.len();
        self.push_policy_prompt(prompt);
        let tool = event.payload["tool_name"].as_str().unwrap_or("tool");
        self.set_session_busy(true, format!("Waiting for policy: {tool}"));
    }

    fn set_session_busy(&mut self, busy: bool, status: impl Into<SharedString>) {
        let status: SharedString = status.into();
        if self.session_busy != busy || self.session_status != status {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                busy,
                prev_busy = self.session_busy,
                status = %status,
                prev_status = %self.session_status,
                session_id = ?self.session_id,
                "session_busy changed"
            );
        }
        self.session_busy = busy;
        self.session_status = status;
    }

    fn is_stale_run_event(kind: EventKind) -> bool {
        matches!(
            kind,
            EventKind::PromptAdmitted
                | EventKind::Prompted
                | EventKind::MessageDelta
                | EventKind::MessageCompleted
                | EventKind::ToolStarted
                | EventKind::ToolCompleted
                | EventKind::ProviderTurnStarted
                | EventKind::ContextUpdated
        )
    }

    fn mark_file_changed(&mut self, path: &str) {
        self.file_tree.changed_paths.insert(path.to_string());
    }

    fn maybe_scroll_session_to_tail(&mut self) {
        if self.session_follow_tail {
            self.pending_scroll_to_tail = true;
        }
    }

    fn update_session_follow_tail(&mut self) {
        let max_scroll = self.session_scroll.max_offset().y;
        if max_scroll <= px(0.) {
            // Keep previous follow-tail preference until layout has a real scroll range.
            return;
        }
        let at_bottom = crate::session::is_session_at_bottom(&self.session_scroll);
        if self.session_follow_tail != at_bottom {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                at_bottom,
                offset_y = ?self.session_scroll.offset().y,
                max_scroll = ?max_scroll,
                "session follow-tail updated"
            );
        }
        self.session_follow_tail = at_bottom;
    }

    pub(crate) fn mark_session_scroll_position(&mut self, scroll: &ScrollHandle) {
        self.session_follow_tail = crate::session::is_session_at_bottom(scroll);
    }

    fn apply_event(&mut self, event: EventEnvelope, cx: &mut Context<Self>) {
        if let Some(session_id) = event.session_id {
            if self.session_id != Some(session_id) {
                return;
            }
        }
        if !self.activity.session_events_enabled {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                kind = %event.kind.as_str(),
                seq = event.seq,
                "apply_event deferred (hydrate in progress)"
            );
            self.activity.since_seq = event.seq;
            return;
        }
        if self.activity.ignore_stale_run_events && Self::is_stale_run_event(event.kind) {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                kind = %event.kind.as_str(),
                seq = event.seq,
                busy = self.session_busy,
                "apply_event ignored stale run event"
            );
            self.activity.since_seq = event.seq;
            return;
        }
        if event.kind == EventKind::ActivityUpdated {
            self.activity.since_seq = event.seq;
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                session_id = ?event.session_id,
                seq = event.seq,
                since_seq = event.payload.get("since_seq").and_then(|v| v.as_i64()),
                "activity updated — refreshing child session projections"
            );
            self.refresh_all_child_session_projections(cx, "activity_updated");
            return;
        }
        self.activity.since_seq = event.seq;
        let mut rebuild_activity = false;
        if self.session_id.is_some() {
            let outcome = self.activity.apply_session_event(&event);
            if outcome.rebuild_activity {
                rebuild_activity = true;
            }
        }
        let log_apply = matches!(
            event.kind,
            EventKind::ProviderTurnStarted
                | EventKind::SessionIdle
                | EventKind::PromptAdmitted
                | EventKind::Prompted
                | EventKind::SessionInterrupted
        );
        if log_apply {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                session_id = ?event.session_id,
                kind = %event.kind.as_str(),
                seq = event.seq,
                busy = self.session_busy,
                "apply_event"
            );
        } else {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                session_id = ?event.session_id,
                kind = %event.kind.as_str(),
                seq = event.seq,
                busy = self.session_busy,
                "apply_event"
            );
        }
        match event.kind {
            EventKind::SessionCreated => {
                self.set_session_busy(false, "Ready");
            }
            EventKind::PromptAdmitted => {
                self.set_session_busy(true, "Prompt queued…");
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.insert(session_id);
                }
                if let Some(prompt) = event.payload.get("prompt").and_then(|v| v.as_str()) {
                    if let Some(sid) = event.session_id {
                        self.session_first_prompts
                            .entry(sid)
                            .or_insert_with(|| prompt.to_string());
                    }
                }
            }
            EventKind::Prompted => {
                self.set_session_busy(true, "Thinking…");
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.insert(session_id);
                }
            }
            EventKind::MessageDelta => {
                let delta = event.payload["delta"].as_str().unwrap_or("");
                if !delta.is_empty() {
                    self.set_session_busy(true, "Generating response…");
                }
            }
            EventKind::MessageCompleted => {
                let content = event.payload["content"]
                    .as_str()
                    .unwrap_or("(completed)")
                    .to_string();
                let has_tool_calls = event.payload["has_tool_calls"].as_bool().unwrap_or(false);
                if !has_tool_calls {
                    if let Some(message_id) = event.payload["message_id"].as_str() {
                        self.activity
                            .expanded_activity_leaves
                            .insert(message_id.to_string());
                    }
                }
                let reasoning = event.payload["reasoning"]
                    .as_str()
                    .map(String::from)
                    .filter(|r| !r.is_empty());
                if let Some((input, output, reasoning_tokens)) =
                    token_usage_from_payload(&event.payload)
                {
                    self.apply_token_usage(input, output, reasoning_tokens);
                } else {
                    self.apply_token_usage(
                        0,
                        Self::estimate_tokens(&content),
                        reasoning.as_deref().map(Self::estimate_tokens).unwrap_or(0),
                    );
                }
                if has_tool_calls {
                    self.set_session_busy(true, "Running tools…");
                }
            }
            EventKind::ToolStarted => {
                let name = event.payload["tool_name"]
                    .as_str()
                    .unwrap_or("tool")
                    .to_string();
                let input = event.payload.get("input").cloned().unwrap_or_default();
                let detail = format_tool_input(&name, &input);
                if let Some(path) = extract_tool_path(&name, &input) {
                    self.mark_file_changed(&path);
                }
                self.set_session_busy(true, format!("Running tool: {name}"));
                if let Some(id) = event.payload["tool_call_id"].as_str() {
                    self.activity.detail_store.insert(
                        id,
                        crate::activity_timeline::DetailPayload {
                            input: Some(detail),
                            output: None,
                            todos: None,
                            title: event
                                .payload
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            url: None,
                            web_preview: None,
                        },
                    );
                }
            }
            EventKind::ToolCompleted => {
                let name = event
                    .payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let output = event.payload.get("output").cloned().unwrap_or_default();
                if let Some(id) = event.payload["tool_call_id"].as_str() {
                    let streamed = self
                        .activity
                        .detail_store
                        .get(id)
                        .and_then(|d| d.output.clone());
                    let merged_display = if name == "bash" {
                        activity_click_router::merge_bash_completed_display(
                            streamed.as_deref(),
                            &output,
                        )
                    } else {
                        String::new()
                    };
                    let stored_output = if name == "bash" && !merged_display.is_empty() {
                        merged_display.clone()
                    } else {
                        truncate_text(
                            &serde_json::to_string_pretty(&output)
                                .unwrap_or_else(|_| output.to_string()),
                            50_000,
                        )
                    };
                    self.activity.detail_store.insert(
                        id,
                        crate::activity_timeline::DetailPayload {
                            input: self
                                .activity
                                .detail_store
                                .get(id)
                                .and_then(|d| d.input.clone()),
                            output: Some(stored_output),
                            todos: None,
                            title: event
                                .payload
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            url: None,
                            web_preview: None,
                        },
                    );
                    if name == "bash" {
                        self.refresh_bash_terminal_output(id, cx);
                        self.close_bash_terminal_for_detail(id, cx);
                    }
                }
                if let Some(path) = extract_tool_path(&name, &event.payload) {
                    self.mark_file_changed(&path);
                }
                if name == "task" {
                    if let Some(child_id) = task_session_id_from_payload(&event.payload) {
                        self.refresh_child_session_projection(child_id, cx);
                    }
                }
                self.refresh_git_status_local();
                self.set_session_busy(true, "Continuing analysis…");
            }
            EventKind::TodoUpdated => {
                let todos: Vec<TodoItem> = event
                    .payload
                    .get("todos")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                self.session_todos = todos.clone();
                let leaf_id = format!("todo-{}", event.seq);
                self.highlight_todo_leaf(&leaf_id);
                self.activity.detail_store.insert(
                    leaf_id,
                    crate::activity_timeline::DetailPayload {
                        input: None,
                        output: None,
                        todos: Some(todos),
                        title: Some("Updated todos".into()),
                        url: None,
                        web_preview: None,
                    },
                );
            }
            EventKind::ToolProgress => {
                if let (Some(id), Some(chunk)) = (
                    event.payload["tool_call_id"].as_str(),
                    event.payload["chunk"].as_str(),
                ) {
                    let stream = event.payload["stream"].as_str().unwrap_or("stdout");
                    self.activity.detail_store.append_output(id, chunk, stream);
                    if self.bash_output_detail_ref.as_deref() == Some(id)
                        || self
                            .terminals
                            .iter()
                            .any(|t| t.bash_detail_key.as_deref() == Some(id))
                    {
                        self.refresh_bash_terminal_output(id, cx);
                    }
                }
            }
            EventKind::ProviderTurnStarted => {
                let turn = event.payload["turn"].as_u64().unwrap_or(0);
                let max_turns = event.payload["max_turns"].as_u64().unwrap_or(0);
                let message_count = event.payload["message_count"].as_u64().unwrap_or(0);
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.insert(session_id);
                }
                let status = if turn > 0 && max_turns > 0 {
                    format!("Calling model… (turn {turn}/{max_turns}, {message_count} msgs)")
                } else if turn > 0 {
                    format!("Calling model… (turn {turn}, {message_count} msgs)")
                } else {
                    format!("Calling model… ({message_count} msgs)")
                };
                self.set_session_busy(true, status.clone());
                self.activity
                    .expanded_activity_groups
                    .insert(format!("turn-{}", event.seq));
            }
            EventKind::PermissionRequested | EventKind::PolicyPrompted => {
                self.handle_policy_prompt_event(&event);
            }
            EventKind::ContextUpdated => {
                self.set_session_busy(true, "Updating context…");
                let summary = event.payload["summary"]
                    .as_str()
                    .or_else(|| event.payload["detail"].as_str())
                    .unwrap_or("Context sources refreshed")
                    .to_string();
                let detail = event.payload["sources"]
                    .as_array()
                    .map(|sources| {
                        sources
                            .iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| summary.clone());
            }
            EventKind::SessionCompacted => {
                truncate_session_events_on_compaction(&mut self.activity.session_events);
                self.context_usage = ContextUsageState::default();
                self.sync_context_window();
                self.set_session_busy(false, "Ready");
                self.hydrate_session_from_api(cx, true);
            }
            EventKind::SessionInterrupted => {
                let error = event.payload["error"].as_str().unwrap_or("").to_string();
                self.pending_policies.clear();
                self.policy_reject_expanded = false;
                self.activity.ignore_stale_run_events = true;
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.remove(&session_id);
                }
                if error.is_empty() {
                    self.set_session_busy(false, "Interrupted");
                } else {
                    self.set_session_busy(false, "Error");
                    self.composer_error = Some(if error.contains("context unavailable") {
                        format!(
                            "Project context is not ready. Add AGENTS.md to the project root \
                             (or configure instructions in {CONFIG_FILE} for {PRODUCT_NAME}), \
                             then send your prompt again."
                        )
                    } else {
                        truncate_text(&error, 2_000)
                    });
                }
            }
            EventKind::SessionIdle => {
                self.set_session_busy(false, "Ready");
                self.activity.ignore_stale_run_events = true;
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.remove(&session_id);
                }
            }
            EventKind::McpStatusChanged => {
                let name = event.payload["name"].as_str().unwrap_or("mcp");
                let connected = event.payload["connected"].as_bool().unwrap_or(false);
                self.refresh_mcp_catalog(cx);
            }
            EventKind::RevertCommitted => {}
            EventKind::ActivityUpdated => {}
            EventKind::PermissionResolved | EventKind::PolicyResolved => {
                let id = event.payload["policy_id"]
                    .as_str()
                    .or_else(|| event.payload["permission_id"].as_str())
                    .unwrap_or("");
                if !id.is_empty() {
                    self.remove_policy_prompt(id);
                }
            }
            EventKind::QuestionRequested => {
                let question = event.payload["question"].as_str().unwrap_or("").to_string();
                let options: Vec<String> = event.payload["options"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let allow_custom = event.payload["allow_custom"].as_bool().unwrap_or(true);
                let has_option_list = !options.is_empty();
                self.pending_question = Some(QuestionPromptState {
                    question,
                    options,
                    allow_custom,
                    custom_mode: false,
                });
                let placeholder = if has_option_list && allow_custom {
                    "输入不在上述选项中的答案"
                } else {
                    "Your answer…"
                };
                self.question_input.update(cx, |input, cx| {
                    input.set_placeholder(placeholder, cx);
                    input.clear(cx);
                });
            }
            EventKind::PlanCompleted => {
                self.ingest_plan_completed_event(&event, cx);
            }
        }
        if rebuild_activity {
            let roots_before = self.activity.activity_tree.roots.len();
            self.rebuild_activity_tree("apply_event");
            let roots_after = self.activity.activity_tree.roots.len();
            if roots_after == 0 && roots_before > 0 {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    kind = %event.kind.as_str(),
                    seq = event.seq,
                    event_count = self.activity.session_events.len(),
                    roots_before,
                    "apply_event rebuild cleared activity tree"
                );
            }
        }
    }

    pub(crate) fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        if self.composer_blocked() {
            return;
        }
        let mut text = self.prompt_input.read(cx).text();
        if text.is_empty() || text == "Type a prompt…" {
            return;
        }
        let (_, provider_id) = self.resolve_selected_model();
        if let Some(provider) = self.providers.iter().find(|p| p.id == provider_id) {
            if !provider.connected {
                self.composer_error = Some(format!(
                    "Provider '{}' is not connected — add an API key in Settings → AI & Providers.",
                    provider.name
                ));
                cx.notify();
                return;
            }
        }
        self.composer_error = None;
        if let Some(dir) = self.project_dir.clone() {
            if let Some(message) = Self::ensure_agents_md(&dir) {
                self.composer_error = Some(format!(
                    "{message} Send your prompt again after AGENTS.md is ready."
                ));
                cx.notify();
                return;
            }
        }
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            text_len = text.len(),
            session_id = ?self.session_id,
            "submit_prompt invoked"
        );
        self.activity.ignore_stale_run_events = false;
        // Cancel any in-flight timeline hydration so it cannot wipe the optimistic user row.
        self.hydrate_generation.fetch_add(1, Ordering::SeqCst);
        self.start_event_listener(cx);
        let user_prompt = text.clone();
        let Some(session_id) = self.session_id else {
            cx.notify();
            return;
        };

        let mut blocks = Vec::new();
        if let Some(path) = &self.editor.path {
            blocks.push(format!("Regarding file `{path}`:\n\n{text}"));
        } else {
            blocks.push(text.clone());
        }
        for attachment in &self.attachments {
            blocks.push(format!(
                "Attached file `{}`:\n```\n{}\n```",
                attachment.path, attachment.label
            ));
        }
        text = blocks.join("\n\n");
        self.session_first_prompts
            .entry(session_id)
            .or_insert_with(|| truncate_preview(&user_prompt, 80));
        let delivery = self.prompt_delivery;
        let optimistic = EventEnvelope {
            seq: self.activity.since_seq.saturating_add(1),
            kind: EventKind::PromptAdmitted,
            session_id: Some(session_id),
            payload: serde_json::json!({
                "prompt": text,
                "delivery": match delivery {
                    deepseekcode_schema::session::PromptDelivery::Queue => "queue",
                    deepseekcode_schema::session::PromptDelivery::Steer => "steer",
                },
            }),
            created_at: chrono::Utc::now(),
        };
        push_session_event(&mut self.activity.session_events, optimistic);
        self.rebuild_activity_tree("submit_prompt");
        self.attachments.clear();
        self.session_follow_tail = true;
        self.maybe_scroll_session_to_tail();
        self.set_session_busy(true, "Sending prompt…");
        self.normalize_model_selection();
        self.persist_preferences();
        let client_slot = Arc::clone(&self.client);
        let agent = self.selected_agent.to_string();
        let (model_id, provider_id) = self.resolve_selected_model();
        let title_candidate = Some(truncate_preview(&user_prompt, 40));
        self.prompt_input.update(cx, |input, cx| input.clear(cx));
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                if let Err(e) = client
                    .update_model(session_id, Some(agent.as_str()), &model_id, &provider_id)
                    .await
                {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                        session_id = %session_id,
                        provider_id = %provider_id,
                        model_id = %model_id,
                        error = %e,
                        "failed to sync session model before prompt"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.set_session_busy(false, "Error");
                        cx.notify();
                    });
                    return;
                }
                match client
                    .prompt(PromptRequest {
                        id: None,
                        session_id,
                        prompt: text,
                        delivery,
                        resume: true,
                    })
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::DESKTOP,
                            session_id = %session_id,
                            "prompt submitted"
                        );
                        if let Some(title) = title_candidate.as_ref() {
                            let _ = client
                                .update_session(session_id, Some(title.as_str()))
                                .await;
                        }
                        let _ = view.update(cx, |this, cx| {
                            if let Some(title) = title_candidate {
                                this.session_first_prompts.insert(session_id, title.clone());
                                this.update_session_title_in_nav(session_id, title);
                            }
                            this.refresh_project_nav_from_client(cx);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            session_id = %session_id,
                            error = %e,
                            "prompt submit failed"
                        );
                        let _ = view.update(cx, |this, cx| {
                            this.set_session_busy(false, "Error");
                            this.composer_error = Some(e.to_string());
                            cx.notify();
                        });
                    }
                }
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn interrupt_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        self.set_session_busy(true, "Stopping…");
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                if let Err(e) = client.interrupt(session_id).await {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                        session_id = %session_id,
                        error = %e,
                        "interrupt failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.set_session_busy(false, "Error");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn select_timeline_message(
        &mut self,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        if self.selected_message_id == Some(message_id) {
            self.selected_message_id = None;
        } else {
            self.selected_message_id = Some(message_id);
        }
        cx.notify();
    }

    pub(crate) fn toggle_export_sanitize(&mut self, cx: &mut Context<Self>) {
        self.export_sanitize = !self.export_sanitize;
        cx.notify();
    }

    pub(crate) fn set_prompt_delivery(&mut self, delivery: PromptDelivery, cx: &mut Context<Self>) {
        self.prompt_delivery = delivery;
        self.persist_preferences();
        cx.notify();
    }

    pub(crate) fn toggle_composer_dropdown(
        &mut self,
        kind: &'static str,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let next = if self.composer_dropdown.as_deref() == Some(kind) {
            None
        } else {
            Some(SharedString::from(kind))
        };
        self.session_context_menu = None;
        self.composer_dropdown = next;
        self.popover_anchor = if self.composer_dropdown.is_some() {
            Some(anchor)
        } else {
            None
        };
        cx.notify();
    }

    pub(crate) fn select_composer_dropdown(
        &mut self,
        kind: &str,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.composer_dropdown = None;
        self.popover_anchor = None;
        match kind {
            "agent" => self.set_agent(&id, cx),
            "provider" => {
                if let Some(provider) = self.providers.iter().find(|p| p.id == id) {
                    let pid = provider.id.clone();
                    let pname = provider.name.clone();
                    let connected = provider.connected;
                    if connected {
                        self.selected_provider = Some(id);
                        self.persist_preferences();
                    } else {
                        self.open_api_key_dialog(&pid, &pname, cx);
                        return;
                    }
                }
            }
            "model" => {
                if let Some((provider_id, model_id)) = id.split_once(':') {
                    if let Some(provider) = self.providers.iter().find(|p| p.id == provider_id) {
                        let pid = provider.id.clone();
                        let pname = provider.name.clone();
                        let connected = provider.connected;
                        let model_name = provider
                            .models
                            .iter()
                            .find(|m| m.id == model_id)
                            .map(|m| m.name.clone());
                        if !connected {
                            self.open_api_key_dialog(&pid, &pname, cx);
                            return;
                        }
                        if let Some(name) = model_name {
                            self.set_model(model_id, &name, provider_id, cx);
                            return;
                        }
                    }
                }
            }
            "delivery" => {
                let delivery = if id == "queue" {
                    PromptDelivery::Queue
                } else {
                    PromptDelivery::Steer
                };
                self.set_prompt_delivery(delivery, cx);
                return;
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn open_api_key_dialog(
        &mut self,
        provider_id: &str,
        provider_name: &str,
        cx: &mut Context<Self>,
    ) {
        self.api_key_dialog = Some(ApiKeyDialogState {
            provider_id: provider_id.to_string(),
            provider_name: provider_name.to_string(),
        });
        self.api_key_error = None;
        cx.notify();
    }

    pub(crate) fn compact_current_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        self.set_session_busy(true, "Compacting…");
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                let _ = view.update(cx, |this, cx| {
                    this.set_session_busy(false, "Error");
                    cx.notify();
                });
                return;
            };
            let result = client.compact_session(session_id).await;
            let failed = result.is_err();
            let message = match result {
                Ok(()) => "Compaction requested".to_string(),
                Err(e) => format!("Compaction failed: {e}"),
            };
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.compaction_message = message;
                if failed {
                    this.set_session_busy(false, "Error");
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn attach_clipboard_image(
        &mut self,
        bytes: Vec<u8>,
        format: gpui::ImageFormat,
        cx: &mut Context<Self>,
    ) {
        let ext = match format {
            gpui::ImageFormat::Png => "png",
            gpui::ImageFormat::Jpeg => "jpg",
            gpui::ImageFormat::Gif => "gif",
            gpui::ImageFormat::Webp => "webp",
            gpui::ImageFormat::Bmp => "bmp",
            gpui::ImageFormat::Tiff => "tiff",
            gpui::ImageFormat::Ico => "ico",
            _ => "png",
        };
        let dir = std::env::temp_dir().join("deepseekcode-paste");
        let _ = std::fs::create_dir_all(&dir);
        let file_name = format!("paste-{}.{}", chrono::Utc::now().timestamp_millis(), ext);
        let path = dir.join(file_name);
        if std::fs::write(&path, bytes).is_ok() {
            self.attach_path(path, cx);
        }
    }

    pub(crate) fn connect_mcp_server(&mut self, name: String, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let _ = client.connect_mcp(&name).await;
            let mcp = client.list_mcp().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.mcp = mcp;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn disconnect_mcp_server(&mut self, name: String, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let _ = client.disconnect_mcp(&name).await;
            let mcp = client.list_mcp().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.mcp = mcp;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn remove_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let message = match client.remove_worktree(&path).await {
                Ok(()) => format!("Removed worktree {path}"),
                Err(e) => format!("Failed to remove worktree: {e}"),
            };
            let worktrees = client.list_worktrees().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.worktrees = worktrees;
                this.settings_catalog.worktree_message = message;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn prune_worktrees(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let message = match client.prune_worktrees().await {
                Ok(()) => "Pruned stale worktrees".to_string(),
                Err(e) => format!("Failed to prune worktrees: {e}"),
            };
            let worktrees = client.list_worktrees().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.worktrees = worktrees;
                this.settings_catalog.worktree_message = message;
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_session_model(&mut self, cx: &mut Context<Self>) {
        self.update_session_model(cx);
    }

    pub(crate) fn respond_policy_once(&mut self, id: String, cx: &mut Context<Self>) {
        self.send_policy_reply(
            id,
            PolicyReply {
                reply: PolicyReplyKind::Once,
                message: None,
            },
            cx,
        );
    }

    pub(crate) fn respond_policy_project(&mut self, id: String, cx: &mut Context<Self>) {
        self.send_policy_reply(
            id,
            PolicyReply {
                reply: PolicyReplyKind::Project,
                message: None,
            },
            cx,
        );
    }

    pub(crate) fn handle_policy_deny_click(&mut self, id: String, cx: &mut Context<Self>) {
        if self.policy_reject_expanded {
            self.respond_policy_reject(Some(id), cx);
        } else {
            self.policy_reject_expanded = true;
            cx.notify();
        }
    }

    pub(crate) fn respond_policy_reject(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let id = id.or_else(|| self.pending_policies.front().map(|p| p.id.clone()));
        let Some(id) = id else {
            return;
        };
        let feedback = self.permission_deny_input.read(cx).text();
        let message = feedback.trim();
        let message = if message.is_empty() {
            None
        } else {
            Some(message.to_string())
        };
        self.clear_policy_reject_input(cx);
        self.send_policy_reply(
            id,
            PolicyReply {
                reply: PolicyReplyKind::Reject,
                message,
            },
            cx,
        );
    }

    fn send_policy_reply(&mut self, id: String, reply: PolicyReply, cx: &mut Context<Self>) {
        self.remove_policy_prompt(&id);
        let client_slot = Arc::clone(&self.client);
        let restore_id = id.clone();
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                if let Err(e) = client.reply_policy(&id, reply).await {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                        policy_id = %id,
                        error = %e,
                        "policy reply failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.sync_pending_permissions(cx);
                        this.composer_error =
                            Some("Policy reply failed — request restored to the queue.".into());
                        cx.notify();
                    });
                }
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        let _ = restore_id;
        cx.notify();
    }

    #[allow(dead_code)]
    pub(crate) fn respond_permission(&mut self, allow: bool, always: bool, cx: &mut Context<Self>) {
        if always {
            if let Some(p) = self.pending_policies.front() {
                self.respond_policy_project(p.id.clone(), cx);
            }
        } else if allow {
            if let Some(p) = self.pending_policies.front() {
                self.respond_policy_once(p.id.clone(), cx);
            }
        } else {
            self.respond_policy_reject(None, cx);
        }
    }

    pub(crate) fn submit_question_answer(&mut self, answer: String, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        self.pending_question = None;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                if let Err(e) = client.submit_question_answer(session_id, &answer).await {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                        session_id = %session_id,
                        error = %e,
                        "question answer failed"
                    );
                }
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn enable_question_custom_mode(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.pending_question.as_mut() {
            prompt.custom_mode = true;
            cx.notify();
        }
    }

    fn sync_agent_from_nav(&mut self, session_id: SessionId) {
        for project in &self.project_nav {
            if let Some(session) = project.sessions.iter().find(|s| s.id == session_id) {
                self.selected_agent = agent_selector_for_kind(session.agent).into();
                return;
            }
        }
    }

    pub(crate) fn refresh_session_plans(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            self.session_plans.clear();
            return;
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let plans = if let Some(client) = client {
                client
                    .list_session_plans(session_id)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let _ = view.update(cx, |this, cx| {
                this.session_plans = plans;
                cx.notify();
            });
        })
        .detach();
    }

    fn rebuild_session_plans_from_events(&mut self) {
        let dismissed: HashSet<String> = self
            .session_plans
            .iter()
            .filter(|p| p.implement_prompt_dismissed)
            .map(|p| p.plan_id.clone())
            .collect();
        self.session_plans =
            deepseekcode_sdk::session_plans_from_events(&self.activity.session_events, &dismissed);
    }

    fn ingest_plan_completed_event(&mut self, event: &EventEnvelope, cx: &mut Context<Self>) {
        self.rebuild_session_plans_from_events();
        let plan_id = event.payload["plan_id"].as_str().unwrap_or("").to_string();
        if !plan_id.is_empty() {
            if let Some(plan) = self.session_plans.iter_mut().find(|p| p.plan_id == plan_id) {
                plan.implement_prompt_dismissed = false;
            }
        }
        self.refresh_session_plans(cx);
        cx.notify();
    }

    fn plan_implement_prompt(&self) -> Option<PlanImplementPromptState> {
        if self.selected_agent.as_ref() != "plan" {
            return None;
        }
        let plan = self
            .session_plans
            .iter()
            .filter(|p| !p.implement_prompt_dismissed)
            .last()?;
        Some(PlanImplementPromptState {
            plan_id: plan.plan_id.clone(),
            title: plan.title.clone(),
            plan_path: plan.plan_path.clone(),
            summary: plan.summary.clone(),
        })
    }

    pub(crate) fn dismiss_plan_implement_prompt(&mut self, plan_id: &str, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        if let Some(plan) = self.session_plans.iter_mut().find(|p| p.plan_id == plan_id) {
            plan.implement_prompt_dismissed = true;
        }
        let plan_id = plan_id.to_string();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client
                    .dismiss_plan_implement_prompt(session_id, &plan_id)
                    .await;
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn implement_from_plan(&mut self, plan_id: &str, cx: &mut Context<Self>) {
        let Some(plan) = self
            .session_plans
            .iter()
            .find(|p| p.plan_id == plan_id)
            .cloned()
        else {
            return;
        };
        self.plan_picker_expanded = false;
        self.set_agent("build", cx);
        let prompt = implement_prompt_from_plan_path(&plan.plan_path);
        self.prompt_input.update(cx, |input, cx| {
            input.set_text(&prompt, cx);
        });
        cx.notify();
    }

    pub(crate) fn open_plan_file(&mut self, plan_path: &str, cx: &mut Context<Self>) {
        let path = if let Some(dir) = self.project_dir.as_ref() {
            let candidate = dir.join(plan_path);
            if candidate.is_file() {
                candidate.display().to_string()
            } else {
                plan_path.to_string()
            }
        } else {
            plan_path.to_string()
        };
        self.open_file(&path, cx);
    }

    pub(crate) fn toggle_plan_picker(&mut self, cx: &mut Context<Self>) {
        self.plan_picker_expanded = !self.plan_picker_expanded;
        cx.notify();
    }

    pub(crate) fn toggle_session_plans_menu(&mut self, cx: &mut Context<Self>) {
        self.plan_picker_expanded = !self.plan_picker_expanded;
        cx.notify();
    }

    pub(crate) fn toggle_composer_todo_expanded(&mut self, cx: &mut Context<Self>) {
        self.composer_todo_expanded = !self.composer_todo_expanded;
        cx.notify();
    }

    pub(crate) fn highlight_todo_at_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.session_todos.len() {
            self.highlighted_todo_index = Some(index);
            self.highlighted_todo_leaf_id =
                find_todo_leaf_id_for_index(&self.activity.activity_tree, index);
            if let Some(leaf_id) = self.highlighted_todo_leaf_id.clone() {
                for group_id in ancestor_group_ids_for_leaf(&self.activity.activity_tree, &leaf_id)
                {
                    self.activity.collapsed_activity_groups.remove(&group_id);
                }
                self.pending_scroll_to_activity_leaf = Some(leaf_id);
            }
        }
        cx.notify();
    }

    pub(crate) fn cycle_todo_status(
        &mut self,
        index: usize,
        next_status: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session_id else {
            return;
        };
        if index >= self.session_todos.len() {
            return;
        }
        self.session_todos[index].status = next_status.to_string();
        self.highlight_todo_at_index(index, cx);
        let todos = self.session_todos.clone();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client.session_todo_update(session_id, &todos).await;
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn connect_provider(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let name = self
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| provider_id.to_string());
        self.api_key_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("Enter API key (stored securely in OS keychain)", cx);
        });
        self.base_url_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("Base URL (optional, e.g. https://api.openai.com/v1)", cx);
        });
        self.api_key_dialog = Some(ApiKeyDialogState {
            provider_id: provider_id.to_string(),
            provider_name: name,
        });
        cx.notify();
    }

    pub(crate) fn attach_file(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            let path = rfd::FileDialog::new().pick_file();
            if let Some(path) = path {
                let _ = view.update(cx, |this, cx| {
                    this.attach_path(path, cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn attach_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if !path.is_file() {
            return;
        }
        let path_str = path.display().to_string();
        let is_image = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico"
                )
            })
            .unwrap_or(false);
        let label = if is_image {
            format!(
                "[image: {}]",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("image")
            )
        } else {
            let content =
                std::fs::read_to_string(&path).unwrap_or_else(|e| format!("(unreadable: {e})"));
            truncate_preview(&content, 4_000)
        };
        self.attachments.push(PromptAttachment {
            path: path_str,
            label,
        });
        cx.notify();
    }

    pub(crate) fn attach_external_paths(
        &mut self,
        paths: &gpui::ExternalPaths,
        cx: &mut Context<Self>,
    ) {
        let dropped: Vec<_> = paths.paths().to_vec();
        if dropped.is_empty() {
            return;
        }
        for path in dropped {
            self.attach_path(path, cx);
        }
    }

    pub(crate) fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    fn load_settings_catalog(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let project_dir = self.project_dir.clone();
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let (
                mcp,
                skills,
                plugins,
                worktrees,
                location_summary,
                health_status,
                project_directories,
                policy_grants,
            ) = if let Some(client) = client {
                let location_summary = client
                    .get_location()
                    .await
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_default();
                let health_status = if client.health().await.is_ok() {
                    "ok".to_string()
                } else {
                    "error".to_string()
                };
                let project_directories = client
                    .list_project_directories()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|entry| {
                        entry
                            .get("directory_path")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .collect();
                let policy_grants = client.list_policy_grants().await.unwrap_or_default();
                (
                    client.list_mcp().await.unwrap_or_default(),
                    client.list_skills().await.unwrap_or_default(),
                    client.list_plugins().await.unwrap_or_default(),
                    client.list_worktrees().await.unwrap_or_default(),
                    location_summary,
                    health_status,
                    project_directories,
                    policy_grants,
                )
            } else {
                (
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    String::new(),
                    "unknown".to_string(),
                    Vec::new(),
                    Vec::new(),
                )
            };
            let compaction_summary = project_dir
                .as_ref()
                .and_then(|dir| {
                    deepseekcode_config::load_effective_config(camino::Utf8Path::new(
                        &dir.to_string_lossy(),
                    ))
                    .ok()
                })
                .map(|cfg| {
                    if let Some(compaction) = cfg.compaction {
                        format!(
                            "auto: {}\nbuffer: {}\nkeep.tokens: {:?}",
                            compaction.auto,
                            compaction.buffer,
                            compaction.keep.and_then(|k| k.tokens)
                        )
                    } else {
                        "No compaction section in config (defaults apply)".into()
                    }
                })
                .unwrap_or_else(|| "No project loaded".into());
            let _ = view.update(cx, |this, cx| {
                let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
                let plugin_names: Vec<String> = plugins.iter().map(|p| p.name.clone()).collect();
                this.settings_catalog = SettingsCatalog {
                    mcp,
                    skills: skill_names,
                    plugins: plugin_names,
                    compaction_summary,
                    compaction_message: String::new(),
                    worktrees,
                    worktree_message: String::new(),
                    location_summary,
                    health_status,
                    project_directories,
                    policy_grants,
                    policy_mode: project_dir
                        .as_ref()
                        .and_then(|dir| {
                            deepseekcode_config::load_effective_config(camino::Utf8Path::new(
                                &dir.to_string_lossy(),
                            ))
                            .ok()
                        })
                        .and_then(|cfg| cfg.policy_mode)
                        .unwrap_or_default(),
                    loaded: true,
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_settings(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.settings_open = true;
        if self.settings_section != section {
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.settings_section = section;
        self.load_permission_inputs(cx);
        self.load_settings_catalog(cx);
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        cx.notify();
    }

    pub(crate) fn open_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section != section {
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.settings_section = section;
        cx.notify();
    }

    pub(crate) fn set_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.selected_agent = agent.into();
        self.persist_preferences();
        self.update_session_model(cx);
    }

    pub(crate) fn set_model(
        &mut self,
        model_id: &str,
        model_name: &str,
        provider_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.selected_model = model_name.into();
        self.selected_provider = Some(provider_id.to_string());
        self.persist_preferences();
        self.update_session_model(cx);
        let _ = model_id;
    }

    pub(crate) fn disconnect_provider(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let provider_id = provider_id.to_string();
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client.disconnect_provider(&provider_id).await;
                let providers = client.list_providers().await.unwrap_or_default();
                let _ = view.update(cx, |this, cx| {
                    this.providers = providers;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn persist_preferences(&self) {
        save_preferences(&AppPreferences {
            selected_agent: self.selected_agent.to_string(),
            selected_model: self.selected_model.to_string(),
            selected_provider: self.selected_provider.clone(),
            prompt_delivery: self.prompt_delivery,
            theme: self.theme,
            server_url: self.server_url.clone(),
            server_password: self.server_password.clone(),
        });
    }

    pub(crate) fn set_theme(&mut self, theme_id: ThemeId, cx: &mut Context<Self>) {
        self.theme = theme_id;
        crate::theme::DeepSeekCodeTheme::switch(cx, theme_id);
        self.persist_preferences();
        cx.notify();
    }

    pub(crate) fn save_server_settings(&mut self, cx: &mut Context<Self>) {
        let url_text = self.server_url_input.read(cx).text();
        let pass_text = self.server_password_input.read(cx).text();
        self.server_url = if url_text.is_empty() {
            None
        } else {
            Some(url_text)
        };
        self.server_password = if pass_text.is_empty() {
            None
        } else {
            Some(pass_text)
        };
        self.persist_preferences();
        if let Some(dir) = self.project_dir.clone() {
            self.bootstrap_project(dir, cx);
        }
        cx.notify();
    }

    pub(crate) fn open_agents_md(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = &self.project_dir {
            let path = dir.join("AGENTS.md");
            let path_str = path.to_string_lossy().to_string();
            if path.exists() {
                self.open_file(&path_str, cx);
            }
        }
        self.close_settings(cx);
    }

    pub(crate) fn open_project_config(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = &self.project_dir {
            let candidates = [
                dir.join("deepseekcode.jsonc"),
                dir.join(".deepseekcode").join("deepseekcode.jsonc"),
            ];
            for path in candidates {
                if path.exists() {
                    let path_str = path.to_string_lossy().to_string();
                    self.open_file(&path_str, cx);
                    self.close_settings(cx);
                    return;
                }
            }
            let path_str = dir.join("deepseekcode.jsonc").to_string_lossy().to_string();
            self.open_file(&path_str, cx);
            self.close_settings(cx);
        }
    }

    pub(crate) fn open_openapi_docs(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |_view, _cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let url = format!("{}/api/v1/openapi.json", client.base_url());
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", "", &url])
                        .spawn();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn move_current_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        self.close_session_menu(cx);
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let dest = rfd::FileDialog::new().pick_folder();
            let Some(directory) = dest else {
                return;
            };
            let dir_str = directory.to_string_lossy().to_string();
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                match client.move_session(session_id, &dir_str).await {
                    Ok(()) => {
                        let _ = view.update(cx, |this, cx| {
                            this.refresh_project_nav_from_client(cx);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            error = %e,
                            "move session failed"
                        );
                    }
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn refresh_file_tree(&mut self, cx: &mut Context<Self>) {
        self.file_tree.entries.clear();
        self.file_tree.expanded.clear();
        self.load_file_tree_root(cx);
        cx.notify();
    }

    pub(crate) fn add_project_directory(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let picked = rfd::FileDialog::new().pick_folder();
            let Some(directory) = picked else {
                return;
            };
            let dir_str = directory.to_string_lossy().to_string();
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client.add_project_directory(&dir_str, None).await;
                let _ = view.update(cx, |this, cx| {
                    this.refresh_file_tree(cx);
                    this.refresh_project_nav_from_client(cx);
                    this.load_settings_catalog(cx);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_file_diff(&mut self, path: &str, cx: &mut Context<Self>) {
        self.open_file(path, cx);
    }

    pub(crate) fn switch_editor_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.editor_tabs.len() {
            self.active_editor_tab = index;
            if let Some(path) = self.editor_tabs.get(index).cloned() {
                self.open_file(&path, cx);
            }
        }
    }

    pub(crate) fn close_editor_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.editor_tabs.len() {
            return;
        }
        self.editor_tabs.remove(index);
        if self.editor_tabs.is_empty() {
            self.editor = EditorState::default();
            self.active_editor_tab = 0;
        } else {
            self.active_editor_tab = self.active_editor_tab.min(self.editor_tabs.len() - 1);
            if let Some(path) = self.editor_tabs.get(self.active_editor_tab).cloned() {
                self.open_file(&path, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn save_editor_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.editor.path.clone() else {
            return;
        };
        let content = self.file_editor_input.read(cx).text();
        self.editor.set_content(content.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                match client.fs_write(&path, &content).await {
                    Ok(()) => {
                        let _ = view.update(cx, |this, cx| {
                            this.editor_read_only = false;
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            error = %e,
                            "save file failed"
                        );
                        let _ = view.update(cx, |this, cx| {
                            this.editor_read_only = true;
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }

    pub(crate) fn close_terminal_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        if self.terminals.is_empty() {
            self.active_terminal = 0;
        } else {
            self.active_terminal = self.active_terminal.min(self.terminals.len() - 1);
        }
        cx.notify();
    }

    fn close_bash_terminal_for_detail(&mut self, detail_key: &str, cx: &mut Context<Self>) {
        if let Some(idx) = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        {
            self.close_terminal_tab(idx, cx);
        }
    }

    pub(crate) fn begin_panel_resize(
        &mut self,
        edge: PanelResizeEdge,
        start_x: Pixels,
        cx: &mut Context<Self>,
    ) {
        self.panel_resize_edge = Some(edge);
        self.panel_resize_start_x = f32::from(start_x);
        self.panel_resize_start_width = match edge {
            PanelResizeEdge::Sidebar => f32::from(self.layout.sidebar_width),
            PanelResizeEdge::RightPanel => f32::from(self.layout.right_panel_width),
        };
        cx.notify();
    }

    pub(crate) fn handle_panel_resize_drag(
        &mut self,
        event: &DragMoveEvent<PanelResizeEdge>,
        cx: &mut Context<Self>,
    ) {
        let edge = *event.drag(cx);
        let delta = f32::from(event.event.position.x) - self.panel_resize_start_x;
        match edge {
            PanelResizeEdge::Sidebar => {
                let next = self.panel_resize_start_width + delta;
                self.layout.sidebar_width = px(next.clamp(180., 480.));
            }
            PanelResizeEdge::RightPanel => {
                let next = self.panel_resize_start_width - delta;
                self.layout.right_panel_width = px(next.clamp(240., 640.));
            }
        }
        self.sync_terminal_pty_size(cx);
        cx.notify();
    }

    pub(crate) fn sync_terminal_pty_size(&mut self, cx: &mut Context<Self>) {
        if self.layout.right_panel_tab != RightPanelTab::Terminal {
            return;
        }
        let cols = (f32::from(self.layout.right_panel_width) / 7.5).clamp(40., 200.) as u16;
        let rows = 32u16;
        if self.terminal_pty_size == Some((rows, cols)) {
            return;
        }
        self.terminal_pty_size = Some((rows, cols));
        let idx = self.active_terminal;
        if let Some(tab) = self.terminals.get_mut(idx) {
            if let Ok(mut grid) = tab.grid.try_lock() {
                grid.resize(rows as usize, cols as usize);
            }
        }
        let Some(pty_id) = self
            .terminals
            .get(idx)
            .and_then(|tab| tab.state.pty_id.clone())
        else {
            return;
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |_view, _cx| {
            if let Some(client) = lock_sdk_client(&client_slot).await {
                let _ = client.resize_pty(&pty_id, rows, cols).await;
            }
        })
        .detach();
    }

    pub(crate) fn start_session_title_edit(&mut self, cx: &mut Context<Self>) {
        if self.session_id.is_none() {
            return;
        }
        let title = self.current_session_title();
        self.session_title_input.update(cx, |input, cx| {
            input.set_text(title, cx);
        });
        self.session_title_editing = true;
        cx.notify();
    }

    pub(crate) fn save_session_title(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let title = self.session_title_input.read(cx).text();
        if title.is_empty() {
            self.session_title_editing = false;
            cx.notify();
            return;
        }
        self.session_first_prompts.insert(session_id, title.clone());
        self.update_session_title_in_nav(session_id, title.clone());
        self.session_title_editing = false;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            if let Some(client) = lock_sdk_client(&client_slot).await {
                let _ = client
                    .update_session(session_id, Some(title.as_str()))
                    .await;
            }
            let _ = view.update(cx, |this, cx| {
                this.refresh_project_nav_from_client(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn cancel_session_title_edit(&mut self, cx: &mut Context<Self>) {
        self.session_title_editing = false;
        cx.notify();
    }

    pub(crate) fn refresh_mcp_catalog(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let mcp = if let Some(client) = lock_sdk_client(&client_slot).await {
                client.list_mcp().await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let _ = view.update(cx, |this, cx| {
                this.settings_catalog.mcp = mcp;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn refresh_mcp_servers(&mut self, cx: &mut Context<Self>) {
        self.refresh_mcp_catalog(cx);
    }

    fn load_permission_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir.clone() else {
            return;
        };
        if let Ok(cfg) = deepseekcode_config::load_effective_config(camino::Utf8Path::new(
            &dir.to_string_lossy(),
        )) {
            let policy_text = deepseekcode_config::policy_to_lines(&cfg.policy);
            self.policy_rule_rows = deepseekcode_config::ruleset_from_config(&cfg.policy)
                .into_iter()
                .map(|rule| PolicyRuleRow {
                    scope: rule.scope,
                    pattern: rule.pattern,
                    action: rule.action,
                })
                .collect();
            self.policy_auto_mode = cfg.policy_mode == Some(PolicyMode::Auto);
            self.selected_policy_row = None;
            self.permission_allow_input.update(cx, |input, cx| {
                input.set_text(policy_text, cx);
            });
            self.permission_deny_input.update(cx, |input, cx| {
                input.set_placeholder("Pattern for selected rule…", cx);
                input.set_text(String::new(), cx);
            });
        }
    }

    fn sync_policy_text_from_rows(&mut self, cx: &mut Context<Self>) {
        let text = self
            .policy_rule_rows
            .iter()
            .map(|row| {
                format!(
                    "{} | {} | {}",
                    row.scope,
                    row.pattern,
                    match row.action {
                        PolicyAction::Permit => "permit",
                        PolicyAction::Prompt => "prompt",
                        PolicyAction::Block => "block",
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.permission_allow_input.update(cx, |input, cx| {
            input.set_text(text, cx);
        });
    }

    pub(crate) fn set_policy_auto_mode(&mut self, auto: bool, cx: &mut Context<Self>) {
        self.policy_auto_mode = auto;
        cx.notify();
    }

    pub(crate) fn add_policy_rule_row(&mut self, cx: &mut Context<Self>) {
        self.policy_rule_rows.push(PolicyRuleRow {
            scope: "bash".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        });
        self.sync_policy_text_from_rows(cx);
        cx.notify();
    }

    pub(crate) fn remove_policy_rule_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.policy_rule_rows.len() {
            self.policy_rule_rows.remove(index);
            if self.selected_policy_row == Some(index) {
                self.selected_policy_row = None;
            }
            self.sync_policy_text_from_rows(cx);
            cx.notify();
        }
    }

    pub(crate) fn cycle_policy_rule_scope(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.policy_rule_rows.get_mut(index) else {
            return;
        };
        let current = POLICY_SCOPES
            .iter()
            .position(|s| *s == row.scope)
            .unwrap_or(0);
        let next = (current + 1) % POLICY_SCOPES.len();
        row.scope = POLICY_SCOPES[next].to_string();
        self.sync_policy_text_from_rows(cx);
        cx.notify();
    }

    pub(crate) fn cycle_policy_rule_action(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.policy_rule_rows.get_mut(index) else {
            return;
        };
        row.action = match row.action {
            PolicyAction::Permit => PolicyAction::Prompt,
            PolicyAction::Prompt => PolicyAction::Block,
            PolicyAction::Block => PolicyAction::Permit,
        };
        self.sync_policy_text_from_rows(cx);
        cx.notify();
    }

    pub(crate) fn select_policy_rule_row(
        &mut self,
        index: usize,
        pattern: String,
        cx: &mut Context<Self>,
    ) {
        self.selected_policy_row = Some(index);
        self.permission_deny_input.update(cx, |input, cx| {
            input.set_text(pattern, cx);
        });
        cx.notify();
    }

    fn apply_selected_policy_row_pattern(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected_policy_row else {
            return;
        };
        let pattern = self.permission_deny_input.read(cx).text();
        if let Some(row) = self.policy_rule_rows.get_mut(index) {
            row.pattern = pattern;
            self.sync_policy_text_from_rows(cx);
        }
    }

    pub(crate) fn apply_policy_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        let text = match preset {
            "dev" => {
                "bash | * | prompt\noutside_workspace | * | prompt\nloop_guard | * | prompt\n"
            }
            "readonly" => {
                "edit | * | block\nbash | * | block\nwrite | * | block\napply_patch | * | block\n"
            }
            "strict" => {
                "bash | * | block\nedit | * | block\nwrite | * | block\noutside_workspace | * | block\n"
            }
            _ => return,
        };
        self.policy_rule_rows = deepseekcode_config::lines_to_policy(text)
            .into_iter()
            .flat_map(|(scope, cfg)| {
                use deepseekcode_config::PolicyScopeConfig;
                match cfg {
                    PolicyScopeConfig::Simple(action) => vec![PolicyRuleRow {
                        scope,
                        pattern: "*".into(),
                        action,
                    }],
                    PolicyScopeConfig::Patterns(map) => map
                        .into_iter()
                        .map(|(pattern, action)| PolicyRuleRow {
                            scope: scope.clone(),
                            pattern,
                            action,
                        })
                        .collect(),
                }
            })
            .collect();
        self.permission_allow_input.update(cx, |input, cx| {
            input.set_text(text, cx);
        });
        cx.notify();
    }

    pub(crate) fn save_policy_rules(&mut self, cx: &mut Context<Self>) {
        if self.project_dir.is_none() {
            return;
        }
        self.apply_selected_policy_row_pattern(cx);
        let policy_text = self.permission_allow_input.read(cx).text();
        let policy = deepseekcode_config::lines_to_policy(&policy_text);
        let policy_mode = if self.policy_auto_mode {
            Some(PolicyMode::Auto)
        } else {
            Some(PolicyMode::Interactive)
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            if let Some(client) = lock_sdk_client(&client_slot).await {
                if let Err(e) = client.update_policy_settings(&policy, policy_mode).await {
                    let _ = view.update(cx, |this, cx| {
                        this.composer_error = Some(format!("Failed to save policy: {e}"));
                        cx.notify();
                    });
                    return;
                }
            }
            let _ = view.update(cx, |this, cx| {
                this.load_settings_catalog(cx);
                this.load_permission_inputs(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn delete_policy_grant(&mut self, grant_id: String, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            if let Some(client) = lock_sdk_client(&client_slot).await {
                let _ = client.delete_policy_grant(&grant_id).await;
            }
            let _ = view.update(cx, |this, cx| {
                this.load_settings_catalog(cx);
                cx.notify();
            });
        })
        .detach();
    }

    #[allow(dead_code)]
    pub(crate) fn save_permission_rules(&mut self, cx: &mut Context<Self>) {
        self.save_policy_rules(cx);
    }

    fn merged_sidebar_recents(&self, _cx: &App) -> Vec<RecentProject> {
        let mut paths = HashSet::new();
        let mut merged = Vec::new();
        for nav in &self.project_nav {
            let key = project_path_key(&nav.path);
            if paths.insert(key) {
                merged.push(RecentProject {
                    path: canonical_project_path(&nav.path),
                    last_opened: String::new(),
                });
            }
        }
        for recent in self.home_view.read(_cx).recent_projects() {
            let key = project_path_key(&recent.path);
            if paths.insert(key) {
                merged.push(RecentProject {
                    path: canonical_project_path(&recent.path),
                    last_opened: recent.last_opened.clone(),
                });
            }
        }
        merged
    }

    fn filter_project_nav(&self) -> Vec<ProjectNavItem> {
        dedupe_project_nav(self.project_nav.clone())
    }

    pub(crate) fn confirm_api_key(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.api_key_dialog.clone() else {
            return;
        };
        let provider_id = dialog.provider_id.clone();
        let provider_name = dialog.provider_name.clone();
        let api_key = self.api_key_input.read(cx).text();
        let key = if api_key.is_empty() {
            None
        } else {
            Some(api_key)
        };
        let base_url_text = self.base_url_input.read(cx).text();
        let base_url = if base_url_text.is_empty() {
            None
        } else {
            Some(base_url_text)
        };
        self.api_key_dialog = None;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                match client.connect_provider(&provider_id, key, base_url).await {
                    Ok(()) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::DESKTOP,
                            provider_id = %provider_id,
                            "provider connected from desktop"
                        );
                        let providers = client.list_providers().await.unwrap_or_default();
                        let _ = view.update(cx, |this, cx| {
                            this.providers = providers.clone();
                            this.init_defaults_from_providers(&providers);
                            this.api_key_error = None;
                            this.sync_session_model(cx);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        let _ = view.update(cx, |this, cx| {
                            this.api_key_dialog = Some(ApiKeyDialogState {
                                provider_id,
                                provider_name,
                            });
                            this.api_key_error = Some(e.to_string());
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }

    pub(crate) fn cancel_api_key_dialog(&mut self, cx: &mut Context<Self>) {
        self.api_key_dialog = None;
        cx.notify();
    }

    pub(crate) fn cycle_agent(&mut self, cx: &mut Context<Self>) {
        self.selected_agent = match self.selected_agent.as_ref() {
            "build" => "plan".into(),
            "plan" => "general".into(),
            _ => "build".into(),
        };
        self.persist_preferences();
        self.update_session_model(cx);
    }

    pub(crate) fn cycle_model(&mut self, cx: &mut Context<Self>) {
        let models: Vec<_> = self
            .providers
            .iter()
            .filter(|p| {
                self.selected_provider
                    .as_ref()
                    .map(|id| &p.id == id)
                    .unwrap_or(true)
            })
            .flat_map(|p| p.models.iter())
            .collect();
        if models.is_empty() {
            return;
        }
        let current = self.selected_model.to_string();
        let next_idx = models
            .iter()
            .position(|m| m.name == current || m.id == current)
            .map(|i| (i + 1) % models.len())
            .unwrap_or(0);
        let model = &models[next_idx];
        self.selected_model = model.name.clone().into();
        self.selected_provider = Some(model.provider_id.clone());
        self.persist_preferences();
        self.update_session_model(cx);
    }

    fn update_session_model(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            cx.notify();
            return;
        };
        self.normalize_model_selection();
        self.persist_preferences();
        let agent = self.selected_agent.to_string();
        let (model_id, provider_id) = self.resolve_selected_model();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                let _ = client
                    .update_model(session_id, Some(agent.as_str()), &model_id, &provider_id)
                    .await;
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    fn current_session_model(&self) -> (AgentKind, String, String) {
        let (model_id, provider_id) = self.resolve_selected_model();
        (
            match self.selected_agent.as_ref() {
                "plan" => AgentKind::Plan,
                "general" => AgentKind::General,
                _ => AgentKind::Build,
            },
            model_id,
            provider_id,
        )
    }

    fn provider_label(&self) -> String {
        let (_, provider_id) = self.resolve_selected_model();
        self.providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .map(|provider| provider.name.clone())
            .unwrap_or(provider_id)
    }

    pub(crate) fn open_new_window(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = self.project_dir.clone() {
            let window_id = self.window_manager.open(dir.clone());
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                window_id = window_id,
                path = %dir.display(),
                windows = ?self.window_manager.window_summaries(),
                "spawning additional workspace window"
            );
            WindowManager::spawn_window(dir, cx);
        }
    }

    pub(crate) fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        let run_id = deepseekcode_telemetry::run_context().map(|ctx| ctx.run_id.clone());
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |_view, _cx| {
            let default_name = format!(
                "{DIAGNOSTICS_ARCHIVE_PREFIX}-{}.zip",
                run_id.as_deref().unwrap_or("latest")
            );
            let dest = rfd::FileDialog::new()
                .set_file_name(&default_name)
                .save_file();
            let Some(path) = dest else {
                return;
            };
            let client = lock_sdk_client(&client_slot).await;
            let exported = if let Some(client) = client {
                match client.export_diagnostics(run_id.as_deref()).await {
                    Ok(bytes) => std::fs::write(&path, bytes).map(|_| path.clone()).ok(),
                    Err(_) => {
                        deepseekcode_telemetry::export_diagnostic_bundle(run_id.as_deref(), &path)
                            .ok()
                    }
                }
            } else {
                deepseekcode_telemetry::export_diagnostic_bundle(run_id.as_deref(), &path).ok()
            };
            if let Some(exported) = exported {
                tracing::info!(
                    target: deepseekcode_telemetry::DESKTOP,
                    path = %exported.display(),
                    run_id = run_id.as_deref().unwrap_or("current"),
                    "diagnostics exported"
                );
            }
        })
        .detach();
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.project_dir.is_some();
        self.refresh_terminal_output();
        self.update_session_follow_tail();
        if self.pending_scroll_to_tail {
            self.pending_scroll_to_tail = false;
            self.session_scroll.scroll_to_bottom();
        } else if let Some(leaf_id) = self.pending_scroll_to_activity_leaf.take() {
            if let Some(row) = activity_visible_row_index(
                &self.activity.activity_tree,
                &leaf_id,
                &self.activity.expanded_activity_groups,
                &self.activity.collapsed_activity_groups,
                &self.activity.expanded_activity_leaves,
            ) {
                let y = gpui::px(row as f32 * ACTIVITY_ROW_HEIGHT_PX);
                self.session_scroll
                    .set_offset(gpui::point(gpui::px(0.), -y));
            }
        }
        if has_project && self.layout.right_panel_tab == RightPanelTab::Terminal {
            if self.terminals.get(self.active_terminal).is_some_and(|tab| {
                tab.bash_detail_key.is_none() && !tab.state.connected && !tab.state.pty_connecting
            }) {
                self.connect_terminal_tab(self.active_terminal, cx);
            }
            self.sync_terminal_pty_size(cx);
        }

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg_base)
            .text_color(theme.text_primary)
            .font_family(crate::theme::font_ui(&theme))
            .text_xs()
            .line_height(crate::theme::leading_xs(&theme))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PromptSubmit, window, cx| {
                if this
                    .prompt_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    this.submit_prompt(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SessionTitleSave, window, cx| {
                if this.session_title_editing
                    && this
                        .session_title_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                {
                    this.save_session_title(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SessionTitleCancel, window, cx| {
                if this.session_title_editing
                    && this
                        .session_title_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                {
                    this.cancel_session_title_edit(cx);
                }
            }))
            .child(TitlebarView::render(window, cx, self.health_ok))
            .child(
                div()
                    .id("workspace-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(self.render_shell(cx)),
            )
            .children(self.render_question_overlay(cx))
            .child(SettingsView::render_overlay(
                cx,
                self.settings_open,
                self.settings_section,
                self.theme,
                &self.settings_scroll,
                &self.providers,
                self.selected_agent.as_ref(),
                self.selected_model.as_ref(),
                self.selected_provider.as_deref(),
                &self.settings_catalog,
                self.prompt_delivery,
                self.export_sanitize,
                self.server_url_input.clone(),
                self.server_password_input.clone(),
                self.worktree_branch_input.clone(),
                self.permission_allow_input.clone(),
                self.permission_deny_input.clone(),
                &self.policy_rule_rows,
                self.policy_auto_mode,
                self.selected_policy_row,
            ))
            .children(self.render_api_key_dialog(cx))
            .child(RevertDock::render_overlay(
                cx,
                &self.revert_dock,
                self.selected_message_id,
            ))
            .child(self.render_popover_overlay(cx))
    }
}

impl WorkspaceView {
    fn render_shell(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.project_dir.is_some();
        let recent = self.merged_sidebar_recents(cx);
        let project_nav = self.filter_project_nav();
        let project_path = self.project_dir.as_ref().map(|p| p.display().to_string());

        div()
            .id("workspace-shell")
            .size_full()
            .flex()
            .overflow_hidden()
            .child(SidebarView::render(
                cx,
                self.layout.sidebar_collapsed,
                self.layout.sidebar_width,
                has_project,
                project_path.as_deref(),
                &project_nav,
                &recent,
                self.session_id,
                &self.expanded_projects,
                &self.session_nav_visible,
                &self.project_sessions_loading,
                &self.sidebar_scroll,
            ))
            .child(crate::panel_resize::render_resize_handle(
                cx,
                PanelResizeEdge::Sidebar,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(if has_project {
                        self.render_workspace(cx).into_any_element()
                    } else {
                        self.render_welcome_center(cx).into_any_element()
                    }),
            )
            .children(if has_project && self.layout.show_right_panel {
                vec![
                    crate::panel_resize::render_resize_handle(cx, PanelResizeEdge::RightPanel)
                        .into_any_element(),
                    RightPanelView::render(
                        cx,
                        self.layout.right_panel_width,
                        self.layout.right_panel_tab,
                        project_path.as_deref().unwrap_or(""),
                        &self.file_tree,
                        &self.editor,
                        self.file_editor_input.clone(),
                        &self.editor_tabs,
                        self.active_editor_tab,
                        self.editor_read_only,
                        &self.terminals,
                        self.active_terminal,
                        &self.terminal_focus,
                        &self.terminal_scroll,
                        "",
                        true,
                        &self.git_staged,
                        &self.git_unstaged,
                        &self.git_untracked,
                        self.git_diff_summary.as_deref(),
                        self.git_status_message.as_deref(),
                        &self.git_log,
                        self.selected_detail_key
                            .as_deref()
                            .and_then(|k| self.activity.detail_store.get(k)),
                    )
                    .into_any_element(),
                ]
            } else {
                vec![]
            })
    }

    fn render_welcome_center(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        div()
            .id("welcome-center")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(theme.bg_base)
            .child(
                div()
                    .w(px(480.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child("DeepSeekCode"),
                    )
                    .child(div().text_sm().text_color(theme.text_muted).child(
                        "Open a project from the sidebar or create AGENTS.md after opening.",
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .child("Rust-native AI programming environment"),
                    ),
            )
    }

    fn render_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let project_path = self.project_dir.as_ref().map(|p| p.display().to_string());

        div()
            .id("workspace-center-column")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(SessionHeaderView::render(
                cx,
                true,
                self.session_id.is_some(),
                project_path.as_deref(),
                &self.current_session_title(),
                self.session_title_editing,
                self.session_title_input.clone(),
                self.session_busy,
                self.session_status.as_ref(),
                self.session_plans.len(),
                self.plan_picker_expanded,
            ))
            .child(
                div()
                    .id("session-timeline-host")
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child({
                        let timeline =
                            crate::session_timeline_view::SessionTimelineView::from_store(
                                &self.activity.activity_tree,
                                &self.activity.expanded_activity_groups,
                                &self.activity.collapsed_activity_groups,
                                &self.activity.expanded_activity_leaves,
                                self.activity.timeline_render_generation,
                            );
                        timeline.render(
                            cx,
                            self.highlighted_todo_leaf_id.as_deref(),
                            &self.session_scroll,
                            self.session_status.as_ref() == "Loading session…",
                            !self.activity.session_events.is_empty(),
                        )
                    }),
            )
            .children(if self.session_todos.is_empty() {
                Vec::new()
            } else {
                vec![div()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(crate::todo_panel::TodoPanel::render_composer_strip(
                        cx,
                        &self.session_todos,
                        self.composer_todo_expanded,
                        self.highlighted_todo_index,
                    ))
                    .into_any_element()]
            })
            .children(if let Some(prompt) = self.pending_policies.front() {
                vec![div()
                    .flex_shrink_0()
                    .px_4()
                    .pb_2()
                    .child(PermissionDock::render(
                        cx,
                        prompt,
                        self.policy_reject_expanded,
                        self.policy_patterns_expanded,
                        self.permission_deny_input.clone(),
                    ))
                    .into_any_element()]
            } else {
                Vec::new()
            })
            .children(if let Some(prompt) = self.plan_implement_prompt() {
                vec![div()
                    .flex_shrink_0()
                    .px_4()
                    .pb_2()
                    .child(PlanImplementDock::render(cx, &prompt))
                    .into_any_element()]
            } else {
                Vec::new()
            })
            .children(
                PlanSessionStrip::render(
                    cx,
                    &self.session_plans,
                    self.plan_picker_expanded,
                    self.selected_agent.as_ref() == "plan",
                )
                .into_iter()
                .map(|el| {
                    div()
                        .flex_shrink_0()
                        .px_4()
                        .pb_2()
                        .child(el)
                        .into_any_element()
                })
                .collect::<Vec<_>>(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(ComposerView::render(
                        cx,
                        &self.prompt_input,
                        self.session_busy,
                        self.composer_blocked(),
                        self.selected_agent.as_ref(),
                        self.selected_model.as_ref(),
                        &self.attachments,
                        self.composer_dropdown.as_deref().map(|s| s.as_ref()),
                        self.composer_error.as_deref(),
                        self.session_id.is_some(),
                        self.health_ok,
                        &self.context_usage,
                    )),
            )
    }

    fn render_popover_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui::dropdown::DropDirection;
        use crate::ui::popover::{
            render_dismiss_layer, render_menu_panel, render_project_remove_row,
            render_session_delete_row,
        };

        let Some(anchor) = self.popover_anchor else {
            return div().into_any_element();
        };

        let mut layer: AnyElement = div()
            .id("popover-overlay")
            .absolute()
            .inset_0()
            .child(render_dismiss_layer(cx))
            .into_any_element();

        if let Some(session_id) = self.session_context_menu {
            let menu = render_session_delete_row(session_id, anchor, cx);
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        } else if let Some(path) = self.project_context_menu.clone() {
            let menu = render_project_remove_row(path, anchor, cx);
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        } else if self.session_menu_open {
            let menu = crate::session_header::render_session_menu_panel(
                cx,
                anchor,
                self.selected_message_id.is_some(),
            );
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        } else if let Some(kind) = self.composer_dropdown.as_deref() {
            let options = match kind {
                "agent" => crate::composer::agent_dropdown_options(),
                "model" => crate::composer::model_dropdown_options(&self.providers),
                _ => Vec::new(),
            };
            let selected = crate::composer::composer_dropdown_selected_id(
                kind,
                self.selected_agent.as_ref(),
                self.selected_model.as_ref(),
                &self.providers,
            );
            let kind_owned = kind.to_string();
            let menu = render_menu_panel(
                &options,
                &selected,
                anchor,
                DropDirection::Up,
                move |this, id, cx| {
                    this.select_composer_dropdown(&kind_owned, id, cx);
                },
                cx,
            );
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        }

        layer
    }

    fn render_question_overlay(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(prompt) = &self.pending_question else {
            return vec![];
        };
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let question = prompt.question.clone();
        let options = prompt.options.clone();
        let mut panel = div()
            .w(px(480.))
            .p_4()
            .rounded_lg()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.info)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child("Question"),
            )
            .child(
                div()
                    .text_xs()
                    .mt_2()
                    .text_color(theme.text_secondary)
                    .child(question),
            );
        if options.is_empty() {
            panel = panel
                .child(div().mt_3().child(self.question_input.clone()))
                .child(
                    div().flex().gap_2().mt_3().justify_end().child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.info)
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    let answer = this.question_input.read(cx).text();
                                    if !answer.trim().is_empty() {
                                        this.submit_question_answer(answer, cx);
                                    }
                                }),
                            )
                            .child("Submit"),
                    ),
                );
        } else {
            let allow_custom = prompt.allow_custom;
            let custom_mode = prompt.custom_mode;
            let mut opts = div().flex().flex_col().gap_2().mt_3();
            for opt in options {
                opts = opts.child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(theme.info)
                        .text_xs()
                        .text_color(theme.text_on_accent)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, {
                            let opt = opt.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.submit_question_answer(opt.clone(), cx);
                            })
                        })
                        .child(opt),
                );
            }
            if allow_custom {
                opts = opts.child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(if custom_mode {
                            theme.bg_selected
                        } else {
                            theme.bg_base
                        })
                        .border_1()
                        .border_color(theme.border_focus)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.enable_question_custom_mode(cx);
                            }),
                        )
                        .child("Other (custom input)"),
                );
                panel = panel.child(opts).child(
                    div().mt_3().child(self.question_input.clone()).child(
                        div().flex().gap_2().mt_3().justify_end().child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(theme.info)
                                .text_xs()
                                .text_color(theme.text_on_accent)
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let answer = this.question_input.read(cx).text();
                                        if !answer.trim().is_empty() {
                                            this.submit_question_answer(answer, cx);
                                        }
                                    }),
                                )
                                .child("Submit"),
                        ),
                    ),
                );
            } else {
                panel = panel.child(opts);
            }
        }
        vec![div()
            .id("question-dialog")
            .absolute()
            .inset_0()
            .bg(theme.bg_overlay)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
            .into_any_element()]
    }

    fn render_api_key_dialog(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(dialog) = &self.api_key_dialog else {
            return vec![];
        };
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        vec![div()
            .id("api-key-dialog")
            .absolute()
            .inset_0()
            .bg(theme.bg_overlay)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(400.))
                    .p_4()
                    .rounded_lg()
                    .bg(theme.bg_elevated)
                    .border_1()
                    .border_color(theme.border_default)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(format!("Connect {}", dialog.provider_name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .mt_2()
                            .text_color(theme.text_muted)
                            .child(
                                "Keys are stored in your OS keychain (encrypted local fallback). Leave blank to keep the existing key.",
                            ),
                    )
                    .child(
                        div()
                            .mt_3()
                            .h(px(36.))
                            .child(self.api_key_input.clone()),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .child("Base URL (optional)"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .h(px(36.))
                            .child(self.base_url_input.clone()),
                    )
                    .children(if let Some(err) = &self.api_key_error {
                        vec![div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme.error)
                            .child(err.clone())
                            .into_any_element()]
                    } else {
                        vec![]
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .mt_4()
                            .justify_end()
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(theme.bg_hover)
                                    .text_xs()
                                    .text_color(theme.text_primary)
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.cancel_api_key_dialog(cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(theme.accent_rgba())
                                    .text_xs()
                                    .text_color(theme.text_on_accent)
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.confirm_api_key(cx);
                                    }))
                                    .child("Connect"),
                            ),
                    ),
            )
            .into_any_element()]
    }

    fn rebuild_activity_tree(&mut self, reason: &'static str) {
        self.activity.rebuild_activity_tree(reason);
    }

    fn refresh_child_session_projection(&mut self, child_id: SessionId, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        let child_key = child_id.to_string();
        cx.spawn(async move |view, cx| {
            let projection = if let Some(client) = lock_sdk_client(&client_slot).await {
                let mut result = None;
                for attempt in 0..2 {
                    let events = client
                        .list_session_events(child_id, 0)
                        .await
                        .unwrap_or_default();
                    let messages = client.list_messages(child_id).await.unwrap_or_default();
                    if !events.is_empty() || attempt == 1 {
                        result =
                            Some(deepseekcode_sdk::ChildSessionProjection { events, messages });
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                result
            } else {
                None
            };
            if let Some(projection) = projection {
                let _ = view.update(cx, |this, cx| {
                    this.activity
                        .child_session_projections
                        .insert(child_key, projection);
                    this.rebuild_activity_tree("child_projection_single");
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn open_file_at_line(
        &mut self,
        path: &str,
        line: Option<u32>,
        diff_stats: Option<deepseekcode_schema::activity::DiffStats>,
        cx: &mut Context<Self>,
    ) {
        let path = path.to_string();
        let snapshot_id = self
            .revert_staged_files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.snapshot_id.clone());
        let client_slot = Arc::clone(&self.client);
        let line = line;
        let diff_stats = diff_stats;
        cx.spawn(async move |view, cx| {
            let result = if let Some(client) = lock_sdk_client(&client_slot).await {
                let content = client.fs_file(&path).await.ok();
                let diff = match client.fs_diff(&path, None, snapshot_id.as_deref()).await {
                    Ok(hunks) if !hunks.is_empty() => Some(hunks),
                    _ => content
                        .as_ref()
                        .map(|c| crate::editor::compute_local_diff("", c)),
                };
                Some((content, diff))
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                if let Some((content, diff)) = result {
                    let text = content.unwrap_or_default();
                    this.editor.path = Some(path.clone());
                    this.editor.set_content(text.clone());
                    this.editor.diff_hunks = diff;
                    this.editor.diff_stats = diff_stats;
                    this.editor.scroll_to_line = line;
                    this.editor_read_only = line.is_some();
                    let file_input = this.file_editor_input.clone();
                    let editor_text = text.clone();
                    cx.defer(move |cx| {
                        file_input.update(cx, |input, cx| {
                            input.set_text(editor_text, cx);
                        });
                    });
                    if !this.editor_tabs.iter().any(|p| p == &path) {
                        this.editor_tabs.push(path.clone());
                    }
                    this.active_editor_tab = this
                        .editor_tabs
                        .iter()
                        .position(|p| p == &path)
                        .unwrap_or(0);
                    this.layout.right_panel_tab = RightPanelTab::Editor;
                    this.layout.show_right_panel = true;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_activity_group(
        &mut self,
        id: &str,
        collapsed_default: bool,
        cx: &mut Context<Self>,
    ) {
        toggle_activity_group_expansion(
            id,
            collapsed_default,
            &mut self.activity.expanded_activity_groups,
            &mut self.activity.collapsed_activity_groups,
        );
        cx.notify();
    }

    pub(crate) fn toggle_activity_leaf(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.activity.expanded_activity_leaves.contains(id) {
            self.activity.expanded_activity_leaves.remove(id);
        } else {
            self.activity
                .expanded_activity_leaves
                .insert(id.to_string());
        }
        cx.notify();
    }

    pub(crate) fn open_git_changed_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.layout.right_panel_tab = RightPanelTab::Editor;
        self.layout.show_right_panel = true;
        let path = path.to_string();
        let project_dir = self.project_dir.clone();
        cx.spawn(async move |view, cx| {
            let result = project_dir.map(|dir| {
                let dir_str = dir.to_string_lossy().to_string();
                let content = git_worktree_file_content(&dir_str, &path).unwrap_or_default();
                let diff = git_unified_diff(&dir_str, &path);
                (content, diff)
            });
            let _ = view.update(cx, |this, cx| {
                if let Some((content, diff)) = result {
                    this.editor.path = Some(path.clone());
                    this.editor.set_content(content.clone());
                    this.editor.diff_hunks = diff.filter(|d| !d.is_empty());
                    this.editor.diff_stats = None;
                    this.editor.scroll_to_line = None;
                    this.editor_read_only = false;
                    let file_input = this.file_editor_input.clone();
                    let editor_text = content.clone();
                    cx.defer(move |cx| {
                        file_input.update(cx, |input, cx| {
                            input.set_text(editor_text, cx);
                        });
                    });
                    if !this.editor_tabs.iter().any(|p| p == &path) {
                        this.editor_tabs.push(path.clone());
                    }
                    this.active_editor_tab = this
                        .editor_tabs
                        .iter()
                        .position(|p| p == &path)
                        .unwrap_or(0);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn refresh_git_status_local(&mut self) {
        let Some(dir) = self.project_dir.clone() else {
            self.clear_git_status();
            return;
        };
        self.apply_git_status(collect_git_status(&dir));
    }

    pub(crate) fn refresh_git_status_async(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir.clone() else {
            self.clear_git_status();
            return;
        };
        cx.spawn(async move |view, cx| {
            let status = tokio::task::spawn_blocking(move || collect_git_status(&dir))
                .await
                .ok();
            if let Some(status) = status {
                let _ = view.update(cx, |this, cx| {
                    this.apply_git_status(status);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clear_git_status(&mut self) {
        self.git_staged.clear();
        self.git_unstaged.clear();
        self.git_untracked.clear();
        self.git_diff_summary = None;
        self.git_status_message = None;
        self.git_log.clear();
    }

    fn apply_git_status(&mut self, status: GitStatusSnapshot) {
        self.git_staged = status.staged;
        self.git_unstaged = status.unstaged;
        self.git_untracked = status.untracked;
        self.git_diff_summary = status.diff_summary;
        self.git_status_message = status.status_message;
        self.git_log = status.log;
        self.sync_file_tree_git_badges();
    }

    fn sync_file_tree_git_badges(&mut self) {
        use crate::file_tree::GitFileBadge;
        use std::collections::HashMap;
        let mut badges = HashMap::new();
        for entry in &self.git_untracked {
            badges.insert(entry.path.clone(), GitFileBadge::Untracked);
        }
        for entry in &self.git_unstaged {
            badges.insert(entry.path.clone(), GitFileBadge::from_git_raw(&entry.raw));
        }
        for entry in &self.git_staged {
            badges.insert(entry.path.clone(), GitFileBadge::from_git_raw(&entry.raw));
        }
        self.file_tree.git_badges = badges;
    }

    pub(crate) fn open_external_url(&self, url: &str) {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
    }

    fn refresh_all_child_session_projections(
        &mut self,
        cx: &mut Context<Self>,
        reason: &'static str,
    ) {
        let children: Vec<SessionId> = task_child_session_ids(&self.activity.session_events);
        if children.is_empty() {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                reason,
                "child projection refresh skipped (no child sessions)"
            );
            return;
        }
        let generation = self
            .child_projection_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            reason,
            child_count = children.len(),
            generation,
            "child projection refresh scheduled"
        );
        let client_slot = Arc::clone(&self.client);
        let generation_slot = Arc::clone(&self.child_projection_generation);
        cx.spawn(async move |view, cx| {
            let mut projections = HashMap::new();
            if let Some(client) = lock_sdk_client(&client_slot).await {
                for child_id in children {
                    for attempt in 0..2 {
                        let events = client
                            .list_session_events(child_id, 0)
                            .await
                            .unwrap_or_default();
                        let messages = client.list_messages(child_id).await.unwrap_or_default();
                        if !events.is_empty() || attempt == 1 {
                            projections.insert(
                                child_id.to_string(),
                                deepseekcode_sdk::ChildSessionProjection { events, messages },
                            );
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    }
                }
            }
            if projections.is_empty() {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    reason,
                    "child projection refresh returned no data"
                );
                return;
            }
            let _ = view.update(cx, |this, cx| {
                if generation_slot.load(Ordering::SeqCst) != generation {
                    tracing::debug!(
                        target: deepseekcode_telemetry::DESKTOP,
                        reason,
                        generation,
                        "child projection refresh superseded"
                    );
                    return;
                }
                this.activity.child_session_projections.extend(projections);
                this.rebuild_activity_tree("child_projection");
                cx.notify();
            });
        })
        .detach();
    }
}

fn format_tool_input(tool: &str, input: &serde_json::Value) -> String {
    let input = &deepseekcode_types::normalize_tool_arguments(input);
    match tool {
        "bash" => input["command"]
            .as_str()
            .unwrap_or("(no command)")
            .to_string(),
        "read" | "write" | "edit" => {
            let path = input["path"].as_str().unwrap_or("(no path)");
            if tool == "read" {
                format!("path: {path}")
            } else {
                format!("path: {path}\n{}", truncate_text(&input.to_string(), 800))
            }
        }
        "grep" => format!(
            "pattern: {}\npath: {}",
            input["pattern"].as_str().unwrap_or(""),
            input["path"].as_str().unwrap_or(".")
        ),
        "glob" => {
            let pattern = input["pattern"]
                .as_str()
                .or_else(|| input["glob_pattern"].as_str())
                .unwrap_or("*");
            format!("pattern: {pattern}")
        }
        _ => serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string()),
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}\n… ({max_chars}+ chars)")
}

fn task_session_id_from_payload(payload: &serde_json::Value) -> Option<SessionId> {
    let output = payload.get("output")?;
    let stored = output.as_object()?;
    let session = stored
        .get("output")
        .and_then(|v| v.get("session_id"))
        .or_else(|| stored.get("session_id"))
        .and_then(|v| v.as_str())?;
    SessionId::from_str(session).ok()
}

fn task_child_session_ids(events: &[EventEnvelope]) -> Vec<SessionId> {
    events
        .iter()
        .filter(|e| e.kind == EventKind::ToolCompleted)
        .filter(|e| e.payload.get("tool_name").and_then(|v| v.as_str()) == Some("task"))
        .filter_map(|e| task_session_id_from_payload(&e.payload))
        .collect()
}

pub fn canonical_project_path(path: impl AsRef<std::path::Path>) -> String {
    let path = path.as_ref();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_windows_extended_path(&canonical.display().to_string())
}

fn strip_windows_extended_path(path: &str) -> String {
    #[cfg(windows)]
    {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

pub fn project_path_key(path: &str) -> String {
    let trimmed = strip_windows_extended_path(path.trim());
    let trimmed = trimmed.trim_end_matches(['\\', '/']);
    #[cfg(windows)]
    {
        trimmed.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        trimmed.to_string()
    }
}

pub fn project_paths_equal(a: &str, b: &str) -> bool {
    project_path_key(a) == project_path_key(b)
}

fn dedupe_project_nav(items: Vec<ProjectNavItem>) -> Vec<ProjectNavItem> {
    let mut merged: HashMap<String, ProjectNavItem> = HashMap::new();
    for item in items {
        let key = project_path_key(&item.path);
        let path = canonical_project_path(&item.path);
        merged
            .entry(key)
            .and_modify(|existing| {
                existing.path = path.clone();
                for session in &item.sessions {
                    if !existing.sessions.iter().any(|s| s.id == session.id) {
                        existing.sessions.push(session.clone());
                    }
                }
                existing
                    .sessions
                    .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            })
            .or_insert(ProjectNavItem {
                path,
                sessions: item.sessions,
            });
    }
    let mut out: Vec<_> = merged.into_values().collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[derive(Clone, Debug, Default)]
struct GitStatusSnapshot {
    staged: Vec<crate::logic::GitStatusEntry>,
    unstaged: Vec<crate::logic::GitStatusEntry>,
    untracked: Vec<crate::logic::GitStatusEntry>,
    diff_summary: Option<String>,
    status_message: Option<String>,
    log: Vec<String>,
}

fn collect_git_status(dir: &std::path::Path) -> GitStatusSnapshot {
    let dir_str = dir.to_string_lossy().to_string();
    let is_repo = std::process::Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false);
    if !is_repo {
        return GitStatusSnapshot {
            status_message: Some("Not a git repository.".into()),
            ..GitStatusSnapshot::default()
        };
    }
    let output = std::process::Command::new("git")
        .args(["-C", &dir_str, "status", "--porcelain"])
        .output();
    let lines: Vec<String> = match &output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(String::from)
            .collect(),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return GitStatusSnapshot {
                status_message: Some(if err.is_empty() {
                    "Git status failed.".into()
                } else {
                    err
                }),
                ..GitStatusSnapshot::default()
            };
        }
        Err(e) => {
            return GitStatusSnapshot {
                status_message: Some(format!("Git unavailable: {e}")),
                ..GitStatusSnapshot::default()
            };
        }
    };
    let (staged, unstaged, untracked) = classify_git_porcelain(&lines);
    let diff_summary = std::process::Command::new("git")
        .args(["-C", &dir_str, "diff", "--shortstat"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let log = std::process::Command::new("git")
        .args(["-C", &dir_str, "log", "--oneline", "--decorate", "-n", "20"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    GitStatusSnapshot {
        staged,
        unstaged,
        untracked,
        diff_summary,
        status_message: None,
        log,
    }
}

fn last_provider_turn_group_id(
    tree: &deepseekcode_schema::activity::ActivityTree,
) -> Option<String> {
    use deepseekcode_schema::activity::{ActivityGroupKind, ActivityNode};
    for node in tree.roots.iter().rev() {
        if let ActivityNode::Group(g) = node {
            if g.kind == ActivityGroupKind::ProviderTurn {
                return Some(g.id.clone());
            }
        }
    }
    None
}

fn keystroke_to_bytes(keystroke: &Keystroke) -> Vec<u8> {
    if let Some(key_char) = &keystroke.key_char {
        return key_char.as_bytes().to_vec();
    }
    match keystroke.key.as_str() {
        "enter" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        _ => Vec::new(),
    }
}
