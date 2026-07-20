use gpui::*;
use crate::composer::bind_prompt_keys;
use crate::theme::DeepSeekCodeTheme;
use crate::workspace::WorkspaceView;
use tokio::runtime::Runtime;

pub struct DeepSeekCodeApp;

impl DeepSeekCodeApp {
    pub fn run() {
        let runtime = Runtime::new().expect("tokio runtime");
        let _guard = runtime.enter();

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

            cx.open_window(options, |_, cx| cx.new(|cx| WorkspaceView::new(cx)))
                .expect("open window");
        });
    }
}
