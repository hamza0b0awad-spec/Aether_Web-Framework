# 📚 التوثيق الشامل والرسمي لإطار عمل Aether Web Framework ⚡

> **ملاحظة تدقيق (2026-10-18):** هذا المستند يصف التصميم المقصود، وليس كل ما ورد فيه مُنفَّذاً.
> `aether-http` لا يدعم HTTP/2 (الخادم HTTP/1.1 فقط)؛ `RequestArena` موجودة ومُختبرة لكنها غير
> مستخدمة في مسار الخادم؛ معاملات المسارات تُفكّ وتُنسخ إلى `String`، والجسم يُنسخ إلى `Bytes`
> قبل تنفيذ المعالج. أُضيف لاحقاً دعم WebSocket كامل (handshake + connection driver +
> `app.ws`)، ومحلل `multipart/form-data`، ودعم `Range` للملفات الثابتة.
> **تحديث (2026-10-20):** إنهاء TLS أصبح مُنفَّذاً ومُختَبَراً داخل العملية (`aether::tls` على
> rustls)، على نفس المنفذ الذي يخدم `http://`، مع SNI وإعادة تحميل الشهادات بدون إعادة تشغيل
> (§18). **تحديث (2026-10-21):** HTTP/2 أصبح مُنفَّذاً فوق TLS عبر ALPN `h2`، وفوق النص الصريح
> على منفذ يختار ذلك بـ `AetherApp::enable_h2c` (§19)؛ `Upgrade: h2c` غير مُنفَّذ وعن قصد،
> وHTTP/3 ما زال غير مُنفَّذ. راجع
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).

### المرجع الكامل لكافة عناصر، مكونات، وصناديق إطار العمل مع أمثلة عملية شاملة

---

## 📑 جدول المحتويات (Table of Contents)
1. [نظرة عامة على بنية الإطار وهندسته (System Architecture)](#1-نظرة-عامة-على-بنية-الإطار-وهندسته)
2. [التثبيت والإعداد في المشاريع (Installation & Cargo Setup)](#2-التثبيت-والإعداد-في-المشاريع)
3. [عنصر التطبيق الأساسي (AetherApp & Server Configuration)](#3-عنصر-التطبيق-الأساسي)
4. [محرك المسارات والمعالجات (Routing: Sync & Async Handlers)](#4-محرك-المسارات-والمعالجات)
5. [سياق الطلب واستخراج البيانات (RequestContext & Extractors)](#5-سياق-الطلب-واستخراج-البيانات)
6. [نظام الاستجابات والتحويل التلقائي (Response & IntoResponse)](#6-نظام-الاستجابات-والتحويل-التلقائي)
7. [إدارة الحالة المشتركة للتطبيق (Global Application State)](#7-إدارة-الحالة-المشتركة-للتطبيق)
8. [البرمجيات الوسيطة والأمان (Middleware & Web Security)](#8-البرمجيات-الوسيطة-والأمان)
9. [محرك الذاكرة فائق السرعة (RequestArena & Zero-Copy Engine)](#9-محرك-الذاكرة-فائق-السرعة)
10. [التحقق السريع بـ SIMD (SIMD Validation)](#10-التحقق-السريع-بـ-simd)
11. [حاوية حقن التبعيات والرسم البياني (Dependency Injection & DAG)](#11-حاوية-حقن-التبعيات-والرسم-البياني)
12. [بروتوكول WebSocket فائق السرعة (RFC 6455 Frame Parser)](#12-بروتوكول-websocket-فائق-السرعة)
13. [المولد التلقائي لمواصفات OpenAPI v3.1](#13-المولد-التلقائي-لمواصفات-openapi-v31)
14. [محرك التشخيص الذكي (Terminal Diagnostics)](#14-محرك-التشخيص-الذكي)
15. [خدمة الملفات الثابتة و ETag (Static Files & Asset Serving)](#15-خدمة-الملفات-الثابتة-و-etag)
16. [مشروع إنتاجي متكامل شامل لجميع العناصر (Full Production Blueprint)](#16-مشروع-إنتاجي-متكامل-شامل-لجميع-العناصر)
17. [إنهاء TLS على نفس المنفذ (aether::tls, SNI, live reload)](#18-إنهاء-tls-على-نفس-المنفذ-aethertls-)

---

## 1. نظرة عامة على بنية الإطار وهندسته

تم تصميم **Aether** وفق مبادئ **Zero-Cost Abstractions** و **Zero-Copy Parsing** و **Cache-Line Alignment**، حيث تم تقسيمه إلى حزم معيارية معزولة داخل Cargo Workspace:

| الحزمة (Crate) | المسؤولية الفنية والوظيفية |
| :--- | :--- |
| `aether` | واجهة الاستخدام الموحدة للواجهة والمطور (Facade & Prelude) |
| `aether-core` | مخصص الذاكرة السريعة `RequestArena` ومحرك التشخيص `Diagnostic Engine` |
| `aether-http` | محلل طلبات HTTP/1.1 والردود، والملفات الثابتة — **لا يحتوي أي كود HTTP/2**: الترميز يتولّاه `h2` 0.4 والجسر فوق `ServiceCore` في `aether/src/http2.rs` (انظر «حالة التوصيل» أدناه و§19) |
| `aether-router` | شجرة المسارات `RadixRouter` مع فحص الأفعال بالـ 64-Bit Bitmask |
| `aether-runtime` | حلقة أحداث العامل (Worker Event Loop) ومعالجة اتصالات TCP |
| `aether-middleware` | الترويسات الأمنية، CORS، ومسجل الطلبات الدقيق بالميكروثانية |
| `aether-security` | التشفير السريع، التحقق من JWT، والمقارنة بزمن ثابت `constant_time_eq` — **موصولة بالواجهة الموحدة منذ المسار 91 (2026-10-28): `aether::{JwtValidator, JwtValidationOptions, VerifiedJwt, constant_time_eq, audit_request_desync}`** |
| `aether-validation` | التحقق من صحة البريد الإلكتروني والنصوص بقناع `u64` يفحص ثمانية بايتات في كل طلقة (فحص كلمة-بكلمة، لا تعليمات متجهية) — **لا تعتمد عليها `aether`** (مختبَرة داخل `framework/tests` فقط) |
| `aether-dependency` | حاوية حقن التبعيات بحل شجرة الرسم البياني `Topological DAG Sort` |
| `aether-websocket` | محلل إطارات الـ WebSocket (RFC 6455) بدون تخصيص ذاكرة |
| `aether-openapi` | مولد وثائق ومخططات OpenAPI v3.1 JSON — **موصولة عبر `aether::openapi` منذ المسار 91 (2026-10-28)، ويُولَّد المخطط من جدول المسارات منذ المسار 108 (2026-10-31) عبر `AetherApp::openapi_spec`** |

> **حالة التوصيل (تحقّق 2026-10-20، صُحّحت 2026-10-28، وأعيدت في المسار 111 في 2026-11-02).**
> `aether` تعتمد ثماني حزم: `aether-core`، `aether-http`، `aether-router`، `aether-middleware`،
> `aether-scheduler`، `aether-security`، `aether-websocket`، `aether-openapi`. وُصلت
> `aether-security` و`aether-openapi` في المسار 91 وتُعاد تصديرهما عبر `aether::{security, openapi}`
> وأسمائهما في جذر الواجهة، مع اختبارات `facade_wiring` التي تستورد من `aether::` فقط.
> حذف المسار 111 حزمتي observability وserialization من الـ workspace كله: لا حزمة كانت
> تعتمد عليهما — إعادة grep على كل Cargo.toml قبل الحذف لا تجد إلا سطر اسم
> كل واحدة في حزمتها — ولا اختبار في أيٍّ منهما (لا `#[test]`، لا مجلد tests، لا
> مثال موثّق في مصادرهما الثلاثة)، وهذا ما يفسّر بقاء مجموع الـ workspace بعد الحذف
> 814 ناجحًا و0 فاشلًا و1 متجاهلًا كما هو قبله. وأزال المسار نفسه عن `aether`
> تبعتي runtime وdependency لأن لا سطر في مصادرهما يذكر اسم أيٍّ منهما؛ وبقيت
> runtime وdependency داخل الـ workspace واختباراتهما تُشغّل كما كانت.

---

## 2. التثبيت والإعداد في المشاريع

للبدء في أي مشروع Rust جديد، أنشئ مجلد المشروع وأضف الاعتماديات التالية إلى `Cargo.toml`:

```toml
[package]
name = "my_aether_api"
version = "0.2.0"
edition = "2024"

[dependencies]
aether = { path = "../framework/aether" } # أو استخدام الإصدار في حال النشر
tokio = { version = "1.38", features = ["full"] }
bytes = "1.6"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

---

## 3. عنصر التطبيق الأساسي (`AetherApp`)

`AetherApp` هو الهيكل الأساسي الذي يقوم بتجميع المسارات، الحالة، وسيطات الأمان، وتكوين خادم الشبكة.

```rust
use aether::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. الحدود العامة للتطبيق تُعبَّر بـ AetherConfig (الواجهة العامة)
    let config = AetherConfig {
        max_header_bytes: 8 * 1024,        // 8 KB
        max_body_bytes: 10 * 1024 * 1024,  // 10 MB
        max_headers_count: 64,             // عدد أسطر الترويسة لكل طلب
        max_request_time: Duration::from_secs(30),
        max_receive_time: Duration::from_secs(30), // ميزانية استقبال مستقلة عن مهلة المعالج
        ..AetherConfig::default()
    };

    // 2. إنشاء التطبيق بهذه الحدود
    let app = AetherApp::with_config(config);

    // 3. تشغيل الخادم
    app.listen("0.0.0.0:3000").await
}
```

> هذه الكتلة فُحصت بـ `cargo check` بتاريخ 2026-10-22 مقابل `aether` بالمسار (exit 0 بلا تحذيرات).
> نصّها السابق كان `let limits = ServerLimits { max_headers: 64, max_header_bytes: 8192, max_body_bytes: ... };`
> ولا يمكن أن يترجم لثلاثة أسباب: `ServerLimits` ليس مُصدَّرًا من `aether` إطلاقًا — هو في
> `aether-http` والـ prelude لا يعيد تصديره —، اسم الحقل الصحيح
> `max_headers_count` لا `max_headers`، وبناؤها كمُنشئ جزئي ناقص سبعة حقول. والأهم: المتغيّر كان
> يُبنى ولا يُمرَّر إلى أي شيء، فالخادم يعمل بالافتراضات أيًا كانت.
> `AetherConfig` هي الطريق العامة: كل حقولها تُترجَم عبر اشتقاق واحد (`AetherApp::limits_for`)
> إلى `ServerLimits` التي ينفذها المحلّل.

---

## 4. محرك المسارات والمعالجات (Routing Engine)

### أ) المعالجات المتزامنة السريعة (Synchronous Handlers):
للعمليات الحسابية الخفيفة التي لا تتطلب I/O (تُنفَّذ في نفس الـ task بلا `.await` ولا وسائط
مؤقتة؛ لا رقم زمني هنا لأن هذا النص لم يُقس في هذه الشجرة — مقياس المسارات المتاح هو
`cargo bench -p aether-benchmarks --bench router_bench` وهو يقيس الـ trie لا تنفيذ المعالج):
```rust
app = app.get("/ping", |_req| "pong");

app = app.post("/calculate", |req| {
    format!("Payload byte length: {}", req.body.len())
});
```

### ب) المعالجات غير المتزامنة (Asynchronous Handlers):
لقواعد البيانات والاتصالات الشبكية:
```rust
app = app.get_async("/users", |req: RequestContext| async move {
    // محاكاة عملية I/O لقاعدة بيانات
    tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
    Response::json(bytes::Bytes::from(r#"[{"id":1,"name":"حمزة"}]"#))
});

app = app.post_async("/orders/create", |req: RequestContext| async move {
    // قراءة البيانات ومعالجتها عبر async/await
    Response::ok().header("X-Order-Status", "Created")
});
```

### ج) استخراج وتحويل متغيرات المسار (Dynamic Route Parameters & Typed Parsing):
تُستخرج المتغيرات بتقنية Zero-Copy بدون أي حجز للذاكرة، مع إمكانية التحويل التلقائي للأنواع (`i32`, `u64`, `bool`, إلخ):
```rust
// 1. استخراج كنص خام
app = app.get("/users/{id}", |req| {
    let user_id = req.param("id").unwrap_or("0");
    format!(r#"{{"user_id":"{}"}}"#, user_id)
});

// 2. استخراج وتحويل تلقائي مباشر إلى أرقام (Typed Extractor)
app = app.get("/products/{id}", |req| {
    let product_id: u64 = req.param_parse("id").unwrap_or(0);
    format!("Product ID: {}", product_id)
});

app = app.get("/orgs/{org_id}/repos/{repo_name}", |req| {
    let org = req.param("org_id").unwrap_or("unknown");
    let repo = req.param("repo_name").unwrap_or("unknown");
    format!("Organization: {}, Repository: {}", org, repo)
});
```

### د) تجميع المسارات مع دعم كامل لجميع أفعال REST (`Route Grouping`):
يدعم `RouteGroup` كافة أفعال HTTP (`get`, `post`, `put`, `delete`, `patch`) المتزامنة وغير المتزامنة:
```rust
app = app.group("/api/v1", |v1| {
    v1.get("/status", |_req| r#"{"status":"v1_active"}"#);
    v1.get_async("/metrics", |_req: RequestContext| async move { r#"{"cpu_load":"4%"}"# });
    v1.post_async("/auth/login", |req: RequestContext| async move { /* معالجة الدخول */ Response::ok() });
    v1.put_async("/users/{id}", |req: RequestContext| async move { /* تحديث مستخدم */ Response::ok() });
    v1.delete_async("/items/{id}", |req: RequestContext| async move { /* حذف عنصر */ Response::ok() });
});

app = app.group("/admin", |admin| {
    admin.get("/panel", |_req| Response::html("<h1>Admin Panel</h1>"));
});
```

---

## 5. سياق الطلب واستخراج البيانات (`RequestContext`)

يمثل `RequestContext` الكائن الشامل لكل تفاصيل الطلب الوارد مع دوال مساعدة فائقة الراحة:

```rust
app = app.post("/api/checkout", |req| {
    // 1. المتغيرات في مسار الرابط (نص أو محولة رقمياً)
    let user_id = req.param("id");
    let user_id_num: i32 = req.param_parse("id").unwrap_or(1);

    // 2. معرف الطلب الفريد للتتبع (Distributed Tracing ID) — استعارة، لا نقل ملكية
    let trace_id = req.request_id.as_str(); // مثال: "req_000000000001"

    // 3. الترويسات والكوكيز (Headers & Cookies)
    let auth_header = req.header("authorization");
    let session_cookie = req.cookie("session_id"); // القيمة كما كتبها العميل، لا دليل بها
    // وكوكي لا يقرره العميل يُقرأ موقَّعاً: req.signed_cookie(&signer, "session") -> SignedCookie

    // 4. متغيرات الاستعلام (Query String) - نص أو محولة رقمياً
    let page: i32 = req.query_parse("page").unwrap_or(1);
    let filter = req.query("filter").unwrap_or("all");

    // 5. قراءة نماذج الويب المرسلة عبر Form POST (URL-Encoded Form Data)
    let form_fields = req.form_data(); // يرجع HashMap<String, String> مفككة الرموز %XX و + تلقائياً
    let action = form_fields.get("action");

    // 6. قراءة الـ Body كنص UTF-8 أو كشريحة بايتات مستعارة (بلا نسخ)
    let body_text = req.body_str().unwrap_or("");
    let raw_bytes: &[u8] = &req.body;

    // 7. تحويل الـ JSON تلقائياً إلى بنية Struct
    #[derive(serde::Deserialize)]
    struct OrderRequest {
        product_id: u64,
        quantity: u32,
    }

    if let Ok(order) = req.json::<OrderRequest>() {
        println!("Received order for product: {}", order.product_id);
    }

    Response::ok()
});
```

### قيم تخصّ طلباً واحداً فقط (`Extensions`)

الحالة المشتركة عبر `manage` تعيش عمر العملية كلها، فهي المكان الصحيح لاتصال قاعدة بيانات أو مفتاح
تشفير، وهي المكان الخاطئ لشيء قرّرته البرمجيات الوسيطة عن **هذا** الطلب: من المتصل، أي مستأجر هو،
أو أي ميزة فُعّلت له. لهذا توجد `Extensions` — خريطة مفاتيحها نوع Rust نفسه، تركب مع
`RequestContext` وتنتقل معه عبر سلسلة البرمجيات الوسيطة حتى المعالج:

```rust
use aether::prelude::*;

#[derive(Clone)]
struct Principal(String);

// طبقة وسيطة تضع القيمة
app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
    if let Some(token) = req.header("authorization") {
        req.insert_extension(Principal(token.to_string()));
    }
    next(req).await
});

// 1) كوسيط مكتوب بالنوع (يتطلب `Clone`، وهو نفس القيد في Axum)
app = app.get_async("/whoami", |Extension(p): Extension<Principal>| async move {
    format!("مرحباً {}", p.0)
});

// 2) أو بالاقتراض من السياق — وهذا يعمل أيضاً للقيم التي لا تكون `Clone`
app = app.get_async("/whoami2", |req: RequestContext| async move {
    match req.extension::<Principal>() {
        Some(p) => Response::text_dynamic(p.0.clone()),
        None => Response::text_dynamic("anonymous".to_string()),
    }
});
```

قواعد مهمّة:
- الخريطة تُحَجَّز (boxed) عند أول `insert` فقط، فالطلب الذي لا يستخدمها لا يخصّص شيئاً إطلاقاً.
- القيمة لكل **طلب**: في HTTP/2 كل تدفق (stream) له خريطته ولو شاركوا اتصالاً واحداً، وفي HTTP/1.1
  كل طلب على اتصال keep-alive له خريطته. الاختباران `extensions_are_per_stream_never_per_connection`
  و`extensions_do_not_leak_between_requests_on_one_connection` يثبِتان ذلك على الشبكة لا في الوحدة.
- استخراج نوع غير موجود عبر `Extension<T>` يرجع **500** ولا ينفّذ المعالج.
- معالج `app.ws` يستلم نفس الخريطة: سلسلة وسائط المسار تُنفَّذ حول **طلب الترقية** قبل التحقق من
  الـ handshake (انظر §12)، فما يختمه الحارس يقرأه معالج الجلسة، وما يعيده من ردّ دون `next`
  يمنع الترقية.
- `Debug` يطبع عدد القيم ولا يطبع قيمها، حتى لا تسرب بيانات طلب إلى السجلات.
- **حدّ معروف:** حجم القيمة المخزّنة لا يُحسب ضمن ميزانية ذاكرة الجدولة (التي تحاسب الاتصالات
  والمسارات ومخازن الطلب)، فمسؤولية أي قيمة كبيرة هي مسؤولية من أدخلها.

---

## 6. نظام الاستجابات والتحويل التلقائي (`Response & IntoResponse`)
### أ) بناء الاستجابات عبر `Response`:
```rust
// 1. استجابة نجاح 200 OK فارغة
Response::ok();

// 2. استجابة HTML من نص ثابت (Zero-Copy &'static str)
Response::html("<h1>صفحة ترحيبية ثابتة</h1>");

// قيمتان تمهيديتان حتى يترجم هذا المقتطف بذاته؛ القارئ يستبدلهما بنتائجه الحقيقية
let username = "حمزة";
let image_bytes = bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n");

// 3. استجابة HTML من نص ديناميكي أو قوالب (String / Tera / Askama)
Response::html_dynamic(format!("<h1>مرحباً بك يا {}!</h1>", username));

// 4. قراءة وخدمة ملف مباشرة من القرص الصلب مع اكتشاف نوع الـ MIME تلقائياً
Response::file("public/index.html"); // يرجع Result<Response, std::io::Error>

// 5. استجابة JSON مع حالة 200 الافتراضية
Response::json(bytes::Bytes::from(r#"{"status":"success"}"#));

// 6. استجابة JSON مع رمز حالة مخصص (Custom Status Code)
Response::json_status(401, "Unauthorized", r#"{"error":"Invalid Credentials"}"#);
Response::json_status(400, "Bad Request", r#"{"error":"Missing fields"}"#);

// 7. استجابة بايتات من أي مصدر مع تحديد نوع المحتوى
Response::bytes(image_bytes, "image/png");

// 8. إعادة التوجيه (Redirect)
Response::redirect("/dashboard", 302);

// 9. إضافة ترويسات وكوكيز برمجية بنمط الـ Builder
let res = Response::ok()
    .header("X-Custom-Engine", "Aether")
    .set_cookie("session_id", "token_12345", true, true, "Strict", Some(86400));
```

### ب) التحويل التلقائي (`IntoResponse`):
```rust
// نص مباشر
app = app.get("/text", |_req| "نص عادي بترميز UTF-8");

// كود حالة مع نص
app = app.get("/forbidden", |_req| (403, "غير مصرح بالدخول"));

// معالجة الأخطاء عبر Result — نوع الخطأ يجب أن يطبّق IntoResponseError، والوسيط tuple لا يطبّقه
app = app.get("/check", |_req| -> Result<&'static str, &'static str> {
    let authorized = true;
    if authorized {
        Ok("تم التحقق بنجاح")
    } else {
        Err("مطلوب تسجيل الدخول")
    }
});
```

> فُحص هذا المقطع بـ `cargo check` بتاريخ 2026-10-24 (exit 0 بلا تحذيرات). نوع الخطأ في النسخة
> السابقة كان `(u16, &'static str)` أي `Result<&'static str, (u16, &'static str)>`، وهو مرفوض
> بـ `E0277` — أعيد قياس الرفض بعد التغيير وسببه أن `(u16, &'static str)` لا يطبّق
> `IntoResponseError` (قبل التغيير كان الرفض لأن الـ tuple لا يطبّق `Display`). والسلوك الحالي:
> التطبيق العام (`aether/src/app.rs:224-265`) يسأل الخطأ عن حالته عبر `IntoResponseError`
> (`app.rs:123-166`)، فـ `&'static str` و`String` تجيب **400** مع جسم JSON
> `{"error":"<الرسالة>","code":"E4003"}`، و`AetherError` تجيب 400 أو 404 أو 405 أو 413 أو 431 أو
> 500 أو 504 بحسب الحالة (الجدول كامل في `ERROR_HANDLING_GUIDE_AR.md` قسم 1أ). قبل 2026-10-24 كان
> الطلب `E: Display + Send` فيُجبر كل `Err` على 400. لحالة خاصة لخطأك طبّق عليه
> `aether::IntoResponseError`. الاختباران
> `a_handler_error_answers_400_and_says_so_in_the_body` و`typed_handler_errors_answer_their_own_class`
> يثبّتان الجدول على TCP حقيقي.

---

## 7. إدارة الحالة المشتركة للتطبيق (`Global Application State`)

لمشاركة اتصالات قواعد البيانات، المخابئ، وحالة الخادم بين كافة المسارات:

> ما يخصّ **طلباً واحداً** لا يوضع هنا: انظر «قيم تخصّ طلباً واحداً فقط (`Extensions`)» في القسم 5.

```rust
use aether::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

// 1. تعريف بنية الحالة
struct AppState {
    db_pool: String,
    total_requests: AtomicUsize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 2. تسجيل الحالة داخل التطبيق
    app = app.manage(AppState {
        db_pool: "PostgresConnectionPool".to_string(),
        total_requests: AtomicUsize::new(0),
    });

    // 3. استخراج الحالة واستخدامها داخل أي معالج
    app = app.get("/stats", |req| {
        if let Some(state) = req.state::<AppState>() {
            let count = state.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
            format!("Database: {}, Total Served Requests: {}", state.db_pool, count)
        } else {
            "AppState is not configured".into()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```

---

## 8. البرمجيات الوسيطة والأمان (`Middleware & Security`)

### أ) تفعيل ترويسات الأمان المتقدمة تلقائياً:
```rust
// يفعل تلقائياً HSTS, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, X-XSS-Protection
app = app.enable_security_headers();
```

### ب) تفعيل سياسة تبادل الموارد (CORS):
```rust
app = app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization, X-Request-ID",
    max_age_secs: 86400,
    ..CorsConfig::default()
}));
```

### ج) المقارنة في زمن ثابت لمنع هجمات التوقيت (`constant_time_eq`):

> صارت `aether_security` موصولة بالواجهة الموحدة منذ المسار 91 (2026-10-28): استخدم
> `aether::constant_time_eq` أو `aether::security::constant_time_eq`. واستيراد الحزمة مباشرةً
> (`aether_security::…`) يبقى صالحاً لمن لا يريد الواجهة الموحدة.

```rust
use aether::constant_time_eq;

let expected_hash = b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
let provided_hash = b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

let is_valid = constant_time_eq(expected_hash, provided_hash);
```

### د) التحقق من توقيع JWT:
```rust
use aether::JwtValidator;

let validator = JwtValidator::new(b"your-256-bit-secret-key-here");
let raw_token = "header.payload.signature";

if let Ok((header, payload)) = validator.verify_signature(raw_token) {
    println!("Token Validated! Header: {}, Payload: {}", header, payload);
}
```

### هـ) أين يُحفظ السرّ: لا نوع مغلِّف في الإطار (غير هدف مقصود، المسار 103):

السرّ يُمرَّر وسيطاً إلى الباني — `JwtValidator::new`، `BearerAuth::new`، `CsrfTokens::new`،
`TlsCertSet::from_pem` — ولا يُخزَّن حقلَ إعدادات في `AetherConfig`. والقرار أنه لا يوجد غلاف
`Secret<T>` عام: غلافٌ لا يُلزم أحداً باستخدامه سطحُ واجهةٍ بلا ضمان خلفه. الضمانُ المنفَّذ بدلاً
منه أن الإطار لا يعرض السرَّ أبداً:

- `JwtValidator` و`BearerAuth` و`CsrfTokens` لا تنفّذ `Debug` إطلاقاً، ولكل واحدة كتلة
  `compile_fail,E0277` توثيقية تسقط إن أُضيفت `Debug`.
- `JwtValidator::new` و`BearerAuth::new` ترفضان سرّاً فارغاً ذعراً عند البناء، لأن مفتاح HMAC
  بطول صفر يقبل كل توكن وُقّع به؛ و`CookieSigner::new` و`CsrfTokens::new` ترفضان القيمة نفسها.
- `CsrfToken` تطبع `CsrfToken(<redacted>)` وحدها `as_str()` تعيد النص.
- خريطة `Extensions` تطبع العدد والبایتات المحجوزة، لا القيم.
- `BearerError` لا تحمل سبباً ولا نصّ الاعتماد نفسه؛ ورفض CSRF فارغ الجسم.
- أنواع TLS تطبع السلسلة العامة فقط؛ مفتاح التوقيع في rustls يطبع الخوارزمية لا البايتات،
  ويُفحص ذلك باختبار يبحث في العرض عن نوافذ من مفتاح PEM/DER لا تظهر في الشهادة.

أما سرّ التطبيق نفسه — مفتاحٌ يقرأه من متغيّر بيئة ثم يطبع هو بنفسه — فتنقيحه مسؤولية
التطبيق، وهذا حدُّ ما يضمنه إطارٌ يمكن أن يُطبع نوعُه هو.

---

## 9. محرك الذاكرة فائق السرعة (`RequestArena`)

يوفر Aether مخصص ذاكرة Bump Allocator مخصص لكل اتصال TCP:
- لا يقوم باستدعاء `malloc` أو `free` في كل طلب HTTP.
- تتم إعادة التصفير عبر تعديل مؤشر واحد $O(1)$ (`arena.reset()`).

```rust
use aether_core::arena::RequestArena;

// إنشاء Arena بسعة 4096 بايت (محاذاة 64-byte للـ Cache Line)
let mut arena = RequestArena::with_capacity(4096);

// حجز مساحة لشريحة بايتات دون حجز على الـ Heap
let slice = arena.alloc_slice(b"Zero-Copy payload data").unwrap();

// التصفير يرجع مؤشر النهاية إلى الصفر ولا يحرر الذاكرة — للطلب التالي
arena.reset();
```

> فُحص هذا المقطع بـ `cargo check` بتاريخ 2026-10-22 مقابل `aether-core` بالمسار (exit 0 بلا
> تحذيرات). كان `let arena = ...` فيُرفض: `reset(&mut self)` تحتاج استعارة mutable
> (E0596). أما رقم «أقل من 1 نانوثانية» فحُذف لأنه لم يُقس في هذه الشجرة؛ ما يضمنه الكود
> بنظره هو أن `reset` لا يمر على العناصر ولا يحرّر شيئًا، والمحاذاة الأساسية 64 بايت
> (`ARENA_ALIGN` في `core/src/arena.rs:15`) وطلب محاذاة أعلى من ذلك يُرفض بدل أن يُعاد مزالًا.
> للتنبيه: `RequestArena` موجودة ومُختبرة في `aether-core` لكنها ليست على مسار خدمة الطلبات
> في `aether-http` (انظر ملاحظة رأس هذا الملف).

---

## 10. التحقق السريع بـ SIMD

> `aether_validation` لا تزال خارج حزمة `aether` (انظر «حالة التوصيل» في §1). قسم التسلسل
> الذي كان هنا حُذف مع حزمة serialization في المسار 111 (2026-11-02).
>
> تصحيح عنوان: لا تستخدم هاتان الدالتان تعليمات متجهية (لا يوجد `core::arch` في أي ملف `.rs`
> من الشجرة). `validate_ascii_fast` يفحص ثمانية بايتات بقناع واحد من نوع `u64`، أي فحص
> كلمة-بكلمة وليس SIMD، ومصفوفة الاكتمال تسجّله تحت هذا الاسم بحالة `PARTIALLY_IMPLEMENTED`.
> بقي عنوان §10 بكلمة SIMD لأنه مقصد رابط الفهرس؛ هذه الملاحظة هي ما يجعل العبارة دقيقة.

### أ) الفحص السريع بقناع `u64` (كلمة-بكلمة، لا تعليمات متجهية):
```rust
use aether_validation::{validate_email_format, validate_ascii_fast};

let is_email = validate_email_format("user@aether-framework.io"); // true
let is_ascii = validate_ascii_fast(b"High Performance Rust");     // true
```

---

## 11. حاوية حقن التبعيات والرسم البياني (`Dependency Injection`)

بناء رسم بياني موجه غير دوري (DAG) للخدمات مع التحقق من عدم وجود دورات مفرغة وقت الإقلاع.
الواجهة العامة هي `DependencyGraph` (عقد تُبنى بـ `DependencyNode` وتُضاف بـ `add_node`)، والخطة
تُشتق بـ `compile_execution_plan()`:

```rust
use aether_dependency::{DependencyGraph, DependencyNode, DependencyScope};

let mut graph = DependencyGraph::new();

// تسجيل الخدمات — المعرّف من نوع NodeId (== usize) واسم العقدة &'static str
graph.add_node(DependencyNode {
    id: 1,
    name: "ConfigService",
    scope: DependencyScope::Singleton,
    dependencies: vec![],
});
graph.add_node(DependencyNode {
    id: 2,
    name: "DatabasePool",
    scope: DependencyScope::Singleton,
    dependencies: vec![1],
});
graph.add_node(DependencyNode {
    id: 3,
    name: "AuthService",
    scope: DependencyScope::RequestScoped,
    dependencies: vec![2],
});

// التحقق وترتيب التسلسل تلقائياً (Topological Sorting)
// Err عند وجود دورة، أو عند مرجع إلى معرّف غير موجود في الرسم.
let execution_order: Vec<usize> = graph.compile_execution_plan().unwrap();
```

> فُحص هذا المقطع بـ `cargo check` بتاريخ 2026-10-22 مقابل `aether-dependency` بالمسار (exit 0 بلا
> تحذيرات). كتلة الوثيقة السابقة كانت `use aether_dependency::{DependencyContainer, DependencyScope};`
> ثم `container.register(...)` و`container.add_dependency(...)` و`container.resolve_build_order()`:
> لا `DependencyContainer` ولا هذه الدوال موجودة في الصندوق، و`DependencyScope` لا يحوي `Scoped` —
> الصيغ هي `Singleton` و`RequestScoped` و`Transient`. الخطة مرتّبة ترتيبًا طوبولوجيًا
> وحتميًا (min-heap على المعرّف) منذ إصلاح الحتمية، لا حسب ترتيب تكرار `HashMap`.

---

## 12. بروتوكول WebSocket (RFC 6455)

الخادم يدعم WebSocket كاملاً: يتحقق من الـ handshake، ويكتب `101 Switching Protocols`، ثم
يشغّل المعالج مع connection driver يجمّع الرسائل المجزأة ويرد على `Ping` ويغلق الاتصال
بشكل صحيح. الترقية نفسها يطلبها **HTTP/1.1 فقط**: `Upgrade` آلية لا وجود لها في HTTP/1.0
(RFC 9110 §6.7) وسطرُ `101` الذي تكتبه الواجهة يسمّي تلك النسخة، فطلبُ ترقية بـ `HTTP/1.0`
يُرفض بـ `400` مؤطَّرًا بـ `HTTP/1.0` ولا تبدأ أي جلسة
(`an_http10_upgrade_request_is_refused_instead_of_switching_protocols`). الرسائل الافتراضية محدودة بـ 16 MiB للإطار والرسالة:

```rust
use aether::prelude::*;

app = app.ws("/ws", |_req, mut socket| async move {
    while let Ok(Some(message)) = socket.recv().await {
        match message {
            WsMessage::Text(text) => {
                let _ = socket.send_text(&text).await;
            }
            WsMessage::Binary(bytes) => {
                let _ = socket.send_binary(&bytes).await;
            }
        }
    }
});
```

طلبات الـ handshake غير الصحيحة تُرفض قبل تنفيذ المعالج: `400` للمفتاح/الترويسات غير
الصالحة، و`426` مع `Sec-WebSocket-Version: 13` للإصدار غير المدعوم، و`405` لغير `GET`.
التعطيل compression غير مدعوم بعد. أمّا التفاوض على البروتوكول الفرعي (subprotocol) فمُسلَّم منذ
2026-10-29: `ws_subprotocols` على التطبيق أو المجموعة يعلن بروتوكولات المسار بترتيب تفضيلها،
ويُنتقى أول اسم معلن عرضه العميل في الترويسة `Sec-WebSocket-Protocol` وفق RFC 6455 §4.2.2
ويُعاد كتابته في `101`؛ ولا يُعاد إلا اسم أعلنه المسار، وعرضٌ لا يطابق شيئاً من المسارات
المعلِنة يُرفض `400` قبل أي جلسة.

**البرمجيات الوسيطة تسري على طلب الترقية أيضاً** (PROB-0017، 2026-10-21). كان `app.ws` يسجّل
معَالَجه في خريطة لا تلمسها `wrap_with_middleware`، فكان مسار WebSocket لا ينفّذ أي طبقة وسيطة:
سلسلةُ تحققٍ تحمي كل مسار HTTP بينما كل مسار `app.ws` مكشوف، وقيمةُ `Extensions` يضعها حارسٌ
لا تصل إلى معالج الجلسة. الآن تُلفّ سلسلةُ المسار حول طلب الترقية ويمر عبر نفس مسار `dispatch`
(اعتماد الجدولة، مهلة الطلب، وتحويل panic إلى 500)، فالحارس يستطيع أن يختم `Extensions` أو أن يرفض
الترقية بردّه هو.

الترقية تتم فقط عندما ترجع السلسلة `101` الخاصة بالطرف النهائي دون تغيير؛ لذلك رفضُ الحارس نافذٌ
سواء أجاب **قبل** استدعاء `next` أو **بعد** انتظاره: `403` يُعاد بعد `next`، و`500` من panic بعد
`next`، و`504` من حارس يتوقف في طريق العودة — كل واحدة تُخدَم كما هي ولا تُسلَّم المقبس. الأدلة:
`a_websocket_upgrade_runs_the_app_middleware_chain` و
`a_middleware_can_refuse_a_websocket_upgrade` و
`a_middleware_that_answers_after_next_refuses_the_upgrade` و
`a_middleware_hanging_after_next_times_the_upgrade_request_out` و
`a_panicking_middleware_answers_the_upgrade_request_with_500` (panic قبل `next` وبعده)، و
`a_middleware_that_forks_the_chain_still_answers_the_upgrade_request` و
`a_middleware_that_abandons_the_chain_future_refuses_the_upgrade` و
`a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` و
`group_middleware_wraps_the_upgrade_request_of_a_group_websocket_route` و
`middleware_registered_after_a_websocket_route_does_not_wrap_it`.

أشكال السلاسل الغريبة محسومة بنفس القاعدة، لا بقاعدة جديدة: ما يراه العميل هو **ردّ الوسيط**.
وسيطٌ ينقل `next(req)` إلى `tokio::spawn` — أو ينافسُه بـ `select!` — قد يُكمل الطرف النهائي
بـ `101` ثم يردّ هو `202`، فيُخدَم `202` ولا يبدأ أي معالج جلسة. ووسيطٌ يبني مستقبَل السلسلة ثم
يُلقيه — دون أن يُنهيَه — يُخدَم رده هو ولا تبدأ جلسة. والآلية هنا مقيَسة بالعدسة المضادة لا
مفترَضة: استدعاء `next` يسجّل الطلب **بشكل متزامن**، لأن جسم إغلاق الطرف النهائي يعمل قبل أن يرجع
مستقبَله؛ فإلقاء المستقبَل لا يعني أن الطرف النهائي لم يُبلَغ، والذي يمنع التسليم هو **حالة الردّ**
الترجع به السلسلة لا بلوغ الطرف النهائي. لهذا كان الحارس مقارنة حالة (`chain_response.status == 101`)
ولم يكن علماً «طُرق الطرف النهائي». بدليل القياس: باستبدال الحارس بـ `.filter(|_| true)` — أي سلوك
«سلّم دائماً» القديم — يفشل اختبار التفرّع بـ `left: 101, right: 202` واختبار الإلقاء بـ
`left: 101, right: 503`، ويعبران مع الحارس الصحيح. أما استدعاء `next`
مرتين فغير قابل للكتابة أصلاً: `RequestContext` لا يشتق `Clone` و`next` يستهلكه، ولا مُنشئ عام له
(`from_parts` هو `pub(crate)`)، فالحالة خطأ ترجمة وليست سلوكاً غير مختبَر — والضمان النوعي أقوى من
اختبار.

حدود صريحة: الوسطاء لا يعدّلون `101` المُصنَّع — سطر المصافحة الذي تراه العميل عند ترقية **مقبولة**
تصنعه الواجهة البروتوكولية بعد رجوع السلسلة، فتمريرة CORS/ترويسات الأمان/الضغط لا تسري عليه. أما
`101` الذي يرجعه وسيطٌ من عند نفسه (`Response::status(101, ..)`) دون أن يصل الطرف النهائي، فهو
ادّعاءٌ بانتقال بروتوكولي لم يحدث: التسليم يحتاج سياق الطلب الذي يسجّله الطرف النهائي، ولا تسجيل هنا.
منذ 2026-10-22 ترفضه نواةُ الخدمة بـ `500 Internal Server Error` بدل أن ترسله، في فرعٍ واحد داخل
مسار WebSocket (`None if chain_response.status == 101 => Response::status(500, ..)`). القياسُ لا
التقدير: أُعطيَ الوسيطُ `101` يحمل `Upgrade` و`Connection` و`Sec-WebSocket-Accept` وجسماً JSON، فلم
يَنْجُ منه شيء — يستلم العميل `HTTP/1.1 500` بترويسات `X-Request-ID` و`Date` و`Content-Length: 0`
فقط، بلا جسم، وبلا جلسة، والمقبسُ نفسه يجيب طلباً تالياً بـ `200`. ويثبت الاختبارُ ذاته أن المسار
الصحيح سليم: من نفس الوسيط، حين يُستدعى `next` فعلاً، تخرج الترقية بـ `101` ورمز القبول القياسي في
RFC 6455 (`s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`) وتبدأ الجلسة ويُعاد الإطار المسموع. ومنذ track 58 صارت
القاعدة على القيمة لا على الفرع: مخرج الردّ الواحد في النواة يستبدل **كل** `101` لم تصنعه الواجهة
لتسليم مقبس، فيأخذ `101` الذي يرجعه معالج على مسار عادي الجواب نفسه
(`a_101_without_a_handoff_is_never_served`)، ويعطي HTTP/2 الـ`500` ذاته بدل إعادة تعيين الدفق
(`a_101_from_a_middleware_is_refused_over_h2`) — وهذان الاختباران يسقطان دون الفحص. ويبقى المحصورُ هو
الفئة: `103` أو `418` على مسار WebSocket ما تزال تخرج من المسار العادي، وتوسيعُ الشرط إلى
`status < 200` يُسقط الاختبار بـ `left: 500, right: 103`. أما مسار WebSocket عبر HTTP/2 وh2c فيبقى
مرفوضًا بـ `400` قبل السلسلة، ولم يتغيّر عدد اختبارات `http2_e2e` بهذا الرفض: كان 35 قبل اختبار
track 58 و36 بعده، ثم 37 بعد اختبار track 67 المصاحب لحدّ التقطير، ثم 43 بعد اختبارات نهايات الجسم
المجزّأ (track 99), و49 اليوم بعد اختبارات الجسم المجزّأ عبر h2 (track 100b) — إعادة عدّ 2026-10-30
(وكان الرقم المكتوب هنا 29، أي أنه كان قديمًا قبل هذا التغيير أيضًا).
فالقيمة `101` ما يُصدره الإطار عند تسليم مقبس، لا ادّعاءٌ يتمنّكه وسيط؛
والقرارُ وتنفيذُه موثّقان في `PRODUCTION_READINESS_REPORT.md` §15 و
`.project-governance/notes/2026-10-21-improvement-register.md` بندَي A4 وA6. كذلك لا يعني الرفضُ
إلغاءَ عملٍ بدءه الحارس: إسقاط `JoinHandle` لسلسلة مُنشأة بـ `tokio::spawn` لا يوقفها — العميل يُجاب
والمقبس لا يُسلَّم ولا تبدأ جلسة، لكن الوسطاء المُنشأ يستمر حتى ينتهي. إذن إذنُ التسليم قاعدةٌ على
الردّ لا مفتاح إيقاف؛ ومن يريد إيقاف العمل المُنشأ فعليه أن يلغيه بنفسه.
ولا يستطيع وسيطٌ إلغاء الترقية بتعديل ترويسات الطلب: منذ 0.2 لا حقلَ عاماً في
`RequestContext`، بل مخزنٌ واحد يُقرأ عبر `header`/`header_values`/`headers` فقط، فلا سطحَ
كتابةٍ يغيّر ما يقرأه المدقّق — الرفض يجب أن يكون استجابة تُعاد بها. وقد صار هذا **عقداً
موثّقاً** لا حادثة: كتلة `compile_fail,E0609` في وثائق `RequestContext` ترصد غياب الحقل،
ويؤكد اختبار السلك `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` أن السلسلة رأت
الأسطر وأن الوسيط لا يملك إلا الردّ. ولا شيء يلفّ الجلسة بعد تسليم المقبس. وهناك رفضان يسبقان
السلسلة
ولا قرار لطلبٍ وراءهما: `400` لمسار `app.ws` بلغه HTTP/2 (حاجز النقل `allows_upgrade`، RFC 7540 §8.1
يمنع الآلية هناك)، و`405` لغير `GET` على مسار `app.ws`. صار كلا الرافضين مُختبَراً على السلك بعدّاد
وسطاء (`a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain` واختبار HTTP/2 الممدّد
بنفس العدّاد)، وبُيّن أن العدّاد حيّ: تعطيل حاجز النقل جعل اختبار HTTP/2 يفشل بـ `left: 1, right: 0`
بينما بقيت الحالة `400` — أي أن الرواية القديمة التي تثبّت الحالة وحدها كانت عمياء عن هذا الخلل.
تصحيحٌ فرضه القياس: `405` لغير `GET` يصنعه **الموجِّه** لا نواة الخدمة، لأن `app.ws` يسجّل المسار
لـ `GET` فقط فلا يصل الطلب أصلاً إلى فرع WebSocket في `serve.rs`؛ فرعُ `serve.rs` حاجزٌ ثانٍ لا
يُبلَغ عبر الواجهة العامة، ولمّا جُعل قابلاً للبلوغ في بناءٍ مؤقت أجاب `405` قبل السلسلة فعلاً، وعطّلُه
سلّم `POST` بـ `101`. فالحارس سارٍ، والنِّسبة في المسودة السابقة كانت خاطئة. وانتبه
للفرق: `allows_upgrade` علمُ قدرةٍ على مستوى النقل لا فحصُ ترويسة لكل طلب، فـ `GET` عادي بلا
ترويسات ترقية على مسار `app.ws` **يمرّ بالسلسلة** و`400` يأتي من المدقّق بعدها — لا من حاجز قبلها.
**كسر سلوكي للمدمجين:** سلسلةُ تحققٍ كانت تحمي HTTP وحدها صارت الآن تحمي مسارات `app.ws` فعلاً،
فأي تطبيق كان يعتمد على مقبسٍ مفتوح خلف حارسٍ سيبدأ باستقبال رفض ذلك الحارس.

يمكن أيضاً استخدام محلل الإطارات المنخفض المستوى بدون أي حجز للذاكرة:

```rust
use aether_websocket::{parse_frame, WsOpcode};

let raw_ws_frame = [0x81, 0x05, 0x48, 0x65, 0x6C, 0x6C, 0x6F]; // Text Frame: "Hello"

if let Ok(Some((frame, consumed))) = parse_frame(&raw_ws_frame) {
    if frame.opcode == WsOpcode::Text {
        let text = std::str::from_utf8(frame.payload).unwrap();
        println!("WebSocket message received: {}", text);
    }
}
```

---

## 13. المولد التلقائي لمواصفات OpenAPI v3.1

> منذ المسار 108 (2026-10-31) لم يعد المخطط يدوياً بالضرورة: `AetherApp::openapi_spec(title,
> version)` تسأل الراوتر عن تسجيلاته (`RadixRouter::route_listing`) وتمرّرها إلى
> `aether::openapi::OpenApiSpec::from_routes`، فتنطبق الوثيقة على ما يجيبه الخادم فعلاً لا على ما
> يدّعيه ملف وصف. البناء اليدوي (`OpenApiSpec::new` مع `PathItem`/`Operation`) ما زال متاحاً وهو (ب).

### أ) الاشتقاق من تطبيق مسجَّل المسارات:

```rust
use aether::prelude::*;

let mut app = AetherApp::new();
app = app.get("/api/v1/users", |_req| "[]");
app = app.get(
    "/api/v1/users/{id}",
    |req| format!("user {}", req.param("id").unwrap_or("0")),
);
app = app.post("/api/v1/users", |_req| "created");
app = app.ws("/api/v1/live", |_req, _socket| async move {});

let spec = app.openapi_spec("Aether High-Performance REST API", "1.0.0");
let openapi_json = spec.to_json().unwrap();
```

ادعُ `openapi_spec` بعد تسجيل المسارات: ناتجها دالة في جدول المسارات وحده لا في ترتيب التسجيل،
فيُكتب في ملف داخل خطوة بناء ويُقاطع (diff) في المراجعة.

ما تضعه الوثيقة المشتقّة لكل تسجيل:

- مساره بقالب الراوتر نفسه، والقائمة مرتَّبة حسب `(المسار، الطريقة)`؛ `/assets/*filepath` يظهر
  بصيغة `{filepath}` لأن لا نموذج wildcard في OpenAPI.
- الطريقة بحرف صغير مفتاحاً داخل `PathItem`: خريطة `methods` لا حقلاً لكل فعل، حتى لا تُسقط
  الوثيقة تسجيلاً بطريقة لم يفكّر فيها من كتب الهيكل.
- `operationId` من الطريقة وكلمات القالب، مع `_2` ثم `_3` عند التصادم (`/a/b` و`/a_b` كلاهما
  `get_a_b`)؛ الرقم من الترتيب المرتَّب، فهو قابل لإعادة الإنتاج.
- كل `{slot}` في القالب يصبح معاملاً `in: path` مطلوباً من نوع `string`؛ و`{}` أو `{` بلا إغلاق
  لا يعلن شيئاً.
- مجموعة الحالات: `200` لمسار HTTP، و`101`/`400`/`426` لمسار `ws` المصنَّف بمعرّف المعالج لا
  بالطريقة.

ما لا تعرفه الوثيقة — لأن لا معالج يُقرأ، وجدول المسارات هو المصدر كله:

- لا مخطط لطلب ولا لاستجابة، ولا نوع محتوى، ولا الحالات التي يضيفها حارس أو `middleware` أو
  المعالج نفسه.
- `HEAD` و`OPTIONS` العام غائبتان: الخادم يجيبهما بلا تسجيل في الجدول.
- تسجيلان بطريقة `Method::Custom` بنفس اسم الطريقة على مسار واحد يندمجان في عملية واحدة، لأن
  خريطة الأفعال مفتاحها اسم الطريقة.

> الكتلتان فُحصتا بـ `cargo check` بتاريخ 2026-10-31 في حزمة مؤقتة تعتمد `aether` بمسار محلي
> (exit 0 بلا تحذيرات). السلوك نفسه مثبَّت بـ 8 اختبارات في
> `tests/tests/openapi_derivation.rs`، و7 اختبارات وحدة في `aether-openapi`، و4 في `aether-router`.

### ب) البناء اليدوي لوثيقة لا يولّدها الخادم:

```rust
use aether::openapi::{OpenApiSpec, PathItem, Operation, ResponseSpec};

let mut spec = OpenApiSpec::new("Aether High-Performance REST API", "1.0.0");

let mut get_users = Operation::default();
get_users.summary = "Retrieve all registered users".to_string();
get_users.responses.insert(
    "200".to_string(),
    ResponseSpec {
        description: "Successful response returning array of users".to_string(),
    },
);

let mut item = PathItem::default();
item.methods.insert("get".to_string(), get_users);
spec.paths.insert("/api/v1/users".to_string(), item);

let openapi_json = spec.to_json().unwrap();
```

> نصّ هذا المثال قبل المسار 108 كان `item.get = Some(get_users);` ولا يُترجم اليوم: `PathItem` لم
> يعد يحمل حقلاً لكل فعل بل `methods: BTreeMap<String, Operation>` خلف `#[serde(flatten)]`.

---

## 14. محرك التشخيص الذكي (Terminal Diagnostics)

> حزمة observability ومؤقّت RequestTimer الذي كان مثالها وحده حُذفا في المسار 111
> (2026-11-02): صفر اعتماديات وصفر اختبارات. التشخيص الموصول بالإطار هو
> `Diagnostic Engine` في `aether-core`، وهو ما يبقى في هذا القسم بعدها.

### محرك التشخيص الملون للطرفية (Rich Terminal Diagnostics):
```rust
use aether_core::diagnostic::{Diagnostic, ErrorCode};

let diag = Diagnostic::new(ErrorCode::E1001ServerStartupFailed, "TCP Bind failed: Address already in use")
    .with_context("Port", "3000")
    .with_hint("Port 3000 is occupied by another process.")
    .with_suggestion("Set AETHER_PORT=3001 to run on a different port.")
    .with_suggestion("Find what holds it: netstat -ano | findstr LISTENING | findstr :3000")
    .with_suggestion("Stop it: taskkill /PID <pid> /F, the PID being the last column above");

eprintln!("{}", diag.format_terminal(true));
```

> هذان السطران ليسا زينة: كانا يطبعان `lsof -ti:3000 | xargs kill -9`، وهما أمران لا وجود لهما على
> Windows وهو النظام الوحيد الذي يُبنى عليه هذا الإطار. الكود نفسه يبنيهما الآن من المنفذ الذي فشل
> ومن نظام التشغيل (`port_holder_advice` في `framework/aether/src/app.rs`)، والمرشّح `LISTENING`
> لازم لأن `netstat` بلا مرشّح يعيد أيضًا اتصالات `TIME_WAIT` إلى نفس المنفذ وعمود الـ PID فيها صفر.

---

## 15. خدمة الملفات الثابتة والأصول (Static Files & Asset Serving)

يوفر Aether طريقتين مريحتين للغاية لخدمة الملفات الثابتة (CSS, JS, صور, خطوط):

### أ) خدمة أصل برمجي من الذاكرة مباشرة (`app.static_asset`):
الأسرع على الإطلاق لتضمين ملفات الـ CSS والـ JS داخل التطبيق (Zero-Disk-I/O):
```rust
let learning_css = include_bytes!("../static/css/learning.css");
app = app.static_asset("/static/css/learning.css", "text/css; charset=utf-8", learning_css);
```

> `include_bytes!` مسارها نسبي إلى ملف المصدر الذي يكتبها، والمستودع هنا لا يحوي
> `framework/static/css/learning.css` — فالكتلة تُترجَم (compiles) في مشروع القارئ الذي يملك هذا
> الملف، وتفشل في هذا المستودع لو لُصقت كما هي. الوسيط الثالث يجب أن يبقى `&'static [u8]`
> (وهو ما ينتجه `include_bytes!`)، بينما `static_file` تأخذ مسار القرص كـ `&'static str`.

### ب) خدمة ملف مباشرة من القرص الصلب (`app.static_file`):
```rust
app = app.static_file("/favicon.ico", "public/favicon.ico");
app = app.static_file("/robots.txt", "public/robots.txt");
```

---

## 16. دمج محركات القوالب (Tera / Askama Template Engines)

يمكن دمج أي محرك قوالب HTML بسهولة تامة مع حالة التطبيق المشتركة `app.manage`.

> Aether لا يحتوي محرك قوالب ولا يعتمد على `tera`: لا توجد `tera` في أي `Cargo.toml` ضمن هذا
> المستودع، فمثال هذه القائمة يفترض أنك أضفتها إلى اعتماديات تطبيقك (`tera = "1"`). فُحصت الكتلة
> كلها بـ `cargo check` في crate مؤقت عند `tera = "1"`، أي أن وجه Aether فيها
> (`Response::html_dynamic(String)` و`Response::json_status(u16, &'static str, impl Into<Bytes>)`
> و`req.state::<AppState>()` و`app.get_async(.., |req: RequestContext| async move)`) ووجوه
> `Tera`/`Context` كلها اسم موجود وطوابعه مطابقة — لكن لا يعني ذلك أن المستودع يعتمد على `tera`.

```rust
use tera::{Tera, Context};

// 1. تعريف حالة التطبيق متضمنة Tera
struct AppState {
    tera: Tera,
}

// 2. دالة مساعدة لعرض قوالب Tera في استجابة Aether
fn render_template(tera: &Tera, template_name: &str, ctx: &Context) -> Response {
    match tera.render(template_name, ctx) {
        Ok(html_string) => Response::html_dynamic(html_string),
        Err(err) => {
            Response::json_status(500, "Internal Server Error", format!(r#"{{"error":"Template render error: {}"}}"#, err))
        }
    }
}

// 3. الاستخدام داخل معالجات المسارات
app = app.get_async("/dashboard", |req: RequestContext| async move {
    let state = req.state::<AppState>().expect("State missing");
    let mut ctx = Context::new();
    ctx.insert("page_title", "لوحة التحكم");
    ctx.insert("user_id", &101);
    render_template(&state.tera, "dashboard.html", &ctx)
});
```

---

## 17. مشروع إنتاجي متكامل شامل لجميع العناصر (Full Production Blueprint)

إليك كود خادم ويب إنتاجي متكامل جاهز للتشغيل يوضح دمج جميع الميزات في ملف واحد:

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{sleep, Duration};

// 1. حالة التطبيق المشتركة
struct ProductionState {
    service_name: &'static str,
    active_requests: AtomicU64,
}

// 2. نماذج البيانات (DTOs)
#[derive(Deserialize)]
struct CreateAccountDto {
    username: String,
    email: String,
}

#[derive(Serialize)]
struct AccountResponseDto {
    id: u64,
    username: String,
    email: String,
    created_at_epoch_ms: u128,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تسجيل الحالة المشتركة
    app = app.manage(ProductionState {
        service_name: "Aether Enterprise Microservice",
        active_requests: AtomicU64::new(0),
    });

    // تفعيل البرمجيات الوسيطة والأمان
    app = app.enable_security_headers();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allow_origin: "*",
        allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
        allow_headers: "Content-Type, Authorization, X-Request-ID",
        max_age_secs: 86400,
        ..CorsConfig::default()
    }));

    // الصفحة الرئيسية HTML
    app = app.get("/", |_req| {
        Response::html(concat!(
            "<!DOCTYPE html><html lang='ar' dir='rtl'><head><meta charset='utf-8'>",
            "<title>⚡ Aether Enterprise Core</title>",
            "<style>body{background:#0b0f19;color:#fff;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}.c{text-align:center;padding:40px;background:#111827;border-radius:16px;border:1px solid #1f2937}h1{color:#38bdf8}a{display:inline-block;margin:8px;padding:8px 16px;background:#2563eb;color:#fff;text-decoration:none;border-radius:6px}</style></head>",
            "<body><div class='c'><h1>⚡ خادم Aether الفائق يعمل الآن!</h1><p>خادم HTTP/1.1 و HTTP/2 على نفس المنفذ</p><a href='/api/v1/health'>Health Check</a><a href='/api/v1/users/101'>User 101</a></div></body></html>"
        ))
    });

    // تجميع مسارات API v1
    app = app.group("/api/v1", |v1| {
        // فحص الصحة مع قراءة الحالة
        v1.get("/health", |req| {
            let state = req.state::<ProductionState>().unwrap();
            let count = state.active_requests.fetch_add(1, Ordering::Relaxed) + 1;
            format!(
                r#"{{"status":"healthy","service":"{}","served_count":{}}}"#,
                state.service_name, count
            )
        });

        // إنشاء مستخدم جديد عبر POST مع قراءة الـ JSON
        v1.post_async("/accounts", |req: RequestContext| async move {
            let payload: CreateAccountDto = match req.json() {
                Ok(dto) => dto,
                Err(_) => return Response::status(400, "Invalid JSON body"),
            };

            // محاكاة معالجة غير متزامنة
            sleep(Duration::from_millis(2)).await;

            let response_dto = AccountResponseDto {
                id: 99401,
                username: payload.username,
                email: payload.email,
                created_at_epoch_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            };

            let out_bytes = serde_json::to_string(&response_dto).unwrap();
            Response::json(bytes::Bytes::from(out_bytes))
        });

        // جلب حساب محدد بالـ Dynamic Parameter
        v1.get_async("/users/{id}", |req: RequestContext| async move {
            let user_id = req.param("id").unwrap_or("0");
            sleep(Duration::from_millis(1)).await;
            format!(r#"{{"user_id":"{}","status":"active"}}"#, user_id)
        });
    });

    // تشغيل الخادم على العنوان والمنفذ المحددين
    app.listen("0.0.0.0:3000").await
}
```

---

## 18. إنهاء TLS على نفس المنفذ (`aether::tls`) 🔒

الطبقة مبنية على `rustls` + `tokio-rustls` مع مزوّد التشفير `ring` (تُجنَّب `aws-lc-rs` لأنها
تُصرِّف شيفرة C عبر cmake، وهذا يجعل دعم Windows يعتمد على سلسلة أدوات خارجية). القرار بشأن
التشفير يُتَّخذ **لكل اتصال** لا لكل مستمع: أول ثلاث بايتات إن كانت `0x16 0x03` فالاتصال TLS،
وإلا فهو HTTP/1.1، لهذا يخدم المنفذ الواحد `http://` و`https://` معاً.

```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // كل دوال التسجيل وبنائات TLS تستهلك التطبيق وتعيده (`self -> Self`)، فيُسلسَل الإعداد
    // كله — المسارات والنقل — في تعبير واحد. بصيغة الجملة يُعاد الربط: `app = app.get(...)`.
    let app = AetherApp::new()
        .get("/ping", |_| "pong")
        // الشهادة أولاً ثم المفتاح الخاص؛ المفتاح المُشفَّر أو غير المطابق لسلسلة الشهادات
        // يُرفض هنا في وقت البناء، لا عند أول client hello.
        .tls_from_files("/etc/ssl/leaf.pem", "/etc/ssl/leaf.key")?
        // اختياري: ارفض الاتصالات النصية بدل أن تجيبها على نفس المنفذ
        .require_tls(false);

    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

### أ) أسماء متعددة على منفذ واحد (SNI)
```rust
use aether::{TlsAcceptor, TlsCertSet, TlsIdentity};

let base = TlsCertSet::from_files("default.pem", "default.key")?;
let api  = TlsCertSet::from_files("api.pem", "api.key")?;

let acceptor = TlsAcceptor::builder()
    .insert(TlsIdentity::default_identity(base))          // الـ catch-all: مطلوب واحد بالضبط
    .insert(TlsIdentity::for_name("api.example.com", api)?)
    .require_tls(true)
    .build()?;
```
اسم المضيف يُطبَّع (حروف صغيرة، نقطة الجذر النهائية تُزال، رُفوض عنوان IP)، و`client hello`
باسم غير معروف أو بدون SNI يأخذ الـ catch-all بدل أن يفشل المصافحة. إذا لم تكن المائدة تحتوي
catch-all فإن `build()` يُرجع خطأً بدل أن يبني خادماً يُسقط اتصالات عملاء لا يعرفهم.

### ب) إعادة تحميل الشهادات بدون إعادة تشغيل
```rust
if let Some(acceptor) = app.tls_acceptor() {
    let fresh = TlsCertSet::from_files("new-leaf.pem", "new-leaf.key")?;
    acceptor.replace_certificates(vec![TlsIdentity::default_identity(fresh)])?;
}
```
`replace_certificates` يتحقق من المائدة الجديدة كاملةً ثم يُبدّل `Arc` واحداً: الفشل يترك
الشهادة الحيّة كما هي، والاتصال الذي بدأ المصافحة قبل التبديل يبقى على النسخة التي أخذها.

### ج) حدود هذه الطبقة (لا تُوصف كميزة موجودة)
- **ALPN يُعلن `h2` ثم `http/1.1`.** جسر HTTP/2 مُنفَّذ (§19)، فالإعلان يعكس ما يخدمه الخادم
  فعلاً. `TlsAcceptorBuilder::advertise_h2(false)` تُثبّت المنفذ على HTTP/1.1 وحده لبيئة تريد
  ترميزاً واحداً على السلك.
- **HTTP/2 فوق النص الصريح اختيارِي:** بدون `AetherApp::enable_h2c` فإن بادئة `PRI * HTTP/2.0`
  تُغلَق ولا تُجاب، لأن سطر حالة HTTP/1 هناك شيء لا يستطيع العميل تحليله.
- **الإبطال مُطبَّق محلياً بلا جلب:** `TlsCertSet::with_crl` يتحقق من توقيع القائمة بمُصدر
  السلسلة وصلاحيتها ويرفض الورقة المدرجة، و`TlsCertSet::with_ocsp` لا يُلصق إلا استجابة ناجحة
  تغطي هذه الورقة وتكون حالية وموقّعة من المُصدر أو من مُستجيب مُجاز بـ `id-kp-OCSPSigning`؛
  المادة غير الصالحة تُرفض عند التحميل. أما التحديث فمهمة المشغّل عبر `replace_certificates`.
- **التحقق من الشهادة مسؤوليته العميل**: هذه الطبقة تحمّل مادة الخادم فقط، ولا تثق بأي عميل
  (no client auth).
- **المفاتيح الخاصة:** تُقرأ من القرص أو من بايتات في الذاكرة؛ لا شيء منها يُخزَّن في المستودع،
  واختبارات `tls_e2e` تولد CA وورقة كل تشغيل بـ `rcgen` (اعتماد تطويري فقط).
- **`WebSocket<TcpStream>` → `WebSocket<aether::transport::Stream>`:** كسر في الواجهة العامة
  لأن الجلسة المُرقَّاة قد تجري فوق مقبس TLS؛ الإصلاح اسمُ نوع واحد، و`Stream` مُصدَّر من
  جذر الصناديق ومن `prelude`.

الأدلة: 25 اختبار وحدة في `framework/aether/src/tls.rs` (15 منها للإبطال: ستة CRL وتسعة OCSP) و11 اختبار أسلاك حقيقية في
`framework/tests/tests/tls_e2e.rs` تشغّل مصافحة فعلية مقابل `AetherServer` حيّ: نص + TLS على
منفذ واحد، SNI، إعادة تحميل، رفض النص عند `require_tls`، وWSS حتى إطار الإغلاق.

---

## 19. الواجهة الأمامية HTTP/2 (`aether::http2`) 🧵

HTTP/2 في Aether **جسر** لا محرك مكتوب من الصفر: `h2` 0.4.20 يملك آلة حالة المسارات وHPACK
وترميز الإطارات وحساب تدفق النوافذ وPING/SETTINGS، وما يُكتب هنا هو الترجمة — رأس الطلب الذي
يُسلّمه `h2` يتحوّل إلى نفس `RequestParts` الذي يُنتجه مسار HTTP/1.1، فيُجاب الطلب الواحد من
تنفيذ واحد.

### أ) متى يُخدم HTTP/2
قرار الترميز يُتَّخذ **مرة واحدة** في `transport::establish` ويُعيد `Frontend::{Http1, Http2}`؛
حلقة الخادم تُوزّع على قيمتها ولا تُخمّن من المقبس لاحقاً (المقبس النصّي لا ALPN له ليُبلَغ عنه):
- **فوق TLS:** عندما يتفق ALPN على `h2` (الإعلان الافتراضي `h2` ثم `http/1.1`).
- **فوق النص الصريح:** فقط على منفذ اختار ذلك صراحة، بالمعرفة القَبْلية من RFC 9113 §3.3:
```rust
let mut app = AetherApp::new();
app = app.get("/ping", |_| Response::text("pong"));
// نفس المنفذ: اتصال يفتح بالبادئة يُخدَم HTTP/2، وأي شيء آخر يبقى HTTP/1.1.
let server = app.enable_h2c().bind("127.0.0.1:8443").await?;
```
الإطفاء: `TlsAcceptorBuilder::advertise_h2(false)` تُثبّت TLS على HTTP/1.1، و`require_tls(true)`
تُغلق كل اتصال نصّي — بما فيه اتصال يحمل بادئة HTTP/2.

### ب) ما يملكه الخادم
- **المال:** كل مسار مُقبَل يُعيد تسعير تسجيل الاتصال بما يحمله فعلاً (`STREAM_BASE_MEMORY`
  زائد بايتات الجسم وصولاً إليها)، فمسار لا يغطيه ميزانية الذاكرة يُرفض بـ 503 بينما تكمل
  المسارات الحيّة على نفس المقبس — دون أن يُسرق منها.
- **رأس الرد** يُصفّى ضد `FORBIDDEN_FIELDS` قبل السلك، لأن `h2` يُجيب حقلًا خاصّاً بالاتصال
  بخطأ مستخدم يُصفّر المسار: معالج وضع `Connection: close` لأجل HTTP/1 لا يجب أن يدمّر ردّه نفسه
  فوق h2. اتجاه **الطلب** لا يحتاج تصفية: `h2` يرفض تلك الحقول قبل المعالج (مُتحقَّق بإطارات
  HEADERS مكتوبة يدوياً).
- **المصافحة** محدودة بـ `read_timeout` على المسارين، فالعميل الذي يرسل البادية ثم يتوقف يُغلق
  ولا يُحتجز.
- **الإغلاق** يبدأ بـ `graceful_shutdown()` (GOAWAY) ويظل يقبل ويستفرّط حتى تنهي المسارات
  الجارية داخل نفس ميزانية التصريف التي يعطيها المنفذ لمقبس يصرف.

### ج) حدود هذه الطبقة (لا تُوصف كميزة موجودة)
- **`Upgrade: h2c` غير مُنفَّذ وعن قصد:** RFC 9113 §3.1 يُهمل رمز الترقية هذا و`HTTP2-Settings`،
  وRFC 7540 §3.2 يطلب أن يُجاب طلب الترقية نفسه كمسار 1 نصف-مغلق ضمنيّاً — حالة لا تُصنعها
  `h2`، فتطبيقها يعني كتابة التبديل يدوياً فوق حلقة HTTP/1. العميل الذي يريد HTTP/2 نصيّاً يرسل
  البادية.
- **WebSocket لا يصل عبر HTTP/2:** لا آلية ترقية في RFC 9113 §8.1، فمسار `app.ws` المُبلَّغ عبر
  h2 يُجاب **400** بدل أن يتظاهر بالتبديل.
- **لا دفع من الخادم (server push):** مُهمل في RFC 9113 §3.6 و§10.8.
- **كل شيء مُخمَّد:** الأجسام والأجزاء تُقرأ كاملة قبل المعالج، كما في HTTP/1.

الأدلة: 14 اختبار وحدة في `framework/aether/src/http2.rs` (معدودة من سماتها في 2026-10-30:
5 لرأس الطلب، 6 لتسعير جسم الاستجابة، 1 لحدّ التصريف، و2 لمسار الرفض وصياغة 413) و49 اختبار سلك حقيقي في
`framework/tests/tests/http2_e2e.rs` (معدودة من سمات الاختبار نفسها في 2026-10-30:
`grep -c tokio::test`): أكثرها يفتح اتصالاً عبر `H2Client::connect` فوق TLS بـ ALPN `h2` أو عبر
`H2Client::connect_plain` فوق النص الصريح h2c، وبعضها يكتب الإطارات بنفسها أو يفتح مقبساً خاماً
(إغلاق البادئة، واختبارا حدّ التفرّع اللذان يشتركان في مساعد تجاوز واحد، وإطارات HEADERS وRST_STREAM
يدوية)، ثم تشغيل
المثال `cargo run -p aether-tests --example h2_interop_server -- 18443` و`... 18444 h2c` مقابل
عملاء لا يستخدمون نفس المكتبات: Node.js 24.18 (`http2`/nghttp2) فوق TLS، وpython-h2 + hyperframe +
hpack فوق النص الصريح (20/20)، وإطارات HEADERS مكتوبة يدوياً بـ hpack (8/8) لحقول RFC 9113 §8.2.2.
**Linux وmacOS لم يُختبَرا بعد** — كل ما سبق قياس على Windows.

---

تم إنشاء هذا التوثيق ليكون دليلاً مرجعياً دائماً داخل مجلد التوثيق في المشروع (`framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`).
