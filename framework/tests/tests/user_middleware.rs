//! Tests for user-defined and group-scoped middleware.

use aether::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn middleware_modifies_response() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Processed", "yes".to_string()));
        res
    });
    app = app.get("/", |_| "ok");

    let client = TestClient::start(app).await;
    let res = client.get("/").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.header("x-processed").unwrap(), "yes");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_runs_in_registration_order() {
    let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let mut app = AetherApp::new();
    app = app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("mw1-in");
                let res = next(req).await;
                log.lock().unwrap().push("mw1-out");
                res
            }
        }
    });
    app = app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("mw2-in");
                let res = next(req).await;
                log.lock().unwrap().push("mw2-out");
                res
            }
        }
    });
    app = app.get("/", |_| "ok");

    let client = TestClient::start(app).await;
    client.get("/").send().await;
    client.shutdown().await;

    let seen: Vec<&'static str> = log.lock().unwrap().clone();
    assert_eq!(seen, ["mw1-in", "mw2-in", "mw2-out", "mw1-out"]);
}

#[tokio::test]
async fn middleware_can_short_circuit() {
    static HANDLER_HITS: AtomicUsize = AtomicUsize::new(0);
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("x-block").is_some() {
            return Response::status(403, "Forbidden");
        }
        next(req).await
    });
    app = app.get("/", |_| {
        HANDLER_HITS.fetch_add(1, Ordering::SeqCst);
        "ok"
    });

    let client = TestClient::start(app).await;
    let blocked = client.get("/").header("X-Block", "1").send().await;
    assert_eq!(blocked.status, 403);
    let allowed = client.get("/").send().await;
    assert_eq!(allowed.status, 200);
    assert_eq!(HANDLER_HITS.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_applies_only_to_routes_registered_after() {
    let mut app = AetherApp::new();
    app = app.get("/before", |_| "untouched");
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Mw", "1".to_string()));
        res
    });
    app = app.get("/after", |_| "touched");

    let client = TestClient::start(app).await;
    let before = client.get("/before").send().await;
    let after = client.get("/after").send().await;
    assert!(before.header("x-mw").is_none());
    assert_eq!(after.header("x-mw").unwrap(), "1");
    client.shutdown().await;
}

#[tokio::test]
async fn group_middleware_scopes_to_group() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-App", "1".to_string()));
        res
    });
    app = app.group("/admin", |g| {
        g.use_middleware(|req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Admin", "1".to_string()));
            res
        });
        g.get("/panel", |_| "admin panel");
        g.get_async("/stats", |req: RequestContext| async move {
            let _ = req;
            "stats"
        });
    });
    app = app.get("/public", |_| "public");

    let client = TestClient::start(app).await;
    let admin = client.get("/admin/panel").send().await;
    assert_eq!(admin.header("x-app").unwrap(), "1");
    assert_eq!(admin.header("x-admin").unwrap(), "1");

    let public = client.get("/public").send().await;
    assert_eq!(public.header("x-app").unwrap(), "1");
    assert!(public.header("x-admin").is_none());

    let admin_async = client.get("/admin/stats").send().await;
    assert_eq!(admin_async.header("x-admin").unwrap(), "1");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_observes_and_can_modify_request() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        req.request_id.clear();
        req.request_id.push_str("injected");
        next(req).await
    });
    app = app.get("/", |req: RequestContext| req.request_id().to_string());

    let client = TestClient::start(app).await;
    let res = client.get("/").send().await;
    assert_eq!(res.text(), "injected");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_error_does_not_break_server() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.path == "/fail" {
            panic!("middleware exploded");
        }
        next(req).await
    });
    app = app.get("/fail", |_| "never reached");
    app = app.get("/ok", |_| "still alive");

    let client = TestClient::start(app).await;
    let failed = client.get("/fail").send().await;
    assert_eq!(failed.status, 500);
    let ok = client.get("/ok").send().await;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.text(), "still alive");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_composes_with_extractors() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Mw", "on".to_string()));
        res
    });
    app = app.get_async("/users/{id}", |Path(id): Path<u32>| async move {
        format!("user {id}")
    });

    let client = TestClient::start(app).await;
    let res = client.get("/users/5").send().await;
    assert_eq!(res.text(), "user 5");
    assert_eq!(res.header("x-mw").unwrap(), "on");
    client.shutdown().await;
}

/// The chain a `route_*_with` call binds belongs to the registration, not to the path or the
/// app: the same path under another method and every other route keep the app-wide stack only.
#[tokio::test]
async fn per_route_middleware_covers_one_registration_only() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-App", "1".to_string()));
        res
    });
    app = app.route_sync_with(
        Method::Get,
        "/stats",
        |req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Route", "get".to_string()));
            res
        },
        |_| "stats",
    );
    app = app.route_sync(Method::Post, "/stats", |_| "posted");
    app = app.get("/elsewhere", |_| "elsewhere");

    let client = TestClient::start(app).await;
    let guarded = client.get("/stats").send().await;
    assert_eq!(guarded.status, 200);
    assert_eq!(guarded.header("x-app").unwrap(), "1");
    assert_eq!(guarded.header("x-route").unwrap(), "get");

    let sibling_method = client.post("/stats").send().await;
    assert_eq!(sibling_method.text(), "posted");
    assert_eq!(sibling_method.header("x-app").unwrap(), "1");
    assert!(
        sibling_method.header("x-route").is_none(),
        "the same path under another method must not inherit the GET registration's chain"
    );

    let sibling_path = client.get("/elsewhere").send().await;
    assert!(sibling_path.header("x-route").is_none());
    client.shutdown().await;
}

/// Composition order is the documented one: app-wide first, the route's own last, unwinding in
/// reverse — the same order a group's stack takes.
#[tokio::test]
async fn per_route_middleware_runs_inside_the_app_stack() {
    let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let mut app = AetherApp::new();
    app = app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("app-in");
                let res = next(req).await;
                log.lock().unwrap().push("app-out");
                res
            }
        }
    });
    app = app.route_sync_with(
        Method::Get,
        "/",
        {
            let log = Arc::clone(&log);
            move |req: RequestContext, next: Next| {
                let log = Arc::clone(&log);
                async move {
                    log.lock().unwrap().push("route-in");
                    let res = next(req).await;
                    log.lock().unwrap().push("route-out");
                    res
                }
            }
        },
        |_| "ok",
    );

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/").send().await.status, 200);
    client.shutdown().await;

    let seen: Vec<&'static str> = log.lock().unwrap().clone();
    assert_eq!(seen, ["app-in", "route-in", "route-out", "app-out"]);
}

/// A per-route guard is a middleware that answers without calling `next`. The wire has to show
/// the refusal *and* that the handler never ran, which the counter is for.
#[tokio::test]
async fn per_route_guard_refuses_and_the_handler_never_runs() {
    static STATS_HITS: AtomicUsize = AtomicUsize::new(0);
    let mut app = AetherApp::new();
    app = app.route_sync_with(
        Method::Get,
        "/stats",
        |req: RequestContext, next: Next| async move {
            if req.header("x-api-key") == Some("secret") {
                next(req).await
            } else {
                Response::status(401, "Unauthorized")
            }
        },
        |_| {
            STATS_HITS.fetch_add(1, Ordering::SeqCst);
            "stats"
        },
    );
    app = app.get("/public", |_| "public");

    let client = TestClient::start(app).await;
    let refused = client.get("/stats").send().await;
    assert_eq!(refused.status, 401);
    assert_eq!(STATS_HITS.load(Ordering::SeqCst), 0);

    let allowed = client
        .get("/stats")
        .header("X-Api-Key", "secret")
        .send()
        .await;
    assert_eq!(allowed.status, 200);
    assert_eq!(allowed.text(), "stats");
    assert_eq!(STATS_HITS.load(Ordering::SeqCst), 1);

    // A guard on one route says nothing about its neighbours.
    assert_eq!(client.get("/public").send().await.status, 200);
    client.shutdown().await;
}

/// The async form must carry the same chain, and must still hand the handler its extractors.
#[tokio::test]
async fn per_route_middleware_composes_with_extractors() {
    let mut app = AetherApp::new();
    app = app.route_async_with(
        Method::Get,
        "/users/{id}",
        |req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Route", "on".to_string()));
            res
        },
        |Path(id): Path<u32>| async move { format!("user {id}") },
    );

    let client = TestClient::start(app).await;
    let res = client.get("/users/7").send().await;
    assert_eq!(res.text(), "user 7");
    assert_eq!(res.header("x-route").unwrap(), "on");
    client.shutdown().await;
}

/// The empty-prefix group was the documented workaround for a single route; the dedicated form
/// exists to replace it, so the two must produce the same wire result and the plain route must
/// stay untouched by both.
#[tokio::test]
async fn an_empty_prefix_group_and_a_per_route_chain_agree() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-App", "1".to_string()));
        res
    });
    app = app.group("", |group| {
        group.use_middleware(|req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Scoped", "group".to_string()));
            res
        });
        group.get("/by-group", |_| "one");
    });
    app = app.route_sync_with(
        Method::Get,
        "/by-route",
        |req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Scoped", "group".to_string()));
            res
        },
        |_| "two",
    );
    app = app.get("/plain", |_| "three");

    let client = TestClient::start(app).await;
    let by_group = client.get("/by-group").send().await;
    let by_route = client.get("/by-route").send().await;
    assert_eq!(by_group.status, by_route.status);
    assert_eq!(by_group.header("x-app"), by_route.header("x-app"));
    assert_eq!(by_group.header("x-scoped").unwrap(), "group");
    assert_eq!(by_group.header("x-scoped"), by_route.header("x-scoped"));

    let plain = client.get("/plain").send().await;
    assert_eq!(plain.header("x-app").unwrap(), "1");
    assert!(plain.header("x-scoped").is_none());
    client.shutdown().await;
}
