# 03. محرك التوجيه وخوارزمية الـ Radix Trie (Routing Engine)

تم تصميم محرك التوجيه في Aether لتحقيق سرعة بحث متناهية $O(K)$ حيث $K$ هو عدد مقاطع المسار، مع تقليل التخصيصات في الذاكرة (Allocations) إلى الصفر في المسارات العادية.

---

## 1. شجرة التوجيه الراديكسية (Radix Trie Architecture)

يتم تخزين المسارات في شجرة مقاطع (`RouteNode`) بدلاً من البحث الخطي:

```text
               / (Root)
             ┌────┴─────────┐
           api            static (CatchAll: *filepath)
         ┌───┴───┐
       v1       v2
     ┌───┴───┐
   users   posts
     │
   {id} (Param)
     │
   orders
```

---

## 2. أسبقية مطابقة المسارات الحتمية (Deterministic Route Precedence)

لضمان عدم وجود أي غموض في مطابقة الروابط، يتبع الـ Router الترتيب الصارم التالي:

1. **المسارات الثابتة أولاً (Exact Static Match)**: مثل `/users/me`
2. **المعلمات الديناميكية ثانياً (Parameterized Match)**: مثل `/users/{id}` — الصيغة الوحيدة التي
   يقرأها `router/src/trie.rs:126` كمعلمة هي المقطع المحاط بأقواس معقوفة؛ `:id` يبقى مقطعًا ثابتًا
   حرفيًا ويُطابَق كنص لا كمتغيّر.
3. **المسارات الشاملة ثالثاً (Wildcard / Catch-All)**: مثل `/static/*filepath`

---

## 3. مطابقة أساليب HTTP عبر الـ Bitmask (Single-Instruction Method Matching)

بدلاً من مقارنة النصوص أو البحث في مصفوفات، يتم تمثيل كل أسلوب HTTP بـ Bit في حقل `u64`.
الترقيم الفعلي في `framework/http/src/method.rs:25-43` (لا توجد ثوابت اسمية من نوع `METHOD_GET`):

```rust
impl Method {
    #[inline(always)]
    pub const fn bit(self) -> u8 {
        match self {
            Method::Get => 0,
            Method::Post => 1,
            Method::Put => 2,
            Method::Delete => 3,
            Method::Patch => 4,
            Method::Head => 5,
            Method::Options => 6,
            Method::Trace => 7,
            Method::Connect => 8,
            Method::Custom(n) => (n % 55) + 9,
        }
    }

    #[inline(always)]
    pub const fn bitmask(self) -> u64 {
        1u64 << self.bit()
    }
}
```

**الميزة:**
- فحص ما إذا كان المسار يدعم الأسلوب المطلوب يتم بتعليمة معالج واحدة (Single CPU Cycle)، وهو جسم
  `RouteNode::supports_method` كما هو في `framework/router/src/node.rs:42-44`
  (الحقل على العقدة اسمه `allowed_methods`، لا `allowed_methods_mask`):
  ```rust
  (self.allowed_methods & method.bitmask()) != 0
  ```
- إرجاع خطأ `405 Method Not Allowed` فوراً مع قائمة الأساليب المسموحة بدقة
  (`MatchResult::MethodNotAllowed { allowed_bitmask }` في `framework/router/src/trie.rs:24-26`،
  و`RadixRouter::bitmask_to_allow_header` عند `trie.rs:64` تبني ترويسة `Allow` من نفس القناع).

---

## 4. تقليل الـ Allocations باستخدام `SmallVec`

عند استقبال مسار مثل `/api/v1/users/42`، يتم تقسيم المسار باستخدام:
```rust
let mut segments: SmallVec<[&str; 8]> = SmallVec::new();
```
- **0 Heap Allocations:** للمسارات التي تحتوي على أقل من 8 مقاطع (أكثر من 99.9% من مسارات الويب الحقيقية)، يتم حجز السلاسل بالكامل على الـ CPU Stack وتفادي الـ Heap Allocator تماماً.
