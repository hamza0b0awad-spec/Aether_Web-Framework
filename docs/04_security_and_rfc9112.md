# 04. معالجة الترويسات وحماية الـ Request Smuggling (RFC 9112 Security)

يلتزم Aether بصرامة بمتطلبات مواصفات RFC 9112 لحماية الخادم والتطبيقات المبنية عليه من هجمات Desync و HTTP Request Smuggling.

---

## 1. حماية الترويسات (Content-Length & Transfer-Encoding)

تحدث هجمات الـ Request Smuggling عندما يختلف الخادم الأمامي (Reverse Proxy / Nginx / Cloudflare) عن الخادم الخلفي في تحديد نهاية الطلب.

### القواعد الأمنية المطبقة في Aether:

1. **رفض تكرار ترويسة `Content-Length` (Dual CL Attack):**
   - إذا تضمن الطلب أكثر من ترويسة `Content-Length`، أو ترويسة بقيم متعددة (مثل `Content-Length: 10, 20`)، يتم رفض الطلب فوراً بـ `400 Bad Request`.

2. **رفض دمج `Transfer-Encoding` مع `Content-Length` (CL.TE / TE.CL Attack):**
   - وفقاً لـ RFC 9112 بند 6.3، وجود كلا الترويستين معاً يُعد دليلاً على محاولة تلاعب أمني. يرفض Aether هذا الطلب مباشرة.

3. **التحقق من صحة القيمة الرقمية:**
   - رفض القيم السالبة أو غير الرقمية أو القيم التي تتجاوز السعة القصوى للـ `usize`.

4. **رفض تكرار ترويسة `Host`:**
   - خادم أمامي يقرأ الحقل الأول وخادم المصدر يقرأ الأخير، فتكرار `Host` انقسام في تعريف الطلب
     لا تنسيق زائد. يرفض Aether بأكثر من حقل `Host` واحد بعدّاد الحقول لا بمقارنة قيمها، استجابةً
     لـ RFC 9112 بند 3.2 الذي يجعل ذلك `400` لكل طلب، لا لـ HTTP/1.1 وحده.

---

## 2. كود التحقق الأمني في الـ Parser (`parser.rs`)

المقتطف أدناه مُصرَّف من `framework/http/src/parser.rs:382-468`: أُبقيت فيه فروع التهريب مع
عدّاد `Host` ورفضِ تكرارها (حُذف عدّاد `chunked` مقابل بقية الترميزات و`te_chunked`/`te_other`
وفرع `allow_transfer_encoding`)، و`ows_trim` دالة خاصة داخل `parser.rs` و`HeaderView`/`HeaderMap` من
`framework/http/src/header.rs`. فالكتلة تُقرأ لا تُترجَم في مشروع القارئ. كل رفضٍ هنا يخرج بـ
`AetherError::MalformedRequest` فيجاب الطلب بـ `400`، ولا توجد أصناف خطأ خاصة بالتهريب في
`AetherError` — الأصناف الموجودة هي `HeaderTooLarge` و`BodyLimitExceeded` و
`MalformedRequest` وغيرها (`framework/core/src/error.rs:5-19`).

```rust
let mut headers = HeaderMap::new();
let mut content_length: Option<usize> = None;
let mut te_count: usize = 0;
let mut host_count: usize = 0;

for h in req.headers.iter() {
    headers.push(HeaderView { name: h.name, value: h.value });

    if h.name.eq_ignore_ascii_case("content-length") {
        if content_length.is_some() {
            // الحماية من هجمات Request Smuggling: تم رصد تكرار Content-Length
            return Err(AetherError::MalformedRequest);
        }
        let val_str = std::str::from_utf8(h.value).map_err(|_| AetherError::MalformedRequest)?;
        let trimmed = ows_trim(val_str);
        // يرفض هذا الشرط القيم غير الرقمية والفارغة و`Content-Length: 10, 20` معاً
        if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AetherError::MalformedRequest);
        }
        let parsed_len = trimmed
            .parse::<usize>()
            .map_err(|_| AetherError::MalformedRequest)?;
        content_length = Some(parsed_len);
    } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
        te_count += 1;
    } else if h.name.eq_ignore_ascii_case("host") {
        host_count += 1;
    }
}

// RFC 9112 بند 3.2: أكثر من حقل Host واحد يُجاب بـ 400 لكل إصدار، والرفض يسبق استبدال
// الصيغة المطلقة لسلطة الهدف مكان Host — وإلا انهار الحقلان المكرران في قيمة واحدة قبل العدّاد
if host_count > 1 {
    return Err(AetherError::MalformedRequest);
}

// RFC 9112 بند 6.3: حظر الجمع بين Transfer-Encoding و Content-Length (منع ثغرات CL.TE و TE.CL)
if headers.len() > limits.max_headers_count {
    return Err(AetherError::HeaderTooLarge);
}
if te_count > 0 && content_length.is_some() {
    return Err(AetherError::MalformedRequest);
}
if let Some(cl) = content_length {
    if cl > limits.max_body_bytes {
        return Err(AetherError::BodyLimitExceeded);
    }
}
```

هذا الفرع يحجز المفكوك (decoded) لا الأسلاك: `Content-Length` المعلن يُقارن بعدد البايتات التي
سيصلها المحلّل بعد فك الترميز. حدّ الأسلاك على HTTP/1.1 يسبقه وهو مجموع
`max_header_bytes + max_body_bytes`، وأثره على جسم chunked مقاسٌ ومُثبَّتُ اختباراً في
`docs/07` (§4).

قبل هذه الكتلة يصنّف `classify_target` هدفَ الطلب في أحد أشكال RFC 9112 بند 3.2 الأربعة:
origin-form التي تبدأ بـ `/`، وabsolute-form `http(s)://host/path`، وauthority-form التي يملكها
`CONNECT` وحدها، والنجمة `*` التي يملكها `OPTIONS` وحدها. كل هدف خارجها مرفوض بـ `400` ولا
يُوجَّه كأنه مسار: قبل هذا التصنيف كان `GET hello HTTP/1.1` يطابق المسار `/hello`، وكان
`GET ?x=1` يطابق `/`. وحين يكون الهدف absolute-form تستبدل الدالة نفسها حقل `Host` الوارد
بسلطة الهدف، امتثالاً لبند 3.2.2 الذي يلزم خادم المصدر بأن يتجاهل `Host` المستلمة ويستخدم مضيف
هدف الطلب؛ الاستبدال لا التجاهل هو ما يجعل قائمةَ السماح بالـ `Host` وCORS ومعالجاً يقرأ `Host`
ترى المضيف نفسه. أما غياب `Host` على HTTP/1.1 فيبقى `400` ولو حمل الهدف سلطة، لأن الاستبدال
كان ليخفيه. ومسار HTTP/2 يحسم المضيف بالمثل: سلطة `:authority` غير الفارغة تستبدل `Host`
الواردة ولا تخسر لها — RFC 9113 بند 8.3.1 يمنع استخدام `Host` في تحديد الهدف حين تكون `:authority`
موجودة، ويكتفي بأن «ينبغي» معاملة حقل `Host` مخالفٍ لها كمطلوبٍ معطوب؛ والاستبدال يفي بالواجب
ويُبقي الطلب خلف وسيطٍ «يجوز» أن يحتفظ بحقل `Host` كما ورد، وهو ما يجعل البروتوكولين يقرآن المضيف
نفسه للطلب نفسه. ويبقى هدف `OPTIONS *` غير موجَّه إلى مسار: يردّه الخادم بـ `204` مع ترويسة `Allow`
واحدة لكل الخادم، محسوبة مرةً عند بناء الخدمة من كل المسارات المسجّلة (RFC 9110 بند 9.3.7)، فلا
يُخبر عميلٌ بطرقٍ لا يملكها خادمٌ أو تُحسب لكل طلب. ويبقى `CONNECT` سؤالاً عن نفقٍ لا عن مسار: هدفُه سلطةٌ وحدها (بند 3.2.3) وهذه الطبقة لا تقيم نفقاً، فيُجاب `501 Not Implemented` — RFC 9110 بند 9.3.6 يخيّر خادمَ الأنفاق غيرَ الداعم بين `405` و`501`، و`501` هنا هو الصدق، لأن `404` الموجِّه كان يلوم المضيف والمنفذ على قدرةٍ ينقص الخادمَ نفسه. وقراراتُ الأمان تسبق جوابَ القدرة: `Host` خارج قائمة السماح وحصةٌ متجاوزة تُجابان قبله.

موثوقية هذه المسارات ليست ادّعاءً في نصّ: `test_smuggling_cl_te_conflict_rejected`
(`framework/http/src/parser.rs:1132`) تؤكّد رفض CL.TE على البايتات الخام. و
`test_duplicate_host_is_refused_on_http10_too` تؤكّد رفض تكرار `Host` حتى على إصدار لا يفرض
وجودها أصلاً، و`test_absolute_form_does_not_excuse_a_missing_host` تؤكّد أن الهدف المطلق لا يعفي
الطلب من `Host`، و`test_absolute_form_routes_on_its_own_path_and_replaces_host` تثبّت أن المسار
والاستعلام يُقرآن من الهدف وأن `Host` الوحيدة الباقية هي سلطته، و
`test_targets_outside_the_four_forms_are_rejected` تمرّر ثلاثة عشر هدفاً خارج الأشكال الأربعة على
مفكوك الطلبات. وعلى الأسلاك نفسها: `targets_outside_the_four_forms_are_refused` في
`framework/tests/tests/http_conformance.rs` يقيس اثني عشر منها ويجيب كلَّها `400`. و`connect_authority_form_is_answered_not_implemented` تثبّت `501` لهدفِ سلطةٍ صحيح، أما `host_validation_outranks_the_connect_refusal` فتثبّت أن رفض المضيف يسبق جواب القدرة.

الأشكال الأربعة تُقرأ على HTTP/2 أيضاً، لا على سطر الطلب وحده. بند 8.3.1 لا يسقط صيغة النجمة:
`OPTIONS` الخادم-كلّي يصل كـ `:path` قيمتها `*` وحدها، فيُجاب `204` بـ `Allow` واحدة للخادم كما على
HTTP/1 (`options_asterisk_answers_for_the_whole_server_over_h2`)؛ و`*` على طريقةٍ أخرى هدفٌ خارج
الأشكال الأربعة، فكانت هذه الطبقة تراه مساراً صالحاً ويُجاب `404`، والآن يُجاب `400` مثل HTTP/1
(`an_asterisk_on_a_method_that_is_not_options_is_answered_400_over_h2`). أما `:path` الفارغة أو
المطلقة أو التي لا تبدأ بـ `/` فمعطوبةٌ ببند 8.1.1 وتُغلَق بمسارها فقط برسالة `PROTOCOL_ERROR`، ويبقى
الاتصال بخدمة ما بعده (`a_path_that_is_not_origin_form_is_reset_and_the_connection_survives`).
و`CONNECT` على HTTP/2 لا يحمل `:scheme` ولا `:path` أصلاً، ويجعل بند 8.5 نهايةَ التدفق معنى
إغلاقِ وصلة TCP — أي أن عميلاً مطابقاً للمواصفة يترك التدفق نصف مفتوح حتى يُقام النفق؛ فكانت هذه
الطبقة تنتظر جسماً لن يصل، فيتأخّر جوابُها خلف مهلة الاستقبال، والآن يُجاب من الرأس وحده
(`a_connect_is_answered_from_the_head_without_a_body`). ويستبدل `:authority` حقل `Host` المخالف
أو المكرر على الأسلاك نفسها، واحدةً لا قيمتين (`the_authority_replaces_a_disagreeing_host_on_the_wire`)،
وتقرأ قائمةُ السماح المضيفَ نفسه فلا يغسل حقلٌ مسموح هدفاً غير مسموح
(`host_validation_reads_the_authority_and_not_the_field_it_replaces`).
يبقى فرقٌ لا تملكه هذه الطبقة: `h2` تحذف الشظية `#` من `:path` قبل أن تسلّمنا الطلب، فرفضُ HTTP/1
لهدفٍ يحمل `#` لا نظيرَ له هنا، والبروتوكولان يحدّدان المورد نفسه مهما بلغ ذلك الفرق.

---

## 3. الأمان التشفيري في الـ JWT (Constant-Time Verification)

عند التحقق من تواقيع JWT أو رموز الأمان:
- لا يتم استخدام مقارنة السلاسل النصية القياسية `==` (والتي تخرج مبكراً عند أول بايت غير متطابق مسببة تسريب توقيت - Timing Attack).
- يتم استخدام `ring::hmac::verify` (`framework/security/src/jwt.rs:123`)، وهي مقارنة ثابتة التوقيت
  فوق الوسم بالكامل، إضافةً إلى مساعد المشروع نفسه
  `aether_security::constant_time::constant_time_eq` للمقارنات فوق البايتات
  (`framework/security/src/constant_time.rs:6`). لا يعتمد المشروع على crate اسمه `subtle`.

---

## 4. مصادقة حامل التوكن (Bearer) وحدود الرفض الموحّد

`require_bearer` (`framework/aether/src/auth.rs`) يبني طبقةً حول `BearerAuth`
(`framework/security/src/auth.rs`)، وهي بدورها تغلّف `JwtValidator`:

- الحقل `Authorization` يُقرأ كاملاً: صفر أسطر أو سطران ← رفض، لأن الحقل singleton (RFC 9110 §11.6.2)،
  فلا تختار طبقة سطراً وتختار طبقة أخرى غيره.
- المخطط `Bearer` غير حسّاس لحالة الأحرف (RFC 9110 §11.1)، والتوكن يجب أن يصطف على `token68`
  (RFC 7235 §2.1) قبل أي عمل تشفيري.
- التوقيع يُتحقق بـ`ring::hmac::verify` (مقارنة ثابتة فوق الوسم)، ثم يُفحص `alg` المثبَّت، ثم
  `exp`/`nbf`/`iat` و`iss`/`aud` — فـ`alg: none` أو استبدال الخوارزمية يُرفض قبل حساب أي HMAC.
- باني `JwtValidator::new` يرفض السرّ الفارغ ذعراً عند البناء: مفتاح HMAC بطول صفر يقبل كل توكن
  وُقّع به، فأي أحد يستطيع توقيع توكن فيمرّ. `BearerAuth::new` يمرّ عبر الباني نفسه فيرث الرفض،
  كما ترفض `CookieSigner::new` و`CsrfTokens::new` القيمة نفسها — فسوء الإعداد يظهر عند الإقلاع لا
  عند أول طلب مزوّر. والرفض على الفراغ لا على الطول: مفتاح قصير اختاره المصدر يبقى سرّاً.
- كل فشل يعطي الاستجابة نفسها: `401` مع `WWW-Authenticate: Bearer error="invalid_token"` وجسم فارغ.
  التمييز بين "منتهٍ" و"مزوّر" يخبر المهاجم أي تخمين اقترب، وإخبار العميل الشرعي مسؤولية المصدر.
- الطلب الناجح يحمل `Principal` (الموضوع والمطالبات) في امتدادات الطلب، ومطالباته تُحتسب في ميزانية
  ذاكرة الطلب (`insert_extension_sized`)، والمعالج يقرؤها ليتخذ قرار التصريح بنفسه.
- حدود معلنة: لا إبطال ولا منع إعادة تشغيل (توكن مسروق يبقى صالحاً حتى `exp`)، ولا خوارزميات غير
  متماثلة ولا JWKS ولا توكنات تحديث. المصدر يوقّع وهذا الخادم يتحقق.

---

## 5. الحماية من CSRF (التحقق المزدوج الموقَّع)

الكوكي اعتماد ambient: المتصفح يرسله مع أي طلب من أي موقع، فلا يثبت أن الطلب صدر من صفحتك.
`AetherApp::enable_csrf` يضيف الشرط الثاني: القيمة نفسها تُقرأ من كوكي ومن ترويسة، والكوكي موقّع
بمفتاح لا يعرفه العميل.

- **الكوكي لا يُقبل إلا موقّعاً** (`framework/security/src/csrf.rs:330-344`): القيمة `nonce.mac`،
  والوسم HMAC-SHA256 على نص النونس يُتحقق بـ`ring::hmac::verify`، فكوكي زرعه موقع آخر لا يمر.
- **المطابقة تامة وأحادية**: كوكي واحد (اثنان بنفس الاسم محاولة تثبيت عبر نطاق شقيق ← رفض) وترويسة
  واحدة (الحقل singleton)، والمقارنة ثابتة التوقيت (`framework/security/src/csrf.rs:312-320`).
- **الرفض موحّد ومبكر**: `403` بجسم فارغ، قبل تشغيل المعالج، ودون كوكي جديد
  (`framework/aether/src/serve.rs:422-459`) — فلا يفرّق المهاجم بين تخمين وتخمين.
- **الطرق الآمنة معفاة** (GET/HEAD/OPTIONS/TRACE، RFC 9110 §9.2.1) وتُسلَّم التوكن لترسمه الصفحة.
- **الحدود المعلنة**: التوقيع يثبت أن هذا الخادم أصدر التوكن، لا أنه حديث — العمر يحكمه المتصفح،
  ولا ربط بجلسة على الخادم لأن الإطار لا يملك مخزن جلسات. وبادئة مستثناة (`exempt_prefixes`) تعني
  حماية معدومة على مساراتها. و`SameSite=Lax` يمنع أبسط إرسال نموذج عبر المواقع لكنه ليس الحماية:
  طلب `POST` من أعلى مستوى ما زال يحمل الكوكي.

