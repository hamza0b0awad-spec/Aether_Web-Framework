//! Track 74 / PROB-0016 durable evidence: what a per-request `Extensions` payload actually costs
//! the heap, and therefore what the memory budget was blind to.
//!
//! Run it as a standalone crate (`cargo run --release`, dev-dependency-free; the map shapes below are
//! the ones `framework/aether/src/extensions.rs` builds, priced and pre-pricing). Every figure quoted
//! in `framework/docs/PRODUCTION_READINESS_REPORT.md` section 16, in `FEATURE_COMPLETENESS_MATRIX.md`
//! item 17, and in the track-74 governance record is this program's stdout on 2026-10-26 on the
//! owner's Windows/MSVC x86_64 host, release profile:
//!
//! ```text
//! container, priced shape (Entry { value, price, charged }) - the map the framework builds:
//!   1 entry 264 B, 2 entries 268 B, 4 entries 684 B, 8 entries 1 288 B,
//!   16 entries 2 492 B, 24 entries 2 492 B   ->  (2 492 - 264)/15 = ~148 B marginal per entry
//! container, pre-pricing shape (Box<dyn Any> as the value) - where the 128 draft came from:
//!   1 entry 200 B, 2 entries 204 B, 4 entries 492 B, 8 entries 904 B,
//!   16 entries 1 724 B, 24 entries 1 724 B   ->  (1 724 - 200)/15 = ~101 B marginal per entry
//! one entry + payload (peak held by map + value):
//!          16 B: String in map 300 | Vec<u8> in map 300 | Vec<String> in map 318
//!         512 B: String in map 796 | Vec<u8> in map 796 | Vec<String> in map 1 372
//!        4 096 B: String in map 4 380 | Vec<u8> in map 4 380 | Vec<String> in map 8 988
//!      1 048 576 B: String in map 1 048 860 | Vec<u8> in map 1 048 860 | Vec<String> 2 228 508
//! JSON amplification (wire bytes -> parsed Value kept in the map):
//!         989 B ->    7 623 B retained (8.00x)     15 921 B ->  112 529 B (7.09x)
//!       259 937 B -> 1 793 577 B (6.90x)         4 237 633 B -> 28 727 065 B (6.78x)
//! integrity: 0 full probe chains, and every case freed everything it allocated
//! size_of: priced Entry 32 B and its HashMap block 48 B | pre-pricing bare-value HashMap 48 B
//!        (both maps are boxed in one 48 B block; the entries differ, not the map)
//! ```
//!
//! Both container rows live in this one artifact deliberately. The first version of
//! `EXTENSION_ENTRY_MEMORY` (128) was priced from the pre-pricing shape's ~101-byte marginal cost,
//! and the very same track then gave `Entry` two more `u64` fields, which is 16 bytes per slot and a
//! different table growth ladder. A container price has to be measured against the shape that ships;
//! keeping the old shape beside it makes the correction checkable instead of asserted. 160 rounds the
//! ~148-byte marginal up, because over-pricing a container is safe and under-pricing it is what these
//! constants exist to avoid. The 24-entry rows read identically to the 16-entry rows, i.e. the table did
//! not grow between them: the marginal figure is the 2-to-16 slope, not a promise about every single
//! insertion.
//!
//! Why the rows are trustworthy for a 48-byte map entry: the wrapper allocator tags each block with
//! the measurement epoch that allocated it and reports, per epoch, the peak live bytes of that
//! epoch's blocks and the bytes still live when the case's value dropped (the leak check). Frees of
//! blocks belonging to another epoch - this run excluded 16 777 472 B of them, mostly the JSON
//! fixtures built between cases and the runtime's own lazy work - are counted separately and never
//! enter a row, which is exactly what a raw before/after counter cannot do: it charges whichever
//! case happens to trigger the foreign free. A full probe chain would silently lose a record, so the
//! run reports its own table integrity in the last line above.

use std::alloc::{GlobalAlloc, Layout, System};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering::Relaxed};

const TABLE: usize = 1 << 20;
const PROBE: usize = 64;
// Slot states: 0 = never used, 1 = tombstone (freed), anything else = the live block's pointer.
// Deletion leaves a tombstone instead of an empty slot, because zeroing a mid-chain slot makes the
// next search stop early and silently loses records — which is what corrupted the first run's
// multi-thousand-allocation cases.
const EMPTY: usize = 0;
const TOMB: usize = 1;

static KEYS: [AtomicUsize; TABLE] = [const { AtomicUsize::new(0) }; TABLE];
static META: [AtomicUsize; TABLE] = [const { AtomicUsize::new(0) }; TABLE]; // size | epoch<<24

static EPOCH: AtomicUsize = AtomicUsize::new(1);
static EPOCH_LIVE: AtomicI64 = AtomicI64::new(0);
static EPOCH_PEAK: AtomicI64 = AtomicI64::new(0);
static FOREIGN_FREE: AtomicI64 = AtomicI64::new(0);
static LOST_SLOTS: AtomicI64 = AtomicI64::new(0);

struct Counting;

#[inline]
fn slot_of(p: usize) -> usize {
    // Multiplicative hash: masking the raw pointer clustered badly on the Windows heap, which made
    // probe chains overflow and lost records (the first run's multi-megabyte cases).
    (p.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (usize::BITS - 20)) as usize
}

#[inline]
fn pack(size: usize, epoch: usize) -> usize {
    size | (epoch << 24)
}

impl Counting {
    fn insert(&self, p: usize, size: usize, epoch: usize) {
        let mut i = slot_of(p);
        let mut first_free = None;
        for _ in 0..PROBE {
            let k = KEYS[i].load(Relaxed);
            if k == p {
                META[i].store(pack(size, epoch), Relaxed);
                return;
            }
            if k == EMPTY || k == TOMB {
                if first_free.is_none() {
                    first_free = Some(i);
                }
            }
            i = (i + 1) & (TABLE - 1);
        }
        if let Some(j) = first_free {
            KEYS[j].store(p, Relaxed);
            META[j].store(pack(size, epoch), Relaxed);
            return;
        }
        LOST_SLOTS.fetch_add(1, Relaxed);
    }
    fn remove(&self, p: usize) -> Option<(usize, usize)> {
        let mut i = slot_of(p);
        for _ in 0..PROBE {
            let k = KEYS[i].load(Relaxed);
            if k == EMPTY {
                return None;
            }
            if k == p {
                let m = META[i].swap(0, Relaxed);
                KEYS[i].store(TOMB, Relaxed);
                return Some((m & 0xFF_FFFF, m >> 24));
            }
            i = (i + 1) & (TABLE - 1);
        }
        None
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let e = EPOCH.load(Relaxed);
            self.insert(p as usize, l.size(), e);
            let live = EPOCH_LIVE.fetch_add(l.size() as i64, Relaxed) + l.size() as i64;
            if live > EPOCH_PEAK.load(Relaxed) {
                EPOCH_PEAK.store(live, Relaxed);
            }
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        match self.remove(p as usize) {
            Some((size, epoch)) => {
                if epoch == EPOCH.load(Relaxed) {
                    EPOCH_LIVE.fetch_sub(size as i64, Relaxed);
                } else {
                    FOREIGN_FREE.fetch_add(size as i64, Relaxed);
                }
            }
            None => {
                FOREIGN_FREE.fetch_add(l.size() as i64, Relaxed);
            }
        }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            let e = EPOCH.load(Relaxed);
            if let Some((size, _)) = self.remove(p as usize) {
                if size == n {
                    self.insert(q as usize, n, e);
                    return q;
                }
                if e == EPOCH.load(Relaxed) {
                    EPOCH_LIVE.fetch_sub(size as i64, Relaxed);
                } else {
                    FOREIGN_FREE.fetch_add(size as i64, Relaxed);
                }
            }
            self.insert(q as usize, n, e);
            let live = EPOCH_LIVE.fetch_add(n as i64, Relaxed) + n as i64;
            if live > EPOCH_PEAK.load(Relaxed) {
                EPOCH_PEAK.store(live, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Runs one isolated case. Returns (peak bytes the case held, bytes it left behind).
fn measure<F: FnOnce() -> T, T>(f: F) -> (i64, i64) {
    EPOCH.fetch_add(1, Relaxed);
    EPOCH_LIVE.store(0, Relaxed);
    EPOCH_PEAK.store(0, Relaxed);
    let held = f();
    let peak = EPOCH_PEAK.load(Relaxed);
    drop(held);
    let left = EPOCH_LIVE.load(Relaxed);
    (peak, left)
}

fn live() -> i64 {
    EPOCH_LIVE.load(Relaxed)
}

/// The map shape as `framework/aether/src/extensions.rs` holds it after track 74 priced it: the
/// boxed `HashMap` keyed by `TypeId`, whose value is `Entry { value, price, charged }` rather than a
/// bare `Box<dyn Any>`. Measured against this shape, because this is the shape whose container cost
/// the price has to cover.
type Values = HashMap<TypeId, Entry>;

struct Entry {
    value: Box<dyn Any + Send + Sync>,
    price: u64,
    charged: u64,
}

#[derive(Default)]
struct Extensions {
    values: Option<Box<Values>>,
    charged: u64,
}

/// The same map before track 74 priced it: `HashMap<TypeId, Box<dyn Any + Send + Sync>>`. Measured
/// here too, so the 128-byte draft of `EXTENSION_ENTRY_MEMORY` has its number in this artifact
/// instead of only in the history of the discussion — that draft was this shape's marginal cost.
#[derive(Default)]
struct Bare {
    values: Option<Box<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

/// Insert one already-boxed payload into either shape. The box is allocated by the caller inside
/// the measured epoch, so the difference between the two shapes' rows is the container entry alone.
trait Put {
    fn put(&mut self, id: TypeId, value: Box<dyn Any + Send + Sync>);
}

impl Put for Extensions {
    fn put(&mut self, id: TypeId, value: Box<dyn Any + Send + Sync>) {
        // `price` and `charged` are plain `u64` fields: what they hold allocates nothing, so any
        // plausible price names the shape without changing a single measured byte.
        const PRICE: u64 = 160 + 4;
        self.values
            .get_or_insert_with(|| Box::new(Values::new()))
            .insert(
                id,
                Entry {
                    value,
                    price: PRICE,
                    charged: PRICE,
                },
            );
        self.charged += PRICE;
    }
}

impl Put for Bare {
    fn put(&mut self, id: TypeId, value: Box<dyn Any + Send + Sync>) {
        self.values
            .get_or_insert_with(|| Box::new(HashMap::new()))
            .insert(id, value);
    }
}

/// The real insertion path, used by the payload and amplification sections below.
impl Extensions {
    fn new() -> Self {
        Self::default()
    }
    fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        let price = 160 + std::mem::size_of::<T>() as u64;
        let values = self.values.get_or_insert_with(|| Box::new(Values::new()));
        let previous = values.insert(
            TypeId::of::<T>(),
            Entry {
                value: Box::new(value),
                price,
                charged: price,
            },
        );
        self.charged += price;
        previous
            .and_then(|entry| entry.value.downcast::<T>().ok())
            .map(|previous| *previous)
    }
}

/// Peak bytes held while inserting `k` entries for `k` in the ladder. One `u32` payload per entry,
/// so every row is the same size of value and only the container varies.
fn container_rows<M: Default + Put>(ids: &[TypeId]) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for k in [1usize, 2, 4, 8, 16, 24] {
        let (cost, left) = measure(move || {
            let mut m = M::default();
            for id in ids.iter().take(k) {
                m.put(*id, Box::new(1u32));
            }
            m
        });
        out.push((
            format!(
                "  {k:>2} entries: {cost:>6} B peak  ({:.1} B/entry)",
                cost as f64 / k as f64
            ),
            left,
        ));
    }
    out
}

/// One distinct `TypeId` per class, so a map entry here is exactly a middleware's map entry.
struct Principal(String);
struct Blob(Vec<u8>);
struct Json(serde_json::Value);
struct Bag(Vec<String>);

macro_rules! distinct {
    ($($n:ident),+) => {
        $( struct $n(#[allow(dead_code)] u32); )+
    };
}
distinct!(
    D1, D2, D3, D4, D5, D6, D7, D8, D9, D10, D11, D12, D13, D14, D15, D16, D17, D18, D19, D20, D21,
    D22, D23, D24
);

fn json_doc(items: usize) -> String {
    let mut s = String::with_capacity(items * 120);
    s.push_str("{\"records\":[");
    for i in 0..items {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"id\":{i},\"name\":\"record-{i}\",\"active\":true,\"tags\":[\"a\",\"b\",\"c\"],\"note\":\"a plain english sentence of about forty-five bytes\"}}"
        ));
    }
    s.push_str("]}");
    s
}

fn main() {
    let mut rows: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    macro_rules! case {
        ($label:expr, $body:expr) => {{
            let (peak, left) = measure($body);
            if left != 0 {
                failures.push(format!("  {} left {left} bytes behind", $label));
            }
            (peak, left)
        }};
    }

    rows.push("== 1. container alone: entries added by distinct type, u32 payloads ==".to_string());
    let ids: Vec<TypeId> = vec![
        TypeId::of::<D1>(),
        TypeId::of::<D2>(),
        TypeId::of::<D3>(),
        TypeId::of::<D4>(),
        TypeId::of::<D5>(),
        TypeId::of::<D6>(),
        TypeId::of::<D7>(),
        TypeId::of::<D8>(),
        TypeId::of::<D9>(),
        TypeId::of::<D10>(),
        TypeId::of::<D11>(),
        TypeId::of::<D12>(),
        TypeId::of::<D13>(),
        TypeId::of::<D14>(),
        TypeId::of::<D15>(),
        TypeId::of::<D16>(),
        TypeId::of::<D17>(),
        TypeId::of::<D18>(),
        TypeId::of::<D19>(),
        TypeId::of::<D20>(),
        TypeId::of::<D21>(),
        TypeId::of::<D22>(),
        TypeId::of::<D23>(),
        TypeId::of::<D24>(),
    ];
    rows.push("  priced shape (Entry { value, price, charged }) — what the framework builds:".to_string());
    for (row, left) in container_rows::<Extensions>(&ids) {
        if left != 0 {
            failures.push(format!("  priced {row} left {left} bytes behind"));
        }
        rows.push(row);
    }
    rows.push("  pre-pricing shape (Box<dyn Any> as the value) — where 128 came from:".to_string());
    for (row, left) in container_rows::<Bare>(&ids) {
        if left != 0 {
            failures.push(format!("  pre-pricing {row} left {left} bytes behind"));
        }
        rows.push(row);
    }

    rows.push("== 2. one entry, payload of class/size (peak bytes held by map + value) ==".to_string());
    for len in [16usize, 64, 512, 4096, 65_536, 1 << 20] {
        let (bare, _) = case!(format!("boxed {len}"), move || Principal("x".repeat(len)));
        let (string_in_map, _) = case!(format!("String {len} in map"), move || {
            let mut m = Extensions::new();
            m.insert(Principal("x".repeat(len)));
            m
        });
        let (vec_in_map, _) = case!(format!("Vec {len} in map"), move || {
            let mut m = Extensions::new();
            m.insert(Blob(vec![7u8; len]));
            m
        });
        let (bag_in_map, _) = case!(format!("Bag {len} in map"), move || {
            let mut m = Extensions::new();
            m.insert(Bag((0..len / 16).map(|i| format!("flag-{i}")).collect()));
            m
        });
        rows.push(format!(
            "  {len:>8} B payload: boxed value alone {bare:>8} | String in map {string_in_map:>8} | Vec<u8> in map {vec_in_map:>8} | Vec<String> in map {bag_in_map:>8}"
        ));
    }

    rows.push("== 3. amplification: wire bytes -> parsed value kept in the map ==".to_string());
    for items in [8usize, 128, 2048, 32_768] {
        let doc = json_doc(items);
        let wire = doc.len() as i64;
        let (value_alone, _) = case!(format!("json {items}"), move || {
            let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
            v
        });
        let doc2 = json_doc(items);
        let (in_map, _) = case!(format!("json {items} in map"), move || {
            let mut m = Extensions::new();
            let v: serde_json::Value = serde_json::from_str(&doc2).unwrap();
            m.insert(Json(v));
            m
        });
        rows.push(format!(
            "  {items:>6} records: wire {wire:>9} B -> parsed Value {value_alone:>9} B -> in map {in_map:>9} B  (retained/wire {:.2}x)",
            in_map as f64 / wire as f64
        ));
    }

    rows.push("== 4. what the budget already prices, for scale ==".to_string());
    rows.push("  CONN_BASE_MEMORY   32768 B  (framework/aether/src/scheduling.rs)".to_string());
    rows.push("  STREAM_BASE_MEMORY 16384 B  (framework/aether/src/http2.rs)".to_string());
    rows.push(format!(
        "  size_of: priced Entry {} B, its HashMap block {} B | pre-pricing bare-value HashMap block {} B",
        std::mem::size_of::<Entry>(),
        std::mem::size_of::<Values>(),
        std::mem::size_of::<HashMap<TypeId, Box<dyn Any + Send + Sync>>>(),
    ));
    rows.push("  request/response body blocks: charged by capacity (HTTP/1 read buffer, h2 price_response_body)".to_string());
    rows.push(format!(
        "  runtime activity the rows exclude: {} B of foreign frees, {} full probe chains",
        FOREIGN_FREE.load(Relaxed),
        LOST_SLOTS.load(Relaxed)
    ));

    for row in &rows {
        println!("{row}");
    }
    println!("peak readings above are epoch-scoped: bytes the case allocated and had not freed at its high-water mark");
    if failures.is_empty() {
        println!("every case freed everything it allocated");
    } else {
        println!("CASES THAT LEFT BYTES BEHIND:");
        for f in failures {
            println!("{f}");
        }
    }
    let _ = live();
}
