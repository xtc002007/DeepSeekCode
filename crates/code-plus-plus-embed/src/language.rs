//! Request-scoped response language for the HTTP API.
//!
//! The API keeps `code` as the stable machine-readable contract. Human text is
//! localized in three outcome fields:
//!
//! - `error`: the message in the request's language,
//! - `error_en`: the same message in English,
//! - `detail`: present only when the English text alone would discard useful
//!   diagnostics (for example an OS I/O failure).
//!
//! Language selection never trusts an unbounded value. An explicit
//! `X-CodePlusPlus-Locale` header wins when it parses to a shipped locale,
//! `Accept-Language` is honoured with quality values, and otherwise the
//! project config (`ui.language`) or process environment decides.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use code_plus_plus_i18n::{message::ArgValue, Locale};
use serde_json::{json, Value};

use crate::state::AppState;

/// Client-controlled override. Documented in `docs/i18n.md`.
pub const LOCALE_HEADER: &str = "x-code-plus-plus-locale";
const MAX_LOCALE_HEADER_LEN: usize = 64;
const MAX_ACCEPT_LANGUAGE_LEN: usize = 512;

tokio::task_local! {
    static REQUEST_LOCALE: Locale;
}

fn request_locale_from_state(state: &AppState) -> Locale {
    state.locale.resolve(Locale::En)
}

/// The locale for the current request.
///
/// Handlers call this when constructing an error. Outside a request (tests,
/// background tasks, startup paths), it returns the process locale.
pub fn current_locale() -> Locale {
    REQUEST_LOCALE
        .try_with(|locale| *locale)
        .unwrap_or_else(|_| code_plus_plus_i18n::locale())
}

/// Set the request locale for `future`, then run it.
pub async fn scope_locale<F, T>(locale: Locale, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    REQUEST_LOCALE.scope(locale, future).await
}

fn parse_header_locale(headers: &HeaderMap) -> Option<Locale> {
    let raw = headers.get(LOCALE_HEADER)?.to_str().ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_LOCALE_HEADER_LEN {
        return None;
    }
    Locale::parse(trimmed)
}

fn parse_accept_language(headers: &HeaderMap) -> Option<Locale> {
    let raw = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)?
        .to_str()
        .ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_ACCEPT_LANGUAGE_LEN {
        return None;
    }
    Locale::parse_accept_language(trimmed)
}

/// Resolve `Accept-Language`/`X-CodePlusPlus-Locale` without a configured
/// default. Exposed for unit tests of the selection rules.
pub fn resolve_request_locale(headers: &HeaderMap) -> Option<Locale> {
    parse_header_locale(headers).or_else(|| parse_accept_language(headers))
}

pub async fn locale_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let locale =
        resolve_request_locale(req.headers()).unwrap_or_else(|| request_locale_from_state(&state));
    scope_locale(locale, next.run(req)).await
}

/// Localized error JSON for a fixed catalog message.
pub fn error_body(key: &str, code: &str, args: &[ArgValue]) -> Value {
    let locale = current_locale();
    json!({
        "error": code_plus_plus_i18n::t_args_in(locale, key, args),
        "error_en": code_plus_plus_i18n::t_args_in(Locale::En, key, args),
        "code": code,
        "lang": locale.tag(),
    })
}

/// Localized payload for an unknown agent/model selector.
///
/// The unknown selector is user controlled, so it is passed as an argument to
/// an exact catalog message rather than interpolated into an English string.
pub fn unknown_agent_body(other: &str) -> Value {
    error_body(
        "error.unknown_agent",
        "invalid_input",
        &[ArgValue::new("value", other)],
    )
}

/// Localized response for a fixed catalog message.
pub fn error_response(status: StatusCode, key: &str, code: &str, args: &[ArgValue]) -> Response {
    (status, Json(error_body(key, code, args))).into_response()
}

/// Localized response when only the original English diagnostics should be
/// kept in a separate `detail` field.
///
/// `error` and `error_en` carry the stable category; raw operating-system and
/// provider details stay in `detail` instead of being concatenated into a
/// translated sentence. That keeps translations exact and avoids putting a new
/// English fragment in front of translators for every possible I/O failure.
pub fn service_error_body(code: &str, key: &str, detail_en: Option<&str>) -> Value {
    let locale = current_locale();
    let mut body = serde_json::Map::with_capacity(5);
    body.insert(
        "error".to_string(),
        Value::String(code_plus_plus_i18n::t_in(locale, key)),
    );
    body.insert(
        "error_en".to_string(),
        Value::String(code_plus_plus_i18n::t_in(Locale::En, key)),
    );
    if let Some(detail) = detail_en.filter(|detail| !detail.trim().is_empty()) {
        let mut detail = detail.trim().to_string();
        if detail.len() > 500 {
            detail.truncate(500);
        }
        body.insert("detail".to_string(), Value::String(detail));
    }
    body.insert("code".to_string(), Value::String(code.to_string()));
    body.insert("lang".to_string(), Value::String(locale.tag().to_string()));
    Value::Object(body)
}

/// Localized response for a service failure with raw English diagnostics.
pub fn service_error_response(
    status: StatusCode,
    code: &str,
    key: &str,
    detail_en: Option<&str>,
) -> Response {
    (status, Json(service_error_body(code, key, detail_en))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                name.parse::<axum::http::HeaderName>().expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        map
    }

    #[test]
    fn explicit_locale_header_beats_accept_language() {
        let map = headers(&[(LOCALE_HEADER, "ru"), ("accept-language", "de-DE,de;q=0.9")]);
        assert_eq!(resolve_request_locale(&map), Some(Locale::Ru));
    }

    #[test]
    fn accept_language_quality_values_are_honoured() {
        let map = headers(&[("accept-language", "fr;q=0.2,ja;q=0.9")]);
        assert_eq!(resolve_request_locale(&map), Some(Locale::Ja));
    }

    #[test]
    fn invalid_or_oversized_headers_do_not_select_a_locale() {
        let oversized = "x".repeat(MAX_ACCEPT_LANGUAGE_LEN + 1);
        let map = headers(&[
            (LOCALE_HEADER, "klingon"),
            ("accept-language", oversized.as_str()),
        ]);
        assert_eq!(resolve_request_locale(&map), None);

        let map = headers(&[(LOCALE_HEADER, "zh-HK")]);
        assert_eq!(resolve_request_locale(&map), Some(Locale::ZhHant));
    }

    #[tokio::test]
    async fn errors_carry_both_the_request_language_and_english() {
        let rendered = scope_locale(Locale::ZhHans, async {
            error_body("error.session_not_found", "not_found", &[])
        })
        .await;
        assert_eq!(rendered["lang"], "zh-Hans");
        assert_eq!(rendered["code"], "not_found");
        assert_eq!(rendered["error"], "未找到会话");
        assert_eq!(rendered["error_en"], "session not found");
        assert!(rendered.get("detail").is_none());
    }

    #[tokio::test]
    async fn dynamic_diagnostics_stay_separate_from_the_translated_category() {
        let rendered = scope_locale(Locale::Ru, async {
            service_error_body("internal", "error.internal", Some("disk is full"))
        })
        .await;
        assert_eq!(rendered["error"], "внутренняя ошибка");
        assert_eq!(rendered["error_en"], "internal error");
        assert_eq!(rendered["detail"], "disk is full");
        assert_eq!(rendered["lang"], "ru");
    }

    #[tokio::test]
    async fn outside_a_request_the_process_locale_is_used() {
        code_plus_plus_i18n::set_locale(Locale::Es);
        let rendered = error_body("error.forbidden", "forbidden", &[]);
        code_plus_plus_i18n::set_locale(Locale::En);
        assert_eq!(rendered["error"], "prohibido");
        assert_eq!(rendered["lang"], "es");
    }
}
