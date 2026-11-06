//! # Segment Trie Router with Single-Instruction Bitmask Method Matching
//!
//! Paths are split into `/`-separated segments and matched against a trie where each segment
//! consumes exactly one node. This is *not* a compressed radix/prefix tree: there is no edge
//! splitting, so a lookup costs O(segments x siblings), not O(segments). The sibling count per
//! node is small and the scan stays allocation-free on the hot path (params borrow from the
//! request path), which is what actually matters for HTTP routing. If prefix compression ever
//! lands, the name and the complexity claim can meet again.

use crate::node::{HandlerId, NodeType, RouteNode};
use crate::params::{Param, PathParams};
use aether_http::method::Method;

pub struct RadixRouter {
    root: RouteNode,
}

#[derive(Debug)]
pub enum MatchResult<'a> {
    Found {
        handler_id: HandlerId,
        params: PathParams<'a>,
    },
    MethodNotAllowed {
        allowed_bitmask: u64,
    },
    NotFound,
}

/// One registration as [`RadixRouter::route_listing`] reconstructed it: the method, the canonical
/// path template it answers under, and the handler id `add_route` was handed for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredRoute {
    pub method: Method,
    pub path: String,
    pub handler: HandlerId,
}

impl RadixRouter {
    pub fn new() -> Self {
        Self {
            root: RouteNode::new("/".to_string(), NodeType::Static),
        }
    }

    /// The `Allow` value for a target that registered `bitmask`, including the methods the
    /// server answers without a registration.
    ///
    /// The registered set understates the resource. `OPTIONS` is answered on every routable
    /// path by the serving core's generic responder (RFC 9110 §9.3.7), so it is always listed.
    /// `HEAD` is listed only when the caller says it is served — an HTTP `GET` route serves it
    /// from the GET handler with the body dropped (RFC 9110 §9.3.2), while a WebSocket route
    /// refuses every non-`GET` before the handler, and the caller is the only one that knows
    /// which of the two a path is.
    pub fn allow_header_for_target(bitmask: u64, head_served: bool) -> String {
        let mut effective = bitmask | Method::Options.bitmask();
        if head_served {
            effective |= Method::Head.bitmask();
        }
        Self::bitmask_to_allow_header(effective)
    }

    /// Converts a method bitmask to a comma-separated list of HTTP methods for the Allow header
    pub fn bitmask_to_allow_header(bitmask: u64) -> String {
        let mut methods = Vec::new();
        if bitmask & Method::Get.bitmask() != 0 {
            methods.push("GET");
        }
        if bitmask & Method::Post.bitmask() != 0 {
            methods.push("POST");
        }
        if bitmask & Method::Put.bitmask() != 0 {
            methods.push("PUT");
        }
        if bitmask & Method::Delete.bitmask() != 0 {
            methods.push("DELETE");
        }
        if bitmask & Method::Patch.bitmask() != 0 {
            methods.push("PATCH");
        }
        if bitmask & Method::Head.bitmask() != 0 {
            methods.push("HEAD");
        }
        if bitmask & Method::Options.bitmask() != 0 {
            methods.push("OPTIONS");
        }
        if bitmask & Method::Trace.bitmask() != 0 {
            methods.push("TRACE");
        }
        if bitmask & Method::Connect.bitmask() != 0 {
            methods.push("CONNECT");
        }
        methods.join(", ")
    }

    /// تسجيل مسار جديد داخل شجرة التوجيه (مثل `/api/v1/users/{id}` أو `/static/*filepath`).
    ///
    /// يتم تحليل المقاطع وإنشاء العقد الفرعية المناسبة مع فرزها لضمان أسبقية المطابقة الحتمية.
    ///
    /// المقطع الشامل (`*filepath`) يجب أن يكون آخر مقطع في المسار: لأنه يستهلك بقية المسار، فلا
    /// يمكن لأي مقطع بعده أن يطابق، وتسجيله في وسط المسار يُثير `panic` عند التسجيل تمامًا كما يحدث
    /// عند تكرار نفس (`method`, `path`).
    pub fn add_route(&mut self, method: Method, path: &str, handler: HandlerId) {
        let mut curr = &mut self.root;

        if path == "/" || path.is_empty() {
            Self::reject_duplicate(curr, method, path);
            curr.add_handler(method, handler);
            return;
        }

        // Collected because a catch-all has to know whether anything is declared after it.
        let segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        let segments_len = segments.len();

        // The dynamic names this one registration declares, in path order. They are attached to
        // the leaf this route lands on, because a match ends there and only there does the router
        // know which of the sibling routes sharing a radix level answered.
        let mut declared: Vec<String> = Vec::new();

        for (idx, seg) in segments.into_iter().enumerate() {
            let (node_type, is_catchall) = if seg.starts_with('{') && seg.ends_with('}') {
                let name = &seg[1..seg.len() - 1];
                (NodeType::Param(name.to_string()), false)
            } else if let Some(rest) = seg.strip_prefix('*') {
                let name = if rest.is_empty() { "wildcard" } else { rest };
                (NodeType::CatchAll(name.to_string()), true)
            } else {
                (NodeType::Static, false)
            };

            // A node level may hold at most one Param child and one CatchAll child,
            // regardless of the variable's *name*. Matching by name (the previous
            // behaviour) let `/users/{id}` and `/users/{user}` coexist as two siblings;
            // they compete for the exact same request segments, so the second was
            // silently shadowed and `reject_duplicate` never fired. Matching by kind
            // collapses them onto the first-registered node, turning a duplicate method
            // into a startup panic instead of an unreachable handler. A route still extracts
            // under the names it declared — see `RouteNode::param_names`.
            let dyn_name = match &node_type {
                NodeType::Param(n) | NodeType::CatchAll(n) => Some(n.as_str()),
                NodeType::Static => None,
            };
            if let Some(name) = dyn_name {
                declared.push(name.to_string());
            }
            let child_idx = curr
                .children
                .iter()
                .position(|c| match (&c.node_type, &node_type) {
                    (NodeType::Static, NodeType::Static) => c.prefix == seg,
                    (NodeType::Param(_), NodeType::Param(_)) => true,
                    (NodeType::CatchAll(_), NodeType::CatchAll(_)) => true,
                    _ => false,
                });

            if let Some(idx) = child_idx {
                curr = &mut curr.children[idx];
            } else {
                let new_node = RouteNode::new(seg.to_string(), node_type);
                curr.children.push(new_node);

                // فرز العقد الفرعية لضمان الأسبقية الحتمية: Static أولاً، ثم Param، ثم CatchAll
                curr.children.sort_by_key(|c| match &c.node_type {
                    NodeType::Static => 0,
                    NodeType::Param(_) => 1,
                    NodeType::CatchAll(_) => 2,
                });

                let pos = curr.children.iter().position(|c| c.prefix == seg).unwrap();
                curr = &mut curr.children[pos];
            }

            if is_catchall {
                // A catch-all matches the *rest* of the path, so the segments declared after it
                // are not merely unreachable at match time — the handler would be attached to the
                // catch-all node and every `/files/…` request would run it, whatever came after
                // `*rest` in the declaration. Silent, like the duplicate registration that
                // `reject_duplicate` panics on, so panic here too.
                if idx + 1 != segments_len {
                    panic!(
                        "Aether: route '{path}' declares catch-all '{seg}' before its last segment; \
                         a catch-all matches the rest of the path, so nothing after it can ever \
                         be reached",
                    );
                }
                break;
            }
        }

        Self::reject_duplicate(curr, method, path);
        curr.add_handler(method, handler);
        if !declared.is_empty() {
            curr.set_param_names(method, declared);
        }
    }

    /// Visit every registered route with the handler it dispatches to.
    ///
    /// The server-wide `OPTIONS *` answer needs the set of methods that exist anywhere, and it
    /// has to distinguish an HTTP `GET` route from a WebSocket upgrade (only the former answers
    /// `HEAD`). Both are properties of the handler behind each leaf, so walking the tree once
    /// when the server is built is cheaper and more truthful than recomputing a union per
    /// request.
    pub fn for_each_route(&self, mut visit: impl FnMut(Method, HandlerId)) {
        fn walk(node: &RouteNode, visit: &mut impl FnMut(Method, HandlerId)) {
            for (method, handler_id) in &node.handlers {
                visit(*method, *handler_id);
            }
            for child in &node.children {
                walk(child, visit);
            }
        }
        walk(&self.root, &mut visit);
    }

    /// Every registration in the table, as the `(method, path)` pair it answers under plus the
    /// handler behind it, sorted by path then method name.
    ///
    /// `for_each_route` reports a handler id and nothing else, which is enough to build an
    /// `Allow` value but not enough to describe the API: a spec generator, a diagnostics route or
    /// a startup log needs each route's *shape*, and the shape is what this walk reconstructs.
    ///
    /// It cannot simply echo the node prefixes. One level holds one `Param` and one `CatchAll`
    /// child for every route that competes for it, and the child keeps the name of whichever
    /// registration created it — so `/a/{x}/b` and `/a/{y}/c` share a node whose prefix names
    /// only the first of them. The names a request actually gets are per method and stored on
    /// the leaf (`RouteNode::param_names`), so the walk records the dynamic positions on the way
    /// down and stamps each route's own names into them.
    ///
    /// The listing is the canonical form `find` matches against, not the source text it was
    /// declared in: `/users`, `/users/` and `users` all list as `/users`, `""` and `/` list as
    /// `/`, and a bare `*` catch-all lists under the default name the registration gave it
    /// (`/x/*` → `/x/*wildcard`). A route that arrived through a mount lists with its prefix
    /// attached, because the mount moved it into this table.
    ///
    /// The sort is part of the contract, not a courtesy: siblings are ordered by node kind, so an
    /// unsorted walk would list `/b` before `/a` according to which was registered first. Sorting
    /// by `(path, method)` makes the listing — and any document built on it — byte identical
    /// across runs of the same table.
    pub fn route_listing(&self) -> Vec<RegisteredRoute> {
        /// Rebuild one route's path: the walk's current segments, with this route's declared
        /// names stamped into the dynamic positions it passed through.
        fn render(
            segments: &[String],
            dynamic: &[(usize, bool)],
            names: Option<&[String]>,
        ) -> String {
            let mut rendered = segments.to_vec();
            // `add_route` pushes exactly one name per dynamic level on the way to the leaf, so
            // the two lengths agree and `names` is always `Some` here. If they ever disagree the
            // level keeps the node's own name instead of panicking: this is a description of the
            // table, and a wrong one must not take a server down.
            if let Some(names) = names {
                for ((index, catch_all), name) in dynamic.iter().zip(names) {
                    rendered[*index] = if *catch_all {
                        format!("*{name}")
                    } else {
                        format!("{{{name}}}")
                    };
                }
            }
            if rendered.is_empty() {
                return "/".to_string();
            }
            let mut path =
                String::with_capacity(rendered.iter().map(|segment| segment.len() + 1).sum());
            for segment in rendered {
                path.push('/');
                path.push_str(&segment);
            }
            path
        }

        fn walk(
            node: &RouteNode,
            segments: &mut Vec<String>,
            dynamic: &mut Vec<(usize, bool)>,
            listed: &mut Vec<RegisteredRoute>,
        ) {
            for (method, handler) in &node.handlers {
                listed.push(RegisteredRoute {
                    method: *method,
                    path: render(segments, dynamic, node.param_names_for(*method)),
                    handler: *handler,
                });
            }
            for child in &node.children {
                let (segment, kind) = match &child.node_type {
                    NodeType::Static => (child.prefix.clone(), None),
                    NodeType::Param(name) => (format!("{{{name}}}"), Some(false)),
                    NodeType::CatchAll(name) => (format!("*{name}"), Some(true)),
                };
                let position = segments.len();
                segments.push(segment);
                if let Some(catch_all) = kind {
                    dynamic.push((position, catch_all));
                }
                walk(child, segments, dynamic, listed);
                segments.pop();
                if kind.is_some() {
                    dynamic.pop();
                }
            }
        }

        let mut listed = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut Vec::new(), &mut listed);
        listed.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.method.as_str().cmp(b.method.as_str()))
        });
        listed
    }

    /// Duplicate `method + path` registration is a programming error. Silently ignoring the
    /// second registration (the previous behaviour) hides the mistake until runtime, so fail
    /// loudly at startup the way Axum and Actix Web do.
    fn reject_duplicate(node: &RouteNode, method: Method, path: &str) {
        if node.supports_method(method) {
            panic!(
                "Aether: duplicate route registration for {} '{}'; each method+path pair must be unique",
                method.as_str(),
                path
            );
        }
    }

    /// مطابقة المسار وأسلوب HTTP.
    ///
    /// Matching answers two separate questions: does the *path* exist, and is the *method*
    /// allowed on it. Keeping them apart is what makes backtracking correct — a static sibling
    /// that matches the path but not the method must not mask a parameter route that matches
    /// both. A full (path + method) match always wins over any partial one.
    pub fn find<'a>(&'a self, method: Method, path: &'a str) -> MatchResult<'a> {
        let mut params = PathParams::new();
        let trimmed = path.trim_matches('/');

        if trimmed.is_empty() {
            return self.leaf_result(&self.root, method, params);
        }

        // استخدام SmallVec على الـ Stack لتفادي تخصيص مصفوفة على الـ Heap
        let mut segments: smallvec::SmallVec<[&'a str; 8]> = smallvec::SmallVec::new();
        for seg in trimmed.split('/') {
            if !seg.is_empty() {
                segments.push(seg);
            }
        }

        // Accumulate the methods of *every* branch whose path matched. `/users/me` may exist
        // as a static GET route and a param POST route simultaneously; RFC 9110 §15.5.6
        // requires the 405 to advertise all methods the target resource supports, so the
        // union — not the first branch encountered — is the correct Allow value.
        let mut allowed_bitmask: u64 = 0;
        match self.search(
            &self.root,
            method,
            trimmed,
            &segments,
            0,
            &mut params,
            &mut allowed_bitmask,
        ) {
            Search::Found { handler_id, params } => MatchResult::Found { handler_id, params },
            Search::NoMatch if allowed_bitmask != 0 => {
                MatchResult::MethodNotAllowed { allowed_bitmask }
            }
            Search::NoMatch => MatchResult::NotFound,
        }
    }

    fn leaf_result<'a>(
        &'a self,
        node: &'a RouteNode,
        method: Method,
        params: PathParams<'a>,
    ) -> MatchResult<'a> {
        if let Some(handler_id) = node.get_handler(method) {
            MatchResult::Found { handler_id, params }
        } else if node.allowed_methods != 0 {
            MatchResult::MethodNotAllowed {
                allowed_bitmask: node.allowed_methods,
            }
        } else {
            MatchResult::NotFound
        }
    }

    /// Restamp the captured values with the names the route that answered declared.
    ///
    /// A radix level is shared: `/users/{id}` and `/users/{user}/edit` for the same method both
    /// descend through one `Param` child of `/users`, so the key pushed while descending belongs
    /// to the node, not to the route. Only the leaf knows which route matched, and it holds that
    /// route's names in path order — the same order the captures were pushed in.
    fn resolve_keys<'a>(node: &'a RouteNode, method: Method, params: &mut PathParams<'a>) {
        if let Some(names) = node.param_names_for(method) {
            for (param, name) in params.iter_mut().zip(names) {
                param.key = name.as_str();
            }
        }
    }

    /// The traversal state is passed as separate arguments instead of a context struct: the
    /// walk is per-request hot code and every argument except `params` and `allowed_bitmask`
    /// is a copy-free borrow of data the caller already owns.
    #[expect(
        clippy::too_many_arguments,
        reason = "recursive trie walk: bundling these into a struct would add a per-call aggregate without removing any state"
    )]
    fn search<'a>(
        &'a self,
        curr: &'a RouteNode,
        method: Method,
        path: &'a str,
        segments: &[&'a str],
        index: usize,
        params: &mut PathParams<'a>,
        allowed_bitmask: &mut u64,
    ) -> Search<'a> {
        if index == segments.len() {
            if let Some(handler_id) = curr.get_handler(method) {
                let mut found = params.clone();
                Self::resolve_keys(curr, method, &mut found);
                return Search::Found {
                    handler_id,
                    params: found,
                };
            }
            *allowed_bitmask |= curr.allowed_methods;
            return Search::NoMatch;
        }

        let seg = segments[index];

        // 1. Static children first: an exact literal beats any dynamic match.
        for child in &curr.children {
            if let NodeType::Static = &child.node_type
                && child.prefix == seg
            {
                let res = self.search(
                    child,
                    method,
                    path,
                    segments,
                    index + 1,
                    params,
                    allowed_bitmask,
                );
                if matches!(res, Search::Found { .. }) {
                    return res;
                }
            }
        }

        // 2. Single-segment parameters. The key pushed here is the node's own first-declared
        // name and is only a placeholder: `resolve_keys` restamps every capture from the names
        // of the route that actually answered, because a radix level is shared by siblings.
        for child in &curr.children {
            if let NodeType::Param(key) = &child.node_type {
                params.push(Param {
                    key: key.as_str(),
                    value: seg,
                });
                let res = self.search(
                    child,
                    method,
                    path,
                    segments,
                    index + 1,
                    params,
                    allowed_bitmask,
                );
                if matches!(res, Search::Found { .. }) {
                    return res;
                }
                params.pop();
            }
        }

        // 3. Catch-all consumes the entire remaining path, not just the current segment.
        for child in &curr.children {
            if let NodeType::CatchAll(key) = &child.node_type {
                // `seg` is a subslice of `path`, so this offset arithmetic recovers the whole
                // remaining path (including its `/` separators) without allocating.
                let start = seg.as_ptr() as usize - path.as_ptr() as usize;
                let rest = &path[start..];

                params.push(Param {
                    key: key.as_str(),
                    value: rest,
                });
                if let Some(handler_id) = child.get_handler(method) {
                    let mut found = params.clone();
                    Self::resolve_keys(child, method, &mut found);
                    return Search::Found {
                        handler_id,
                        params: found,
                    };
                }
                if child.allowed_methods != 0 {
                    *allowed_bitmask |= child.allowed_methods;
                }
                params.pop();
            }
        }

        Search::NoMatch
    }
}

/// Internal search outcome. A method mismatch is accumulated into `allowed_bitmask` at the
/// leaf rather than returned, so the search can keep looking for a branch that matches both
/// path and method before deciding between 405 and 404.
enum Search<'a> {
    Found {
        handler_id: HandlerId,
        params: PathParams<'a>,
    },
    NoMatch,
}

impl Default for RadixRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radix_routing_with_params_and_precedence() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 10);
        router.add_route(Method::Get, "/users", 20);
        router.add_route(Method::Get, "/users/me", 25); // Exact static route
        router.add_route(Method::Get, "/users/{id}", 30); // Param route
        router.add_route(Method::Post, "/users/{id}/posts/{post_id}", 40);

        // Test root
        match router.find(Method::Get, "/") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 10),
            _ => panic!("Expected root"),
        }

        // Test static precedence over dynamic parameter (/users/me must resolve to 25, not 30)
        match router.find(Method::Get, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 25);
                assert_eq!(params.len(), 0);
            }
            _ => panic!("Expected /users/me to match static route with precedence"),
        }

        // Test param extraction without allocation
        match router.find(Method::Get, "/users/42") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 30);
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].key, "id");
                assert_eq!(params[0].value, "42");
            }
            _ => panic!("Expected user 42"),
        }

        // Test multi-param
        match router.find(Method::Post, "/users/alice/posts/101") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 40);
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].value, "alice");
                assert_eq!(params[1].value, "101");
            }
            _ => panic!("Expected post 101"),
        }

        // Test 405 Method Not Allowed
        match router.find(Method::Delete, "/users/42") {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                assert!(allowed_bitmask & Method::Get.bitmask() != 0);
            }
            _ => panic!("Expected 405"),
        }

        // Test Allow header generation
        let allow =
            RadixRouter::bitmask_to_allow_header(Method::Get.bitmask() | Method::Post.bitmask());
        assert!(allow.contains("GET"));
        assert!(allow.contains("POST"));
        assert!(!allow.contains("PUT"));

        let allow_all = RadixRouter::bitmask_to_allow_header(
            Method::Get.bitmask()
                | Method::Post.bitmask()
                | Method::Put.bitmask()
                | Method::Delete.bitmask(),
        );
        assert!(allow_all.contains("GET"));
        assert!(allow_all.contains("POST"));
        assert!(allow_all.contains("PUT"));
        assert!(allow_all.contains("DELETE"));
    }

    /// `Allow` must describe what the target answers, not what was registered: the serving core
    /// answers `OPTIONS` generically on any routable path, and `HEAD` only where the caller says
    /// it is served (an HTTP `GET` route, never a WebSocket one).
    #[test]
    fn the_allow_value_names_the_methods_the_target_really_answers() {
        assert_eq!(
            RadixRouter::allow_header_for_target(Method::Get.bitmask(), true),
            "GET, HEAD, OPTIONS"
        );
        // A WebSocket route: `GET` without the implied `HEAD`.
        assert_eq!(
            RadixRouter::allow_header_for_target(Method::Get.bitmask(), false),
            "GET, OPTIONS"
        );
        // No `GET` bit, so nothing to inherit either way; `OPTIONS` is always there.
        assert_eq!(
            RadixRouter::allow_header_for_target(Method::Post.bitmask(), false),
            "POST, OPTIONS"
        );
        // Already-registered bits are not duplicated.
        assert_eq!(
            RadixRouter::allow_header_for_target(
                Method::Get.bitmask() | Method::Head.bitmask() | Method::Options.bitmask(),
                true,
            ),
            "GET, HEAD, OPTIONS"
        );
    }

    fn get<'a>(params: &PathParams<'a>, key: &str) -> Option<&'a str> {
        params.iter().find(|p| p.key == key).map(|p| p.value)
    }

    /// The server-wide `OPTIONS *` answer is built from this walk, so it has to see every
    /// leaf — including ones behind params and a catch-all — exactly once.
    #[test]
    fn for_each_route_visits_every_leaf_once() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 1);
        router.add_route(Method::Post, "/users", 2);
        router.add_route(Method::Delete, "/users/{id}", 3);
        router.add_route(Method::Put, "/files/*rest", 4);
        router.add_route(Method::Get, "/users/me", 5);

        let mut seen = Vec::new();
        router.for_each_route(|method, handler_id| seen.push((method, handler_id)));
        seen.sort_by_key(|(method, handler_id)| (*handler_id, method.bit()));

        assert_eq!(
            seen,
            vec![
                (Method::Get, 1),
                (Method::Post, 2),
                (Method::Delete, 3),
                (Method::Put, 4),
                (Method::Get, 5),
            ]
        );

        // An empty router has nothing to report, which leaves `OPTIONS *` answering `OPTIONS`.
        let empty = RadixRouter::new();
        let mut count = 0;
        empty.for_each_route(|_, _| count += 1);
        assert_eq!(count, 0);
    }

    /// The listing is the API's shape, so it has to survive the normalisation `add_route`
    /// applies, name every capture the route declared, and carry the handler behind each pair.
    #[test]
    fn route_listing_rebuilds_each_declared_template() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 1);
        router.add_route(Method::Post, "/users", 2);
        router.add_route(Method::Get, "/users/{id}", 3);
        router.add_route(Method::Post, "/users/{id}/posts/{post_id}", 4);
        router.add_route(Method::Delete, "/files/{bucket}/*rest", 5);

        let got: Vec<(Method, String, HandlerId)> = router
            .route_listing()
            .into_iter()
            .map(|route| (route.method, route.path, route.handler))
            .collect();
        assert_eq!(
            got,
            vec![
                (Method::Get, "/".to_string(), 1),
                (Method::Delete, "/files/{bucket}/*rest".to_string(), 5),
                (Method::Post, "/users".to_string(), 2),
                (Method::Get, "/users/{id}".to_string(), 3),
                (Method::Post, "/users/{id}/posts/{post_id}".to_string(), 4),
            ]
        );
        assert!(RadixRouter::new().route_listing().is_empty());
    }

    /// One `Param` level serves every route competing for it and keeps the name of whichever
    /// registration created it; ditto for a `CatchAll`. A listing that echoed the node prefixes
    /// would report `/a/{x}/c` and `/files/*first` for routes that declared `{y}` and `*second`.
    #[test]
    fn route_listing_stamps_shared_levels_with_each_routes_own_names() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/a/{x}/b", 1);
        router.add_route(Method::Post, "/a/{y}/c", 2);
        router.add_route(Method::Put, "/a/{z}", 3);
        router.add_route(Method::Delete, "/files/*first", 4);
        router.add_route(Method::Head, "/files/*second", 5);

        let listed = router.route_listing();
        let got: Vec<(&str, &str)> = listed
            .iter()
            .map(|route| (route.method.as_str(), route.path.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("GET", "/a/{x}/b"),
                ("POST", "/a/{y}/c"),
                ("PUT", "/a/{z}"),
                ("DELETE", "/files/*first"),
                ("HEAD", "/files/*second"),
            ]
        );
    }

    /// The listing describes what `find` matches, so it is the canonical form of the declaration
    /// rather than its source text — including the default name a bare `*` is registered with.
    #[test]
    fn route_listing_reports_the_canonical_form_of_each_declaration() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/", 1);
        router.add_route(Method::Post, "health", 2);
        router.add_route(Method::Put, "//deep//nested//", 3);
        router.add_route(Method::Delete, "/x/*", 4);
        router.add_route(Method::Patch, "", 5);

        let listed = router.route_listing();
        let paths: Vec<&str> = listed.iter().map(|route| route.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/", "/deep/nested", "/health", "/users", "/x/*wildcard"]
        );
    }

    /// The derived document is a build artifact that gets diffed, so the listing must not depend
    /// on registration order — the order mounts and groups collect routes in is not fixed. It
    /// must also agree with `for_each_route`, the other walk over the same tree.
    #[test]
    fn route_listing_is_independent_of_registration_order() {
        const REGISTRATIONS: [(Method, &str, HandlerId); 5] = [
            (Method::Get, "/users/{id}", 1),
            (Method::Post, "/users", 2),
            (Method::Delete, "/users/{user}/posts/{post}", 3),
            (Method::Put, "/assets/*filepath", 4),
            (Method::Get, "/x/{a}/y/{b}", 5),
        ];

        let build = |registrations: &[(Method, &str, HandlerId)]| {
            let mut router = RadixRouter::new();
            for (method, path, handler) in registrations {
                router.add_route(*method, path, *handler);
            }
            router
        };
        let reversed: Vec<(Method, &str, HandlerId)> =
            REGISTRATIONS.iter().rev().copied().collect();
        let forward = build(&REGISTRATIONS);
        let listed_forward = forward.route_listing();
        let listed_reverse = build(&reversed).route_listing();
        assert_eq!(listed_forward, listed_reverse);

        let mut walked: Vec<(Method, HandlerId)> = Vec::new();
        forward.for_each_route(|method, handler| walked.push((method, handler)));
        walked.sort_by_key(|(_, handler)| *handler);
        let mut from_listing: Vec<(Method, HandlerId)> = listed_forward
            .iter()
            .map(|route| (route.method, route.handler))
            .collect();
        from_listing.sort_by_key(|(_, handler)| *handler);
        assert_eq!(from_listing, walked);
    }

    /// Regression: a catch-all must capture the *entire* remaining path.
    /// It previously yielded only the first segment, so `/static/css/app.css`
    /// produced `filepath="css"` and silently dropped `app.css`.
    #[test]
    fn test_catchall_captures_full_remaining_path() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);

        match router.find(Method::Get, "/static/css/app.css") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 1);
                assert_eq!(get(&params, "filepath"), Some("css/app.css"));
            }
            other => panic!("expected catch-all match, got {other:?}"),
        }

        // Deeply nested paths, a single segment, and a trailing slash must all round-trip.
        match router.find(Method::Get, "/static/a/b/c/d/e.txt") {
            MatchResult::Found { params, .. } => {
                assert_eq!(get(&params, "filepath"), Some("a/b/c/d/e.txt"))
            }
            other => panic!("expected deep catch-all match, got {other:?}"),
        }
        match router.find(Method::Get, "/static/logo.png") {
            MatchResult::Found { params, .. } => {
                assert_eq!(get(&params, "filepath"), Some("logo.png"))
            }
            other => panic!("expected single-segment catch-all match, got {other:?}"),
        }

        // A catch-all is a child node, so it is only reachable when at least one segment
        // remains to consume. `/static/` therefore 404s, matching httprouter's rule that a
        // wildcard requires one or more characters. Documented here so a future change to
        // this semantic is a deliberate one.
        match router.find(Method::Get, "/static/") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for bare catch-all prefix, got {other:?}"),
        }
    }

    /// A catch-all that is not the last declared segment can never match, and the code used to
    /// register it as if it had: `/files/*rest/trailer` attached its handler to the `*rest` node,
    /// so *every* `/files/…` request ran that handler and the declared tail was never looked at.
    /// Registration is a programming error, so it fails at startup exactly like a duplicate
    /// `method + path` does.
    #[test]
    #[should_panic(expected = "declares catch-all '*rest' before its last segment")]
    fn a_non_terminal_catchall_panics_at_registration() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/files/*rest/trailer", 1);
    }

    /// The rule is about position, not about the wildcard: the same declaration with the
    /// catch-all last registers and matches the whole remainder, trailer segment included.
    #[test]
    fn a_terminal_catchall_registers_and_matches_its_remainder() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/files/*rest", 1);
        router.add_route(Method::Get, "/download/*path", 2);

        match router.find(Method::Get, "/files/a/b/trailer") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 1);
                assert_eq!(get(&params, "rest"), Some("a/b/trailer"));
            }
            other => panic!("expected catch-all match, got {other:?}"),
        }
        match router.find(Method::Get, "/download/x/y") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(get(&params, "path"), Some("x/y"));
            }
            other => panic!("expected catch-all match, got {other:?}"),
        }
    }

    /// One dynamic node can be reached by two sibling routes that name the same segment
    /// differently: `/users/{user}/edit` and `/users/{id}` both descend into the one `Param`
    /// child of `/users`, and `RouteNode::param_names` keys the declared name by *method* alone,
    /// so the node holds one GET name and answers it for both routes. Each handler must see the
    /// key its own path declared.
    #[test]
    fn sibling_routes_on_one_param_node_capture_under_their_own_names() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{user}/edit", 10);
        router.add_route(Method::Get, "/users/{id}", 20);

        match router.find(Method::Get, "/users/42") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 20);
                assert_eq!(params.len(), 1);
                assert_eq!(get(&params, "id"), Some("42"));
            }
            other => panic!("expected the leaf param route to match, got {other:?}"),
        }

        match router.find(Method::Get, "/users/42/edit") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 10);
                assert_eq!(params.len(), 1);
                assert_eq!(get(&params, "user"), Some("42"));
            }
            other => {
                panic!("expected the sibling route with a static tail to match, got {other:?}")
            }
        }
    }

    /// Restamping is positional, so the names must be paired with the captures in declaration
    /// order: a route with two dynamic segments would still pass every single-parameter test if
    /// the two lists were zipped backwards. `/a/{x}/{y}` and `/a/{one}/{two}/z` also share both
    /// `Param` levels, so they exercise the same shared-node naming rule at two depths.
    #[test]
    fn multi_param_siblings_restamp_captures_in_declaration_order() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/a/{one}/{two}/z", 1);
        router.add_route(Method::Get, "/a/{x}/{y}", 2);

        match router.find(Method::Get, "/a/1/2") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(params.len(), 2);
                assert_eq!(get(&params, "x"), Some("1"));
                assert_eq!(get(&params, "y"), Some("2"));
                assert_eq!(get(&params, "one"), None);
                assert_eq!(get(&params, "two"), None);
            }
            other => panic!("expected the two-param route to match, got {other:?}"),
        }

        match router.find(Method::Get, "/a/1/2/z") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 1);
                assert_eq!(params.len(), 2);
                assert_eq!(get(&params, "one"), Some("1"));
                assert_eq!(get(&params, "two"), Some("2"));
                assert_eq!(get(&params, "x"), None);
                assert_eq!(get(&params, "y"), None);
            }
            other => {
                panic!("expected the two-param route with a static tail to match, got {other:?}")
            }
        }
    }

    /// The captured catch-all value must be a borrow of the request path, not a copy.
    #[test]
    fn test_catchall_borrows_from_the_request_path() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/files/*rest", 1);

        let path = String::from("/files/deep/nested/value");
        match router.find(Method::Get, &path) {
            MatchResult::Found { params, .. } => {
                let value = get(&params, "rest").unwrap();
                let path_start = path.as_ptr() as usize;
                let value_start = value.as_ptr() as usize;
                assert!(
                    value_start > path_start
                        && value_start + value.len() <= path_start + path.len(),
                    "catch-all value must point inside the original request path"
                );
                assert_eq!(value, "deep/nested/value");
            }
            other => panic!("expected match, got {other:?}"),
        }
    }

    /// Regression: a static sibling that matches the path but not the method must not
    /// mask a parameter route that matches both. `POST /users/me` used to return
    /// 405 `Allow: GET` because the static `/users/me` node short-circuited the search.
    #[test]
    fn test_method_mismatch_on_static_does_not_block_param_route() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/me", 25);
        router.add_route(Method::Post, "/users/{id}", 30);

        match router.find(Method::Post, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 30);
                assert_eq!(get(&params, "id"), Some("me"));
            }
            other => panic!("expected POST to fall through to the param route, got {other:?}"),
        }

        // The static route still wins when both path and method match.
        match router.find(Method::Get, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 25);
                assert!(params.is_empty());
            }
            other => panic!("expected static precedence, got {other:?}"),
        }
    }

    /// A path that exists only under a rejected method must still report 405, not 404.
    /// The Allow bitmask must be the union across every branch whose path matched: for
    /// `/users/me` both the static GET leaf and the param POST leaf are resources at that
    /// URL, so DELETE must advertise `GET, POST`.
    #[test]
    fn test_405_still_reported_when_no_route_matches_both() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/me", 25);
        router.add_route(Method::Post, "/users/{id}", 30);

        match router.find(Method::Delete, "/users/me") {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                assert_ne!(allowed_bitmask & Method::Get.bitmask(), 0);
                assert_ne!(
                    allowed_bitmask & Method::Post.bitmask(),
                    0,
                    "Allow must include methods from the param branch too, got {}",
                    RadixRouter::bitmask_to_allow_header(allowed_bitmask)
                );
                assert_eq!(
                    RadixRouter::bitmask_to_allow_header(allowed_bitmask),
                    "GET, POST"
                );
            }
            other => panic!("expected 405, got {other:?}"),
        }

        // Unrelated branches must not contribute methods to the Allow list.
        match router.find(Method::Delete, "/users/42/posts") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for unrelated path, got {other:?}"),
        }
    }

    /// Registration order must not change the outcome of method backtracking.
    #[test]
    fn test_method_backtracking_is_order_independent() {
        let mut param_first = RadixRouter::new();
        param_first.add_route(Method::Post, "/users/{id}", 30);
        param_first.add_route(Method::Get, "/users/me", 25);

        let mut static_first = RadixRouter::new();
        static_first.add_route(Method::Get, "/users/me", 25);
        static_first.add_route(Method::Post, "/users/{id}", 30);

        for router in [&param_first, &static_first] {
            match router.find(Method::Post, "/users/me") {
                MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 30),
                other => panic!("expected param route, got {other:?}"),
            }
            match router.find(Method::Get, "/users/me") {
                MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 25),
                other => panic!("expected static route, got {other:?}"),
            }
        }
    }

    /// A catch-all must lose to a more specific static route on the same prefix.
    #[test]
    fn test_static_wins_over_catchall_at_same_depth() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/assets/index.html", 1);
        router.add_route(Method::Get, "/assets/*path", 2);

        match router.find(Method::Get, "/assets/index.html") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 1),
            other => panic!("expected static precedence over catch-all, got {other:?}"),
        }
        match router.find(Method::Get, "/assets/js/app.js") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(get(&params, "path"), Some("js/app.js"));
            }
            other => panic!("expected catch-all, got {other:?}"),
        }
    }

    /// Unregistered paths must 404, not 405.
    #[test]
    fn test_unregistered_path_is_not_found() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 30);

        match router.find(Method::Get, "/orders/1") {
            MatchResult::NotFound => {}
            other => panic!("expected 404, got {other:?}"),
        }
        match router.find(Method::Get, "/users/1/extra/segments") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for over-long path, got {other:?}"),
        }
    }

    /// Duplicate `method + path` registration is a startup programming error and must panic
    /// instead of being silently ignored.
    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/users/{id}", 2);
    }

    /// The same path under a different method is not a duplicate.
    #[test]
    fn test_same_path_different_method_is_allowed() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Post, "/users/{id}", 2);
        router.add_route(Method::Delete, "/users/{id}", 3);

        assert!(matches!(
            router.find(Method::Get, "/users/7"),
            MatchResult::Found { handler_id: 1, .. }
        ));
        assert!(matches!(
            router.find(Method::Post, "/users/7"),
            MatchResult::Found { handler_id: 2, .. }
        ));
        assert!(matches!(
            router.find(Method::Delete, "/users/7"),
            MatchResult::Found { handler_id: 3, .. }
        ));
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_root_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 1);
        router.add_route(Method::Get, "/", 2);
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_catchall_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);
        router.add_route(Method::Get, "/static/*filepath", 2);
    }

    /// Regression: two params on the same level compete for exactly the same request
    /// segments, so a differently-named duplicate (`{user}` after `{id}`) used to create a
    /// second sibling node, bypass `reject_duplicate`, and leave handler 2 unreachable.
    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_param_registration_with_alias_name_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/users/{user}", 2);
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_catchall_registration_with_alias_name_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);
        router.add_route(Method::Get, "/static/*rest", 2);
    }

    /// Differently-named params on the same level share one radix node (they compete for the
    /// same request segment), but each method must extract under the name *it* registered.
    /// Before `param_names`, the first registration silently won and `param("user")` returned
    /// `None` for the POST handler that declared `{user}`.
    #[test]
    fn test_cross_name_param_registration_keeps_each_methods_name() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Post, "/users/{user}", 2);
        router.add_route(Method::Get, "/static/*filepath", 3);
        router.add_route(Method::Post, "/static/*rest", 4);

        match router.find(Method::Post, "/users/7") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(get(&params, "user"), Some("7"));
                assert_eq!(
                    get(&params, "id"),
                    None,
                    "POST declared {{user}}, not {{id}}"
                );
            }
            other => panic!("expected merged param route, got {other:?}"),
        }
        match router.find(Method::Get, "/users/7") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 1);
                assert_eq!(get(&params, "id"), Some("7"));
                assert_eq!(
                    get(&params, "user"),
                    None,
                    "GET declared {{id}}, not {{user}}"
                );
            }
            other => panic!("expected merged param route, got {other:?}"),
        }
        match router.find(Method::Post, "/static/css/app.css") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 4);
                assert_eq!(get(&params, "rest"), Some("css/app.css"));
                assert_eq!(get(&params, "filepath"), None, "POST declared *rest");
            }
            other => panic!("expected merged catch-all route, got {other:?}"),
        }
        match router.find(Method::Get, "/static/css/app.css") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 3);
                assert_eq!(get(&params, "filepath"), Some("css/app.css"));
                assert_eq!(get(&params, "rest"), None, "GET declared *filepath");
            }
            other => panic!("expected merged catch-all route, got {other:?}"),
        }
    }

    /// A third method on the same level binds its own name too, and the node's declared
    /// `NodeType` name stays the fallback for anything that used it.
    #[test]
    fn test_three_way_cross_name_params_are_independent() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/files/{name}", 1);
        router.add_route(Method::Post, "/files/{filename}", 2);
        router.add_route(Method::Delete, "/files/{key}", 3);

        for (method, id, key) in [
            (Method::Get, 1, "name"),
            (Method::Post, 2, "filename"),
            (Method::Delete, 3, "key"),
        ] {
            match router.find(method, "/files/report.pdf") {
                MatchResult::Found { handler_id, params } => {
                    assert_eq!(handler_id, id, "method={method:?}");
                    assert_eq!(get(&params, key), Some("report.pdf"), "method={method:?}");
                }
                other => panic!("method={method:?} expected match, got {other:?}"),
            }
        }
    }

    /// Trailing slashes and repeated separators must not change routing.
    #[test]
    fn test_trailing_slash_and_empty_segments_are_normalized() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);

        for path in ["/users/9", "/users/9/", "/users//9"] {
            match router.find(Method::Get, path) {
                MatchResult::Found { params, .. } => {
                    assert_eq!(get(&params, "id"), Some("9"), "path={path}")
                }
                other => panic!("path={path} expected match, got {other:?}"),
            }
        }
    }

    /// Malformed or hostile input must never panic; it should simply fail to match.
    #[test]
    fn test_malformed_paths_do_not_panic() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/static/*filepath", 2);

        let hostile = [
            "",
            "/",
            "//",
            "///",
            "/users/",
            "/users",
            "/users/{id}",
            "/users/*",
            "/static/",
            "/static/../../etc/passwd",
            "/static/%2e%2e/%2e%2e/etc/passwd",
            "/users/\u{0}",
            "/users/日本語/extra",
            "/static/\u{1F600}/deep/path",
            "/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];

        for path in hostile {
            let _ = router.find(Method::Get, path);
            let _ = router.find(Method::Post, path);
            let _ = router.find(Method::Delete, path);
        }
    }

    /// The catch-all offset arithmetic borrows from the request path, so it must stay in
    /// bounds for paths that begin at a non-zero offset and for maximal segment counts.
    #[test]
    fn test_catchall_offset_arithmetic_stays_in_bounds() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/s/*rest", 1);

        // 64 segments deep, exercising the SmallVec spill to the heap.
        let deep = format!(
            "/s/{}",
            (0..64)
                .map(|i| format!("d{i}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        match router.find(Method::Get, &deep) {
            MatchResult::Found { params, .. } => {
                let value = get(&params, "rest").unwrap();
                assert_eq!(value, &deep[3..]);
                assert_eq!(value.matches('/').count(), 63);
            }
            other => panic!("expected deep match, got {other:?}"),
        }
    }

    /// Routing must scale to a large table without changing correctness.
    #[test]
    fn test_large_route_table() {
        let mut router = RadixRouter::new();
        for i in 0..1000 {
            router.add_route(Method::Get, &format!("/api/v1/resource{i}"), i as HandlerId);
        }
        router.add_route(Method::Get, "/api/v1/{name}/detail", 9999);

        match router.find(Method::Get, "/api/v1/resource500") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 500),
            other => panic!("expected resource500, got {other:?}"),
        }
        match router.find(Method::Get, "/api/v1/unknown/detail") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 9999);
                assert_eq!(get(&params, "name"), Some("unknown"));
            }
            other => panic!("expected param route, got {other:?}"),
        }
    }
}
