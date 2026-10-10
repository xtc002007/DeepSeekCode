//! Right panel: Files, Editor, Terminal, Git tabs.

use crate::detail_inspector::DetailInspector;
use crate::editor::EditorView;
use crate::file_tree::{FileTreeState, FileTreeView};
use crate::git_panel::GitPanel;
use crate::layout::RightPanelTab;
use crate::terminal::{TerminalPanel, TerminalTab};
use crate::ui::icon::IconName;
use crate::ui::tab_bar::{render_tab_bar_shell, tab_item_shell};
use gpui::prelude::FluentBuilder;
use gpui::*;

pub struct RightPanelView;

impl RightPanelView {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        width: Pixels,
        tab: RightPanelTab,
        project_dir: &str,
        file_tree: &FileTreeState,
        editor: &crate::editor::EditorState,
        file_editor_input: Entity<crate::file_editor_input::FileEditorInput>,
        editor_tabs: &[String],
        active_editor_tab: usize,
        editor_read_only: bool,
        editor_dirty: bool,
        editor_preview: &str,
        terminals: &[TerminalTab],
        active_terminal: usize,
        terminal_session: Option<deepseekcode_types::SessionId>,
        terminal_focus: &FocusHandle,
        terminal_scroll: &ScrollHandle,
        terminal_details: &std::collections::HashMap<
            String,
            crate::activity_timeline::BashTranscript,
        >,
        search_query: &str,
        has_project: bool,
        workspace: &crate::workspace::WorkspaceView,
        selected_detail: Option<&crate::activity_timeline::DetailPayload>,
    ) -> AnyElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        if !has_project {
            // P1-9: keep the tab bar + an empty state instead of a blank
            // column (previously users had nowhere to open a project from).
            return div()
                .w(width)
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .bg(theme.bg_surface)
                .border_l_1()
                .border_color(theme.border_subtle)
                .child(render_tab_bar(cx, tab, &theme))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.text_primary)
                                .child(deepseekcode_i18n::t("workspace.no_project_title")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(deepseekcode_i18n::t("workspace.no_project")),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(theme.accent)
                                .text_xs()
                                .text_color(theme.text_on_accent)
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.accent))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.pick_project_folder(cx);
                                    }),
                                )
                                .child(deepseekcode_i18n::t("home.open_folder")),
                        ),
                )
                .into_any_element();
        }

        div()
            .id("right-panel")
            .w(width)
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.bg_surface)
            .border_l_1()
            .border_color(theme.border_subtle)
            .child(render_tab_bar(cx, tab, &theme))
            .child({
                let border_subtle = theme.border_subtle;
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(
                        render_tab_content(
                            cx,
                            tab,
                            project_dir,
                            file_tree,
                            editor,
                            file_editor_input,
                            editor_tabs,
                            active_editor_tab,
                            editor_read_only,
                            editor_dirty,
                            editor_preview,
                            terminals,
                            active_terminal,
                            terminal_session,
                            terminal_focus,
                            terminal_scroll,
                            terminal_details,
                            search_query,
                            workspace,
                        ),
                    ))
                    .when(
                        selected_detail.is_some()
                            && tab != RightPanelTab::Terminal
                            && tab != RightPanelTab::Editor,
                        |panel| {
                            panel.child(
                                div()
                                    .flex_shrink_0()
                                    .max_h(px(240.))
                                    .border_t_1()
                                    .border_color(border_subtle)
                                    .overflow_hidden()
                                    .child(DetailInspector::render(cx, selected_detail)),
                            )
                        },
                    )
            })
            .into_any_element()
    }
}

fn render_tab_bar(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    active: RightPanelTab,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let theme = theme.clone();
    let mut row = render_tab_bar_shell(&theme).flex_shrink_0();

    for tab in RightPanelTab::all() {
        let is_active = *tab == active;
        let icon = match tab {
            RightPanelTab::Files => IconName::Files,
            RightPanelTab::Editor => IconName::Code,
            RightPanelTab::Terminal => IconName::Terminal,
            RightPanelTab::Git => IconName::GitBranch,
        };
        let tab_val = *tab;
        let tab_item = crate::ui::tab_bar::TabItem {
            id: tab.label(),
            label: tab.localized_label(),
            icon,
        };

        row = row.child(tab_item_shell(tab_item, is_active, &theme).on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                // P1-1 release-to-confirm + P1-2 stop so a tab switch never
                // bubbles to the panel resize/drag layer.
                cx.stop_propagation();
                this.set_right_panel_tab(tab_val, cx);
            }),
        ));
    }

    row
}

#[allow(clippy::too_many_arguments)]
fn render_tab_content(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    tab: RightPanelTab,
    project_dir: &str,
    file_tree: &FileTreeState,
    editor: &crate::editor::EditorState,
    file_editor_input: Entity<crate::file_editor_input::FileEditorInput>,
    editor_tabs: &[String],
    active_editor_tab: usize,
    editor_read_only: bool,
    editor_dirty: bool,
    editor_preview: &str,
    terminals: &[TerminalTab],
    active_terminal: usize,
    terminal_session: Option<deepseekcode_types::SessionId>,
    terminal_focus: &FocusHandle,
    terminal_scroll: &ScrollHandle,
    terminal_details: &std::collections::HashMap<String, crate::activity_timeline::BashTranscript>,
    search_query: &str,
    workspace: &crate::workspace::WorkspaceView,
) -> impl IntoElement {
    match tab {
        RightPanelTab::Files => {
            FileTreeView::render(cx, project_dir, file_tree, search_query).into_any_element()
        }
        RightPanelTab::Editor => EditorView::render(
            editor,
            editor_tabs,
            active_editor_tab,
            editor_read_only,
            editor_dirty,
            workspace.editor_saving,
            file_editor_input,
            editor_preview,
            cx,
        )
        .into_any_element(),
        RightPanelTab::Terminal => TerminalPanel::render(
            cx,
            terminals,
            active_terminal,
            terminal_session,
            terminal_focus,
            terminal_scroll,
            terminal_details,
        )
        .into_any_element(),
        RightPanelTab::Git => GitPanel::render(cx, workspace).into_any_element(),
    }
}
