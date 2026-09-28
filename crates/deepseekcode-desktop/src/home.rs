//! Home screen: recent projects and open-folder entry.

use gpui::*;
use std::path::PathBuf;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub last_opened: String,
}

pub struct HomeView {
    recent: Vec<RecentProject>,
    loading: bool,
    focus: FocusHandle,
}

impl HomeView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            recent: load_recent_projects(),
            loading: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn refresh_recent(&mut self) {
        self.recent = load_recent_projects();
    }

    pub fn set_server_projects(&mut self, projects: Vec<RecentProject>) {
        let mut merged: Vec<RecentProject> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for mut project in projects {
            project.path = normalize_recent_path_str(&project.path);
            let key = recent_path_key(&project.path);
            if seen.insert(key) {
                merged.push(project);
            }
        }
        for mut local in load_recent_from_disk() {
            local.path = normalize_recent_path_str(&local.path);
            let key = recent_path_key(&local.path);
            if seen.insert(key) {
                merged.push(local);
            }
        }
        merged.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        if merged.len() > 12 {
            merged.truncate(12);
        }
        self.recent = merged;
        self.loading = false;
    }

    pub fn recent_projects(&self) -> &[RecentProject] {
        &self.recent
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    pub fn remove_recent(&mut self, path: &str) {
        let key = recent_path_key(path);
        self.recent.retain(|p| recent_path_key(&p.path) != key);
        remove_recent_project(path);
    }

    pub fn add_recent(&mut self, path: PathBuf) {
        let path_str = normalize_recent_path_str(&path.display().to_string());
        let entry = RecentProject {
            path: path_str.clone(),
            last_opened: chrono::Utc::now().to_rfc3339(),
        };
        let key = recent_path_key(&path_str);
        self.recent.retain(|p| recent_path_key(&p.path) != key);
        self.recent.insert(0, entry);
        if self.recent.len() > 12 {
            self.recent.truncate(12);
        }
        save_recent_to_disk(&self.recent);
    }
}

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let mut list = div().flex().flex_col().gap_1();
        if self.loading {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(theme.text_disabled)
                    .child("Loading projects…"),
            );
        } else if self.recent.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(theme.text_disabled)
                    .child("No recent projects"),
            );
        } else {
            for project in &self.recent {
                let path = project.path.clone();
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(MouseButton::Left, {
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(HomeEvent::OpenProject(path.clone()));
                            })
                        })
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_primary)
                                .child(project.path.clone()),
                        ),
                );
            }
        }

        div()
            .id("home")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(theme.bg_base)
            .track_focus(&self.focus)
            .child(
                div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child("DeepSeekCode"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("Rust-native AI programming environment"),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(theme.accent_rgba())
                            .text_color(theme.text_on_accent)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.accent_hover_rgba()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| {
                                    cx.emit(HomeEvent::PickFolder);
                                }),
                            )
                            .child("Open Folder…"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_disabled)
                            .child("RECENT PROJECTS"),
                    )
                    .child(list),
            )
    }
}

#[derive(Clone, Debug)]
pub enum HomeEvent {
    PickFolder,
    OpenProject(String),
}

impl EventEmitter<HomeEvent> for HomeView {}

pub fn load_recent_projects() -> Vec<RecentProject> {
    load_recent_from_disk()
}

pub fn record_recent_project(path: PathBuf) {
    let mut recent = load_recent_from_disk();
    let path_str = normalize_recent_path_str(&path.display().to_string());
    let entry = RecentProject {
        path: path_str.clone(),
        last_opened: chrono::Utc::now().to_rfc3339(),
    };
    let key = recent_path_key(&path_str);
    recent.retain(|p| recent_path_key(&p.path) != key);
    recent.insert(0, entry);
    if recent.len() > 12 {
        recent.truncate(12);
    }
    save_recent_to_disk(&recent);
}

pub fn remove_recent_project(path: &str) {
    let key = recent_path_key(path);
    let mut recent = load_recent_from_disk();
    recent.retain(|p| recent_path_key(&p.path) != key);
    save_recent_to_disk(&recent);
}

pub fn projects_from_api(
    projects: Vec<deepseekcode_schema::project::Project>,
) -> Vec<RecentProject> {
    projects
        .into_iter()
        .map(|p| RecentProject {
            path: p.path,
            last_opened: p.last_opened_at.to_rfc3339(),
        })
        .collect()
}

fn prefs_path() -> PathBuf {
    deepseekcode_types::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("deepseekcode")
        .join("recent_projects.json")
}

fn load_recent_from_disk() -> Vec<RecentProject> {
    let path = prefs_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_recent_to_disk(recent: &[RecentProject]) {
    if let Some(parent) = prefs_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(recent) {
        let _ = std::fs::write(prefs_path(), json);
    }
}

fn normalize_recent_path_str(path: &str) -> String {
    let path = path.trim();
    let pb = std::path::Path::new(path);
    let canonical = std::fs::canonicalize(pb).unwrap_or_else(|_| pb.to_path_buf());
    let s = canonical.display().to_string();
    #[cfg(windows)]
    {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

fn recent_path_key(path: &str) -> String {
    let normalized = normalize_recent_path_str(path);
    #[cfg(windows)]
    {
        normalized
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        normalized.trim_end_matches('/').to_string()
    }
}
