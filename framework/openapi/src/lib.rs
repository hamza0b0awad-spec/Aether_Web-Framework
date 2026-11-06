//! # Aether OpenAPI v3.1 Spec Generator
//!
//! Builds an OpenAPI 3.1 document. [`OpenApiSpec::from_routes`] derives one from the routes a
//! server registered; `new` plus `paths` is for a document that needs prose the route table does
//! not hold.
//!
//! What a derived document claims, and what it refuses to:
//! - one operation per registration, keyed by the lowercase method name, on the template the
//!   matcher uses — `/assets/*filepath` is written `/assets/{filepath}` with a `filepath` path
//!   parameter, because a brace slot is how OpenAPI spells one;
//! - `200` for an HTTP route, and `101`/`400`/`426` for a WebSocket one. Those are the statuses
//!   the framework itself produces for such a registration. A guard's refusal, a middleware
//!   short-circuit or a handler's own error body is not in the route table, so it is not here
//!   either;
//! - no request or response schema, no media type, no body description. A registration is a
//!   method, a path and a handler id; anything more in the document would be a claim that nothing
//!   in the server enforces;
//! - no `HEAD` and no server-wide `OPTIONS`. The serving core answers both without a
//!   registration — a `GET` route serves `HEAD` (RFC 9110 §9.3.2) and `OPTIONS` is answered
//!   generically on any routable path (§9.3.7) — so there is no handler to derive an operation
//!   from. A consumer that needs them reads `Allow`.
//!
//! The input order cannot change the output: the routes are sorted before they are read, so two
//! runs over the same table — however it was assembled, by groups or mounts or both — produce
//! byte-identical JSON, including the `operationId` suffixes that collisions depend on.
//!
//! One shape the document cannot carry: `Method::Custom` variants share the name `CUSTOM`, and
//! OpenAPI keys a path item's operations by method name, so a path with two custom-method
//! registrations keeps one operation for them. The router answers both.
//!
//! The crate depends on serde and on nothing from the runtime: it takes method and path names as
//! `&str`, so describing a server never pulls a server into the build.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Debug, Clone)]
pub struct OpenApiSpec {
    pub openapi: &'static str,
    pub info: Info,
    /// `BTreeMap`, not `HashMap`: a spec is a build artifact that gets diffed and
    /// version-controlled, so the key order in the emitted JSON must be stable across
    /// runs and processes.
    pub paths: BTreeMap<String, PathItem>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Info {
    pub title: String,
    pub version: String,
    pub description: String,
}

/// The operations one path answers, keyed by the lowercase HTTP method name OpenAPI uses
/// (`get`, `post`, `delete`, …).
///
/// A map rather than a field per verb: the verbs a route table holds is not a fixed set, and a
/// struct with one field for each verb it thought of in advance silently drops every registration
/// behind the missing field — which is exactly how a `PUT` route vanishes from a document.
#[derive(Serialize, Debug, Clone, Default)]
pub struct PathItem {
    #[serde(flatten)]
    pub methods: BTreeMap<String, Operation>,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct Operation {
    pub summary: String,
    /// OpenAPI spells this key `operationId`; a snake_case key is not the document it claims to be.
    #[serde(rename = "operationId")]
    pub operation_id: String,
    /// The path slots this operation's template declares. Left out of the JSON when the path is
    /// static, so a document never carries an empty `parameters` array to read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    pub responses: BTreeMap<String, ResponseSpec>,
}

/// A parameter, in the one place a route table can prove one exists: a `{slot}` in the path.
#[derive(Serialize, Debug, Clone)]
pub struct Parameter {
    pub name: String,
    /// OpenAPI 3.1 spells this key `in`, and `in` is a Rust keyword, so the field carries the
    /// spec's name instead of the Rust one.
    #[serde(rename = "in")]
    pub location: ParameterLocation,
    /// Always true for a path slot: a request that omits the segment cannot match the template.
    pub required: bool,
    /// A captured segment is text and nothing more. The router records a name, not a type, so the
    /// document says `string` and stops there.
    pub schema: ParameterSchema,
}

#[derive(Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    Path,
}

#[derive(Serialize, Debug, Clone, Copy)]
pub struct ParameterSchema {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Serialize, Debug, Clone)]
pub struct ResponseSpec {
    pub description: String,
}

/// One registration as the server reports it: the method name, the canonical path template the
/// router matches (`/users/{id}`, `/assets/*filepath`) and whether the handler behind it is a
/// WebSocket upgrade rather than a request/response handler.
#[derive(Debug, Clone, Copy)]
pub struct RouteDeclaration<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub websocket: bool,
}

/// The status set a derived document gives an HTTP operation.
const HTTP_RESPONSES: [(&str, &str); 1] = [(
    "200",
    "Successful response. The route table records no schema, media type or body for a handler, so \
     this names the success case only; the handler answers any other status it returns.",
)];

/// The status set a derived document gives a WebSocket registration: the upgrade the framework
/// performs, and the two refusals it performs before any handler runs.
const WS_RESPONSES: [(&str, &str); 3] = [
    (
        "101",
        "Switching Protocols. The handshake completed and the connection carries WebSocket frames; \
         the document describes the upgrade, not the message payloads.",
    ),
    (
        "400",
        "The upgrade was refused: the handshake was invalid, an offered subprotocol matched none \
         the route declares, or the request arrived over HTTP/2, which has no Upgrade mechanism.",
    ),
    (
        "426",
        "Upgrade Required. The handshake asked for a WebSocket version this server does not \
         implement; the answer carries `Sec-WebSocket-Version: 13`.",
    ),
];

impl OpenApiSpec {
    pub fn new(title: &str, version: &str) -> Self {
        Self {
            openapi: "3.1.0",
            info: Info {
                title: title.to_string(),
                version: version.to_string(),
                description: "Native Aether Ultra-High-Performance REST API".to_string(),
            },
            paths: BTreeMap::new(),
        }
    }

    /// Derive the document from the routes a server registered.
    ///
    /// `routes` is the shape a server can produce without knowing anything about OpenAPI; the app
    /// hands this its router's own listing. Input order is not significant: the slice is sorted by
    /// `(path, method)` before it is read, so the document is a function of the table alone.
    pub fn from_routes(title: &str, version: &str, routes: &[RouteDeclaration<'_>]) -> Self {
        let mut sorted = routes.to_vec();
        sorted.sort_by(|a, b| {
            (a.path, a.method.to_ascii_uppercase()).cmp(&(b.path, b.method.to_ascii_uppercase()))
        });

        let mut spec = Self::new(title, version);
        let mut taken_ids: BTreeSet<String> = BTreeSet::new();
        for route in sorted {
            let template = template_of(route.path);
            let operation = derived_operation(&route, &template, &mut taken_ids);
            spec.paths
                .entry(template)
                .or_default()
                .methods
                .insert(route.method.to_ascii_lowercase(), operation);
        }
        spec
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// The OpenAPI spelling of a router template: a `*name` catch-all is the same slot a `{name}`
/// parameter declares, written with braces, because OpenAPI has no wildcard form of its own.
/// Everything else is carried over as the router reported it.
fn template_of(path: &str) -> String {
    let mut template = String::with_capacity(path.len());
    for segment in path.split('/') {
        // The empty pieces are the separators themselves; the router lists no path with an empty
        // interior segment, and folding one that arrives from a hand-built declaration keeps the
        // template the matcher would answer.
        if segment.is_empty() {
            continue;
        }
        template.push('/');
        match segment.strip_prefix('*') {
            Some(name) => {
                // A lone `*` has no name to put in braces; the router registers that form under
                // its own default, so the template says the same.
                let name = if name.is_empty() { "wildcard" } else { name };
                template.push('{');
                template.push_str(name);
                template.push('}');
            }
            None => template.push_str(segment),
        }
    }
    if template.is_empty() {
        template.push('/');
    }
    template
}

/// The `{names}` a template declares, in path order. Text between slots is skipped, and a
/// malformed slot — `{}`, or a `{` that never closes — declares nothing rather than a parameter
/// with an empty name.
fn slot_names(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        if close > 0 {
            names.push(after[..close].to_string());
        }
        rest = &after[close + 1..];
    }
    names
}

fn derived_operation(
    route: &RouteDeclaration<'_>,
    template: &str,
    taken_ids: &mut BTreeSet<String>,
) -> Operation {
    let responses: &[(&str, &str)] = if route.websocket {
        &WS_RESPONSES
    } else {
        &HTTP_RESPONSES
    };
    Operation {
        summary: format!("{} {}", route.method.to_ascii_uppercase(), template),
        operation_id: unique_operation_id(route.method, template, taken_ids),
        parameters: slot_names(template)
            .into_iter()
            .map(|name| Parameter {
                name,
                location: ParameterLocation::Path,
                required: true,
                schema: ParameterSchema { kind: "string" },
            })
            .collect(),
        responses: responses
            .iter()
            .map(|(code, description)| {
                (
                    code.to_string(),
                    ResponseSpec {
                        description: (*description).to_string(),
                    },
                )
            })
            .collect(),
    }
}

/// `get_users_id` for `GET /users/{id}`: the method, then the template's words in order.
///
/// Two different paths can sanitise to the same words — `/a/b` and `/a_b` both read `get_a_b` —
/// and an `operationId` has to be unique in the document, so the *n*th operation sharing a set of
/// words carries `_n`. That is reproducible from the sorted order; dropping the operation or
/// emitting a duplicate key would not be.
fn unique_operation_id(method: &str, template: &str, taken_ids: &mut BTreeSet<String>) -> String {
    let mut id = method.to_ascii_lowercase();
    for word in template.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if !word.is_empty() {
            id.push('_');
            id.push_str(word);
        }
    }
    let base = id.clone();
    let mut ordinal = 1;
    while !taken_ids.insert(id.clone()) {
        ordinal += 1;
        id = format!("{base}_{ordinal}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl<'a>(method: &'a str, path: &'a str, websocket: bool) -> RouteDeclaration<'a> {
        RouteDeclaration {
            method,
            path,
            websocket,
        }
    }

    #[test]
    fn the_document_uses_the_specs_field_names() {
        let mut spec = OpenApiSpec::new("Aether", "1.0.0");
        spec.paths.insert(
            "/health".to_string(),
            PathItem {
                methods: BTreeMap::from([(
                    "get".to_string(),
                    Operation {
                        summary: "Health check".to_string(),
                        operation_id: "health".to_string(),
                        parameters: Vec::new(),
                        responses: BTreeMap::from([(
                            "200".to_string(),
                            ResponseSpec {
                                description: "healthy".to_string(),
                            },
                        )]),
                    },
                )]),
            },
        );

        let json = spec.to_json().expect("a populated spec serialises");
        assert!(json.contains("\"openapi\": \"3.1.0\""), "{json}");
        assert!(json.contains("\"operationId\": \"health\""), "{json}");
        // The verb is the key OpenAPI reads, not a Rust field name it would ignore.
        assert!(
            json.contains("\"get\": {"),
            "the operation must be keyed by its lowercase method: {json}"
        );
        assert!(
            !json.contains("operation_id"),
            "the Rust field name is not an OpenAPI key: {json}"
        );
        assert!(
            !json.contains("methods"),
            "the verb map is flattened into the path item, not nested under a key: {json}"
        );
    }

    /// `/assets/*filepath` is how the router stores a catch-all and `/assets/{filepath}` is how
    /// OpenAPI spells the same slot; the derivation is where one becomes the other, and the
    /// declared name has to survive the trip as a path parameter.
    #[test]
    fn a_catch_all_is_derived_as_a_path_parameter() {
        let spec = OpenApiSpec::from_routes(
            "Aether",
            "1.0.0",
            &[decl("GET", "/assets/*filepath", false)],
        );
        let item = spec
            .paths
            .get("/assets/{filepath}")
            .expect("the catch-all template is written with a brace slot");
        let get = item.methods.get("get").expect("one get operation");
        assert_eq!(get.parameters.len(), 1);
        assert_eq!(get.parameters[0].name, "filepath");
        assert!(get.parameters[0].required);

        let json = spec.to_json().unwrap();
        assert!(json.contains("\"in\": \"path\""), "{json}");
        assert!(json.contains("\"type\": \"string\""), "{json}");
    }

    /// A document is diffed and committed, so the order the routes were collected in — which
    /// depends on how groups and mounts were written — must not reach the output.
    #[test]
    fn the_derived_document_does_not_depend_on_input_order() {
        let routes = [
            decl("POST", "/users", false),
            decl("GET", "/users/{id}", false),
            decl("DELETE", "/files/*rest", false),
            decl("GET", "/", false),
            // Three paths that sanitise to one `operationId`. Their collision suffixes are handed
            // out in the order the routes are read, so this is what proves the sort happens here
            // and not in whatever order the caller built the table.
            decl("GET", "/a_b", false),
            decl("GET", "/a/b", false),
            decl("GET", "/a~b", false),
        ];
        let forwards = OpenApiSpec::from_routes("Aether", "1.0.0", &routes)
            .to_json()
            .unwrap();
        let backwards: Vec<_> = routes.iter().rev().copied().collect();
        let reversed = OpenApiSpec::from_routes("Aether", "1.0.0", &backwards)
            .to_json()
            .unwrap();
        assert_eq!(forwards, reversed);

        let spec = OpenApiSpec::from_routes("Aether", "1.0.0", &routes);
        let paths: Vec<&str> = spec.paths.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            vec![
                "/",
                "/a/b",
                "/a_b",
                "/a~b",
                "/files/{rest}",
                "/users",
                "/users/{id}"
            ]
        );
        assert_eq!(
            spec.paths["/a/b"].methods["get"].operation_id, "get_a_b",
            "the sorted order, not the caller's, decides which path keeps the plain id"
        );
        assert_eq!(spec.paths["/a~b"].methods["get"].operation_id, "get_a_b_3");
    }

    /// Two paths can sanitise to one `operationId`, and the document may carry neither a duplicate
    /// key nor a dropped operation.
    #[test]
    fn operation_ids_are_unique_when_two_paths_sanitise_alike() {
        let spec = OpenApiSpec::from_routes(
            "Aether",
            "1.0.0",
            &[
                decl("GET", "/a_b", false),
                decl("GET", "/a/b", false),
                decl("GET", "/a~b", false),
            ],
        );
        let mut ids: Vec<(String, String, String)> = Vec::new();
        for (path, item) in &spec.paths {
            for (method, operation) in &item.methods {
                ids.push((path.clone(), method.clone(), operation.operation_id.clone()));
            }
        }
        assert_eq!(ids.len(), 3, "{ids:?}");
        let mut sorted: Vec<&str> = ids.iter().map(|(_, _, id)| id.as_str()).collect();
        sorted.sort_unstable();
        let unique = sorted.len();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            unique,
            "operation ids must be unique: {ids:?}"
        );
        assert_eq!(
            sorted,
            vec!["get_a_b", "get_a_b_2", "get_a_b_3"],
            "the suffix counts collisions in the sorted order the routes arrive in: {ids:?}"
        );
    }

    #[test]
    fn a_websocket_registration_answers_the_upgrade_and_its_refusals() {
        let spec = OpenApiSpec::from_routes(
            "Aether",
            "1.0.0",
            &[decl("GET", "/live", true), decl("GET", "/status", false)],
        );
        let ws = &spec.paths["/live"].methods["get"];
        let codes: Vec<&str> = ws.responses.keys().map(String::as_str).collect();
        assert_eq!(codes, vec!["101", "400", "426"]);
        let http = &spec.paths["/status"].methods["get"];
        assert_eq!(
            http.responses
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["200"],
            "an http route must not claim an upgrade"
        );
    }

    /// The honesty boundary of the whole generator: a route table says nothing about bodies, so
    /// neither may the document built from it.
    #[test]
    fn nothing_in_a_derived_document_claims_a_body_schema() {
        let spec = OpenApiSpec::from_routes(
            "Aether",
            "1.0.0",
            &[
                decl("POST", "/users/{id}/posts", false),
                decl("GET", "/ws/*tail", true),
            ],
        );
        let json = spec.to_json().unwrap();
        assert!(
            json.contains("\"summary\": \"POST /users/{id}/posts\""),
            "{json}"
        );
        let posts = &spec.paths["/users/{id}/posts"].methods["post"];
        assert_eq!(
            posts
                .parameters
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id"]
        );

        // Read the document the way a consumer does: no key in it may promise a media type, a
        // request body, or a parameter type the router could not have declared.
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let paths = value["paths"].as_object().expect("paths is an object");
        assert_eq!(paths.len(), 2);
        for (path, item) in paths {
            for (method, operation) in item.as_object().expect("a path item is an object") {
                let at = format!("{method} {path}");
                assert!(
                    operation.get("requestBody").is_none(),
                    "{at}: the table holds no request body"
                );
                for (code, response) in operation["responses"]
                    .as_object()
                    .expect("responses is an object")
                {
                    assert!(
                        response.get("content").is_none(),
                        "{at} {code}: the table holds no media type"
                    );
                }
                // Every parameter the document does name comes from a path slot and nothing else:
                // required, located in the path, typed as the text segment it captured.
                for parameter in operation
                    .get("parameters")
                    .map(|p| p.as_array().expect("parameters is an array"))
                    .into_iter()
                    .flatten()
                {
                    assert_eq!(parameter["in"], serde_json::json!("path"), "{at}");
                    assert_eq!(parameter["required"], serde_json::json!(true), "{at}");
                    assert_eq!(
                        parameter["schema"],
                        serde_json::json!({ "type": "string" }),
                        "{at}: the router records a name, not a type"
                    );
                }
            }
        }
        assert_eq!(value["openapi"], serde_json::json!("3.1.0"));
    }

    /// A malformed template declares no slot; it must not produce a parameter with an empty name.
    #[test]
    fn a_malformed_slot_declares_no_parameter() {
        assert_eq!(slot_names("/a/{}/b"), Vec::<String>::new());
        assert_eq!(slot_names("/a/{unclosed"), Vec::<String>::new());
        assert_eq!(slot_names("/a/}b/{id}"), vec!["id".to_string()]);
        assert_eq!(template_of("/a/{}"), "/a/{}");
        assert_eq!(template_of("/"), "/");
        assert_eq!(template_of("*"), "/{wildcard}");
    }
}
