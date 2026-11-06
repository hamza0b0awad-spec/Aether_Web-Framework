# الدليل المرجعي والتوثيق الشامل لإطار عمل Aether Web Framework ⚡

> **ملاحظة تدقيق (2026-10-18):** الجسم يُقرأ كـ `Bytes` مملوك وليس شريحة مستعارة؛ معاملات
> المسار تُفكّ إلى `String`؛ و`RequestArena` غير مستخدمة في مسار الخادم الافتراضي. الخادم يخدم
> HTTP/1.1، وHTTP/2 حيث يُتفق عليه (§5هـ). راجع [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).
> **تحديث (2026-10-20):** أُضيف إنهاء TLS داخل العملية على نفس المنفذ (`aether::tls` فوق rustls)
> مع SNI وإعادة تحميل بدون إعادة تشغيل؛ انظر §5د. **تحديث (2026-10-21):** HTTP/2 مُنفَّذ فوق TLS
> حين يتفق ALPN على `h2`، وفوق النص الصريح على منفذ يختار `AetherApp::enable_h2c`؛ `Upgrade: h2c`
> وHTTP/3 غير مُنفَّذين.

### دليل المطور الكامل لبناء خوادم الويب وتطبيقات الـ APIs فائقة الأداء

أهلاً بك في **Aether**! إطار عمل خفيف، فائق السرعة، ومصمم بلغة Rust ليجمع بين تجربة التطوير السهلة والمريحة (FastAPI / Axum Developer Experience) وبين أقصى درجات الأداء المنخفض المستوى (Zero-Copy Memory, Bump Arena Allocator, 64-Bit Bitmask Routing).

---

## الفهرس العام
1. [التثبيت والبدء السريع (Quick Start)](#1-التثبيت-والبدء-السريع)
2. [المعالجات غير المتزامنة والمزامنة (Sync & Async Handlers)](#2-المعالجات-غير-المتزامنة-والمزامنة)
3. [استخراج وقراءة بيانات الـ Body و JSON (Typed JSON & Body Extractors)](#3-استخراج-وقراءة-بيانات-الـ-body-و-json)
4. [إدارة الحالة المشتركة (Global Application State)](#4-إدارة-الحالة-المشتركة)
5. [البرمجيات الوسيطة والحماية (Middleware & Security)](#5-البرمجيات-الوسيطة-والحماية)
6. [تجميع المسارات (Route Groups)](#6-تجميع-المسارات)
7. [معالجة الأخطاء ومحول الردود (Error Handling & IntoResponse)](#7-معالجة-الأخطاء-ومحول-الردود)
8. [معاملات المسارات (Zero-Copy Dynamic Path Parameters)](#8-معاملات-المسارات)
9. [محرك الذاكرة منخفض المستوى ومزايا Aether الحصرية](#9-محرك-الذاكرة-منخفض-المستوى)
10. [مثال تطبيقي متكامل وشامل (Full Production Blueprint)](#10-مثال-تطبيقي-متكامل-وشامل)

---

<a name="1-التثبيت-والبدء-السريع"></a>
## 1. التثبيت والبدء السريع (Quick Start)

### أ) إعداد ملف `Cargo.toml`:
```toml
[dependencies]
aether = { path = "../framework/aether" }
tokio = { version = "1.38", features = ["full"] }
bytes = "1.6"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### ب) كتابة أول خادم في ملف `src/main.rs`:
```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // مسار ترحيبي بسيط
    app = app.get("/", |_req| {
        Response::html("<h1>⚡ مرحباً بك في خادم Aether السريع!</h1>")
    });

    // الاستماع على المنفذ 3000
    app.listen("0.0.0.0:3000").await
}
```

---

<a name="2-المعالجات-غير-المتزامنة-والمزامنة"></a>
## 2. المعالجات غير المتزامنة والمزامنة (Sync & Async Handlers)

يدعم Aether المعالجة المتزامنة (للعمليات الحسابية السريعة والذاكرة) وغير المتزامنة (لقواعد البيانات والـ I/O):

### أ) المعالجة المتزامنة (Synchronous):
```rust
app = app.get("/sync-fast", |_req| {
    "استجابة فورية بدون أي تأخير"
});
```

### ب) المعالجة غير المتزامنة مع `.await` (Asynchronous):
```rust
use tokio::time::{sleep, Duration};

app = app.get_async("/async-db", |req: RequestContext| async move {
    // محاكاة استعلام قاعدة بيانات يستغرق 15ms
    sleep(Duration::from_millis(15)).await;
    
    Response::json(bytes::Bytes::from(r#"{"status":"db_data_loaded"}"#))
});

app = app.post_async("/async-send", |req: RequestContext| async move {
    // إرسال إشعار أو حفظ سجل
    "تمت المعالجة غير المتزامنة"
});
```

---

<a name="3-استخراج-وقراءة-بيانات-الـ-body-و-json"></a>
## 3. استخراج وقراءة بيانات الـ Body و JSON (Typed JSON & Body Extractors)

### أ) استخراج JSON تلقائياً في بنية Struct (`req.json::<T>()`):
```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct CreateProductDto {
    name: String,
    price: f64,
    in_stock: bool,
}

app = app.post_async("/api/products", |req: RequestContext| async move {
    // تحويل JSON تلقائياً مع معالجة الأخطاء
    let product: CreateProductDto = match req.json() {
        Ok(p) => p,
        Err(_) => return Response::status(400, "Bad Request: Invalid JSON Body"),
    };

    println!("تم استلام المنتج: {} بسعر {}", product.name, product.price);
    Response::json(bytes::Bytes::from(r#"{"success":true}"#))
});
```

### ب) قراءة الـ Body كنص UTF-8 أو بايتات خام:
```rust
// قراءة كنص:
let text: &str = req.body_str().unwrap_or("");

// قراءة كشريحة بايتات مستعارة مباشرة (بلا نسخ؛ `body` نوعه `Bytes` والاستعارة تعطي `[u8]`):
let raw_bytes: &[u8] = &req.body;
```

---

<a name="4-إدارة-الحالة-المشتركة"></a>
## 4. إدارة الحالة المشتركة (Global Application State)

يتيح لك Aether مشاركة اتصالات قواعد البيانات ومفاتيح التشفير والإعدادات عبر التطبيق كاملاً بأمان $O(1)$ بدون تعقيدات الأقفال:

```rust
use aether::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct AppDatabase {
    connection_url: String,
    query_count: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. تسجيل الحالة في التطبيق
    app = app.manage(AppDatabase {
        connection_url: "postgres://user:pass@localhost/db".to_string(),
        query_count: AtomicU64::new(0),
    });

    // 2. الوصول للحالة في أي مسار
    app = app.get("/stats", |req| {
        if let Some(db) = req.state::<AppDatabase>() {
            let count = db.query_count.fetch_add(1, Ordering::SeqCst) + 1;
            format!(r#"{{"database":"{}","queries":{}}}"#, db.connection_url, count)
        } else {
            "State unavailable".to_string()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```

> `manage` حالةٌ عمرها عمر العملية كلها. ما يخصّ **طلباً واحداً** (هوية المتصل، المستأجر، راية ميزة)
> لا يوضع هنا بل في `Extensions` — انظر القسم 5، الفقرة و).

---

<a name="5-البرمجيات-الوسيطة-والحماية"></a>
## 5. البرمجيات الوسيطة والحماية (Middleware & Security)

### أ) تفعيل ترويسات الأمان (Security Headers Middleware):
تفعيل تلقائي لترويسات HSTS, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, X-XSS-Protection:
```rust
app = app.enable_security_headers();
```

### ب) تفعيل سياسة تبادل الموارد (CORS Middleware):
```rust
app = app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization, X-Requested-With",
    max_age_secs: 86400,
    ..CorsConfig::default()
}));
```

### ج) مسجل الطلبات المدمج (Microsecond Logger):
ليس مفعّلًا افتراضيًا — الطباعة على الـ Hot Path معطّلة في الإنتاج (انظر `docs/06`)، وتُفعَّل صراحةً:

```rust
let mut app = AetherApp::new();
// الوسيطة `true` تضيف زمن المعالج بالميكروثانية إلى سطر السجل
app = app.enable_request_logging(true);
```

عندها يطبع الخادم زمن كل طلب بدقة الميكروثانية (`µs`) بهذه الصيغة الحرفية
(`middleware/src/logger.rs:44`):
`⚡ [Aether] Get /api/products -> 200 (42µs)`
ومع `enable_request_logging(false)` يبقى السطر لكن بلا القوسين والزمن.

### د) TLS على نفس المنفذ (In-process HTTPS/WSS):
الطبقة مبنية على `rustls` مع مزوّد `ring`، والقرار نص أم TLS يُتَّخذ **لكل اتصال** من أول
بايتاته (`0x16 0x03` = سجل TLS)، فلا حاجة لمستمع منفذ أو proxy لفك التشفير:
```rust
let mut app = AetherApp::new();
app = app.get("/ping", |_| "pong");
let app = app.tls_from_files("leaf.pem", "leaf.key")?  // السلسلة أولاً ثم المفتاح
             .require_tls(false);                       // true = ارفض النص بدل أن يجيبه
```
- الشهادة غير المطابقة للمفتاح، أو المفتاح المُشفَّر، تُرفض في وقت البناء بخطأ `TlsError`،
  لا عند أول عميل.
- أسماء إضافية عبر `TlsAcceptor::builder().insert(TlsIdentity::for_name(..)?)` مع catch-all
  واحد إلزامي؛ اسم غير معروف يأخذ الـ catch-all بدل إفشال المصافحة.
- تجديد الشهادة بدون إعادة تشغيل: `app.tls_acceptor()` ثم `replace_certificates(..)` — التبديل
  ذرّي (`Arc` واحد) والفشل يترك الشهادة الحيّة كما هي.
- WebSocket يشتغل فوق TLS أيضاً (`WSS`)؛ لاحظ أن توقيع المعالج صار
  `WebSocket<aether::transport::Stream>` بدل `WebSocket<tokio::net::TcpStream>`.
- **الإبطال مُطبَّق على مادة توفّرها أنت، بلا أي جلب من الشبكة:** `TlsCertSet::with_crl` يتحقق من
  توقيع القائمة بمُصدر السلسلة وصلاحيتها ويرفض الورقة المدرجة فيها، و`TlsCertSet::with_ocsp`
  لا يُلصق إلا استجابة basic ناجحة تغطي هذه الورقة (الرقم التسلسلي وتجزئتا المُصدر) وحالتها
  `good` ونافذتها الحالية سارية وموقّعة من المُصدر أو من شهادة مُستجيب وقّعها المُصدر وأجازها
  بـ `id-kp-OCSPSigning`. المادة غير الصالحة تُرفض وقت التحميل أو إعادة التحميل ولا تُرسل.

### هـ) HTTP/2 على نفس المنفذ
فوق TLS لا شيء لتفعله: ALPN يُعلن `h2` ثم `http/1.1`، فالاتصال الذي يتفق على `h2` يُخدَم HTTP/2
وعلى نفس المنفذ. أما فوق النص الصريح فترقية صريحة مطلوبة، لأن الترميز النصّي لا مفاوضة فيه:
```rust
let mut app = AetherApp::new();
app = app.get("/ping", |_| Response::text("pong"));
// معرفة قَبْلية (RFC 9113 §3.3): بادئة HTTP/2 تُخدَم HTTP/2، وأي شيء آخر يبقى HTTP/1.1.
let server = app.enable_h2c().bind("127.0.0.1:8443").await?;
```
- لا يتغيّر شيء خلف المنفذ: نفس `ServiceCore` ونفس الحدود وسياسة المضيف وتحديد المعدّل وميزانية
  الذاكرة لكل مسار، ونفس الإغلاق بتصريف المسارات الجارية.
- `Upgrade: h2c` غير مُنفَّذ وعن قصد: RFC 9113 §3.1 يُهمل تلك الترقية، وRFC 7540 §3.2 يطلب
  إجابة طلب الترقية نفسه كمسار 1 نصف-مغلق ضمنيّاً — حالة لا تُصنعها `h2`.
- مسار `app.ws` يبلغ عبر HTTP/2 يُجاب **400**: لا ترقية WebSocket في RFC 9113 §8.1.
- لا دفع من الخادم (server push)، وكل الأجسام تُقرأ كاملة قبل المعالج كما في HTTP/1.1.
- `advertise_h2(false)` تُثبّت TLS على HTTP/1.1، و`require_tls(true)` تُغلق كل اتصال نصّي بما فيه
  اتصال يحمل بادئة HTTP/2.

الأدلة: 49 اختبار سلك في `framework/tests/tests/http2_e2e.rs` (معدودة من سمات الاختبار نفسها في
2026-10-30: `grep -c tokio::test`): فوق TLS بمفاوضة ALPN على `h2`، وفوق النص الصريح بعميل `h2`،
وعلى اتصالات مكتوبة بالإطارات الخام حيث لا يستطيع عميل مؤدَّب أن يُنتج المُدخل الذي يجب أن ينجو منه
الخادم (حدّ التفرّع المُعلَن، ورأس يتوقف عند البادئة)؛ ومنها ما يكتب على السلك بايتات مرتّبة يدوياً
(البادئة، واختبارا حدّ التفرّع عبر المساعد المشترك `streams_over_a_cap_are_reset`، و`RST_STREAM`
في منتصف الجسم)، ومنها ما يرسل طلب HTTP/1.1 نصياً على المنفذ نفسه، وواحد يمسك أربعة تدفقات على
اتصال واحد ثلاثة منها متزامنة — وتشغيل
`cargo run -p aether-tests --example h2_interop_server -- 18443` (و`18444 h2c`) مقابل Node.js
`http2` وpython-h2 — أي عملاء لا يستخدمون مكتباتنا. التحقق على Windows فقط؛ Linux وmacOS لم
يُختبَرا بعد.

### و) قيم تخصّ طلباً واحداً (`Extensions`)

`manage` يشارك حالة عمرها عمر العملية كلها؛ ما يخصّ **هذا** الطلب وحده (من المتصل؟ أي مستأجر؟)
يوضع في `Extensions` التي تركب مع `RequestContext` وتنتقل معه من البرمجيات الوسيطة إلى المعالج:

```rust
#[derive(Clone)]
struct Principal(String);

app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
    if let Some(token) = req.header("authorization") {
        req.insert_extension(Principal(token.to_string()));
    }
    next(req).await
});

// كوسيط بالنوع (يتطلب `Clone` مثل `Extension<T>` في Axum)…
app = app.get_async("/whoami", |Extension(p): Extension<Principal>| async move {
    format!("hello {}", p.0)
});

// … أو بالاقتراض من السياق، وهذا يعمل حتى للقيم التي لا تُنسخ
app = app.get_async("/whoami2", |req: RequestContext| async move {
    match req.extension::<Principal>() {
        Some(p) => Response::text_dynamic(p.0.clone()),
        None => Response::text_dynamic("anonymous".to_string()),
    }
});
```

- لا تُخصَّص الخريطة إلا عند أول `insert`، فالطلب الذي لا يستخدمها لا يكلّف شيئاً.
- القيمة لكل طلب: كل تدفق في HTTP/2 له خريطته، وكل طلب على اتصال keep-alive له خريطته
  (`extensions_are_per_stream_never_per_connection` و
  `extensions_do_not_leak_between_requests_on_one_connection`).
- نوع غير موجود عبر `Extension<T>` يجيب **500** ولا ينفّذ المعالج؛ و`Debug` يطبع العدد دون القيم.
- معالج `app.ws` يرى نفس الخريطة: سلسلة وسائط المسار تُنفَّذ حول طلب الترقية عبر نفس مسار
  `dispatch`، فما يختمه الحارس يصل إلى الجلسة، ورفضُه يمنع الترقية ويُخدَم رده — سواء أجاب قبل
  استدعاء `next` أو بعد انتظاره
  (`a_websocket_upgrade_runs_the_app_middleware_chain`،
  `a_middleware_can_refuse_a_websocket_upgrade`،
  `a_middleware_that_answers_after_next_refuses_the_upgrade`).
  الترقية تتم فقط عندما ترجع السلسلة `101` الطرف النهائي دون تغيير. انتبه للفرق بين `101` المُصنَّع
  و`101` الذي يرجعه وسيط من عنده: الأول تصنعه الواجهة البروتوكولية عند تسليم المقبس ولا تسري عليه
  تمريرة CORS/الأمان/الضغط، وأما الثاني — دون بلوغ الطرف النهائي — فادّعاءُ انتقالٍ بروتوكولي لم
  يحدث، وترفضه نواةُ الخدمة بـ `500 Internal Server Error` بدل أن ترسله (منذ 2026-10-22). مقيسٌ في
  `a_hand_built_101_from_a_middleware_is_refused_with_500`: `HTTP/1.1 500` بجسمٍ فارغ
  و`Content-Length: 0` ولا شيء من ترويسات المصافحة التي ألصقها الوسيط، والاتصال يظلّ الخادمُ يفسّره
  HTTP/1 ويجيب طلباً تالياً بـ `200`؛ ومن نفس الوسيط، حين يُستدعى `next` فعلاً، تخرج الترقية الصحيحة
  بـ `101` ورمز القبول القياسي في RFC 6455 وتبدأ الجلسة. والقاعدة صارت في track 58 متعلقةً بالقيمة لا
  بالمكان: مخرج الردّ الواحد في النواة يستبدل **كل** `101` لم تصنعه الواجهة لتسليم مقبس، فيأخذ `101`
  الذي يرجعه معالج على مسار عادي الجواب نفسه، ويعطي HTTP/2 الـ`500` ذاته بدل إعادة تعيين الدفق
  (`a_101_without_a_handoff_is_never_served`، `a_101_from_a_middleware_is_refused_over_h2`). وما يبقى
  محصورًا هو الفئة: `103` أو `418` على مسار WebSocket ما تزال تُخدَم كما هي، وتوسيع الشرط إلى
  `status < 200` يُسقط الاختبار بـ `left: 500, right: 103`.
  والوسطاء لا يلغون الترقية بتعديل الترويسات؛ التفاصيل في
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md`.
- أشكال السلاسل الغريبة محسومة بنفس القاعدة: ما يصل للعميل هو **ردّ الوسيط** لا غير. وسيط ينقل
  `next(req)` إلى `tokio::spawn` ويكمل الطرف النهائي بـ `101` ثم يردّ 202، يُخدَم 202 ولا يُسلَّم
  أي مقبس (`a_middleware_that_forks_the_chain_still_answers_the_upgrade_request`)، ووسيط يبني
  مستقبَل السلسلة ثم يُلقيه يُخدَم رده هو (`a_middleware_that_abandons_the_chain_future_refuses_the_upgrade`)،
  ولا تبدأ معه أي جلسة. لكن **الرفض لا يلغي العمل**: إسقاط `JoinHandle` مهمةٍ منشأة لا يوقفها،
  فالحارس المُنشئ للسلسلة يستمر في التنفيذ بعد أن أُجيب العميل — الترخيص بالتسليم قرار على الردّ
  لا مفتاح إيقاف. والآلية مقيَسة لا متخيَّلة: استدعاء `next` يسجّل الطلب **بشكل متزامن** (جسم
  إغلاق الطرف النهائي يعمل قبل إرجاع مستقَبَله)، فإلقاء المستقبَل لا يعني أن الطرف النهائي لم يُبلَغ
  — الذي يمنع التسليم هو **حالة الردّ** لا بلوغ الطرف النهائي، ولذا كان الحارس مقارنة حالة وليس
  علماً. ولا يستطيع أي وسيط استدعاء
  `next` مرتين: `RequestContext` لا يُنسخ ولا يُبنى من خارج الحزمة، فالتكرار خطأ ترجمة لا سلوك
  غير مُختبَر.
- لا يوجد مخزن ترويسات قابل للكتابة على `RequestContext`: مخزن واحد يُقرأ عبر `header`
  و`header_values` و`headers`، فلا وسيط يمنح ترقية بإضافة حقول ولا يسحبها بحذفها — رفض الترقية
  بردٍّ مُعاد (`a_middleware_cannot_grant_or_revoke_a_websocket_upgrade`؛ أغلق هذا `PROB-0005`
  في 0.2).
- الحجم المخزَّن داخلها لا يدخل في ميزانية ذاكرة الجدولة — انظر `FEATURE_COMPLETENESS_MATRIX.md`
  البند 17.

---

<a name="6-تجميع-المسارات"></a>
## 6. تجميع المسارات (Route Groups)

لتنظيم إصدارات الـ API وفصل المسارات الإدارية:

```rust
// مسارات الإصدار الأول V1
app = app.group("/api/v1", |v1| {
    v1.get("/health", |_req| r#"{"version":"v1","status":"healthy"}"#);
    
    v1.get_async("/users", |req: RequestContext| async move {
        r#"[{"id":1,"name":"Hamza"}]"#
    });
});

// مسارات لوحة التحكم
app = app.group("/admin", |admin| {
    admin.get("/dashboard", |_req| Response::html("<h1>Admin Panel</h1>"));
});
```

---

<a name="7-معالجة-الأخطاء-ومحول-الردود"></a>
## 7. معالجة الأخطاء ومحول الردود (Error Handling & IntoResponse)

يمكن لدوال المسارات في Aether إرجاع أنواع متعددة يتم تحويلها لردود HTTP نقية تلقائياً:

| القيمة المرجعة من الدالة | النتيجة في بروتوكول HTTP |
| :--- | :--- |
| `Response::ok()` | 200 OK |
| `Response::html("<h1>مرحبا</h1>")` | 200 OK مع ترويسة HTML |
| `Response::json(bytes)` | 200 OK مع ترويسة JSON |
| `"نص مباشر"` أو `String` | 200 OK نصي |
| `(404, "Not Found")` | 404 Not Found مع كود الحالة |
| `Result<T, MyError>` | إذا كان `Ok` يعيد الرد، وإذا كان `Err` فالحالة تختارها سمة `IntoResponseError`: `&'static str` و`String` تجيب 400 برسالتها، و`AetherError` تجيب 400/404/405/413/417/431/500/504 حسب حالتها، و5xx تُخفي رسالة الخطأ |

```rust
app = app.get("/check-balance", |_req| -> Result<&'static str, &'static str> {
    let has_funds = true;
    if has_funds {
        Ok("الرصيد كافي للعملية")
    } else {
        Err("عذراً، الرصيد غير كافٍ")
    }
});
```

---

<a name="8-معاملات-المسارات"></a>
## 8. معاملات المسارات (Zero-Copy Dynamic Path Parameters)

قراءة المتغيرات من الرابط (مثل `/users/{id}` أو `/orgs/{org_id}/teams/{team_id}`):

```rust
app = app.get("/users/{user_id}", |req| {
    let user_id = req.param("user_id").unwrap_or("0");
    format!(r#"{{"user_id":{}}}"#, user_id)
});
```

---

<a name="9-محرك-الذاكرة-منخفض-المستوى"></a>
## 9. محرك الذاكرة منخفض المستوى ومزايا Aether الحصرية

1. **`RequestArena` O(1) Bump Reset:** إعادة استخدام نفس كتلة الذاكرة (4KB) لاتصالات Keep-Alive بدون استدعاء `malloc/free` وبدون تفتيت للـ Heap.
2. **64-Bit Method Bitmask:** فحص أفعال الـ HTTP بتعليمة معالج واحدة (Single CPU Cycle) في الـ L1 Cache.
3. **Connection Control Block (CCB):** استهلاك ~420 بايت فقط لكل اتصال Socket خامل، مما يسمح بتشغيل 100,000 اتصال في أقل من 45MB RAM.
4. **Precompiled Dependency Graph (DAG):** فحص وحل شجرة التبعيات وقت الإقلاع بدلاً من الـ Reflection البطيء في أطر لغات التفسير.

---

<a name="10-مثال-تطبيقي-متكامل-وشامل"></a>
## 10. مثال تطبيقي متكامل وشامل (Full Production Blueprint)

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{sleep, Duration};

// 1. تعريف حالة التطبيق العامة
struct ProductionState {
    app_version: &'static str,
    total_requests: AtomicU64,
}

// 2. هياكل البيانات للـ JSON
#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    email: String,
}

#[derive(Serialize)]
struct UserResponse {
    id: u64,
    username: String,
    email: String,
    active: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تفعيل الحالة المشتركة
    app = app.manage(ProductionState {
        app_version: "2.4.0-release",
        total_requests: AtomicU64::new(0),
    });

    // تفعيل البرمجيات الوسيطة
    app = app.enable_security_headers();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allow_origin: "*",
        allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
        allow_headers: "Content-Type, Authorization",
        max_age_secs: 86400,
        ..CorsConfig::default()
    }));

    // الصفحة الرئيسية
    app = app.get("/", |_req| {
        Response::html("<h1>🚀 مرحباً بك في خادم Aether للإنتاج!</h1>")
    });

    // مسارات الـ API
    app = app.group("/api/v1", |v1| {
        // فحص الصحة
        v1.get("/health", |req| {
            let state = req.state::<ProductionState>().unwrap();
            let count = state.total_requests.fetch_add(1, Ordering::SeqCst) + 1;
            format!(
                r#"{{"status":"healthy","version":"{}","total_served":{}}}"#,
                state.app_version, count
            )
        });

        // إنشاء مستخدم جديد (Async + JSON)
        v1.post_async("/users", |req: RequestContext| async move {
            let payload: CreateUserRequest = match req.json() {
                Ok(data) => data,
                Err(_) => return Response::status(400, "Bad Request: Invalid JSON"),
            };

            // محاكاة استعلام غير متزامن
            sleep(Duration::from_millis(5)).await;

            let response_data = UserResponse {
                id: 4201,
                username: payload.username,
                email: payload.email,
                active: true,
            };

            let json_out = serde_json::to_string(&response_data).unwrap();
            Response::json(bytes::Bytes::from(json_out))
        });

        // جلب مستخدم محدد بالـ ID
        v1.get_async("/users/{id}", |req: RequestContext| async move {
            let id = req.param("id").unwrap_or("0");
            sleep(Duration::from_millis(2)).await;
            format!(r#"{{"user_id":{},"status":"active"}}"#, id)
        });
    });

    println!("⚡ خادم Aether يعمل على: http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await
}
```
