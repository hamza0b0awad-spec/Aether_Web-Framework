# 01. نظرة عامة على المعمارية ودورة حياة الطلب (Architecture & Request Lifecycle)

تم بناء Aether بنظام طبقات معيارية تفصل بين عمليات القراءة من الشبكة، والتحليل (Parsing)، والتوجيه (Routing)، والتنفيذ (Handler Execution).

---

## 1. مخطط تدفق دورة حياة الطلب (Request Lifecycle Flow)

```text
       Incoming TCP Stream (Client)
                   │
                   ▼
┌─────────────────────────────────────────┐
│     1. Connection Semaphore Check       │  (Limits: max_connections = 10,000)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│  2. Persistent Buffering (BytesMut)     │  (Two-tier read with read_timeout)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│      3. Incremental Parser & Security   │  (RFC 9112, No Smuggling, Zero-Copy)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│      4. Radix Trie Router ($O(K)$)      │  (Bitmask Method Match, Path Params)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│  5. Middleware Pipeline & Context Init  │  (CORS, Security Headers, Tracing)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│         6. Async Handler Execution      │  (JSON / Text / Custom Response)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│   7. Direct Buffer Serialization        │  (write_http1_to direct to buffer)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│   8. Async TCP Write & Buffer Advance   │  (write_timeout applied, split_to advance)
└─────────────────────────────────────────┘
```

---

## 2. الصناديق البرمجية الأساسية (Crates Hierarchy)

- **`aether`**: الواجهة الموحدة والخادم الذي يدمج جميع المكونات ويشغل الـ Event Loop مع Tokio.
- **`aether-core`**: محرك الذاكرة `RequestArena` ومحرك التشخيص `Diagnostic` وأنواع `AetherError`.
- **`aether-http`**: تراكيب البيانات ونماذج HTTP (الترويسات، محلل HTTP/1.1، حدود الأمان `ServerLimits`، والردود) — لا يحتوي على أي كود HTTP/2، فالترميز يتولاه `h2` والجسر في `aether/src/http2.rs`.
- **`aether-router`**: محرك التوجيه الراديكسي (Radix Trie) مع دعم المعلمات الديناميكية وفحص الأفعال بـ Bitmask.
- **`aether-runtime`**: حلقة أحداث العامل (Worker Event Loop) ومعالجة اتصالات TCP.
- **`aether-middleware`**: معالجات الأمان وCORS وتسجيل الطلبات الموجه للأداء الفائق.
- **`aether-security`، `aether-websocket`، `aether-openapi`، `aether-scheduler`**: التشفير والتحقق من JWT، محلل إطارات WebSocket (RFC 6455)، مولد OpenAPI v3.1 المشتق من جدول المسارات، والجدولة التكيفية — أربعتها موصولة بواجهة `aether`.
- **`aether-validation`، `aether-dependency`**: حزمتان داخل الـ workspace لا تعتمد عليهما `aether` بعد المسار 111: الأولى فحص ASCII والبريد بقناع `u64` (كلمة-بكلمة، لا تعليمات متجهية)، والثانية حاوية حقن التبعيات (DAG).
