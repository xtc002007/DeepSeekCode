//! Cursor-parity Git workbench (P0-P5).
//!
//! Layout mirrors Cursor's Source Control sidebar:
//! toolbar -> commit box (CHANGES) -> file groups -> AGENT REVIEW ->
//! GRAPH -> STASH -> REMOTES. All actions delegate to
//! `WorkspaceView::git_*` async ops (which call `core::git::ops`).

use crate::logic::{GitCommitNode, GitStatusEntry};
use crate::ui::icon::render_icon_rgba;
use crate::ui::icon::IconName;
use crate::workspace::WorkspaceView;
use gpui::prelude::FluentBuilder;
use gpui::*;

actions!(
    git_commit_box,
    [GitCommitSubmit, GitBranchSubmit, GitStashSubmit, GitReviewSubmit]
);

/// All `PromptInput` key contexts owned by the Git panel. The stock editing
/// bindings in `bind_prompt_keys` only cover `"PromptInput"`, so every git
/// input must be registered explicitly or backspace/arrows/clipboard go dead.
pub const GIT_INPUT_CONTEXTS: &[&str] = &[
    "GitCommitInput",
    "GitFilterInput",
    "GitBranchInput",
    "GitStashInput",
    "GitReviewScopeInput",
];

/// Hover tooltip for a change row: full repo-relative path + status,
/// mirroring Cursor (truncated name in the row, full path on hover).
pub(crate) struct GitFileTooltip {
    pub(crate) text: SharedString,
}

impl Render for GitFileTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_default)
            .text_xs()
            .font_family(crate::theme::font_mono(&theme))
            .text_color(theme.text_primary)
            .child(self.text.clone())
    }
}

pub struct GitPanel;

impl GitPanel {
    pub fn render(cx: &mut Context<WorkspaceView>, view: &WorkspaceView) -> impl IntoElement {
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
        let mut list = div()
            .size_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .overflow_hidden();

        list = list.child(render_toolbar(cx, view, &theme));

        // Verdict card: one-line title (success vs failure is unambiguous),
        // raw hook dumps stay in a bounded scroll + copy + retry actions.
        if let Some(err) = view.git_error.as_deref().filter(|s| !s.is_empty()) {
            let detail = view.git_error_detail.clone().unwrap_or_default();
            let expanded = view.git_error_expanded && !detail.is_empty();
            let copy_text: SharedString = if detail.is_empty() {
                err.to_string().into()
            } else {
                detail.clone().into()
            };
            let hookish = view.git_error_looks_like_hook();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.error)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(render_icon_rgba(
                                IconName::AlertCircle,
                                px(14.),
                                theme.text_on_accent,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_on_accent)
                                    .child(err.to_string()),
                            )
                            .child(
                                div()
                                    .id("git-error-copy")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(22.))
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| GitFileTooltip {
                                            text: code_plus_plus_i18n::t("common.copy").into(),
                                        })
                                        .into()
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy_text.to_string(),
                                            ));
                                        }),
                                    )
                                    .child(render_icon_rgba(
                                        IconName::Copy,
                                        px(12.),
                                        theme.text_on_accent,
                                    )),
                            )
                            .when(!detail.is_empty(), |el| {
                                el.child(
                                    div()
                                        .id("git-error-toggle")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(22.))
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.bg_hover))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.git_error_expanded =
                                                    !this.git_error_expanded;
                                                cx.notify();
                                            }),
                                        )
                                        .child(render_icon_rgba(
                                            if expanded {
                                                IconName::ChevronUp
                                            } else {
                                                IconName::ChevronDown
                                            },
                                            px(12.),
                                            theme.text_on_accent,
                                        )),
                                )
                            }),
                    )
                    .when(expanded, |el| {
                        el.child(
                            div()
                                .id("git-error-detail")
                                .max_h(px(200.))
                                .overflow_y_scroll()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_base)
                                .text_xs()
                                .font_family(crate::theme::font_mono(&theme))
                                .text_color(theme.text_primary)
                                .child(detail),
                        )
                    })
                    .when(hookish, |el| {
                        el.child(
                            div()
                                .id("git-retry-no-verify")
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.bg_base)
                                .text_color(theme.error)
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.git_commit_no_verify = true;
                                        this.git_commit(false, cx);
                                    }),
                                )
                                .child(code_plus_plus_i18n::t("git.retry_no_verify")),
                        )
                    }),
            );
        }
        if let Some(notice) = view.git_notice.as_deref().filter(|s| !s.is_empty()) {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.bg_elevated)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child(notice.to_string()),
            );
        }
        if view.git_busy {
            list = list.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(code_plus_plus_i18n::t("git.loading")),
            );
        }
        if let Some(confirm) = view.git_pending_confirm.clone() {
            let detail = confirm.detail.clone();
            let title = confirm.title.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.error)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_on_accent)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .child(detail),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("git-confirm-yes")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(theme.bg_base)
                                    .text_color(theme.error)
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_confirm_pending(cx);
                                        }),
                                    )
                                    .child(code_plus_plus_i18n::t("common.ok")),
                            )
                            .child(
                                div()
                                    .id("git-confirm-no")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(theme.text_on_accent)
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_cancel_confirm(cx);
                                        }),
                                    )
                                    .child(code_plus_plus_i18n::t("common.cancel")),
                            ),
                    ),
            );
        }

        if let Some(message) = view.git_status_message.as_deref().filter(|s| !s.is_empty()) {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(message.to_string()),
            );
        } else if let Some(summary) = view.git_diff_summary.as_deref().filter(|s| !s.is_empty()) {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_family(crate::theme::font_mono(&theme))
                    .text_color(theme.text_muted)
                    .child(summary.to_string()),
            );
        }

        list = list.child(render_changes_section(cx, view, &theme));
        list = list.child(render_agent_review(cx, view, &theme));
        list = list.child(render_graph_section(cx, view, &theme));
        list = list.child(render_stash_section(cx, view, &theme));
        list = list.child(render_remotes_section(cx, view, &theme));

        // Legacy recent-commits fallback when graph is empty.
        if view.git_commits.is_empty() && !view.git_log.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_link)
                    .child(code_plus_plus_i18n::t("git.recent_commits")),
            );
            for line in view.git_log.iter().take(20) {
                list = list.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .font_family(crate::theme::font_mono(&theme))
                        .text_color(theme.text_secondary)
                        .child(line.clone()),
                );
            }
        }

        // Cursor-style single page: every section owns its own scrollbar, so
        // the commit box / review button / graph header stay pinned while
        // long lists scroll underneath. No outer scroll.
        div()
            .id("git-panel")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(list)
    }
}

// ---------------------------------------------------------------------------
// Toolbar: Refresh / Fetch / Pull / Push / branch + ahead-behind / search
// ---------------------------------------------------------------------------

fn render_toolbar(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let branch_label = view
        .git_current_branch
        .clone()
        .unwrap_or_else(|| code_plus_plus_i18n::t("git.branch"));
    let sync_label = if view.git_ahead > 0 || view.git_behind > 0 {
        format!("↑{} ↓{}", view.git_ahead, view.git_behind)
    } else if view.git_current_branch.is_some() {
        code_plus_plus_i18n::t("git.no_upstream")
    } else {
        String::new()
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(toolbar_btn(cx, "git-tb-refresh", IconName::RefreshCw, {
                    let _ = view.git_current_branch.clone();
                    move |this, _, _, cx| this.refresh_git_status_async(cx)
                }))
                .child(toolbar_btn(
                    cx,
                    "git-tb-fetch",
                    IconName::Download,
                    |this, _, _, cx| this.git_fetch(cx),
                ))
                .child(toolbar_btn(
                    cx,
                    "git-tb-pull",
                    IconName::ArrowRight,
                    |this, _, _, cx| this.git_pull(cx),
                ))
                .child(toolbar_btn(
                    cx,
                    "git-tb-push",
                    IconName::Upload,
                    |this, _, _, cx| this.git_push(cx),
                ))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .child(render_icon_rgba(
                            IconName::GitBranch,
                            px(13.),
                            theme.text_secondary,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_primary)
                                .child(branch_label),
                        )
                        .when(!sync_label.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_xs()
                                    .font_family(crate::theme::font_mono(theme))
                                    .text_color(theme.text_muted)
                                    .child(sync_label),
                            )
                        }),
                ),
        )
}

fn toolbar_btn(
    cx: &mut Context<WorkspaceView>,
    id: &'static str,
    icon: IconName,
    on_click: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
        + 'static,
) -> Stateful<Div> {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .child(render_icon_rgba(icon, px(14.), theme.text_secondary))
        .on_mouse_down(MouseButton::Left, cx.listener(on_click))
}

// ---------------------------------------------------------------------------
// CHANGES: commit box + file groups
// ---------------------------------------------------------------------------

fn render_changes_section(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let filter = view.git_filter_input.read(cx).text().trim().to_string();
    let staged = crate::logic::filter_changes(&view.git_staged, &filter);
    let unstaged = crate::logic::filter_changes(&view.git_unstaged, &filter);
    let untracked = crate::logic::filter_changes(&view.git_untracked, &filter);
    let conflicted = crate::logic::filter_changes(&view.git_conflicted, &filter);
    let total = staged.len() + unstaged.len() + untracked.len() + conflicted.len();

    let header = section_header(
        cx,
        "git-sec-changes",
        &code_plus_plus_i18n::t("git.changes"),
        &format!("{total}"),
        view.git_changes_expanded,
        |this, _, _, cx| {
            this.git_changes_expanded = !this.git_changes_expanded;
            cx.notify();
        },
        theme,
    );

    let show_tree = view.git_show_tree;
    let selected_count = view.git_selected.len();
    let tree_toggle_label = if show_tree {
        code_plus_plus_i18n::t("git.show_tree")
    } else {
        code_plus_plus_i18n::t("git.show_list")
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_h(px(0.))
        .flex_shrink_1()
        .overflow_hidden()
        .child(header)
        .when(view.git_changes_expanded, |el| {
            el.child(render_commit_box(cx, view, theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .child(small_action(
                            cx,
                            "git-view-toggle",
                            &tree_toggle_label,
                            theme,
                            |this, _, _, cx| this.git_toggle_tree(cx),
                        ))
                        .child(small_action(
                            cx,
                            "git-bulk-stage-all",
                            &code_plus_plus_i18n::t("git.stage_all"),
                            theme,
                            |this, _, _, cx| {
                                this.git_stage_all(cx);
                            },
                        ))
                        .child(small_action(
                            cx,
                            "git-bulk-unstage-all",
                            &code_plus_plus_i18n::t("git.unstage_all"),
                            theme,
                            |this, _, _, cx| {
                                this.git_unstage_all(cx);
                            },
                        ))
                        .child(small_action(
                            cx,
                            "git-bulk-discard-all",
                            &code_plus_plus_i18n::t("git.discard_all"),
                            theme,
                            |this, _, _, cx| {
                                this.git_request_discard_all(cx);
                            },
                        )),
                )
                .when(selected_count > 0, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .mx_2()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.bg_selected)
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(theme.text_primary)
                                    .child(format!("{selected_count} selected")),
                            )
                            .child(small_action(
                                cx,
                                "git-sel-stage",
                                &code_plus_plus_i18n::t("git.stage"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_stage_paths(Vec::new(), cx);
                                },
                            ))
                            .child(small_action(
                                cx,
                                "git-sel-unstage",
                                &code_plus_plus_i18n::t("git.unstage"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_unstage_paths(Vec::new(), cx);
                                },
                            ))
                            .child(small_action(
                                cx,
                                "git-sel-discard",
                                &code_plus_plus_i18n::t("git.discard"),
                                theme,
                                |this, _, _, cx| {
                                    let sel: Vec<String> =
                                        this.git_selected.iter().cloned().collect();
                                    this.git_request_discard_paths(sel, cx);
                                },
                            ))
                            .child(small_action(
                                cx,
                                "git-sel-clear",
                                &code_plus_plus_i18n::t("common.close"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_clear_selection(cx);
                                },
                            )),
                    )
                })
                .child(
                    div()
                        .id("git-changes-scroll")
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .overflow_y_scroll()
                        .child(render_group(
                            cx,
                            view,
                            theme,
                            "git-grp-conflict",
                            &code_plus_plus_i18n::t("git.merge_conflicts"),
                            &conflicted,
                            view.git_conflict_expanded,
                            |this, _, _, cx| {
                                this.git_conflict_expanded = !this.git_conflict_expanded;
                                cx.notify();
                            },
                            GroupKind::Conflicted,
                        ))
                        .child(render_group(
                            cx,
                            view,
                            theme,
                            "git-grp-staged",
                            &code_plus_plus_i18n::t("git.staged"),
                            &staged,
                            view.git_staged_expanded,
                            |this, _, _, cx| {
                                this.git_staged_expanded = !this.git_staged_expanded;
                                cx.notify();
                            },
                            GroupKind::Staged,
                        ))
                        .child(render_group(
                            cx,
                            view,
                            theme,
                            "git-grp-changes",
                            &code_plus_plus_i18n::t("git.changes_group"),
                            &unstaged,
                            true,
                            |_, _, _, _| {},
                            GroupKind::Unstaged,
                        ))
                        .child(render_group(
                            cx,
                            view,
                            theme,
                            "git-grp-untracked",
                            &code_plus_plus_i18n::t("git.untracked"),
                            &untracked,
                            view.git_untracked_expanded,
                            |this, _, _, cx| {
                                this.git_untracked_expanded = !this.git_untracked_expanded;
                                cx.notify();
                            },
                            GroupKind::Untracked,
                        ))
                        .when(total == 0, |el| {
                            el.child(
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(theme.text_muted)
                                    .child(code_plus_plus_i18n::t("git.clean")),
                            )
                        }),
                )
        })
}

fn render_commit_box(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let no_verify = view.git_commit_no_verify;
    // While a git op is in flight the commit controls go inert (no handler)
    // and the main button shows a spinner — double-submit protection.
    let busy = view.git_busy;
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(30.))
                .rounded_md()
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border_subtle)
                .child(div().flex_1().h_full().child(view.git_commit_input.clone()))
                .child(
                    div()
                        .id("git-generate-message")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded_md()
                        .when(!view.git_ai_busy, |el| {
                            el.cursor_pointer()
                                .hover(|s| s.bg(theme.bg_hover))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.git_generate_message(cx);
                                    }),
                                )
                        })
                        .child(render_icon_rgba(
                            if view.git_ai_busy {
                                IconName::Loader
                            } else {
                                IconName::Wand
                            },
                            px(14.),
                            theme.text_secondary,
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    // Cursor-style split button: always blue, chevron opens
                    // the overflow menu (Commit & Push / Amend / Undo / …).
                    div()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .overflow_hidden()
                        .bg(Rgba::from(theme.accent))
                        .when(!busy, |el| {
                            el.hover(|s| s.bg(Rgba::from(theme.accent_hover)))
                        })
                        .when(busy, |el| el.opacity(0.75))
                        .child(
                            div()
                                .id("git-commit-btn")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_on_accent)
                                .when(!busy, |el| {
                                    el.cursor_pointer().on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_commit(false, cx);
                                        }),
                                    )
                                })
                                .child(render_icon_rgba(
                                    if busy {
                                        IconName::Loader
                                    } else {
                                        IconName::Check
                                    },
                                    px(14.),
                                    theme.text_on_accent,
                                ))
                                .child(code_plus_plus_i18n::t("git.commit")),
                        )
                        .child(
                            div()
                                .w(px(1.))
                                .h(px(20.))
                                .bg(theme.text_on_accent)
                                .opacity(0.35),
                        )
                        .child(
                            div()
                                .id("git-commit-menu-toggle")
                                .flex()
                                .items_center()
                                .justify_center()
                                .px_2()
                                .py_1()
                                .when(!busy, |el| {
                                    el.cursor_pointer().on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_commit_menu_open =
                                                !this.git_commit_menu_open;
                                            cx.notify();
                                        }),
                                    )
                                })
                                .child(render_icon_rgba(
                                    if view.git_commit_menu_open {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    },
                                    px(14.),
                                    theme.text_on_accent,
                                )),
                        ),
                )
                .when(view.git_commit_menu_open, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme.bg_elevated)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .child(menu_row(
                                cx,
                                "git-commit-push-btn",
                                &code_plus_plus_i18n::t("git.commit_and_push"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_commit(true, cx);
                                },
                            ))
                            .child(menu_row(
                                cx,
                                "git-amend",
                                &code_plus_plus_i18n::t("git.amend"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_amend(cx);
                                },
                            ))
                            .child(menu_row(
                                cx,
                                "git-undo",
                                &code_plus_plus_i18n::t("git.undo_last_commit"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_undo_last_commit(cx);
                                },
                            ))
                            .child(
                                div()
                                    .id("git-no-verify-toggle")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_commit_no_verify =
                                                !this.git_commit_no_verify;
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .size(px(12.))
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(theme.border_default)
                                            .bg(if no_verify {
                                                Rgba::from(theme.accent)
                                            } else {
                                                theme.bg_elevated
                                            })
                                            .when(no_verify, |el| {
                                                el.child(div().size_full())
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .child(code_plus_plus_i18n::t(
                                                "git.commit_no_verify",
                                            )),
                                    ),
                            ),
                    )
                }),
        )
}

/// Full-width dropdown menu row (Commit ▾ / Find Issues ▾ overflow).
fn menu_row(
    cx: &mut Context<WorkspaceView>,
    id: impl Into<ElementId>,
    label: &str,
    theme: &crate::theme::ThemeTokens,
    on_click: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
    + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .text_color(theme.text_primary)
        .hover(|s| s.bg(theme.bg_hover))
        .on_mouse_down(MouseButton::Left, cx.listener(on_click))
        .child(label.to_string())
}

fn small_action(
    cx: &mut Context<WorkspaceView>,
    id: impl Into<ElementId>,
    label: &str,
    theme: &crate::theme::ThemeTokens,
    on_click: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
    + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .bg(theme.bg_elevated)
        .text_color(theme.text_secondary)
        .hover(|s| s.bg(theme.bg_hover))
        .on_mouse_down(MouseButton::Left, cx.listener(on_click))
        .child(label.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}

#[allow(clippy::too_many_arguments)]
fn render_group(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
    _id: &'static str,
    title: &str,
    entries: &[GitStatusEntry],
    expanded: bool,
    on_toggle: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
        + 'static,
    kind: GroupKind,
) -> AnyElement {
    if entries.is_empty() {
        return div().into_any_element();
    }
    let accent = match kind {
        GroupKind::Staged => theme.success,
        GroupKind::Conflicted => theme.error,
        GroupKind::Unstaged => theme.warning,
        GroupKind::Untracked => theme.text_muted,
    };
    let mut col = div().flex().flex_col().gap_0p5();
    col = col.child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(on_toggle))
            .child(render_icon_rgba(
                if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                px(12.),
                theme.text_muted,
            ))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(accent)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(crate::theme::font_mono(theme))
                    .text_color(theme.text_muted)
                    .child(format!("{}", entries.len())),
            )
            .child(div().flex_1())
            .child(group_bulk_actions(cx, kind, theme)),
    );
    if expanded {
        // Perf guard P5: cap initial paint, search narrows further.
        let cap = 400usize;
        if view.git_show_tree {
            col = render_tree_entries(cx, view, theme, entries, cap, kind, col);
        } else {
            for entry in entries.iter().take(cap) {
                col = col.child(render_file_row(cx, view, theme, entry, kind));
            }
            if entries.len() > cap {
                col = col.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(format!("+ {} more", entries.len() - cap)),
                );
            }
        }
    }
    col.into_any_element()
}

/// Tree view (P5): group flat change list by top-level directory.
fn render_tree_entries(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
    entries: &[GitStatusEntry],
    cap: usize,
    kind: GroupKind,
    mut col: Div,
) -> Div {
    use std::collections::BTreeMap;
    let mut dirs: BTreeMap<String, Vec<&GitStatusEntry>> = BTreeMap::new();
    for entry in entries.iter().take(cap) {
        let dir = std::path::Path::new(&entry.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ".".to_string());
        dirs.entry(dir).or_default().push(entry);
    }
    for (dir, files) in dirs {
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .child(render_icon_rgba(IconName::Folder, px(12.), theme.warning))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(theme.text_primary)
                        .child(format!("{dir} ({})", files.len())),
                ),
        );
        for entry in files {
            col = col.child(
                div()
                    .pl(px(16.))
                    .child(render_file_row(cx, view, theme, entry, kind)),
            );
        }
    }
    if entries.len() > cap {
        col = col.child(
            div()
                .px_2()
                .text_xs()
                .text_color(theme.text_muted)
                .child(format!("+ {} more", entries.len() - cap)),
        );
    }
    col
}

fn group_bulk_actions(
    cx: &mut Context<WorkspaceView>,
    kind: GroupKind,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let (id, icon, label) = match kind {
        GroupKind::Staged => ("bulk-unstage", IconName::Minus, "unstage"),
        GroupKind::Unstaged | GroupKind::Untracked => ("bulk-stage", IconName::Plus, "stage"),
        GroupKind::Conflicted => ("bulk-resolve", IconName::Check, "resolve"),
    };
    let _ = label;
    div()
        .id(ElementId::Name(format!("git-bulk-{id}").into()))
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded_sm()
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| match kind {
                GroupKind::Staged => this.git_unstage_all(cx),
                GroupKind::Unstaged | GroupKind::Untracked => this.git_stage_all(cx),
                GroupKind::Conflicted => {}
            }),
        )
        .child(render_icon_rgba(icon, px(12.), theme.text_muted))
}

fn status_letter(entry: &GitStatusEntry, kind: GroupKind) -> (String, bool) {
    match kind {
        GroupKind::Untracked => ("U".to_string(), false),
        GroupKind::Conflicted => ("!".to_string(), true),
        GroupKind::Staged => {
            let c = entry.raw.chars().next().unwrap_or('M');
            (c.to_string(), false)
        }
        GroupKind::Unstaged => {
            let c = entry.raw.chars().nth(1).unwrap_or('M');
            (c.to_string(), false)
        }
    }
}

/// Human-readable status for the row tooltip (`"<path> • <status>"`).
fn status_text(entry: &GitStatusEntry, kind: GroupKind) -> String {
    match kind {
        GroupKind::Untracked => code_plus_plus_i18n::t("git.status.untracked"),
        GroupKind::Conflicted => code_plus_plus_i18n::t("git.merge_conflicts"),
        GroupKind::Staged => {
            if entry.staged_label.is_empty() {
                code_plus_plus_i18n::t("git.staged")
            } else {
                entry.staged_label.clone()
            }
        }
        GroupKind::Unstaged => {
            if entry.unstaged_label.is_empty() {
                code_plus_plus_i18n::t("git.status.modified")
            } else {
                entry.unstaged_label.clone()
            }
        }
    }
}

/// Cursor-style language badge color (TS blue / JS amber / PY green…).
fn lang_badge_color(badge: &str, theme: &crate::theme::ThemeTokens) -> gpui::Rgba {
    match badge {
        "TS" => theme.info,
        "JS" => theme.warning,
        "PY" => theme.success,
        "RS" => theme.warning,
        "!" => theme.error,
        _ => theme.text_muted,
    }
}

fn status_color(letter: &str, theme: &crate::theme::ThemeTokens) -> gpui::Rgba {
    match letter {
        "M" => theme.warning,
        "A" | "U" => theme.success,
        "D" | "!" => theme.error,
        "R" | "C" => theme.info,
        _ => theme.text_muted,
    }
}

fn lang_badge(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();
    match ext.as_str() {
        "TS" | "TSX" => "TS".to_string(),
        "JS" | "JSX" | "MJS" | "CJS" => "JS".to_string(),
        "RS" => "RS".to_string(),
        "PY" => "PY".to_string(),
        "JSON" | "JSONC" => "JS".to_string(),
        "MD" => "MD".to_string(),
        "" => "≡".to_string(),
        other => other.chars().take(2).collect(),
    }
}

fn render_file_row(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
    entry: &GitStatusEntry,
    kind: GroupKind,
) -> impl IntoElement {
    let path = entry.path.clone();
    let path_open = path.clone();
    let path_stage_a = path.clone();
    let path_stage_b = path.clone();
    let path_discard = path.clone();
    let path_select = path.clone();
    let path_diff = path.clone();
    let selected = view.git_selected.contains(&path);
    let (letter, _conflict) = status_letter(entry, kind);
    let letter_color = status_color(&letter, theme);
    let badge = lang_badge(&path);
    // Cursor-style hover key: the same path can appear in staged and
    // unstaged at once, so the group disambiguates the overlay target.
    let group_tag = match kind {
        GroupKind::Staged => "staged",
        GroupKind::Unstaged => "unstaged",
        GroupKind::Untracked => "untracked",
        GroupKind::Conflicted => "conflicted",
    };
    let hover_key = format!("{group_tag}:{path}");
    let hovered = view.git_hovered_row.as_deref() == Some(hover_key.as_str());
    let hover_key_outer = hover_key.clone();
    // Cursor parity: the row shows only the file name; the floating tooltip
    // (wired below) carries the full repo-relative path.
    let file_name: String = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.clone());
    let tooltip_text: SharedString =
        format!("{} • {}", path, status_text(entry, kind)).into();

    // Compact Cursor-style row: badge + truncated name + status letter.
    // Action buttons live in a floating overlay (zero layout cost) that only
    // renders while the row is hovered; the full path shows as a tooltip.
    let mut row = div()
        .id(ElementId::Name(format!("git-file-{hover_key}").into()))
        .relative()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .h(px(26.))
        .rounded_md()
        .overflow_hidden()
        .cursor_pointer()
        .bg(if selected {
            theme.bg_selected
        } else {
            gpui::hsla(0., 0., 0., 0.).into()
        })
        .hover(|s| s.bg(theme.bg_hover))
        .tooltip(move |_, cx| {
            cx.new(|_| GitFileTooltip {
                text: tooltip_text.clone(),
            })
            .into()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.modifiers.control || event.modifiers.platform {
                    this.git_toggle_select(&path_select, cx);
                } else {
                    this.open_git_changed_file(&path_open, cx);
                }
            }),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(22.))
                .text_xs()
                .font_family(crate::theme::font_mono(theme))
                .text_color(lang_badge_color(&badge, theme))
                .child(badge),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .font_family(crate::theme::font_mono(theme))
                .text_color(theme.text_secondary)
                .child(file_name),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .font_family(crate::theme::font_mono(theme))
                .text_color(letter_color)
                .child(letter),
        );
    if hovered {
        let mut bar = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .gap_0p5()
            .pl_2()
            .pr_1()
            .bg(theme.bg_elevated)
            .border_l_1()
            .border_color(theme.border_subtle)
            .rounded_md();
        if kind == GroupKind::Conflicted {
            let ours = entry.path.clone();
            let theirs = entry.path.clone();
            bar = bar
                .child(small_action(
                    cx,
                    ElementId::Name(format!("git-ours-{hover_key}").into()),
                    &code_plus_plus_i18n::t("git.resolve_ours"),
                    theme,
                    move |this, _, _, cx| {
                        this.git_resolve(true, vec![ours.clone()], cx);
                    },
                ))
                .child(small_action(
                    cx,
                    ElementId::Name(format!("git-theirs-{hover_key}").into()),
                    &code_plus_plus_i18n::t("git.resolve_theirs"),
                    theme,
                    move |this, _, _, cx| {
                        this.git_resolve(false, vec![theirs.clone()], cx);
                    },
                ));
        } else {
            // Single contextual stage toggle (Cursor shows one affordance).
            if kind == GroupKind::Staged {
                bar = bar.child(tip_btn(
                    cx,
                    ElementId::Name(format!("git-row-unstage-{hover_key}").into()),
                    IconName::Minus,
                    code_plus_plus_i18n::t("git.unstage"),
                    theme,
                    move |this, _, _, cx| {
                        this.git_unstage_paths(vec![path_stage_b.clone()], cx);
                    },
                ));
            } else {
                bar = bar.child(tip_btn(
                    cx,
                    ElementId::Name(format!("git-row-stage-{hover_key}").into()),
                    IconName::Plus,
                    code_plus_plus_i18n::t("git.stage"),
                    theme,
                    move |this, _, _, cx| {
                        this.git_stage_paths(vec![path_stage_a.clone()], cx);
                    },
                ));
            }
            bar = bar.child(tip_btn(
                cx,
                ElementId::Name(format!("git-row-discard-{hover_key}").into()),
                IconName::Trash2,
                code_plus_plus_i18n::t("git.discard"),
                theme,
                move |this, _, _, cx| {
                    this.git_request_discard_paths(vec![path_discard.clone()], cx);
                },
            ));
        }
        bar = bar.child(tip_btn(
            cx,
            ElementId::Name(format!("git-row-diff-{hover_key}").into()),
            IconName::Eye,
            code_plus_plus_i18n::t("git.open_diff"),
            theme,
            move |this, _, _, cx| {
                this.open_git_changed_file(&path_diff, cx);
            },
        ));
        if kind != GroupKind::Untracked {
            let hunk_path = entry.path.clone();
            let hunk_expanded_now = view.git_hunks_expanded.contains(&hunk_path);
            let hunk_tip: SharedString = code_plus_plus_i18n::t("git.hunks").into();
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("git-row-hunks-{hover_key}").into()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if hunk_expanded_now {
                        theme.bg_selected
                    } else {
                        gpui::hsla(0., 0., 0., 0.).into()
                    })
                    .hover(|s| s.bg(theme.bg_hover))
                    .tooltip(move |_, cx| {
                        cx.new(|_| GitFileTooltip {
                            text: hunk_tip.clone(),
                        })
                        .into()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.git_toggle_hunks(&hunk_path, cx);
                        }),
                    )
                    .child(render_icon_rgba(IconName::ListTree, px(12.), theme.text_muted)),
            );
        }
        bar = bar.child({
            let copy_path = entry.path.clone();
            tip_btn(
                cx,
                ElementId::Name(format!("git-row-copy-{hover_key}").into()),
                IconName::Copy,
                code_plus_plus_i18n::t("git.copy_path"),
                theme,
                move |_, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
                },
            )
        });
        bar = bar.child({
            let reveal_path = entry.path.clone();
            tip_btn(
                cx,
                ElementId::Name(format!("git-row-reveal-{hover_key}").into()),
                IconName::ExternalLink,
                code_plus_plus_i18n::t("git.reveal"),
                theme,
                move |this, _, _, _cx| {
                    this.git_reveal_in_explorer(&reveal_path);
                },
            )
        });
        bar = bar.child({
            let ignore_path = entry.path.clone();
            tip_btn(
                cx,
                ElementId::Name(format!("git-row-ignore-{hover_key}").into()),
                IconName::FilePlus,
                code_plus_plus_i18n::t("git.ignore"),
                theme,
                move |this, _, _, cx| {
                    this.git_ignore_paths(vec![ignore_path.clone()], cx);
                },
            )
        });
        row = row.child(bar);
    }
    // Hunks float in the same right-side window as the action buttons:
    // never a second line under the row. The panel is a child of the
    // hover-tracked outer container, so reaching for Stage keeps it alive.
    let hunk_path = entry.path.clone();
    let expanded = view.git_hunks_expanded.contains(&hunk_path);
    let loading = view.git_hunks_loading.contains(&hunk_path);
    let hunks = view.git_hunks.get(&hunk_path).cloned().unwrap_or_default();
    let mut outer = div()
        .id(ElementId::Name(format!("git-filerow-{hover_key}").into()))
        .relative()
        .flex()
        .flex_col()
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.git_set_hovered_row(
                if *hovered {
                    Some(hover_key_outer.clone())
                } else {
                    None
                },
                cx,
            );
        }))
        .child(row);
    if kind != GroupKind::Untracked && expanded && hovered {
        let hunk_path_close = entry.path.clone();
        let hunk_total = hunks.len();
        let mut panel = div()
            .id(ElementId::Name(format!(
                "git-hunks-panel-{hover_key}"
            ).into()))
            .absolute()
            .top(px(26.))
            .right(px(4.))
            .w(px(320.))
            .max_h(px(220.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_default)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(if loading {
                                code_plus_plus_i18n::t("git.loading")
                            } else {
                                code_plus_plus_i18n::tr!(
                                    "git.hunk_count",
                                    "count" => hunk_total as u64
                                )
                            }),
                    )
                    .child(tip_btn(
                        cx,
                        ElementId::Name(format!("git-hunks-close-{hover_key}").into()),
                        IconName::X,
                        code_plus_plus_i18n::t("common.close"),
                        theme,
                        move |this, _, _, cx| {
                            this.git_toggle_hunks(&hunk_path_close, cx);
                        },
                    )),
            );
        for (idx, hunk) in hunks.iter().enumerate().take(6) {
            let hp = entry.path.clone();
            let preview: String = hunk.lines().take(4).collect::<Vec<_>>().join("\n");
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.bg_surface)
                    .child(
                        div()
                            .text_xs()
                            .font_family(crate::theme::font_mono(theme))
                            .text_color(theme.text_muted)
                            .child(preview),
                    )
                    .child(div().flex().items_center().gap_1().child(small_action(
                        cx,
                        ElementId::Name(format!("git-hunk-stage-{hover_key}-{idx}").into()),
                        &code_plus_plus_i18n::t("git.stage"),
                        theme,
                        move |this, _, _, cx| {
                            this.git_stage_hunk(&hp, idx, cx);
                        },
                    ))),
            );
        }
        if hunk_total > 6 {
            panel = panel.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("+{} more", hunk_total - 6)),
            );
        }
        outer = outer.child(panel);
    }
    outer.into_any_element()
}

/// Icon button with a hover name tooltip (Cursor parity: icon-only actions
/// always announce themselves, e.g. Stage / Discard / Open diff).
fn tip_btn(
    cx: &mut Context<WorkspaceView>,
    id: impl Into<ElementId>,
    icon: IconName,
    tip: String,
    theme: &crate::theme::ThemeTokens,
    on_click: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
    + 'static,
) -> Stateful<Div> {
    let tip_text: SharedString = tip.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded_sm()
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .tooltip(move |_, cx| {
            cx.new(|_| GitFileTooltip {
                text: tip_text.clone(),
            })
            .into()
        })
        .on_mouse_down(MouseButton::Left, cx.listener(on_click))
        .child(render_icon_rgba(icon, px(12.), theme.text_muted))
}

fn section_header(
    cx: &mut Context<WorkspaceView>,
    id: &'static str,
    title: &str,
    count: &str,
    expanded: bool,
    on_toggle: impl Fn(&mut WorkspaceView, &MouseDownEvent, &mut Window, &mut Context<WorkspaceView>)
        + 'static,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_1()
        .py_0p5()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(on_toggle))
        .child(render_icon_rgba(
            if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            },
            px(13.),
            theme.text_primary,
        ))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(title.to_string()),
        )
        .child(div().flex_1())
        .child(
            div()
                .px_1p5()
                .rounded_sm()
                .bg(theme.bg_elevated)
                .text_xs()
                .font_family(crate::theme::font_mono(theme))
                .text_color(theme.text_secondary)
                .child(count.to_string()),
        )
        .id(id)
}

// ---------------------------------------------------------------------------
// AGENT REVIEW
// ---------------------------------------------------------------------------

fn render_agent_review(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let scope_label =
        code_plus_plus_i18n::tr!("git.review_vs", "scope" => view.git_review_scope.clone());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_shrink_0()
        .child(section_header(
            cx,
            "git-sec-review",
            &code_plus_plus_i18n::t("git.agent_review"),
            &format!("{}", view.git_review_issues.len()),
            view.git_review_expanded,
            |this, _, _, cx| {
                this.git_review_expanded = !this.git_review_expanded;
                cx.notify();
            },
            theme,
        ))
        .when(view.git_review_expanded, |el| {
            let col = el
                .when(view.git_review_scope_open, |el| {
                    el.child(
                        div().flex().items_center().gap_1().px_2().child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .h(px(30.))
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .child(render_icon_rgba(
                                    IconName::GitBranch,
                                    px(12.),
                                    theme.text_muted,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .h_full()
                                        .child(view.git_review_scope_input.clone()),
                                ),
                        ),
                    )
                })
                .child(
                    div().flex().items_center().gap_1().px_2().child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .overflow_hidden()
                            .bg(Rgba::from(theme.accent))
                            .when(!view.git_review_busy, |el| {
                                el.hover(|s| s.bg(Rgba::from(theme.accent_hover)))
                            })
                            .when(view.git_review_busy, |el| el.opacity(0.75))
                            .child(
                                div()
                                    .id("git-find-issues")
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_on_accent)
                                    .when(!view.git_review_busy, |el| {
                                        el.cursor_pointer().on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.git_find_issues(cx);
                                            }),
                                        )
                                    })
                                    .child(render_icon_rgba(
                                        if view.git_review_busy {
                                            IconName::Loader
                                        } else {
                                            IconName::Search
                                        },
                                        px(14.),
                                        theme.text_on_accent,
                                    ))
                                    .child(if view.git_review_busy {
                                        code_plus_plus_i18n::t("git.generating")
                                    } else {
                                        code_plus_plus_i18n::t("git.find_issues")
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(20.))
                                    .bg(theme.text_on_accent)
                                    .opacity(0.35),
                            )
                            .child(
                                div()
                                    .id("git-review-scope-toggle")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_2()
                                    .py_1()
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.git_review_scope_open =
                                                !this.git_review_scope_open;
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_icon_rgba(
                                        IconName::ChevronDown,
                                        px(14.),
                                        theme.text_on_accent,
                                    )),
                            ),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(scope_label)
                        .child(render_icon_rgba(
                            IconName::Info,
                            px(12.),
                            theme.text_muted,
                        )),
                );
            // Findings live in their own scroll container (Cursor parity).
            let mut issues_box = div()
                .id("git-review-scroll")
                .flex()
                .flex_col()
                .gap_1()
                .max_h(px(190.))
                .overflow_y_scroll();
            if view.git_review_issues.is_empty() {
                issues_box = issues_box.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(code_plus_plus_i18n::t("git.review_empty")),
                );
            }
            for issue in view.git_review_issues.iter().take(30) {
                let sev_color = match issue.severity.as_str() {
                    "error" => theme.error,
                    "warning" => theme.warning,
                    _ => theme.info,
                };
                let loc = match issue.line {
                    Some(l) => format!("{}:{l}", issue.path),
                    None => issue.path.clone(),
                };
                issues_box = issues_box.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .mx_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(sev_color)
                                        .child(issue.severity.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .text_xs()
                                        .font_family(crate::theme::font_mono(theme))
                                        .text_color(theme.text_secondary)
                                        .child(loc),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_primary)
                                .child(issue.message.clone()),
                        ),
                );
            }
            col.child(issues_box)
        })
}

// ---------------------------------------------------------------------------
// GRAPH
// ---------------------------------------------------------------------------

fn render_graph_section(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_h(px(0.))
        .flex_shrink_1()
        .overflow_hidden()
        .child(section_header(
            cx,
            "git-sec-graph",
            &code_plus_plus_i18n::t("git.graph"),
            &format!("{}", view.git_commits.len()),
            view.git_graph_expanded,
            |this, _, _, cx| {
                this.git_graph_expanded = !this.git_graph_expanded;
                cx.notify();
            },
            theme,
        ))
        .when(view.git_graph_expanded, |el| {
            let filter_active = !view.git_filter_input.read(cx).text().trim().is_empty();
            let filter_box = view.git_filter_input.clone();
            let col = el
                .child(
                    // Cursor-style slim toolbar: branch picker affordance,
                    // Auto filter pill, then fetch / new-branch / refresh.
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .child(render_icon_rgba(
                            IconName::GitBranch,
                            px(14.),
                            theme.text_secondary,
                        ))
                        .child(
                            div()
                                .id("git-graph-auto")
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(if filter_active {
                                    theme.bg_selected
                                } else {
                                    Rgba::from(theme.accent)
                                })
                                .text_color(if filter_active {
                                    theme.text_secondary
                                } else {
                                    theme.text_on_accent
                                })
                                .hover(|s| s.bg(theme.bg_hover))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_, _, _, cx| {
                                        filter_box.update(cx, |input, cx| input.clear(cx));
                                        cx.notify();
                                    }),
                                )
                                .child(code_plus_plus_i18n::t("git.auto")),
                        )
                        .child(div().flex_1())
                        .child(toolbar_btn(
                            cx,
                            "git-graph-fetch",
                            IconName::Download,
                            |this, _, _, cx| this.git_fetch(cx),
                        ))
                        .child(toolbar_btn(
                            cx,
                            "git-graph-branch-box",
                            if view.git_branch_box_open {
                                IconName::ChevronUp
                            } else {
                                IconName::Plus
                            },
                            |this, _, _, cx| {
                                this.git_branch_box_open = !this.git_branch_box_open;
                                cx.notify();
                            },
                        ))
                        .child(toolbar_btn(
                            cx,
                            "git-graph-refresh",
                            IconName::RefreshCw,
                            |this, _, _, cx| this.refresh_git_status_async(cx),
                        )),
                )
                .when(view.git_branch_box_open, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .h(px(30.))
                                    .px_2()
                                    .rounded_md()
                                    .bg(theme.bg_elevated)
                                    .border_1()
                                    .border_color(theme.border_subtle)
                                    .child(
                                        div()
                                            .flex_1()
                                            .h_full()
                                            .child(view.git_branch_input.clone()),
                                    ),
                            )
                            .child(small_action(
                                cx,
                                "git-branch-create",
                                &code_plus_plus_i18n::t("git.create_branch"),
                                theme,
                                |this, _, _, cx| {
                                    this.git_create_branch_from_input(cx);
                                },
                            ))
                            .child(small_action(
                                cx,
                                "git-branch-rename",
                                &code_plus_plus_i18n::t("git.rename_branch"),
                                theme,
                                |this, _, _, cx| {
                                    let new_name = this
                                        .git_branch_input
                                        .read(cx)
                                        .text()
                                        .trim()
                                        .to_string();
                                    if let Some(cur) = this.git_current_branch.clone() {
                                        this.git_rename_branch(&cur, &new_name, cx);
                                    }
                                },
                            )),
                    )
                });
            // Branches + commits share one scroll region under the pinned
            // branch box (Cursor parity: Graph scrolls internally).
            let mut scroller = div()
                .id("git-graph-scroll")
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .gap_0p5()
                .overflow_y_scroll()
                .child(render_branch_list(cx, view, theme));
            // The file search box doubles as a commit filter (subject /
            // author / branch), matching Cursor's GRAPH `Auto` filter feel.
            let query = view.git_filter_input.read(cx).text().trim().to_lowercase();
            let commits: Vec<GitCommitNode> = view
                .git_commits
                .iter()
                .filter(|c| {
                    if query.is_empty() {
                        return true;
                    }
                    c.subject.to_lowercase().contains(&query)
                        || c.author.to_lowercase().contains(&query)
                        || c.branches.iter().any(|b| b.to_lowercase().contains(&query))
                        || c.sha.to_lowercase().starts_with(&query)
                })
                .take(120)
                .cloned()
                .collect();
            let lanes: Vec<usize> = commits.iter().map(|c| c.lane).collect();
            for (i, c) in commits.into_iter().enumerate() {
                let lane = lanes[i];
                let up = i > 0 && lanes[i - 1] == lane;
                let down = i + 1 < lanes.len() && lanes[i + 1] == lane;
                scroller = scroller.child(render_commit_row(cx, view, c, theme, up, down));
            }
            if view.git_commits.is_empty() {
                scroller = scroller.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(code_plus_plus_i18n::t("git.history")),
                );
            }
            col.child(scroller)
        })
}

fn render_branch_list(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap_0p5().px_2();
    for b in view.git_branches.iter().take(20) {
        let name = b.name.clone();
        let name_checkout = name.clone();
        let name_delete = name.clone();
        let is_head = b.is_head;
        let upstream = b
            .upstream
            .clone()
            .map(|u| format!(" → {u}"))
            .unwrap_or_default();
        let ahead_behind = if b.ahead > 0 || b.behind > 0 {
            format!(" ↑{} ↓{}", b.ahead, b.behind)
        } else {
            String::new()
        };
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .bg(if is_head {
                    theme.bg_selected
                } else {
                    gpui::hsla(0., 0., 0., 0.).into()
                })
                .hover(|s| s.bg(theme.bg_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.git_checkout_branch(&name_checkout, cx);
                    }),
                )
                .child(render_icon_rgba(
                    IconName::GitBranch,
                    px(12.),
                    if is_head {
                        Rgba::from(theme.accent)
                    } else {
                        theme.text_muted
                    },
                ))
                .child(
                    div()
                        .text_xs()
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(if is_head {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        })
                        .child(format!("{name}{upstream}{ahead_behind}")),
                )
                .child(div().flex_1())
                .when(!b.is_remote && !is_head, |el| {
                    el.child(
                        div()
                            .id(ElementId::Name(format!("git-del-{name}").into()))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.git_request_delete_branch(&name_delete, false, cx);
                                }),
                            )
                            .child(render_icon_rgba(IconName::X, px(11.), theme.text_muted)),
                    )
                }),
        );
    }
    col
}

fn lane_color(lane: usize, theme: &crate::theme::ThemeTokens) -> gpui::Rgba {
    let palette = [
        Rgba::from(theme.accent),
        theme.success,
        theme.warning,
        theme.info,
        theme.error,
    ];
    palette[lane % palette.len()]
}

fn format_relative_time(ts: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let delta = (now - ts).max(0);
    if delta < 60 {
        format!("{delta}s")
    } else if delta < 3600 {
        format!("{}m", delta / 60)
    } else if delta < 86400 {
        format!("{}h", delta / 3600)
    } else {
        format!("{}d", delta / 86400)
    }
}

fn render_commit_row(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    commit: GitCommitNode,
    theme: &crate::theme::ThemeTokens,
    rail_up: bool,
    rail_down: bool,
) -> impl IntoElement {
    let dot = lane_color(commit.lane, theme);
    let indent = px(commit.lane.min(6) as f32 * 10.0);
    let subject: String = commit.subject.chars().take(60).collect();
    let sha_copy = commit.sha.clone();
    let sha_pick = commit.sha.clone();
    let sha_revert = commit.sha.clone();
    let hover_key = format!("commit:{}", commit.sha);
    let hovered = view.git_hovered_row.as_deref() == Some(hover_key.as_str());
    let hover_key_in = hover_key.clone();
    let tooltip_text: SharedString =
        format!("{} • {}", commit.subject, commit.sha).into();
    // Rail stubs above/below the dot connect consecutive same-lane commits
    // into Cursor's continuous vertical rail (flex_1 fills any row height).
    let stub = |on: bool| {
        div().flex_1().w(px(2.)).bg(if on {
            dot
        } else {
            gpui::hsla(0., 0., 0., 0.).into()
        })
    };
    let mut row = div()
        .id(ElementId::Name(format!("git-commit-{hover_key}").into()))
        .relative()
        .flex()
        .items_stretch()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .hover(|s| s.bg(theme.bg_hover))
        .tooltip(move |_, cx| {
            cx.new(|_| GitFileTooltip {
                text: tooltip_text.clone(),
            })
            .into()
        })
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.git_set_hovered_row(
                if *hovered {
                    Some(hover_key_in.clone())
                } else {
                    None
                },
                cx,
            );
        }))
        .child(div().w(indent).flex_shrink_0())
        .child(
            div()
                .flex_shrink_0()
                .w(px(14.))
                .flex()
                .flex_col()
                .items_center()
                .child(stub(rail_up))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(10.))
                        .rounded_full()
                        .bg(dot)
                        .when(commit.is_head, |el| {
                            el.border_2().border_color(theme.text_on_accent)
                        }),
                )
                .child(stub(rail_down)),
        )
        .child({
            let has_badges = !commit.branches.is_empty() || !commit.remotes.is_empty();
            // Cursor parity: plain rows are a single line (subject + author);
            // the badge line only exists when the commit names a branch.
            let subject_row = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(theme.text_primary)
                        .child(subject),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{} {}",
                            commit.author,
                            format_relative_time(commit.timestamp)
                        )),
                );
            let mut text_col = div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .justify_center()
                .child(subject_row);
            if has_badges {
                text_col = text_col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .children(commit.branches.iter().take(2).map(|b| {
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .bg(Rgba::from(theme.accent))
                                .text_color(theme.text_on_accent)
                                .text_xs()
                                .font_family(crate::theme::font_mono(theme))
                                .child(b.clone())
                        }))
                        .children(commit.remotes.iter().take(1).map(|r| {
                            let short: String = r.chars().take(24).collect();
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .bg(theme.bg_selected)
                                .text_color(theme.text_secondary)
                                .text_xs()
                                .font_family(crate::theme::font_mono(theme))
                                .child(short)
                        })),
                );
            }
            text_col
        });
    // Hover-only actions (Cursor parity: graph rows are plain text).
    if hovered {
        let bar = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .gap_0p5()
            .pl_2()
            .pr_1()
            .bg(theme.bg_elevated)
            .border_l_1()
            .border_color(theme.border_subtle)
            .rounded_md()
            .child(tip_btn(
                cx,
                ElementId::Name(format!("git-copy-{hover_key}").into()),
                IconName::Copy,
                code_plus_plus_i18n::t("git.copy_sha"),
                theme,
                move |_, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(sha_copy.clone()));
                },
            ))
            .child(small_action(
                cx,
                ElementId::Name(format!("git-pick-{hover_key}").into()),
                &code_plus_plus_i18n::t("git.cherry_pick"),
                theme,
                move |this, _, _, cx| {
                    this.git_cherry_pick(&sha_pick, cx);
                },
            ))
            .child(small_action(
                cx,
                ElementId::Name(format!("git-revert-{hover_key}").into()),
                &code_plus_plus_i18n::t("git.revert_commit"),
                theme,
                move |this, _, _, cx| {
                    this.git_revert_commit(&sha_revert, cx);
                },
            ));
        row = row.child(bar);
    }
    row
}

// ---------------------------------------------------------------------------
// STASH + REMOTES
// ---------------------------------------------------------------------------

fn render_stash_section(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_shrink_0()
        .child(section_header(
            cx,
            "git-sec-stash",
            &code_plus_plus_i18n::t("git.stash"),
            &format!("{}", view.git_stashes.len()),
            view.git_stash_expanded,
            |this, _, _, cx| {
                this.git_stash_expanded = !this.git_stash_expanded;
                cx.notify();
            },
            theme,
        ))
        .when(view.git_stash_expanded, |el| {
            let include = view.git_include_untracked_stash;
            let col = el
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .h(px(30.))
                                .px_2()
                                .rounded_md()
                                .bg(theme.bg_elevated)
                                .border_1()
                                .border_color(theme.border_subtle)
                                .child(
                                    div()
                                        .flex_1()
                                        .h_full()
                                        .child(view.git_stash_input.clone()),
                                ),
                        )
                        .child(small_action(
                            cx,
                            "git-stash-push",
                            &code_plus_plus_i18n::t("git.stash_push"),
                            theme,
                            |this, _, _, cx| {
                                this.git_stash_push_from_input(cx);
                            },
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.git_include_untracked_stash =
                                    !this.git_include_untracked_stash;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .size(px(12.))
                                .rounded_sm()
                                .border_1()
                                .border_color(theme.border_default)
                                .bg(if include {
                                    Rgba::from(theme.accent)
                                } else {
                                    theme.bg_elevated
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(code_plus_plus_i18n::t("git.include_untracked")),
                        ),
                );
            let mut stash_list = div()
                .id("git-stash-scroll")
                .flex()
                .flex_col()
                .gap_0p5()
                .max_h(px(170.))
                .overflow_y_scroll();
            if view.git_stashes.is_empty() {
                stash_list = stash_list.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(code_plus_plus_i18n::t("git.stash_empty")),
                );
            }
            for s in view.git_stashes.iter().take(20) {
                let pop_ref = s.ref_name.clone();
                let apply_ref = s.ref_name.clone();
                let drop_ref = s.ref_name.clone();
                stash_list = stash_list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .mx_2()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_xs()
                                .font_family(crate::theme::font_mono(theme))
                                .text_color(theme.text_secondary)
                                .child(format!("{} {}", s.ref_name, s.subject)),
                        )
                        .child(tip_btn(
                            cx,
                            ElementId::Name(format!("git-stash-pop-{}", s.ref_name).into()),
                            IconName::Upload,
                            code_plus_plus_i18n::t("git.stash_pop"),
                            theme,
                            move |this, _, _, cx| {
                                this.git_stash_pop(&pop_ref, cx);
                            },
                        ))
                        .child(tip_btn(
                            cx,
                            ElementId::Name(format!("git-stash-apply-{}", s.ref_name).into()),
                            IconName::Check,
                            code_plus_plus_i18n::t("git.stash_apply"),
                            theme,
                            move |this, _, _, cx| {
                                this.git_stash_apply(&apply_ref, cx);
                            },
                        ))
                        .child(tip_btn(
                            cx,
                            ElementId::Name(format!("git-stash-drop-{}", s.ref_name).into()),
                            IconName::Trash2,
                            code_plus_plus_i18n::t("git.stash_drop"),
                            theme,
                            move |this, _, _, cx| {
                                this.git_request_drop_stash(&drop_ref, cx);
                            },
                        )),
                );
            }
            col.child(stash_list)
        })
}

fn render_remotes_section(
    cx: &mut Context<WorkspaceView>,
    view: &WorkspaceView,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section_header(
            cx,
            "git-sec-remotes",
            &code_plus_plus_i18n::t("git.remotes"),
            &format!("{}", view.git_remotes.len()),
            view.git_remotes_expanded,
            |this, _, _, cx| {
                this.git_remotes_expanded = !this.git_remotes_expanded;
                cx.notify();
            },
            theme,
        ))
        .when(view.git_remotes_expanded, |el| {
            let mut col = el;
            for r in &view.git_remotes {
                col = col.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(theme.text_secondary)
                        .child(format!("{} {}", r.name, r.fetch_url)),
                );
            }
            for t in view.git_tags.iter().take(10) {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .child(render_icon_rgba(
                            IconName::CircleDot,
                            px(11.),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .font_family(crate::theme::font_mono(theme))
                                .text_color(theme.text_muted)
                                .child(t.clone()),
                        ),
                );
            }
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .child(small_action(
                        cx,
                        "git-fetch2",
                        &code_plus_plus_i18n::t("git.fetch"),
                        theme,
                        |this, _, _, cx| {
                            this.git_fetch(cx);
                        },
                    ))
                    .child(small_action(
                        cx,
                        "git-pull2",
                        &code_plus_plus_i18n::t("git.pull"),
                        theme,
                        |this, _, _, cx| {
                            this.git_pull(cx);
                        },
                    ))
                    .child(small_action(
                        cx,
                        "git-push2",
                        &code_plus_plus_i18n::t("git.push"),
                        theme,
                        |this, _, _, cx| {
                            this.git_push(cx);
                        },
                    ))
                    .child(small_action(
                        cx,
                        "git-push-force",
                        "force",
                        theme,
                        |this, _, _, cx| {
                            this.git_request_force_push(cx);
                        },
                    ))
                    .child(small_action(
                        cx,
                        "git-publish",
                        &code_plus_plus_i18n::t("git.publish_branch"),
                        theme,
                        |this, _, _, cx| {
                            this.git_publish_current(cx);
                        },
                    ))
                    .child(small_action(
                        cx,
                        "git-sync",
                        &code_plus_plus_i18n::t("git.sync"),
                        theme,
                        |this, _, _, cx| {
                            this.git_fetch(cx);
                        },
                    )),
            );
            col
        })
}
