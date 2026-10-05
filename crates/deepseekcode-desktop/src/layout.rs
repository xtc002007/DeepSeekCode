//! Three-column layout types for the desktop shell.

use deepseekcode_schema::agent::AgentKind;
use deepseekcode_types::SessionId;
use gpui::{px, Pixels};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RightPanelTab {
    #[default]
    Files,
    Editor,
    Terminal,
    Git,
}

impl RightPanelTab {
    pub fn label(self) -> &'static str {
        // Stable English tokens for logs/tests. UI renders `localized_label`.
        match self {
            Self::Files => "Files",
            Self::Editor => "Editor",
            Self::Terminal => "Terminal",
            Self::Git => "Git",
        }
    }

    /// Localized label for the tab bar.
    pub fn localized_label(self) -> String {
        match self {
            Self::Files => deepseekcode_i18n::t("nav.files"),
            Self::Editor => deepseekcode_i18n::t("nav.editor"),
            Self::Terminal => deepseekcode_i18n::t("nav.terminal"),
            Self::Git => deepseekcode_i18n::t("nav.git"),
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Files, Self::Editor, Self::Terminal, Self::Git]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_panel_tab_labels() {
        assert_eq!(RightPanelTab::Files.label(), "Files");
        assert_eq!(RightPanelTab::Editor.label(), "Editor");
        assert_eq!(RightPanelTab::Terminal.label(), "Terminal");
        assert_eq!(RightPanelTab::Git.label(), "Git");
    }

    #[test]
    fn layout_state_defaults() {
        let layout = LayoutState::default();
        assert_eq!(layout.right_panel_tab, RightPanelTab::Files);
        assert!(layout.show_right_panel);
        assert!(!layout.sidebar_collapsed);
    }

    #[test]
    fn right_panel_tab_all_includes_git() {
        assert_eq!(RightPanelTab::all().len(), 4);
        assert!(RightPanelTab::all().contains(&RightPanelTab::Git));
    }
}

#[derive(Clone, Debug)]
pub struct LayoutState {
    pub sidebar_width: Pixels,
    pub right_panel_width: Pixels,
    pub sidebar_collapsed: bool,
    pub right_panel_tab: RightPanelTab,
    pub show_right_panel: bool,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_width: px(280.),
            right_panel_width: px(320.),
            sidebar_collapsed: false,
            right_panel_tab: RightPanelTab::Files,
            show_right_panel: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionNavItem {
    pub id: SessionId,
    pub title: String,
    pub updated_at: String,
    pub agent: AgentKind,
    pub running: bool,
}

#[derive(Clone, Debug)]
pub struct ProjectNavItem {
    pub path: String,
    pub sessions: Vec<SessionNavItem>,
}
