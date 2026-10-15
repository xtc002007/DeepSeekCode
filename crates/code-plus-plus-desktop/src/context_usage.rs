//! Context usage meter — displays token utilization vs model context window.

use crate::theme::ThemeTokens;
use gpui::*;

#[derive(Clone, Debug, Default)]
pub struct ContextUsageState {
    pub total_tokens: u32,
    pub context_window: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub reasoning_tokens: u32,
    pub expanded: bool,
    /// P1-4: last turn cache receipt (` (H cached / M new)`). Empty when
    /// `ui.show_turn_usage` is off — accounting still runs server-side.
    pub last_receipt: String,
    /// Per-turn cost line (`¥0.0123`), priced with the official per-model row
    /// at occurrence time (Reasonix catalog rule). Empty when unpriced.
    pub last_cost: String,
    /// Current model id for the price hint (`deepseek-flash`, …).
    pub last_model: String,
    /// Session totals so the user sees money spent at a glance.
    pub session_cost: f64,
    pub session_currency: String,
    pub session_turns: u64,
}

impl ContextUsageState {
    pub fn percent(&self) -> u32 {
        if self.context_window == 0 {
            return 0;
        }
        ((self.total_tokens as f64 / self.context_window as f64) * 100.0).round() as u32
    }

    /// Bar fill width capped at 100% for layout; label may exceed 100%.
    pub fn bar_fill_percent(&self) -> u32 {
        self.percent().min(100)
    }

    pub fn color(&self, theme: &ThemeTokens) -> Rgba {
        let p = self.percent();
        if p >= 90 {
            theme.error
        } else if p >= 70 {
            theme.warning
        } else {
            theme.success
        }
    }
}

pub struct ContextUsageMeter;

impl ContextUsageMeter {
    /// Compact inline meter for the session header.
    pub fn render_compact(
        cx: &Context<crate::workspace::WorkspaceView>,
        state: &ContextUsageState,
    ) -> impl IntoElement {
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
        let pct = state.percent();
        let bar_pct = state.bar_fill_percent();
        let ring_color = state.color(&theme);

        div()
            .id("context-usage-compact")
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .tooltip(move |_, cx| {
                cx.new(|_| crate::git_panel::GitFileTooltip {
                    text: code_plus_plus_i18n::t("context.title").into(),
                })
                .into()
            })
            // P1-1 release-to-confirm + P1-2 stop.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_context_usage_detail(cx);
                }),
            )
            .child(
                div()
                    .w(px(56.))
                    .h(px(4.))
                    .rounded_full()
                    .bg(theme.bg_active)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .w(px((bar_pct as f32 / 100.0 * 56.0).max(2.0)))
                            .bg(ring_color),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ring_color)
                    .child(code_plus_plus_i18n::tr!(
                        "context.percent",
                        "percent" => pct as f64 / 100.0
                    )),
            )
    }

    pub fn render_detail_popover(
        state: &ContextUsageState,
        theme: &ThemeTokens,
    ) -> impl IntoElement {
        let mut panel = div()
            .absolute()
            .top(px(32.))
            .right_0()
            .w(px(240.))
            .p_3()
            .rounded_md()
            .bg(theme.bg_surface)
            .border_1()
            .border_color(theme.border_default)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .text_xs()
            .text_color(theme.text_secondary)
            .child(code_plus_plus_i18n::tr!(
                "context.title",
                "used" => state.total_tokens as u64,
                "total" => state.context_window as u64
            ))
            .child(code_plus_plus_i18n::tr!(
                "context.input",
                "value" => code_plus_plus_i18n::count(state.input_tokens as u64)
            ))
            .child(code_plus_plus_i18n::tr!(
                "context.last_output",
                "value" => code_plus_plus_i18n::count(state.output_tokens as u64)
            ))
            .child(code_plus_plus_i18n::tr!(
                "context.last_reasoning",
                "value" => code_plus_plus_i18n::count(state.reasoning_tokens as u64)
            ));
        // P1-4: receipt line only when `show_turn_usage` produced one.
        if !state.last_receipt.is_empty() {
            panel = panel.child(code_plus_plus_i18n::tr!(
                "context.cache",
                "value" => state.last_receipt.as_str()
            ));
        }
        if !state.last_model.is_empty() {
            panel = panel.child(code_plus_plus_i18n::tr!(
                "context.model",
                "value" => state.last_model.as_str()
            ));
        }
        if !state.last_cost.is_empty() {
            panel = panel.child(code_plus_plus_i18n::tr!(
                "context.cost",
                "value" => state.last_cost.as_str()
            ));
        }
        if state.session_turns > 0 {
            panel = panel.child(code_plus_plus_i18n::tr!(
                "context.session_cost",
                "value" => code_plus_plus_core::pricing::format_cost(
                    if state.session_currency.is_empty() { "CNY" } else { state.session_currency.as_str() },
                    state.session_cost,
                ).as_str(),
                "count" => state.session_turns
            ));
        }
        panel
    }

    pub fn render(
        cx: &Context<crate::workspace::WorkspaceView>,
        state: &ContextUsageState,
    ) -> impl IntoElement {
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
        let pct = state.percent();
        let ring_color = state.color(&theme);

        let mut panel = div()
            .id("context-usage")
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_context_usage_detail(cx);
                }),
            )
            .child(
                div()
                    .w(px(28.))
                    .h(px(28.))
                    .rounded_full()
                    .border_2()
                    .border_color(ring_color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ring_color)
                    .child(code_plus_plus_i18n::tr!(
                        "context.percent",
                        "percent" => pct as f64 / 100.0
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(code_plus_plus_i18n::tr!(
                        "context.usage",
                        "used" => state.total_tokens as u64,
                        "total" => state.context_window as u64
                    )),
            );

        if state.expanded {
            let mut detail = div()
                .id("context-usage-detail")
                .occlude()
                .absolute()
                .top(px(36.))
                .right_0()
                .w(px(240.))
                .p_3()
                .rounded_md()
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border_default)
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(theme.text_secondary)
                // P1-6: swallow clicks inside the popup so selecting/copying
                // numbers never bubbles to the parent toggle and collapses it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(code_plus_plus_i18n::tr!(
                    "context.input",
                    "value" => code_plus_plus_i18n::count(state.input_tokens as u64)
                ))
                .child(code_plus_plus_i18n::tr!(
                    "context.output",
                    "value" => code_plus_plus_i18n::count(state.output_tokens as u64)
                ))
                .child(code_plus_plus_i18n::tr!(
                    "context.reasoning",
                    "value" => code_plus_plus_i18n::count(state.reasoning_tokens as u64)
                ));
            // P1-4: receipt line only when `show_turn_usage` produced one.
            if !state.last_receipt.is_empty() {
                detail = detail.child(code_plus_plus_i18n::tr!(
                    "context.cache",
                    "value" => state.last_receipt.as_str()
                ));
            }
            if !state.last_model.is_empty() {
                detail = detail.child(code_plus_plus_i18n::tr!(
                    "context.model",
                    "value" => state.last_model.as_str()
                ));
            }
            if !state.last_cost.is_empty() {
                detail = detail.child(code_plus_plus_i18n::tr!(
                    "context.cost",
                    "value" => state.last_cost.as_str()
                ));
            }
            if state.session_turns > 0 {
                detail = detail.child(code_plus_plus_i18n::tr!(
                    "context.session_cost",
                    "value" => code_plus_plus_core::pricing::format_cost(
                        if state.session_currency.is_empty() { "CNY" } else { state.session_currency.as_str() },
                        state.session_cost,
                    ).as_str(),
                    "count" => state.session_turns
                ));
            }
            panel = panel.child(detail);
        }

        panel
    }
}
