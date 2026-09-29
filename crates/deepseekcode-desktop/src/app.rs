use crate::composer::bind_prompt_keys;
use crate::theme::DeepSeekCodeTheme;
use crate::workspace::WorkspaceView;
use gpui::*;
use tokio::runtime::Runtime;

pub struct DeepSeekCodeApp;

impl DeepSeekCodeApp {
    pub fn run() {
        let runtime = Runtime::new().expect("tokio runtime");
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
                        title: Some("DeepSeekCode".into()),
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
                        panic!("open window: {e}");
                    }
                }
            });
    }
}
