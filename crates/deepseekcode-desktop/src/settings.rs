use crate::preferences::ThemeId;
use crate::theme::ThemeTokens;
use crate::ui::scrollbar::render_flex_scroll_viewport;
use deepseekcode_config::PolicyMode;
use deepseekcode_types::{CONFIG_FILE, PRODUCT_NAME};
use deepseekcode_schema::policy::{PolicyAction, PolicyGrant};
use deepseekcode_schema::session::PromptDelivery;
use deepseekcode_schema::provider::ProviderInfo;
use gpui::prelude::FluentBuilder;
use gpui::*;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct PolicyRuleRow {
    pub scope: String,
    pub pattern: String,
    pub action: PolicyAction,
}

pub(crate) const POLICY_SCOPES: &[&str] = &[
    "bash", "read", "edit", "write", "grep", "glob", "list", "outside_workspace", "loop_guard",
    "webfetch", "websearch", "skill", "task", "lsp", "question", "*",
];

#[derive(Clone, Debug, Default)]
pub struct SettingsCatalog {
    pub mcp: Vec<Value>,
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
    pub compaction_summary: String,
    pub compaction_message: String,
    pub worktrees: Vec<Value>,
    pub worktree_message: String,
    pub location_summary: String,
    pub health_status: String,
    pub project_directories: Vec<String>,
    pub policy_grants: Vec<PolicyGrant>,
    pub policy_mode: PolicyMode,
    pub loaded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Appearance,
    General,
    Ai,
    Project,
    Permissions,
    Integrations,
    Worktrees,
    Diagnostics,
    Advanced,
}

pub struct SettingsView;

impl SettingsView {
    pub fn render_overlay(
        cx: &Context<crate::workspace::WorkspaceView>,
        open: bool,
        section: SettingsSection,
        current_theme: ThemeId,
        scroll_handle: &ScrollHandle,
        providers: &[ProviderInfo],
        selected_agent: &str,
        selected_model: &str,
        selected_provider: Option<&str>,
        catalog: &SettingsCatalog,
        prompt_delivery: PromptDelivery,
        export_sanitize: bool,
        server_url_input: Entity<crate::composer::PromptInput>,
        server_password_input: Entity<crate::composer::PromptInput>,
        worktree_branch_input: Entity<crate::composer::PromptInput>,
        permission_allow_input: Entity<crate::composer::PromptInput>,
        permission_deny_input: Entity<crate::composer::PromptInput>,
        policy_rule_rows: &[PolicyRuleRow],
        policy_auto_mode: bool,
        selected_policy_row: Option<usize>,
    ) -> AnyElement {
        if !open {
            return div().into_any_element();
        }

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        div()
            .id("settings-overlay")
            .absolute()
            .inset_0()
            .bg(theme.bg_overlay)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("settings-panel")
                    .relative()
                    .w(px(760.))
                    .h(px(560.))
                    .rounded_lg()
                    .bg(theme.bg_surface)
                    .border_1()
                    .border_color(theme.border_default)
                    .flex()
                    .overflow_hidden()
                    .child(render_sidebar(cx, section))
                    .child(render_content(
                        cx,
                        section,
                        current_theme,
                        scroll_handle,
                        providers,
                        selected_agent,
                        selected_model,
                        selected_provider,
                        catalog,
                        prompt_delivery,
                        export_sanitize,
                        server_url_input,
                        server_password_input,
                        worktree_branch_input,
                        permission_allow_input,
                        permission_deny_input,
                        policy_rule_rows,
                        policy_auto_mode,
                        selected_policy_row,
                    ))
                    .child(render_close_button(cx)),
            )
            .into_any_element()
    }
}

fn render_sidebar(cx: &Context<crate::workspace::WorkspaceView>, section: SettingsSection) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .w(px(200.))
        .h_full()
        .flex_shrink_0()
        .bg(theme.bg_base)
        .border_r_1()
        .border_color(theme.border_subtle)
        .flex()
        .flex_col()
        .child(
            div()
                .px_4()
                .py_3()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child("Settings"),
        )
        .child(nav_item(
            cx,
            SettingsSection::Appearance,
            section,
            "Appearance",
            "Theme & display",
        ))
        .child(nav_item(
            cx,
            SettingsSection::General,
            section,
            "General",
            "Workspace preferences",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Ai,
            section,
            "AI & Providers",
            "Models, agents, API keys",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Project,
            section,
            "Project",
            "AGENTS.md & config",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Permissions,
            section,
            "Permissions",
            "Allow / deny rules",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Integrations,
            section,
            "Integrations",
            "MCP, skills, plugins",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Worktrees,
            section,
            "Worktrees",
            "Git worktrees",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Diagnostics,
            section,
            "Diagnostics",
            "Health & export",
        ))
        .child(nav_item(
            cx,
            SettingsSection::Advanced,
            section,
            "Session & Data",
            "Export, revert, sessions",
        ))
        .child(div().mt_auto())
}

fn render_close_button(cx: &Context<crate::workspace::WorkspaceView>) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .absolute()
        .top(px(10.))
        .right(px(10.))
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.error)
        .hover(|style| style.bg(theme.bg_hover))
        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
            this.close_settings(cx);
        }))
        .child("✕")
}

fn nav_item(
    cx: &Context<crate::workspace::WorkspaceView>,
    target: SettingsSection,
    current: SettingsSection,
    title: &'static str,
    subtitle: &'static str,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let active = target == current;
    div()
        .px_3()
        .py_2()
        .mx_2()
        .rounded_md()
        .cursor_pointer()
        .bg(if active {
            theme.bg_selected
        } else {
            hsla(0., 0., 0., 0.).into()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.open_settings_section(target, cx)),
        )
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(subtitle),
        )
}

fn render_content(
    cx: &Context<crate::workspace::WorkspaceView>,
    section: SettingsSection,
    current_theme: ThemeId,
    scroll_handle: &ScrollHandle,
    providers: &[ProviderInfo],
    selected_agent: &str,
    selected_model: &str,
    selected_provider: Option<&str>,
    catalog: &SettingsCatalog,
    prompt_delivery: PromptDelivery,
    export_sanitize: bool,
    server_url_input: Entity<crate::composer::PromptInput>,
    server_password_input: Entity<crate::composer::PromptInput>,
    worktree_branch_input: Entity<crate::composer::PromptInput>,
    permission_allow_input: Entity<crate::composer::PromptInput>,
    permission_deny_input: Entity<crate::composer::PromptInput>,
    policy_rule_rows: &[PolicyRuleRow],
    policy_auto_mode: bool,
    selected_policy_row: Option<usize>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let body: AnyElement = match section {
        SettingsSection::Appearance => {
            render_appearance_section(cx, current_theme).into_any_element()
        }
        SettingsSection::Ai => render_ai_section(
            cx,
            providers,
            selected_agent,
            selected_model,
            selected_provider,
        )
        .into_any_element(),
        SettingsSection::General => render_general_section(
            cx,
            prompt_delivery,
            server_url_input,
            server_password_input,
        )
        .into_any_element(),
        SettingsSection::Project => {
            render_project_section(cx, catalog).into_any_element()
        }
        SettingsSection::Permissions => render_permissions_section(
            cx,
            catalog,
            permission_allow_input,
            permission_deny_input,
            policy_rule_rows,
            policy_auto_mode,
            selected_policy_row,
        )
        .into_any_element(),
        SettingsSection::Integrations => {
            render_integrations_section(cx, catalog).into_any_element()
        }
        SettingsSection::Worktrees => render_worktrees_only_section(cx, catalog, worktree_branch_input)
            .into_any_element(),
        SettingsSection::Diagnostics => {
            render_diagnostics_section(cx, catalog).into_any_element()
        }
        SettingsSection::Advanced => render_advanced_section(cx, catalog, export_sanitize)
            .into_any_element(),
    };

    div()
        .flex_1()
        .h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .px_6()
                .py_4()
                .border_b_1()
                .border_color(theme.border_subtle)
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child(match section {
                    SettingsSection::Appearance => "Appearance",
                    SettingsSection::Ai => "AI & Providers",
                    SettingsSection::General => "General",
                    SettingsSection::Project => "Project",
                    SettingsSection::Permissions => "Permissions",
                    SettingsSection::Integrations => "Integrations",
                    SettingsSection::Worktrees => "Worktrees",
                    SettingsSection::Diagnostics => "Diagnostics",
                    SettingsSection::Advanced => "Session & Data",
                }),
        )
        .child(render_scrollable_body(cx, scroll_handle, body))
}

fn render_scrollable_body(
    cx: &Context<crate::workspace::WorkspaceView>,
    scroll_handle: &ScrollHandle,
    body: AnyElement,
) -> impl IntoElement {
    render_flex_scroll_viewport(
        cx,
        scroll_handle,
        "settings-content-scroll",
        "settings-scrollbar-track",
        false,
        div().px_6().py_4().child(body),
    )
}

fn render_appearance_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    current_theme: ThemeId,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let mut row = div().flex().gap_2();
    for id in ThemeId::ALL {
        row = row.child(theme_choice(cx, id, current_theme, &theme));
    }
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            "Theme",
            "Choose the application color scheme. System follows your OS preference when supported.",
            row,
        ))
}

fn theme_choice(
    cx: &Context<crate::workspace::WorkspaceView>,
    id: ThemeId,
    current: ThemeId,
    theme: &ThemeTokens,
) -> impl IntoElement {
    let active = id == current;
    div()
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(if active {
            theme.bg_selected
        } else {
            theme.bg_elevated
        })
        .border_1()
        .border_color(if active {
            theme.border_focus
        } else {
            theme.border_default
        })
        .text_color(if active {
            theme.text_primary
        } else {
            theme.text_secondary
        })
        .on_mouse_down(MouseButton::Left, {
            cx.listener(move |this, _, _, cx| this.set_theme(id, cx))
        })
        .child(id.label())
}

fn render_ai_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    providers: &[ProviderInfo],
    selected_agent: &str,
    selected_model: &str,
    selected_provider: Option<&str>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            "Default Agent",
            "Choose how the assistant behaves for new prompts.",
            render_agent_choices(cx, selected_agent),
        ))
        .child(settings_group(
            &theme,
            "Default Model",
            "Select the model used when sending prompts.",
            render_model_choices(cx, providers, selected_model, selected_provider),
        ))
        .child(settings_group(
            &theme,
            "Providers",
            "Connect API keys once — stored in OS keychain with encrypted local fallback.",
            render_providers(cx, providers),
        ))
}

fn render_agent_choices(
    cx: &Context<crate::workspace::WorkspaceView>,
    selected_agent: &str,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let mut row = div().flex().gap_2();
    for option in ["build", "plan", "general"] {
        let active = option == selected_agent;
        row = row.child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(if active {
                    theme.bg_selected
                } else {
                    theme.bg_elevated
                })
                .border_1()
                .border_color(if active {
                    theme.border_focus
                } else {
                    theme.border_default
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.set_agent(option, cx)),
                )
                .child(option),
        );
    }
    row
}

fn render_general_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    prompt_delivery: PromptDelivery,
    server_url_input: Entity<crate::composer::PromptInput>,
    server_password_input: Entity<crate::composer::PromptInput>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            "Prompt delivery",
            "Steer inserts prompts during an active turn; Queue waits until idle.",
            div()
                .flex()
                .gap_2()
                .child(delivery_choice(
                    cx,
                    PromptDelivery::Steer,
                    "Steer (default)",
                    prompt_delivery,
                ))
                .child(delivery_choice(
                    cx,
                    PromptDelivery::Queue,
                    "Queue",
                    prompt_delivery,
                )),
        ))
        .child(settings_group(
            &theme,
            "Data location",
            "Application data is stored under your platform data directory.",
            div()
                .text_xs()
                .font_family("Consolas")
                .text_color(theme.text_secondary)
                .child(
                    dirs::data_dir()
                        .map(|d| d.join("deepseekcode").display().to_string())
                        .unwrap_or_else(|| "~/.local/share/deepseekcode".to_string()),
                ),
        ))
        .child(settings_group(
            &theme,
            "Remote server",
            "Connect to a remote DeepSeekCode server instead of the embedded engine (non-localhost URLs).",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(server_url_input.clone())
                .child(server_password_input.clone())
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_selected)
                        .text_xs()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.save_server_settings(cx);
                        }))
                        .child("Save server settings"),
                ),
        ))
        .child(settings_group(
            &theme,
            "Multi-workspace",
            "Open another project in a separate window.",
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.open_new_window(cx);
                }))
                .child("New Window"),
        ))
        .child(settings_group(
            &theme,
            "Keyboard",
            "Common shortcuts for the prompt input.",
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(theme.text_muted)
                .child("Enter — send prompt")
                .child("Ctrl+A — select all")
                .child("Ctrl+V / Ctrl+C / Ctrl+X — paste / copy / cut"),
        ))
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_xs()
                .text_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.open_settings_section(SettingsSection::Ai, cx);
                }))
                .child("Go to AI & Providers →"),
        )
}

fn delivery_choice(
    cx: &Context<crate::workspace::WorkspaceView>,
    delivery: PromptDelivery,
    label: &'static str,
    current: PromptDelivery,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let active = delivery == current;
    div()
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(if active {
            theme.bg_selected
        } else {
            theme.bg_elevated
        })
        .border_1()
        .border_color(if active {
            theme.border_focus
        } else {
            theme.border_default
        })
        .text_color(if active {
            theme.text_primary
        } else {
            theme.text_secondary
        })
        .on_mouse_down(MouseButton::Left, {
            cx.listener(move |this, _, _, cx| this.set_prompt_delivery(delivery, cx))
        })
        .child(label)
}

fn render_project_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            "Project directories",
            "Additional workspace roots registered for this location.",
            if catalog.project_directories.is_empty() {
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(if catalog.loaded {
                        "No extra directories — use Files → + to add."
                    } else {
                        "Loading…"
                    })
            } else {
                div().flex().flex_col().gap_1().children(
                    catalog
                        .project_directories
                        .iter()
                        .map(|path| {
                            div()
                                .text_xs()
                                .font_family("Consolas")
                                .text_color(theme.text_primary)
                                .child(path.clone())
                                .into_any_element()
                        })
                        .collect::<Vec<_>>(),
                )
            },
        ))
        .child(settings_group(
            &theme,
            "AGENTS.md",
            "Project context file used by the session engine.",
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.open_agents_md(cx);
                }))
                .child("Open AGENTS.md"),
        ))
        .child(settings_group(
            &theme,
            "Configuration",
            format!(
                "Edit the {PRODUCT_NAME} project config (`{CONFIG_FILE}`) in the editor panel."
            ),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.open_project_config(cx);
                }))
                .child(format!("Open {PRODUCT_NAME} config")),
        ))
}

fn render_permissions_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
    permission_allow_input: Entity<crate::composer::PromptInput>,
    permission_deny_input: Entity<crate::composer::PromptInput>,
    policy_rule_rows: &[PolicyRuleRow],
    policy_auto_mode: bool,
    selected_policy_row: Option<usize>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            "Policy mode",
            "Auto (YOLO) permits non-block rules without prompting.",
            div()
                .flex()
                .gap_2()
                .child(policy_mode_button(
                    cx,
                    &theme,
                    false,
                    "Interactive",
                    !policy_auto_mode,
                ))
                .child(policy_mode_button(
                    cx,
                    &theme,
                    true,
                    "Auto (YOLO)",
                    policy_auto_mode,
                )),
        ))
        .child(settings_group(
            &theme,
            "Policy rules",
            format!(
                "Scope / pattern / action table. Saved to {PRODUCT_NAME} config (`{CONFIG_FILE}`) policy section."
            ),
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(render_policy_table_header(&theme))
                .children(if policy_rule_rows.is_empty() {
                    vec![div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(if catalog.loaded {
                            "No rules yet — add one below."
                        } else {
                            "Open a project to edit policy."
                        })
                        .into_any_element()]
                } else {
                    policy_rule_rows
                        .iter()
                        .enumerate()
                        .map(|(index, row)| {
                            render_policy_table_row(
                                cx,
                                &theme,
                                index,
                                row,
                                selected_policy_row == Some(index),
                            )
                            .into_any_element()
                        })
                        .collect()
                })
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .text_xs()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.add_policy_rule_row(cx);
                                }))
                                .child("+ Add rule"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .text_xs()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.apply_policy_preset("dev", cx);
                                }))
                                .child("Dev preset"),
                        )
                        .child(policy_preset_button(cx, &theme, "readonly", "Read-only"))
                        .child(policy_preset_button(cx, &theme, "strict", "Strict")),
                )
                .when(selected_policy_row.is_some(), |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child("Pattern for selected rule:"),
                            )
                            .child(div().min_h(px(28.)).child(permission_deny_input.clone())),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("Advanced: edit as lines (scope | pattern | action)"),
                )
                .child(div().min_h(px(80.)).child(permission_allow_input.clone()))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_selected)
                        .text_sm()
                        .text_center()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.save_policy_rules(cx);
                        }))
                        .child("Save policy rules"),
                ),
        ))
        .child(settings_group(
            &theme,
            "Remembered allows",
            "Project grants from Always (project) approvals.",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(if catalog.policy_grants.is_empty() {
                    vec![div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(if catalog.loaded {
                            "No remembered grants for this project."
                        } else {
                            "Open a project to view grants."
                        })
                        .into_any_element()]
                } else {
                    catalog
                        .policy_grants
                        .iter()
                        .map(|grant| {
                            let grant_id = grant.id.clone();
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family("Consolas")
                                        .text_color(theme.text_secondary)
                                        .child(format!("{} | {}", grant.scope, grant.pattern)),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme.error.opacity(0.15))
                                        .text_xs()
                                        .text_color(theme.error)
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, {
                                            let grant_id = grant_id.clone();
                                            cx.listener(move |this, _, _, cx| {
                                                this.delete_policy_grant(grant_id.clone(), cx);
                                            })
                                        })
                                        .child("Delete"),
                                )
                                .into_any_element()
                        })
                        .collect()
                }),
        ))
}

fn policy_mode_button(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    auto: bool,
    label: &'static str,
    active: bool,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .when(active, |el| el.bg(theme.bg_selected))
        .when(!active, |el| {
            el.bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border_subtle)
        })
        .text_xs()
        .text_color(if active {
            theme.text_primary
        } else {
            theme.text_secondary
        })
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.set_policy_auto_mode(auto, cx);
        }))
        .child(label)
}

fn render_policy_table_header(theme: &ThemeTokens) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .px_2()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_muted)
        .child(div().w(px(120.)).child("Scope"))
        .child(div().flex_1().child("Pattern"))
        .child(div().w(px(72.)).child("Action"))
        .child(div().w(px(48.)).child(""))
}

fn render_policy_table_row(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    index: usize,
    row: &PolicyRuleRow,
    selected: bool,
) -> impl IntoElement {
    let scope = row.scope.clone();
    let pattern = row.pattern.clone();
    let action_label = policy_action_label(row.action);
    div()
        .flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .rounded_md()
        .when(selected, |el| el.bg(theme.bg_selected))
        .when(!selected, |el| el.bg(theme.bg_elevated))
        .child(
            div()
                .w(px(120.))
                .text_xs()
                .font_family("Consolas")
                .text_color(theme.text_primary)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.cycle_policy_rule_scope(index, cx);
                    })
                })
                .child(scope),
        )
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_family("Consolas")
                .text_color(theme.text_secondary)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, {
                    let pattern_for_select = pattern.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.select_policy_rule_row(index, pattern_for_select.clone(), cx);
                    })
                })
                .child(pattern),
        )
        .child(
            div()
                .w(px(72.))
                .text_xs()
                .text_color(theme.text_primary)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.cycle_policy_rule_action(index, cx);
                    })
                })
                .child(action_label),
        )
        .child(
            div()
                .w(px(48.))
                .text_xs()
                .text_color(theme.error)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.remove_policy_rule_row(index, cx);
                    })
                })
                .child("Remove"),
        )
}

fn policy_action_label(action: PolicyAction) -> &'static str {
    match action {
        PolicyAction::Permit => "permit",
        PolicyAction::Prompt => "prompt",
        PolicyAction::Block => "block",
    }
}

fn policy_preset_button(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    preset: &'static str,
    label: &'static str,
) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_subtle)
        .text_xs()
        .text_color(theme.text_secondary)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.apply_policy_preset(preset, cx);
        }))
        .child(label)
}

fn render_integrations_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(settings_group(
            &theme,
            "Location",
            "Workspace location metadata (skills path, plugins).",
            div()
                .text_xs()
                .font_family("Consolas")
                .text_color(theme.text_secondary)
                .child(if catalog.location_summary.is_empty() {
                    "Loading…".to_string()
                } else {
                    catalog.location_summary.clone()
                }),
        ))
        .child(settings_group(
            &theme,
            "MCP servers",
            "Connect or disconnect configured MCP servers.",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_selected)
                        .text_xs()
                        .text_center()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.refresh_mcp_servers(cx);
                        }))
                        .child("Refresh MCP list"),
                )
                .child(render_mcp_list(cx, catalog)),
        ))
        .child(settings_group(
            &theme,
            "Skills",
            "Skill catalog entries discovered for this workspace.",
            render_string_list(&theme, &catalog.skills, catalog.loaded, "No skills found"),
        ))
        .child(settings_group(
            &theme,
            "Plugins",
            "Loaded plugin hooks for this workspace.",
            render_string_list(&theme, &catalog.plugins, catalog.loaded, "No plugins loaded"),
        ))
}

fn render_worktrees_only_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
    worktree_branch_input: Entity<crate::composer::PromptInput>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(settings_group(
            &theme,
            "Branch name",
            "Optional branch name for new worktrees (leave empty for auto).",
            worktree_branch_input.clone(),
        ))
        .child(settings_group(
            &theme,
            "Git worktrees",
            "Create isolated DeepSeekCode worktrees under `.deepseekcode/worktrees` for parallel branches.",
            render_worktree_section(cx, catalog),
        ))
}

fn render_diagnostics_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(settings_group(
            &theme,
            "Health",
            "Server health status from the last poll.",
            div()
                .text_sm()
                .text_color(if catalog.health_status == "ok" {
                    theme.success
                } else {
                    theme.warning
                })
                .child(format!("Status: {}", catalog.health_status)),
        ))
        .child(settings_group(
            &theme,
            "Diagnostics export",
            "Export logs and telemetry for troubleshooting.",
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.info)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.export_diagnostics(cx);
                }))
                .child("Export Diagnostics Bundle"),
        ))
        .child(settings_group(
            &theme,
            "OpenAPI",
            "Browse the HTTP API schema in your browser.",
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.open_openapi_docs(cx);
                }))
                .child("Open /api/v1/openapi.json"),
        ))
}

fn render_advanced_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
    export_sanitize: bool,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(settings_group(
            &theme,
            "Compaction",
            "Auto-compaction settings and manual compaction trigger.",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_family("Consolas")
                        .text_color(theme.text_secondary)
                        .child(if catalog.loaded {
                            catalog.compaction_summary.clone()
                        } else {
                            "Open settings after a project is loaded.".into()
                        }),
                )
                .child(if catalog.compaction_message.is_empty() {
                    div().into_any_element()
                } else {
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(catalog.compaction_message.clone())
                        .into_any_element()
                })
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.accent_muted)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.compact_current_session(cx);
                        }))
                        .child("Compact Now"),
                ),
        ))
        .child(settings_group(
            &theme,
            "Session",
            "Fork, export, revert, or start a new conversation.",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.new_session(cx);
                        }))
                        .child("New Session"),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.fork_current_session(cx);
                        }))
                        .child("Fork Session"),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(if export_sanitize {
                            theme.bg_selected
                        } else {
                            theme.bg_elevated
                        })
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.toggle_export_sanitize(cx);
                        }))
                        .child(if export_sanitize {
                            "Sanitize on export: ON"
                        } else {
                            "Sanitize on export: OFF"
                        }),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.export_current_session(cx);
                        }))
                        .child("Export Session…"),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.open_revert_dock(cx);
                        }))
                        .child("Revert…"),
                ),
        ))
}

fn render_worktree_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let mut body = div().flex().flex_col().gap_2();

    if !catalog.loaded {
        return body.child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .child("Open settings after a project is loaded."),
        );
    }

    if !catalog.worktree_message.is_empty() {
        body = body.child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(catalog.worktree_message.clone()),
        );
    }

    body = body.child(
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.accent_muted)
                    .text_sm()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.create_worktree(cx);
                    }))
                    .child("Create worktree"),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.bg_hover)
                    .text_sm()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.prune_worktrees(cx);
                    }))
                    .child("Prune stale"),
            ),
    );

    if catalog.worktrees.is_empty() {
        body = body.child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .child("No worktrees yet (requires a git repository)."),
        );
    } else {
        for entry in &catalog.worktrees {
            let path = entry
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let branch = entry
                .get("branch")
                .and_then(|v| v.as_str())
                .unwrap_or("detached");
            let path_owned = path.to_string();
            body = body.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(theme.bg_elevated)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Consolas")
                                    .child(path.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(branch.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(theme.bg_selected)
                                    .on_mouse_down(MouseButton::Left, {
                                        let path_owned = path_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_worktree_path(path_owned.clone(), cx);
                                        })
                                    })
                                    .child("Open"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(theme.error)
                                    .on_mouse_down(MouseButton::Left, {
                                        let path_owned = path_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            this.remove_worktree(path_owned.clone(), cx);
                                        })
                                    })
                                    .child("Remove"),
                            ),
                    ),
            );
        }
    }

    body
}

fn render_mcp_list(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    if !catalog.loaded {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child("Loading…");
    }
    if catalog.mcp.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child("No MCP servers configured");
    }
    let mut list = div().flex().flex_col().gap_2();
    for entry in &catalog.mcp {
        let name = entry
            .get("name")
            .or_else(|| entry.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("mcp");
        let connected = entry
            .get("connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let tools = entry
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let name_owned = name.to_string();
        list = list.child(
            div()
                .p_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .flex()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    div().flex().flex_col().gap_1().child(
                        div()
                            .text_sm()
                            .child(name.to_string()),
                    ).child(
                        div()
                            .text_xs()
                            .text_color(if connected {
                                theme.success
                            } else {
                                theme.text_muted
                            })
                            .child(if connected {
                                "Connected"
                            } else {
                                "Disconnected"
                            }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_disabled)
                                .child(format!("{tools} tools")),
                        )
                        .child(if connected {
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(theme.error)
                                .on_mouse_down(MouseButton::Left, {
                                    let name_owned = name_owned.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.disconnect_mcp_server(name_owned.clone(), cx);
                                    })
                                })
                                .child("Disconnect")
                                .into_any_element()
                        } else {
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(theme.accent_muted)
                                .on_mouse_down(MouseButton::Left, {
                                    let name_owned = name_owned.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.connect_mcp_server(name_owned.clone(), cx);
                                    })
                                })
                                .child("Connect")
                                .into_any_element()
                        }),
                ),
        );
    }
    list
}

fn render_string_list(theme: &ThemeTokens, items: &[String], loaded: bool, empty_label: &'static str) -> impl IntoElement {
    if !loaded {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child("Loading…");
    }
    if items.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child(empty_label);
    }
    let mut list = div().flex().flex_col().gap_1();
    for item in items {
        list = list.child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_xs()
                .font_family("Consolas")
                .child(item.clone()),
        );
    }
    list
}

fn render_providers(
    cx: &Context<crate::workspace::WorkspaceView>,
    providers: &[ProviderInfo],
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let mut list = div().flex().flex_col().gap_2();
    for provider in providers {
        let provider_id = provider.id.clone();
        let disconnect_id = provider.id.clone();
        let status = if provider.connected {
            "Connected"
        } else {
            "Not connected"
        };
        let status_color = if provider.connected {
            theme.success
        } else {
            theme.text_muted
        };

        let mut actions = div().flex().gap_2().child(
            div()
                .px_3()
                .py_1()
                .rounded_md()
                .bg(theme.info)
                .text_xs()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.connect_provider(&provider_id, cx);
                    }),
                )
                .child(if provider.connected {
                    "Update Key"
                } else {
                    "Connect"
                }),
        );

        if provider.connected {
            actions = actions.child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(theme.error)
                    .text_xs()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.disconnect_provider(&disconnect_id, cx);
                        }),
                    )
                    .child("Disconnect"),
            );
        }

        list = list.child(
            div()
                .p_3()
                .rounded_md()
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border_default)
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(provider.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(status_color)
                                .child(status),
                        ),
                )
                .child(actions),
        );
    }
    list
}

fn render_model_choices(
    cx: &Context<crate::workspace::WorkspaceView>,
    providers: &[ProviderInfo],
    selected_model: &str,
    selected_provider: Option<&str>,
) -> AnyElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let models: Vec<_> = providers
        .iter()
        .filter(|provider| {
            selected_provider
                .map(|id| provider.id == id)
                .unwrap_or(true)
        })
        .flat_map(|provider| {
            provider.models.iter().map(move |model| {
                (
                    provider.name.clone(),
                    model.id.clone(),
                    model.name.clone(),
                    model.provider_id.clone(),
                )
            })
        })
        .collect();

    if models.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child("Connect a provider to choose a model.")
            .into_any_element();
    }

    let mut list = div().flex().flex_col().gap_1();
    for (provider_name, model_id, model_name, provider_id) in models {
        let active = selected_model == model_id || selected_model == model_name;
        let label = format!("{provider_name} · {model_name}");
        list = list.child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(if active {
                    theme.bg_selected
                } else {
                    theme.bg_elevated
                })
                .border_1()
                .border_color(if active {
                    theme.border_focus
                } else {
                    theme.border_default
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_model(&model_id, &model_name, &provider_id, cx);
                    }),
                )
                .child(label),
        );
    }
    list.into_any_element()
}

fn settings_group(
    theme: &ThemeTokens,
    title: &'static str,
    description: impl IntoElement,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(description),
        )
        .child(body)
}
