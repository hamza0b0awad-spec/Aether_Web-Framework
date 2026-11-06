//! Tests for the typed extractor system (`Path`, `Query`, `Json`, `Form`, `State`).

use aether::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Deserialize, PartialEq)]
struct UserPath {
    id: u32,
    tab: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct PageQuery {
    page: u32,
    filter: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct CreateUser {
    name: String,
    age: u8,
}

#[derive(Debug, Deserialize, PartialEq)]
struct LoginForm {
    username: String,
    remember: Option<bool>,
}

struct Hits(Arc<AtomicUsize>);

fn extractor_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.manage(Hits(Arc::new(AtomicUsize::new(0))));

    app = app.get_async("/users/{id}", |Path(id): Path<u32>| async move {
        format!("user {id}")
    });
    app = app.get_async("/named/{name}", |Path(name): Path<String>| async move {
        format!("hello {name}")
    });
    app = app.get_async(
        "/users/{id}/tabs/{tab}",
        |Path(p): Path<UserPath>| async move { format!("{}@{}", p.id, p.tab) },
    );
    app = app.get_async("/items/{id}", |req: RequestContext| async move {
        format!("get-id:{}", req.param("id").unwrap_or("?"))
    });
    app = app.post_async("/items/{item}", |req: RequestContext| async move {
        let item = req.param("item").unwrap_or("?").to_owned();
        let leaked = if req.param("id").is_some() {
            "leaked"
        } else {
            "clean"
        };
        format!("post-item:{item}:{leaked}")
    });
    app = app.get_async("/search", |Query(q): Query<PageQuery>| async move {
        let filter = q.filter.unwrap_or_else(|| "none".to_string());
        format!("page={} filter={filter}", q.page)
    });
    app = app.post_async("/json", |Json(body): Json<CreateUser>| async move {
        format!("{} is {}", body.name, body.age)
    });
    app = app.post_async("/form", |Form(f): Form<LoginForm>| async move {
        let remember = f.remember.unwrap_or(false);
        format!("{} remember={remember}", f.username)
    });
    app = app.get_async("/hits", |State(h): State<Hits>| async move {
        let n = h.0.fetch_add(1, Ordering::SeqCst);
        format!("hit {n}")
    });
    app = app.get_async(
        "/multi/{id}",
        |Path(id): Path<u32>, Query(q): Query<PageQuery>| async move { format!("{id}:{}", q.page) },
    );
    app = app.get_async("/ctx", |req: RequestContext| async move {
        req.header("x-trace").unwrap_or("no-trace").to_string()
    });
    app = app.get_async("/zero", || async { "zero args" });
    app
}

#[tokio::test]
async fn path_primitive() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/42").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "user 42");
    client.shutdown().await;
}

#[tokio::test]
async fn path_string() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/named/sara%20ahmad").send().await;
    assert_eq!(res.text(), "hello sara ahmad");
    client.shutdown().await;
}

#[tokio::test]
async fn path_struct_with_multiple_params() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/7/tabs/inbox").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "7@inbox");
    client.shutdown().await;
}

/// `/items/{id}` (GET) and `/items/{item}` (POST) share one radix level, so they must not
/// shadow each other's parameter name. Extraction used to see only the first-registered name.
#[tokio::test]
async fn cross_name_params_extract_under_each_methods_name() {
    let client = TestClient::start(extractor_app()).await;

    let res = client.get("/items/7").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "get-id:7");

    let res = client.post("/items/shirt").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(
        res.text(),
        "post-item:shirt:clean",
        "POST must not see the GET-only param name"
    );
    client.shutdown().await;
}

/// Two *sibling* routes on one method can share one radix `Param` level while naming the same
/// captured segment differently (`/users/{user}/edit` and `/users/{id}`), so the shared node
/// historically answered the first-registered name for both handlers. Restamping must reach the
/// handler-visible key through the `serve.rs` param copy, not just the router's own return.
#[tokio::test]
async fn sibling_same_method_param_routes_keep_their_own_capture_names() {
    let mut app = AetherApp::new();
    app = app.get_async("/users/{user}/edit", |req: RequestContext| async move {
        format!(
            "edit user={} id={:?}",
            req.param("user").unwrap_or("?"),
            req.param("id")
        )
    });
    app = app.get_async("/users/{id}", |req: RequestContext| async move {
        format!(
            "view id={} user={:?}",
            req.param("id").unwrap_or("?"),
            req.param("user")
        )
    });

    let client = TestClient::start(app).await;

    let res = client.get("/users/42").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(
        res.text(),
        "view id=42 user=None",
        "leaf route must see 'id', not 'user'"
    );

    let res = client.get("/users/42/edit").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(
        res.text(),
        "edit user=42 id=None",
        "sibling route must see 'user', not 'id'"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn path_rejects_non_numeric_primitive() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/notanumber").send().await;
    assert_eq!(res.status, 400);
    let body: Value = res.json().expect("json error body");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("invalid path parameters")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn query_struct_with_optional_field() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/search?page=3").send().await;
    assert_eq!(res.text(), "page=3 filter=none");
    let res = client
        .get("/search?page=3&filter=active%20users")
        .send()
        .await;
    assert_eq!(res.text(), "page=3 filter=active users");
    client.shutdown().await;
}

#[tokio::test]
async fn query_rejects_missing_required_field() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/search?filter=x").send().await;
    assert_eq!(res.status, 400);
    client.shutdown().await;
}

#[tokio::test]
async fn json_body_roundtrip() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .json(&json!({"name": "Zaid", "age": 30}))
        .expect("serialize")
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "Zaid is 30");
    client.shutdown().await;
}

#[tokio::test]
async fn json_rejects_wrong_content_type_with_415() {
    let client = TestClient::start(extractor_app()).await;
    for content_type in [
        "text/plain",
        // A JSON *sequence* is a framed stream of texts, not one JSON document; accepting it
        // here would hand `serde_json` a body it parses differently from the type it names.
        "application/json-seq",
        // `json` inside a parameter or another subtype is not the media type.
        "text/plain; format=json",
        // The structured suffix is the last five characters of the subtype. A type that merely
        // contains `+json` earlier, or ends in bare `json`, is not a JSON document type.
        "application/vnd.foo+jsonx",
        "application/x-json",
    ] {
        let res = client
            .post("/json")
            .header("Content-Type", content_type)
            .body("name=Zaid")
            .send()
            .await;
        assert_eq!(
            res.status, 415,
            "{content_type:?} is not a JSON document type"
        );
    }
    client.shutdown().await;
}

#[tokio::test]
async fn json_media_type_is_case_insensitive_on_both_sides() {
    // RFC 9110 §8.3.1 makes a media type case-insensitive, and the extractor's own doc says a
    // `+json` media type is accepted. `application/vnd.api+JSON` is therefore the same type as
    // `application/vnd.api+json`: refusing it answers a legal request with 415.
    let client = TestClient::start(extractor_app()).await;
    for content_type in [
        "application/json",
        "APPLICATION/JSON",
        "application/vnd.api+json",
        "application/vnd.api+JSON",
        "application/vnd.api+Json",
        "Application/Vnd.Api+JSON; charset=utf-8",
        // Optional whitespace before the parameter delimiter is the sender's formatting, not
        // part of the media type.
        "application/vnd.api+json ; charset=utf-8",
    ] {
        let res = client
            .post("/json")
            .header("Content-Type", content_type)
            .body(r#"{"name":"Zaid","age":30}"#)
            .send()
            .await;
        assert_eq!(res.status, 200, "{content_type:?} is a JSON media type");
        assert_eq!(res.text(), "Zaid is 30");
    }
    client.shutdown().await;
}

#[tokio::test]
async fn json_without_a_content_type_is_parsed_leniently() {
    // `Json`'s own type doc states the media-type check runs only when the field is present, so
    // a request from a client that omits it is still parsed rather than answered 415.
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .body(r#"{"name":"Zaid","age":30}"#)
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "Zaid is 30");
    client.shutdown().await;
}

#[tokio::test]
async fn json_rejects_malformed_body_with_400() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .header("Content-Type", "application/json")
        .body("{not json")
        .send()
        .await;
    assert_eq!(res.status, 400);
    let body: Value = res.json().expect("json error body");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("invalid JSON body")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn form_body_with_optional_flag() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/form")
        .form(&[("username", "omar"), ("remember", "true")])
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "omar remember=true");
    client.shutdown().await;
}

#[tokio::test]
async fn state_extractor_reads_managed_state() {
    let client = TestClient::start(extractor_app()).await;
    let first = client.get("/hits").send().await;
    let second = client.get("/hits").send().await;
    assert_eq!(first.text(), "hit 0");
    assert_eq!(second.text(), "hit 1");
    client.shutdown().await;
}

#[tokio::test]
async fn multi_extractor_handler() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/multi/9?page=5").send().await;
    assert_eq!(res.text(), "9:5");
    client.shutdown().await;
}

#[tokio::test]
async fn request_context_extractor_still_passes_by_move() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/ctx").header("X-Trace", "abc123").send().await;
    assert_eq!(res.text(), "abc123");
    let res = client.get("/ctx").send().await;
    assert_eq!(res.text(), "no-trace");
    client.shutdown().await;
}

#[tokio::test]
async fn zero_argument_handler() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/zero").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "zero args");
    client.shutdown().await;
}

#[tokio::test]
async fn extractors_work_in_route_groups() {
    let mut app = AetherApp::new();
    app = app.group("/api/v1", |g| {
        g.get_async("/items/{id}", |Path(id): Path<u64>| async move {
            format!("item {id}")
        });
    });
    let client = TestClient::start(app).await;
    let res = client.get("/api/v1/items/99").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "item 99");
    client.shutdown().await;
}

#[tokio::test]
async fn state_extractor_missing_type_is_500() {
    struct NeverManaged;
    let mut app = AetherApp::new();
    app = app.get_async("/oops", |_: State<NeverManaged>| async { "unreachable" });
    let client = TestClient::start(app).await;
    let res = client.get("/oops").send().await;
    assert_eq!(res.status, 500);
    let body: Value = res.json().expect("json error body");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("managed state not found")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn extraction_failure_short_circuits_handler() {
    static CALLED: AtomicUsize = AtomicUsize::new(0);
    let mut app = AetherApp::new();
    app = app.post_async("/guarded", |Json(_): Json<CreateUser>| async {
        CALLED.fetch_add(1, Ordering::SeqCst);
        "ran"
    });
    let client = TestClient::start(app).await;
    let res = client
        .post("/guarded")
        .header("Content-Type", "application/json")
        .body("[]")
        .send()
        .await;
    assert_eq!(res.status, 400);
    assert_eq!(
        CALLED.load(Ordering::SeqCst),
        0,
        "handler must not run on rejection"
    );
    client.shutdown().await;
}
