# 08. دليل البدء السريع وأمثلة الاستخدام (Getting Started & Examples)

يوضح هذا الدليل كيفية إنشاء وتوسيع تطبيق باستخدام إطار عمل Aether، مع أمثلة للتعامل مع الـ JSON، المسارات الديناميكية، الـ State المشتركة، وحماية الـ Endpoints.

---

## 1. تطبيق بسيط (Hello World & JSON API)

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct User {
    id: u32,
    name: String,
}

#[derive(Deserialize)]
struct CreateUserRequest {
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. مسار نصي بسيط (المعالج غير المتزامن يُسجَّل بـ get_async)
    app = app.get("/", |_ctx| Response::text("Hello from Aether Fast Framework! ⚡"));

    // 2. مسار ديناميكي مع معلمة مسار (Path Parameter) — الصيغة `{id}` لا `:id`
    app = app.get_async("/users/{id}", |ctx: RequestContext| async move {
        let user_id = ctx.param("id").unwrap_or("0");
        let response_data = User {
            id: user_id.parse().unwrap_or(0),
            name: "John Doe".to_string(),
        };
        Response::json(serde_json::to_string(&response_data).unwrap().into())
    });

    // 3. مسار POST لاستقبال وفك تشفير JSON
    app = app.post_async("/users", |ctx: RequestContext| async move {
        match ctx.json::<CreateUserRequest>() {
            Ok(payload) => {
                let new_user = User {
                    id: 101,
                    name: payload.name,
                };
                Response::json_status(
                    201,
                    "Created",
                    serde_json::to_string(&new_user).unwrap(),
                )
            }
            Err(_) => Response::status(400, "Bad Request: Invalid JSON"),
        }
    });

    println!("🚀 Aether Server listening on http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await?;

    Ok(())
}
```

`Response::json` لا تُسلسل شيئًا بنفسها ولا تقبل `&serializable`؛ هي تأخذ `Bytes`، و`json_status`
تأخذ أي `Into<Bytes>`. ولا وجود لدالة `.with_json(..)` على الاستجابة — الحالة والـ JSON يُختاران
بدوال البناء نفسها.

---

## 2. إدارة الحالة المشتركة (Shared State)

```rust
use aether::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct AppState {
    counter: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // التسجيل بـ manage؛ القيمة تُقرأ داخل المعالج بالـ Type عبر ctx.state::<T>()
    app = app.manage(AppState {
        counter: AtomicU64::new(0),
    });

    app = app.get_async("/hits", |ctx: RequestContext| async move {
        // الوصول للحالة المشتركة بأمان فائق دون أقفال ثقيلة
        let state = ctx.state::<AppState>().expect("AppState registered");
        let hits = state.counter.fetch_add(1, Ordering::Relaxed) + 1;
        Response::text_dynamic(format!("Total Hits: {}", hits))
    });

    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

---

## 3. تخصيص حدود الأمان والـ Timeouts

```rust
use aether::app::AetherApp;
use aether::config::AetherConfig;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = AetherConfig {
        max_header_bytes: 16 * 1024,          // 16 KB
        max_body_bytes: 10 * 1024 * 1024,     // 10 MB
        max_headers_count: 100,               // عدد أسطر الترويسة المسموح بها لكل طلب
        max_concurrent_streams: 100,          // أقصى عدد تدفقات HTTP/2 متزامنة لكل اتصال
        max_request_time: Duration::from_secs(3),
        max_receive_time: Duration::from_secs(30), // استقبال رفع بطيء دون توسيع مهلة المعالج
        max_response_time: Duration::from_secs(5),
        keep_alive_timeout: Duration::from_secs(30),
        ..AetherConfig::default()
    };

    let app = AetherApp::with_config(config);
    // تسجيل المسارات...
    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

> `AetherConfig` هو الواجهة العامة للحدود: كل حقل فيه يُترجَم إلى `ServerLimits` التي ينفذها
> المحلّل وطبقة HTTP/2. أما `max_connections` و`read_timeout` و`tls_handshake_timeout` فلا تزال
> قيم ثابتة على `ServerLimits::default()` ولا تُضبط من `AetherConfig`.
