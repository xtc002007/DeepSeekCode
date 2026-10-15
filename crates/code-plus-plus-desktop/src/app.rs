use crate::composer::bind_prompt_keys;
use crate::theme::CodePlusPlusTheme;
use crate::workspace::WorkspaceView;
use gpui::*;
use tokio::runtime::Runtime;

/// Keymap for the Git panel's `PromptInput`s.
///
/// Root cause of the dead-backspace reports: `bind_prompt_keys` only binds
/// the stock editing actions under the `"PromptInput"` key context, while
/// every git input renders with its own context (`"GitCommitInput"`, …).
/// Without these, backspace/delete/arrows/home/end/select-all/paste/cut/copy
/// silently do nothing inside commit/filter/branch/stash/scope inputs.
fn bind_git_input_keys(cx: &mut App) {
    use crate::composer::*;
    use crate::git_panel::*;

    macro_rules! editing_keys {
        ($context:expr) => {
            cx.bind_keys([
                KeyBinding::new("backspace", PromptBackspace, Some($context)),
                KeyBinding::new("delete", PromptDelete, Some($context)),
                KeyBinding::new("left", PromptLeft, Some($context)),
                KeyBinding::new("right", PromptRight, Some($context)),
                KeyBinding::new("shift-left", PromptSelectLeft, Some($context)),
                KeyBinding::new("shift-right", PromptSelectRight, Some($context)),
                KeyBinding::new("ctrl-a", PromptSelectAll, Some($context)),
                KeyBinding::new("ctrl-v", PromptPaste, Some($context)),
                KeyBinding::new("paste", PromptPaste, Some($context)),
                KeyBinding::new("shift-insert", PromptPaste, Some($context)),
                KeyBinding::new("ctrl-c", PromptCopy, Some($context)),
                KeyBinding::new("ctrl-x", PromptCut, Some($context)),
                KeyBinding::new("home", PromptHome, Some($context)),
                KeyBinding::new("end", PromptEnd, Some($context)),
            ])
        };
    }
    // SessionTitleInput has the same custom-context problem (enter/escape
    // were bound, editing keys were not), so it joins the same table.
    for context in GIT_INPUT_CONTEXTS
        .iter()
        .copied()
        .chain(["SessionTitleInput"])
    {
        editing_keys!(context);
    }
    // Submit gestures: Cursor parity is Ctrl+Enter on the commit box; plain
    // Enter also submits since the box is single-line (and would otherwise
    // insert a stray space). Branch/stash/scope confirm on Enter.
    cx.bind_keys([
        KeyBinding::new("ctrl-enter", GitCommitSubmit, Some("GitCommitInput")),
        KeyBinding::new("enter", GitCommitSubmit, Some("GitCommitInput")),
        KeyBinding::new("enter", GitBranchSubmit, Some("GitBranchInput")),
        KeyBinding::new("enter", GitStashSubmit, Some("GitStashInput")),
        KeyBinding::new("enter", GitReviewSubmit, Some("GitReviewScopeInput")),
    ]);
    // P0-7: question dialog must always have an Esc exit (previously the
    // modal had no Cancel/X/Esc and could trap focus forever).
    // P2: global overlay Esc — closes image preview / api dialog / popover.
    cx.bind_keys([
        KeyBinding::new("escape", crate::workspace::QuestionCancel, Some("QuestionInput")),
        KeyBinding::new("escape", crate::workspace::QuestionCancel, Some("PromptInput")),
        KeyBinding::new("escape", crate::workspace::CloseOverlay, None),
    ]);
}

pub struct CodePlusPlusApp;

impl CodePlusPlusApp {
    pub fn run() {
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(
                    target: code_plus_plus_telemetry::DESKTOP,
                    error = %e,
                    "desktop tokio runtime creation failed"
                );
                eprintln!(
                    "{}: {e}",
                    code_plus_plus_i18n::tr!(
                        "error.startup_runtime",
                        "app" => code_plus_plus_i18n::t("app.name")
                    )
                );
                std::process::exit(1);
            }
        };
        let _guard = runtime.enter();
        tracing::info!(
            target: code_plus_plus_telemetry::DESKTOP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "desktop app starting"
        );

        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx: &mut App| {
                CodePlusPlusTheme::init(cx);
                bind_prompt_keys(cx);
                bind_git_input_keys(cx);
                cx.activate(true);

                let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Maximized(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(code_plus_plus_i18n::t("app.name").into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                match cx.open_window(options, |_, cx| cx.new(|cx| WorkspaceView::new(cx))) {
                    Ok(_) => {
                        tracing::info!(
                            target: code_plus_plus_telemetry::DESKTOP,
                            run_id = %code_plus_plus_telemetry::current_run_id(),
                            "desktop window opened"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: code_plus_plus_telemetry::DESKTOP,
                            run_id = %code_plus_plus_telemetry::current_run_id(),
                            error = %e,
                            "desktop window open failed"
                        );
                        eprintln!(
                            "{}: {e}",
                            code_plus_plus_i18n::tr!(
                                "error.startup_window",
                                "app" => code_plus_plus_i18n::t("app.name")
                            )
                        );
                        std::process::exit(1);
                    }
                }
            });
    }
}
