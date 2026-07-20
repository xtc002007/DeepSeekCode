use deepseekcode_sdk::FsEntry;
use crate::theme::ThemeTokens;
use crate::ui::icon::file_type::{color_for_path, icon_for_path};
use crate::ui::badge::render_status_badge;
use crate::ui::button::icon_button_shell;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileBadge {
    Modified,
    Added,
    Deleted,
    Untracked,
}

impl GitFileBadge {
    pub fn label(self) -> &'static str {
        match self {
            GitFileBadge::Modified => "M",
            GitFileBadge::Added => "A",
            GitFileBadge::Deleted => "D",
            GitFileBadge::Untracked => "U",
        }
    }

    pub fn color(self, theme: &ThemeTokens) -> gpui::Rgba {
        match self {
            GitFileBadge::Modified => theme.warning,
            GitFileBadge::Added => theme.success,
            GitFileBadge::Deleted => theme.error,
            GitFileBadge::Untracked => theme.info,
        }
    }

    pub fn from_git_raw(raw: &str) -> Self {
        if raw.contains('?') {
            return GitFileBadge::Untracked;
        }
        let staged = raw.chars().next().unwrap_or(' ');
        let unstaged = raw.chars().nth(1).unwrap_or(' ');
        if staged == 'D' || unstaged == 'D' {
            GitFileBadge::Deleted
        } else if staged == 'A' || unstaged == 'A' {
            GitFileBadge::Added
        } else {
            GitFileBadge::Modified
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FileTreeState {
    pub entries: HashMap<String, Vec<FsEntry>>,
    pub expanded: Vec<String>,
    pub loading: Vec<String>,
    pub changed_paths: HashSet<String>,
    pub git_badges: HashMap<String, GitFileBadge>,
    pub totals: HashMap<String, usize>,
    pub offsets: HashMap<String, usize>,
}

impl FileTreeState {
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.iter().any(|p| p == path)
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading.iter().any(|p| p == path)
    }

    pub fn git_badge(&self, path: &str) -> Option<GitFileBadge> {
        self.git_badges.get(path).copied()
    }
}

pub struct FileTreeView;

impl FileTreeView {
    pub fn render(
        cx: &Context<crate::workspace::WorkspaceView>,
        project_dir: &str,
        state: &FileTreeState,
        search_query: &str,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let query = search_query.trim().to_lowercase();
        let mut tree = div()
            .id("file-tree")
            .h_full()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme.bg_surface)
            .child(
                div()
                    .w_full()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child("FILES"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                icon_button_shell(
                                    "file-tree-refresh",
                                    IconName::RefreshCw,
                                    &theme,
                                    px(24.),
                                    false,
                                    false,
                                    false,
                                )
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.refresh_file_tree(cx);
                                })),
                            )
                            .child(
                                icon_button_shell(
                                    "file-tree-add",
                                    IconName::FilePlus,
                                    &theme,
                                    px(24.),
                                    false,
                                    false,
                                    false,
                                )
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.add_project_directory(cx);
                                })),
                            ),
                    ),
            );

        let root_entries = state
            .entries
            .get(project_dir)
            .cloned()
            .unwrap_or_default();
        let root_entries: Vec<_> = if query.is_empty() {
            root_entries
        } else {
            root_entries
                .into_iter()
                .filter(|entry| entry.name.to_lowercase().contains(&query))
                .collect()
        };

        if root_entries.is_empty() && !state.is_loading(project_dir) {
            tree = tree.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme.text_disabled)
                    .child("Loading…"),
            );
        } else {
            let total = state.totals.get(project_dir).copied().unwrap_or(root_entries.len());
            let offset = state.offsets.get(project_dir).copied().unwrap_or(0);
            let has_more = offset + root_entries.len() < total;
            tree = tree.child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(render_entries(cx, &theme, state, project_dir, &root_entries, 0)),
            );
            if has_more {
                tree = tree.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_center()
                        .text_color(theme.info)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(MouseButton::Left, {
                            let dir = project_dir.to_string();
                            cx.listener(move |this, _, _, cx| {
                                this.load_more_directory(&dir, cx);
                            })
                        })
                        .child(format!(
                            "Load more ({}/{total})",
                            offset + root_entries.len()
                        )),
                );
            }
        }

        tree
    }
}

fn render_entries(
    cx: &Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    state: &FileTreeState,
    _parent: &str,
    entries: &[FsEntry],
    depth: usize,
) -> impl IntoElement {
    let theme = theme.clone();
    let mut col = div().flex().flex_col();
    let indent = px(12. * depth as f32);

    for entry in entries {
        let path = entry.path.clone();
        let is_dir = entry.is_dir;
        let expanded = is_dir && state.is_expanded(&path);
        let file_icon = if is_dir {
            if expanded {
                IconName::FolderOpen
            } else {
                icon_for_path(&path)
            }
        } else {
            icon_for_path(&path)
        };
        let icon_color = color_for_path(&path, &theme);
        let chevron = if is_dir {
            if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            }
        } else {
            IconName::Circle
        };
        let git_badge = state.git_badge(&path);
        let changed = git_badge.is_some() || state.changed_paths.contains(&path);

        let mut row = div()
            .pl(indent)
            .pr_3()
            .py_1()
            .text_xs()
            .line_height(crate::theme::leading_sm(&theme))
            .flex()
            .items_center()
            .gap_1()
            .hover(|s| s.bg(theme.bg_hover))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, {
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    if is_dir {
                        this.toggle_directory(&path, cx);
                    } else {
                        this.open_file(&path, cx);
                    }
                })
            })
            .child(if is_dir {
                render_icon_rgba(chevron, px(12.), theme.text_muted).into_any_element()
            } else {
                div().w(px(12.)).into_any_element()
            })
            .child(render_icon_rgba(file_icon, px(14.), icon_color))
            .child(
                div()
                    .text_color(theme.text_primary)
                    .child(entry.name.clone()),
            );

        if let Some(badge) = git_badge {
            row = row.child(render_status_badge(
                badge.label(),
                &theme,
                badge.color(&theme),
            ));
        } else if changed {
            row = row.child(render_status_badge("M", &theme, theme.warning));
        }
        if changed {
            let path_for_diff = path.clone();
            row = row.child(
                div()
                    .ml_auto()
                    .px_1()
                    .text_xs()
                    .text_color(theme.info)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text_link))
                    .on_mouse_down(MouseButton::Left, {
                        let path_for_diff = path_for_diff.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.open_file_diff(&path_for_diff, cx);
                            cx.stop_propagation();
                        })
                    })
                    .child("diff"),
            );
        }

        col = col.child(row);

        if is_dir && expanded {
            if state.is_loading(&path) {
                col = col.child(
                    div()
                        .pl(indent + px(12.))
                        .text_xs()
                        .text_color(theme.text_disabled)
                        .child("Loading…"),
                );
            } else if let Some(children) = state.entries.get(&path) {
                col = col.child(render_entries(cx, &theme, state, &path, children, depth + 1));
            }
        }
    }

    col
}
