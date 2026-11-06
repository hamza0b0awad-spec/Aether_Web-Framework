//! The OpenAPI document is derived from the route table, not written by hand.
//!
//! `AetherApp::openapi_spec` reads the router's own listing, so every claim here is about the two
//! halves meeting: the enumeration has to report each registration under the name *that* route
//! declared — not the one that happened to create the shared level — and the generator has to turn
//! that into a document whose keys, verbs and parameters say what the server actually answers.
//! Groups and mounts are in scope because their routes are only reachable through the app's one
//! table, and a document that missed them would be describing a server that does not exist.

use aether::openapi::OpenApiSpec;
use aether::prelude::*;

/// Every operation the document answers, as `VERB /path` in the order the document holds them:
/// paths by template, verbs by name. A missing route, an extra one, and a mount that failed to
/// prefix all show up as a diff here rather than as an absent key someone forgets to check.
fn operations(spec: &OpenApiSpec) -> Vec<String> {
    let mut listed = Vec::new();
    for (path, item) in &spec.paths {
        for verb in item.methods.keys() {
            listed.push(format!("{} {}", verb.to_uppercase(), path));
        }
    }
    listed
}

fn parameter_names(spec: &OpenApiSpec, path: &str, verb: &str) -> Vec<String> {
    let item = spec
        .paths
        .get(path)
        .unwrap_or_else(|| panic!("{path} is not in the document: {:?}", operations(spec)));
    let operation = item
        .methods
        .get(verb)
        .unwrap_or_else(|| panic!("{verb} {path} is not in the document"));
    operation
        .parameters
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

fn status_codes(spec: &OpenApiSpec, path: &str, verb: &str) -> Vec<String> {
    let item = spec
        .paths
        .get(path)
        .unwrap_or_else(|| panic!("{path} is not in the document"));
    item.methods
        .get(verb)
        .unwrap_or_else(|| panic!("{verb} {path} is not in the document"))
        .responses
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_document_covers_every_route_the_app_registered() {
    let mut app = AetherApp::new();
    app = app.get("/", |_| "root");
    app = app.get("/users", |_| "list");
    app = app.post("/users", |_| "created");
    app = app.get("/users/{id}", |_| "one");
    app = app.put("/users/{id}", |_| "replaced");
    app = app.delete("/users/{id}", |_| "gone");
    app = app.patch("/users/{id}/profile", |_| "patched");

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(
        operations(&spec),
        vec![
            "GET /",
            "GET /users",
            "POST /users",
            "DELETE /users/{id}",
            "GET /users/{id}",
            "PUT /users/{id}",
            "PATCH /users/{id}/profile",
        ]
    );
    assert_eq!(
        parameter_names(&spec, "/users/{id}", "get"),
        vec!["id".to_string()]
    );
    assert_eq!(
        parameter_names(&spec, "/users", "post"),
        Vec::<String>::new()
    );
}

/// The defect this whole track starts from: one `Param` node serves `/files/{name}/meta` and
/// `/files/{id}`, and the node remembers only the first name. A document that echoed the node
/// would declare `name` for the upload route, and a client following it would build a URL whose
/// capture the server calls `name` — the same restamp the matcher performs at request time has to
/// be the one the listing performs at build time.
#[test]
fn a_shared_dynamic_level_is_named_per_route() {
    let mut app = AetherApp::new();
    app = app.get("/files/{name}/meta", |_| "meta");
    app = app.post("/files/{id}", |_| "uploaded");

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(
        parameter_names(&spec, "/files/{name}/meta", "get"),
        vec!["name".to_string()]
    );
    assert_eq!(
        parameter_names(&spec, "/files/{id}", "post"),
        vec!["id".to_string()]
    );
}

/// A group and a mounted sub-app both contribute to the application's table, and the prefix is
/// what they contribute: the document has to show the address the server answers, not the path
/// the sub-app was written with.
#[test]
fn a_group_and_a_mount_contribute_their_prefixed_routes() {
    let mut sub = Router::new();
    sub.get("/health", |_| "healthy");
    sub.get("/orders/{oid}", |_| "one order");

    let mut app = AetherApp::new();
    app = app.group("/v1", |g| {
        g.get("/status", |_| "ok");
        g.post("/items/{sku}", |_| "an item");
    });
    app = app.mount("/api", sub);
    app = app.get("/self", |_| "self");

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(
        operations(&spec),
        vec![
            "GET /api/health",
            "GET /api/orders/{oid}",
            "GET /self",
            "POST /v1/items/{sku}",
            "GET /v1/status",
        ]
    );
    assert!(
        !spec.paths.contains_key("/health"),
        "a mounted router's own path must not also be documented"
    );
    assert_eq!(
        parameter_names(&spec, "/api/orders/{oid}", "get"),
        vec!["oid".to_string()]
    );
}

/// `static_dir` registers `/assets/*filepath`; OpenAPI spells that slot with braces, so the
/// document is where the two spellings meet. One catch-all in the table, one `{filepath}`
/// parameter in the document, and the file server itself is not described beyond that.
#[test]
fn a_static_dir_is_listed_as_the_catch_all_it_registers() {
    let mut app = AetherApp::new();
    app = app.static_dir("/assets", std::env::temp_dir());

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(operations(&spec), vec!["GET /assets/{filepath}"]);
    assert_eq!(
        parameter_names(&spec, "/assets/{filepath}", "get"),
        vec!["filepath".to_string()]
    );
}

/// A WebSocket registration is a `GET` route that answers a handshake, and the document has to say
/// so: `101` for the upgrade the framework performs plus the two refusals it performs before any
/// handler runs, and no `200`, which would promise an HTTP response the route never gives.
#[test]
fn a_websocket_route_carries_the_upgrade_instead_of_a_twenty() {
    let mut app = AetherApp::new();
    app = app.get("/status", |_| "ok");
    app = app.ws("/live", |_req, _socket| async move {});

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(
        status_codes(&spec, "/live", "get"),
        vec!["101", "400", "426"]
    );
    assert_eq!(status_codes(&spec, "/status", "get"), vec!["200"]);
    assert_eq!(operations(&spec), vec!["GET /live", "GET /status"]);
}

/// `HEAD` comes from a `GET` route and `OPTIONS` comes from the serving core, so neither is a
/// registration and neither may be claimed as one. The document says what was registered; a
/// consumer that needs the rest reads `Allow`.
#[test]
fn the_document_omits_the_answers_that_have_no_registration() {
    let mut app = AetherApp::new();
    app = app.get("/health", |_| "healthy");
    app = app.post("/health", |_| "probed");

    let spec = app.openapi_spec("Aether", "1.0.0");
    assert_eq!(operations(&spec), vec!["GET /health", "POST /health"]);
    let json = spec.to_json().unwrap();
    for implied in ["\"head\"", "\"options\"", "\"trace\"", "\"connect\""] {
        assert!(
            !json.contains(implied),
            "{implied} is not a registration, so it is not an operation: {json}"
        );
    }
}

/// The document is a build artifact: writing it out and diffing it is only possible if the same
/// table always yields the same bytes. The order routes are written in is exactly what a
/// refactoring changes, so two apps holding the same routes — registered in opposite orders and,
/// for one of them, through a group — must serialise identically.
#[test]
fn the_document_is_the_same_whatever_order_the_routes_were_registered_in() {
    let mut forwards = AetherApp::new();
    forwards = forwards.get("/a/{x}/b", |_| "one");
    forwards = forwards.post("/a/{y}", |_| "two");
    forwards = forwards.get("/assets/*filepath", |_| "three");

    let mut backwards = AetherApp::new();
    backwards = backwards.get("/assets/*filepath", |_| "three");
    backwards = backwards.group("", |g| {
        g.post("/a/{y}", |_| "two");
        g.get("/a/{x}/b", |_| "one");
    });

    let first = forwards.openapi_spec("Aether", "1.0.0").to_json().unwrap();
    let second = backwards.openapi_spec("Aether", "1.0.0").to_json().unwrap();
    assert_eq!(first, second);
}

/// The last mile is the JSON a consumer reads, not the Rust struct: the spec's key names, the
/// verb-keyed path item, and a parameter that claims nothing beyond the text a path slot is.
#[test]
fn the_derived_json_carries_the_specs_keys() {
    let mut app = AetherApp::new();
    app = app.get("/users/{id}", |_| "one");

    let spec = app.openapi_spec("Directory", "2.1.0");
    let json = spec.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["openapi"], serde_json::json!("3.1.0"));
    assert_eq!(value["info"]["title"], serde_json::json!("Directory"));
    assert_eq!(value["info"]["version"], serde_json::json!("2.1.0"));
    assert_eq!(
        value["paths"]["/users/{id}"]["get"]["operationId"],
        serde_json::json!("get_users_id")
    );
    assert_eq!(
        value["paths"]["/users/{id}"]["get"]["parameters"],
        serde_json::json!([
            {
                "name": "id",
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
            }
        ])
    );
    assert!(
        value["paths"]["/users/{id}"]["get"]["summary"]
            .as_str()
            .expect("a summary")
            .contains("GET /users/{id}"),
        "{json}"
    );
}
