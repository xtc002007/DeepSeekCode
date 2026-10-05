use crate::composer::bind_prompt_keys;
use crate::theme::DeepSeekCodeTheme;
use crate::workspace::WorkspaceView;
use gpui::*;
use tokio::runtime::Runtime;

pub struct DeepSeekCodeApp;

impl DeepSeekCodeApp {
    pub fn run() {
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(
                    target: deepseekcode_telemetry::DESKTOP,
                    error = %e,
                    "desktop tokio runtime creation failed"
                );
                eprintln!(
                    "{}: {e}",
                    deepseekcode_i18n::tr!(
                        "error.startup_runtime",
                        "app" => deepseekcode_i18n::t("app.name")
                    )
                );
                std::process::exit(1);
            }
        };
        let _guard = runtime.enter();
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "desktop app starting"
        );

        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx: &mut App| {
                DeepSeekCodeTheme::init(cx);
                bind_prompt_keys(cx);
                cx.activate(true);

                let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Maximized(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(deepseekcode_i18n::t("app.name").into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                match cx.open_window(options, |_, cx| cx.new(|cx| WorkspaceView::new(cx))) {
                    Ok(_) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            "desktop window opened"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::DESKTOP,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            error = %e,
                            "desktop window open failed"
                        );
                        eprintln!(
                            "{}: {e}",
                            deepseekcode_i18n::tr!(
                                "error.startup_window",
                                "app" => deepseekcode_i18n::t("app.name")
                            )
                        );
                        std::process::exit(1);
                    }
                }
            });
    }
}
