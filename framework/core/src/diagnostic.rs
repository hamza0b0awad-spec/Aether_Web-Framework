//! # Aether Diagnostic & Error Experience Engine
//!
//! Provides structured error codes (E1001-E5002), actionable developer hints,
//! human-readable and machine-parsable terminal diagnostics, and safe production sanitization.

use std::fmt;

/// Standardized Aether Error Code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// E1001: Server startup failed (e.g., port binding / address already in use)
    E1001ServerStartupFailed,
    /// E1002: Invalid or missing environment/configuration value
    E1002InvalidConfig,
    /// E2001: Route configuration collision or invalid pattern
    E2001RouteCollision,
    /// E2002: Route not found (HTTP 404)
    E2002RouteNotFound,
    /// E2003: HTTP Method not allowed for route (HTTP 405)
    E2003MethodNotAllowed,
    /// E3001: Malformed HTTP request syntax or RFC 9112 smuggling violation
    E3001MalformedRequest,
    /// E3002: HTTP payload budget exceeded (Headers or Body limit)
    E3002PayloadLimitExceeded,
    /// E3003: `Expect` named an expectation-extension this server cannot meet (HTTP 417).
    /// Kept out of `E3001` because a malformed head fails the grammar and this one passes it —
    /// the request is well-formed, the promise is not.
    E3003UnsupportedExpectation,
    /// E4001: JSON or form deserialization failure
    E4001DeserializationFailed,
    /// E4002: Input validation or constraint failure
    E4002ValidationFailed,
    /// E4003: A handler returned `Err` and its `IntoResponseError` chose a 4xx status that no
    /// more specific code covers. It stayed separate from `E5002` because a 4xx body must never
    /// promise an internal failure — and a 5xx must never be reported as a 4xx.
    E4003HandlerReturnedError,
    /// E5001: Circular dependency in DI graph
    E5001DependencyCycle,
    /// E5002: Internal server failure, produced by `AetherError::Timeout` and `AetherError::Io`
    /// and sent as 500 or 504 on a handler error
    E5002InternalServerError,
}

impl ErrorCode {
    pub fn code_str(&self) -> &'static str {
        match self {
            ErrorCode::E1001ServerStartupFailed => "E1001",
            ErrorCode::E1002InvalidConfig => "E1002",
            ErrorCode::E2001RouteCollision => "E2001",
            ErrorCode::E2002RouteNotFound => "E2002",
            ErrorCode::E2003MethodNotAllowed => "E2003",
            ErrorCode::E3001MalformedRequest => "E3001",
            ErrorCode::E3002PayloadLimitExceeded => "E3002",
            ErrorCode::E3003UnsupportedExpectation => "E3003",
            ErrorCode::E4001DeserializationFailed => "E4001",
            ErrorCode::E4002ValidationFailed => "E4002",
            ErrorCode::E4003HandlerReturnedError => "E4003",
            ErrorCode::E5001DependencyCycle => "E5001",
            ErrorCode::E5002InternalServerError => "E5002",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            ErrorCode::E1001ServerStartupFailed => "Server startup failed",
            ErrorCode::E1002InvalidConfig => "Invalid configuration parameter",
            ErrorCode::E2001RouteCollision => "Route registration conflict",
            ErrorCode::E2002RouteNotFound => "Route not found",
            ErrorCode::E2003MethodNotAllowed => "Method not allowed for route",
            ErrorCode::E3001MalformedRequest => "Malformed HTTP request syntax",
            ErrorCode::E3002PayloadLimitExceeded => "HTTP payload budget exceeded",
            ErrorCode::E3003UnsupportedExpectation => "Unsupported expectation",
            ErrorCode::E4001DeserializationFailed => "JSON/Form deserialization failure",
            ErrorCode::E4002ValidationFailed => "Input validation failed",
            ErrorCode::E4003HandlerReturnedError => "Handler returned an error",
            ErrorCode::E5001DependencyCycle => "Circular dependency detected in DI graph",
            ErrorCode::E5002InternalServerError => "Internal server error",
        }
    }
}

/// Rich Diagnostic Report
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: ErrorCode,
    pub reason: String,
    pub context_key: Option<String>,
    pub context_val: Option<String>,
    pub hint: Option<String>,
    pub suggestions: Vec<String>,
}

impl Diagnostic {
    pub fn new(code: ErrorCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            context_key: None,
            context_val: None,
            hint: None,
            suggestions: Vec::new(),
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.context_key = Some(key.into());
        self.context_val = Some(val.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    /// Formats the diagnostic into an actionable, clean terminal string
    pub fn format_terminal(&self, color: bool) -> String {
        let code = self.code.code_str();
        let title = self.code.title();

        let prefix = if color {
            format!("\x1b[1;31mERROR [{}]\x1b[0m \x1b[1m{}\x1b[0m", code, title)
        } else {
            format!("ERROR [{}] {}", code, title)
        };

        let mut out = String::new();
        out.push_str(&prefix);
        out.push('\n');

        if let (Some(k), Some(v)) = (&self.context_key, &self.context_val) {
            out.push_str(&format!("  {}: {}\n", k, v));
        }

        out.push_str(&format!("  Reason : {}\n", self.reason));

        if let Some(ref hint) = self.hint {
            out.push_str("\n  Hint:\n");
            out.push_str(&format!("  {}\n", hint));
        }

        if !self.suggestions.is_empty() {
            out.push_str("\n  Try:\n");
            for s in &self.suggestions {
                out.push_str(&format!("  • {}\n", s));
            }
        }

        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_terminal(false))
    }
}

/// Helper for rendering visual server startup banners
pub struct ServerBanner {
    pub env: &'static str,
    pub address: String,
    pub routes_count: usize,
    pub workers: usize,
}

impl ServerBanner {
    pub fn render(&self) -> String {
        // One number sizes the whole box: every line is "│" + an INNER-wide field + "│". The rows
        // used to be hand-padded and drifted out of agreement — the border carried 61 dashes, the
        // `{:<44}` content rows came to 61 characters in total (two short of the frame), the title
        // landed at 62, and only the literal Allocator line, with its own two extra spaces,
        // happened to reach 63. Measuring here is in characters, not display cells: the ⚡ is one
        // character that terminals commonly render two cells wide, so the title row measures 63
        // characters and 64 cells and its right border can still sit one column outside the frame.
        const INNER: usize = 61;
        let rule = |left: &str, right: &str| -> String {
            format!("{}{}{}\n", left, "─".repeat(INNER), right)
        };
        let row = |text: &str| -> String { format!("│{:<w$}│\n", text, w = INNER) };

        let mut out = String::new();
        out.push_str(&rule("┌", "┐"));
        out.push_str(&row(
            " ⚡ AETHER WEB FRAMEWORK (Rust Async HTTP/1.1 Engine)",
        ));
        out.push_str(&rule("├", "┤"));
        out.push_str(&row(&format!(" {:<12}: {:<44}", "Environment", self.env)));
        out.push_str(&row(&format!(" {:<12}: {:<44}", "Address", self.address)));
        out.push_str(&row(&format!(
            " {:<12}: {:<44}",
            "Routes", self.routes_count
        )));
        out.push_str(&row(&format!(
            " {:<12}: {:<44}",
            "Workers",
            format!("{} (active; tokio work-stealing)", self.workers)
        )));
        out.push_str(&row(&format!(
            " {:<12}: {:<44}",
            "Allocator", "System allocator (per-request heap)"
        )));
        out.push_str(&rule("└", "┘"));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code, by value. `ErrorCode` is `Copy`, so a table is the whole enum rather than a
    /// sample; a new arm without a row here fails the count assertion instead of escaping both
    /// tests below.
    const CODES: [ErrorCode; 13] = [
        ErrorCode::E1001ServerStartupFailed,
        ErrorCode::E1002InvalidConfig,
        ErrorCode::E2001RouteCollision,
        ErrorCode::E2002RouteNotFound,
        ErrorCode::E2003MethodNotAllowed,
        ErrorCode::E3001MalformedRequest,
        ErrorCode::E3002PayloadLimitExceeded,
        ErrorCode::E3003UnsupportedExpectation,
        ErrorCode::E4001DeserializationFailed,
        ErrorCode::E4002ValidationFailed,
        ErrorCode::E4003HandlerReturnedError,
        ErrorCode::E5001DependencyCycle,
        ErrorCode::E5002InternalServerError,
    ];

    /// `code_str()` is what reaches the wire and the terminal; the variant name is what the
    /// source reads. A code whose string and name disagree is a diagnostic that points at the
    /// wrong place in the documentation, and no test compared them before track 118.
    #[test]
    fn every_code_string_agrees_with_its_own_variant_name() {
        assert_eq!(CODES.len(), 13);
        for code in CODES {
            let text = code.code_str();
            assert_eq!(text.len(), 5, "{code:?}");
            assert!(text.starts_with('E'), "{code:?} -> {text}");
            assert!(
                text[1..].bytes().all(|byte| byte.is_ascii_digit()),
                "{code:?} -> {text}"
            );
            assert!(
                format!("{code:?}").starts_with(text),
                "the variant name and its code disagree: {code:?} -> {text}"
            );
        }
    }

    #[test]
    fn every_title_is_present_and_distinct() {
        let mut titles = std::collections::BTreeSet::new();
        for code in CODES {
            let title = code.title();
            assert!(!title.trim().is_empty(), "{code:?}");
            assert!(titles.insert(title), "two codes share the title {title:?}");
        }
        assert_eq!(titles.len(), CODES.len());
    }

    /// The exact rendered shape, not a substring: a report that dropped the context line, moved
    /// `Reason` or lost its bullet would still satisfy `contains` checks on each fragment.
    #[test]
    fn the_terminal_form_renders_every_field_the_builder_accepts() {
        let diag = Diagnostic::new(
            ErrorCode::E4001DeserializationFailed,
            "missing field `name`",
        )
        .with_context("Field Validation", "missing field `name`")
        .with_hint("The body did not match the DTO.")
        .with_suggestion("Send a `name` string.");

        let expected = concat!(
            "ERROR [E4001] JSON/Form deserialization failure\n",
            "  Field Validation: missing field `name`\n",
            "  Reason : missing field `name`\n",
            "\n",
            "  Hint:\n",
            "  The body did not match the DTO.\n",
            "\n",
            "  Try:\n",
            "  • Send a `name` string.\n",
        );
        assert_eq!(diag.format_terminal(false), expected);
        assert_eq!(
            diag.to_string(),
            expected,
            "Display must be the uncoloured form"
        );

        let coloured = concat!(
            "\x1b[1;31mERROR [E4001]\x1b[0m ",
            "\x1b[1mJSON/Form deserialization failure\x1b[0m\n",
            "  Field Validation: missing field `name`\n",
            "  Reason : missing field `name`\n",
            "\n",
            "  Hint:\n",
            "  The body did not match the DTO.\n",
            "\n",
            "  Try:\n",
            "  • Send a `name` string.\n",
        );
        assert_eq!(diag.format_terminal(true), coloured);
    }

    /// A report with no hint and no suggestions is two lines and no empty section headings.
    #[test]
    fn an_empty_builder_renders_no_empty_sections() {
        let plain =
            Diagnostic::new(ErrorCode::E5002InternalServerError, "boom").format_terminal(false);
        assert_eq!(
            plain,
            "ERROR [E5002] Internal server error\n  Reason : boom\n"
        );
    }
}
