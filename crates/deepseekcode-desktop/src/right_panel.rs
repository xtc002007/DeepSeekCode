//! Right panel: Files, Editor, Terminal, Git tabs.

use crate::detail_inspector::DetailInspector;
use crate::editor::EditorView;
use crate::file_tree::{FileTreeState, FileTreeView};
use crate::git_panel::GitPanel;
use crate::layout::RightPanelTab;
use crate::logic::GitStatusEntry;
use crate::terminal::{TerminalPanel, TerminalTab};
use crate::ui::icon::IconName;
use crate::ui::tab_bar::{render_tab_bar_shell, tab_item_shell};
use gpui::prelude::FluentBuilder;
use gpui::*;

pub struct RightPanelView;

impl RightPanelView {
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
        git_staged: &[GitStatusEntry],
        git_unstaged: &[GitStatusEntry],
        git_untracked: &[GitStatusEntry],
        git_diff_summary: Option<&str>,
        git_status_message: Option<&str>,
        git_log: &[String],
        selected_detail: Option<&crate::activity_timeline::DetailPayload>,
    ) -> AnyElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        if !has_project {
            return div()
                .w(width)
                .flex_shrink_0()
                .h_full()
                .bg(theme.bg_surface)
                .border_l_1()
                .border_color(theme.border_subtle)
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
                            terminals,
                            active_terminal,
                            terminal_session,
                            terminal_focus,
                            terminal_scroll,
                            terminal_details,
                            search_query,
                            git_staged,
                            git_unstaged,
                            git_untracked,
                            git_diff_summary,
                            git_status_message,
                            git_log,
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

        row = row.child(tab_item_shell(tab_item, is_active, &theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.set_right_panel_tab(tab_val, cx);
            }),
        ));
    }

    row
}

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
        git_staged: &[GitStatusEntry],
    git_unstaged: &[GitStatusEntry],
    git_untracked: &[GitStatusEntry],
    git_diff_summary: Option<&str>,
    git_status_message: Option<&str>,
    git_log: &[String],
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
            file_editor_input,
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
        RightPanelTab::Git => GitPanel::render(
            cx,
            git_diff_summary,
            git_status_message,
            git_staged,
            git_unstaged,
            git_untracked,
            git_log,
        )
        .into_any_element(),
    }
}
