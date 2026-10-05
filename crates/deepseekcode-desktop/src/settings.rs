use crate::preferences::{CustomModelEntry, ThemeId};
use crate::theme::ThemeTokens;
use crate::ui::scrollbar::render_flex_scroll_viewport;
use deepseekcode_config::PolicyMode;
use deepseekcode_schema::policy::{PolicyAction, PolicyGrant};
use deepseekcode_schema::provider::ProviderInfo;
use deepseekcode_schema::session::PromptDelivery;
use deepseekcode_types::{CONFIG_FILE, PRODUCT_NAME};
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
    "bash",
    "read",
    "edit",
    // `write` maps to `edit` at runtime (tool_scope); listing it creates
    // dead `write:{…}` rules that never match. Kept out intentionally.
    "grep",
    "glob",
    "list",
    "outside_workspace",
    "loop_guard",
    "webfetch",
    "websearch",
    "skill",
    "task",
    "plan",
    "mcp",
    "lsp",
    "question",
    "todo",
    "context",
    "*",
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
        current_language: deepseekcode_i18n::LanguagePref,
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
        policy_pattern_input: Entity<crate::composer::PromptInput>,
        custom_model_input: Entity<crate::composer::PromptInput>,
        custom_models: &[CustomModelEntry],
        has_project: bool,
    ) -> AnyElement {
        if !open {
            return div().into_any_element();
        }

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        div()
            .id("settings-overlay")
            .absolute()
            .inset_0()
            // Translucent scrim: the workspace behind stays visible around
            // the centered panel (opaque `bg_overlay` hid the whole page).
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            // Swallow clicks: without this, mouse-downs on the panel fall
            // through to the timeline behind and start text selection there
            // instead of focusing the tapped control. Children (front-most)
            // still receive the event first; only behind elements are cut off.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                        current_language,
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
                        policy_pattern_input,
                        custom_model_input,
                        custom_models,
                        has_project,
                    ))
                    .child(render_close_button(cx)),
            )
            .into_any_element()
    }
}

fn render_sidebar(
    cx: &Context<crate::workspace::WorkspaceView>,
    section: SettingsSection,
) -> impl IntoElement {
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
                .child(deepseekcode_i18n::t("settings.title")),
        )
        .child(nav_item(
            cx,
            SettingsSection::Appearance,
            section,
            deepseekcode_i18n::t("settings.section.appearance"),
            deepseekcode_i18n::t("settings.section.appearance_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::General,
            section,
            deepseekcode_i18n::t("settings.section.general"),
            deepseekcode_i18n::t("settings.section.general_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Ai,
            section,
            deepseekcode_i18n::t("settings.section.ai"),
            deepseekcode_i18n::t("settings.section.ai_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Project,
            section,
            deepseekcode_i18n::t("settings.section.project"),
            deepseekcode_i18n::t("settings.section.project_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Permissions,
            section,
            deepseekcode_i18n::t("settings.section.permissions"),
            deepseekcode_i18n::t("settings.section.permissions_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Integrations,
            section,
            deepseekcode_i18n::t("settings.section.integrations"),
            deepseekcode_i18n::t("settings.section.integrations_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Worktrees,
            section,
            deepseekcode_i18n::t("settings.section.worktrees"),
            deepseekcode_i18n::t("settings.section.worktrees_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Diagnostics,
            section,
            deepseekcode_i18n::t("settings.section.diagnostics"),
            deepseekcode_i18n::t("settings.section.diagnostics_desc"),
        ))
        .child(nav_item(
            cx,
            SettingsSection::Advanced,
            section,
            deepseekcode_i18n::t("settings.section.session"),
            deepseekcode_i18n::t("settings.section.session_desc"),
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
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.close_settings(cx);
            }),
        )
        .child(deepseekcode_i18n::t("settings.close"))
}

fn nav_item(
    cx: &Context<crate::workspace::WorkspaceView>,
    target: SettingsSection,
    current: SettingsSection,
    title: String,
    subtitle: String,
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
        .child(div().text_xs().text_color(theme.text_muted).child(subtitle))
}

fn render_content(
    cx: &Context<crate::workspace::WorkspaceView>,
    section: SettingsSection,
    current_theme: ThemeId,
    current_language: deepseekcode_i18n::LanguagePref,
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
    _permission_deny_input: Entity<crate::composer::PromptInput>,
    policy_rule_rows: &[PolicyRuleRow],
    policy_auto_mode: bool,
    selected_policy_row: Option<usize>,
    policy_pattern_input: Entity<crate::composer::PromptInput>,
    custom_model_input: Entity<crate::composer::PromptInput>,
    custom_models: &[CustomModelEntry],
    has_project: bool,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let body: AnyElement = match section {
        SettingsSection::Appearance => {
            render_appearance_section(cx, current_theme, current_language).into_any_element()
        }
        SettingsSection::Ai => render_ai_section(
            cx,
            providers,
            selected_agent,
            selected_model,
            selected_provider,
            custom_model_input,
            custom_models,
            has_project,
        )
        .into_any_element(),
        SettingsSection::General => {
            render_general_section(cx, prompt_delivery, server_url_input, server_password_input)
                .into_any_element()
        }
        SettingsSection::Project => render_project_section(cx, catalog).into_any_element(),
        SettingsSection::Permissions => render_permissions_section(
            cx,
            catalog,
            permission_allow_input,
            policy_pattern_input,
            policy_rule_rows,
            policy_auto_mode,
            selected_policy_row,
        )
        .into_any_element(),
        SettingsSection::Integrations => {
            render_integrations_section(cx, catalog).into_any_element()
        }
        SettingsSection::Worktrees => {
            render_worktrees_only_section(cx, catalog, worktree_branch_input).into_any_element()
        }
        SettingsSection::Diagnostics => render_diagnostics_section(cx, catalog).into_any_element(),
        SettingsSection::Advanced => {
            render_advanced_section(cx, catalog, export_sanitize).into_any_element()
        }
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
                    SettingsSection::Appearance => {
                        deepseekcode_i18n::t("settings.section.appearance")
                    }
                    SettingsSection::Ai => deepseekcode_i18n::t("settings.section.ai"),
                    SettingsSection::General => deepseekcode_i18n::t("settings.section.general"),
                    SettingsSection::Project => deepseekcode_i18n::t("settings.section.project"),
                    SettingsSection::Permissions => {
                        deepseekcode_i18n::t("settings.section.permissions")
                    }
                    SettingsSection::Integrations => {
                        deepseekcode_i18n::t("settings.section.integrations")
                    }
                    SettingsSection::Worktrees => {
                        deepseekcode_i18n::t("settings.section.worktrees")
                    }
                    SettingsSection::Diagnostics => {
                        deepseekcode_i18n::t("settings.section.diagnostics")
                    }
                    SettingsSection::Advanced => deepseekcode_i18n::t("settings.section.session"),
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
    current_language: deepseekcode_i18n::LanguagePref,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let mut row = div().flex().gap_2();
    for id in ThemeId::ALL {
        row = row.child(theme_choice(cx, id, current_theme, &theme));
    }
    let mut lang_row = div().flex().flex_col().gap_2();
    for pref in deepseekcode_i18n::LanguagePref::all() {
        lang_row = lang_row.child(language_choice(cx, pref, current_language, &theme));
    }
    // Detected language, for the `Auto` row context. Shown once, not per row.
    let detected = deepseekcode_i18n::detect::detection_report();
    let detected_label = match detected.locale {
        Some(locale) => deepseekcode_i18n::tr!(
            "settings.language_detected",
            "language" => locale.endonym()
        ),
        None => String::new(),
    };
    let lang_body =
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(lang_row)
            .when(!detected_label.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(detected_label),
                )
            });
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.theme"),
            deepseekcode_i18n::t("settings.theme_desc"),
            row,
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.language"),
            deepseekcode_i18n::t("settings.language_desc"),
            lang_body,
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
        .child(id.localized_label())
}

fn language_choice(
    cx: &Context<crate::workspace::WorkspaceView>,
    pref: deepseekcode_i18n::LanguagePref,
    current: deepseekcode_i18n::LanguagePref,
    theme: &ThemeTokens,
) -> impl IntoElement {
    use deepseekcode_i18n::{LanguagePref, Locale};
    let active = pref == current;
    // `Auto` shows the localized "System (detected)" row; a pinned locale
    // shows its endonym so it is readable in any UI language.
    let label: String = match pref {
        LanguagePref::Auto => deepseekcode_i18n::t("common.language_system"),
        LanguagePref::Locale(locale) => locale.endonym().to_string(),
    };
    // Secondary hint: the language name in the *current* UI language, so a
    // user can confirm what they picked (e.g. endonym `日本語` + `日语`).
    let hint: Option<String> = match pref {
        LanguagePref::Auto => {
            let detected = deepseekcode_i18n::detect::detection_report()
                .locale
                .unwrap_or(Locale::En);
            Some(detected.endonym().to_string())
        }
        LanguagePref::Locale(locale) => {
            let key = format!("language.name.{}", locale.tag());
            deepseekcode_i18n::t_strict(&key)
        }
    };
    let row = div()
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
            cx.listener(move |this, _, _, cx| this.set_language(pref, cx))
        })
        .child(label);
    match hint {
        Some(hint) => row
            .child(div().text_xs().text_color(theme.text_muted).child(hint))
            .into_any_element(),
        None => row.into_any_element(),
    }
}

fn render_ai_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    providers: &[ProviderInfo],
    selected_agent: &str,
    selected_model: &str,
    selected_provider: Option<&str>,
    custom_model_input: Entity<crate::composer::PromptInput>,
    custom_models: &[CustomModelEntry],
    has_project: bool,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    // Custom model ids only make sense for providers that proxy arbitrary
    // ids (OpenRouter / OpenAI-compatible). Default to OpenRouter.
    let custom_provider_id = match selected_provider {
        Some("openrouter") | Some("openai-compatible") => {
            selected_provider.unwrap_or("openrouter").to_string()
        }
        _ => "openrouter".to_string(),
    };
    let custom_provider_name = providers
        .iter()
        .find(|p| p.id == custom_provider_id)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| custom_provider_id.clone());
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.default_agent"),
            deepseekcode_i18n::t("settings.default_agent_desc"),
            render_agent_choices(cx, selected_agent),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.default_model"),
            deepseekcode_i18n::t("settings.default_model_desc"),
            render_model_choices(cx, providers, selected_model, selected_provider),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.providers"),
            deepseekcode_i18n::t("settings.providers_desc"),
            render_providers(cx, providers, has_project),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.custom_models"),
            deepseekcode_i18n::t("settings.custom_models_desc"),
            render_custom_model_manager(
                cx,
                &theme,
                providers,
                custom_models,
                custom_model_input,
                custom_provider_id,
                custom_provider_name,
            ),
        ))
}

/// User-managed model list: existing entries with per-row Remove plus an
/// input + Add row. Adding also selects the new id as the default model.
fn render_custom_model_manager(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    providers: &[ProviderInfo],
    custom_models: &[CustomModelEntry],
    custom_model_input: Entity<crate::composer::PromptInput>,
    provider_id: String,
    provider_name: String,
) -> impl IntoElement {
    let mut body = div().flex().flex_col().gap_2();
    if custom_models.is_empty() {
        body = body.child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::t("settings.custom_models_empty")),
        );
    } else {
        for entry in custom_models {
            let entry_provider = entry.provider_id.clone();
            let entry_id = entry.id.clone();
            let provider_label = providers
                .iter()
                .find(|p| p.id == entry_provider)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| entry_provider.clone());
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.bg_elevated)
                    .child(
                        div()
                            .text_xs()
                            .font_family(crate::theme::font_mono(&theme))
                            .text_color(theme.text_secondary)
                            .child(format!("{provider_label} | {}", entry.id)),
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
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.remove_custom_model(&entry_provider, &entry_id, cx);
                                }),
                            )
                            .child(deepseekcode_i18n::t("common.remove")),
                    )
                    .into_any_element(),
            );
        }
    }
    body = body.child(
        div()
            .rounded_md()
            .border_1()
            .border_color(theme.border_default)
            .bg(theme.bg_elevated)
            .px_2()
            .child(custom_model_input.clone()),
    );
    body = body.child(
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(theme.bg_selected)
            .text_xs()
            .text_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.add_custom_model(&provider_id, cx);
                }),
            )
            .child(deepseekcode_i18n::tr!(
                "settings.add_to_provider",
                "provider" => provider_name.as_str()
            )),
    );
    body
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
            deepseekcode_i18n::t("settings.prompt_delivery"),
            deepseekcode_i18n::t("settings.prompt_delivery_desc"),
            div()
                .flex()
                .gap_2()
                .child(delivery_choice(
                    cx,
                    PromptDelivery::Steer,
                    deepseekcode_i18n::t("delivery.steer_default"),
                    prompt_delivery,
                ))
                .child(delivery_choice(
                    cx,
                    PromptDelivery::Queue,
                    deepseekcode_i18n::t("delivery.queue"),
                    prompt_delivery,
                )),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.data_location"),
            deepseekcode_i18n::t("settings.data_location_desc"),
            div()
                .text_xs()
                .font_family(crate::theme::font_mono(&theme))
                .text_color(theme.text_secondary)
                .child(
                    deepseekcode_types::data_dir()
                        .map(|d| d.join("deepseekcode").display().to_string())
                        .unwrap_or_else(|| "~/.local/share/deepseekcode".to_string()),
                ),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.remote_server"),
            deepseekcode_i18n::t("settings.remote_server_desc"),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_server_settings(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("settings.save_server")),
                ),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.multi_workspace"),
            deepseekcode_i18n::t("settings.multi_workspace_desc"),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_new_window(cx);
                    }),
                )
                .child(deepseekcode_i18n::t("settings.new_window")),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.keyboard"),
            deepseekcode_i18n::t("settings.keyboard_desc"),
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::t("settings.shortcut_enter"))
                .child(deepseekcode_i18n::t("settings.shortcut_select_all"))
                .child(deepseekcode_i18n::t("settings.shortcut_clipboard")),
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_settings_section(SettingsSection::Ai, cx);
                    }),
                )
                .child(deepseekcode_i18n::t("settings.goto_providers")),
        )
}

fn delivery_choice(
    cx: &Context<crate::workspace::WorkspaceView>,
    delivery: PromptDelivery,
    label: String,
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
            deepseekcode_i18n::t("settings.project_directories"),
            deepseekcode_i18n::t("settings.project_directories_desc"),
            if catalog.project_directories.is_empty() {
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(if catalog.loaded {
                        deepseekcode_i18n::t("settings.project_directories_empty")
                    } else {
                        deepseekcode_i18n::t("common.loading")
                    })
            } else {
                div().flex().flex_col().gap_1().children(
                    catalog
                        .project_directories
                        .iter()
                        .map(|path| {
                            div()
                                .text_xs()
                                .font_family(crate::theme::font_mono(&theme))
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
            deepseekcode_i18n::t("settings.agents_md"),
            deepseekcode_i18n::t("settings.agents_md_desc"),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_agents_md(cx);
                    }),
                )
                .child(deepseekcode_i18n::t("settings.open_agents_md")),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.configuration"),
            deepseekcode_i18n::tr!(
                "settings.configuration_desc",
                "product" => PRODUCT_NAME,
                "file" => CONFIG_FILE
            ),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_project_config(cx);
                    }),
                )
                .child(deepseekcode_i18n::tr!(
                    "settings.open_config",
                    "product" => PRODUCT_NAME
                )),
        ))
}

fn render_permissions_section(
    cx: &Context<crate::workspace::WorkspaceView>,
    catalog: &SettingsCatalog,
    permission_allow_input: Entity<crate::composer::PromptInput>,
    policy_pattern_input: Entity<crate::composer::PromptInput>,
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
            deepseekcode_i18n::t("settings.policy_mode"),
            deepseekcode_i18n::t("settings.policy_mode_desc"),
            div()
                .flex()
                .gap_2()
                .child(policy_mode_button(
                    cx,
                    &theme,
                    false,
                    deepseekcode_i18n::t("settings.policy_interactive"),
                    !policy_auto_mode,
                ))
                .child(policy_mode_button(
                    cx,
                    &theme,
                    true,
                    deepseekcode_i18n::t("settings.policy_auto"),
                    policy_auto_mode,
                )),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.policy_rules"),
            deepseekcode_i18n::tr!(
                "settings.policy_rules_desc",
                "product" => PRODUCT_NAME,
                "file" => CONFIG_FILE
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
                            deepseekcode_i18n::t("settings.policy_rules_empty")
                        } else {
                            deepseekcode_i18n::t("settings.policy_rules_no_project")
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
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.add_policy_rule_row(cx);
                                    }),
                                )
                                .child(deepseekcode_i18n::t("settings.policy_add_rule")),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .text_xs()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.apply_policy_preset("dev", cx);
                                    }),
                                )
                                .child(deepseekcode_i18n::t("settings.policy_dev_preset")),
                        )
                        .child(policy_preset_button(
                            cx,
                            &theme,
                            "readonly",
                            deepseekcode_i18n::t("settings.policy_readonly_preset"),
                        ))
                        .child(policy_preset_button(
                            cx,
                            &theme,
                            "strict",
                            deepseekcode_i18n::t("settings.policy_strict_preset"),
                        )),
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
                                    .child(deepseekcode_i18n::t("workspace.pattern_for_rule")),
                            )
                            .child(div().min_h(px(28.)).child(policy_pattern_input.clone())),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(deepseekcode_i18n::t("settings.policy_advanced")),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_policy_rules(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("settings.policy_save")),
                ),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.remembered_allows"),
            deepseekcode_i18n::t("settings.remembered_allows_desc"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(if catalog.policy_grants.is_empty() {
                    vec![div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(if catalog.loaded {
                            deepseekcode_i18n::t("settings.remembered_allows_empty")
                        } else {
                            deepseekcode_i18n::t("settings.remembered_allows_no_project")
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
                                        .font_family(crate::theme::font_mono(&theme))
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
                                        .child(deepseekcode_i18n::t("common.delete")),
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
    label: String,
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
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.set_policy_auto_mode(auto, cx);
            }),
        )
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
        .child(
            div()
                .w(px(120.))
                .child(deepseekcode_i18n::t("settings.policy_column_scope")),
        )
        .child(
            div()
                .flex_1()
                .child(deepseekcode_i18n::t("settings.policy_column_pattern")),
        )
        .child(
            div()
                .w(px(72.))
                .child(deepseekcode_i18n::t("settings.policy_column_action")),
        )
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
                .font_family(crate::theme::font_mono(theme))
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
                .font_family(crate::theme::font_mono(theme))
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
                .child(deepseekcode_i18n::t("common.remove")),
        )
}

fn policy_action_label(action: PolicyAction) -> String {
    match action {
        PolicyAction::Permit => deepseekcode_i18n::t("policy.action.permit"),
        PolicyAction::Prompt => deepseekcode_i18n::t("policy.action.prompt"),
        PolicyAction::Block => deepseekcode_i18n::t("policy.action.block"),
    }
}

fn policy_preset_button(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    preset: &'static str,
    label: String,
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
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.apply_policy_preset(preset, cx);
            }),
        )
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
            deepseekcode_i18n::t("settings.location"),
            deepseekcode_i18n::t("settings.location_desc"),
            div()
                .text_xs()
                .font_family(crate::theme::font_mono(&theme))
                .text_color(theme.text_secondary)
                .child(if catalog.location_summary.is_empty() {
                    deepseekcode_i18n::t("common.loading")
                } else {
                    catalog.location_summary.clone()
                }),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.mcp_servers"),
            deepseekcode_i18n::t("settings.mcp_servers_desc"),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.refresh_mcp_servers(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("settings.mcp_refresh")),
                )
                .child(render_mcp_list(cx, catalog)),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.skills"),
            deepseekcode_i18n::t("settings.skills_desc"),
            render_string_list(
                &theme,
                &catalog.skills,
                catalog.loaded,
                deepseekcode_i18n::t("settings.skills_empty"),
            ),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.plugins"),
            deepseekcode_i18n::t("settings.plugins_desc"),
            render_string_list(
                &theme,
                &catalog.plugins,
                catalog.loaded,
                deepseekcode_i18n::t("settings.plugins_empty"),
            ),
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
            deepseekcode_i18n::t("git.worktree_branch_name"),
            deepseekcode_i18n::t("git.worktree_branch_name_desc"),
            worktree_branch_input.clone(),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("git.worktrees"),
            deepseekcode_i18n::t("git.worktrees_desc"),
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
            deepseekcode_i18n::t("settings.health"),
            deepseekcode_i18n::t("settings.health_desc"),
            div()
                .text_sm()
                .text_color(if catalog.health_status == "ok" {
                    theme.success
                } else {
                    theme.warning
                })
                .child(deepseekcode_i18n::tr!(
                    "settings.health_status",
                    "status" => catalog.health_status.as_str()
                )),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.diagnostics_export"),
            deepseekcode_i18n::t("settings.diagnostics_export_desc"),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.info)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.export_diagnostics(cx);
                    }),
                )
                .child(deepseekcode_i18n::t("settings.diagnostics_export_action")),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.openapi"),
            deepseekcode_i18n::t("settings.openapi_desc"),
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.bg_elevated)
                .text_sm()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_openapi_docs(cx);
                    }),
                )
                .child(deepseekcode_i18n::t("settings.openapi_open")),
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
            deepseekcode_i18n::t("settings.compaction"),
            deepseekcode_i18n::t("settings.compaction_desc"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_family(crate::theme::font_mono(&theme))
                        .text_color(theme.text_secondary)
                        .child(if catalog.loaded {
                            catalog.compaction_summary.clone()
                        } else {
                            deepseekcode_i18n::t("settings.no_project")
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.compact_current_session(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("settings.compact_now")),
                ),
        ))
        .child(settings_group(
            &theme,
            deepseekcode_i18n::t("settings.session_section"),
            deepseekcode_i18n::t("settings.session_section_desc"),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.new_session(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("session.new")),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.fork_current_session(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("session.fork")),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_export_sanitize(cx);
                            }),
                        )
                        .child(if export_sanitize {
                            deepseekcode_i18n::t("settings.sanitize_on")
                        } else {
                            deepseekcode_i18n::t("settings.sanitize_off")
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.export_current_session(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("session.export")),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.open_revert_dock(cx);
                            }),
                        )
                        .child(deepseekcode_i18n::t("session.revert")),
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
                .child(deepseekcode_i18n::t("settings.no_project")),
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.create_worktree(cx);
                        }),
                    )
                    .child(deepseekcode_i18n::t("git.worktree_create")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.bg_hover)
                    .text_sm()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.prune_worktrees(cx);
                        }),
                    )
                    .child(deepseekcode_i18n::t("git.worktree_prune")),
            ),
    );

    if catalog.worktrees.is_empty() {
        body = body.child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::t("git.worktree_none")),
        );
    } else {
        for entry in &catalog.worktrees {
            let path = entry.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let branch = entry
                .get("branch")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| deepseekcode_i18n::t("git.worktree_detached"));
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
                                    .font_family(crate::theme::font_mono(&theme))
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
                                    .child(deepseekcode_i18n::t("common.open")),
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
                                    .child(deepseekcode_i18n::t("common.remove")),
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
            .child(deepseekcode_i18n::t("common.loading"));
    }
    if catalog.mcp.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child(deepseekcode_i18n::t("settings.mcp_empty"));
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
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child(name.to_string()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if connected {
                                    theme.success
                                } else {
                                    theme.text_muted
                                })
                                .child(if connected {
                                    deepseekcode_i18n::t("settings.mcp_connected")
                                } else {
                                    deepseekcode_i18n::t("settings.mcp_disconnected")
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(theme.text_disabled).child(
                            deepseekcode_i18n::tr!(
                                "settings.mcp_tool_count",
                                "count" => tools as u64
                            ),
                        ))
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
                                .child(deepseekcode_i18n::t("common.disconnect"))
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
                                .child(deepseekcode_i18n::t("common.connect"))
                                .into_any_element()
                        }),
                ),
        );
    }
    list
}

fn render_string_list(
    theme: &ThemeTokens,
    items: &[String],
    loaded: bool,
    empty_label: String,
) -> impl IntoElement {
    if !loaded {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child(deepseekcode_i18n::t("common.loading"));
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
                .font_family(crate::theme::font_mono(&theme))
                .child(item.clone()),
        );
    }
    list
}

fn render_providers(
    cx: &Context<crate::workspace::WorkspaceView>,
    providers: &[ProviderInfo],
    has_project: bool,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    if providers.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.text_muted)
            .child(if has_project {
                deepseekcode_i18n::t("settings.providers_load_failed")
            } else {
                deepseekcode_i18n::t("settings.no_project_open")
            });
    }
    let mut list = div().flex().flex_col().gap_2();
    for provider in providers {
        let provider_id = provider.id.clone();
        let disconnect_id = provider.id.clone();
        let status = if provider.connected {
            deepseekcode_i18n::t("settings.connected")
        } else {
            deepseekcode_i18n::t("settings.not_connected")
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
                    deepseekcode_i18n::t("settings.update_key")
                } else {
                    deepseekcode_i18n::t("common.connect")
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
                    .child(deepseekcode_i18n::t("common.disconnect")),
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
                        .child(div().text_xs().text_color(status_color).child(status)),
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
            .child(deepseekcode_i18n::t("settings.connect_provider_first"))
            .into_any_element();
    }

    let mut list = div().flex().flex_col().gap_1();
    // A custom model id (set via "Custom model" below) is not in the
    // catalog — surface it as the active row so the selection is visible
    // instead of silently showing nothing selected.
    let known = models.iter().any(|(_, model_id, model_name, _)| {
        selected_model == model_id || selected_model == model_name
    });
    if !known && !selected_model.is_empty() {
        let custom_id = selected_model.to_string();
        let custom_provider = selected_provider.unwrap_or("openrouter").to_string();
        let custom_label = deepseekcode_i18n::tr!(
            "settings.custom_model_selected",
            "model" => selected_model
        );
        list = list.child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(theme.bg_selected)
                .border_1()
                .border_color(theme.border_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_model(&custom_id, &custom_id, &custom_provider, cx);
                    }),
                )
                .child(custom_label),
        );
    }
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
    title: impl IntoElement,
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
