# 07. الحدود، المهلات، والأمان ضد هجمات الـ DoS (Limits & Slowloris Mitigation)

لحماية الخادم من هجمات استنزاف الموارد (Resource Exhaustion) والاتصالات المعلقة، يوفر Aether هيكل `ServerLimits` مع تطبيق فعلي على مستوى المقابس (Sockets).

---

## 1. بنية `ServerLimits`

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // الحد الأقصى لحجم الترويسات (افتراضياً 8 KB)
    pub max_body_bytes: usize,       // الحد الأقصى لحجم جسم الطلب المفكوك (افتراضياً 2 MB) —
                                     // وHTTP/1.1 يرفض أولاً على بايتات الأسلاك (§4)
    pub max_headers_count: usize,    // الحد الأقصى لعدد الترويسات (افتراضياً 100)
    pub max_concurrent_streams: u32, // أقصى عدد تدفقات HTTP/2 متزامنة لكل اتصال (افتراضياً 100)
    pub max_connections: usize,      // الحد الأقصى للاتصالات المتزامنة (افتراضياً 10,000)
    pub read_timeout: Duration,      // مهلة قراءة حزمة TCP (افتراضياً 5 ثوانٍ)
    pub write_timeout: Duration,     // مهلة كتابة الاستجابة (افتراضياً 10 ثوانٍ)
    pub keep_alive_timeout: Duration,// مهلة الحفاظ على الاتصال (افتراضياً 60 ثانية)
    pub request_timeout: Duration,   // مهلة تنفيذ المعالج (افتراضياً 30 ثانية)
    pub receive_timeout: Duration,   // مهلة استقبال الطلب كاملاً — ترويسة+جسم — على الواجهتين (افتراضياً 30 ثانية)
    pub tls_handshake_timeout: Duration, // مهلة إتمام مصافحة TLS (افتراضياً 10 ثوانٍ)
}
```

---

## 2. آليات الحماية المطبقة

### 1. الحماية من هجمات Slowloris (Read Timeout):
يتم تغليف كل عملية قراءة بمهلة زمنية. الجسم أدناه من `framework/aether/src/app.rs:3038-3059`
(مع حذف التعليق): المهلة ليست ثابتاً واحداً — مقبس لم يُستلم منه طلب كامل بعد يخضع
لـ`read_timeout` مقتطعاً بما تبقّى من ميزانية الاستقبال، ومقبس فارغ بين طلبَي keep-alive
يخضع لـ`keep_alive_timeout`:

```rust
let half_received = recv_started.is_some();
let read_limit = if half_received || !served_any {
    let idle = limits_clone.read_timeout;
    match recv_started {
        Some(started) => idle.min(
            limits_clone
                .receive_timeout
                .saturating_sub(started.elapsed()),
        ),
        None => idle,
    }
} else {
    limits_clone.keep_alive_timeout
};
let read_result = tokio::select! {
    biased;
    _ = conn_shutdown.changed() => break,
    r = tokio::time::timeout(
        read_limit,
        stream.read(&mut temp_buf),
    ) => r,
};

match read_result {
    Ok(Ok(0)) | Ok(Err(_)) => break, // مغادرة العميل أو فشل المقبس: لا أحد يُجاب
    Err(_) if half_received => { answer_receive_timeout(&mut stream, &mut write_buf, &limits_clone).await; /* ... */ }
    Err(_) => break, // مقبس خامل بين الطلبات: يُغلق بلا جواب
    Ok(Ok(n)) => { conn_buf.extend_from_slice(&temp_buf[..n]); /* ... */ }
}
```
(الفرع الأخير مبسَّط: في المصدر يُفحص سقف البافر أولاً عند `app.rs:3084` ثم يُضمّ المخزون بعد
خدمة الطلب عند `app.rs:2920-2924`.)
إذا حاول المهاجم فتح اتصال وإرسال بايت واحد كل دقيقة لإبقاء المقبس مفتوحاً، فإن انتظار القراءة
الواحدة لا يتجاوز `read_timeout` (5 ثوانٍ افتراضياً)، ويُقتطع منه ما تبقّى من ميزانية الاستقبال؛
فالطلب الذي بدأ وصوله ولم يكتمل يُجاب بـ`408` ويُغلق الاتصال، وتُحرَّر الموارد.

وفوق مهلة القراءة المفردة، تُقيَّد مرحلة الاستقبال كاملةً (الترويسة والجسم معاً) بـ`receive_timeout`،
وهو **مستقل** عن `request_timeout` الذي يقيس تنفيذ المعالج فقط: رفعٌ بطيء لكن مشروع لا يُرفض إلا
بتجاوز ميزانية الاستقبال، ومعالج بطيء لا يُمدَّد له وقت الاستقبال. والانتظار لا يتجاوز الأقصر من
`read_timeout` وما تبقّى من الميزانية، فالطلب الذي بدأ وصوله ثم توقف — ترويسة ناقصة، أو جسم لم
يبدأ، أو جسم توقف في منتصفه، أو جسم chunked توقف قبل ختامه — يُجاب بـ`408` ويُغلق الاتصال،
وينتهي جسم التدفقة المتوقفة على HTTP/2 بالمهلة نفسها. أما مقبس keep-alive خامل بين طلبين فلا
طلب له يُجاب، فيُغلق صامتًا وهو الفرق الوحيد بين الحالتين. يثبت الفصل
اختباران: `slow_trickle_receives_408_within_the_total_receive_deadline` و
`a_slow_upload_is_not_bounded_by_the_handler_timeout`؛ وتثبت حدود الميزانية هذه ثلاثة اختبارات:
`a_request_that_stops_arriving_is_answered_408_by_the_receive_budget` و
`a_stopped_body_waits_no_longer_than_the_budget_it_started_with` و
`an_idle_keep_alive_connection_is_closed_without_an_answer`.

### 2. الحماية من بطء العميل (Write Timeout):
يتم تغليف كتابة الاستجابة أيضاً بمهلة `write_timeout` لمنع العميل البطيء من احتجاز الـ Worker Task.

### 3. سقف الاتصالات المتزامنة (Connection Limiting via Semaphore):
يتم تقييد حلقة الاستقبال `accept()` بـ `tokio::sync::Semaphore`. كما في
`framework/aether/src/app.rs:2274-2281`، الطلب على الـ permit داخل `select!` منافسٍ بإشارة
الإيقاف، وفشل الـ acquire يُنهي حلقة القبول ولا يفشل الخدمة بـ `unwrap`:

```rust
let permit = tokio::select! {
    biased;
    _ = shutdown_rx.changed() => break,
    p = conn_semaphore.clone().acquire_owned() => match p {
        Ok(p) => p,
        Err(_) => break,
    },
};
// يتم تمرير الـ permit لداخل الـ tokio task
// عند إغلاق الاتصال أو انتهائه، يتحرر الـ permit تلقائياً عبر Drop
```
يمنع هذا فتح أكثر من `max_connections` (10,000 افتراضياً) مما يحمي من هجمات الـ File Descriptor Exhaustion.

### 4. التحكم الصارم بأحجام الذاكرة:
إذا تجاوزت الترويسات 8 KB، يتم الرد بـ `431 Request Header Fields Too Large`.  
هناك حدّان مطبَّقان ولا يجوز قراءة أحدهما مكان الآخر:
- **المفكوك (decoded):** المحلّل يفحص حجم الجسم بعد فك الترميز ضد `max_body_bytes` (2 MB
  افتراضياً) — للـ chunked التراكمي (`framework/http/src/parser.rs:1067`، `:1084`)، وللـ DATA على
  HTTP/2 (`framework/aether/src/http2.rs:891`). أما `Content-Length` المعلن فيُرفض ضد السقف الذي
  يسمّيه المنفذ: المسارات المخزَّنة (buffered) تفحصه ضد `max_body_bytes`
  (`framework/http/src/parser.rs:465`)، ومنفذ البثّ لا يفرض سقفاً عند قراءة الـ head لأن السقف
  قرارٌ يُعرف بعد التوجيه، فيطبّق المسار `max_stream_body_bytes` (64 MB افتراضياً) قبل الإرسال.
- **الأسلاك (wire):** HTTP/1.1 يرفض قبل ذلك بمجرد أن يتجاوز عدد البايتات التي استلمها المقبس
  مجموع الحدّين: `conn_buf.len() + n > max_header_bytes + max_body_bytes` = 2,105,344
  (`framework/aether/src/app.rs:3084`) فيُجاب `413` مع `Connection: close` ويُغلق الاتصال قبل
  أن يكبر البافر أكثر. هذا هو الحدّ الذي يحمي الذاكرة، لأن البايتات على الأسلاك والإسخة المفكوكة
  كلاهما مقيم في الطلب حتى يكتمل.

الفرق بينهما هو الترويسات (framing): جسم chunked مقطّع بقطع من بايت واحد يكلّف 6 بايت أسلاك
مقابل كل بايت محتوى، لذلك أقصى محتوى ينفّذه حدّ الأسلاك على الإعدادات الافتراضية هو **350,879
بايت** (350,880 تُرفض) — أي **16.7 %** من الـ 2 MB المعلنة. هذا مقاس وليس محسوباً:
`a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes`
(`framework/tests/tests/http_e2e.rs`) يقدّم الطرفين على المقبس ويرفض نصفهما، ويتحرّك إذا تحرّك
الحارس. رفع بـ `Content-Length` لا يرى هذا الفرق لأن إطاره ثابت.
