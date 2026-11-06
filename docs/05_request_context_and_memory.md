# 05. السياق وتفادي الـ Allocations (RequestContext & Memory Management)

تمت إعادة هندسة كائن سياق الطلب `RequestContext` في Aether لإلغاء أي تسريب للذاكرة (Zero Memory Leaks) وتقليل الحجوزات غير الضرورية.

---

## 1. التخلص التام من `Box::leak`

### المشكلة السابقة:
كانت النسخ الأولية تستخدم `Box::leak(path.into_boxed_str())` لتحويل السلاسل النصية إلى `&'static str` لإرضاء الـ Borrow Checker أثناء التنفيذ غير المتزامن (`tokio::spawn`).
- **النتيجة الكارثية:** كل طلب وارد كان يقتطع جزءاً دائماً من الذاكرة (Memory Leak) لا يتحرر أبداً، مما يسبب امتلاء الرام وانهيار الخادم تحت الضغط العالي.

### الحل المطبق:
تم استبدال ذلك بملكية واضحة ومباشرة:
- النصوص تُدار كـ `String` منضبطة.
- البايتات تُدار كـ `Bytes` (التي توفر Zero-Copy slicing ومشاركة مؤشرات الذاكرة بحساب مراجع ذري ذرّي دون نسخ البايتات).
- فور انتهاء الـ Handler، يتم تحرير كافة هياكل الـ `RequestContext` تلقائياً عبر آلية الـ RAII في Rust.

---

## 2. بنية الـ `RequestContext`

الكتلة أدناه هي `framework/aether/src/app.rs:394-428` مع حذف التعليقات الطويلة، أي أنها مُقتطَفة
من المصدر لتُقرأ لا لتُترجَم في مشروع القارئ (`HeaderLines` هو `pub(crate) type` عند `app.rs:308`
و`lines_for` دالة خاصة عند `app.rs:717`):

```rust
pub struct RequestContext {
    pub raw_uri: String,
    pub path: String,
    pub query_str: Option<String>,
    pub method: Method,
    pub params: HashMap<String, String>,
    /// اسم الحقل بعد التحويل إلى أحرف صغيرة → كل سطوره بترتيب الشبكة؛ المخزن الوحيد.
    header_multi: HashMap<String, HeaderLines>,
    pub body: Bytes,
    /// جسمٌ متدفّق على مسار `route_stream`: المصدر يُقرأ أثناء تنفيذ المعالج، ويبقى `body` فارغاً.
    body_stream: Option<crate::request_body::RequestBody>,
    pub request_id: String,
    request_serial: u64,
    state: Arc<AppStateContainer>,
    scoped_state: Option<Arc<AppStateContainer>>,
    /// مقبس TCP الذي فتح الاتصال — خاص حتى لا تُكسر بنائية الحرف الواحد عند إضافة حقل.
    peer_addr: Option<SocketAddr>,
    /// قيم خاصة بهذا الطلب فقط (`Extensions`) تملؤها الطبقات الوسيطة.
    extensions: crate::extensions::Extensions,
}

impl RequestContext {
    /// استخراج جسم الطلب كنص UTF-8 بدون نسخ
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// فك تشفير JSON مباشرة من بايتات الطلب
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// قراءة معلمة مسار بسهولة
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    /// قراءة ترويسة بشكل مستقل عن حالة الأحرف (RFC 9110 §4.2)، تعيد أول سطر (RFC 9110 §5.2)
    pub fn header(&self, name: &str) -> Option<&str> {
        self.lines_for(name).and_then(|values| values.first()).map(String::as_str)
    }
}
```

> `header_multi` هو مصدر الحقيقة الوحيد: `header()` تعيد أول سطر، و`header_values()` كل
> الأسطر بترتيب الشبكة، و`headers()` تعداد `(اسم صغير، أول سطر)` بلا ترتيب. وهو أيضاً ما
> تقرأه طبقات الإطار (CORS، ضغط المحتوى، قرار `Connection: close`). لا حقل عام قابل
> للكتابة: طبقة وسيطة تريد تمرير قرار إلى المعالج تستخدم `extensions_mut()`، وهي لكل طلب
> ولا يقرأها أي فحص داخلي في الإطار.
> الحقول `peer_addr` و`extensions` أُضيفا بعد كتابة هذه الصفحة أول مرة؛ ترك القائمة عليهما كان
> يجعلها تصف بنيةً لم تعد موجودة.
> و`peer_addr` يبقى عنوان المقبس أبداً؛ الهوية التي عُزي إليها الطلب تُقرأ من `ctx.client_addr()`،
> وهي `peer_addr` نفسها ما لم تسمِّ `enable_trusted_proxies` الوكلاء الموثوقين، وعندها فقط تُحل من
> سلسلة `Forwarded`/`X-Forwarded-For`.

---

## 3. تحسين تسلسل الاستجابة (Direct Buffer Serialization)

عند تجهيز الـ Response، تم استبدال استخدام الـ `format!()` والـ `String` الوسيطة بكتابة مباشرة إلى الـ `BytesMut` الخاص بالمقبس. الواجهة العامة ثلاث دوال (`framework/http/src/response.rs:394-424`)
كلها يتفويض إلى جسم واحد `write_http1_response` عند `response.rs:426`:

```rust
pub fn write_http1_to(&self, out: &mut BytesMut) {
    self.write_http1_response(out, 1, true);
}

fn write_http1_response(&self, out: &mut BytesMut, minor: u8, include_body: bool) {
    use std::io::Write;
    {
        let mut writer = bytes::BufMut::writer(&mut *out);
        let _ = write!(writer, "HTTP/1.{} {} {}\r\n", minor, self.status, self.status_text);

        let mut has_date = false;
        for (k, v) in &self.headers {
            if k.eq_ignore_ascii_case("content-length") {
                continue;
            }
            // الجسم مؤطَّر دائماً بـ Content-Length هنا، لذا ترويسة Transfer-Encoding
            // من المتصل تُبقي العميل في TE.CL response-desync
            if k.eq_ignore_ascii_case("transfer-encoding") {
                continue;
            }
            if k.eq_ignore_ascii_case("date") {
                has_date = true;
            }
            let _ = write!(writer, "{}: ", k);
            let _ = write_sanitized_field_value(&mut writer, v); // لا CR/LF/NUL داخل القيمة
            let _ = write!(writer, "\r\n");
        }
        if !has_date {
            write_http_date_now(&mut writer);
        }
        // 1xx/204/304 لا تحمل جسماً فلا يُصدر لها Content-Length (RFC 9110 §15.3.5، §15.4.5)
        if !status_forbids_body(self.status) {
            let _ = write!(writer, "Content-Length: {}\r\n", self.body.len());
        }
        let _ = write!(writer, "\r\n");
    }
    // ... ثم البايتات نفسها عند include_body، وتُحذف تماماً عند 1xx/204/304
}
```
**الفائدة:** صفر Heap Allocations وسيطة لكتابة ترويسات الاستجابة.

- السطر الأول يُصدر `HTTP/1.{minor}` حيث `minor` نسخة الطلب لا ثابت `1.1`:
  `write_http1_to_for_request_version(&self, out, minor, include_body)` عند `response.rs:415-424`
  هو ما تستخدمه الاستجابات العادية، عبر المُصدِّر المشترك `write_http1_response`
  (`framework/aether/src/app.rs:4027`)؛ وله نداء ثانٍ عند رفض ترقية WebSocket لتجاوز الميزانية،
  وهو رفضٌ يُكتب أيضاً بنسخة الطلب (`app.rs:2801`).
  أما مخارج الأخطاء والرفض (`app.rs:2343` و`:4256` و`:3002` و`:4305`)
  فتمرّ عبر `write_http1_to`، أي `minor = 1` ثابت: تُكتب `HTTP/1.1` حتى لطلبٍ وصل بعنوان
  `HTTP/1.0`. الفرق مرئيّ لعميل 1.0 يقرأ نسخة الرد ليقرر بقاء الاتصال، والاتصال في هذه المسارات
  يُغلق بـ `Connection: close` على أي حال.
- كل قيمة ترويسة تمرّ عبر `write_sanitized_field_value` (`response.rs:490`)، لأن RFC 9110 §5.5
  يمنع CR وLF وNUL داخل قيمة الحقل — قيمة تحويها تسمح بحشو ترويسات كاملة في الرد.
