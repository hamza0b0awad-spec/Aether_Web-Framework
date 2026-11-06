# دليل المواصفات والوظائف الشامل لإطار عمل Aether (بالعربية)

توثيق هندسي ومعماري شامل لكافة وظائف، كائنات، دوال، وأنظمة أمان إطار عمل **Aether** المبني بلغة Rust لتطوير خدمات الويب فائقة الأداء والأمان.

---

## 📑 فهرس الأنظمة والوظائف

1. [معمارية خط المعالجة غير المتزامن (Execution Pipeline)](#1-معمارية-خط-المعالجة-غير-المتزامن-execution-pipeline)
2. [طبقة المقابس والتأطير التراكمي لـ TCP (Adaptive Two-Tier TCP Framing)](#2-طبقة-المقابس-والتأطير-التراكمي-لـ-tcp-adaptive-two-tier-tcp-framing)
3. [محلل HTTP فائق السرعة والأمان (Zero-Copy Parser & RFC 9112)](#3-محلل-http-فائق-السرعة-والأمان-zero-copy-parser--rfc-9112)
4. [محرك التوجيه الراديكسي ومطابقة الأساليب بالـ Bitmask ($O(K)$ Router)](#4-محرك-التوجيه-الراديكسي-ومطابقة-الأساليب-بالـ-bitmask-ok-router)
5. [سياق الطلب وإدارة الذاكرة (`RequestContext`)](#5-سياق-الطلب-وإدارة-الذاكرة-requestcontext)
6. [بناء الاستجابات والتسلسل المباشر للذاكرة (`Response`)](#6-بناء-الاستجابات-والتسلسل-المباشر-للذاكرة-response)
7. [حقن التبعيات وإدارة الحالة المشتركة (`AppStateContainer` / DI DAG)](#7-حقن-التبعيات-وإدارة-الحالة-المشتركة-appstatecontainer--di-dag)
8. [منظومة البرمجيات الوسيطة (CORS، ترويسات الأمان، وتتبع الطلبات)](#8-منظومة-البرمجيات-الوسيطة-cors-ترويسات-الأمان-وتتبع-الطلبات)
9. [حدود الخادم ومكافحة هجمات Slowloris وحجب الخدمة (`ServerLimits`)](#9-حدود-الخادم-ومكافحة-هجمات-slowloris-وحجب-الخدمة-serverlimits)
10. [منظومة إدارة الأخطاء ورموز الحالة (`AetherError`)](#10-منظومة-إدارة-الأخطاء-ورموز-الحالة-aethererror)
11. [أمثلة برمجية شاملة للمطورين (Production Examples)](#11-أمثلة-برمجية-شاملة-للمطورين-production-examples)

---

## 1. معمارية خط المعالجة غير المتزامن (Execution Pipeline)

يعتمد Aether على خط معالجة مباشر غير متزامن (Fully Async Pipeline) بدون أي أقفال حابسة (Zero Global Contention):

```text
 اتصال TCP جديد من العميل
            │
            ▼
┌─────────────────────────────────────────┐
│ 1. حارس التزامن الذري (Semaphore RAII)  │  -> تحديد سقف الاتصالات (افتراضياً 10,000)
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 2. بافر القراءة التراكمي (BytesMut)     │  -> قراءة مدمجة مع مهلة Read Timeout
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 3. محلل الطلب بدون نسخ (Incremental)    │  -> `parse_request_incremental` (`framework/http/src/parser.rs:748`) بمطابقة RFC 9112 وحظر Request Smuggling
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 4. شجرة التوجيه الراديكسية ($O(K)$)     │  -> مطابقة الأسلوب بالـ Bitmask واستخراج المعلمات
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 5. تنفيذ البرمجيات الوسيطة (Middleware) │  -> CORS، ترويسات الأمان، ونظام التتبع
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 6. تنفيذ معالج المسار (Async Handler)   │  -> كود التطبيق المطور باستخدام RequestContext
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 7. تحويل الاستجابة مباشرة للبافر        │  -> write_http1_to بدون تخصيص Strings وسيطة
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 8. إرسال البيانات وإفراغ المقبس (Flush) │  -> مطبق مع Write Timeout وتحرير فوري للذاكرة
└─────────────────────────────────────────┘
```

---

## 2. طبقة المقابس والتأطير التراكمي لـ TCP (Adaptive Two-Tier TCP Framing)

### طبيعة التحدي في شبكات TCP:
بيانات طلبات الـ HTTP قد تصل مجزأة عبر حزم (Packets) متعددة، أو قد تصل عدة طلبات معاً في دفعة واحدة (HTTP Pipelining).

### الحل الهندسي المعتمد:
- **المسار السريع (Fast-Path Stack Buffer):** قراءة سريعة في مصفوفة 8KB على الـ Stack.
- **البافر التراكمي المستمر (`BytesMut`):** إذا رجّعت `parse_request_incremental` قيمة `Ok(None)` (طلب جزئي)، تُحفظ البايتات في `conn_buf` وتُنتظر الحزمة التالية (`app.rs:2639` لموضع النداء، و`app.rs:3038-3059` للمهلة — وهي `keep_alive_timeout` ما إن يصبح المقبس ساكناً بين طلبين، لا `read_timeout`).
- **الاقتطاع الذري (`split_to`):** عند اكتمال طلب بحجم $N$ بايت، يتم اقتطاع الحجم بدقة $O(1)$ دون نسخ، وإعادة فحص الباقي فوراً للطلبات التالية في وضع Keep-Alive.

---

## 3. محلل HTTP فائق السرعة والأمان (Zero-Copy Parser & RFC 9112)

يقوم `RequestView` باستعارة بايتات الطلب ونصوص الترويسات مباشرة من بافر المقبس بدون أي تخصيص على الـ Heap:

### آليات الحماية الصارمة (RFC 9112):
1. **منع هجوم تكرار Content-Length (Dual CL):** رفض أي طلب يحتوي على أكثر من ترويسة أو قيم مفصولة بفواصل فوراً بـ `400 Bad Request`.
2. **منع هجوم الجمع بين Transfer-Encoding و Content-Length (CL.TE / TE.CL):** حظر فوري لأي محاولة دمج لضمان عدم اختراق الـ Reverse Proxies.
3. **التحقق من الحدود الحجمية:** فحص صارم لحجم الترويسات ضد `max_header_bytes` وحجم الجسم المفكوك ضد `max_body_bytes`. وHTTP/1.1 يرفض قبل ذلك على بايتات الأسلاك عند مجموع `max_header_bytes + max_body_bytes`، لذلك جسم chunked يبلغ سقف المقبس قبل أن يبلغ حدّ المحلّل (`docs/07` §4، ومثبَّت باختبار).
4. **أشكال هدف الطلب (RFC 9112 بند 3.2):** لا يوجَّه إلا هدفٌ يقع في أحد الأشكال الأربعة المعرّفة: `/path`، أو `http(s)://authority/path`، أو سلطة `CONNECT`، أو النجمة `*` لـ `OPTIONS` على مستوى الخادم. وكل ما عدّاها `400` لا مسار: قبل هذه القاعدة كان `GET hello` يطابق المسار `/hello`. والهدف المطلق هو الذي يعطي `Host` (بند 3.2.2) فيُستبدل الحقل الوارد به لتقرأ قائمةُ السماح بالـ `Host` وCORS والمعالِجون المضيفَ نفسه، مع بقاء غياب `Host` على HTTP/1.1 رفضاً بـ `400` وبقاء أكثر من حقل `Host` واحداً رفضاً في كل الإصدارات. وأما `CONNECT` الذي يحمل سلطةً صحيحة فيُجاب `501 Not Implemented`، لأن هذه الطبقة لا تقيم نفقاً وRFC 9110 بند 9.3.6 يخيّر بين `405` و`501` لمن لا يدعم النفق؛ وسبقَ قرارُ الأمان جوابَ القدرة، فـ `Host` خارج قائمة السماح تُجاب `400` قبله. والأشكال الأربعة هي ما يحلّ محلّها `:path` في HTTP/2 (RFC 9113 بند 8.3.1): `OPTIONS` على مستوى الخادم يصل بـ `:path` قيمتُها `*` فيُجاب `204` بـ `Allow` كما على HTTP/1، والنجمة على طريقةٍ أخرى تُجاب `400` ولا تُوجَّه كمسار، و`:path` الفارغة أو المطلقة أو التي لا تبدأ بـ `/` معطوبةٌ (بند 8.1.1) فلا تكلّف أكثر من إعادة ضبط `PROTOCOL_ERROR` على تدفقها، و`CONNECT` — الذي يُسقط `:scheme` و`:path` ويُغلق تدفقَه بعد قيام النفق فقط (بند 8.5) — يُجاب `501` من الرأس ولا ينتظر جسماً لا يرسله عميلٌ مطابق. وفرقٌ واحد لا تملكه هذه الطبقة: `h2` تحذف شظية `#` من `:path` قبل تسليم الطلب، فلا نظيرَ لرفض HTTP/1 لها هنا، والمورد المحدَّد يبقى واحداً للبروتوكولين.

---

## 4. محرك التوجيه الراديكسي ومطابقة الأساليب بالـ Bitmask ($O(K)$ Router)

### الخوارزمية وهيكل البيانات:
- **Radix Trie مع ترتيب العقد:** تخزين المسارات كشجرة مقاطع مفروزة حتمياً:
  1. **المسارات الثابتة (Static)** أولاً (مثل `/users/profile`).
  2. **المعلمات المتغيرة (Parameterized)** ثانياً (مثل `/users/{id}` — الأقواس المعقوفة هي الصيغة
     الوحيدة؛ `:id` مقطع حرفي).
  3. **المسارات الشاملة (Wildcard / Catch-All)** أخيراً (مثل `/static/*file`).
- **تجزئة بدون Heap Allocations:** استخدام `SmallVec<[&str; 8]>` على الـ Stack لمطابقة مقاطع المسار في $O(K)$ حيث $K$ هو عمق المسار فقط.
- **مطابقة أسلوب الطلب بتعليمة معالج واحدة (64-bit Bitmask):** يشغل كل أسلوب بتاً واحداً من حقل
  `u64` اسمه `allowed_methods` على عقدة الـ Trie، والفحص هو
  (`framework/http/src/method.rs:25-43` و`framework/router/src/node.rs:42-44`):
  ```rust
  (self.allowed_methods & method.bitmask()) != 0
  ```

---

## 5. سياق الطلب وإدارة الذاكرة (`RequestContext`)

كائن `RequestContext` يوفر للمطور واجهة نظيفة مع إدارة ذاكرة منضبطة (تم إلغاء `Box::leak` بالكامل لضمان عدم حدوث أي تسريب ذاكرة):

### دوال الاستخراج المتاحة:

| الدالة | التوقيع البرمجي (Signature) | الوظيفة |
| :--- | :--- | :--- |
| `param` | `pub fn param(&self, key: &str) -> Option<&str>` | استخراج قيمة معلمة مسار من صيغة `{id}` (المفتاح `id`). |
| `param_parse` | `pub fn param_parse<T: FromStr>(&self, key: &str) -> Option<T>` | استخراج وتحويل معلمة المسار لنوع محدد (`u32`, `bool`, إلخ). |
| `query` | `pub fn query(&self, key: &str) -> Option<&str>` | استخراج معلمة من الـ Query String (`?page=2`). |
| `header` | `pub fn header(&self, name: &str) -> Option<&str>` | قراءة قيمة ترويسة معينة (غير حساسة لحالة الأحرف). |
| `cookie` | `pub fn cookie(&self, name: &str) -> Option<&str>` | استخراج قيمة Cookie معينة من ترويسة `Cookie` كما كتبها العميل. |
| `signed_cookie` | `pub fn signed_cookie(&self, signer: &CookieSigner, name: &str) -> SignedCookie` | قراءة كوكي موقَّع؛ يفصل `Present` عن `Absent` و`Invalid` و`Ambiguous`، والاسم داخل الوسم. |
| `body_str` | `pub fn body_str(&self) -> Result<&str, Utf8Error>` | قراءة جسم الطلب كنص UTF-8 بدون نسخ. |
| `json` | `pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error>` | فك تشفير JSON مباشرة من بايتات الجسم إلى Struct. |
| `state` | `pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>` | استرجاع خدمة أو حالة مشتركة من حاوية الـ DI. |
| `request_id` | `pub fn request_id(&self) -> &str` | الحصول على معرّف التتبع الفريد للطلب. |

---

## 6. بناء الاستجابات والتسلسل المباشر للذاكرة (`Response`)

### دوال إنشاء الاستجابات:
- `Response::ok()`: استجابة `200 OK` **فارغة** — لا تقبل جسمًا؛ دوال البناء بالأسفل هي التي تضبط
  `Content-Type` و`Content-Length`.
- `Response::text(&'static str)` و`Response::text_dynamic(impl Into<String>)`: نص `text/plain`.
- `Response::json(Bytes)` و`Response::json_status(u16, &'static str, impl Into<Bytes>)`: تضيف
  `Content-Type: application/json`. **لا تُسلسل شيئًا بنفسها** — المستدعي يمرر البايتات
  (`serde_json::to_string(&data).unwrap().into()`)، ولهذا لا وجود لدالة `Response::json(&serializable)`.
- `Response::status(code, text)`: إنشاء استجابة برمز حالة مخصص (مثل `201 Created` أو `404 Not Found`).

### التحويل المباشر للبافر (`write_http1_to`):
تتم كتابة الترويسات وجسم الاستجابة مباشرة إلى الـ `BytesMut` الخاص بالمقبس عبر `bytes::BufMut::writer` بدون إنشاء أي `String` مؤقتة أو استدعاء `format!()` على الـ Heap.

---

## 7. حقن التبعيات وإدارة الحالة المشتركة (`AppStateContainer` / DI DAG)

يوفر Aether حاوية حقن تبعيات آمنة للـ Threads تعتمد على `TypeId` لاسترجاع الخدمات في $O(1)$.
التسجيل عبر `app.manage(...)` والقراءة داخل أي معالج عبر `ctx.state::<T>()` الذي يعيد `Arc<T>`:

```rust
let mut app = AetherApp::new();

// تسجيل الخدمات
app = app.manage(DatabaseConnectionPool::new());
app = app.manage(RedisCacheClient::new());

// استدعاؤها داخل أي Handler
app = app.get_async("/items", |ctx: RequestContext| async move {
    let db = ctx.state::<DatabaseConnectionPool>().unwrap();
    let _ = db;
    Response::text("Items fetched")
});
```

لا وجود لحقل عام باسم `app.state_container` ولا لدالة `AetherApp::with_state(..)`؛ `manage` هي طريق
التسجيل الوحيدة.

---

## 8. منظومة البرمجيات الوسيطة (Middleware)

- **`CorsMiddleware`**: ضبط سياسات النطاقات المسموحة، الأساليب، والترويسات.
- **`SecurityHeadersMiddleware`**: تفعيل ترويسات الحماية (HSTS, Frame-Options `DENY`, Content-Type `nosniff`, CSP).
- **`LoggerMiddleware`**: مبني بتجريد صفري التكلفة (`#[inline(always)]`) ومعطل افتراضياً على الـ Hot Path لمنع خنق الأداء بسبب أقفال `stdout` العامة.

---

## 9. حدود الخادم ومكافحة هجمات Slowloris وحجب الخدمة (`ServerLimits`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // الحد الأقصى للترويسات (8 KB افتراضياً)
    pub max_body_bytes: usize,       // حدّ حجم الجسم المفكوك (2 MB افتراضياً)
    pub max_headers_count: usize,    // الحد الأقصى لعدد الترويسات (100 ترويسة)
    pub max_concurrent_streams: u32, // أقصى عدد تدفقات HTTP/2 متزامنة لكل اتصال (100)
    pub max_connections: usize,      // سقف الاتصالات المتزامنة (10,000 اتصال)
    pub read_timeout: Duration,      // مهلة قراءة الحزمة (5 ثوانٍ لمكافحة Slowloris)
    pub write_timeout: Duration,     // مهلة كتابة الاستجابة (10 ثوانٍ)
    pub keep_alive_timeout: Duration,// مهلة إبقاء الاتصال (60 ثانية)
    pub request_timeout: Duration,   // مهلة تنفيذ المعالج (30 ثانية)
    pub tls_handshake_timeout: Duration, // مهلة إتمام مصافحة TLS (10 ثوانٍ)
}
```

---

## 10. منظومة إدارة الأخطاء ورموز الحالة (`AetherError`)

أصناف العدّاد هي `HeaderTooLarge` و`BodyLimitExceeded` و`MalformedRequest` و`RouteNotFound` و
`MethodNotAllowed` و`ValidationFailed(String)` و`DependencyCycleDetected(String)` و`Timeout` و`Io`
و`Serialization(String)` و`Custom(String)` (`framework/core/src/error.rs:5-19`)، ولا وجود لصنف
`PayloadTooLarge`. أما ما تفعله حلقة التحليل بها فهو مطابق بثلاثة فروع في
`framework/aether/src/app.rs:2991-2999`:

- `AetherError::HeaderTooLarge` -> ينتج `431 Request Header Fields Too Large`.
- `AetherError::BodyLimitExceeded` -> ينتج `413 Content Too Large`.
- أي خطأ تحليل آخر -> ينتج `400 Bad Request`.
- الثلاثة تُكتب مع `Connection: close` (`app.rs:3000`) فلا يُعاد استخدام المقبس بعد خطأ تأطير.
- `AetherError::Timeout` لا يمرّ عبر هذا المطابقة: المعالج الذي يتجاوز `request_timeout` يُجاب
  `504 Gateway Timeout` مع `Connection: close` من طبقة الخدمة (`framework/aether/src/serve.rs:972-977`)
  ويُلغى عمله المُنشأ (`abort_handle.abort()`).

---

## 11. أمثلة برمجية شاملة للمطورين (Production Examples)

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Product {
    id: u64,
    name: String,
    price: f64,
}

#[derive(Deserialize)]
struct CreateProductDto {
    name: String,
    price: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. مسار استعلام بسيط
    app = app.get("/", |_ctx| Response::text("محرك Aether يعمل بكفاءة قصوى"));

    // 2. مسار ديناميكي مع معلمة مسار
    app = app.get_async("/products/{id}", |ctx: RequestContext| async move {
        let product_id = ctx.param_parse::<u64>("id").unwrap_or(0);
        let product = Product {
            id: product_id,
            name: format!("Product #{}", product_id),
            price: 49.99,
        };
        Response::json(serde_json::to_string(&product).unwrap().into())
    });

    // 3. مسار استقبال وفك تشفير JSON
    app = app.post_async("/products", |ctx: RequestContext| async move {
        match ctx.json::<CreateProductDto>() {
            Ok(dto) => {
                let product = Product {
                    id: 101,
                    name: dto.name,
                    price: dto.price,
                };
                Response::json_status(
                    201,
                    "Created",
                    serde_json::to_string(&product).unwrap(),
                )
            }
            Err(_) => Response::status(400, "Bad Request: Invalid JSON Body"),
        }
    });

    println!("🚀 الخادم يعمل على http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

حُصّر هذا المثال بالـ `cargo check` بتاريخ 2026-10-22 داخل مشروع مؤقت يعتمد على `aether` بالمسار
(معه `serde` و`serde_json` و`tokio`)، وخرج بنتيجة exit 0 بلا تحذيرات. كان المثال السابق لا يُصرَّف
ولا يمكن أن يُصرَّف: `aether::context` و`http::response::Response` ليستا مساراتنا (الـ prelude يعيد
تصدير `aether_http::response::Response`)، و`Response::ok(body)` لا تقبل جسمًا، ولا وجود لـ
`Response::json(&serializable)` ولا لـ `Response::status(..).with_json(..)`، وأن `/products/:id`
مقطع **حرفي** لأن الـ trie يعتبر `{name}` فقط معلمة، و`app.get` مع `async move` مرفوضة —
الصيغتان غير المتزامنتين هما `get_async` و`post_async`. نوع خطأ `main` هو
`Box<dyn Error + Send + Sync>` لأن `listen` تُرجعه؛ `Box<dyn Error>` وحدها لا تُصرَّف مع `?`.
