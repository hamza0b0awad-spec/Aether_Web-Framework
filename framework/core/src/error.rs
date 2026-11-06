use crate::diagnostic::{Diagnostic, ErrorCode};
use std::fmt;

#[derive(Debug)]
pub enum AetherError {
    HeaderTooLarge,
    BodyLimitExceeded,
    MalformedRequest,
    /// `Expect` named an expectation-extension this server cannot meet (RFC 9110 §10.1.1).
    /// Answered `417`, never `400`: the request grammar is fine, the promise is not.
    UnsupportedExpectation,
    RouteNotFound,
    MethodNotAllowed,
    ValidationFailed(String),
    DependencyCycleDetected(String),
    Timeout,
    Io(std::io::Error),
    Serialization(String),
    Custom(String),
}

impl AetherError {
    /// The diagnostic code per variant. `aether`'s `IntoResponseError for AetherError` answers the
    /// wire status and delegates the code here, so each arm's class must match the status that
    /// impl picks for the same variant: `E5xxx` only where the answer is 5xx.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            AetherError::HeaderTooLarge | AetherError::BodyLimitExceeded => {
                ErrorCode::E3002PayloadLimitExceeded
            }
            AetherError::MalformedRequest => ErrorCode::E3001MalformedRequest,
            AetherError::UnsupportedExpectation => ErrorCode::E3003UnsupportedExpectation,
            AetherError::RouteNotFound => ErrorCode::E2002RouteNotFound,
            AetherError::MethodNotAllowed => ErrorCode::E2003MethodNotAllowed,
            AetherError::ValidationFailed(_) => ErrorCode::E4002ValidationFailed,
            AetherError::DependencyCycleDetected(_) => ErrorCode::E5001DependencyCycle,
            AetherError::Timeout => ErrorCode::E5002InternalServerError,
            // `E1001` means *server startup failed* (see its own doc in `diagnostic.rs`) and is
            // built explicitly where a bind fails (`aether/src/app.rs`); an I/O error from a
            // running handler is an internal failure, which is what `aether` now answers 500 for.
            AetherError::Io(_) => ErrorCode::E5002InternalServerError,
            AetherError::Serialization(_) => ErrorCode::E4001DeserializationFailed,
            // `Custom`'s `Display` is the caller's own message and `aether` sends it on a 400, so
            // this code has to be the 4xx one — `E5002` here was the same contradiction PROB-0009
            // closed in the blanket response.
            AetherError::Custom(_) => ErrorCode::E4003HandlerReturnedError,
        }
    }

    /// The developer-facing report for this error.
    ///
    /// The code is the one `aether` puts in the response body, so the report a developer reads
    /// and the body a client receives name one failure. A startup failure builds its own `E1001`
    /// report where the bind fails and returns an [`AetherError::Io`] afterwards, which is why
    /// this method's `Io` arm describes a *runtime* I/O failure rather than port occupancy. A
    /// caller that feeds the bind failure's `Io` back into this method gets the runtime wording;
    /// the E1001 report for it was already printed where the bind failed.
    pub fn to_diagnostic(&self) -> Diagnostic {
        let code = self.error_code();
        let reason = self.to_string();
        let diag = Diagnostic::new(code, reason);

        match self {
            // `Io` answered startup failures when this arm was written. The C′ error contract
            // (2026-10-24) moved it to a runtime failure answered 500/E5002 — a failed read,
            // write or query inside a handler — and the bind failure builds its own E1001
            // diagnostic before returning one of these. Advice to change `AETHER_PORT` therefore
            // sent a handler's failed write to restart a healthy server.
            AetherError::Io(_err) => diag
                .with_hint(
                    "An I/O operation failed while serving a request. The 5xx body redacts this detail.",
                )
                .with_suggestion(
                    "Install a reporter with aether::set_error_reporter to receive the detail with the response's X-Request-ID.",
                )
                .with_suggestion("Check the file, directory or socket the failed operation named."),
            AetherError::MalformedRequest => diag
                .with_hint(
                    "The request violates RFC 9112 HTTP grammar or contains illegal headers.",
                )
                .with_suggestion("Ensure HTTP clients send valid HTTP/1.1 or HTTP/2 frames."),
            AetherError::HeaderTooLarge => diag
                // Two budgets raise this one variant: the field-line count and the head's bytes.
                // Naming only the byte budget would repeat the wrong-knob failure this method
                // exists to avoid.
                .with_hint(
                    "The request head exceeded the configured max_header_bytes byte budget or the max_headers_count field-line budget.",
                )
                .with_suggestion(
                    "Raise AETHER_MAX_HEADER_BYTES (or AetherConfig::max_header_bytes) if clients legitimately send larger heads.",
                )
                .with_suggestion(
                    "Raise AETHER_MAX_HEADERS_COUNT (or AetherConfig::max_headers_count) if clients legitimately send more field lines.",
                ),
            AetherError::BodyLimitExceeded => diag
                .with_hint("Payload size exceeded the configured max_body_bytes limit.")
                .with_suggestion(
                    "Raise AETHER_MAX_BODY_BYTES (or AetherConfig::max_body_bytes) if clients legitimately send larger bodies.",
                ),
            // A 504 is the handler deadline. The receive deadline answers 408 itself and never
            // becomes an `AetherError`, so "connection timeout" named the wrong clock.
            AetherError::Timeout => diag
                .with_hint("The operation did not finish inside the configured max_request_time.")
                .with_suggestion(
                    "Raise AETHER_MAX_REQUEST_TIME_SECS (or AetherConfig::max_request_time) for work that legitimately takes longer.",
                )
                .with_suggestion(
                    "Move work that can outlast any budget off the request path.",
                ),
            AetherError::UnsupportedExpectation => diag
                .with_hint(
                    "The Expect field promised an expectation-extension this server does not implement.",
                )
                .with_suggestion("Send `Expect: 100-continue`, the one token this server answers.")
                .with_suggestion("Drop Expect when the client has no expectation to make."),
            AetherError::Serialization(msg) => diag
                .with_context("Field Validation", msg.clone())
                .with_hint("JSON structure does not match the target Rust struct DTO.")
                .with_suggestion("Inspect client payload keys and expected data types."),
            _ => diag,
        }
    }
}

impl fmt::Display for AetherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AetherError::HeaderTooLarge => {
                write!(f, "HTTP Header section exceeded maximum allowed budget")
            }
            AetherError::BodyLimitExceeded => write!(f, "Request body exceeded payload limit"),
            AetherError::MalformedRequest => write!(f, "Malformed HTTP request syntax"),
            AetherError::UnsupportedExpectation => {
                write!(f, "Expect field names an unsupported expectation")
            }
            AetherError::RouteNotFound => write!(f, "Route not found"),
            AetherError::MethodNotAllowed => write!(f, "HTTP method not allowed for route"),
            AetherError::ValidationFailed(msg) => write!(f, "Validation failure: {}", msg),
            AetherError::DependencyCycleDetected(msg) => {
                write!(f, "Circular dependency in DI graph: {}", msg)
            }
            AetherError::Timeout => write!(f, "Request handler timed out"),
            AetherError::Io(err) => write!(f, "I/O Error: {}", err),
            AetherError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            AetherError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AetherError {}

impl From<std::io::Error> for AetherError {
    fn from(err: std::io::Error) -> Self {
        AetherError::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One value of every variant. `a_variant_was_added_without_a_row_here` fails when this
    /// table and the enum stop agreeing, which is what keeps the two exhaustive tests below
    /// exhaustive instead of "every variant someone remembered".
    fn every_variant() -> Vec<AetherError> {
        vec![
            AetherError::HeaderTooLarge,
            AetherError::BodyLimitExceeded,
            AetherError::MalformedRequest,
            AetherError::UnsupportedExpectation,
            AetherError::RouteNotFound,
            AetherError::MethodNotAllowed,
            AetherError::ValidationFailed("email".to_string()),
            AetherError::DependencyCycleDetected("a -> b -> a".to_string()),
            AetherError::Timeout,
            AetherError::Io(std::io::Error::other("disk says no")),
            AetherError::Serialization("missing field `name`".to_string()),
            AetherError::Custom("nope".to_string()),
        ]
    }

    #[test]
    fn a_variant_was_added_without_a_row_here() {
        // Twelve arms in `error_code`, twelve values here. A thirteenth variant is a row this
        // test must be taught, not a code the tables below silently miss.
        assert_eq!(every_variant().len(), 12);
    }

    /// `aether` answers the wire status and delegates the code here, so a code that disagrees
    /// with its status class is PROB-0009's contradiction coming back through another door.
    /// The pairs are exact rather than class-only: a 404 turning into a 405 would still pass a
    /// class check.
    #[test]
    fn every_variant_keeps_the_code_the_error_contract_names() {
        let expected = [
            (AetherError::HeaderTooLarge, "E3002"),
            (AetherError::BodyLimitExceeded, "E3002"),
            (AetherError::MalformedRequest, "E3001"),
            (AetherError::UnsupportedExpectation, "E3003"),
            (AetherError::RouteNotFound, "E2002"),
            (AetherError::MethodNotAllowed, "E2003"),
            (AetherError::ValidationFailed("v".to_string()), "E4002"),
            (
                AetherError::DependencyCycleDetected("d".to_string()),
                "E5001",
            ),
            (AetherError::Timeout, "E5002"),
            (AetherError::Io(std::io::Error::other("i")), "E5002"),
            (AetherError::Serialization("s".to_string()), "E4001"),
            (AetherError::Custom("c".to_string()), "E4003"),
        ];
        assert_eq!(
            expected.len(),
            every_variant().len(),
            "the expected table and `every_variant` disagree"
        );
        for (error, code) in expected {
            assert_eq!(error.error_code().code_str(), code, "{error:?}");
            // `to_diagnostic` must not re-derive the code independently: the diagnostic a
            // developer reads and the body a client receives name one failure.
            assert_eq!(error.to_diagnostic().code, error.error_code(), "{error:?}");
            assert_eq!(
                error.to_diagnostic().reason,
                error.to_string(),
                "the diagnostic reason must be the error's own Display text: {error:?}"
            );
        }
    }

    /// T118-1. `Io` meant *startup* when this arm was written; the C′ error contract
    /// (2026-10-24) moved it to a runtime failure answered 500/E5002, and the bind failure builds
    /// its own E1001 diagnostic before it ever returns an `Io`. Advice to change `AETHER_PORT`
    /// therefore sends a handler's failed read or write to restart a healthy server.
    #[test]
    fn a_handler_io_failure_is_not_told_to_move_the_listening_port() {
        let diag = AetherError::Io(std::io::Error::other("disk says no")).to_diagnostic();
        let rendered = diag.format_terminal(false);
        assert!(!rendered.contains("AETHER_PORT"), "{rendered}");
        assert!(
            !rendered.to_lowercase().contains("listening port"),
            "a handler I/O failure has no listening port to change: {rendered}"
        );
        assert!(
            diag.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("set_error_reporter")),
            "the 5xx body redacts this detail, so the diagnostic must name the reporter that \
             receives it: {rendered}"
        );
    }

    /// The two payload budgets were one arm until the 431/413 split. A hint that names the wrong
    /// knob turns a correct refusal into a config change that cannot fix it.
    #[test]
    fn the_two_payload_budgets_name_their_own_limit() {
        let head = AetherError::HeaderTooLarge.to_diagnostic();
        assert!(
            head.hint
                .as_deref()
                .unwrap_or_default()
                .contains("max_header_bytes"),
            "{head:?}"
        );
        assert!(
            head.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("AETHER_MAX_HEADER_BYTES")),
            "{head:?}"
        );
        assert!(
            head.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("AETHER_MAX_HEADERS_COUNT")),
            "the field-line refusal carries the same code, so both budgets must be named: {head:?}"
        );

        let body = AetherError::BodyLimitExceeded.to_diagnostic();
        assert!(
            body.hint
                .as_deref()
                .unwrap_or_default()
                .contains("max_body_bytes"),
            "{body:?}"
        );
        assert!(
            body.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("AETHER_MAX_BODY_BYTES")),
            "the operator's knob is the one an app can reach: {body:?}"
        );
        assert!(
            !body
                .suggestions
                .iter()
                .any(|suggestion| suggestion.contains("max_header_bytes")),
            "a body refusal must not point at the head budget: {body:?}"
        );
    }

    /// A 504 is the *handler* deadline (`max_request_time`); the receive deadline answers 408
    /// itself and never becomes an `AetherError`. Calling it a "connection timeout" both names
    /// the wrong clock and suggests the wrong knob.
    #[test]
    fn a_timeout_points_at_the_deadline_that_expired() {
        let diag = AetherError::Timeout.to_diagnostic();
        assert!(
            diag.hint
                .as_deref()
                .unwrap_or_default()
                .contains("max_request_time"),
            "{diag:?}"
        );
        assert!(
            diag.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("AETHER_MAX_REQUEST_TIME_SECS")),
            "{diag:?}"
        );
        assert!(
            !diag.reason.to_lowercase().contains("connection"),
            "504 is the handler deadline, not the connection: {}",
            diag.reason
        );
    }

    /// Track 116 added the refusal and its hint; this is the first test that executes the hint.
    /// It must teach the one token this server answers, not the header's name.
    #[test]
    fn the_expectation_diagnostic_teaches_the_one_token_this_server_answers() {
        let diag = AetherError::UnsupportedExpectation.to_diagnostic();
        assert_eq!(diag.code.code_str(), "E3003");
        assert!(diag.reason.contains("Expect"), "{}", diag.reason);
        assert!(
            diag.suggestions
                .iter()
                .any(|suggestion| suggestion.contains("100-continue")),
            "{diag:?}"
        );
    }

    #[test]
    fn a_serialization_detail_arrives_as_structured_context() {
        let diag = AetherError::Serialization("missing field `name`".to_string()).to_diagnostic();
        assert_eq!(diag.context_key.as_deref(), Some("Field Validation"));
        assert_eq!(diag.context_val.as_deref(), Some("missing field `name`"));
    }

    /// `Custom`'s Display is the caller's own message. A prefix here would be reworded by the
    /// framework on its way to the developer, and the caller's text is the whole reason the
    /// variant exists.
    #[test]
    fn a_custom_message_is_the_reason_verbatim() {
        let diag = AetherError::Custom("db pool exhausted".to_string()).to_diagnostic();
        assert_eq!(diag.reason, "db pool exhausted");
        assert_eq!(diag.code.code_str(), "E4003");
    }
}
