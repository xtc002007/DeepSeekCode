#[path = "activity_click_router.rs"]
pub(crate) mod activity_click_router;

use deepseekcode_schema::activity::ActivityTree;
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::plan::SessionPlanSummary;
use deepseekcode_schema::policy::{PolicyReply, PolicyReplyKind};
use deepseekcode_schema::provider::{ModelInfo, ProviderInfo};
use deepseekcode_schema::session::{
    CreateSessionRequest, PromptDelivery, PromptRequest, RevertFileEntry,
};
use deepseekcode_schema::todo::TodoItem;
use deepseekcode_sdk::{client::DeepSeekCodeClient, EmbeddedEngine, EventStreamItem};
use deepseekcode_types::{MessageId, SessionId, DIAGNOSTICS_ARCHIVE_PREFIX, SESSION_EXPORT_PREFIX};
use futures::{SinkExt, StreamExt};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::str::FromStr;
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
    classify_git_porcelain, commit_placeholder, estimate_tokens, extract_tool_path,
    find_todo_leaf_id_for_index,
    first_user_message_text, git_unified_diff, git_worktree_file_content,
    implement_prompt_from_plan_path, parse_branch_vv, parse_log_pretty, parse_remotes,
    parse_stash_list, pick_latest_by_timestamp, revert_files_preview, sessions_for_project,
    summarize_permission_request, toggle_activity_group_expansion, token_usage_from_payload,
    truncate_preview, GitBranch, GitCommitNode, GitRemote, GitReviewIssue, GitStashEntry,
    GitStatusEntry, ACTIVITY_ROW_HEIGHT_PX,
};
use crate::multi_window::{engine_client_for, shared_engine, WindowManager};
use crate::panel_resize::PanelResizeEdge;
use crate::permission_dock::{PermissionDock, PolicyPromptState};
use crate::plan_implement_dock::{PlanImplementDock, PlanImplementPromptState};
use crate::plan_session_strip::PlanSessionStrip;
use crate::preferences::{
    load_preferences, save_preferences, AppPreferences, CustomModelEntry, ThemeId,
};
use crate::revert::{RevertAction, RevertDock, RevertDockState};
use crate::right_panel::RightPanelView;
use crate::session_activity::{push_session_event, truncate_session_events_on_compaction};
use crate::session_header::SessionHeaderView;
use crate::settings::{
    PolicyRuleRow, SettingsCatalog, SettingsSection, SettingsView, POLICY_SCOPES,
};
use crate::sidebar::SidebarView;
use crate::terminal::TerminalTab;
use crate::titlebar::TitlebarView;
use deepseekcode_config::PolicyMode;
use deepseekcode_schema::policy::PolicyAction;

actions!(question_dialog, [QuestionCancel]);
actions!(overlay, [CloseOverlay]);

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

/// Destructive git action awaiting explicit user confirmation (P5 safety).
#[derive(Clone, Debug)]
pub struct GitConfirm {
    pub title: String,
    pub detail: String,
    pub action: GitConfirmAction,
}

#[derive(Clone, Debug)]
pub enum GitConfirmAction {
    DiscardAll,
    DiscardPaths(Vec<String>),
    DeleteBranch { name: String, force: bool },
    /// Dropping a stash permanently destroys its diff.
    DropStash { ref_name: String },
    /// Force-with-lease rewrites remote history for everyone tracking it.
    ForcePush,
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
    /// P0-5: in-flight policy reply ids. Prevents double-click / keyboard+mouse
    /// concurrent replies for the same prompt (second RPC would fail and
    /// resurrect the dock via rollback).
    pending_policy_reply: HashSet<String>,
    policy_rule_rows: Vec<PolicyRuleRow>,
    policy_auto_mode: bool,
    /// P1-4: mirrors `ui.show_turn_usage` (default true). Gates the per-turn
    /// cache receipt display only — accounting always runs server-side.
    show_turn_usage: bool,
    selected_policy_row: Option<usize>,
    pending_question: Option<QuestionPromptState>,
    prompt_input: Entity<PromptInput>,
    api_key_input: Entity<PromptInput>,
    base_url_input: Entity<PromptInput>,
    custom_model_input: Entity<PromptInput>,
    server_url_input: Entity<PromptInput>,
    server_password_input: Entity<PromptInput>,
    worktree_branch_input: Entity<PromptInput>,
    selected_agent: SharedString,
    selected_model: SharedString,
    selected_provider: Option<String>,
    /// User-added models (Settings → Custom models), persisted in preferences
    /// and merged into `providers` after every catalog fetch.
    custom_models: Vec<CustomModelEntry>,
    api_key_dialog: Option<ApiKeyDialogState>,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_catalog: SettingsCatalog,
    window_manager: WindowManager,
    file_tree: FileTreeState,
    pub(crate) editor: EditorState,
    editor_tabs: Vec<String>,
    active_editor_tab: usize,
    editor_buffers: HashMap<String, String>,
    editor_dirty: HashSet<String>,
    open_generation: u64,
    editor_read_only: bool,
    file_editor_input: Entity<crate::file_editor_input::FileEditorInput>,
    session_title_input: Entity<PromptInput>,
    session_title_editing: bool,
    revert_staged_files: Vec<RevertFileEntry>,
    composer_error: Option<String>,
    permission_allow_input: Entity<PromptInput>,
    permission_deny_input: Entity<PromptInput>,
    // Dedicated pattern editor for Settings rule rows. Previously
    // `permission_deny_input` doubled as reject-message box + pattern editor,
    // so opening Settings during a pending prompt overwrote the reject text
    // into the rule table (and vice versa).
    policy_pattern_input: Entity<PromptInput>,
    question_input: Entity<PromptInput>,
    panel_resize_edge: Option<PanelResizeEdge>,
    panel_resize_start_x: f32,
    panel_resize_start_width: f32,
    terminal_pty_size: Option<(u16, u16)>,
    terminals: Vec<TerminalTab>,
    active_terminal: usize,
    next_terminal_id: usize,
    /// Session-independent bash transcripts (see `BashTranscript`): terminal
    /// tabs are global while `activity.detail_store` is rebuilt per session,
    /// so tab content is mirrored here or tabs go blank on session switch.
    bash_transcripts: HashMap<String, crate::activity_timeline::BashTranscript>,
    terminal_focus: FocusHandle,
    pub(crate) session_busy: bool,
    /// P0-2: true between Stop click and interrupt RPC completion.
    /// While set the composer send button is disabled (no Stop->Send flip
    /// on rapid double-click).
    pub(crate) stopping: bool,
    session_status: SharedString,
    session_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    terminal_scroll: ScrollHandle,
    pub(crate) session_context_menu: Option<SessionId>,
    pub(crate) project_context_menu: Option<String>,
    popover_anchor: Option<Point<Pixels>>,
    /// P0-6: two-stage delete confirm. First click arms the confirm, second
    /// click executes. Prevents one-click data loss on session/project rows.
    pub(crate) pending_session_delete: Option<SessionId>,
    pub(crate) pending_project_remove: Option<String>,
    /// P0-8: plan Implement overwrite confirm (plan_id awaiting decision) +
    /// debounce timestamp. Prevents one-click draft loss and double-fire.
    pub(crate) pending_plan_confirm: Option<String>,
    pub(crate) last_plan_implement_at: Option<std::time::Instant>,
    /// P2: true while an editor fs_write is in flight (debounces Ctrl+S).
    pub(crate) editor_saving: bool,
    /// P1-1: press-origin for click-confirm. `on_mouse_down` arms the id,
    /// `on_mouse_up` fires only if the same id is still armed — so a
    /// press-drag-release across rows never mis-fires (native click semantics).
    pub(crate) pressed_row: Option<String>,
    /// P1-8: active scrollbar thumb drag (start_y_px, start_offset_y_px).
    pub(crate) scrollbar_drag: Option<(f32, f32)>,
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
    language: deepseekcode_i18n::LanguagePref,
    server_url: Option<String>,
    server_password: Option<String>,
    attachments: Vec<PromptAttachment>,
    image_preview: Option<PromptAttachment>,
    event_stream_generation: Arc<AtomicU64>,
    bootstrap_generation: Arc<AtomicU64>,
    hydrate_generation: Arc<AtomicU64>,
    child_projection_generation: Arc<AtomicU64>,
    last_sse_notify_at: Option<std::time::Instant>,
    session_todos: Vec<TodoItem>,
    pub(crate) git_staged: Vec<GitStatusEntry>,
    pub(crate) git_unstaged: Vec<GitStatusEntry>,
    pub(crate) git_untracked: Vec<GitStatusEntry>,
    pub(crate) git_conflicted: Vec<GitStatusEntry>,
    pub(crate) git_diff_summary: Option<String>,
    pub(crate) git_status_message: Option<String>,
    pub(crate) git_log: Vec<String>,
    // Cursor-parity extended state (P0-P5).
    pub(crate) git_branches: Vec<GitBranch>,
    pub(crate) git_commits: Vec<GitCommitNode>,
    pub(crate) git_stashes: Vec<GitStashEntry>,
    pub(crate) git_remotes: Vec<GitRemote>,
    pub(crate) git_tags: Vec<String>,
    pub(crate) git_current_branch: Option<String>,
    pub(crate) git_head_sha: Option<String>,
    pub(crate) git_ahead: u32,
    pub(crate) git_behind: u32,
    pub(crate) git_busy: bool,
    /// Short verdict line (e.g. "Commit failed"). Full hook output lives in
    /// `git_error_detail` so the panel stays usable after a failure.
    pub(crate) git_error: Option<String>,
    pub(crate) git_error_detail: Option<String>,
    pub(crate) git_error_expanded: bool,
    pub(crate) git_notice: Option<String>,
    pub(crate) git_commit_input: Entity<PromptInput>,
    pub(crate) git_filter_input: Entity<PromptInput>,
    pub(crate) git_branch_input: Entity<PromptInput>,
    pub(crate) git_stash_input: Entity<PromptInput>,
    pub(crate) git_selected: HashSet<String>,
    pub(crate) git_changes_expanded: bool,
    pub(crate) git_staged_expanded: bool,
    pub(crate) git_conflict_expanded: bool,
    pub(crate) git_untracked_expanded: bool,
    pub(crate) git_graph_expanded: bool,
    pub(crate) git_review_expanded: bool,
    pub(crate) git_stash_expanded: bool,
    pub(crate) git_remotes_expanded: bool,
    pub(crate) git_show_tree: bool,
    pub(crate) git_commit_no_verify: bool,
    pub(crate) git_include_untracked_stash: bool,
    pub(crate) git_review_scope: String,
    pub(crate) git_review_scope_input: Entity<PromptInput>,
    pub(crate) git_review_issues: Vec<GitReviewIssue>,
    pub(crate) git_review_busy: bool,
    pub(crate) git_ai_busy: bool,
    pub(crate) git_graph_limit: usize,
    pub(crate) git_hunks: HashMap<String, Vec<String>>,
    pub(crate) git_hunks_expanded: HashSet<String>,
    pub(crate) git_hunks_loading: HashSet<String>,
    pub(crate) git_pending_confirm: Option<GitConfirm>,
    /// Hovered change-row key (`"<group>/<path>"`) for the Cursor-style
    /// floating action overlay. `None` renders compact rows with no overlay.
    pub(crate) git_hovered_row: Option<String>,
    /// Verdict title consumed by the next completing op (commit only):
    /// proves the message box clears on success, never on failure.
    pub(crate) git_pending_commit_ui: Option<String>,
    /// Cursor-style split-button dropdowns (Commit ▾ / Find Issues ▾ /
    /// branch box). Closed by default to match Cursor's minimal look.
    pub(crate) git_commit_menu_open: bool,
    pub(crate) git_review_scope_open: bool,
    pub(crate) git_branch_box_open: bool,
    selected_detail_key: Option<String>,
    highlighted_todo_leaf_id: Option<String>,
    pub(crate) timeline_text_selection: Option<crate::selectable_text::TimelineTextSelection>,
    pub(crate) timeline_text_layouts:
        std::collections::HashMap<String, crate::selectable_text::TextBlockLayout>,
    pub(crate) timeline_text_focus: FocusHandle,
    pub(crate) timeline_text_press: Option<crate::selectable_text::TimelineTextPress>,
    pub(crate) timeline_row_press: Option<crate::selectable_text::TimelineRowPress>,
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
        prompt_input.update(cx, |input, cx| {
            input.set_small_text(true, cx);
        });
        cx.subscribe(&prompt_input, |this, _, event, cx| {
            let PromptInputEvent::PasteImage { bytes, format } = event;
            this.attach_clipboard_image(bytes.clone(), *format, cx);
        })
        .detach();
        let api_key_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("api-key-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.api_key_placeholder"), cx);
            input.set_masked(true, cx);
            input
        });
        let base_url_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("base-url-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.base_url_placeholder"), cx);
            input
        });
        let custom_model_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("custom-model-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.model_example"), cx);
            input
        });
        let server_url_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("server-url-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.server_url_example"), cx);
            input
        });
        let server_password_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("server-password-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.password_optional"), cx);
            input.set_masked(true, cx);
            input
        });
        let worktree_branch_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("worktree-branch-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.branch_optional"), cx);
            input
        });
        let file_editor_input = cx.new(crate::file_editor_input::FileEditorInput::new);
        cx.subscribe(&file_editor_input, |this, _, event, cx| {
            use crate::file_editor_input::FileEditorInputEvent as EditEvent;
            match event {
                EditEvent::Edited => {
                    if let Some(path) = this.editor.path.clone() {
                        let content = this.file_editor_input.read(cx).text();
                        this.editor_buffers.insert(path.clone(), content);
                        this.editor_dirty.insert(path);
                    }
                    cx.notify();
                }
                EditEvent::SaveRequested => {
                    this.save_editor_file(cx);
                }
            }
        })
        .detach();
        let session_title_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("session-title-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("session.title_placeholder"), cx);
            input.set_key_context("SessionTitleInput", cx);
            input
        });
        let permission_allow_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("permission-allow-input", cx);
            input.set_placeholder(
                deepseekcode_i18n::t("workspace.allow_rules_placeholder"),
                cx,
            );
            input
        });
        let permission_deny_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("permission-deny-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("policy.feedback_placeholder"), cx);
            input
        });
        let question_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("question-input", cx);
            input.set_key_context("QuestionInput", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.answer_placeholder"), cx);
            input
        });
        let policy_pattern_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("policy-pattern-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.pattern_placeholder"), cx);
            input
        });
        let git_commit_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("git-commit-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("git.commit_message"), cx);
            input.set_key_context("GitCommitInput", cx);
            // Cursor-style single-line box: small text, no padded shell
            // (the panel draws its own bordered container).
            input.set_small_text(true, cx);
            input.set_compact(true, cx);
            input
        });
        // All git inputs render compact (no 48px min-height shell): the
        // panel draws its own fixed-height bordered containers instead.
        let git_filter_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("git-filter-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("git.search_placeholder"), cx);
            input.set_key_context("GitFilterInput", cx);
            input.set_small_text(true, cx);
            input.set_compact(true, cx);
            input
        });
        let git_branch_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("git-branch-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("git.create_branch"), cx);
            input.set_key_context("GitBranchInput", cx);
            input.set_small_text(true, cx);
            input.set_compact(true, cx);
            input
        });
        let git_stash_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("git-stash-input", cx);
            input.set_placeholder(deepseekcode_i18n::t("git.stash_push"), cx);
            input.set_key_context("GitStashInput", cx);
            input.set_small_text(true, cx);
            input.set_compact(true, cx);
            input
        });
        let git_review_scope_input = cx.new(|cx| {
            let mut input = PromptInput::new(cx);
            input.set_element_id("git-review-scope-input", cx);
            input.set_placeholder("master", cx);
            input.set_key_context("GitReviewScopeInput", cx);
            input.set_text("master", cx);
            input.set_small_text(true, cx);
            input.set_compact(true, cx);
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
            activity: crate::session_activity::SessionActivityStore::default(),
            providers: Vec::new(),
            pending_policies: VecDeque::new(),
            policy_reject_expanded: false,
            policy_patterns_expanded: false,
            pending_policy_reply: HashSet::new(),
            policy_rule_rows: Vec::new(),
            policy_auto_mode: false,
            show_turn_usage: true,
            selected_policy_row: None,
            pending_question: None,
            prompt_input,
            api_key_input,
            base_url_input,
            custom_model_input,
            server_url_input,
            server_password_input,
            worktree_branch_input,
            selected_agent: prefs.selected_agent.into(),
            selected_model: prefs.selected_model.into(),
            selected_provider: prefs.selected_provider,
            custom_models: prefs.custom_models.clone(),
            api_key_dialog: None,
            settings_open: false,
            settings_section: SettingsSection::General,
            settings_catalog: SettingsCatalog::default(),
            window_manager: WindowManager::new(),
            file_tree: FileTreeState::default(),
            editor: EditorState::default(),
            editor_tabs: Vec::new(),
            active_editor_tab: 0,
            editor_buffers: HashMap::new(),
            editor_dirty: HashSet::new(),
            open_generation: 0,
            editor_read_only: true,
            file_editor_input,
            session_title_input,
            session_title_editing: false,
            revert_staged_files: Vec::new(),
            composer_error: None,
            permission_allow_input,
            permission_deny_input,
            policy_pattern_input,
            question_input,
            panel_resize_edge: None,
            panel_resize_start_x: 0.,
            panel_resize_start_width: 0.,
            terminal_pty_size: None,
            terminals: Vec::new(),
            active_terminal: 0,
            next_terminal_id: 1,
            bash_transcripts: HashMap::new(),
            terminal_focus: cx.focus_handle(),
            session_busy: false,
            stopping: false,
            session_status: "Ready".into(),
            session_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            terminal_scroll: ScrollHandle::new(),
            session_context_menu: None,
            project_context_menu: None,
            popover_anchor: None,
            pending_session_delete: None,
            pending_project_remove: None,
            pending_plan_confirm: None,
            last_plan_implement_at: None,
            pressed_row: None,
            editor_saving: false,
            scrollbar_drag: None,
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
            language: prefs.language,
            server_url: prefs.server_url,
            server_password: prefs.server_password,
            attachments: Vec::new(),
            image_preview: None,
            event_stream_generation: Arc::new(AtomicU64::new(0)),
            bootstrap_generation: Arc::new(AtomicU64::new(0)),
            hydrate_generation: Arc::new(AtomicU64::new(0)),
            child_projection_generation: Arc::new(AtomicU64::new(0)),
            last_sse_notify_at: None,
            session_todos: Vec::new(),
            git_staged: Vec::new(),
            git_unstaged: Vec::new(),
            git_untracked: Vec::new(),
            git_conflicted: Vec::new(),
            git_diff_summary: None,
            git_status_message: None,
            git_log: Vec::new(),
            git_branches: Vec::new(),
            git_commits: Vec::new(),
            git_stashes: Vec::new(),
            git_remotes: Vec::new(),
            git_tags: Vec::new(),
            git_current_branch: None,
            git_head_sha: None,
            git_ahead: 0,
            git_behind: 0,
            git_busy: false,
            git_error: None,
            git_error_detail: None,
            git_error_expanded: true,
            git_notice: None,
            git_commit_input,
            git_filter_input,
            git_branch_input,
            git_stash_input,
            git_selected: HashSet::new(),
            git_changes_expanded: true,
            git_staged_expanded: true,
            git_conflict_expanded: true,
            git_untracked_expanded: true,
            git_graph_expanded: true,
            git_review_expanded: true,
            git_stash_expanded: true,
            git_remotes_expanded: false,
            git_show_tree: false,
            git_commit_no_verify: false,
            git_include_untracked_stash: false,
            git_review_scope: "master".to_string(),
            git_review_scope_input,
            git_review_issues: Vec::new(),
            git_review_busy: false,
            git_ai_busy: false,
            git_graph_limit: 200,
            git_hunks: HashMap::new(),
            git_hunks_expanded: HashSet::new(),
            git_hunks_loading: HashSet::new(),
            git_pending_confirm: None,
            git_hovered_row: None,
            git_pending_commit_ui: None,
            git_commit_menu_open: false,
            git_review_scope_open: false,
            git_branch_box_open: false,
            selected_detail_key: None,
            highlighted_todo_leaf_id: None,
            timeline_text_selection: None,
            timeline_text_layouts: std::collections::HashMap::new(),
            timeline_text_focus: cx.focus_handle(),
            timeline_text_press: None,
            timeline_row_press: None,
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
        // User Settings win over project config: a pinned language in
        // preferences.json applies here (lib.rs already applied the config).
        // `Auto` keeps whatever lib.rs resolved (config or OS detection).
        if prefs.language != deepseekcode_i18n::LanguagePref::Auto {
            deepseekcode_i18n::init_from_preference(prefs.language);
        }
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
                run_id = %deepseekcode_telemetry::current_run_id(),
            path = %path.display(),
            "open_project"
        );
        crate::home::record_recent_project(path.clone());
        self.home_view.update(cx, |home, _| {
            home.add_recent(path.clone());
            home.refresh_recent();
        });
        self.reset_session_workspace_state(cx);
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
        if let Some(_message) = Self::ensure_agents_md(path) {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                path = %path.display(),
                "project context prepared"
            );
            cx.notify();
        }
    }

    /// Clear per-session VIEW state (timeline, todos, prompts, selections).
    ///
    /// Called on session switch AND new-session creation so the incoming
    /// session never inherits the previous session's UI. Does NOT touch
    /// `session_id`, generations, editor buffers, or project state.
    fn clear_session_view_state(&mut self, cx: &mut Context<Self>) {
        self.session_plans.clear();
        self.plan_picker_expanded = false;
        self.activity.session_events.clear();
        self.activity.session_messages.clear();
        self.activity.child_session_projections.clear();
        self.child_projection_generation
            .fetch_add(1, Ordering::SeqCst);
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
        self.pending_question = None;
        self.question_input.update(cx, |input, cx| input.clear(cx));
        self.pending_policies.clear();
        self.policy_reject_expanded = false;
        self.session_menu_open = false;
        self.activity.since_seq = 0;
        self.selected_message_id = None;
        self.context_usage = ContextUsageState::default();
        self.timeline_text_selection = None;
        self.timeline_text_layouts.clear();
        self.timeline_text_press = None;
        self.timeline_row_press = None;
        // Stale-run guards and the deferred queue belong to the previous
        // session's run; a fresh session starts with small seq numbers that
        // must never be mistaken for stale.
        self.activity.ignore_stale_run_events = false;
        self.activity.stale_run_cutoff_seq = None;
        self.activity.pending_hydration_events.clear();
        // In-flight editor file loads belong to the previous context.
        self.open_generation = self.open_generation.wrapping_add(1);
        self.session_follow_tail = true;
    }

    fn reset_session_workspace_state(&mut self, cx: &mut Context<Self>) {
        let pending_new = self.pending_new_session;
        let pending_switch = self.pending_switch_session;
        self.hydrate_generation.fetch_add(1, Ordering::SeqCst);
        self.event_stream_generation.fetch_add(1, Ordering::SeqCst);
        self.activity.timeline_render_generation =
            self.activity.timeline_render_generation.wrapping_add(1);
        self.session_id = None;
        self.clear_session_view_state(cx);
        self.active_session_ids.clear();
        self.activity.session_events_enabled = false;
        self.backup_dirty_editor_buffers();
        self.editor_buffers.clear();
        self.editor_dirty.clear();
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

    pub(crate) fn close_context_usage_detail(&mut self, cx: &mut Context<Self>) {
        // P1-6: external close (Esc / overlay) so the popup can't stack with
        // composer dropdowns forever.
        if self.context_usage.expanded {
            self.context_usage.expanded = false;
            cx.notify();
        }
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
        self.revert_dock.warning = state.warning.clone();
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
                    this.revert_dock.warning = None;
                    this.revert_staged_files.clear();
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
                        if action == RevertAction::Commit {
                            this.revert_dock.staged_message_id = None;
                            this.revert_dock.diff_preview.clear();
                            this.revert_dock.warning = None;
                            this.revert_staged_files.clear();
                            this.file_tree.changed_paths.clear();
                            this.revert_dock.error = None;
                            this.revert_dock.open = true;
                        } else {
                            this.apply_revert_state(&state);
                            this.revert_dock.error = None;
                            this.revert_dock.open = true;
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
                        this.revert_staged_files.clear();
                        this.file_tree.changed_paths.clear();
                        this.revert_dock.error = None;
                        this.revert_dock.open = true;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!(target: deepseekcode_telemetry::DESKTOP, run_id = %deepseekcode_telemetry::current_run_id(), error = %e, "revert action failed");
                    let message = e.to_string();
                    let _ = view.update(cx, |this, cx| {
                        this.revert_dock.error = Some(message);
                        this.revert_dock.open = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn fork_current_session(&mut self, cx: &mut Context<Self>) {
        // P2 SH7: plain Fork forks the whole session (until=None).
        self.fork_session_with_until(None, cx);
    }

    pub(crate) fn fork_current_session_from_message(&mut self, cx: &mut Context<Self>) {
        // P2 SH7: Fork-from-message honors the timeline selection.
        let until_message_id = self.selected_message_id;
        self.fork_session_with_until(until_message_id, cx);
    }

    fn fork_session_with_until(
        &mut self,
        until_message_id: Option<deepseekcode_types::MessageId>,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                let _ = view.update(cx, |this, cx| {
                    this.composer_error = Some(deepseekcode_i18n::t("workspace.stop_unreachable"));
                    cx.notify();
                });
                return;
            };
            match client.fork_session(session_id, until_message_id).await {
                Ok(session) => {
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
                Err(e) => {
                    let _ = view.update(cx, |this, cx| {
                        this.composer_error = Some(deepseekcode_i18n::tr!(
                            "workspace.delete_failed",
                            "error" => e.to_string()
                        ));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn export_current_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            self.composer_error = Some(deepseekcode_i18n::t("workspace.no_active_session"));
            cx.notify();
            return;
        };
        let sanitize = self.export_sanitize;
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                let _ = view.update(cx, |this, cx| {
                    this.composer_error =
                        Some(deepseekcode_i18n::t("workspace.stop_unreachable"));
                    cx.notify();
                });
                return;
            };
            let export_result = client
                .export_session_with_sanitize(session_id, sanitize)
                .await;
            let data = match export_result {
                Ok(d) => d,
                Err(e) => {
                    let _ = view.update(cx, |this, cx| {
                        this.composer_error = Some(deepseekcode_i18n::tr!(
                            "workspace.delete_failed",
                            "error" => e.to_string()
                        ));
                        cx.notify();
                    });
                    return;
                }
            };
            let default_name = format!("{SESSION_EXPORT_PREFIX}-{session_id}.json");
            let dest = rfd::FileDialog::new()
                .set_file_name(&default_name)
                .save_file();
            let Some(path) = dest else {
                // User cancelled — not an error, stay silent.
                return;
            };
            match serde_json::to_string_pretty(&data) {
                Ok(json) => match std::fs::write(&path, json) {
                    Ok(()) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::DESKTOP,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            path = %path.display(),
                            sanitize = sanitize,
                            "session exported"
                        );
                        rfd::MessageDialog::new()
                            .set_title(&deepseekcode_i18n::t("session.exported"))
                            .set_description(&deepseekcode_i18n::tr!(
                                "session.saved_to",
                                "path" => path.display().to_string()
                            ))
                            .show();
                    }
                    Err(e) => {
                        let _ = view.update(cx, |this, cx| {
                            this.composer_error = Some(deepseekcode_i18n::tr!(
                                "workspace.delete_failed",
                                "error" => e.to_string()
                            ));
                            cx.notify();
                        });
                    }
                },
                Err(e) => {
                    let _ = view.update(cx, |this, cx| {
                        this.composer_error = Some(deepseekcode_i18n::tr!(
                            "workspace.delete_failed",
                            "error" => e.to_string()
                        ));
                        cx.notify();
                    });
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
                    if this.project_dir.as_ref().map(|p| p.display().to_string())
                        != Some(path_str.clone())
                    {
                        this.refresh_project_nav_from_client(cx);
                        return;
                    }
                    // Optimistic nav insert: the session list is a load-once
                    // cache (projects with non-empty rows are skipped on
                    // refresh), so without this the new session stays
                    // invisible until a full reload. Later authoritative
                    // fetches replace the whole row vec — no duplicates.
                    let nav_item = crate::layout::SessionNavItem {
                        id: session.id,
                        title: session
                            .title
                            .clone()
                            .filter(|t| !t.trim().is_empty())
                            .unwrap_or_else(|| {
                                deepseekcode_i18n::t("workspace.new_session_short").to_string()
                            }),
                        updated_at: session.updated_at.to_rfc3339(),
                        agent: session.agent.clone(),
                        running: false,
                    };
                    if let Some(nav) = this
                        .project_nav
                        .iter_mut()
                        .find(|n| project_paths_equal(&n.path, &path_str))
                    {
                        nav.sessions.retain(|s| s.id != session.id);
                        nav.sessions.insert(0, nav_item);
                    } else {
                        this.project_nav.push(ProjectNavItem {
                            path: path_str.clone(),
                            sessions: vec![nav_item],
                        });
                    }
                    this.ensure_session_visible_in_nav(session.id);
                    this.hydrate_generation.fetch_add(1, Ordering::SeqCst);
                    this.event_stream_generation.fetch_add(1, Ordering::SeqCst);
                    this.session_id = Some(session.id);
                    // Same full cleanup as a session switch: the new session
                    // must not inherit the previous session's timeline,
                    // todos, pending approvals, or text selection.
                    this.clear_session_view_state(cx);
                    this.sync_agent_from_nav(session.id);
                    // New session owns no tabs yet: pin the cursor so the
                    // post-hydrate shell guarantee becomes the active tab.
                    this.fix_active_terminal_visible();
                    this.sync_context_window();
                    this.set_session_busy(false, "Loading session…");
                    this.schedule_session_loading_repaint(cx);
                    this.hydrate_session_from_api(cx, true);
                    this.refresh_revert_staged(cx);
                    this.refresh_session_plans(cx);
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
        self.event_stream_generation.fetch_add(1, Ordering::SeqCst);
        self.session_id = Some(session_id);
        self.sync_agent_from_nav(session_id);
        self.clear_session_view_state(cx);
        self.sync_context_window();
        self.set_session_busy(false, "Loading session…");
        self.schedule_session_loading_repaint(cx);
        self.hydrate_session_from_api(cx, true);
        self.refresh_revert_staged(cx);
        self.refresh_session_plans(cx);
        // Tabs are session-scoped: land the cursor on this session's tab so
        // input/render never address the previous session's terminals.
        self.fix_active_terminal_visible();
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
        _path_key: &str,
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

    /// P1-1: arm a press-origin for click-confirm semantics.
    pub(crate) fn arm_click(&mut self, id: String, cx: &mut Context<Self>) {
        self.pressed_row = Some(id);
        cx.notify();
    }

    /// P1-1: confirm a release matches the armed press-origin. Returns true
    /// only for a genuine click (down+up on the same row); drag-through and
    /// scroll releases return false. Always disarms.
    pub(crate) fn confirm_click(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let ok = self.pressed_row.as_deref() == Some(id);
        self.pressed_row = None;
        if !ok {
            cx.notify();
        }
        ok
    }

    pub(crate) fn close_popover(&mut self, cx: &mut Context<Self>) {
        let changed = self.session_context_menu.is_some()
            || self.project_context_menu.is_some()
            || self.session_menu_open
            || self.composer_dropdown.is_some()
            || self.popover_anchor.is_some()
            || self.pending_session_delete.is_some()
            || self.pending_project_remove.is_some();
        self.session_context_menu = None;
        self.project_context_menu = None;
        self.session_menu_open = false;
        self.composer_dropdown = None;
        self.popover_anchor = None;
        self.pending_session_delete = None;
        self.pending_project_remove = None;
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

    pub(crate) fn request_delete_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        // P0-6: arm a two-stage confirm instead of deleting immediately.
        self.session_context_menu = None;
        self.pending_session_delete = Some(session_id);
        // Keep popover_anchor so the confirm renders in the overlay.
        cx.notify();
    }

    pub(crate) fn cancel_pending_delete(&mut self, cx: &mut Context<Self>) {
        self.pending_session_delete = None;
        self.pending_project_remove = None;
        self.popover_anchor = None;
        self.session_context_menu = None;
        self.project_context_menu = None;
        cx.notify();
    }

    pub(crate) fn request_remove_project(&mut self, path: String, cx: &mut Context<Self>) {
        // P0-6: arm a two-stage confirm instead of removing immediately.
        self.project_context_menu = None;
        self.pending_project_remove = Some(path);
        cx.notify();
    }

    pub(crate) fn delete_session_by_id(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        self.pending_session_delete = None;
        self.close_session_context_menu(cx);
        let deleting_active = self.session_id == Some(session_id);
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let mut delete_err: Option<String> = None;
            if let Some(client) = client {
                if let Err(e) = client.delete_session(session_id).await {
                    delete_err = Some(e.to_string());
                }
            }
            let _ = view.update(cx, |this, cx| {
                if let Some(err) = delete_err {
                    // P0-6: surface failure instead of silently refreshing.
                    this.composer_error = Some(deepseekcode_i18n::tr!(
                        "workspace.delete_failed",
                        "error" => err
                    ));
                    cx.notify();
                    return;
                }
                if deleting_active {
                    this.session_id = None;
                    this.activity.since_seq = 0;
                    this.selected_message_id = None;
                    this.session_busy = false;
                    this.stopping = false;
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
            return deepseekcode_i18n::t("workspace.session_placeholder");
        };
        for nav in &self.project_nav {
            if let Some(item) = nav.sessions.iter().find(|s| s.id == session_id) {
                return item.title.clone();
            }
        }
        self.session_first_prompts
            .get(&session_id)
            .cloned()
            .unwrap_or_else(|| deepseekcode_i18n::t("workspace.new_session_short"))
    }

    fn update_session_title_in_nav(&mut self, session_id: SessionId, title: String) {
        for nav in &mut self.project_nav {
            if let Some(item) = nav.sessions.iter_mut().find(|s| s.id == session_id) {
                item.title = title;
                break;
            }
        }
    }

    pub(crate) fn confirm_remove_project(&mut self, cx: &mut Context<Self>) {
        // P0-6: execute the armed project removal.
        if let Some(path) = self.pending_project_remove.take() {
            self.remove_project_from_sidebar(path, cx);
        }
        self.popover_anchor = None;
        cx.notify();
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
                run_id = %deepseekcode_telemetry::current_run_id(),
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
                        run_id = %deepseekcode_telemetry::current_run_id(),
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

    pub(crate) fn refresh_health(&mut self, cx: &mut Context<Self>) {
        // P2 T4: manual retry for the titlebar health indicator.
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
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
        })
        .detach();
        cx.notify();
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
        self.event_stream_generation.fetch_add(1, Ordering::SeqCst);
        self.activity.session_events_enabled = false;
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
                        run_id = %deepseekcode_telemetry::current_run_id(),
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

            let activity = match client.list_activity(session_id, 0).await {
                Ok(tree) => tree,
                Err(e) => {
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        session_id = %session_id,
                        error = %e,
                        "list_activity failed; falling back to local projection from events"
                    );
                    ActivityTree::default()
                }
            };
            let session_events = client
                .list_session_events(session_id, 0)
                .await
                .unwrap_or_default();
            if activity.roots.is_empty() && !session_events.is_empty() {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    session_id = %session_id,
                    event_count = session_events.len(),
                    "server returned empty activity tree for non-empty history; rebuilding locally"
                );
            }
            let todos = client.session_todo(session_id).await.unwrap_or_default();

            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
                tokio::task::spawn_blocking(move || collect_git_status(&dir, 200))
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
                this.sync_context_usage_from_events();
                this.pending_scroll_to_tail = true;
                cx.notify();
                true
            });
            if !applied.unwrap_or(false) {
                return;
            }

            let gap_events = client
                .list_session_events(session_id, resume_seq)
                .await
                .unwrap_or_default();
            let gap_head = gap_events
                .last()
                .map(|event| event.seq)
                .unwrap_or(resume_seq);
            let active_ids = client.list_active_sessions().await.unwrap_or_default();
            let is_active = active_ids.contains(&session_id);
            let _ = view.update(cx, |this, cx| {
                if hydrate_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                this.activity.apply_hydrate_gap_events(&gap_events);
                this.activity.enable_session_events(gap_head);
                if !is_active {
                    this.activity.set_idle_stale_cutoff(gap_head);
                }
                cx.notify();
            });

            let _ = view.update(cx, |this, cx| {
                if hydrate_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if is_active {
                    this.active_session_ids.insert(session_id);
                    this.set_session_busy(true, "Running…");
                } else {
                    this.active_session_ids.remove(&session_id);
                    this.set_session_busy(false, "Ready");
                }
                // Replay live events queued during the hydration window that
                // the REST gap backfill does not already cover.
                let replay = this.activity.take_pending_events_after(gap_head);
                for event in replay {
                    this.apply_event(event, cx);
                }
                // Every historical bash execution becomes a terminal tab backed
                // by the session-independent transcript store (which survives
                // later session switches), next to at least one live shell.
                this.sync_all_bash_terminal_tabs(cx);
                this.restore_pending_question_from_events(cx);
                cx.notify();
            });
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
        if let Some(_message) = Self::ensure_agents_md(&project_dir) {}
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
            Ok(()) => Some(deepseekcode_i18n::t("workspace.agents_created")),
            Err(e) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    path = %agents.display(),
                    error = %e,
                    "failed to create AGENTS.md"
                );
                Some(deepseekcode_i18n::t("workspace.no_project_loaded"))
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
                                run_id = %deepseekcode_telemetry::current_run_id(),
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
                            run_id = %deepseekcode_telemetry::current_run_id(),
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
                this.apply_custom_models();
                let merged = this.providers.clone();
                this.init_defaults_from_providers(&merged);
                this.sync_context_window();
                if let Ok(mut guard) = session_model.lock() {
                    *guard = Some(this.current_session_model());
                }
                cx.notify();
            });
            let (agent, model_id, provider_id) = session_model
                .lock()
                .ok()
                .and_then(|mut g| g.take())
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
                let _ = view.update(cx, |this, _cx| {
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
                // Destroy live server shells before dropping their tabs,
                // otherwise every project switch leaks PTY processes.
                let live: Vec<String> = this
                    .terminals
                    .iter()
                    .filter_map(|t| t.state.pty_id.clone())
                    .collect();
                if !live.is_empty() {
                    let client_slot = Arc::clone(&this.client);
                    cx.spawn(async move |_view, _cx| {
                        if let Some(client) = lock_sdk_client(&client_slot).await {
                            for pty_id in live {
                                let _ = client.destroy_pty(&pty_id).await;
                            }
                        }
                    })
                    .detach();
                }
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

        // Preserve a custom model id (Settings → Custom model, e.g. an
        // OpenRouter `author/model` id absent from the catalog) instead of
        // clobbering it with the first catalog entry on every restart.
        if !selected.is_empty() && self.selected_provider.is_some() {
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

    fn sync_editor_buffer_from_input(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.editor.path.clone() {
            let content = self.file_editor_input.read(cx).text();
            self.editor_buffers.insert(path, content);
        }
    }

    /// Drop editor-preview text layouts: their block keys are path-namespaced,
    /// so a previous file's stale rects must never capture drags in the new
    /// file's preview (timeline layouts are untouched).
    fn clear_editor_text_layouts(&mut self) {
        self.timeline_text_layouts
            .retain(|_, layout| layout.surface != crate::selectable_text::TextSurface::Editor);
    }

    fn load_editor_buffer(&mut self, path: &str, content: String, cx: &mut Context<Self>) {
        self.editor.path = Some(path.to_string());
        // Opening a file always lands in Code view; Preview is opt-in.
        self.editor.md_preview = false;
        self.clear_editor_text_layouts();
        self.editor.set_content(content.clone());
        let file_input = self.file_editor_input.clone();
        let editor_text = content;
        cx.defer(move |cx| {
            file_input.update(cx, |input, cx| {
                input.set_text(editor_text, cx);
            });
        });
        self.editor_read_only = false;
        self.layout.right_panel_tab = RightPanelTab::Editor;
        if !self.editor_tabs.iter().any(|p| p == path) {
            self.editor_tabs.push(path.to_string());
        }
        self.active_editor_tab = self.editor_tabs.iter().position(|p| p == path).unwrap_or(0);
    }

    fn rollback_optimistic_prompt(&mut self, optimistic: &EventEnvelope) {
        self.activity
            .session_events
            .retain(|event| event.seq != optimistic.seq || event.kind != EventKind::PromptAdmitted);
        self.rebuild_activity_tree("submit_prompt_rollback");
    }

    pub(crate) fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.editor.scroll_to_line = None;
        self.editor.diff_stats = None;
        let path = path.to_string();
        if self.editor_dirty.contains(&path) {
            if let Some(content) = self.editor_buffers.get(&path).cloned() {
                self.load_editor_buffer(&path, content, cx);
                cx.notify();
                return;
            }
        }
        let snapshot_id = self
            .revert_staged_files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.snapshot_id.clone());
        self.open_generation = self.open_generation.wrapping_add(1);
        let generation = self.open_generation;
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
                if generation != this.open_generation {
                    return;
                }
                if let Some((content, diff)) = result {
                    if content.is_none() {
                        tracing::warn!(
                            target: deepseekcode_telemetry::DESKTOP,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            path = %path,
                            "failed to open file"
                        );
                    }
                    let text = content.unwrap_or_default();
                    this.editor.path = Some(path.clone());
                    this.editor.md_preview = false;
                    this.clear_editor_text_layouts();
                    this.editor.set_content(text.clone());
                    this.editor.diff_hunks = diff;
                    this.editor_buffers.insert(path.clone(), text.clone());
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
        self.reset_session_workspace_state(cx);
        self.session_nav_visible
            .insert(path.display().to_string(), 5);
        self.expanded_projects
            .insert(project_path_key(&path.display().to_string()));
        self.project_dir = Some(path.clone());
        self.refresh_git_status_async(cx);
        self.prepare_project_context(&path, cx);
        self.editor = EditorState::default();
        self.backup_dirty_editor_buffers();
        self.editor_buffers.clear();
        self.editor_dirty.clear();
        self.bootstrap_project(path, cx);
    }

    /// Persist unsaved editor buffers before they are discarded by a project
    /// or session switch, so user edits are never silently lost.
    fn backup_dirty_editor_buffers(&self) {
        if self.editor_dirty.is_empty() {
            return;
        }
        let backup_root = std::env::temp_dir().join("deepseekcode-editor-backups");
        for path in &self.editor_dirty {
            let Some(content) = self.editor_buffers.get(path) else {
                continue;
            };
            let sanitized: String = path
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let dest = backup_root.join(format!("{sanitized}.bak"));
            let result =
                std::fs::create_dir_all(&backup_root).and_then(|_| std::fs::write(&dest, content));
            match result {
                Ok(()) => tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    path = %path,
                    backup = %dest.display(),
                    "unsaved editor buffer backed up before workspace reset"
                ),
                Err(e) => tracing::error!(
                    target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    path = %path,
                    error = %e,
                    "failed to back up unsaved editor buffer"
                ),
            }
        }
    }

    pub(crate) fn show_more_project_sessions(&mut self, path: String, cx: &mut Context<Self>) {
        const PAGE: usize = 5;
        // P2 S9: normalize the key — sidebar looks up by project_path_key,
        // raw display strings would miss and reset the limit to 5.
        let key = project_path_key(&path);
        let next = self
            .session_nav_visible
            .get(&key)
            .copied()
            .unwrap_or(PAGE)
            + PAGE;
        self.session_nav_visible.insert(key, next);
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
        let key = project_path_key(&path);
        let current = self.session_nav_visible.get(&key).copied().unwrap_or(PAGE);
        if need > current {
            self.session_nav_visible.insert(key, need);
        }
    }

    pub(crate) fn add_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let title = deepseekcode_i18n::tr!(
            "terminal.shell_named",
            "name" => (self.terminals.len() + 1) as u64
        );
        let session_id = self.session_id;
        self.terminals.push(TerminalTab::new(id, title, session_id));
        self.active_terminal = self.terminals.len().saturating_sub(1);
        self.connect_terminal_tab(self.active_terminal, cx);
        cx.notify();
    }

    /// Vec indices of the tabs belonging to the current session, in order.
    /// The Terminal page renders exactly this subset; UI callbacks carry
    /// positions into it, never raw vec indices.
    pub(crate) fn visible_terminal_indices(&self) -> Vec<usize> {
        self.terminals
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.session_id == self.session_id)
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Keep `active_terminal` pointing at a tab of the current session.
    /// Called after session switches, tab closes, and history syncs so the
    /// input/render paths never address another session's tab.
    pub(crate) fn fix_active_terminal_visible(&mut self) {
        if self
            .terminals
            .get(self.active_terminal)
            .is_some_and(|tab| tab.session_id == self.session_id)
        {
            return;
        }
        self.active_terminal = self
            .terminals
            .iter()
            .position(|tab| tab.session_id == self.session_id)
            .unwrap_or(0);
    }

    /// Put keyboard focus into the terminal so typing reaches the shell
    /// immediately after clicking a tab / `+` (otherwise keystrokes keep
    /// going to the previously focused input and the terminal looks dead).
    pub(crate) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.terminal_focus, cx);
        cx.notify();
    }

    pub(crate) fn switch_terminal_tab(&mut self, visible: usize, cx: &mut Context<Self>) {
        let Some(&index) = self.visible_terminal_indices().get(visible) else {
            return;
        };
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
            // Detach a failed agent PTY so the tab falls back to its
            // transcript instead of pointing at a dead session.
            tab.agent_pty_id = None;
            tab.state.pty_live = false;
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
        // Take the previous PTY id (if any) so reconnecting never orphans a
        // server shell: it is destroyed once the client is in hand below.
        let old_pty_id = self
            .terminals
            .get_mut(tab_index)
            .and_then(|tab| tab.state.pty_id.take());
        if let Some(tab) = self.terminals.get_mut(tab_index) {
            tab.state.input_tx = None;
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
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    tab_id,
                    reason = "embedded client not ready",
                    "pty connection failed"
                );
                let _ = view.update(cx, |this, cx| {
                    this.mark_terminal_connect_failed(
                        tab_id,
                        deepseekcode_i18n::t("terminal.client_not_ready"),
                    );
                    cx.notify();
                });
                return;
            };
            if let Some(old) = old_pty_id {
                let _ = client.destroy_pty(&old).await;
            }
            let pty_id = match client.create_pty(Some(24), Some(80)).await {
                Ok(id) => id,
                Err(e) => {
                    let reason = deepseekcode_i18n::tr!(
                        "terminal.create_pty_failed",
                        "error" => e.to_string()
                    );
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
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
            // The handshake must carry the project directory: without it the
            // server validates the PTY against its own cwd and rejects the
            // upgrade (403/404) whenever the app was launched elsewhere —
            // a shell that looks idle but can never receive input.
            let ws_request = crate::terminal_vscode::pty_ws_request(&ws_url, client.directory());

            let _ = view.update(cx, |this, cx| {
                if let Some(tab) = this.terminals.iter_mut().find(|t| t.id == tab_id) {
                    tab.state.pty_id = Some(pty_id);
                    tab.state.connected = false;
                }
                cx.notify();
            });

            let ws = match connect_async(ws_request).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    let reason = deepseekcode_i18n::tr!(
                        "terminal.websocket_failed",
                        "url" => ws_url.as_str(),
                        "error" => e.to_string()
                    );
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
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
            // First live frame flips the tab from transcript fallback to the
            // real grid view (see `TerminalState::pty_live`).
            let mut live_marked = false;
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
                        // History replay (if any) goes in right AFTER the
                        // first live bytes (shell banner), so the tab reads
                        // banner → replay → live prompt, like the original.
                        let replay = if !live_marked {
                            live_marked = true;
                            view.update(cx, |this, _cx| {
                                this.terminals.iter_mut().find(|t| t.id == tab_id).and_then(
                                    |tab| {
                                        tab.state.pty_live = true;
                                        tab.pending_replay.take()
                                    },
                                )
                            })
                            .ok()
                            .flatten()
                        } else {
                            None
                        };
                        if let Some(bytes) = replay {
                            grid.lock().await.write(&bytes);
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
        // Session scoping applies to input too: keystrokes must never reach
        // another session's shell (e.g. focus lingering after a switch while
        // the visible list is empty).
        let Some(tab) = self
            .terminals
            .get(idx)
            .filter(|tab| tab.session_id == self.session_id)
        else {
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

    /// Attach a tab to a live agent PTY: the placeholder transcript tab
    /// morphs into the real running terminal (same grid/input pipeline as a
    /// hand-opened shell). Idempotent per `(tab, pty_id)`.
    pub(crate) fn attach_agent_pty_tab(
        &mut self,
        detail_key: &str,
        pty_id: &str,
        session: Option<SessionId>,
        cx: &mut Context<Self>,
    ) {
        let tab_idx = self.ensure_bash_terminal_tab(detail_key, cx, session);
        let (grid, buffer, tab_id) = match self.terminals.get(tab_idx) {
            Some(tab) => (Arc::clone(&tab.grid), Arc::clone(&tab.buffer), tab.id),
            None => return,
        };
        if let Some(tab) = self.terminals.get_mut(tab_idx) {
            if tab.agent_pty_id.as_deref() == Some(pty_id)
                && (tab.state.connected || tab.state.pty_connecting)
            {
                return;
            }
            tab.agent_pty_id = Some(pty_id.to_string());
            tab.state.pty_live = false;
            tab.state.input_tx = None;
            tab.state.pty_id = None;
            tab.state.connect_error = None;
            tab.state.pty_connecting = true;
        }
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(tab) = self.terminals.get_mut(tab_idx) {
            tab.state.input_tx = Some(input_tx.clone());
        }
        let client_slot = Arc::clone(&self.client);
        let pty_id = pty_id.to_string();
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                let _ = view.update(cx, |this, cx| {
                    this.mark_terminal_connect_failed(
                        tab_id,
                        deepseekcode_i18n::t("terminal.client_not_ready"),
                    );
                    cx.notify();
                });
                return;
            };
            grid.lock().await.reset();
            // Agent PTYs need no ticket (REST auth middleware covers the
            // route); the project directory header is the ownership proof.
            let ws_url = client.ws_url(&format!("/api/v1/agent-pty/{pty_id}/connect"));
            let ws_request = crate::terminal_vscode::pty_ws_request(&ws_url, client.directory());
            let ws = match connect_async(ws_request).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    let reason = deepseekcode_i18n::tr!(
                        "terminal.websocket_failed",
                        "url" => ws_url.as_str(),
                        "error" => e.to_string()
                    );
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        tab_id,
                        reason = %reason,
                        "agent pty connection failed"
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
            let mut live_marked = false;
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
                        // History replay (if any) goes in right AFTER the
                        // first live bytes (shell banner), so the tab reads
                        // banner → replay → live prompt, like the original.
                        let replay = if !live_marked {
                            live_marked = true;
                            view.update(cx, |this, _cx| {
                                this.terminals.iter_mut().find(|t| t.id == tab_id).and_then(
                                    |tab| {
                                        tab.state.pty_live = true;
                                        tab.pending_replay.take()
                                    },
                                )
                            })
                            .ok()
                            .flatten()
                        } else {
                            None
                        };
                        if let Some(bytes) = replay {
                            grid.lock().await.write(&bytes);
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
                Ok(entry) => deepseekcode_i18n::tr!(
                    "git.worktree_created",
                    "path" => entry
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                ),
                Err(e) => deepseekcode_i18n::tr!(
                    "git.worktree_create_failed",
                    "error" => e.to_string()
                ),
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
                run_id = %deepseekcode_telemetry::current_run_id(),
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
                                            run_id = %deepseekcode_telemetry::current_run_id(),
                                        since_seq = since_seq,
                                        head = head,
                                        "sse gap detected, backfilling via REST"
                                    );
                                    if let Some(session_id) = filter_session {
                                        if let Ok(events) =
                                            client.list_session_events(session_id, since_seq).await
                                        {
                                            for event in events {
                                                since_seq = since_seq.max(event.seq);
                                                let _ = view.update(cx, |this, cx| {
                                                    this.apply_event(event, cx);
                                                    cx.notify();
                                                });
                                            }
                                            let _ = view.update(cx, |this, cx| {
                                                this.rebuild_activity_tree("sse_gap_backfill");
                                                cx.notify();
                                            });
                                            // Advance only — never move backwards.
                                            // The server's `head` can lag behind what
                                            // this client already applied (e.g. after
                                            // a broadcast-lag gap).
                                            since_seq = since_seq.max(head);
                                        }
                                    }
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
                                            let throttle_ms = if kind == EventKind::ToolProgress {
                                                100
                                            } else {
                                                16
                                            };
                                            if let Some(last) = this.last_sse_notify_at {
                                                if now.duration_since(last)
                                                    < std::time::Duration::from_millis(throttle_ms)
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
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            since_seq = since_seq,
                            "event stream disconnected, reconnecting"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                                run_id = %deepseekcode_telemetry::current_run_id(),
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
                        let detail = summarize_permission_request(&req.tool_name, &req.input);
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
                if let Some(prompt) = self.pending_policies.remove(pos) {
                    self.pending_policies.push_front(prompt);
                } else {
                    tracing::warn!(
                        target: deepseekcode_telemetry::APP,
                        pos,
                        "focus_policy_prompt remove out of bounds"
                    );
                }
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
        // Concise: the dock asks "may this run?", so only the decision-relevant
        // facts belong here. Full payload stays in the detail panel.
        let detail = summarize_permission_request(&tool, &input);
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
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
            // Queue instead of dropping: once hydration completes these events
            // are replayed, so nothing arriving mid-hydration is lost.
            self.activity.defer_event_during_hydration(event.clone());
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                kind = %event.kind.as_str(),
                seq = event.seq,
                "apply_event deferred during hydration (queued for replay)"
            );
            return;
        }
        if self.activity.ignore_stale_run_events && Self::is_stale_run_event(event.kind) {
            if let Some(cutoff) = self.activity.stale_run_cutoff_seq {
                if event.seq <= cutoff {
                    tracing::debug!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        kind = %event.kind.as_str(),
                        seq = event.seq,
                        cutoff,
                        busy = self.session_busy,
                        "apply_event ignored stale run event"
                    );
                    return;
                }
            }
        }
        if event.kind == EventKind::ActivityUpdated {
            self.activity.since_seq = event.seq;
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
                    run_id = %deepseekcode_telemetry::current_run_id(),
                session_id = ?event.session_id,
                kind = %event.kind.as_str(),
                seq = event.seq,
                busy = self.session_busy,
                "apply_event"
            );
        } else {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
                    // Per-turn receipt + per-model cost, gated by `ui.show_turn_usage`.
                    // Display-only: the server always records the fields.
                    self.context_usage.last_receipt =
                        deepseekcode_core::cache_status::receipt_from_payload(
                            &event.payload,
                            self.show_turn_usage,
                        );
                    let model = event.payload
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.context_usage.last_model = model.clone();
                    // Prefer server-stamped cost; fall back to local catalog so
                    // old events still show money instead of going blind.
                    let (cost, currency) = match (
                        event.payload.get("estimated_cost").and_then(|v| v.as_f64()),
                        event.payload.get("currency").and_then(|v| v.as_str()),
                    ) {
                        (Some(c), Some(cur)) => (c, cur.to_string()),
                        (Some(c), None) => (c, "CNY".to_string()),
                        _ => {
                            let cur = event
                                .payload
                                .get("currency")
                                .and_then(|v| v.as_str())
                                .unwrap_or("CNY");
                            match deepseekcode_core::pricing::rate_for(&model, cur) {
                                Some((card, _)) => {
                                    let hit = event.payload.get("cache_hit").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let miss = event.payload.get("cache_miss").and_then(|v| v.as_u64()).unwrap_or_else(|| {
                                        (input as u64).saturating_sub(hit)
                                    });
                                    let c = deepseekcode_core::pricing::cost_for_tokens(
                                        hit,
                                        miss,
                                        output as u64,
                                        &card,
                                    );
                                    (c, card.currency.clone())
                                }
                                None => (0.0, cur.to_string()),
                            }
                        }
                    };
                    if cost > 0.0 && self.show_turn_usage {
                        self.context_usage.last_cost =
                            deepseekcode_core::pricing::format_cost(&currency, cost);
                    } else if !self.show_turn_usage {
                        self.context_usage.last_cost = String::new();
                    }
                    // Session total accumulates so the meter is always current.
                    if cost > 0.0 {
                        if self.context_usage.session_currency.is_empty() {
                            self.context_usage.session_currency = currency.clone();
                        }
                        // Mixed-currency sessions keep the first currency bucket;
                        // per-model split stays exact in `/cache-status.models[]`.
                        if self.context_usage.session_currency == currency {
                            self.context_usage.session_cost += cost;
                        }
                        self.context_usage.session_turns += 1;
                    }
                } else {
                    self.apply_token_usage(
                        0,
                        Self::estimate_tokens(&content),
                        reasoning.as_deref().map(Self::estimate_tokens).unwrap_or(0),
                    );
                    // P1-3: no provider usage in this payload — clear any
                    // previous receipt instead of showing a stale card next
                    // to fresh estimates.
                    self.context_usage.last_receipt = String::new();
                    self.context_usage.last_cost = String::new();
                    self.context_usage.last_model = String::new();
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
                let command = command_from_tool_input(&input);
                if let Some(path) = extract_tool_path(&name, &input) {
                    self.mark_file_changed(&path);
                }
                self.set_session_busy(true, format!("Running tool: {name}"));
                if let Some(id) = event.payload["tool_call_id"].as_str() {
                    // Join key: progress chunks travel under the internal id.
                    if let Some(internal) = event.payload["internal_tool_call_id"].as_str() {
                        self.activity.detail_store.alias_tool_call(internal, id);
                    }
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
                            command,
                        },
                    );
                    // Every LLM shell execution gets a terminal tab immediately
                    // (no focus steal) so it is findable in the Terminal page
                    // with live output while it runs (plus a live shell next
                    // to the transcripts so the user always has somewhere to
                    // type). Tabs are tagged with the owning session.
                    // `powershell` delegates to the bash core, hence covered.
                    if matches!(name.as_str(), "bash" | "powershell") {
                        let session = event.session_id;
                        self.ensure_bash_terminal_tab(id, cx, session);
                    }
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
                    // Persist structured JSON for bash (stdout/stderr/exit_code
                    // stay inspectable in the Terminal INPUT/OUTPUT view);
                    // the renderer derives display text via `parse_bash_result`.
                    let stored_output = if name == "bash" {
                        let merged = crate::terminal_vscode::merge_bash_result_json(
                            streamed.as_deref(),
                            &output,
                        );
                        truncate_text(&merged, 50_000)
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
                            // Keep the command captured on ToolStarted; if
                            // Started was missed (gap backfill), fall back to
                            // the command echoed in completion metadata.
                            command: self
                                .activity
                                .detail_store
                                .get(id)
                                .and_then(|d| d.command.clone())
                                .or_else(|| {
                                    event.payload["metadata"]["command"]
                                        .as_str()
                                        .map(str::to_string)
                                })
                                .or_else(|| {
                                    event.payload["metadata"]["script"]
                                        .as_str()
                                        .map(str::to_string)
                                }),
                        },
                    );
                    if matches!(name.as_str(), "bash" | "powershell") {
                        // Publish final output into the transcript, then drop
                        // the finished tab: completed commands are cleared,
                        // never kept. The transcript survives, so clicking the
                        // activity leaf reopens it on demand.
                        let session = event.session_id;
                        self.ensure_bash_terminal_tab(id, cx, session);
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
                self.refresh_git_status_async(cx);
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
                        title: Some(deepseekcode_i18n::t("activity.leaf.todo_write")),
                        url: None,
                        web_preview: None,
                        command: None,
                    },
                );
            }
            EventKind::ToolProgress => {
                if let Some(raw_id) = event.payload["tool_call_id"].as_str() {
                    // Join key: progress travels under the INTERNAL id while
                    // Started/Completed use the outer one — resolve first so
                    // live output, attach and refresh all hit the SAME call
                    // instead of orphan entries + ghost tabs.
                    let id = self
                        .activity
                        .detail_store
                        .canonical_tool_call(raw_id)
                        .to_string();
                    let id = id.as_str();
                    // Agent-PTY attach handshake (`stream == "pty"`): morph
                    // the placeholder tab into the real running terminal.
                    if event.payload["stream"].as_str() == Some("pty") {
                        let chunk = event.payload["chunk"].as_str().unwrap_or("");
                        if let Some(pty_id) = crate::terminal_vscode::parse_agent_pty_id(chunk) {
                            self.attach_agent_pty_tab(id, &pty_id, event.session_id, cx);
                        }
                    }
                    // NOTE: `apply_session_event` (called above) ALREADY
                    // appended this chunk to `detail_store`. Appending again
                    // here duplicated every streamed line, so the terminal
                    // showed each output line twice. Only the live tab refresh
                    // belongs in this crate-level handler.
                    let has_tab = self
                        .terminals
                        .iter()
                        .any(|t| t.bash_detail_key.as_deref() == Some(id));
                    if !has_tab {
                        // Started event may have been missed (reconnect); if
                        // this is a bash execution, materialise its tab now so
                        // streaming output is visible live.
                        let is_bash = self
                            .activity
                            .detail_store
                            .get(id)
                            .and_then(|d| d.command.clone())
                            .is_some();
                        if is_bash {
                            let session = event.session_id;
                            self.ensure_bash_terminal_tab(id, cx, session);
                        }
                    }
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
                self.activity.ignore_stale_run_events = false;
                self.activity.stale_run_cutoff_seq = None;
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
                    .map(str::to_string)
                    .unwrap_or_else(|| deepseekcode_i18n::t("workspace.context_refreshed"));
                let _detail = event.payload["sources"]
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
                self.policy_patterns_expanded = false;
                // Must also clear a pending QUESTION: `composer_blocked()`
                // ORs both flags, so a surviving question pinned
                // `composer_blocked()` to true forever and left the composer
                // permanently unclickable with no way to recover but reloading.
                self.pending_question = None;
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.remove(&session_id);
                }
                self.activity.stale_run_cutoff_seq = Some(event.seq);
                self.activity.ignore_stale_run_events = true;
                if error.is_empty() {
                    self.set_session_busy(false, "Interrupted");
                } else {
                    self.set_session_busy(false, "Error");
                    self.composer_error = Some(if error.contains("context unavailable") {
                        deepseekcode_i18n::tr!(
                            "workspace.agents_not_ready",
                            "message" => deepseekcode_i18n::t("workspace.no_project")
                        )
                    } else {
                        truncate_text(&error, 2_000)
                    });
                }
            }
            EventKind::SessionIdle => {
                self.set_session_busy(false, "Ready");
                if let Some(session_id) = event.session_id {
                    self.active_session_ids.remove(&session_id);
                }
                self.activity.stale_run_cutoff_seq = Some(event.seq);
                self.activity.ignore_stale_run_events = true;
            }
            EventKind::McpStatusChanged => {
                let _name = event.payload["name"].as_str().unwrap_or("mcp");
                let _connected = event.payload["connected"].as_bool().unwrap_or(false);
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
                self.apply_question_event(&event, cx);
            }
            EventKind::PlanCompleted => {
                self.ingest_plan_completed_event(&event, cx);
            }
        }
        // Tail-follow on content growth: every streamed chunk or completed
        // block keeps a bottom-pinned timeline pinned, so the newest message
        // is never hidden below the fold. Respects manual scroll-up: the flag
        // is only honored while `session_follow_tail` is set, which render
        // clears the moment the user leaves the bottom.
        match event.kind {
            EventKind::MessageDelta
            | EventKind::ToolProgress
            | EventKind::ToolStarted
            | EventKind::ToolCompleted
            | EventKind::MessageCompleted
            | EventKind::TodoUpdated
            | EventKind::ProviderTurnStarted
            | EventKind::PermissionResolved
            | EventKind::PolicyResolved
            | EventKind::QuestionRequested
            | EventKind::PlanCompleted
            | EventKind::SessionIdle
            | EventKind::SessionInterrupted => self.maybe_scroll_session_to_tail(),
            _ => {}
        }
        if rebuild_activity {
            let roots_before = self.activity.activity_tree.roots.len();
            self.rebuild_activity_tree("apply_event");
            let roots_after = self.activity.activity_tree.roots.len();
            if roots_after == 0 && roots_before > 0 {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    kind = %event.kind.as_str(),
                    seq = event.seq,
                    event_count = self.activity.session_events.len(),
                    roots_before,
                    "apply_event rebuild cleared activity tree"
                );
            }
        }
    }

    fn restore_pending_question_from_events(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self
            .activity
            .session_events
            .iter()
            .rev()
            .find(|e| e.kind == EventKind::QuestionRequested)
            .cloned()
        else {
            return;
        };
        let answered = self.activity.session_events.iter().any(|e| {
            e.seq > last.seq
                && matches!(
                    e.kind,
                    EventKind::SessionIdle
                        | EventKind::SessionInterrupted
                        | EventKind::ToolCompleted
                )
        });
        if answered {
            return;
        }
        self.apply_question_event(&last, cx);
    }

    fn apply_question_event(&mut self, event: &EventEnvelope, cx: &mut Context<Self>) {
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
            deepseekcode_i18n::t("workspace.question_other_hint")
        } else {
            deepseekcode_i18n::t("workspace.answer_placeholder")
        };
        self.question_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            input.clear(cx);
        });
    }

    pub(crate) fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        if self.composer_blocked() {
            return;
        }
        let mut text = self.prompt_input.read(cx).text();
        if text.trim().is_empty()
            || text == deepseekcode_i18n::t("composer.placeholder")
            || text == deepseekcode_i18n::t("composer.placeholder_idle")
        {
            return;
        }
        let (_, provider_id) = self.resolve_selected_model();
        if let Some(provider) = self.providers.iter().find(|p| p.id == provider_id) {
            if !provider.connected {
                self.composer_error = Some(deepseekcode_i18n::tr!(
                    "workspace.provider_not_connected",
                    "provider" => provider.name.as_str()
                ));
                cx.notify();
                return;
            }
        }
        self.composer_error = None;
        if let Some(dir) = self.project_dir.clone() {
            if let Some(message) = Self::ensure_agents_md(&dir) {
                self.composer_error = Some(deepseekcode_i18n::tr!(
                    "workspace.agents_not_ready",
                    "message" => message.as_str()
                ));
                cx.notify();
                return;
            }
        }
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
            text_len = text.len(),
            session_id = ?self.session_id,
            "submit_prompt invoked"
        );
        self.activity.ignore_stale_run_events = false;
        self.activity.stale_run_cutoff_seq = None;
        // Cancel any in-flight timeline hydration so it cannot wipe the optimistic user row.
        self.hydrate_generation.fetch_add(1, Ordering::SeqCst);
        self.start_event_listener(cx);
        // The kill above may have cancelled the hydrate that would have
        // enabled live events (new session + immediate prompt). Never leave
        // the session with events disabled here, or streamed output is
        // deferred forever and the LLM reply never renders.
        if !self.activity.session_events_enabled {
            let head = self.activity.since_seq;
            self.activity.enable_session_events(head);
            let replay = self.activity.take_pending_events_after(head);
            for event in replay {
                self.apply_event(event, cx);
            }
        }
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
        push_session_event(&mut self.activity.session_events, optimistic.clone());
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
                            run_id = %deepseekcode_telemetry::current_run_id(),
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
                    let _ = view.update(cx, |this, cx| {
                        this.rollback_optimistic_prompt(&optimistic);
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
                                run_id = %deepseekcode_telemetry::current_run_id(),
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
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            session_id = %session_id,
                            error = %e,
                            "prompt submit failed"
                        );
                        let _ = view.update(cx, |this, cx| {
                            this.set_session_busy(false, "Error");
                            this.composer_error = Some(e.to_string());
                            this.rollback_optimistic_prompt(&optimistic);
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

    /// Abort the running task.
    ///
    /// The UI is unblocked **locally and immediately** — pending permission
    /// prompts and questions are dropped and `session_busy` clears on the same
    /// tick. Previously the handler only flipped a status string and relied
    /// entirely on an SSE `SessionInterrupted` to finish the job, so any stop
    /// the server answered with a bare `200` (session not in the active drain,
    /// e.g. the `PromptAdmitted`→`wake()` admission window) left the composer
    /// pinned to "Stopping…" with a stop button that re-issued the same no-op.
    pub(crate) fn interrupt_session(&mut self, cx: &mut Context<Self>) {
        // P0-2: debounce — a rapid second click while the interrupt RPC is in
        // flight must not flip into Send.
        if self.stopping {
            return;
        }
        let Some(session_id) = self.session_id else {
            self.composer_error = Some(deepseekcode_i18n::t("workspace.no_active_session"));
            self.set_session_busy(false, "Ready");
            cx.notify();
            return;
        };
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %session_id,
            pending_policies = self.pending_policies.len(),
            pending_question = self.pending_question.is_some(),
            "interrupt requested"
        );
        // Local unblock — never leave the user staring at a dead composer.
        self.pending_policies.clear();
        self.policy_reject_expanded = false;
        self.policy_patterns_expanded = false;
        self.pending_question = None;
        self.stopping = true;
        self.set_session_busy(false, "Stopping…");
        cx.notify();

        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                // Previously a silent no-op: no request, no status change.
                let _ = view.update(cx, |this, cx| {
                    this.stopping = false;
                    this.composer_error = Some(deepseekcode_i18n::t("workspace.stop_unreachable"));
                    this.set_session_busy(false, "Error");
                    cx.notify();
                });
                return;
            };
            let result = client.interrupt(session_id).await;
            let _ = view.update(cx, |this, cx| {
                this.stopping = false;
                if let Err(e) = result {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        session_id = %session_id,
                        error = %e,
                        "interrupt failed"
                    );
                    // Surface the reason: the header alone said only "Error".
                    this.composer_error = Some(deepseekcode_i18n::tr!(
                        "workspace.stop_failed",
                        "error" => e.to_string()
                    ));
                    this.set_session_busy(false, "Error");
                }
                // On success keep the local "Stopping…" state: the SSE
                // `SessionInterrupted` event will drive Ready/Interrupted.
                // Clearing `stopping` re-enables Send for the next prompt.
                cx.notify();
            });
        })
        .detach();
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
        // P1-3: clear ALL competing popovers so the composer dropdown can
        // never be shadowed by a stale session/project/session-menu layer
        // (render_popover_overlay is an if/else-if priority chain).
        self.session_context_menu = None;
        self.project_context_menu = None;
        self.session_menu_open = false;
        self.pending_session_delete = None;
        self.pending_project_remove = None;
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
                Ok(true) => deepseekcode_i18n::t("workspace.compaction_installed"),
                Ok(false) => deepseekcode_i18n::t("workspace.nothing_to_compact"),
                Err(e) => deepseekcode_i18n::tr!(
                    "workspace.compaction_failed",
                    "error" => e.to_string()
                ),
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
        let dir = std::env::temp_dir().join("deepseekcode-paste");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = chrono::Utc::now().timestamp_millis();
        // Pastes are normalized to BMP so the on-disk format is deterministic
        // (screenshots arrive as DIB/BMP natively; clipboard enumeration order
        // otherwise decides between PNG/JPG copies of the same image).
        if let Some(bmp) = crate::composer::encode_clipboard_image_as_bmp(&bytes) {
            let path = dir.join(format!("paste-{stamp}.bmp"));
            if std::fs::write(&path, bmp).is_ok() {
                self.attach_path(path, cx);
            }
            return;
        }
        let ext = match format {
            gpui::ImageFormat::Png => "png",
            gpui::ImageFormat::Jpeg => "jpg",
            gpui::ImageFormat::Gif => "gif",
            gpui::ImageFormat::Webp => "webp",
            gpui::ImageFormat::Bmp => "bmp",
            gpui::ImageFormat::Tiff => "tiff",
            gpui::ImageFormat::Ico => "ico",
            gpui::ImageFormat::Svg => "svg",
            _ => "png",
        };
        let path = dir.join(format!("paste-{stamp}.{ext}"));
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
                Ok(()) => deepseekcode_i18n::tr!(
                    "git.worktree_removed",
                    "path" => path.as_str()
                ),
                Err(e) => deepseekcode_i18n::tr!(
                    "git.worktree_remove_failed",
                    "error" => e.to_string()
                ),
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
                Ok(()) => deepseekcode_i18n::t("git.worktrees_pruned"),
                Err(e) => deepseekcode_i18n::tr!(
                    "git.worktree_prune_failed",
                    "error" => e.to_string()
                ),
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

    pub(crate) fn handle_policy_deny_click(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // P1-7: two-stage deny — first click expands AND focuses the reason box
        // (previously focus stayed in the main input so typing went nowhere).
        if self.pending_policy_reply.contains(&id) {
            return;
        }
        if self.policy_reject_expanded {
            self.respond_policy_reject(Some(id), cx);
        } else {
            self.policy_reject_expanded = true;
            let handle = self.permission_deny_input.read(cx).focus_handle_clone();
            window.focus(&handle, cx);
            cx.notify();
        }
    }

    pub(crate) fn cancel_policy_reject(&mut self, cx: &mut Context<Self>) {
        // P1-7: collapse the reason box without sending a reply.
        self.policy_reject_expanded = false;
        self.clear_policy_reject_input(cx);
        cx.notify();
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
        // P0-5: debounce — ignore concurrent replies for the same prompt.
        if self.pending_policy_reply.contains(&id) {
            return;
        }
        self.pending_policy_reply.insert(id.clone());
        self.remove_policy_prompt(&id);
        // P0-5: reset transient dock state so the next approval starts folded.
        self.policy_reject_expanded = false;
        self.policy_patterns_expanded = false;
        // Approving/denying unblocks the run, which continues at the bottom:
        // pin the timeline there. Otherwise the viewport stays at the removed
        // dock's old offset, which reads as "jumped to the item's top".
        self.session_follow_tail = true;
        self.pending_scroll_to_tail = true;
        let client_slot = Arc::clone(&self.client);
        let restore_id = id.clone();
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                if let Err(e) = client.reply_policy(&id, reply).await {
                    tracing::error!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        policy_id = %id,
                        error = %e,
                        "policy reply failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        this.pending_policy_reply.remove(&id);
                        this.sync_pending_permissions(cx);
                        this.composer_error =
                            Some(deepseekcode_i18n::t("workspace.policy_reply_failed"));
                        cx.notify();
                    });
                    return;
                }
            }
            let _ = view.update(cx, |this, cx| {
                this.pending_policy_reply.remove(&id);
                cx.notify();
            });
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
        // Keep pending until success so a transport failure never loses the
        // question (previously cleared upfront with only a log).
        let pending = self.pending_question.clone();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let mut ok = false;
            if let Some(client) = client {
                match client.submit_question_answer(session_id, &answer).await {
                    Ok(()) => ok = true,
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            session_id = %session_id,
                            error = %e,
                            "question answer failed"
                        );
                        let _ = view.update(cx, |this, cx| {
                            // Restore pending + surface error; keep typed text.
                            this.pending_question = pending;
                            this.composer_error = Some(deepseekcode_i18n::tr!(
                                "workspace.question_failed",
                                "error" => e.to_string()
                            ));
                            cx.notify();
                        });
                    }
                }
            }
            if ok {
                let _ = view.update(cx, |this, cx| {
                    this.pending_question = None;
                    this.question_input.update(cx, |input, cx| input.clear(cx));
                    cx.notify();
                });
            } else {
                let _ = view.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn cancel_pending_question(&mut self, cx: &mut Context<Self>) {
        // P0-7: every modal needs an exit. Previously the question dialog had
        // no Cancel/X/Esc and a dead session could trap focus forever.
        // We keep the typed text so a mis-click doesn't lose work.
        self.pending_question = None;
        cx.notify();
    }

    pub(crate) fn enable_question_custom_mode(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.pending_question.as_mut() {
            prompt.custom_mode = true;
            cx.notify();
        }
        // Focus the custom input so keyboard goes there (previously stayed on
        // the option list). GPUI focus goes via Window; Context has no focus().
        // The overlay input is rendered with track_focus, so notifying is
        // sufficient — explicit window focus is handled by the caller.
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
        // Optimistic flag with rollback: on server failure the dock must
        // reappear (previously `let _=` swallowed errors and refresh
        // resurrected it confusingly).
        let prev: Vec<bool> = self
            .session_plans
            .iter()
            .map(|p| p.implement_prompt_dismissed)
            .collect();
        if let Some(plan) = self.session_plans.iter_mut().find(|p| p.plan_id == plan_id) {
            plan.implement_prompt_dismissed = true;
        }
        let plan_id = plan_id.to_string();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let mut ok = true;
            if let Some(client) = lock_sdk_client(&client_slot).await {
                if let Err(e) = client
                    .dismiss_plan_implement_prompt(session_id, &plan_id)
                    .await
                {
                    ok = false;
                    tracing::warn!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        error = %e,
                        "dismiss plan prompt failed; rolling back"
                    );
                }
            }
            let _ = view.update(cx, |this, cx| {
                if !ok {
                    for (plan, was) in this.session_plans.iter_mut().zip(prev.iter()) {
                        plan.implement_prompt_dismissed = *was;
                    }
                    this.composer_error = Some(deepseekcode_i18n::t("workspace.dismiss_failed"));
                }
                this.refresh_session_plans(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn implement_from_plan(&mut self, plan_id: &str, cx: &mut Context<Self>) {
        // P0-8: debounce — ignore rapid double-clicks (<800ms).
        let now = std::time::Instant::now();
        if let Some(last) = self.last_plan_implement_at {
            if now.duration_since(last).as_millis() < 800 {
                return;
            }
        }
        // P0-8: don't silently clobber a non-empty draft. Arm a confirm with
        // Append / Replace / Cancel instead.
        let draft = self.prompt_input.read(cx).text();
        if !draft.trim().is_empty() && self.pending_plan_confirm.as_deref() != Some(plan_id) {
            self.pending_plan_confirm = Some(plan_id.to_string());
            cx.notify();
            return;
        }
        self.pending_plan_confirm = None;
        self.last_plan_implement_at = Some(now);
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
        // If a draft existed and the user confirmed Replace, overwrite;
        // Append path is handled by confirm_plan_implement_append.
        self.prompt_input.update(cx, |input, cx| {
            input.set_text(&prompt, cx);
        });
        // Implementing implies the dock is handled: persist dismissed so it
        // does not linger after switching to build. Local flag first (instant
        // hide), server dismiss best-effort so reload doesn't resurrect it.
        if let Some(p) = self.session_plans.iter_mut().find(|p| p.plan_id == plan_id) {
            p.implement_prompt_dismissed = true;
        }
        if let Some(session_id) = self.session_id {
            let plan_id_owned = plan_id.to_string();
            let client_slot = Arc::clone(&self.client);
            cx.spawn(async move |_, _cx| {
                if let Some(client) = lock_sdk_client(&client_slot).await {
                    let _ = client
                        .dismiss_plan_implement_prompt(session_id, &plan_id_owned)
                        .await;
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn confirm_plan_implement_replace(&mut self, cx: &mut Context<Self>) {
        if let Some(plan_id) = self.pending_plan_confirm.take() {
            // Clear the draft guard by emptying first, then run the normal path.
            self.prompt_input.update(cx, |input, cx| {
                input.set_text("", cx);
            });
            // Reset debounce so the confirmed click isn't swallowed.
            self.last_plan_implement_at = None;
            self.implement_from_plan(&plan_id, cx);
        }
    }

    pub(crate) fn confirm_plan_implement_append(&mut self, cx: &mut Context<Self>) {
        if let Some(plan_id) = self.pending_plan_confirm.take() {
            let now = std::time::Instant::now();
            self.last_plan_implement_at = Some(now);
            let plan_path = self
                .session_plans
                .iter()
                .find(|p| p.plan_id == plan_id)
                .map(|p| p.plan_path.clone())
                .unwrap_or_default();
            let prompt = implement_prompt_from_plan_path(&plan_path);
            let existing = self.prompt_input.read(cx).text();
            let merged = if existing.trim().is_empty() {
                prompt
            } else {
                format!("{existing}\n\n{prompt}")
            };
            self.prompt_input.update(cx, |input, cx| {
                input.set_text(&merged, cx);
            });
            self.plan_picker_expanded = false;
            self.set_agent("build", cx);
            if let Some(p) = self.session_plans.iter_mut().find(|p| p.plan_id == plan_id) {
                p.implement_prompt_dismissed = true;
            }
            cx.notify();
        }
    }

    pub(crate) fn cancel_plan_implement_confirm(&mut self, cx: &mut Context<Self>) {
        self.pending_plan_confirm = None;
        cx.notify();
    }

    pub(crate) fn open_plan_file(&mut self, plan_path: &str, cx: &mut Context<Self>) {
        let path = if let Some(dir) = self.project_dir.as_ref() {
            let candidate = dir.join(plan_path);
            if candidate.is_file() {
                candidate.display().to_string()
            } else {
                // P0-8/P1: surface missing-file instead of silently opening an
                // empty buffer (previously looked like "click did nothing").
                self.composer_error = Some(deepseekcode_i18n::tr!(
                    "workspace.delete_failed",
                    "error" => format!("plan file not found: {plan_path}")
                ));
                cx.notify();
                return;
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
            input.set_placeholder(deepseekcode_i18n::t("workspace.api_key_prompt"), cx);
        });
        self.base_url_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder(deepseekcode_i18n::t("workspace.base_url_prompt"), cx);
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
        // P2: visible feedback instead of silent return (was: non-files and
        // oversize images vanished without a word).
        if !path.is_file() {
            self.composer_error = Some(deepseekcode_i18n::tr!(
                "workspace.delete_failed",
                "error" => format!("not a file: {}", path.display())
            ));
            cx.notify();
            return;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            const MAX_ATTACH_BYTES: u64 = 32 * 1024 * 1024;
            if meta.len() > MAX_ATTACH_BYTES {
                self.composer_error = Some(deepseekcode_i18n::tr!(
                    "workspace.delete_failed",
                    "error" => format!(
                        "file too large ({} MB > 32 MB): {}",
                        meta.len() / 1024 / 1024,
                        path.display()
                    )
                ));
                cx.notify();
                return;
            }
        }
        // P2: don't pile attachments while blocked — they'd sit unsendable.
        if self.composer_blocked() {
            self.composer_error = Some(deepseekcode_i18n::t("composer.waiting_approval"));
            cx.notify();
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
            deepseekcode_i18n::tr!(
                "workspace.image_reference",
                "path" => path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image")
            )
        } else {
            let content = std::fs::read_to_string(&path).unwrap_or_else(
                |e| deepseekcode_i18n::tr!("workspace.unreadable", "error" => e.to_string()),
            );
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
            let removed = self.attachments.remove(index);
            if self
                .image_preview
                .as_ref()
                .is_some_and(|p| p.path == removed.path)
            {
                self.image_preview = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn open_image_preview(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(att) = self.attachments.get(index).cloned() {
            if att.is_image() {
                self.image_preview = Some(att);
                cx.notify();
            }
        }
    }

    pub(crate) fn close_image_preview(&mut self, cx: &mut Context<Self>) {
        self.image_preview = None;
        cx.notify();
    }

    pub(crate) fn focus_prompt_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.prompt_input.read(cx).focus_handle_clone();
        window.focus(&handle, cx);
        cx.notify();
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
                        deepseekcode_i18n::t("workspace.no_compaction_config")
                    }
                })
                .unwrap_or_else(|| deepseekcode_i18n::t("workspace.no_project_loaded"));
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
        // Providers were previously fetched only once at project bootstrap:
        // a transient failure (or opening Settings with no project) left the
        // AI section permanently empty. Refresh on every open instead.
        self.refresh_providers(cx);
        cx.notify();
    }

    /// Re-fetch the provider catalog (connection states + model lists).
    /// No project / unreachable server → keeps the previous list.
    fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            let Some(client) = client else {
                return;
            };
            let Ok(providers) = client.list_providers().await else {
                return;
            };
            let _ = view.update(cx, |this, cx| {
                this.providers = providers;
                this.apply_custom_models();
                cx.notify();
            });
        })
        .detach();
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

    /// Apply an arbitrary model id typed into Settings → Custom model
    /// (OpenRouter proxies any `author/model` id, so the catalog can never
    /// enumerate them all). Stored verbatim in preferences; the session
    /// update passes the id straight through to the provider.
    /// Add the id typed into Settings → Custom models to the user list
    /// (persisted in preferences, merged into every catalog fetch) and
    /// select it. Adding an id that already exists just selects it.
    pub(crate) fn add_custom_model(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let id = self.custom_model_input.read(cx).text().trim().to_string();
        if id.is_empty() {
            return;
        }
        let known = self
            .providers
            .iter()
            .any(|p| p.id == provider_id && p.models.iter().any(|m| m.id == id));
        if !known
            && !self
                .custom_models
                .iter()
                .any(|c| c.provider_id == provider_id && c.id == id)
        {
            self.custom_models.push(CustomModelEntry {
                provider_id: provider_id.to_string(),
                id: id.clone(),
                name: id.clone(),
            });
            self.persist_preferences();
            self.apply_custom_models();
            self.custom_model_input
                .update(cx, |input, cx| input.clear(cx));
        }
        self.set_model(&id, &id, provider_id, cx);
    }

    /// Remove one user-added model (persisted; catalog entries untouched).
    pub(crate) fn remove_custom_model(
        &mut self,
        provider_id: &str,
        model_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.custom_models
            .retain(|c| !(c.provider_id == provider_id && c.id == model_id));
        self.persist_preferences();
        for provider in &mut self.providers {
            if provider.id == provider_id {
                provider.models.retain(|m| m.id != model_id);
            }
        }
        cx.notify();
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
                    this.apply_custom_models();
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
            language: self.language,
            server_url: self.server_url.clone(),
            server_password: self.server_password.clone(),
            custom_models: self.custom_models.clone(),
        });
    }

    /// Merge user-added models into the server catalog (dedup by id).
    /// Called after every provider fetch so customs survive reconnects.
    fn apply_custom_models(&mut self) {
        for custom in &self.custom_models {
            if let Some(provider) = self
                .providers
                .iter_mut()
                .find(|p| p.id == custom.provider_id)
            {
                if !provider.models.iter().any(|m| m.id == custom.id) {
                    provider.models.push(ModelInfo {
                        id: custom.id.clone(),
                        name: custom.name.clone(),
                        provider_id: custom.provider_id.clone(),
                        context_window: None,
                        enabled: true,
                    });
                }
            }
        }
    }

    pub(crate) fn set_theme(&mut self, theme_id: ThemeId, cx: &mut Context<Self>) {
        self.theme = theme_id;
        crate::theme::DeepSeekCodeTheme::switch(cx, theme_id);
        self.persist_preferences();
        cx.notify();
    }

    pub(crate) fn set_language(
        &mut self,
        pref: deepseekcode_i18n::LanguagePref,
        cx: &mut Context<Self>,
    ) {
        // No-op when nothing changes, so a re-render never loops on itself.
        if self.language == pref && deepseekcode_i18n::resolve(pref) == deepseekcode_i18n::locale()
        {
            return;
        }
        self.language = pref;
        deepseekcode_i18n::set_preference(pref);
        self.persist_preferences();
        // Every visible string is read through the catalog at render time, so
        // a single notify repaints the whole shell in the new language.
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
                    let mut cmd = std::process::Command::new("cmd");
                    deepseekcode_core::process::hide_console(&mut cmd);
                    let _ = cmd.args(["/C", "start", "", &url]).spawn();
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
            self.composer_error = Some(deepseekcode_i18n::t("workspace.no_active_session"));
            cx.notify();
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
            let Some(client) = client else {
                let _ = view.update(cx, |this, cx| {
                    this.composer_error =
                        Some(deepseekcode_i18n::t("workspace.stop_unreachable"));
                    cx.notify();
                });
                return;
            };
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
                            run_id = %deepseekcode_telemetry::current_run_id(),
                        error = %e,
                        "move session failed"
                    );
                    let _ = view.update(cx, |this, cx| {
                        // P2 SH11: surface instead of silent log.
                        this.composer_error = Some(deepseekcode_i18n::tr!(
                            "workspace.delete_failed",
                            "error" => e.to_string()
                        ));
                        cx.notify();
                    });
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
        if index >= self.editor_tabs.len() {
            return;
        }
        self.sync_editor_buffer_from_input(cx);
        self.active_editor_tab = index;
        if let Some(path) = self.editor_tabs.get(index).cloned() {
            if self.editor_dirty.contains(&path) {
                if let Some(content) = self.editor_buffers.get(&path).cloned() {
                    self.load_editor_buffer(&path, content, cx);
                    cx.notify();
                    return;
                }
            }
            self.open_file(&path, cx);
        }
    }

    pub(crate) fn close_editor_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.editor_tabs.len() {
            return;
        }
        let removed = self.editor_tabs.remove(index);
        self.editor_buffers.remove(&removed);
        self.editor_dirty.remove(&removed);
        if self.editor_tabs.is_empty() {
            self.editor = EditorState::default();
            self.clear_editor_text_layouts();
            self.active_editor_tab = 0;
        } else {
            self.active_editor_tab = self.active_editor_tab.min(self.editor_tabs.len() - 1);
            if let Some(path) = self.editor_tabs.get(self.active_editor_tab).cloned() {
                if self.editor_dirty.contains(&path) {
                    if let Some(content) = self.editor_buffers.get(&path).cloned() {
                        self.load_editor_buffer(&path, content, cx);
                    }
                } else {
                    self.open_file(&path, cx);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn save_editor_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.editor.path.clone() else {
            return;
        };
        let Some(session_id) = self.session_id else {
            tracing::warn!(
                target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                "save file skipped: no active session"
            );
            // P2: surface instead of silently dropping the save.
            self.composer_error = Some(deepseekcode_i18n::t("workspace.no_active_session"));
            cx.notify();
            return;
        };
        // P2: debounce — ignore re-entrant saves while one is in flight
        // (previously rapid Ctrl+S spammed concurrent fs_write, last-writer-wins).
        if self.editor_saving {
            return;
        }
        self.editor_saving = true;
        let content = self.file_editor_input.read(cx).text();
        self.editor.set_content(content.clone());
        cx.notify();
        let client_slot = Arc::clone(&self.client);
        cx.spawn(async move |view, cx| {
            let client = lock_sdk_client(&client_slot).await;
            if let Some(client) = client {
                match client.fs_write(session_id, &path, &content).await {
                    Ok(()) => {
                        let path_for_clear = path.clone();
                        let content_for_buffer = content.clone();
                        let _ = view.update(cx, |this, cx| {
                            this.editor_saving = false;
                            this.editor_read_only = false;
                            this.editor_buffers
                                .insert(path_for_clear, content_for_buffer);
                            this.editor_dirty.remove(&path);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            error = %e,
                            "save file failed"
                        );
                        let _ = view.update(cx, |this, cx| {
                            this.editor_saving = false;
                            // P2: don't flip to read-only without telling the
                            // user why their buffer just locked.
                            this.editor_read_only = true;
                            this.composer_error = Some(deepseekcode_i18n::tr!(
                                "workspace.delete_failed",
                                "error" => e.to_string()
                            ));
                            cx.notify();
                        });
                    }
                }
            } else {
                let _ = view.update(cx, |this, cx| {
                    this.editor_saving = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Close the tab at a *visible* (session-filtered) position.
    pub(crate) fn close_terminal_tab(&mut self, visible: usize, cx: &mut Context<Self>) {
        let Some(&index) = self.visible_terminal_indices().get(visible) else {
            return;
        };
        self.remove_terminal_at_vec_index(index, cx);
    }

    fn remove_terminal_at_vec_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        let pty_id = self
            .terminals
            .get(index)
            .and_then(|tab| tab.state.pty_id.clone());
        self.terminals.remove(index);
        // Vec shifted left past `index`: keep tracking the same tab.
        if self.active_terminal > index {
            self.active_terminal -= 1;
        }
        self.fix_active_terminal_visible();
        if self.active_terminal >= self.terminals.len() {
            self.active_terminal = 0;
        }
        if let Some(pty_id) = pty_id {
            let client_slot = Arc::clone(&self.client);
            cx.spawn(async move |_view, _cx| {
                if let Some(client) = lock_sdk_client(&client_slot).await {
                    let _ = client.destroy_pty(&pty_id).await;
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// Drop the transcript tab for a finished command (transcript itself is
    /// kept, so click-to-reopen from the activity leaf still works).
    fn close_bash_terminal_for_detail(&mut self, detail_key: &str, cx: &mut Context<Self>) {
        if let Some(idx) = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        {
            self.remove_terminal_at_vec_index(idx, cx);
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
        // Clamp both scroll axes into the new bounds: after a panel-width
        // drag the old offset can point past the content (blank terminal,
        // thumb stuck at a wrong spot) until the user scrolls by hand.
        let offset = self.terminal_scroll.offset();
        let max = self.terminal_scroll.max_offset();
        let nx = f32::from(offset.x).clamp(-f32::from(max.x), 0.);
        let ny = f32::from(offset.y).clamp(-f32::from(max.y), 0.);
        if nx != f32::from(offset.x) || ny != f32::from(offset.y) {
            self.terminal_scroll.set_offset(point(px(nx), px(ny)));
        }
    }

    pub(crate) fn start_session_title_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_id.is_none() {
            return;
        }
        let title = self.current_session_title();
        self.session_title_input.update(cx, |input, cx| {
            input.set_text(title, cx);
        });
        self.session_title_editing = true;
        // P2 SH3: focus the input so Enter/Esc (SessionTitleSave/Cancel,
        // which require focus) work without a second click.
        let handle = self.session_title_input.read(cx).focus_handle_clone();
        window.focus(&handle, cx);
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
            // P1-4: refresh the receipt display gate from the same config
            // load (no extra IO) — accounting is unaffected.
            self.show_turn_usage = cfg.ui.show_turn_usage;
            self.selected_policy_row = None;
            self.permission_allow_input.update(cx, |input, cx| {
                input.set_text(policy_text, cx);
            });
            self.policy_pattern_input.update(cx, |input, cx| {
                input.set_placeholder(
                    deepseekcode_i18n::t("workspace.pattern_for_rule_ellipsis"),
                    cx,
                );
                input.set_text(String::new(), cx);
            });
            // Keep reject box as reject-only placeholder.
            self.permission_deny_input.update(cx, |input, cx| {
                input.set_placeholder(deepseekcode_i18n::t("policy.feedback_placeholder"), cx);
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
        self.policy_pattern_input.update(cx, |input, cx| {
            input.set_text(pattern, cx);
        });
        cx.notify();
    }

    fn apply_selected_policy_row_pattern(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected_policy_row else {
            return;
        };
        let pattern = self.policy_pattern_input.read(cx).text();
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
        // Source of truth is the row table (not the free-text box) so row
        // edits are never lost when the text box is stale.
        let policy = if self.policy_rule_rows.is_empty() {
            let policy_text = self.permission_allow_input.read(cx).text();
            deepseekcode_config::lines_to_policy(&policy_text)
        } else {
            use std::collections::HashMap;
            let mut policy = deepseekcode_config::PolicyConfig::new();
            for row in &self.policy_rule_rows {
                if row.pattern.trim().is_empty() {
                    continue;
                }
                if row.pattern.trim() == "*" {
                    policy.insert(
                        row.scope.clone(),
                        deepseekcode_config::PolicyScopeConfig::Simple(row.action),
                    );
                } else {
                    match policy.get_mut(&row.scope) {
                        Some(deepseekcode_config::PolicyScopeConfig::Patterns(map)) => {
                            map.insert(row.pattern.clone(), row.action);
                        }
                        _ => {
                            let mut map = HashMap::new();
                            map.insert(row.pattern.clone(), row.action);
                            // Preserve existing Simple `*` if present.
                            if let Some(deepseekcode_config::PolicyScopeConfig::Simple(prev)) =
                                policy.get(&row.scope)
                            {
                                map.insert("*".into(), *prev);
                            }
                            policy.insert(
                                row.scope.clone(),
                                deepseekcode_config::PolicyScopeConfig::Patterns(map),
                            );
                        }
                    }
                }
            }
            policy
        };
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
                        this.composer_error = Some(deepseekcode_i18n::tr!(
                            "workspace.policy_save_failed",
                            "error" => e.to_string()
                        ));
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
                                run_id = %deepseekcode_telemetry::current_run_id(),
                            provider_id = %provider_id,
                            "provider connected from desktop"
                        );
                        let providers = client.list_providers().await.unwrap_or_default();
                        let _ = view.update(cx, |this, cx| {
                            this.providers = providers.clone();
                            this.apply_custom_models();
                            let merged = this.providers.clone();
                            this.init_defaults_from_providers(&merged);
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
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
        if let Some(leaf_id) = self.pending_scroll_to_activity_leaf.take() {
            // Explicit navigation (todo jump, leaf click) wins over tail:
            // it also cancels a stale tail request from the same frame.
            self.pending_scroll_to_tail = false;
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
        } else if self.pending_scroll_to_tail || self.session_follow_tail {
            // Persistent pin, not one-shot: tree rebuilds / dock open-close
            // shift rows between frames, so a single scroll_to_bottom can land
            // stale (reads as "jumped to item top"). Re-enforcing every frame
            // while pinned self-heals; scrolling up clears the pin above.
            self.pending_scroll_to_tail = false;
            self.session_scroll.scroll_to_bottom();
        }
        if has_project && self.layout.right_panel_tab == RightPanelTab::Terminal {
            // NEVER auto-connect here: render runs on every notify, so a
            // failing shell reconnects in a hot loop (observed in the wild:
            // a fresh server PTY every ~70ms, tab flickering on
            // "Connecting..."). (Re)connection happens from event handlers
            // only: panel open, tab switch, `+`, hydrate shell guarantee.
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
                    return;
                }
                // Enter inside the Connect dialog confirms it.
                if this.api_key_dialog.is_some()
                    && (this
                        .api_key_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                        || this
                            .base_url_input
                            .read(cx)
                            .focus_handle(cx)
                            .is_focused(window))
                {
                    this.confirm_api_key(cx);
                    return;
                }
                // Enter inside Settings → Custom model adds + selects it.
                if this
                    .custom_model_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    let provider = match this.selected_provider.as_deref() {
                        Some("openrouter") | Some("openai-compatible") => {
                            this.selected_provider.clone().unwrap_or_default()
                        }
                        _ => "openrouter".to_string(),
                    };
                    this.add_custom_model(&provider, cx);
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
            .on_action(
                cx.listener(|this, _: &crate::git_panel::GitCommitSubmit, window, cx| {
                    if this
                        .git_commit_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                    {
                        this.git_commit(false, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::git_panel::GitBranchSubmit, window, cx| {
                    if this
                        .git_branch_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                    {
                        this.git_create_branch_from_input(cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::git_panel::GitStashSubmit, window, cx| {
                    if this
                        .git_stash_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                    {
                        this.git_stash_push_from_input(cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::git_panel::GitReviewSubmit, window, cx| {
                    if this
                        .git_review_scope_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                    {
                        this.git_find_issues(cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &QuestionCancel, _window, cx| {
                // P0-7: Esc always exits the question modal.
                if this.pending_question.is_some() {
                    this.cancel_pending_question(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _window, cx| {
                // P2: Esc closes the topmost overlay (priority order).
                if this.image_preview.is_some() {
                    this.close_image_preview(cx);
                } else if this.api_key_dialog.is_some() {
                    this.cancel_api_key_dialog(cx);
                } else if this.pending_question.is_some() {
                    this.cancel_pending_question(cx);
                } else if this.pending_plan_confirm.is_some() {
                    this.cancel_plan_implement_confirm(cx);
                } else if this.context_usage.expanded {
                    this.close_context_usage_detail(cx);
                } else {
                    this.close_popover(cx);
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
                self.language,
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
                self.policy_pattern_input.clone(),
                self.custom_model_input.clone(),
                &self.custom_models,
                self.project_dir.is_some(),
            ))
            .children(self.render_api_key_dialog(cx))
            .child(RevertDock::render_overlay(
                cx,
                &self.revert_dock,
                self.selected_message_id,
            ))
            .child(self.render_popover_overlay(cx))
            .child(self.render_image_preview_overlay(cx))
    }
}

impl WorkspaceView {
    fn render_shell(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.project_dir.is_some();
        let recent = self.merged_sidebar_recents(cx);
        let project_nav = self.filter_project_nav();
        let project_path = self.project_dir.as_ref().map(|p| p.display().to_string());
        // Live markdown source for preview: dirty buffers win so typing in
        // Code reflects immediately; fall back to loaded content.
        let editor_preview_text = self
            .editor
            .path
            .as_ref()
            .and_then(|p| self.editor_buffers.get(p))
            .cloned()
            .unwrap_or_else(|| self.editor.content.clone());
        // Save only shows once the current file is modified.
        let editor_dirty_now = self
            .editor
            .path
            .as_ref()
            .is_some_and(|p| self.editor_dirty.contains(p));

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
                        editor_dirty_now,
                        &editor_preview_text,
                        &self.terminals,
                        self.active_terminal,
                        self.session_id,
                        &self.terminal_focus,
                        &self.terminal_scroll,
                        &self.bash_transcripts,
                        "",
                        true,
                        self,
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
                            .child(deepseekcode_i18n::t("app.name")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(deepseekcode_i18n::t("workspace.no_project")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .child(deepseekcode_i18n::t("app.tagline")),
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
                            self.selected_detail_key.as_deref(),
                            &self.session_scroll,
                            self.session_status.as_ref() == "Loading session…",
                            self.session_busy,
                            &self.timeline_text_focus,
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
                let inflight = self.pending_policy_reply.contains(&prompt.id);
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
                        inflight,
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
            // P0-8: overwrite confirm — shown when Implement is clicked with a
            // non-empty draft. Append / Replace / Cancel, no silent clobber.
            .children(if self.pending_plan_confirm.is_some() {
                let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
                vec![div()
                    .flex_shrink_0()
                    .px_4()
                    .pb_2()
                    .child(
                        div()
                            .p_3()
                            .rounded_lg()
                            .bg(theme.bg_elevated)
                            .border_1()
                            .border_color(theme.warning)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_primary)
                                    .child(deepseekcode_i18n::t("plan.confirm_overwrite")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        div()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .text_xs()
                                            .text_color(theme.text_muted)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.cancel_plan_implement_confirm(cx);
                                                }),
                                            )
                                            .child(deepseekcode_i18n::t("common.cancel")),
                                    )
                                    .child(
                                        div()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .bg(theme.bg_surface)
                                            .border_1()
                                            .border_color(theme.border_default)
                                            .text_xs()
                                            .text_color(theme.text_primary)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.confirm_plan_implement_append(cx);
                                                }),
                                            )
                                            .child(deepseekcode_i18n::t("plan.append")),
                                    )
                                    .child(
                                        div()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .bg(theme.error)
                                            .text_xs()
                                            .text_color(theme.text_on_accent)
                                            .cursor_pointer()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.confirm_plan_implement_replace(cx);
                                                }),
                                            )
                                            .child(deepseekcode_i18n::t("plan.replace")),
                                    ),
                            ),
                    )
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
                        self.stopping,
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
            render_dismiss_layer, render_menu_panel, render_project_remove_confirm,
            render_project_remove_row, render_session_delete_confirm, render_session_delete_row,
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

        // P0-6: pending confirms take priority over the raw menus.
        if let Some(session_id) = self.pending_session_delete {
            let menu = render_session_delete_confirm(session_id, anchor, cx);
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        } else if let Some(path) = self.pending_project_remove.clone() {
            let menu = render_project_remove_confirm(path, anchor, cx);
            layer = div()
                .id("popover-overlay")
                .absolute()
                .inset_0()
                .child(render_dismiss_layer(cx))
                .child(menu)
                .into_any_element();
        } else if let Some(session_id) = self.session_context_menu {
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

    fn render_image_preview_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(att) = self.image_preview.clone() else {
            return div().into_any_element();
        };
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let file_name = att.file_name();
        let image_el: AnyElement = match crate::composer::load_attachment_image(&att.path) {
            Some(image) => img(image)
                .max_w(px(720.))
                .max_h(px(520.))
                .rounded_md()
                .into_any_element(),
            None => div()
                .px_6()
                .py_8()
                .text_sm()
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::tr!(
                    "workspace.image_preview_failed",
                    "file_name" => file_name.as_str()
                ))
                .into_any_element(),
        };
        div()
            .id("image-preview-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_image_preview(cx);
                }),
            )
            .child(
                div()
                    .id("image-preview-panel")
                    .relative()
                    .max_w(px(760.))
                    .rounded_lg()
                    .bg(theme.bg_surface)
                    .border_1()
                    .border_color(theme.border_default)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .overflow_hidden()
                                    .child(file_name),
                            )
                            .child(
                                div()
                                    .id("image-preview-close")
                                    .flex_shrink_0()
                                    .size(px(28.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| crate::git_panel::GitFileTooltip {
                                            text: deepseekcode_i18n::t("workspace.close_preview")
                                                .into(),
                                        })
                                        .into()
                                    })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.close_image_preview(cx);
                                        }),
                                    )
                                    .child(crate::ui::icon::render_icon_rgba(
                                        crate::ui::icon::IconName::X,
                                        px(14.),
                                        theme.text_secondary,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(theme.bg_elevated)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .p_3()
                            .child(image_el),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(att.path.clone()),
                    ),
            )
            .into_any_element()
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
                    .child(deepseekcode_i18n::t("workspace.question")),
            )
            .child(
                div()
                    .text_xs()
                    .mt_2()
                    .text_color(theme.text_secondary)
                    .child(question),
            );
        if options.is_empty() {
            let empty_hint = self.composer_error.clone().filter(|e| e.contains("answer"));
            panel = panel
                .child(div().mt_3().child(self.question_input.clone()))
                .when(empty_hint.is_some(), |el| {
                    el.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme.error)
                            .child(deepseekcode_i18n::t("workspace.question_empty")),
                    )
                })
                .child(
                    div().flex().gap_2().mt_3().justify_end()
                        .child(
                            div()
                                .id("question-cancel")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.bg_hover))
                                .tooltip(move |_, cx| {
                                    cx.new(|_| crate::git_panel::GitFileTooltip {
                                        text: deepseekcode_i18n::t("workspace.question_cancel").into(),
                                    })
                                    .into()
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.cancel_pending_question(cx);
                                    }),
                                )
                                .child(deepseekcode_i18n::t("common.cancel")),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(theme.info)
                                .text_xs()
                                .text_color(theme.text_on_accent)
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.accent_hover))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        let answer = this.question_input.read(cx).text();
                                        if answer.trim().is_empty() {
                                            // P1-4: visible validation instead of
                                            // silent return ("button looks broken").
                                            this.composer_error = Some(
                                                deepseekcode_i18n::t("workspace.question_empty"),
                                            );
                                            cx.notify();
                                        } else {
                                            this.submit_question_answer(answer, cx);
                                        }
                                    }),
                                )
                                .child(deepseekcode_i18n::t("common.submit")),
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
                        .hover(|s| s.bg(theme.accent_hover))
                        .on_mouse_down(MouseButton::Left, {
                            let opt = opt.clone();
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
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
                        .child(deepseekcode_i18n::t("workspace.other_input")),
                );
                panel = panel.child(opts).child(
                    div().mt_3().child(self.question_input.clone()).child(
                        div().flex().gap_2().mt_3().justify_end()
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.cancel_pending_question(cx);
                                        }),
                                    )
                                    .child(deepseekcode_i18n::t("common.cancel")),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(theme.info)
                                    .text_xs()
                                    .text_color(theme.text_on_accent)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.accent_hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            let answer = this.question_input.read(cx).text();
                                            if answer.trim().is_empty() {
                                                this.composer_error = Some(
                                                    deepseekcode_i18n::t("workspace.question_empty"),
                                                );
                                                cx.notify();
                                            } else {
                                                this.submit_question_answer(answer, cx);
                                            }
                                        }),
                                    )
                                    .child(deepseekcode_i18n::t("common.submit")),
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
            .occlude()
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
            .occlude()
            // Translucent scrim (not opaque `bg_overlay`): the page behind —
            // including the Settings panel this dialog was opened from —
            // stays visible around the popup.
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            // Swallow clicks so they never reach the Settings panel or the
            // timeline behind (same fall-through bug as the settings overlay).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(px(400.))
                    .p_4()
                    .rounded_lg()
                    // `bg_surface` contrasts with the inputs' `bg_elevated`
                    // so the text fields are actually visible as boxes.
                    .bg(theme.bg_surface)
                    .border_1()
                    .border_color(theme.border_default)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(deepseekcode_i18n::tr!(
                                "workspace.connect_named",
                                "provider" => dialog.provider_name.as_str()
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .mt_2()
                            .text_color(theme.text_muted)
                            .child(deepseekcode_i18n::t("workspace.keychain_note")),
                    )
                    .child(
                        div()
                            .mt_3()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border_default)
                            .bg(theme.bg_elevated)
                            .px_2()
                            .child(self.api_key_input.clone()),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .child(deepseekcode_i18n::t("workspace.base_url_optional")),
                    )
                    .child(
                        div()
                            .mt_1()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border_default)
                            .bg(theme.bg_elevated)
                            .px_2()
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
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.cancel_api_key_dialog(cx);
                                        }),
                                    )
                                    .child(deepseekcode_i18n::t("common.cancel")),
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
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_api_key(cx);
                                        }),
                                    )
                                    .child(deepseekcode_i18n::t("common.connect")),
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
        // Guard against landing in a different session: projections belong
        // to the session that requested them.
        let parent_session = self.session_id;
        let generation = self.child_projection_generation.load(Ordering::SeqCst);
        let generation_slot = Arc::clone(&self.child_projection_generation);
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
                    if generation_slot.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if this.session_id != parent_session {
                        return;
                    }
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
        if self.editor_dirty.contains(&path) {
            if let Some(content) = self.editor_buffers.get(&path).cloned() {
                self.editor.scroll_to_line = line;
                self.editor.diff_stats = diff_stats;
                self.load_editor_buffer(&path, content, cx);
                self.layout.show_right_panel = true;
                cx.notify();
                return;
            }
        }
        let snapshot_id = self
            .revert_staged_files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.snapshot_id.clone());
        self.open_generation = self.open_generation.wrapping_add(1);
        let generation = self.open_generation;
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
                if generation != this.open_generation {
                    return;
                }
                if let Some((content, diff)) = result {
                    let text = content.unwrap_or_default();
                    this.editor.path = Some(path.clone());
                    this.editor.md_preview = false;
                    this.clear_editor_text_layouts();
                    this.editor.set_content(text.clone());
                    this.editor.diff_hunks = diff;
                    this.editor.diff_stats = diff_stats;
                    this.editor.scroll_to_line = line;
                    this.editor_buffers.insert(path.clone(), text.clone());
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
                    this.editor_read_only = false;
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
            let result = if let Some(dir) = project_dir {
                let dir_str = dir.to_string_lossy().to_string();
                let path_for_git = path.clone();
                tokio::task::spawn_blocking(move || {
                    let content =
                        git_worktree_file_content(&dir_str, &path_for_git).unwrap_or_default();
                    let diff = git_unified_diff(&dir_str, &path_for_git);
                    (content, diff)
                })
                .await
                .ok()
            } else {
                None
            };
            let _ = view.update(cx, |this, cx| {
                if let Some((content, diff)) = result {
                    this.editor.path = Some(path.clone());
                    this.editor.md_preview = false;
                    this.clear_editor_text_layouts();
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

    pub(crate) fn refresh_git_status_async(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.project_dir.clone() else {
            self.clear_git_status();
            cx.notify();
            return;
        };
        if self.git_busy {
            return;
        }
        self.git_busy = true;
        cx.notify();
        let limit = self.git_graph_limit;
        cx.spawn(async move |view, cx| {
            let status = tokio::task::spawn_blocking(move || collect_git_status(&dir, limit))
                .await
                .ok();
            if let Some(status) = status {
                let _ = view.update(cx, |this, cx| {
                    this.git_busy = false;
                    this.apply_git_status(status);
                    // Seed the review-scope input from the detected default
                    // branch on first load (input still shows "master").
                    if this.git_review_scope == "master" {
                        let detected = this.git_default_branch();
                        if detected != "master" {
                            this.git_review_scope = detected.clone();
                            this.git_review_scope_input.update(cx, |input, cx| {
                                input.set_text(detected, cx);
                            });
                        }
                    }
                    // Cursor-style commit placeholder names the target branch.
                    // Placeholder-only: never touches the typed message.
                    let branch = this.git_current_branch.clone();
                    this.git_commit_input.update(cx, |input, cx| {
                        input.set_placeholder(commit_placeholder(branch.as_deref()), cx);
                    });
                    cx.notify();
                });
            } else {
                let _ = view.update(cx, |this, cx| {
                    this.git_busy = false;
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
        self.git_conflicted.clear();
        self.git_diff_summary = None;
        self.git_status_message = None;
        self.git_log.clear();
        self.git_branches.clear();
        self.git_commits.clear();
        self.git_stashes.clear();
        self.git_remotes.clear();
        self.git_tags.clear();
        self.git_current_branch = None;
        self.git_head_sha = None;
        self.git_ahead = 0;
        self.git_behind = 0;
        self.git_error = None;
    }

    fn apply_git_status(&mut self, status: GitStatusSnapshot) {
        self.git_staged = status.staged;
        self.git_unstaged = status.unstaged;
        self.git_untracked = status.untracked;
        self.git_conflicted = status.conflicted;
        self.git_diff_summary = status.diff_summary;
        self.git_status_message = status.status_message;
        self.git_log = status.log;
        self.git_branches = status.branches;
        self.git_commits = status.commits;
        self.git_stashes = status.stashes;
        self.git_remotes = status.remotes;
        self.git_tags = status.tags;
        self.git_current_branch = status.current_branch;
        self.git_head_sha = status.head_sha;
        self.git_ahead = status.ahead;
        self.git_behind = status.behind;
        // Diffs changed: drop cached hunks (expanded state stays so rows
        // re-expand with fresh content on next toggle).
        self.git_hunks.clear();
        self.git_hunks_loading.clear();
        // Prune selection of files that are no longer changed.
        if !self.git_selected.is_empty() {
            let mut alive = HashSet::new();
            for e in self
                .git_staged
                .iter()
                .chain(self.git_unstaged.iter())
                .chain(self.git_untracked.iter())
                .chain(self.git_conflicted.iter())
            {
                if self.git_selected.contains(&e.path) {
                    alive.insert(e.path.clone());
                }
            }
            self.git_selected = alive;
        }
        // Hover is transient: a refresh rebuilds rows, so drop the overlay key
        // rather than pointing at a stale row.
        self.git_hovered_row = None;
        self.sync_file_tree_git_badges();
    }

    fn sync_file_tree_git_badges(&mut self) {
        use crate::file_tree::GitFileBadge;
        use std::collections::HashMap;
        let mut badges = HashMap::new();
        for entry in &self.git_untracked {
            badges.insert(entry.path.clone(), GitFileBadge::Untracked);
        }
        for entry in &self.git_conflicted {
            badges.insert(entry.path.clone(), GitFileBadge::Modified);
        }
        for entry in &self.git_unstaged {
            badges.insert(entry.path.clone(), GitFileBadge::from_git_raw(&entry.raw));
        }
        for entry in &self.git_staged {
            badges.insert(entry.path.clone(), GitFileBadge::from_git_raw(&entry.raw));
        }
        self.file_tree.git_badges = badges;
    }

    fn git_repo_dir(&self) -> Option<PathBuf> {
        self.project_dir.clone()
    }

    fn set_git_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.git_error = error;
        self.git_error_detail = None;
        self.git_notice = None;
        cx.notify();
    }

    /// Split a failed op's output into a one-line verdict + full detail.
    /// Hook dumps (husky/lint-staged/eslint) are multi-KB; the verdict keeps
    /// the panel answerable ("failed, nothing committed") while the detail
    /// stays one click away instead of flooding the sidebar.
    fn set_git_failure(&mut self, err_title: Option<String>, raw: String) {
        let first = raw
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>();
        let title = err_title.unwrap_or(first);
        let title = if title.trim().is_empty() {
            deepseekcode_i18n::t("git.operation_failed")
                .replace("{error}", "unknown error")
        } else {
            title
        };
        let detail = if raw.trim().lines().count() > 1 {
            Some(truncate_text(raw.trim(), 8000))
        } else {
            None
        };
        self.git_error = Some(title);
        self.git_error_detail = detail;
        self.git_error_expanded = true;
        self.git_notice = None;
    }

    /// Heuristic: did this failure come from a git hook (husky/lint-staged/
    /// eslint/prettier/commitlint)? Used to offer "retry without verify".
    pub(crate) fn git_error_looks_like_hook(&self) -> bool {
        const MARKERS: &[&str] = &[
            "lint-staged",
            "husky",
            "eslint",
            "prettier",
            "commitlint",
            ".husky",
            "githook",
            "hook",
        ];
        let mut haystack = self.git_error.clone().unwrap_or_default();
        if let Some(detail) = self.git_error_detail.clone() {
            haystack.push('\n');
            haystack.push_str(&detail);
        }
        let haystack = haystack.to_lowercase();
        MARKERS.iter().any(|m| haystack.contains(m))
    }

    fn set_git_notice(&mut self, notice: impl Into<String>, cx: &mut Context<Self>) {
        self.git_notice = Some(notice.into());
        self.git_error = None;
        cx.notify();
    }

    fn run_git_op(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(PathBuf) -> std::result::Result<String, String> + Send + 'static,
        ok_notice: Option<String>,
    ) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        // Concurrency guard: never run two git mutations at once (the index
        // lock would fail one of them). Buttons also render inert while
        // `git_busy`, so this is a backstop, not the primary UX.
        if self.git_busy {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                "run_git_op dropped: another git op still in flight"
            );
            return;
        }
        self.git_busy = true;
        self.git_error = None;
        self.git_error_detail = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = tokio::task::spawn_blocking(move || op(dir))
                .await
                .ok()
                .unwrap_or_else(|| Err("git task cancelled".to_string()));
            let _ = view.update(cx, |this, cx| {
                this.git_busy = false;
                // Commit ships a verdict title + clears the box on success
                // only (a rejected commit keeps the typed message for retry).
                let commit_ui = this.git_pending_commit_ui.take();
                match result {
                    Ok(out) => {
                        if let Some(notice) = ok_notice {
                            this.git_notice = Some(notice);
                        } else if !out.trim().is_empty() {
                            this.git_notice = Some(out.trim().to_string());
                        } else {
                            this.git_notice = Some(deepseekcode_i18n::t("git.operation_ok"));
                        }
                        this.git_error = None;
                        this.git_error_detail = None;
                        if commit_ui.is_some() {
                            this.git_commit_input.update(cx, |input, cx| input.clear(cx));
                            this.git_commit_menu_open = false;
                        }
                    }
                    Err(e) => {
                        this.set_git_failure(commit_ui, e);
                    }
                }
                cx.notify();
            });
            let _ = view.update(cx, |this, cx| {
                this.refresh_git_status_async(cx);
            });
        })
        .detach();
    }

    // -- Staging (P0) ---------------------------------------------------------
    pub(crate) fn git_stage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::stage_files(&dir, &paths).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_unstage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::unstage_files(&dir, &paths).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_discard_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        // Untracked deletions go through `clean`; tracked through `restore`.
        let untracked: Vec<String> = paths
            .iter()
            .filter(|p| self.git_untracked.iter().any(|u| &u.path == *p))
            .cloned()
            .collect();
        let tracked: Vec<String> = paths
            .into_iter()
            .filter(|p| !untracked.contains(p))
            .collect();
        self.run_git_op(
            cx,
            move |dir| {
                if !tracked.is_empty() {
                    deepseekcode_core::git::ops::discard_files(&dir, &tracked)
                        .map_err(|e| e.to_string())?;
                }
                if !untracked.is_empty() {
                    deepseekcode_core::git::ops::clean_untracked(&dir, &untracked)
                        .map_err(|e| e.to_string())?;
                }
                Ok(String::new())
            },
            None,
        );
    }

    pub(crate) fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::stage_all(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    pub(crate) fn git_unstage_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::unstage_all(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    pub(crate) fn git_discard_all(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::discard_all(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    fn expand_selection(&self, paths: Vec<String>) -> Vec<String> {
        if paths.is_empty() && !self.git_selected.is_empty() {
            self.git_selected.iter().cloned().collect()
        } else {
            paths
        }
    }

    pub(crate) fn git_toggle_select(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.git_selected.contains(path) {
            self.git_selected.remove(path);
        } else {
            self.git_selected.insert(path.to_string());
        }
        cx.notify();
    }

    pub(crate) fn git_clear_selection(&mut self, cx: &mut Context<Self>) {
        self.git_selected.clear();
        cx.notify();
    }

    pub(crate) fn git_set_hovered_row(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.git_hovered_row != key {
            self.git_hovered_row = key;
            cx.notify();
        }
    }

    pub(crate) fn git_toggle_tree(&mut self, cx: &mut Context<Self>) {
        self.git_show_tree = !self.git_show_tree;
        cx.notify();
    }

    // -- Destructive confirm flow (P5 safety) ----------------------------------
    pub(crate) fn git_request_discard_all(&mut self, cx: &mut Context<Self>) {
        self.git_pending_confirm = Some(GitConfirm {
            title: deepseekcode_i18n::t("git.discard_all"),
            detail: deepseekcode_i18n::t("git.confirm_discard_all"),
            action: GitConfirmAction::DiscardAll,
        });
        cx.notify();
    }

    pub(crate) fn git_request_discard_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        if paths.is_empty() {
            return;
        }
        let detail = if paths.len() == 1 {
            deepseekcode_i18n::tr!("git.confirm_discard", "path" => paths[0].clone())
        } else {
            deepseekcode_i18n::t("git.confirm_discard_all")
        };
        self.git_pending_confirm = Some(GitConfirm {
            title: deepseekcode_i18n::t("git.discard"),
            detail,
            action: GitConfirmAction::DiscardPaths(paths),
        });
        cx.notify();
    }

    pub(crate) fn git_request_delete_branch(
        &mut self,
        name: &str,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let name = name.to_string();
        self.git_pending_confirm = Some(GitConfirm {
            title: deepseekcode_i18n::t("git.delete_branch"),
            detail: deepseekcode_i18n::tr!("git.confirm_delete_branch", "branch" => name.clone()),
            action: GitConfirmAction::DeleteBranch { name, force },
        });
        cx.notify();
    }

    pub(crate) fn git_request_drop_stash(&mut self, ref_: &str, cx: &mut Context<Self>) {
        let ref_ = ref_.to_string();
        self.git_pending_confirm = Some(GitConfirm {
            title: deepseekcode_i18n::t("git.stash_drop"),
            detail: deepseekcode_i18n::tr!("git.confirm_drop_stash", "ref" => ref_.clone()),
            action: GitConfirmAction::DropStash { ref_name: ref_ },
        });
        cx.notify();
    }

    pub(crate) fn git_request_force_push(&mut self, cx: &mut Context<Self>) {
        self.git_pending_confirm = Some(GitConfirm {
            title: deepseekcode_i18n::t("git.push"),
            detail: deepseekcode_i18n::t("git.confirm_force_push"),
            action: GitConfirmAction::ForcePush,
        });
        cx.notify();
    }

    pub(crate) fn git_cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.git_pending_confirm = None;
        cx.notify();
    }

    pub(crate) fn git_confirm_pending(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.git_pending_confirm.take() else {
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                "git_confirm_pending: nothing pending"
            );
            return;
        };
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            action = ?confirm.action,
            "git_confirm_pending confirmed"
        );
        cx.notify();
        match confirm.action {
            GitConfirmAction::DiscardAll => self.git_discard_all(cx),
            GitConfirmAction::DiscardPaths(paths) => {
                // Paths were resolved at request time; execute directly.
                self.git_discard_paths(paths, cx);
            }
            GitConfirmAction::DeleteBranch { name, force } => {
                self.git_delete_branch(&name, force, cx)
            }
            GitConfirmAction::DropStash { ref_name } => self.git_stash_drop(&ref_name, cx),
            GitConfirmAction::ForcePush => self.run_git_op(
                cx,
                |dir| {
                    deepseekcode_core::git::ops::push_force_with_lease(&dir)
                        .map_err(|e| e.to_string())
                },
                None,
            ),
        }
    }

    /// Append paths to the repo-root `.gitignore` (one per line, deduped).
    pub(crate) fn git_ignore_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        if paths.is_empty() {
            return;
        }
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        // Best-effort file edit, then refresh: untracked entries disappear
        // once ignored (subject to `status.showUntrackedFiles`).
        let ignore_path = dir.join(".gitignore");
        let existing = std::fs::read_to_string(&ignore_path).unwrap_or_default();
        let mut lines: Vec<String> = existing.lines().map(String::from).collect();
        for p in &paths {
            if !lines.iter().any(|l| l.trim() == p) {
                lines.push(p.clone());
            }
        }
        match std::fs::write(&ignore_path, lines.join("\n") + "\n") {
            Ok(()) => self.set_git_notice(deepseekcode_i18n::t("git.operation_ok"), cx),
            Err(e) => self.set_git_error(Some(e.to_string()), cx),
        }
        self.refresh_git_status_async(cx);
    }

    /// Reveal a repo-relative path in the OS file explorer.
    pub(crate) fn git_reveal_in_explorer(&self, path: &str) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        let full = dir.join(path);
        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("explorer");
            deepseekcode_core::process::hide_console(&mut cmd);
            let _ = cmd.arg("/select,").arg(&full).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(&full)
                .spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let parent = full.parent().map(|p| p.to_path_buf()).unwrap_or(dir);
            let _ = std::process::Command::new("xdg-open").arg(&parent).spawn();
        }
    }

    /// Default review/compare branch: prefer `origin/HEAD`, then local
    /// `main`/`master`, then first remote-tracking branch.
    pub(crate) fn git_default_branch(&self) -> String {
        for b in &self.git_branches {
            if b.is_remote && (b.name == "origin/HEAD" || b.name.ends_with("/HEAD")) {
                if let Some(target) = b.upstream.clone().or_else(|| {
                    // `origin/HEAD -> origin/main` shows up in last_commit text.
                    b.last_commit
                        .split("->")
                        .nth(1)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                }) {
                    return target
                        .strip_prefix("origin/")
                        .unwrap_or(&target)
                        .to_string();
                }
            }
        }
        for want in ["main", "master"] {
            if self
                .git_branches
                .iter()
                .any(|b| !b.is_remote && b.name == want)
            {
                return want.to_string();
            }
        }
        for b in &self.git_branches {
            if b.is_remote && b.name.starts_with("origin/") {
                return b.name["origin/".len()..].to_string();
            }
        }
        if self.git_review_scope.trim().is_empty() {
            "master".to_string()
        } else {
            self.git_review_scope.clone()
        }
    }

    /// Sync the review-scope text input into `git_review_scope`.
    pub(crate) fn git_sync_review_scope_from_input(&mut self, cx: &mut Context<Self>) {
        let scope = self
            .git_review_scope_input
            .read(cx)
            .text()
            .trim()
            .to_string();
        if !scope.is_empty() {
            self.git_review_scope = scope;
        } else {
            self.git_review_scope = self.git_default_branch();
        }
        cx.notify();
    }

    // -- Hunks (P1) -------------------------------------------------------------
    pub(crate) fn git_toggle_hunks(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.git_hunks_expanded.contains(path) {
            self.git_hunks_expanded.remove(path);
            cx.notify();
            return;
        }
        self.git_hunks_expanded.insert(path.to_string());
        // Load on first expand; cached afterwards until next refresh clears it.
        if !self.git_hunks.contains_key(path) {
            self.git_load_hunks(path, cx);
        } else {
            cx.notify();
        }
    }

    fn git_load_hunks(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        if self.git_hunks_loading.contains(path) {
            return;
        }
        self.git_hunks_loading.insert(path.to_string());
        cx.notify();
        let path_owned = path.to_string();
        let path_inner = path_owned.clone();
        cx.spawn(async move |view, cx| {
            let hunks = tokio::task::spawn_blocking(move || {
                deepseekcode_core::git::ops::diff_for_path(&dir, &path_inner)
                    .map(|d| crate::logic::split_diff_hunks(&d))
                    .unwrap_or_default()
            })
            .await
            .ok()
            .unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.git_hunks_loading.remove(&path_owned);
                this.git_hunks.insert(path_owned, hunks);
                cx.notify();
            });
        })
        .detach();
    }

    // -- Commit (P0) ----------------------------------------------------------

    pub(crate) fn git_commit(&mut self, and_push: bool, cx: &mut Context<Self>) {
        let message = self.git_commit_input.read(cx).text().trim().to_string();
        if message.is_empty() {
            self.set_git_error(Some(deepseekcode_i18n::t("git.commit_empty_error")), cx);
            return;
        }
        // Smart commit (VSCode/Cursor parity): the REAL index is checked at
        // commit time — UI state can lag behind external git tools, and a
        // stale "staged non-empty" would send an empty index into hooks like
        // lint-staged ("could not find any staged files"). Empty index +
        // dirty tree => stage everything first; clean tree => clear error.
        let no_verify = self.git_commit_no_verify;
        self.git_commit_menu_open = false;
        // Verdict title consumed on completion; the box clears on success
        // only so a hook-rejected commit keeps its message for retry.
        self.git_pending_commit_ui = Some(deepseekcode_i18n::t("git.commit_failed"));
        self.run_git_op(
            cx,
            move |dir| {
                use deepseekcode_core::git::ops as gitops;
                let staged_now = gitops::staged_names(&dir).map_err(|e| e.to_string())?;
                if staged_now.is_empty() {
                    if !gitops::has_uncommitted_changes(&dir).map_err(|e| e.to_string())? {
                        return Err(deepseekcode_i18n::t("git.no_changes"));
                    }
                    gitops::stage_all(&dir).map_err(|e| e.to_string())?;
                }
                gitops::commit(&dir, &message, no_verify)
                    .map_err(|e| e.to_string())
                    .and_then(|out| {
                        if and_push {
                            deepseekcode_core::git::ops::push(&dir)
                                .map_err(|e| e.to_string())
                                .map(|p| format!("{out}\n{p}"))
                        } else {
                            Ok(out)
                        }
                    })
            },
            None,
        );
    }

    pub(crate) fn git_amend(&mut self, cx: &mut Context<Self>) {
        let message = self.git_commit_input.read(cx).text().trim().to_string();
        let message_opt = if message.is_empty() {
            None
        } else {
            Some(message)
        };
        let no_verify = self.git_commit_no_verify;
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::commit_amend(&dir, message_opt.as_deref(), no_verify)
                    .map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_undo_last_commit(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::undo_last_commit(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    // -- Branch / sync (P2) ----------------------------------------------------
    pub(crate) fn git_checkout_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::checkout_branch(&dir, &name).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_create_branch_from_input(&mut self, cx: &mut Context<Self>) {
        let name = self.git_branch_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        let input = self.git_branch_input.clone();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::checkout_new_branch(&dir, &name, None)
                    .map_err(|e| e.to_string())
            },
            None,
        );
        input.update(cx, |i, cx| i.clear(cx));
    }

    pub(crate) fn git_delete_branch(&mut self, name: &str, force: bool, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::delete_branch(&dir, &name, force)
                    .map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_fetch(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::fetch_all(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    pub(crate) fn git_pull(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::pull_rebase(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    pub(crate) fn git_push(&mut self, cx: &mut Context<Self>) {
        self.run_git_op(
            cx,
            |dir| deepseekcode_core::git::ops::push(&dir).map_err(|e| e.to_string()),
            None,
        );
    }

    pub(crate) fn git_publish_current(&mut self, cx: &mut Context<Self>) {
        let (branch, remote) = (
            self.git_current_branch.clone().unwrap_or_default(),
            self.git_remotes
                .first()
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "origin".to_string()),
        );
        if branch.is_empty() {
            return;
        }
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::push_upstream(&dir, &remote, &branch)
                    .map_err(|e| e.to_string())
            },
            None,
        );
    }

    // -- Stash (P3) ------------------------------------------------------------
    pub(crate) fn git_stash_push_from_input(&mut self, cx: &mut Context<Self>) {
        let message = self.git_stash_input.read(cx).text().trim().to_string();
        let include = self.git_include_untracked_stash;
        let input = self.git_stash_input.clone();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::stash_push(
                    &dir,
                    if message.is_empty() {
                        None
                    } else {
                        Some(message.as_str())
                    },
                    include,
                )
                .map_err(|e| e.to_string())
            },
            None,
        );
        input.update(cx, |i, cx| i.clear(cx));
    }

    pub(crate) fn git_stash_pop(&mut self, ref_: &str, cx: &mut Context<Self>) {
        let ref_ = ref_.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::stash_pop(&dir, &ref_).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_stash_apply(&mut self, ref_: &str, cx: &mut Context<Self>) {
        let ref_ = ref_.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::stash_apply(&dir, &ref_).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_stash_drop(&mut self, ref_: &str, cx: &mut Context<Self>) {
        let ref_ = ref_.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::stash_drop(&dir, &ref_).map_err(|e| e.to_string())
            },
            None,
        );
    }

    // -- Conflicts (P1) ---------------------------------------------------------
    pub(crate) fn git_resolve(&mut self, ours: bool, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths = self.expand_selection(paths);
        self.run_git_op(
            cx,
            move |dir| {
                if ours {
                    deepseekcode_core::git::ops::resolve_with_ours(&dir, &paths)
                } else {
                    deepseekcode_core::git::ops::resolve_with_theirs(&dir, &paths)
                }
                .map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_rename_branch(&mut self, old: &str, new: &str, cx: &mut Context<Self>) {
        let (old, new) = (old.to_string(), new.to_string());
        if old.is_empty() || new.is_empty() {
            return;
        }
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::rename_branch(&dir, &old, &new)
                    .map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_cherry_pick(&mut self, sha: &str, cx: &mut Context<Self>) {
        let sha = sha.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::cherry_pick(&dir, &sha).map_err(|e| e.to_string())
            },
            None,
        );
    }

    pub(crate) fn git_revert_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        let sha = sha.to_string();
        self.run_git_op(
            cx,
            move |dir| {
                deepseekcode_core::git::ops::revert_commit(&dir, &sha, false)
                    .map_err(|e| e.to_string())
            },
            None,
        );
    }

    /// Stage a single hunk of `path` (0-based `hunk_index` into the unified
    /// diff). Powers the P1 hunk-level Stage action.
    pub(crate) fn git_stage_hunk(&mut self, path: &str, hunk_index: usize, cx: &mut Context<Self>) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        let path = path.to_string();
        self.git_busy = true;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = tokio::task::spawn_blocking(move || {
                let diff = deepseekcode_core::git::ops::diff_for_path(&dir, &path)
                    .map_err(|e| e.to_string())?;
                let hunks = crate::logic::split_diff_hunks(&diff);
                let patch = hunks
                    .get(hunk_index)
                    .cloned()
                    .ok_or_else(|| format!("hunk {hunk_index} out of range ({})", hunks.len()))?;
                deepseekcode_core::git::ops::stage_hunk(&dir, &patch).map_err(|e| e.to_string())
            })
            .await
            .ok()
            .unwrap_or_else(|| Err("git task cancelled".to_string()));
            let _ = view.update(cx, |this, cx| {
                this.git_busy = false;
                match result {
                    Ok(_) => {
                        this.git_notice = Some(deepseekcode_i18n::t("git.operation_ok"));
                        this.git_error = None;
                    }
                    Err(e) => {
                        this.git_error = Some(e);
                        this.git_notice = None;
                    }
                }
                cx.notify();
            });
            let _ = view.update(cx, |this, cx| {
                this.refresh_git_status_async(cx);
            });
        })
        .detach();
    }

    // -- AI message + review (P4, heuristic offline so panel works w/o LLM) -----
    pub(crate) fn git_generate_message(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        if self.git_ai_busy {
            return;
        }
        self.git_ai_busy = true;
        cx.notify();
        let input = self.git_commit_input.clone();
        cx.spawn(async move |view, cx| {
            let generated = tokio::task::spawn_blocking(move || heuristic_commit_message(&dir))
                .await
                .ok()
                .flatten();
            let _ = view.update(cx, |this, cx| {
                this.git_ai_busy = false;
                if let Some(msg) = generated {
                    input.update(cx, |i, cx| i.set_text(msg, cx));
                    this.git_notice = None;
                    this.git_error = None;
                } else {
                    this.git_error = Some(
                        deepseekcode_i18n::t("git.operation_failed")
                            .replace("{error}", "empty diff"),
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn git_find_issues(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.git_repo_dir() else {
            return;
        };
        if self.git_review_busy {
            return;
        }
        self.git_sync_review_scope_from_input(cx);
        let scope = if self.git_review_scope.trim().is_empty() {
            self.git_default_branch()
        } else {
            self.git_review_scope.clone()
        };
        self.git_review_busy = true;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let issues = tokio::task::spawn_blocking(move || heuristic_review(&dir, &scope))
                .await
                .ok()
                .unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                this.git_review_busy = false;
                this.git_review_issues = issues;
                this.git_review_expanded = true;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_external_url(&self, url: &str) {
        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("cmd");
            deepseekcode_core::process::hide_console(&mut cmd);
            let _ = cmd.args(["/C", "start", "", url]).spawn();
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
                    run_id = %deepseekcode_telemetry::current_run_id(),
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
                run_id = %deepseekcode_telemetry::current_run_id(),
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
                        run_id = %deepseekcode_telemetry::current_run_id(),
                    reason,
                    "child projection refresh returned no data"
                );
                return;
            }
            let _ = view.update(cx, |this, cx| {
                if generation_slot.load(Ordering::SeqCst) != generation {
                    tracing::debug!(
                        target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
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
            .map(str::to_string)
            .unwrap_or_else(|| deepseekcode_i18n::t("tool.detail.no_command")),
        "read" | "write" | "edit" => {
            let path = input["path"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| deepseekcode_i18n::t("tool.detail.no_path"));
            if tool == "read" {
                deepseekcode_i18n::tr!("tool.detail.read", "path" => path)
            } else {
                deepseekcode_i18n::tr!(
                    "tool.detail.write",
                    "path" => path,
                    "content" => truncate_text(&input.to_string(), 800),
                    "first_line" => truncate_text(&input.to_string(), 80)
                )
            }
        }
        "grep" => deepseekcode_i18n::tr!(
            "tool.detail.grep",
            "pattern" => input["pattern"].as_str().unwrap_or(""),
            "path" => input["path"].as_str().unwrap_or(".")
        ),
        "glob" => {
            let pattern = input["pattern"]
                .as_str()
                .or_else(|| input["glob_pattern"].as_str())
                .unwrap_or("*");
            deepseekcode_i18n::tr!("tool.detail.glob", "pattern" => pattern)
        }
        _ => serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string()),
    }
}

/// Shell command carried in a tool input, for the terminal's `$ …` line.
///
/// Kept out of the rendered `input` string so the terminal echoes the real
/// command instead of the serialized argument object.
fn command_from_tool_input(input: &serde_json::Value) -> Option<String> {
    for key in ["command", "script"] {
        if let Some(cmd) = input.get(key).and_then(|v| v.as_str()) {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    deepseekcode_i18n::tr!(
        "common.truncated_chars",
        "truncated" => truncated,
        "max_chars" => max_chars as u64
    )
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
    conflicted: Vec<crate::logic::GitStatusEntry>,
    diff_summary: Option<String>,
    status_message: Option<String>,
    log: Vec<String>,
    branches: Vec<GitBranch>,
    commits: Vec<GitCommitNode>,
    stashes: Vec<GitStashEntry>,
    remotes: Vec<GitRemote>,
    tags: Vec<String>,
    current_branch: Option<String>,
    head_sha: Option<String>,
    ahead: u32,
    behind: u32,
}

fn collect_git_status(dir: &std::path::Path, graph_limit: usize) -> GitStatusSnapshot {
    use deepseekcode_core::git::ops as gitops;
    let dir_str = dir.to_string_lossy().to_string();
    let mut is_repo_cmd = std::process::Command::new("git");
    deepseekcode_core::process::hide_console(&mut is_repo_cmd);
    let is_repo = is_repo_cmd
        .args(["-C", &dir_str, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false);
    if !is_repo {
        return GitStatusSnapshot {
            status_message: Some(deepseekcode_i18n::t("git.not_a_repository")),
            ..GitStatusSnapshot::default()
        };
    }
    // Status: prefer v2 (branch header), fall back to v1 classify.
    let status_raw = gitops::status_porcelain_v2(dir).unwrap_or_default();
    let lines: Vec<String> = status_raw.lines().map(String::from).collect();
    // v2 lines start with `#` (headers) / `1`/`2`/`u` (entries); filter to
    // classic two-column form when v2 is unavailable.
    let classic: Vec<String> = lines
        .iter()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            if l.starts_with('1') || l.starts_with('2') {
                // `1 XY ... path` -> `XY path`
                let xy: String = l.chars().skip(2).take(2).collect();
                let path = l
                    .split('\t')
                    .nth(1)
                    .or_else(|| l.split(' ').nth(8))
                    .unwrap_or("");
                format!("{xy} {path}")
            } else if l.starts_with('?') {
                format!("?? {}", l[2..].trim())
            } else {
                l.clone()
            }
        })
        .collect();
    let (staged_raw, unstaged_raw, untracked) = classify_git_porcelain(&classic);
    let (staged, unstaged, conflicted) =
        crate::logic::partition_conflicts(staged_raw, unstaged_raw);
    let diff_summary = gitops::diff_shortstat(dir);
    let log = {
        let mut log_cmd = std::process::Command::new("git");
        deepseekcode_core::process::hide_console(&mut log_cmd);
        log_cmd
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
            .unwrap_or_default()
    };
    // Branches / HEAD / ahead-behind.
    let branches = gitops::branch_list_verbose(dir)
        .map(|s| parse_branch_vv(&s))
        .unwrap_or_default();
    let current_branch = gitops::current_branch(dir)
        .ok()
        .filter(|b| !b.is_empty() && b != "HEAD");
    let head_sha = {
        let mut cmd = std::process::Command::new("git");
        deepseekcode_core::process::hide_console(&mut cmd);
        cmd.args(["-C", &dir_str, "rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let (ahead, behind) = gitops::ahead_behind(dir);
    // Graph log (pretty) + stashes + remotes + tags (best effort, never fail).
    let limit = graph_limit.clamp(50, 500);
    let commits = gitops::log_pretty(dir, limit)
        .map(|s| parse_log_pretty(&s, head_sha.as_deref()))
        .unwrap_or_default();
    let stashes = gitops::stash_list(dir)
        .map(|s| parse_stash_list(&s))
        .unwrap_or_default();
    let remotes = gitops::remote_list(dir)
        .map(|s| parse_remotes(&s))
        .unwrap_or_default();
    let tags = gitops::tag_list(dir, 50)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    GitStatusSnapshot {
        staged,
        unstaged,
        untracked,
        conflicted,
        diff_summary,
        status_message: None,
        log,
        branches,
        commits,
        stashes,
        remotes,
        tags,
        current_branch,
        head_sha,
        ahead,
        behind,
    }
}

/// Offline heuristic commit message from staged (or uncommitted) stat.
/// The Agent/LLM path replaces this when a provider is connected; the
/// heuristic keeps the ✨ button useful with zero network.
fn heuristic_commit_message(dir: &std::path::Path) -> Option<String> {
    use deepseekcode_core::git::ops as gitops;
    let stat = gitops::diff_cached(dir, 4096)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            gitops::diff_uncommitted(dir, 4096)
                .ok()
                .filter(|s| !s.trim().is_empty())
        })?;
    // First line of --stat looks like ` file | 12 +++---`.
    let files: Vec<String> = stat
        .lines()
        .take_while(|l| !l.trim_start().starts_with("diff "))
        .filter(|l| l.contains('|'))
        .filter_map(|l| l.split('|').next().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .take(3)
        .collect();
    if files.is_empty() {
        return Some("Update working tree".to_string());
    }
    let scope = files
        .first()
        .and_then(|f| std::path::Path::new(f).parent())
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && s != ".")
        .map(|s| format!("{s}: "))
        .unwrap_or_default();
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .collect();
    Some(format!("Update {scope}{}", names.join(", ")))
}

/// Offline heuristic review over `scope...HEAD` diff: flags TODO/FIXME,
/// over-long lines, debug prints and `unwrap()` in Rust diffs.
fn heuristic_review(dir: &std::path::Path, scope: &str) -> Vec<GitReviewIssue> {
    use deepseekcode_core::git::ops as gitops;
    let range = format!("{scope}...HEAD");
    let diff = gitops::diff_range(dir, &range, 200_000)
        .or_else(|_| gitops::diff_uncommitted(dir, 200_000))
        .unwrap_or_default();
    let mut issues = Vec::new();
    let mut current_file = String::new();
    let mut new_line: u32 = 0;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = rest.trim().to_string();
            continue;
        }
        if line.starts_with("@@") {
            // `@@ -a,b +c,d @@`
            if let Some(plus) = line.split('+').nth(1) {
                new_line = plus
                    .split(|c: char| !(c.is_ascii_digit()))
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            let body = &line[1..];
            new_line += 1;
            let mut flag = |severity: &str, message: String| {
                if issues.len() < 50 {
                    issues.push(GitReviewIssue {
                        path: current_file.clone(),
                        line: Some(new_line),
                        severity: severity.to_string(),
                        message,
                    });
                }
            };
            if body.contains("TODO") || body.contains("FIXME") {
                flag("info", format!("TODO marker: {}", body.trim()));
            }
            if body.chars().count() > 160 {
                flag(
                    "warning",
                    format!("long line ({} chars)", body.chars().count()),
                );
            }
            if body.contains("console.log") || body.contains("dbg!") || body.contains("println!") {
                flag("warning", "debug print left in diff".to_string());
            }
            if current_file.ends_with(".rs") && body.contains(".unwrap()") {
                flag("warning", "unwrap() in changed code".to_string());
            }
            if body.contains("password") && body.contains('=') {
                flag("error", "possible hardcoded secret".to_string());
            }
        } else if !line.starts_with('-') {
            new_line += 1;
        }
    }
    // Fall back to file-level scan when diff is empty (e.g. scope == HEAD).
    if issues.is_empty() && !diff.trim().is_empty() {
        issues.push(GitReviewIssue {
            path: String::new(),
            line: None,
            severity: "info".to_string(),
            message: "diff scanned: no high-signal findings".to_string(),
        });
    }
    issues
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
    if keystroke.modifiers.control || keystroke.modifiers.platform {
        match keystroke.key.as_str() {
            "c" => return vec![0x03],
            "d" => return vec![0x04],
            "z" => return vec![0x1a],
            "l" => return vec![0x0c],
            _ => {}
        }
    }
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
