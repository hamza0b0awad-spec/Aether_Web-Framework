//! # Aether HTTP Core Crate

pub mod header;
pub mod limits;
pub mod method;
pub mod multipart;
pub mod parser;
pub mod response;
pub mod static_files;
pub mod url;

pub use header::{HeaderMap, HeaderView, standard_headers};
pub use limits::ServerLimits;
pub use method::Method;
pub use multipart::{MultipartError, MultipartForm, Part};
pub use parser::{RequestView, parse_request};
pub use response::{
    BodyFraming, BodySource, BodyStream, Chunk, Response, canonical_reason, encode_chunk,
    encode_last_chunk, format_http_date, http_date_now, status_forbids_body,
};
pub use static_files::{
    FilePlan, RangeDecision, file_etag, generate_etag, if_range_date_matches, if_range_matches,
    last_modified, mime_from_extension, parse_range_header, plan_file, serve_static,
    serve_static_with_range,
};
pub use url::{form_decode, percent_decode, percent_decode_cow};
