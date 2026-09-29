use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::time::Duration;

use super::registry::Tool;

pub struct WebsearchTool {
    provider: String,
}

impl WebsearchTool {
    pub fn new(config: &deepseekcode_config::DeepSeekCodeConfig) -> Self {
        let provider = config
            .websearch
            .as_ref()
            .and_then(|w| w.provider.clone())
            .unwrap_or_else(|| "duckduckgo".into());
        Self { provider }
    }
}

#[async_trait]
impl Tool for WebsearchTool {
    fn name(&self) -> &str {
        "websearch"
    }
    fn description(&self) -> &str {
        "Search the web for information"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            tool = "websearch",
            query_len = input["query"].as_str().map(|s| s.len()).unwrap_or(0),
            provider = %self.provider,
            "websearch started"
        );
        let query = input["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return Err(ToolFailure {
                message: "query must not be empty".into(),
                retryable: false,
            });
        }
        let results = match self.provider.as_str() {
            "brave" => search_brave(query).await?,
            _ => search_duckduckgo(query).await?,
        };
        Ok(json!({ "query": query, "results": results, "provider": self.provider }))
    }
}

async fn search_duckduckgo(query: &str) -> Result<Vec<Value>, ToolFailure> {
    let url = format!(
        "https://lite.duckduckgo.com/lite/?q={}",
        urlencoding_encode(query)
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
    let body = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?
        .text()
        .await
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
    Ok(parse_duckduckgo_results(&body))
}

async fn search_brave(query: &str) -> Result<Vec<Value>, ToolFailure> {
    let api_key = std::env::var("BRAVE_SEARCH_API_KEY").map_err(|_| ToolFailure {
        message: "BRAVE_SEARCH_API_KEY not set for brave websearch provider".into(),
        retryable: false,
    })?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}",
        urlencoding_encode(query)
    );
    let body: Value = client
        .get(&url)
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?
        .json()
        .await
        .map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
    let results = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(json!({
                        "title": item.get("title")?.as_str()?,
                        "url": item.get("url")?.as_str()?,
                        "snippet": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(results)
}

fn parse_duckduckgo_results(html: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("result-link") {
        let after = &rest[start..];
        let href = extract_html_attr(after, "href");
        let title = extract_tag_text(after, "a").unwrap_or_default();
        let snippet = after
            .split_once("</a>")
            .and_then(|(_, tail)| tail.lines().find(|l| !l.trim().is_empty()))
            .map(|l| strip_html_tags(l.trim()))
            .unwrap_or_default();
        if let Some(url) = href {
            if !url.is_empty() {
                results.push(json!({
                    "title": title,
                    "url": url,
                    "snippet": snippet,
                }));
            }
        }
        rest = &rest[start + 10..];
        if results.len() >= 8 {
            break;
        }
    }
    results
}

fn extract_html_attr(fragment: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = fragment.find(&needle)? + needle.len();
    let end = fragment[start..].find('"')? + start;
    Some(fragment[start..end].to_string())
}

fn extract_tag_text(fragment: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = fragment.find(&open)?;
    let close_open = fragment[start..].find('>')? + start + 1;
    let close = fragment[close_open..].find(&format!("</{tag}>"))? + close_open;
    Some(strip_html_tags(&fragment[close_open..close]))
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
