# دليل معالجة الأخطاء والتشخيص في إطار عمل Aether ⚡

> **ملاحظة تدقيق (2026-10-18):** لافتة الخادم الواردة أدناه قديمة. اللافتة الحالية تصرّح بأن
> المشغّل هو Tokio متعدد الخيوط (work-stealing) وأن المخصص هو مخصص النظام، وليس
> Thread-per-Core أو RequestArena. راجع
> [PRODUCTION_READINESS_REPORT.md](PRODUCTION_READINESS_REPORT.md).

### (Error Handling, Diagnostic Engine & Terminal UX Guide)

يقدم **Aether** نظام تشخيص أخطاء متطور مستوحى من أفضل ممارسات هندسة الأنظمة (System Diagnostics)، مصمم لتزويد المطور بإجابات فورية وقابلة للتنفيذ عند حدوث أي خطأ، دون التضحية بأداء الإنتاج أو تسريب البيانات الحساسة.

---

## 📑 الفهرس
1. [جدول أكواد الأخطاء القياسية (Standard Error Codes: E1001 - E5002)](#1-جدول-أكواد-الأخطاء-القياسية)
2. [تصميم مخرجات الطرفية (Terminal Diagnostic UX)](#2-تصميم-مخرجات-الطرفية)
3. [معالجة الأخطاء في وضع التطوير مقابل الإنتاج (Dev vs Prod)](#3-معالجة-الأخطاء-في-وضع-التطوير-مقابل-الإنتاج)
4. [معرف التتبع للطلبات (Request ID & Trace Correlation)](#4-معرف-التتبع-للطلبات)
5. [أمثلة عملية لحل المشاكل الشائعة (Troubleshooting Recipes)](#5-أمثلة-عملية-لحل-المشاكل-الشائعة)

---

<a name="1-جدول-أكواد-الأخطاء-القياسية"></a>
## 1. جدول أكواد الأخطاء القياسية (Standard Error Codes)

| كود الخطأ | العنوان | السبب الشائع | الحل المقترح |
| :--- | :--- | :--- | :--- |
| **`E1001`** | `ServerStartupFailed` | المنفذ (Port) محجوز بواسطة تطبيق آخر أو عدم وجود صلاحيات | تغيير المنفذ عبر `AETHER_PORT` أو إيقاف العملية الحاجزة |
| **`E1002`** | `InvalidConfig` | قيمة متغير بيئة غير صالحة (مثل منفذ غير رقمي) | تصحيح قيمة المتغير في ملف `.env` أو إعدادات النظام |
| **`E2001`** | `RouteCollision` | تعارض مسارين بنفس النمط في شجرة الـ Radix | إعادة تسمية المسار المتعارض أو تنظيم المسارات في `group` |
| **`E2002`** | `RouteNotFound` | تم طلب مسار غير مسجل في السيرفر (HTTP 404) | التحقق من صحة رابط الـ URL أو إضافة مسار مخصص |
| **`E2003`** | `MethodNotAllowed` | الفعل المستخدم (مثل `POST`) غير مفعل للمسار (HTTP 405) | تفعيل الفعل المطلوب عبر `app.post(...)` أو تعديل طلب العميل |
| **`E3001`** | `MalformedRequest` | طلب HTTP غير متوافق مع قواعد RFC 9112 أو محاولة تهريب (Smuggling) | التحقق من سلامة عميل HTTP والتأكد من إرسال ترويسات قياسية |
| **`E3002`** | `PayloadLimitExceeded` | حجم الترويسات أو جسم الطلب تجاوز الحد المسموح | زيادة `max_body_bytes` في `AetherConfig` |
| **`E3003`** | `UnsupportedExpectation` | حقل `Expect` يسمّي توقّعًا لا يلبّيه الخادم (RFC 9110 §10.1.1)، أو قائمة تخلط متوقّعًا مدعومًا (`100-continue`) بمتوقّع غير مدعوم — والقائمة كلّها وعد واحد | أرسل `Expect: 100-continue` وحده أو احذف الحقل. الرفض `417` يصدر من رأس الطلب قبل قراءة أي بايت من الجسم، على الواجهتين HTTP/1.1 وHTTP/2، ولهذا كان الغرض من التوقّع محفوظًا: جسمٌ لا يحتاج الخادم إليه لا يُرسَل |
| **`E4001`** | `DeserializationFailed` | حقول الـ JSON لا تتطابق مع بنية الـ Struct في Rust | مطابقة أسماء وأنواع الحقول في الـ Struct مع الـ Payload |
| **`E4002`** | `ValidationFailed` | فشل في قيود التحقق من صحة المدخلات (تحقّق صريح من التطبيق أو `aether-validation`؛ لا فحص متجهي في مسار HTTP) | تصحيح البيانات المدخلة لتوافق الشروط المطلوبة |
| **`E4003`** | `HandlerReturnedError` | الـ Handler أرجع `Err` وخطؤه اختار حالة **4xx** لا يغطّيها رمز أدقّ: `&'static str` و`String` (400) و`AetherError::Custom` (400) — كلها تُرسل رسالتها على السلك (`aether/src/app.rs:123-222`) | طبّق `IntoResponseError::error_status` على نوع خطأك لتحصل على حالة أخرى، أو أرجع `Response` مباشرة. يثبّته `a_handler_error_answers_400_and_says_so_in_the_body` |
| **`E5001`** | `DependencyCycle` | حلقة مغلقة دائرية في شجرة حقن التبعيات (DAG) | فك الاعتمادية الدائرية بين الخدمات في الـ Graph؛ وإن أرجعه الـ Handler كخطأ فهو **500** مع `E5001` ورسالة مخفّاة |
| **`E5002`** | `InternalServerError` | خطأ Handler اختار حالة **5xx**: `AetherError::Io` ← 500 و`AetherError::Timeout` ← 504 (`aether/src/app.rs:188-203`، والرموز في `core/src/error.rs:37` و`core/src/error.rs:41`). صار يُرسل على السلك منذ 2026-10-24، بينما كان رمزًا لا يُرسل أبدًا لأن `Err` كلها كانت تُجبر على 400 مع `E4003` | الحالة 5xx تُخفي رسالة الخطأ ويحمل الجسم عبارة `"Internal Server Error"`؛ ومنذ 2026-10-27 لا تُهمَل الرسالة المخفية: تُسلَّم إلى مراسل الأخطاء إن ثُبِّت عبر `aether::set_error_reporter`، مقترنة بمعرّف `X-Request-ID` نفسه (انظر §أ) |

---

### أ) عقد استجابة أخطاء الـ Handler: سمة `IntoResponseError`

التطبيق العام الذي كان `impl<T: IntoResponse, E: Display + Send> IntoResponse for Result<T, E>`
يخمّن: كل `Err` بلا استثناء كان يجيب **400**. الآن الطلب صار `E: IntoResponseError`
(`aether/src/app.rs:123-166`)، والسمة تُصرّح بالحالة بدل أن تُخمَن، والجدول التالي هو العقد كاملا
كما ينفّذه `app.rs:168-265`:

| نوع الخطأ | الحالة | `code` في الجسم | `error` في الجسم |
| :--- | :--- | :--- | :--- |
| `&'static str` ، `String` | 400 | `E4003` | الرسالة نفسها |
| `AetherError::MalformedRequest` | 400 | `E3001` | الرسالة نفسها |
| `AetherError::Serialization(_)` | 400 | `E4001` | الرسالة نفسها |
| `AetherError::ValidationFailed(_)` | 400 | `E4002` | الرسالة نفسها |
| `AetherError::Custom(_)` | 400 | `E4003` | الرسالة نفسها |
| `AetherError::RouteNotFound` | 404 | `E2002` | الرسالة نفسها |
| `AetherError::MethodNotAllowed` | 405 | `E2003` | الرسالة نفسها |
| `AetherError::BodyLimitExceeded` | 413 | `E3002` | الرسالة نفسها |
| `AetherError::UnsupportedExpectation` | 417 | `E3003` | الرسالة نفسها |
| `AetherError::HeaderTooLarge` | 431 | `E3002` | الرسالة نفسها |
| `AetherError::DependencyCycleDetected(_)` | 500 | `E5001` | `"Internal Server Error"` |
| `AetherError::Io(_)` | 500 | `E5002` | `"Internal Server Error"` |
| `AetherError::Timeout` | 504 | `E5002` | `"Gateway Timeout"` |

لسمةٍ واحدة مطلوب تطبيقها (`error_status`) وثلاث قيم اختيارية: `error_code` التي تُشتقّ افتراضيًا
من صنف الحالة فلا تحمل استجابة 5xx رمزا 4xx ولا العكس، و`error_detail` الذي يُخفي الرسالة عندما
يكون نصّها تفصيلا داخليا، و`redacted_detail` التي تُسلّم الرسالة المخفية إلى مراسل الأخطاء. الحذف
في حالات 5xx مقصود: رسالة `Io` قد تسمّي جدولا أو مسارا أو منفذا؛ لكنها لا تُهمَل، بل تخرج من السلك
وتدخل قناة تشخيص:

```rust
aether::set_error_reporter(|r: &aether::ErrorReport| {
    // r.request_id هو نفس X-Request-ID في الاستجابة، r.status و r.code و r.detail
    eprintln!("{} {} {} {}", r.request_id.unwrap_or("-"), r.status, r.code, r.detail);
}).expect("reporter installed once");
```

المراسل يُثبَّت مرة واحدة على مستوى العملية ويعيد `Err(ReporterAlreadySet)` في الثانية. الحجب
يبقى السلوك الافتراضي: نوع خطأ يطبّق `error_detail` = `None` ولا يطبّق `redacted_detail` لا يُبلَّغ
عنه، حتى يُفعِّل نوعه بنفسه. ولا يُبلَّغ عن 4xx — تفصيلها في الجسم أصلا. يثبّت ذلك
`a_redacted_5xx_detail_reaches_the_reporter_under_the_response_request_id`.

مثال — خطأ معروف الصنف يختار حالته ورمزه بنفسه:

```rust
app = app.get_async("/checkout", || async {
    Err::<&'static str, AetherError>(AetherError::ValidationFailed("cart is empty".into()))
}); // 400 + `{"error":"Validation failure: cart is empty","code":"E4002"}`
```

وللحالة `417` وجهان على السلك، والفرق بينهما مقصود. الرفض الذي يصدره **المحلّل** — حقل `Expect` في
رأس الطلب يسمّي ما لا يلبّيه الخادم — لا يمرّ بعقد أخطاء الـ Handler أصلا، لأن حلقة الاتصال تجيب قبل
الاستدعاء بـ `AetherError::UnsupportedExpectation => Response::status(417, aether_http::response::canonical_reason(417))`
(`app.rs:2997`)، فهي `417 (Expectation Failed)` مع `Connection: close` وبدون جسمٍ يحمل `code`:
يثبّتها `an_unsupported_expectation_is_refused_before_the_body_is_sent` على HTTP/1.1 و
`an_unsupported_expectation_is_refused_over_http2_from_the_head` على HTTP/2، وهذه تشترط صراحة أن يكون
الجسم فارغا. أما الرفض الذي **يُصرّح عنه الـ Handler بنفسه** — `AetherError::UnsupportedExpectation`
أو `DeclaredStatus(417)` — فيمرّ بالسمة أعلاه فيجيب `417` بجسم JSON حمله `code: "E3003"`، ويثبّته
`typed_handler_errors_answer_their_own_class` (`framework/tests/tests/http_e2e.rs`) و
`a_declared_expectation_refusal_inherits_the_code_of_its_status` (`framework/tests/tests/response_exit.rs`).

حذفان متعمّدان في السمة يجب معرفتهما قبل تصميم خطأ خاص:

1. السمة **ليست** `IntoResponse`، والتطبيق العام لا يطلب `E: IntoResponse`. لو فعلت لأصبحت
   `Err("database is unreachable")` تجيب **200** بنص `text/plain`، لأن `&'static str` يطبّق
   `IntoResponse` أصلا (`app.rs:76-80` و`app.rs:82-87`) — أي أن `Ok` و`Err` يصبّان في نفس
   الاستجابة الناجحة.
2. **لا يوجد تطبيق عام فوق `Display`** (كان هو الباب القديم)، لذا نوع خطأ أجنبي مثل `sqlx::Error`
   يدخل العقد بتغليفه في نوع محلي يطبّق السمة. من يُرجعه بدون تغليف يفشل في الترجمة ولا يوصل حالة
   خاطئة بصمت. صيغة التشخيص قياس بتاريخ 2026-10-24 على `aether 0.1.0` لنوع واحد `DbFailure`
   يطبّق `Display` فقط، وفي الحالتين `error[E0277]`:
   * `app.get(..)` مع نوع مصرّح `-> Result<&'static str, DbFailure>` تقول
     `the trait bound DbFailure: IntoResponseError is not satisfied` — أي أنها تسمّي السمة وتعدّد
     تطبيقاتها الثلاثة `&'static str` و`String` و`AetherError` بمواضعها من `app.rs`.
   * `app.get_async(..)` مع `Err::<&'static str, DbFailure>(..)` داخل `async move` تقول
     `the trait bound {closure}: Handler<_> is not satisfied` — ولا تذكر اسم السمة، فيُبحث عنها في
     `aether::IntoResponseError` لا في نص الرسالة.

يثبّت الجدول اختباران على TCP حقيقي: `a_handler_error_answers_400_and_says_so_in_the_body` و
`typed_handler_errors_answer_their_own_class` (`framework/tests/tests/http_e2e.rs:5183` و`:5219`).

---

<a name="2-تصميم-مخرجات-الطرفية"></a>
## 2. تصميم مخرجات الطرفية (Terminal Diagnostic UX)

### أ) لافتة إقلاع السيرفر (Server Startup Banner):
عند تشغيل التطبيق، يظهر ملخص واضح لمعلمات الخادم. النص أدناه مأخوذ من تشغيل فعلي
لـ `framework\target\release\basic_api.exe` على هذا الجهاز بتاريخ 2026-10-25 مع
`AETHER_PORT=3211` و`AETHER_WORKERS=1` — وليس مثالًا مكتوبًا من الذاكرة:
```text
┌─────────────────────────────────────────────────────────────┐
│ ⚡ AETHER WEB FRAMEWORK (Rust Async HTTP/1.1 Engine)         │
├─────────────────────────────────────────────────────────────┤
│ Environment : development                                   │
│ Address     : http://0.0.0.0:3211                           │
│ Routes      : 5                                             │
│ Workers     : 1 (active; tokio work-stealing)               │
│ Allocator   : System allocator (per-request heap)           │
└─────────────────────────────────────────────────────────────┘
```
كان هذا المثال حتى ذلك التاريخ يصف محركًا لا يطبعه البرنامج: `Rust Native Zero-Copy Engine`،
و`Workers : 8 (Thread-per-Core Async Epoll)`، و`Allocator : RequestArena (O(1) Bump Reset -
Zero Heap)`. الثلاثة كلها منقوضة في مكان آخر من التوثيق: المشغّل Tokio متعدد الخيوط مع
work-stealing وليس thread-per-core (`ARCHITECTURE.md` §1، وصف `§1` القديم)، والمخصص هو مخصص
النظام (`FEATURE_COMPLETENESS_MATRIX` سطر Thread-per-core، و`PRODUCTION_READINESS_REPORT` §2).
كما أن صفوف اللافتة كانت غير متساوية مع إطارها: الإطار 63 حرفًا بينما الصفوف 61، وسطر
`Allocator` وحده 63 — أي أن خطأين بصريين كانا يستران بعضهما. الآن العرض مبني من ثابت واحد
(`ServerBanner::render`) وكل سطر يُقاس بنفس الاتساع، ويحرس ذلك اختبار `test_server_banner`
الذي يتحقق من الطول ومن زوايا الإطار ومن وجود كل تسمية (التحقق بالعدّة: نسخة مختبرة بحذف
تسمية `Allocator` أفشلت الاختبار، ونسخة بسطر `Routes` مباعد أفشلته أيضًا).

### ب) رسالة الخطأ التفاعلية عند حجز المنفذ (`E1001`):
من تشغيل فعلي على Windows بتاريخ 2026-10-25: `framework\target\release\basic_api.exe` مع
`AETHER_PORT=3213 AETHER_WORKERS=1` فوق منفذ مشغول، الخروج `1`. النص هو ما طبعه البرنامج حرفيًا
بعد إزالة رموز التلوين (`format_terminal(true)` يلوّن السطر الأول، والرموز لا تُرى في الطرفية):
```text
ERROR [E1001] Server startup failed
  Address: 0.0.0.0:3213
  Reason : TCP Bind failed: Only one usage of each socket address (protocol/network address/port) is normally permitted. (os error 10048)

  Hint:
  Port is already occupied by another running application or requires elevated privileges.

  Try:
  • Change port via AETHER_PORT environment variable (e.g. AETHER_PORT=3214)
  • Find what holds it: netstat -ano | findstr LISTENING | findstr :3213
  • Stop it: taskkill /PID <pid> /F, the PID being the last column above

Error: Io(Os { code: 10048, kind: AddrInUse, message: "Only one usage of each socket address (protocol/network address/port) is normally permitted." })
```
النص الذي كان هنا قبل ذلك كان لينكسيًا (`os error 98`، و`Address: 0.0.0.0:3000`، وترتيب مختلف).
وأحد سطري `Try:` كان هو الآخر لينكسيًا داخل منتج لا يُبنى إلا على Windows: كان يطبع
`lsof -ti:3000 | xargs kill -9` — أمران غير موجودين هناك، ومنفذ 3000 ثابت مهما كان المنفذ الذي
فشل. أُصلح ذلك في الكود (PROB-0024) وصار السطران يُنتَجان من المنفذ الذي فشل فعلًا، والطُبع ما
فوق من الباينري بعد الإصلاح. بقيت ملاحظتان صادقان: `netstat` بلا مرشّح `LISTENING` يعيد أيضًا
اتصالات `TIME_WAIT` موجَّهة إلى نفس المنفذ وعمود الـ PID فيها صفر، أي أن قتل ما يطبعه سيعني
عملية النظام؛ ولهذا يُصفّى الناتج بـ `LISTENING` قبل أن يُعرض. والملاحظة الثانية أن هذا الدليل
لم يُسحَب منه بقية أمثلة المخرجات: بقية المقاطع الملتصقة في الوثائق الأخرى لم تُتحقَّق بالتشغيل.

---

<a name="3-معالجة-الأخطاء-في-وضع-التطوير-مقابل-الإنتاج"></a>
## 3. معالجة الأخطاء في وضع التطوير مقابل الإنتاج (Dev vs Prod)

- **في وضع التطوير (`development`):**
  - يتم إظهار تفاصيل الخطأ وسياقه ورقم السطر للمطور لتسريع عملية تصحيح الكود.
- **في وضع الإنتاج (`production`):**
  - يتم إخفاء مسارات الملفات الداخلية، وحجب كلمات المرور، ومفاتيح الـ API، والـ Tokens المشفرة من السجلات، مع إرجاع استجابة JSON آمنة للعميل تحتوي فقط على `error` و `code`.

---

<a name="4-معرف-التتبع-للطلبات"></a>
## 4. معرف التتبع للطلبات (Request ID & Trace Correlation)

يقوم Aether تلقائياً بتعيين معرف فريد لكل طلب في ترويسة `X-Request-ID`:
```http
HTTP/1.1 200 OK
Content-Type: application/json; charset=utf-8
X-Request-ID: req_000000000042
```
يمكنك قراءة هذا المعرف واستخدامه في الـ Handlers عبر:
```rust
app = app.get("/orders", |req| {
    let trace_id = req.request_id();
    format!(r#"{{"trace_id":"{}"}}"#, trace_id)
});
```

---

<a name="5-أمثلة-عملية-لحل-المشاكل-الشائعة"></a>
## 5. أمثلة عملية لحل المشاكل الشائعة (Troubleshooting Recipes)

### 1. معالجة فشل فك الـ JSON بشكل أنيق:
```rust
app = app.post_async("/api/checkout", |req| async move {
    let payload: CheckoutDto = match req.json() {
        Ok(data) => data,
        Err(err) => {
            return Response::json(bytes::Bytes::from(format!(
                r#"{{"error":"Invalid JSON Payload: {}","code":"E4001"}}"#,
                err
            )))
            .header("Content-Type", "application/json");
        }
    };
    Response::ok()
});
```

### 2. إعادة التوجيه الآمن (Redirects):
```rust
app = app.get("/old-docs", |_req| {
    Response::redirect("/docs/v2", 301)
});
```
