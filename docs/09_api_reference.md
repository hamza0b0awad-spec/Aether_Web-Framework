# 09. المرجع الشامل لدوال وكائنات Aether البرمجية (API Reference)

يوثق هذا الدليل باللغة العربية جميع الكائنات (Structs)، الدوال (Methods)، الواجهات (Traits)، والأنماط البرمجية التي يحتاجها المطور لبناء وتطوير التطبيقات والخدمات السحابية باستخدام إطار عمل **Aether**.

---

## 📑 فهرس الكائنات والمكونات

1. [كائن التطبيق الأساسي (`AetherApp`)](#1-كائن-التطبيق-الأساسي-aetherapp)
2. [واجهة تحويل الاستجابات الموحدة (`IntoResponse`)](#2-واجهة-تحويل-الاستجابات-الموحدة-intoresponse)
3. [كائن سياق الطلب (`RequestContext`)](#3-كائن-سياق-الطلب-requestcontext)
4. [كائن بناء الاستجابة (`Response`)](#4-كائن-بناء-الاستجابة-response)
5. [مجموعات المسارات والمسارات الثابتة (`RouteGroup & Static Assets`)](#5-مجموعات-المسارات-والمسارات-الثابتة-routegroup--static-assets)
6. [كائن حدود وأمان الخادم (`ServerLimits & AetherConfig`)](#6-كائن-حدود-وأمان-الخادم-serverlimits--aetherconfig)
7. [البرمجيات الوسيطة والأمان (`Middleware & Security`)](#7-البرمجيات-الوسيطة-والأمان-middleware--security)
8. [حقن التبعيات والحالة المشتركة (`Dependency Injection & AppStateContainer`)](#8-حقن-التبعيات-والحالة-المشتركة-dependency-injection--appstatecontainer)

---

## 1. كائن التطبيق الأساسي (`AetherApp`)

كائن `AetherApp` هو المدخل الرئيسي لتسجيل المسارات، إضافة البرمجيات الوسيطة، إدارة الحالة وحقن التبعيات، وإطلاق خادم الويب.

### دوال الإنشاء والتهيئة:

#### `AetherApp::new() -> Self`
* **الوصف:** ينشئ تطبيق Aether جديد بالإعدادات الافتراضية.
* **مثال:**
  ```rust
  let mut app = AetherApp::new();
  ```

#### `AetherApp::with_config(config: AetherConfig) -> Self`
* **الوصف:** ينشئ تطبيقاً بالحدود التي يعبّر عنها `AetherConfig` (حجم الجسم والترويسات، عدد أسطر
  الترويسة، سقف تدفقات HTTP/2، مهلات الطلب/الاستجابة/keep-alive، `X-Request-ID`). الحقول الرقمية
  تُترجَم كلها عبر اشتقاق واحد إلى `ServerLimits`. أما `workers` فيُطبَّق فقط حين يبني التطبيق
  وقت تشغيله بنفسه عبر `app.run(...)`؛ `listen` و`bind` و`serve` تعمل على وقت تشغيل المُضيف ولا
  يمكنها تغيير عدد عمّاله. و`max_connections` و`read_timeout` و`tls_handshake_timeout` لا تُضبط من هنا.
* **مثال:**
  ```rust
  let mut config = AetherConfig::default();
  config.port = 3000;
  config.max_body_bytes = 10 * 1024 * 1024; // 10MB
  let mut app = AetherApp::with_config(config);
  ```

#### `app.run(addr: &str) -> Result<(), Box<dyn Error + Send + Sync>>`
* **الوصف:** نقطة تشغيل تحجز الخيط الحالي: تبني وقت تشغيل Tokio من `config.workers` ثم تخدم حتى
  إشعار الإيقاف. وهي **النقطة الوحيدة** التي يطبَّق فيها عدد العمّال (و`AETHER_WORKERS`)؛ إذا
  استُدعيت من داخل وقت تشغيل قائم تُرجِع خطأً بدلاً من أن ينهار، لأن أوقات التشغيل لا تتداخل.
  `listen` و`bind` و`serve` تبقى على وقت تشغيل المُضيف كما هو.
* **مثال:**
  ```rust
  use aether::prelude::*;

  fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
      let mut config = AetherConfig::default();
      config.workers = 1; // قياس على نواة منطقية واحدة
      let mut app = AetherApp::with_config(config);
      app = app.get("/health", |_req| Response::json(bytes::Bytes::from("{}")));
      app.run("0.0.0.0:3000")
  }
  ```

#### `app.manage<T: Send + Sync + 'static>(mut self, state: T) -> Self`
* **الوصف:** تسجيل كائن حالة أو خدمة مشتركة داخل حاوية حقن التبعيات المتوافقة مع تعدد الخيوط ($O(1)$ TypeId DI).
  كل دوال التسجيل والإعداد تأخذ `self` وتعيد `Self`، فتُسلسَل في تعبير واحد أو تُستدعى على الشكل
  `app = app.manage(...)`؛ استدعاؤها كجملة وحدها ينقل `app` ويُسقط الناتج فيرفضه المترجم عند أول
  استخدام لاحق (خطأ نقل، لا مسار مفقود صامت).
* **مثال:**
  ```rust
  app = app.manage(DatabasePool::connect().await);
  ```

---

### دوال تسجيل المسارات المتزامنة وغير المتزامنة:

يدعم Aether تسجيل المسارات المتزامنة (Sync) فائقة السرعة للمهام البسيطة، وغير المتزامنة (Async) للمهام التي تتطلب `.await`:

#### `app.get(path, handler)` و `app.get_async(path, handler)`
* **الوصف:** تسجيل مسار لطلبات `GET`. الدالة يمكن أن تعيد أي نوع يطبق `IntoResponse`، وهي ستة أنواع
  مطبّقة في الشجرة: `Response`، `&'static str`، `String`، `(u16, &'static str)`، `(u16, String)`،
  و`Result<T, E>` حيث `T: IntoResponse` و`E: IntoResponseError` (`aether/src/app.rs:123-166`: سمة
  يطلَب منها `error_status` وحدها، و`error_code` و`error_detail` و`redacted_detail` بقيم افتراضية —
  وهي ليست `IntoResponse` حتى لا تصير `Err` إجابة 200). رسالة 5xx المحجوبة لا تُرسَل للعميل، لكنها
  تُسلَّم إلى مراسل الأخطاء إن ثُبِّت، مقترنة بمعرّف `X-Request-ID` نفسه:
  `aether::set_error_reporter(|r: &aether::ErrorReport| ...)` يُثبَّت مرة واحدة ويعيد
  `Result<(), ReporterAlreadySet>`؛ `ErrorReport` يحمل `request_id: Option<&str>` و`status` و`code`
  و`detail`. النوع الذي يحجب رسالته يبقى صامتًا افتراضيًا: `redacted_detail` تعيد `None` حتى
  يُفعِّلها صاحب النوع. نوع `Json` العام هو مُستخرِج
  (extractor) يُقرأ به جسم الطلب، ولا يطبّق `IntoResponse`، فلا يمكن إرجاع `Json<T>` من معالج.
* **مثال:**
  ```rust
  // متزامن (Zero-overhead Synchronous)
  app = app.get("/health", |_req| "OK");

  // غير متزامن (Async Handler with .await)
  // المعالج الذي يستقبل السياق مباشرة يجب أن يعلّم وسيطه: |req: RequestContext|
  app = app.get_async("/users/{id}", |req: RequestContext| async move {
      let id = req.param("id").unwrap_or("0");
      format!(r#"{{"id":"{}"}}"#, id)
  });
  ```

#### `app.post(path, handler)` و `app.post_async(path, handler)`
* **الوصف:** تسجيل مسار لطلبات `POST`.
* **مثال:**
  ```rust
  app = app.post_async("/api/v1/items", |req: RequestContext| async move {
      match req.json::<CreateItemDto>() {
          Ok(dto) => (201, "Item Created"),
          Err(_e) => (400, "Invalid JSON Payload"),
      }
  });
  ```

#### `app.put(path, handler)` و `app.put_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `PUT` (تحديث كامل للمورد).

#### `app.delete(path, handler)` و `app.delete_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `DELETE` (حذف الموارد).

#### `app.patch(path, handler)` و `app.patch_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `PATCH` (تحديث جزئي للمورد).

#### `app.openapi_spec(title: &str, version: &str) -> OpenApiSpec`
* **الوصف:** يولّد مخطّط OpenAPI 3.1 من جدول المسارات الذي يجيب الطلبات فعلاً، لا من نسخة يدوية
  تُكتب بجانبه فتتفارقان. كل تسجيل يُدرَج بمساره المُعرَّب (`/assets/{filepath}` بدل
  `/assets/*filepath`)، باسم فعلته صغيرًا (`get` و`post` و`put` و`delete` و`patch` والفعل المسجَّل
  به فعليًا عدا ذلك)، بمعرف عملية ثابت، وببارامتر `in: path` مطلوب لكل `{slot}` في مساره.
* **ما لا يعرفه الجدول:** لا مخطّط لجسم الطلب ولا لنوع الاستجابة — المعالج لم يُقرأ؛ و`HEAD`
  والإجابة العامة لـ `OPTIONS` غائبتان لأنهما جوابان بلا تسجيل؛ ومسار `ws` يجيب بـ `101` و`400`
  و`426` لا بـ `200` الذي تجيبه بقية المسارات.
* **الترتيب تعهّد:** المستند مبنيّ من قائمة مرتَّبة بـ (المسار، الفعل)، فجمع نفس المسارات بترتيب
  تسجيل مختلف يعطي نفس المستند، ولا يعتمد معرّف عملية على تجزئة.
* **لا يُخدَم تلقائيًا:** لا يوجد `GET /openapi.json` في الإطار؛ الدالة تُعيد القيمة وقرار نشر شكل
  API داخلي يبقى للمستخدم.
* **مثال:**
  ```rust
  let spec = app.openapi_spec("Aether High-Performance REST API", "1.0.0");
  let openapi_json = spec.to_json().unwrap();
  ```

الأدلة: 8 اختبارات على الأسلاك في `framework/tests/tests/openapi_derivation.rs`، و6 اختبارات وحدة
في `framework/openapi/src/lib.rs`، و4 اختبارات `route_listing` داخل `framework/router/src/trie.rs`،
وخصيصة `the_route_listing_returns_every_registration_with_its_own_names` في
`framework/router/tests/router_properties.rs`.

---

## 2. واجهة تحويل الاستجابات الموحدة (`IntoResponse`)

تتيح واجهة `IntoResponse` للمعالجات إرجاع أنواع بيانات مرنة ومباشرة دون الحاجة لبناء كائن `Response` يدوياً في كل مرة:

```rust
pub trait IntoResponse: Send {
    fn into_response(self) -> Response;
}
```

### الأنواع المدعومة افتراضياً:
1. **`Response`**: إرجاع كائن الاستجابة الكامل مباشرة.
2. **`&'static str`**: يتم تحويله تلقائياً إلى استجابة `200 OK` بنوع `text/plain; charset=utf-8`.
3. **`String`**: يتم تحويله إلى استجابة `200 OK` نصية.
4. **`(u16, &'static str)`**: تعيين رمز الحالة والنص (مثال: `(201, "Created")` أو `(404, "Not Found")`).
5. **`(u16, String)`**: تعيين رمز الحالة مع نص ديناميكي.
6. **`Result<T, E>`**: إذا كانت النتيجة `Ok` يتم تحويل `T` إلى استجابة، وإذا كانت `Err` فالاستجابة
   من نوع الخطأ نفسه عبر `IntoResponseError` (`framework/aether/src/app.rs:123-166`): الحالة من
   `error_status` و`code` من صنفها، و`&'static str` و`String` تجيب 400 بصيغة JSON تحتوي رسالة
   الخطأ، و`AetherError` تجيب 400/404/405/413/417/431/500/504 بحسب الحالة مع إخفاء الرسالة على 5xx
   (والرسالة المخفية تُسلَّم إلى `aether::set_error_reporter` إن ثُبِّت، بمعرّف `X-Request-ID`).

---

## 3. كائن سياق الطلب (`RequestContext`)

يمثل `RequestContext` كافة البيانات والمعلومات الواردة مع الطلب، مع توفير دوال مساعدة لاستخراج البيانات بسرعة وبدون حجز ذاكرة غير ضروري (Zero-Leak RAII).

### أهم الحقول (Fields):
* `ctx.path: String` - مسار الطلب بعد التنقية (مثال: `"/api/v1/users"`).
* `ctx.raw_uri: String` - الرابط الأصلي الكامل بما فيه معلمات الاستعلام.
* `ctx.query_str: Option<String>` - نص الـ Query String إن وجد.
* `ctx.method: Method` - نوع أسلوب الطلب (`Method::Get`, `Method::Post`, إلخ).
* `ctx.body: Bytes` - بايتات جسم الطلب كـ Zero-Copy slice.
* `ctx.request_id: String` - معرّف فريد لكل طلب لتتبعه (مثل `req_000000000001`).
* `ctx.headers()` - مكرِّر على كل حقول الطلب بصيغة `(اسم بحروف صغيرة، أول سطر)`; المخزن نفسه خاص
  ولا يوجد أي طريق للكتابة عليه. الحقول المتكررة كاملة عبر `ctx.header_values(name)`، و`ctx.header(name)`
  يعيد **أول** سطر بمقارنة غير حساسة لحالة الأحرف. الأسماء تُطبَّع إلى أحرف صغيرة، فلا يُحفظ شكل
  الاسم كما وصل. هذه الواجهة للقراءة فقط عمداً: وسيط لا يستطيع منح ترقية WebSocket ولا سحبها بتعديل
  ترويسة، والرفض يكون بجواب مُعاد. البيانات الخاصة بالطلب تُمرَّر عبر `ctx.extensions_mut()`.
* `ctx.params: HashMap<String, String>` - معلمات المسار الديناميكية المستخرجة.

---

### أهم دوال الاستخراج (Methods):

#### `ctx.param(key: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة معلمة مسار ديناميكية. الصيغة الوحيدة التي يراها الـ trie كمعلمة هي `{id}`؛
  `:id` يبقى مقطعًا حرفيًا ولا يُطابَق.

#### `ctx.param_parse<T: FromStr>(key: &str) -> Option<T>`
* **الوصف:** استخراج وتحويل معلمة المسار مباشرة إلى نوع محدد (مثل `u64`, `i32`, `bool`).
* **مثال:**
  ```rust
  let user_id: u64 = req.param_parse("id").unwrap_or(0);
  ```

#### `ctx.query(key: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة معلمة استعلام من الرابط (`?search=rust&page=1`).

#### `ctx.query_parse<T: FromStr>(key: &str) -> Option<T>`
* **الوصف:** استخراج وتحويل معلمة الاستعلام إلى نوع رقمي أو منطقي.

#### `ctx.queries() -> HashMap<&str, &str>`
* **الوصف:** استرجاع جميع معلمات الاستعلام كقاموس مفتاح-قيمة بدون استهلاك ذاكرة إضافية.

#### `ctx.cookie(name: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة ملف تعريف الارتباط (Cookie) بالاسم مباشرة من ترويسة `Cookie`. القيمة كما
  كتبها العميل: يقرؤها العميل ويغيّرها، فلا تصلح مرجعاً لصلاحية شيء. تُقرأ كل أسطر `Cookie` المتكررة
  كوعاء واحد (RFC 9113 §8.2.3 يسمح للعميل بتجزئة الكوكيز على حقول متكررة، وRFC 6265 §5.4 يضمّ الوعاء
  كله في حقل واحد)، وأول ظهور للاسم هو الذي يُعاد حين يتكرر عبر الأسطر. لرفض التكرار بدل الاختيار
  استخدم `signed_cookie`. للتوقيع اقرأ القسم 7، البند 6.
* **مثال:**
  ```rust
  let session_token = req.cookie("session_id");
  ```

#### `ctx.signed_cookie(signer: &CookieSigner, name: &str) -> SignedCookie`
* **الوصف:** قراءة كوكي موقَّع بقيمة لا يقررها العميل. يعيد `Present(payload)` حين حمل الطلب هذا
  الاسم مرة واحدة وتحقّق وسمه، و`Absent` حين لم يحمله، و`Invalid` حين حمله ولم يتحقّق، و`Ambiguous`
  حين حمله مرتين — فلا يُحسم التعارض بأخذ واحدة منهما. اسم الكوكي داخل الوسم، فقيمةٌ وُقّعت لاسم
  لا تُقرأ تحت اسم آخر. التوقيع سلامة لا سرّية: الحِمل base64url والعميل يقرؤه.
* **مثال:**
  ```rust
  match req.signed_cookie(&signer, "session") {
      SignedCookie::Present(user_id) => Response::text_dynamic(user_id),
      SignedCookie::Absent => Response::status(401, "Anonymous"),
      SignedCookie::Invalid | SignedCookie::Ambiguous => Response::status(400, "Bad Request"),
  }
  ```

#### `ctx.form_data() -> HashMap<String, String>`
* **الوصف:** فك تشفير بيانات النماذج (`application/x-www-form-urlencoded`) مباشرة من جسم الطلب مع معالجة الـ URL Decoding تلقائياً.

#### `ctx.header(name: &str) -> Option<&str>`
* **الوصف:** قراءة قيمة ترويسة معينة من ترويسات الطلب (غير حساسة لحالة الأحرف Case-Insensitive).

#### `ctx.body_str() -> Result<&str, Utf8Error>`
* **الوصف:** قراءة جسم الطلب كنص `&str` بترميز UTF-8 بدون أي عمليات نسخ للذاكرة.

#### `ctx.json<T: DeserializeOwned>() -> Result<T, serde_json::Error>`
* **الوصف:** فك تشفير جسم الطلب من صيغة JSON مباشرة إلى بنية بيانات Rust.

#### `ctx.state<T: Send + Sync + 'static>() -> Option<Arc<T>>`
* **الوصف:** استرجاع مرجع الخدمة أو الحالة المشتركة المحقونة بنظام الـ DI عبر نوعها `T`.

#### `ctx.peer_addr() -> Option<SocketAddr>`
* **الوصف:** عنوان الطرف الآخر على المقبس كما وصل، دون أي قراءة لترويسات التمرير. هذا هو العنوان
  الذي يتحدث به الخادم فعلاً، ويبقى صحيحاً حتى خلف وسيط عكسي — لكنه حينها عنوان الوسيط نفسه.

#### `ctx.client_addr() -> Option<IpAddr>`
* **الوصف:** العنوان الذي عُزي إليه الطلب بعد حل سلسلة التمرير. بلا قائمة وكلاء موثوقين يعيد عنوان
  المقبس؛ ومتى ضُبطت القائمة يعيد أول قفزة غير موثوقة في السلسلة (اقرأ القسم 7، البند 3). هو أيضاً
  المفتاح الذي تستخدمه محدِّدات المعدل، فيحصل كل عميل خلف الوسيط على سطلته الخاصة.

#### `ctx.extension::<T>() -> Option<&T>`
* **الوصف:** قراءة قيمة من امتدادات الطلب بسياق نوعها. طبقة المصادقة بحامل التوكن تضع `Principal`
  هنا قبل تشغيل المعالج (اقرأ القسم 7، البند 4)، ويمكن لأي وسيط أن يضع نوعه الخاص.

#### `ctx.insert_extension(value: T) -> Option<T>`
* **الوصف:** كتابة قيمة في امتدادات الطلب. إن كانت القيمة تملك ذاكرة على الكومة (مستند مفكوك، مطالبات
  توكن) فاستخدم `ctx.insert_extension_sized(value, heap_bytes)` لتُحتسب تلك البايتات في ميزانية الطلب.

---

## 4. كائن بناء الاستجابة (`Response`)

يوفر كائن `Response` واجهة بانية مرنة وسريعة لإنشاء ردود HTTP بصيغ متعددة:

### دوال الإنشاء السريع (Constructors):
* `Response::ok()`: استجابة `200 OK` فارغة.
* `Response::text("محتوى نصي")`: استجابة نصية بنوع `text/plain; charset=utf-8`.
* `Response::html("<h1>عنوان</h1>")`: استجابة HTML بنوع `text/html; charset=utf-8`.
* `Response::json(bytes)`: استجابة JSON بنوع `application/json; charset=utf-8`.
* `Response::bytes(bytes, content_type)`: استجابة بايتات خام مع تحديد نوع المحتوى.
* `Response::status(code, message)`: استجابة برمز حالة مخصص.
* `Response::redirect(location, status_code)`: استجابة إعادة توجيه (301, 302, 307, 308).
* `Response::file("path/to/file.ext")`: قراءة وخدمة ملف من القرص مع تحديد الـ MIME Type تلقائياً وحماية المسارات من الاختراق (Path Traversal Protection).

### دوال التعديل والتخصيص:
* `.header("Key", "Value")`: إضافة ترويسة جديدة للاستجابة.
* `.set_cookie(name, value, http_only, secure, same_site, max_age_secs)`: ترويسة `Set-Cookie` تحمل
  الاسم والقيمة بعد percent-encoding، ثم `Path=/` و`SameSite` و`HttpOnly` و`Secure` و`Max-Age` حسب
  وسائطها. الترويسة تخصّ المتصفح ولا توقّع شيئاً: `value` ما زالت كتابةً من العميل، فمن يعتمد عليها
  مدخلاً موثوقاً يوقّعها بنفسه عبر `CookieSigner` (القسم 7، البند 6).

### تدفق الجسم (Response streaming)

* `Response::stream(source, content_type)`: جسم غير معلوم الطول يأتي قطعةً قطعة من `BodySource` (الطريقة `next_chunk()`). يُرسل على HTTP/1.1 بـ`Transfer-Encoding: chunked`، وعلى HTTP/1.0 بجسم ينتهي بإغلاق الاتصال (`Connection: close`)، وعلى HTTP/2 كإطارات `DATA` بلا ترويسة تأطير.
* `Response::stream_with_length(source, content_type, len)`: الجسم نفسه مع `Content-Length` معلن؛ يبقى الاتصال قابلاً لإعادة الاستخدام، ومصدر ينتهي قبل الطول أو يتجاوزه يُغلق الاتصال، لأن التأطير لا يُعاد التفاوض عليه بعد كتابة الرأس.
* `BodyStream::from_chunks([Bytes, ...])`: تكييف سلسلة قطع جاهزة إلى مصدر، و`BodyStream` نفسه `BodySource`، فأي مصدر يمكن تغليفه.
* `HEAD` والحالات `204`/`304`: يُسقطان المصدر دون استدعاء `next_chunk()` إطلاقاً. والملفات الثابتة التي يخدمها `static_dir`/`static_file` تُدفّق بهذه الطريقة تلقائياً بقطع 64 كيلوبايت.

---

## 5. مجموعات المسارات والمسارات الثابتة (`RouteGroup & Static Assets`)

### تجميع المسارات (Route Grouping):
تتيح دالة `app.group` تنظيم مسارات التطبيق في مجموعات منطقية ببادئة مسار موحدة:

```rust
app = app.group("/api/v1", |v1| {
    v1.get("/health", |_| "v1 healthy");
    v1.get_async("/users/{id}", |req: RequestContext| async move {
        format!("User: {}", req.param("id").unwrap_or("0"))
    });
});
```

### حالة المجموعة والموجّه القابل للتركيب (`RouteGroup::manage` و `Router`):
الحالة يمكن أن تُسجَّل على مستوى المجموعة، فيجيب `ctx.state::<T>()` من حاوية المجموعة قبل حاوية
التطبيق — لكن الإظلال **حسب النوع لا حسب الحاوية**: النوع الذي تسجّله المجموعة يجيب لمساراتها، وما
لا تسجّله يستمر يُجاب من التطبيق، فلا تركيب إخفاء خدمات الخادم:

```rust
app = app.group("/admin", |admin| {
    admin.manage(Realm("admin"));
    admin.get("/who", |req: RequestContext| {
        let realm = req.state::<Realm>().map(|r| r.0.clone()).unwrap_or_default();
        format!("realm={realm}")
    });
});
```

أما `Router` فهو موجّه فرعي مستقل: وحدة تسليم لميزة واحدة، يُملأ دون أن يعرف المنفذ ولا إعدادات TLS
ولا الحدود التي سيعمل تحتها، ثم يُركَّب عند بادئة بـ `app.mount` أو داخل مجموعة بـ `group.mount`.
ويحمل كامل سطح `RouteGroup`: دوال التسجيل، `route_stream`، `ws` و`ws_subprotocols`، `static_dir`،
و`group`/`mount` المتداخلة:

```rust
let mut api = Router::new();
api.manage(Db(7));
api.use_middleware(|ctx, next| async move { next(ctx).await });
api.get_async("/health", |ctx: RequestContext| async move {
    format!("pool={}", ctx.state::<Db>().map(|d| d.0).unwrap_or(0))
});
app = app.mount("/api", api);
```

قواعد التركيب:

* **المسار:** مسارٌ مسجّل عند `/health` في موجّه مركّب عند `/api` يُخدَم عند `/api/health`؛ قاعدة
  اللمّ هي نفسها المستخدمة في `app.group`، فالبادئة بشرطة مائلة أخيرة أو بدونها تصل إلى نفس المسار.
* **الترتيب:** وسائط التطبيق ← حالة المجموعة ← وسائط المجموعة/الموجّه ← المعالج. موجّه مركّب داخل
  مجموعة يرث حرّاس تلك المجموعة.
* **الحالة:** حاوية المجموعة نسخةٌ-عند-الكتابة (`Arc::make_mut`)، فمجموعة فرعية ترث حاوية أبيها
  وتنفصل عنها أول ما تسجّل قيمتها الخاصة، وتظل بقية الأنواع مشتركة.
* **الترتيب داخل الموجّه** تعهدٌ كما داخل المجموعة: `manage` و`use_middleware` يصلان إلى التسجيلات
  التي تأتي بعدهما فقط، لا إلى ما جُمع قبلهما.
* **لا شيء على مسار غير مُوسَّع:** التنحية تنفّذ كمدخل وسائط واحد في رأس سلسلة المسار، فمسار سُجّل
  خارج أي مجموعة لا يدفع شيئًا (`RequestContext::scoped_state` يبقى `None`)، وتبقى `serve.rs` كما هي
  على الواجهات الثلاث: HTTP/1.1 وHTTP/2 والأجسام الدفّاقة وترقية WebSocket.
* **التركيب يستهلك الموجّه:** `mount` يأخذ `Router` بالقيمة و`Router` لا ينفّذ `Clone`، فميزةٌ
  يجب أن تجيب عند بادئتين تبني موجّهَين، أو تُعرِّف أحدَهما داخل الآخر بـ `Router::mount`.

الأدلة: 14 اختبارًا على الأسلاك في `framework/tests/tests/nested_routers.rs`، وواحد فوق h2c في
`framework/tests/tests/http2_e2e.rs`
(`a_mounted_router_answers_with_its_own_state_over_h2c`)، وأربعة اختبارات وحدة في
`framework/aether/src/app.rs`.

### خدمة الأصول الثابتة المدمجة (Embedded Static Assets):
خدمة ملفات CSS, JS, الأيقونات المضمنة في الباينري بكفاءة قصوى:

```rust
app = app.static_asset(
    "/static/style.css",
    "text/css",
    b"body { background: #f0f0f0; }",
);
```

---

## 6. كائن حدود وأمان الخادم (`ServerLimits & AetherConfig`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,      // الحد الأقصى لحجم الترويسات بالبايت (الافتراضي: 8KB)
    pub max_body_bytes: usize,        // حدّ حجم جسم الطلب المفكوك (الافتراضي: 2MB)؛ HTTP/1.1 يرفض
                                      // أولاً على بايتات الأسلاك (docs/07 §4)
    pub max_headers_count: usize,     // أقصى عدد مسموح به من الترويسات (الافتراضي: 100)
    pub max_concurrent_streams: u32,  // أقصى عدد تدفقات HTTP/2 متزامنة لكل اتصال (الافتراضي: 100)
    pub max_connections: usize,       // سقف الاتصالات المتزامنة المفتوحة (الافتراضي: 10,000)
    pub read_timeout: Duration,       // مهلة قراءة حزمة TCP لمنع Slowloris (الافتراضي: 5s)
    pub write_timeout: Duration,      // مهلة كتابة الاستجابة إلى المقبس (الافتراضي: 10s)
    pub keep_alive_timeout: Duration, // مهلة إبقاء المقبس مفتوحاً للطلبات التالية (الافتراضي: 60s)
    pub request_timeout: Duration,    // مهلة تنفيذ المعالج (الافتراضي: 30s)
    pub receive_timeout: Duration,    // مهلة استقبال الطلب كاملاً: ترويسة وجسم، على الواجهتين
                                      // (الافتراضي: 30s) — مستقلة عن مهلة المعالج
    pub tls_handshake_timeout: Duration, // مهلة إتمام مصافحة TLS (الافتراضي: 10s)
}
```

### واجهة التشغيل: `AetherConfig`

`ServerLimits` هي ما ينفذه المحلّل والطبقات النقل فعلياً، أما الواجهة العامة فهي `AetherConfig`.
كل حقولها تُترجَم عبر دالة اشتقاق واحدة (`AetherApp::limits_for`) إلى `ServerLimits`، فلا يمكن أن
يختلف الرقم المُعدّ عن الرقم المطبَّق:

```rust
pub struct AetherConfig {
    pub host: String,                    // عنوان الاستماع (الافتراضي: "0.0.0.0")
    pub port: u16,                       // المنفذ (الافتراضي: 3000)
    pub environment: String,             // "development" | "production"
    pub max_body_bytes: usize,           // 2MB مفكوك، وسقف المقبس على الأسلاك هو
                                         // max_header_bytes + max_body_bytes
    pub max_header_bytes: usize,         // 8KB
    pub max_headers_count: usize,        // 100 سطر ترويسة لكل طلب
    pub max_concurrent_streams: u32,     // 100 تدفقة HTTP/2 لكل اتصال
    pub workers: usize,                  // عمّال Tokio؛ يُطبَّق عبر app.run فقط
    pub max_request_time: Duration,      // 30s — مهلة تنفيذ المعالج فقط
    pub max_receive_time: Duration,      // 30s — ميزانية استقبال الطلب (ترويسة+جسم)، مستقلة
                                         // عن مهلة المعالج؛ الطلب المتجاوز يجيب 408
    pub max_response_time: Duration,     // 10s
    pub keep_alive_timeout: Duration,    // 60s
    pub x_request_id: bool,              // إرسال X-Request-ID (الافتراضي: true)
}
```

الحدود التي لا تزال خاصة بـ `ServerLimits::default()` ولا تُضبط من `AetherConfig` هي
`max_connections` و`read_timeout` و`tls_handshake_timeout`.

---

## 7. البرمجيات الوسيطة والأمان (`Middleware & Security`)

### 1. تفعيل CORS:
`CorsConfig` سبعة حقول؛ لا يمكن بناؤها بمُنشئ جزئي دون `..CorsConfig::default()` (الحقول الثلاثة
التي يغفلها هذا المثال — `allowed_origins` و`expose_headers` و`allow_credentials` — لها قيم
deny-by-default). انتبه: إذا كانت `allowed_origins` غير فارغة فهي authoritative وتتجاهل
`allow_origin`، و`allow_credentials: true` مع `*` غير صالح حسب Fetch standard.

```rust
app = app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization",
    max_age_secs: 86400,
    ..CorsConfig::default()
}));
```

### 2. تفعيل ترويسات الأمان (HSTS, CSP, X-Frame-Options):
```rust
app = app.enable_security_headers();
```

### 3. خلف وسيط عكسي (Trusted proxies):
بلا إعداد، كل ترويسة تمرير (`Forwarded` أو `X-Forwarded-For`) هي مجرد ادعاء من العميل، ولا يصدّقها
الخادم ولا تُغيّر هوية الطلب. لتثق بها، سمِّ الشُّبكات التي يصل منها الوسيط، بصيغة CIDR:

```rust
app = app.enable_trusted_proxies(["10.0.0.0/8", "192.0.2.10/32", "2001:db8::/32"]);
```

* القائمة فارغة أو تحوي `0.0.0.0/0` أو `::/0` ← ذعر عند التسجيل (`panic!`) بدل قائمة تقبل الجميع بصمت.
* تُقرأ السلسلة من اليمين إلى اليسار: كل قفزة تُقبل فقط لأن من أوصلها موثوق، والتوقف عند أول عنوان
  خارج القائمة — وهو نتيجة الحل. هذا يمنع ادعاء قفزة يسارية مزيفة من الفوز.
* عند وجود `Forwarded` و`X-Forwarded-For` معاً يُعتمد `Forwarded` وحده؛ وأسطر الحقل المتكررة تُدمج
  بترتيب وصولها قبل القراءة؛ وأي عنصر مشوّه يوقف المسير ولا يُصدّق ما على يساره؛ والحد الأقصى 32 قفزة.
* النتيجة تصل إلى `ctx.client_addr()`، ويبقى `ctx.peer_addr()` عنوان المقبس، ومحدِّد المعدل يفتح سطلاً
  لكل هوية محلولة.

### 4. المصادقة بحامل التوكن (Bearer authentication):
`require_bearer(&auth)` طبقة وسيط جاهزة تُسجَّل على مسار واحد أو مجموعة: تُقرأ ترويسة
`Authorization: Bearer <token>`، يُتحقَّق من التوقيع والمطالبات بـ`JwtValidator`، ثم يُوضع `Principal`
في امتدادات الطلب قبل تشغيل المعالج. كل رفض — غياب الترويسة، مخطط آخر، توكن مشوّه، توقيع مزيف،
انتهاء صلاحية، أو `alg` غير المثبَّت — هو نفس `401` مع `WWW-Authenticate: Bearer error="invalid_token"`
وجسم فارغ، فلا يعرف المهاجم أي تخمين اقترب.

```rust
use aether::prelude::*;
use bytes::Bytes;

let auth = BearerAuth::new(b"المفتاح المشترك مع المصدر", JwtValidationOptions::default());
let mut app = AetherApp::new();
app = app.get("/public", |_| "open");
app = app.route_sync_with(Method::Get, "/me", require_bearer(&auth), |ctx: RequestContext| {
    match ctx.extension::<Principal>() {
        Some(principal) => Response::json(Bytes::from(
            serde_json::json!({ "sub": principal.subject }).to_string(),
        )),
        None => Response::status(500, "no principal"),
    }
});
```

* الحقل singleton (RFC 9110 §11.6.2): طلب يحمل أكثر من سطر `Authorization` يُرفض بدل أن تختار طبقة
  سطراً وتختار أخرى سطراً غيره.
* المخطط غير حسّاس لحالة الأحرف (`Bearer`/`bearer`)، والتوكن يجب أن يكون `token68` (RFC 7235 §2.1).
* باني `JwtValidator::new` — ومنه `BearerAuth::new` — يذعر على سرّ فارغ: مفتاح HMAC صفري الطول
  يقبل كل توكن موقّع به، فتكون خلفية التوقيع عامة. الرفض عند البناء يجعل سوء الإعداد يظهر عند
  الإقلاع؛ والطول ليس شرطاً — مفتاح قصير غير فارغ يبقى مقبولاً.
* المصادقة ليست تصريحاً: من يحتاج فحص دور أو نطاق يقرأ `principal.claims` ويقرر بنفسه.
* التسجيل على مسار واحد لا يمسّ المسارات المجاورة؛ و`use_middleware` الشامل يسري على ما يُسجَّل بعده
  فقط، لذا فالنموذج لكل مسار هو الذي يترك المسارات العامة كما هي.
* خارج النطاق: الإبطال، وإعادة التشغيل (توكن مسروق صالح حتى `exp`)، والخوارزميات غير المتماثلة وJWKS
  وتوكنات التحديث. المصدر يوقّع وهذا الخادم يتحقق.

### 5. الحماية من CSRF (التحقق المزدوج الموقَّع):

تسجيل واحد يغطي التطبيق كله — لا لكل مسار — لأن مساراً نسيه المشغّل يبقى مكشوفاً:

```rust
use aether::prelude::*;

let secret = std::env::var("CSRF_SECRET").expect("CSRF_SECRET must be set");
let mut app = AetherApp::new();
app = app.enable_csrf(CsrfTokens::new(secret, CsrfConfig::default()));
app = app.get("/form", |ctx: RequestContext| {
    let token = ctx.extension::<CsrfToken>().map(CsrfToken::as_str).unwrap_or("");
    format!("<form method=\"post\"><input type=\"hidden\" name=\"_csrf\" value=\"{token}\"></form>")
});
app = app.post("/submit", |_| "accepted");
```

* التفعيل `AetherApp::enable_csrf` (`framework/aether/src/app.rs:1130`) يأخذ `CsrfTokens` المبنيّ من
  السر و`CsrfConfig`.
* ما يُصدر: كوكي `aether_csrf` قيمته `nonce.mac` — نونس من 128 بت عشوائية ثم HMAC-SHA256 عليه
  بمفتاحك (`framework/security/src/csrf.rs:262-267`). الكوكي `SameSite=Lax` و`Secure` و`Path=/`، بلا
  `Domain`، وبلا `HttpOnly` لأن سكربت الصفحة يجب أن يقرأه ليكرر القيمة في الترويسة.
* الطريقة غير الآمنة (كل ما ليس GET/HEAD/OPTIONS/TRACE، RFC 9110 §9.2.1) يجب أن تكرر القيمة في حقل
  واحد `x-csrf-token`. الفحص يقبل كوكي واحداً وُقّع بمفتاحك وترويسة واحدة تطابقه تماماً
  (`framework/security/src/csrf.rs:312-320`)، والمقارنة ثابتة التوقيت.
* كل رفض — غياب الكوكي أو الترويسة، قيمة ناقصة، توقيع غريب، كوكي مكرر، ترويستان — هو `403` واحد
  بجسم فارغ، قبل تشغيل المعالج، ودون إصدار توكن جديد (`framework/aether/src/serve.rs:422-459`).
* الطرق الآمنة تُسلَّم التوكن في امتداد `CsrfToken` لترسمه الصفحة، والتوكن الصالح يُعاد استخدامه ولا
  يُدوَّر، فصفحتان مفتوحتان أو نموذج رُسم للتوّ يبقى صالحاً. الكوكي المزروع (بلا توقيع صالح) يُستبدل
  بتوكن موقَّع في أول استجابة آمنة، ولا يُصدر توكن على preflight ولا على استجابة `>= 400`.
* `max_age` يحسب من لحظة الإصدار ولا يُمدَّد ما دام الكوكي يُعاد استخدامه؛ والافتراضي كوكي جلسة.
* استثناء مسارات الآلات (webhooks) عبر `exempt_prefixes`: المسار المستثنى بلا حماية CSRF، فلا تضعه
  على مسار يعتمد على كوكي.
* `CsrfConfig` تحمل `cookie_name` و`header_name` و`secure` و`same_site` و`max_age` و
  `exempt_prefixes`. `SameSite=None` أو بادئة `__Host-`/`__Secure-` تتطلب `secure = true`، واسم غير
  صالح أو سر فارغ أو بادئة `/` تُفشل البناء فوراً بدل أول طلب.
* السر مشترك: كل عملية خلف موازن الحمل يجب أن تحمل نفس `CSRF_SECRET`، وإلا رفضت عمليةٌ توكناً
  أصدرته أخرى بـ`403`.

### 6. الكوكيز الموقَّعة (`CookieSigner`):

كوكي المتصفح يكتبه العميل، فاعتماده بلا توقيع اعتمادٌ على مدخلات الخصم. `CookieSigner`
(`framework/security/src/cookie.rs:115-190`) يصدر `base64url(payload).base64url(HMAC-SHA256)` ويفحصه،
والاسم داخل الوسم: `name` فبايت `0x00` فالحِمل المرمَّز (`framework/security/src/cookie.rs:183-189`)
— فلا تُقبل قيمةٌ وُقّعت لـ`theme` تحت `role`.

```rust
use aether::prelude::*;

let value = signer.value("session", "user-42").expect("a token name");
let response = Response::ok().set_cookie("session", &value, true, true, "Lax", Some(3600));

match req.signed_cookie(&signer, "session") {
    SignedCookie::Present(user_id) => Response::text_dynamic(user_id),
    SignedCookie::Absent => Response::status(401, "Anonymous"),
    SignedCookie::Invalid | SignedCookie::Ambiguous => Response::status(400, "Bad Request"),
}
```

* الوسم HMAC-SHA256 يتحقق بـ`ring::hmac::verify`، وهي مقارنة ثابتة التوقيت، وكل فشل يجيب `None`
  بشكل واحد: قيمة مقطوعة، وسم بطول خطأ، حِمل ليس UTF-8، أو توقيع صحيح على اسم آخر. تفريقها يخبر
  المهاجم أي جزء أصاب، ولا شيء في الأعلى يفرّقها.
* لا سرّية، وهذا مقصود: الحِمل base64url يقرؤه العميل. من يريد إخفاء القيمة يحتاج تشفيراً لا
  توقيعاً، فسطر «Cookies» في مصفوفة الاكتمال يبقى نصفه الثاني مفتوحاً.
* `CookieSigner::new` لا يعرف سرّاً افتراضياً؛ السرّ وسيط بناء، والفارغ يفشل فوراً بـpanic. كل
  عملية يجب أن تحمل السرّ نفسه، وإلا رفضت ما أصدرته أختها.
* `Ambiguous` ليست تفصيلاً: اسمٌ مكرر مرة أو مرات يُرفض (cookie tossing) بدل أن يختار الخادم
  أوله أو آخره، لأن القيمتين لا تتفقان على من المتصل.
* `CookieSigner` و`SignedCookie` و`CookieNameError` تخرج من `aether::prelude::*`، والقراءة على
  السلك في `framework/aether/src/cookie.rs:31-91`.
* الأدلة: 15 اختبار وحدة في `framework/security/src/cookie.rs:192-401`، و11 اختبار سلك فوق HTTP/1 في
  `framework/tests/tests/signed_cookies.rs`، و5 اختبارات سلك فوق HTTP/2 في
  `framework/tests/tests/http2_e2e.rs` — أربعة منها على h2c بلا اتصال آمن وواحد بعد ALPN فوق TLS، وواحد
  من هذه الأربعة يقرأ الكوكي الموقَّعة على أربعة تدفقات متزامنة في اتصال واحد — وdoctest اثنان،
  و`compile_fail` واحد يمنع `Debug`
  على `CookieSigner`، صاحبة المفتاح؛ وأما `SignedCookie` فمخرجه المطبوع يبدي الحالة ويحجب الحِمل.

---

## 8. حقن التبعيات والحالة المشتركة (`Dependency Injection & AppStateContainer`)

يوفر Aether نظام حقن تبعيات آمن ومتوافق مع تعدد الخيوط (Thread-Safe DI Container) معتمداً على `TypeId` دون أي انعكاس ديناميكي بطيء:

```rust
use aether::prelude::*;

struct DbConnection {
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تسجيل الخدمة
    app = app.manage(DbConnection {
        url: "postgres://localhost/aether_db".to_string(),
    });

    // استهلاك الخدمة داخل المسار
    app = app.get_async("/db-info", |req: RequestContext| async move {
        if let Some(db) = req.state::<DbConnection>() {
            format!("Database URL: {}", db.url)
        } else {
            "Database connection not registered".to_string()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```

> هذه الكتلة فُحصت بـ `cargo check` بتاريخ 2026-10-22 مقابل `aether` بالمسار (exit 0 بلا تحذيرات).
> كانت سابقًا تُرفض: `AetherApp` لا يُرى بدون `use aether::prelude::*;`، ومعالم `get_async` يجب أن
> تُعلَّم (`|req: RequestContext|`) لأن عدد الأنواع وأحجامها في `Handler<Args>` تُقرأ من توقيع
> الـ closure ولا يمكن استنتاجها من مكان الاستدعاء.
