//! Request-scoped values: a type-keyed map that travels with one request.
//!
//! A middleware that decides something the handler needs to know — an authenticated principal, a
//! tenant id, a feature flag resolved for this call — has no place to put it today except shared
//! state, which is process-wide and therefore wrong for per-request facts. This map is that place,
//! and it is the same mechanism Axum's `http::Extensions` and Actix's `req.extensions()` provide.
//!
//! Values are keyed by their `TypeId`, so two middlewares storing different types cannot collide and
//! one overwriting another's type is an explicit `insert`, not a string-name typo. A request has one
//! owner at a time (`RequestContext` moves through the middleware chain into the handler), which is
//! why nothing here is locked and why the map is `Send` but not shared.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use aether_scheduler::JobId;

use crate::scheduling::{EXTENSION_ENTRY_MEMORY, SchedulingLayer};

type Values = HashMap<TypeId, Entry>;

/// One stored value, with what the budget was told to hold for it.
struct Entry {
    value: Box<dyn Any + Send + Sync>,
    /// What this entry is worth to the memory budget: the container cost plus the inline size of
    /// the value plus whatever heap its owner declared. Computed at insert, never re-derived — heap
    /// a value grew afterwards is invisible, which is why [`Extensions::insert_sized`] is priced at
    /// the moment the value is handed over.
    price: u64,
    /// Of `price`, the bytes a live registration actually accepted. Zero without a scheduling layer,
    /// and zero after a refusal.
    charged: u64,
}

/// The registration one running request charges its payloads against: the handler job
/// [`dispatch`](crate::serve::ServiceCore) admitted for this request, which lives exactly as long
/// as the context does. Held by the map so an `insert` from any owner of the context — middleware,
/// the handler, an extractor — reaches the same registration.
#[derive(Clone)]
struct Ticket {
    layer: Arc<SchedulingLayer>,
    id: JobId,
}

/// A type-map of values attached to one request.
///
/// Empty costs nothing: the map is boxed lazily on the first `insert`, so a request that never uses
/// extensions allocates no more than it did before this feature existed.
///
/// Priced, and priced only as far as it can be. Every entry adds [`EXTENSION_ENTRY_MEMORY`] plus
/// [`size_of`] of the value to the running request's memory registration, and
/// [`insert_sized`](Self::insert_sized) adds the heap the value's owner says it holds. What nobody
/// can name is the heap of a type that does not declare it: an arbitrary `Send + Sync + 'static`
/// value has no measurable size from here, and `get_mut` can grow one that was declared. A payload
/// whose heap matters therefore declares it, the same way a body's owner does.
///
/// ```rust,no_run
/// use aether::prelude::*;
///
/// /// What the auth middleware decided about the caller of *this* request.
/// struct Principal(String);
///
/// let app = AetherApp::new();
/// let app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
///     let name = req.header("authorization").unwrap_or("anonymous").to_string();
///     req.insert_extension(Principal(name));
///     next(req).await
/// });
/// let app = app.get_async("/whoami", |req: RequestContext| async move {
///     match req.extension::<Principal>() {
///         Some(principal) => Response::text_dynamic(principal.0.clone()),
///         None => Response::text_dynamic("anonymous".to_string()),
///     }
/// });
/// # let _ = app;
/// ```
///
/// The same map reached as a typed handler argument. `Extension<T>` clones the value out of a map
/// that has one owner, so it requires `Clone` — the same bound Axum's `Extension<T>` carries; a
/// value deliberately left non-`Clone` is reachable by borrow through
/// [`RequestContext::extension`](crate::app::RequestContext::extension).
///
/// ```rust,no_run
/// use aether::prelude::*;
///
/// #[derive(Clone)]
/// struct Tenant(u32);
///
/// # fn example(app: AetherApp) -> AetherApp {
/// let app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
///     req.insert_extension(Tenant(7));
///     next(req).await
/// });
/// app.get_async("/plan", |Extension(tenant): Extension<Tenant>| async move {
///     format!("tenant {}", tenant.0)
/// })
/// # }
/// ```
#[derive(Default)]
pub struct Extensions {
    values: Option<Box<Values>>,
    ticket: Option<Ticket>,
    charged: u64,
}

impl Extensions {
    /// An empty map, without allocating.
    #[inline]
    pub fn new() -> Self {
        Self {
            values: None,
            ticket: None,
            charged: 0,
        }
    }

    /// Store `value` under its own type, returning the value previously stored there.
    ///
    /// Charges the request's registration the container plus the inline size of `T`. Heap the value
    /// owns is invisible to this, because an arbitrary `TypeId` value cannot be sized from here: an
    /// owner that knows its size states it with [`Self::insert_sized`].
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.insert_sized(value, 0)
    }

    /// Store `value` and add `owned_heap` to what the budget holds for it: the bytes this value
    /// points at rather than contains, as its owner counts them.
    ///
    /// This is the only way a per-request payload can be priced at all — `size_of` sees the 24 bytes
    /// of a `String`, not the megabyte behind it — so a middleware that attaches a parsed body, a
    /// decompressed document or a buffered report declares its size here. The declaration is priced
    /// at insert: heap a value grows afterwards through [`Self::get_mut`] is not re-measured, and a
    /// value dropped from the map with [`Self::remove`] gives its charge back.
    pub fn insert_sized<T: Send + Sync + 'static>(
        &mut self,
        value: T,
        owned_heap: usize,
    ) -> Option<T> {
        let price = EXTENSION_ENTRY_MEMORY + std::mem::size_of::<T>() as u64 + owned_heap as u64;
        let key = TypeId::of::<T>();
        let previous_charged = self
            .values
            .as_ref()
            .and_then(|values| values.get(&key))
            .map(|entry| entry.charged)
            .unwrap_or(0);
        let previous = {
            let values = self.values.get_or_insert_with(|| Box::new(Values::new()));
            values.insert(
                key,
                Entry {
                    value: Box::new(value),
                    price,
                    charged: 0,
                },
            )
        };
        // Release what the overwritten entry held before asking for the new one, so a refusal
        // cannot leave the old value's bytes registered against nothing.
        if previous_charged != 0 {
            self.adjust(-(previous_charged as i64));
        }
        if self.adjust(price as i64)
            && let Some(entry) = self.values.as_mut().and_then(|v| v.get_mut(&key))
        {
            entry.charged = price;
        }
        previous
            .and_then(|entry| entry.value.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }

    /// The value stored under `T`'s type, if any.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.values
            .as_ref()?
            .get(&TypeId::of::<T>())
            .and_then(|stored| stored.value.downcast_ref::<T>())
    }

    /// The value stored under `T`'s type, for in-place mutation.
    ///
    /// Mutation does not re-price: an entry is charged for what it was when it was inserted (see
    /// [`Self::insert_sized`]), so growing a value's heap here is a debt the budget was never told.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.values
            .as_mut()?
            .get_mut(&TypeId::of::<T>())
            .and_then(|stored| stored.value.downcast_mut::<T>())
    }

    /// Remove and return the value stored under `T`'s type — the way a middleware hands ownership
    /// of one parsed structure onward without copying it. Whatever the budget held for it is given
    /// back.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        let entry = self.values.as_mut()?.remove(&TypeId::of::<T>())?;
        if entry.charged != 0 {
            self.adjust(-(entry.charged as i64));
        }
        entry.value.downcast::<T>().ok().map(|boxed| *boxed)
    }

    /// How many values are stored.
    pub fn len(&self) -> usize {
        self.values.as_ref().map_or(0, |values| values.len())
    }

    /// Whether nothing is stored — equivalent to `len() == 0`, and true for a fresh map.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes this map has asked the scheduler's memory budget to register for the values it holds:
    /// the sum of the entries [`Self::insert`] and [`Self::insert_sized`] priced, minus what
    /// [`Self::remove`] gave back.
    ///
    /// Zero in two different situations, and the difference is not this number's to state: a map
    /// whose request runs without a scheduling layer never asked, and a grow the budget refused was
    /// never accepted. `false` from an insert is therefore a *registration* fact, not a signal that
    /// the value was rejected — the value stays in the map either way, exactly as a response body
    /// stays in the stream when its pricing is refused.
    #[must_use]
    pub fn charged_bytes(&self) -> u64 {
        self.charged
    }

    /// Point this map's charges at the registration of the request that is running. Called by the
    /// serving core once the handler job has been admitted; a map nobody attached keeps its prices
    /// to itself, which is what makes the type usable outside a request.
    pub(crate) fn attach_charge(&mut self, layer: Arc<SchedulingLayer>, id: JobId) {
        self.ticket = Some(Ticket { layer, id });
        // Values stored before the ticket existed are priced now, so a context built outside a
        // dispatch and then run inside one does not silently keep unpriced entries.
        let unpriced: Vec<(TypeId, u64)> = self
            .values
            .as_ref()
            .map(|values| {
                values
                    .iter()
                    .filter(|(_, entry)| entry.charged == 0)
                    .map(|(key, entry)| (*key, entry.price))
                    .collect()
            })
            .unwrap_or_default();
        for (key, price) in unpriced {
            if self.adjust(price as i64)
                && let Some(entry) = self.values.as_mut().and_then(|v| v.get_mut(&key))
            {
                entry.charged = price;
            }
        }
    }

    /// Ask this request's registration for `delta` bytes, or give `delta` back. A map with no
    /// ticket, a job that has already ended, or a budget that cannot cover the increase accepts
    /// nothing and changes nothing.
    fn adjust(&mut self, delta: i64) -> bool {
        if delta == 0 {
            return true;
        }
        let Some(ticket) = &self.ticket else {
            return false;
        };
        if ticket.layer.adjust_memory(ticket.id, delta) {
            self.charged = (self.charged as i64 + delta) as u64;
            true
        } else {
            false
        }
    }
}

impl fmt::Debug for Extensions {
    /// Reports the count and the registered bytes, never the values. The values are arbitrary
    /// application types, so there is no correctness-preserving way to print them and a wrong one
    /// would leak request data into logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions")
            .field("count", &self.len())
            .field("charged", &self.charged)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::Extensions;
    use crate::scheduling::{AdmissionGuard, EXTENSION_ENTRY_MEMORY, SchedulingLayer};
    use aether_scheduler::{NewJob, ResourceConfig, SchedulingConfig};
    use std::sync::Arc;

    #[derive(Debug, PartialEq)]
    struct Principal(&'static str);

    #[derive(Debug, PartialEq)]
    struct Tenant(u32);

    #[test]
    fn a_fresh_map_holds_nothing_and_has_not_allocated() {
        let map = Extensions::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.get::<Principal>(), None);
        // The lazy box is the whole cost of the feature on an unused request.
        assert!(map.values.is_none());
    }

    #[test]
    fn values_are_keyed_by_type_not_by_name() {
        let mut map = Extensions::new();
        map.insert(Principal("hamza"));
        map.insert(Tenant(42));
        assert_eq!(map.get::<Principal>(), Some(&Principal("hamza")));
        assert_eq!(map.get::<Tenant>(), Some(&Tenant(42)));
        assert_eq!(map.len(), 2);
        assert_eq!(map.get::<String>(), None);
    }

    #[test]
    fn inserting_a_type_twice_returns_the_previous_value() {
        let mut map = Extensions::new();
        assert_eq!(map.insert(Tenant(1)), None);
        assert_eq!(map.insert(Tenant(2)), Some(Tenant(1)));
        assert_eq!(map.get::<Tenant>(), Some(&Tenant(2)));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn remove_hands_over_ownership_and_leaves_nothing() {
        let mut map = Extensions::new();
        map.insert(Principal("hamza"));
        assert_eq!(map.remove::<Principal>(), Some(Principal("hamza")));
        assert_eq!(map.remove::<Principal>(), None);
        assert!(map.is_empty());
    }

    #[test]
    fn get_mut_writes_through_to_the_stored_value() {
        let mut map = Extensions::new();
        map.insert(Tenant(1));
        if let Some(tenant) = map.get_mut::<Tenant>() {
            tenant.0 = 7;
        }
        assert_eq!(map.get::<Tenant>(), Some(&Tenant(7)));
    }

    #[test]
    fn a_stored_value_can_be_a_owned_allocation_and_survives_the_map_being_moved() {
        let mut map = Extensions::new();
        map.insert(vec![1u8, 2, 3]);
        let moved = map;
        assert_eq!(moved.get::<Vec<u8>>().map(Vec::len), Some(3));
    }

    #[test]
    fn debug_reports_a_count_and_not_the_values() {
        let mut map = Extensions::new();
        map.insert(Principal("secret-principal"));
        let rendered = format!("{map:?}");
        assert!(rendered.contains("count"), "{rendered}");
        assert!(!rendered.contains("secret-principal"), "{rendered}");
    }

    #[test]
    fn the_map_is_send_and_sync_because_a_request_crosses_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Extensions>();
    }

    /// One admitted handler job — the registration a running request's values are charged against —
    /// plus a map attached to it. `budget` is the whole ceiling, so a small one makes a grow refuse.
    struct Running {
        layer: Arc<SchedulingLayer>,
        guard: AdmissionGuard,
        registered_at_admission: u64,
    }

    impl Running {
        async fn with_budget(budget: u64) -> Self {
            let layer = SchedulingLayer::new(
                SchedulingConfig::default(),
                ResourceConfig {
                    memory_budget_bytes: budget,
                    ..ResourceConfig::default()
                },
            );
            let guard = layer
                .admit(NewJob::handler(0))
                .await
                .expect("a zero-memory handler job is admissible");
            let registered_at_admission = layer.resources().registered_memory_bytes;
            Self {
                layer,
                guard,
                registered_at_admission,
            }
        }

        fn registered(&self) -> u64 {
            self.layer.resources().registered_memory_bytes
        }

        fn map(&self) -> Extensions {
            let mut map = Extensions::new();
            map.attach_charge(Arc::clone(&self.layer), self.guard.id());
            map
        }

        /// Ends the request's job and hands the layer back, so a test can read what the release left
        /// behind without borrowing the (partially moved) registration it just completed.
        fn complete(self) -> Arc<SchedulingLayer> {
            let layer = Arc::clone(&self.layer);
            self.guard.complete();
            layer
        }
    }

    const TENANT_PRICE: u64 = EXTENSION_ENTRY_MEMORY + (std::mem::size_of::<Tenant>() as u64);

    /// The rule PROB-0016 asked for, tested on the registration rather than described: a value a
    /// middleware attaches raises what the scheduler holds for the running request, and the increase
    /// goes back when that request's job ends — not when the map is dropped, which is the same
    /// moment today but is not the mechanism.
    #[tokio::test]
    async fn an_inserted_value_is_registered_against_the_running_request() {
        let running = Running::with_budget(1024 * 1024).await;
        let mut map = running.map();
        assert_eq!(running.registered(), running.registered_at_admission);

        map.insert(Tenant(42));

        assert_eq!(map.charged_bytes(), TENANT_PRICE);
        assert_eq!(
            running.registered(),
            running.registered_at_admission + TENANT_PRICE,
            "the container cost plus the inline size of the value, which is all a u32 is"
        );

        // The job, not the map, is what releases it: the request's registration ends with the job.
        drop(map);
        let layer = running.complete();
        assert_eq!(layer.resources().registered_memory_bytes, 0);
    }

    /// `size_of` cannot see the bytes behind a `String`, and nothing else can: the 1 MiB the test
    /// declares is exactly what the budget gains, and the same value inserted without a declaration
    /// is charged 24 bytes plus the container. This is the whole difference between the two entry
    /// points, and the reason the declaration is opt-in instead of derived.
    #[tokio::test]
    async fn a_declared_heap_is_the_only_way_a_pointing_value_is_priced() {
        const HEAP: usize = 1024 * 1024;
        let running = Running::with_budget(8 * 1024 * 1024).await;
        let string_price = EXTENSION_ENTRY_MEMORY + std::mem::size_of::<String>() as u64;

        let mut undeclared = running.map();
        undeclared.insert("x".repeat(HEAP));
        assert_eq!(undeclared.charged_bytes(), string_price);
        assert_eq!(
            running.registered(),
            running.registered_at_admission + string_price,
            "a megabyte behind a 24-byte handle, priced as 24 bytes plus the container"
        );
        drop(undeclared);
        let layer = running.complete();
        assert_eq!(layer.resources().registered_memory_bytes, 0);

        let running = Running::with_budget(8 * 1024 * 1024).await;
        let mut declared = running.map();
        declared.insert_sized("x".repeat(HEAP), HEAP);
        assert_eq!(
            declared.charged_bytes(),
            string_price + HEAP as u64,
            "the declaration is added to what insert already priced, never a replacement for it"
        );
        assert_eq!(
            running.registered(),
            running.registered_at_admission + string_price + HEAP as u64
        );
    }

    #[tokio::test]
    async fn removing_a_value_gives_its_charge_back() {
        const HEAP: usize = 256 * 1024;
        let running = Running::with_budget(4 * 1024 * 1024).await;
        let mut map = running.map();
        map.insert_sized(vec![7u8; HEAP], HEAP);
        let priced = map.charged_bytes();
        assert_eq!(
            running.registered(),
            running.registered_at_admission + priced
        );

        let taken = map.remove::<Vec<u8>>().expect("the value is stored");
        assert_eq!(
            taken.len(),
            HEAP,
            "removing hands the value over, not a copy"
        );
        assert_eq!(map.charged_bytes(), 0);
        assert_eq!(running.registered(), running.registered_at_admission);
    }

    /// Overwriting a type replaces its entry, so it must replace its charge: a middleware that runs
    /// twice per request cannot be allowed to register the same bytes twice.
    #[tokio::test]
    async fn overwriting_a_type_replaces_rather_than_adds_its_charge() {
        let running = Running::with_budget(1024 * 1024).await;
        let mut map = running.map();
        map.insert(Tenant(1));
        map.insert(Tenant(2));
        assert_eq!(map.len(), 1);
        assert_eq!(map.charged_bytes(), TENANT_PRICE);
        assert_eq!(
            running.registered(),
            running.registered_at_admission + TENANT_PRICE
        );

        let bigger = TENANT_PRICE + 4096;
        map.insert_sized(Tenant(3), 4096);
        assert_eq!(map.charged_bytes(), bigger);
        assert_eq!(
            running.registered(),
            running.registered_at_admission + bigger
        );
    }

    /// A budget that cannot cover the increase is a *registration* refusal, not a refusal of the
    /// value: the payload stays in the map, the answer is still served, and nothing is registered —
    /// the same best-effort shape `price_response_body` has, and the reason a refusal feeds the
    /// scheduler's pressure window instead of dropping the request.
    #[tokio::test]
    async fn a_refused_grow_keeps_the_value_and_registers_nothing() {
        let running = Running::with_budget(TENANT_PRICE - 1).await;
        let mut map = running.map();
        map.insert(Tenant(42));
        assert_eq!(
            map.get::<Tenant>(),
            Some(&Tenant(42)),
            "the value is stored even though its bytes were not affordable"
        );
        assert_eq!(map.charged_bytes(), 0);
        assert_eq!(running.registered(), running.registered_at_admission);
        let shed = running.layer.metrics().memory_shed;
        assert!(
            shed > 0,
            "a refused grow counts as shedding real work, got {shed}"
        );
    }

    /// The other half of the release rule: a context that outlives its request — returned to a pool,
    /// or held by a handler that runs past the response — must not keep registering bytes against a
    /// job that has ended, and must not panic doing it.
    #[tokio::test]
    async fn a_ticket_whose_job_has_ended_prices_nothing_and_holds_the_value() {
        let running = Running::with_budget(4 * 1024 * 1024).await;
        let layer = Arc::clone(&running.layer);
        let mut map = running.map();
        let id = running.guard.id();
        running.guard.complete();

        map.insert_sized(vec![1u8; 64 * 1024], 64 * 1024);
        assert_eq!(
            map.charged_bytes(),
            0,
            "the job is gone, so there is no registration to grow"
        );
        assert_eq!(layer.resources().registered_memory_bytes, 0);
        assert_eq!(map.get::<Vec<u8>>().map(Vec::len), Some(64 * 1024));
        assert!(
            !layer.adjust_memory(id, 1),
            "and the stale job answers false"
        );
    }

    /// Attach is what `dispatch` does between admission and the handler call. A map that already
    /// holds values — built before the request was admitted, then run inside one — is priced at
    /// attach, so the ordering cannot silently leave entries unpriced.
    #[tokio::test]
    async fn entries_stored_before_the_ticket_exists_are_priced_at_attach() {
        let mut map = Extensions::new();
        map.insert(Tenant(1));
        map.insert_sized(vec![9u8; 1024], 1024);
        assert_eq!(map.charged_bytes(), 0, "no request, no registration");

        let running = Running::with_budget(4 * 1024 * 1024).await;
        map.attach_charge(Arc::clone(&running.layer), running.guard.id());

        let vec_price = EXTENSION_ENTRY_MEMORY + std::mem::size_of::<Vec<u8>>() as u64 + 1024;
        assert_eq!(map.charged_bytes(), TENANT_PRICE + vec_price);
        assert_eq!(
            running.registered(),
            running.registered_at_admission + TENANT_PRICE + vec_price
        );
    }

    /// A map used outside any request (the type is public, and the unit tests above build one
    /// directly) keeps working and registers nothing.
    #[test]
    fn a_map_without_a_request_holds_values_and_registers_nothing() {
        let mut map = Extensions::new();
        map.insert(Principal("hamza"));
        map.insert_sized(Tenant(7), 4096);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get::<Principal>(), Some(&Principal("hamza")));
        assert_eq!(map.charged_bytes(), 0);
        assert_eq!(map.remove::<Tenant>(), Some(Tenant(7)));
        assert_eq!(map.len(), 1);
    }

    /// The debug surface reports how much was registered and still refuses to print a value.
    #[tokio::test]
    async fn debug_reports_the_registered_bytes_and_still_not_the_values() {
        let running = Running::with_budget(1024 * 1024).await;
        let mut map = running.map();
        map.insert(Principal("secret-principal"));
        let rendered = format!("{map:?}");
        assert!(rendered.contains("count"), "{rendered}");
        assert!(rendered.contains("charged"), "{rendered}");
        assert!(
            rendered.contains(
                &(EXTENSION_ENTRY_MEMORY + std::mem::size_of::<Principal>() as u64).to_string()
            ),
            "{rendered}"
        );
        assert!(!rendered.contains("secret-principal"), "{rendered}");
    }
}
