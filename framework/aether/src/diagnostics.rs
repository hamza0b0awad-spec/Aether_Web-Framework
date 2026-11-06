//! Where a 5xx detail goes when the response body redacts it.
//!
//! A handler error that answers 5xx deliberately keeps its message off the wire: the message can
//! name a table, a path or a port. Dropping it as well would leave an operator with a status and
//! no cause, so the conversion in [`IntoResponse`](crate::IntoResponse) hands it to the reporter
//! installed here — if one is installed.
//!
//! The hook is consulted only when a 5xx redacts a detail, so an installed reporter costs nothing
//! on the success path. Nothing is reported by default: a log line that can contain internal
//! detail is an operator's decision, not a framework default.
//!
//! [`ErrorReport::request_id`] is the same value the response carries as `X-Request-ID`, so a
//! report can be tied to the request that produced it. It is `None` when `into_response` ran
//! outside a request (a unit test converting a `Result` directly, for example).

use std::fmt;
use std::future::Future;
use std::sync::OnceLock;

/// A 5xx error detail the response body did not carry.
#[derive(Debug, Clone, Copy)]
pub struct ErrorReport<'a> {
    /// `X-Request-ID` of the request the error came from, when it came from one.
    pub request_id: Option<&'a str>,
    /// The status the response carries.
    pub status: u16,
    /// The `code` field of the response body.
    pub code: &'static str,
    /// The message the body redacted.
    pub detail: &'a str,
}

/// Returned by [`set_error_reporter`] when a reporter is already installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReporterAlreadySet;

impl fmt::Display for ReporterAlreadySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an error reporter is already installed")
    }
}

impl std::error::Error for ReporterAlreadySet {}

type Reporter = Box<dyn Fn(&ErrorReport<'_>) + Send + Sync + 'static>;

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Installs the process-wide reporter for redacted 5xx details.
///
/// Process-wide rather than per-app because the conversion that redacts them
/// (`IntoResponse::into_response`) has no app handle in scope. One process holds one reporter;
/// a second call returns [`ReporterAlreadySet`].
///
/// ```
/// aether::diagnostics::set_error_reporter(|report| {
///     eprintln!(
///         "request {} answered {} {}: {}",
///         report.request_id.unwrap_or("-"),
///         report.status,
///         report.code,
///         report.detail,
///     );
/// })
/// .ok();
/// ```
pub fn set_error_reporter<F>(reporter: F) -> Result<(), ReporterAlreadySet>
where
    F: Fn(&ErrorReport<'_>) + Send + Sync + 'static,
{
    REPORTER
        .set(Box::new(reporter))
        .map_err(|_| ReporterAlreadySet)
}

tokio::task_local! {
    /// The numeric half of the request id, scoped to the task that runs one handler.
    ///
    /// The string is not stored: a request path that copies it would put an allocation on the
    /// hot path to serve a diagnostic that only fires on failures. The number formats back to
    /// the same id on the error path.
    static REQUEST_SERIAL: u64;
}

/// Run `fut` with `serial` visible to a redacted-error report. The scope follows the task, so
/// it survives the handler's awaits and any thread the runtime moves it to.
pub(crate) async fn within_request<F: Future>(serial: u64, fut: F) -> F::Output {
    REQUEST_SERIAL.scope(serial, fut).await
}

pub(crate) fn report_redacted_error(status: u16, code: &'static str, detail: &str) {
    let Some(reporter) = REPORTER.get() else {
        return;
    };
    match REQUEST_SERIAL.try_with(|serial| *serial) {
        Ok(serial) => {
            let request_id = format!("req_{serial:012x}");
            reporter(&ErrorReport {
                request_id: Some(&request_id),
                status,
                code,
                detail,
            });
        }
        Err(_) => reporter(&ErrorReport {
            request_id: None,
            status,
            code,
            detail,
        }),
    }
}
