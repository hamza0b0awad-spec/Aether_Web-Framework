# 02. معالجة شبكة TCP والتأطير (TCP Framing & Connection Buffering)

في بروتوكول TCP، قد تصل بيانات طلب HTTP مجزأة عبر حزم (Segments) متعددة، أو قد تصل عدة طلبات متتالية ضمن قراءة شبكية واحدة (HTTP Pipelining / Keep-Alive).

---

## 1. مشكلة التجزئة والحل الهندسي في Aether

### المشكلة السابقة (Naïve Fixed Buffer):
قراءة البيانات في مصفوفة `[u8; 8192]` وفصلها عن القراءات التالية كان يؤدي إلى:
1. فشل قراءة طلبات `POST` أو الطلبات ذات الترويسات الكبيرة بـ `400 Bad Request` عند وصولها عبر أكثر من TCP packet.
2. فقدان البيانات المتبقية عند وجود طلب ثانٍ في نفس القراءة.

### الحل المطبق (Persistent Two-Tier Framing):

```text
TCP Socket
   │
   ▼
┌───────────────────────┐
│ Temporary Read Chunk  │ (8KB buffer via stream.read)
└───────────┬───────────┘
            ▼
┌───────────────────────┐
│ Connection Buffer     │ (BytesMut - accumulates stream chunks)
└───────────┬───────────┘
            ▼
       parse_request_incremental(&conn_buf, &limits, &mut chunked_body)
            │
     ┌──────┴──────┐
     ▼             ▼
 Ok(Some)       Ok(None)
(Complete)    (Partial / Fragmented)
     │             │
     │             ▼
     │      Wait for next TCP chunk (with read_timeout)
     ▼      and accumulate into conn_buf
 Process Request
     │
     ▼
conn_buf.split_to(consumed)  ──>  Advance buffer by exact bytes
     │
     ▼
Check conn_buf again (Instant loop for pipelined requests!)
```

---

## 2. إدارة الذاكرة واقتطاع البايتات (`split_to`)

باستخدام `BytesMut::split_to(consumed)`:
- يتم فصل الشريحة التي تم استهلاكها بالكامل بـ $O(1)$ ودون أي عمليات نسخ للذاكرة (Zero-Copy pointer slicing).
- البايتات المتبقية في `conn_buf` تصبح هي بداية الطلب التالي مباشرة.
- `split_to` لا يُصغّر الكتلة المحجوزة: بافر ضاعفه جسمٌ كبير يبقى مضاعَفاً لبقية عمر المقبس،
  لذلك يعيد `app.rs:2920-2924` بناءه بحجمه الابتدائي عندما يفرغ، ويُسعَّر الحجز حسب
  `capacity()` لا حسب عدد البايتات المكتوبة (`app.rs:3891-3893`).
- `aether_core::RequestArena` (تصفير بـ $O(1)$ عبر `arena.reset()`) كائن مُختبَر
  (`framework/tests/src/lib.rs`) لكنه ليس في حلقة الخدمة الحيّة: مكانه الوحيد الذي يُستخدم فيه
  هو `framework/runtime/src/worker.rs:24` داخل حلقة نموذجية قديمة لا يستدعيها شيء
  (`framework/runtime/src/lib.rs:3-11`).

---

## 3. كود المعالجة في الـ Runtime (`app.rs`)

هذا المقتطف مركّب من حلقة الخدمة الفعلية: رأس الحلقة ونداء المحلل عند
`framework/aether/src/app.rs:2432-2644`، وتقديم البافر بـ `split_to(consumed)` عند `:2911`،
وفرعا `Ok(None)` و`Err(err)` عند `app.rs:2951` و`app.rs:2990` (حُذف منه جسم تنفيذ الطلب وأسباب الخطأ)،
وهو ليُقرأ لا ليُترجَم: الدالة التي تُحلّل الرأس هي
`parse_request_incremental` وليست `RequestView::parse` (الأخيرة موجودة في
`framework/http/src/parser.rs:59` لكنها تخصّ حلقة `runtime/worker.rs` القديمة).
بين رأس الحلقة ونداء المحلل يقف فرع `route_stream`: مسارٌ مُسجَّل به يُخدَم من رأسه وحده
(`parse_request_head`)، فجسمه يصل أثناء تنفيذ المعالج ولا يدخل هذا البافر أصلاً — ومن هنا كان
نطاق `app.rs:2432-2644` أطول من المقتطف:

```rust
// 1. محاولة تحليل أي بيانات مكتملة متبقية في البافر التراكمي (Pipelining + Keep-Alive)
loop {
    if conn_buf.is_empty() {
        break;
    }

    let parse_result = match aether_http::parser::parse_request_incremental(
        &conn_buf,
        &limits_clone,
        &mut chunked_body,
    ) {
        Ok(Some((req, owned_body, consumed))) => {
            // ... تنفيذ الطلب، ثم تقديم البافر بقدر ما استُهلك فعلاً:
            let _ = conn_buf.split_to(consumed);
        }
        Ok(None) => {
            // الطلب غير مكتمل، ننتظر قراءة الحزمة التالية من TCP (قيد read_timeout)
            break;
        }
        Err(err) => {
            // 431 / 413 / 400 ثم إغلاق المقبس
        }
    }
}
```
