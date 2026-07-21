//! Syntax highlighting helpers using tree-sitter (read-only editor).

use crate::theme::ThemeTokens;
use gpui::Rgba;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

pub struct SyntaxHighlighter {
    parser: Parser,
    #[allow(dead_code)]
    language: Language,
    query: Query,
}

impl SyntaxHighlighter {
    pub fn for_path(path: &str) -> Option<Self> {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext {
            "rs" => Self::for_language(tree_sitter_rust::LANGUAGE.into(), RUST_QUERY),
            "js" | "jsx" | "mjs" | "cjs" => {
                Self::for_language(tree_sitter_javascript::LANGUAGE.into(), JS_QUERY)
            }
            "ts" | "tsx" => {
                Self::for_language(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), TS_QUERY)
            }
            "py" | "pyw" => Self::for_language(tree_sitter_python::LANGUAGE.into(), PY_QUERY),
            _ => None,
        }
    }

    pub fn for_rust() -> Option<Self> {
        Self::for_path("file.rs")
    }

    fn for_language(language: Language, query_src: &str) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        let query = Query::new(&language, query_src).ok()?;
        Some(Self {
            parser,
            language,
            query,
        })
    }

    pub fn highlight_line(&mut self, line: &str) -> Vec<(usize, usize, HighlightKind)> {
        let tree = match self.parser.parse(line, None) {
            Some(t) => t,
            None => return Vec::new(),
        };
        let root = tree.root_node();
        let mut cursor = QueryCursor::new();
        let mut captures = cursor.captures(&self.query, root, line.as_bytes());
        let mut spans = Vec::new();
        while let Some((mat, _)) = captures.next() {
            for capture in mat.captures {
                let name = self.query.capture_names()[capture.index as usize];
                let kind = match name {
                    "comment" => HighlightKind::Comment,
                    "string" => HighlightKind::String,
                    "number" => HighlightKind::Number,
                    "keyword" => HighlightKind::Keyword,
                    "type" => HighlightKind::Type,
                    "function" => HighlightKind::Function,
                    "property" => HighlightKind::Property,
                    _ => HighlightKind::Default,
                };
                let node = capture.node;
                spans.push((node.start_byte(), node.end_byte(), kind));
            }
        }
        spans
    }
}

const RUST_QUERY: &str = r#"
(line_comment) @comment
(block_comment) @comment
(string_literal) @string
(char_literal) @string
(integer_literal) @number
(float_literal) @number
(boolean_literal) @keyword
(identifier) @variable
(type_identifier) @type
(field_identifier) @property
(function_item name: (identifier) @function)
(struct_item name: (type_identifier) @type)
(enum_item name: (type_identifier) @type)
"#;

const JS_QUERY: &str = r#"
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(property_identifier) @property
(identifier) @variable
(function_declaration name: (identifier) @function)
"#;

const TS_QUERY: &str = r#"
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(type_identifier) @type
(property_identifier) @property
(identifier) @variable
(function_declaration name: (identifier) @function)
"#;

const PY_QUERY: &str = r#"
(comment) @comment
(string) @string
(integer) @number
(float) @number
(identifier) @variable
(function_definition name: (identifier) @function)
(class_definition name: (identifier) @type)
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Default,
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Function,
    Property,
}

pub fn highlight_rgba(kind: HighlightKind, theme: &ThemeTokens) -> Rgba {
    match kind {
        HighlightKind::Comment => theme.syntax.comment,
        HighlightKind::String => theme.syntax.string,
        HighlightKind::Number => theme.syntax.number,
        HighlightKind::Keyword => theme.syntax.keyword,
        HighlightKind::Type => theme.syntax.type_,
        HighlightKind::Function => theme.syntax.function,
        HighlightKind::Property => theme.syntax.property,
        HighlightKind::Default => theme.syntax.default,
    }
}

/// Legacy u32 accessor for tests.
pub fn highlight_color(kind: HighlightKind) -> u32 {
    match kind {
        HighlightKind::Comment => 0x6a9955,
        HighlightKind::String => 0xce9178,
        HighlightKind::Number => 0xb5cea8,
        HighlightKind::Keyword => 0x569cd6,
        HighlightKind::Type => 0x4ec9b0,
        HighlightKind::Function => 0xdcdcaa,
        HighlightKind::Property => 0x9cdcfe,
        HighlightKind::Default => 0xd4d4d4,
    }
}

/// Rope-backed read-only buffer for efficient line access.
#[derive(Clone, Debug)]
pub struct ReadOnlyBuffer {
    rope: ropey::Rope,
}

impl ReadOnlyBuffer {
    pub fn from_str(content: &str) -> Self {
        Self {
            rope: ropey::Rope::from_str(content),
        }
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        self.rope.line(idx).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rope_buffer_line_access() {
        let buf = ReadOnlyBuffer::from_str("line1\nline2\n");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line(0).trim_end(), "line1");
    }

    #[test]
    fn highlight_colors_defined() {
        assert_eq!(highlight_color(HighlightKind::Comment), 0x6a9955);
    }

    #[test]
    fn rust_highlighter_alias_matches_path() {
        assert!(SyntaxHighlighter::for_rust().is_some());
        assert_eq!(
            SyntaxHighlighter::for_rust().is_some(),
            SyntaxHighlighter::for_path("main.rs").is_some()
        );
    }

    #[test]
    fn detects_rust_and_python_highlighters() {
        assert!(SyntaxHighlighter::for_path("main.rs").is_some());
        assert!(SyntaxHighlighter::for_path("app.py").is_some());
        assert!(SyntaxHighlighter::for_path("index.ts").is_some());
        assert!(SyntaxHighlighter::for_path("readme.md").is_none());
    }
}
