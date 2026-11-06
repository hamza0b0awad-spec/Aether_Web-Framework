//! Property tests for the segment trie router.
//!
//! They state the five guarantees the router is documented to give: a registered static route
//! always answers for its own path, a route's declared `{names}` are the keys its captures come
//! back under and in declaration order, a static sibling beats a parameter sibling regardless of
//! registration order, a method that is not served on an existing path answers 405 with the union
//! of every method that path serves — while a path that was never registered answers 404, and
//! `route_listing` reports every registration under its own names, sorted, and each name it
//! reports is a name the matcher really extracts. Each property names, in the track-102 register
//! entry, the mutant it is known to kill.

use aether_http::Method;
use aether_router::{MatchResult, RadixRouter};
use proptest::prelude::*;

fn token() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,9}"
}

// ---------------------------------------------------------------------------------------------
// 1. A registered static route matches its own path.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside; a shrunk counterexample is already printed in
        // the failure message, and a mutation run must not leave files in the tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_registered_static_route_matches_its_own_path(segments in prop::collection::vec(token(), 1..=5)) {
        let path = format!("/{}", segments.join("/"));
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, &path, 7);

        match router.find(Method::Get, &path) {
            MatchResult::Found { handler_id, params } => {
                prop_assert_eq!(handler_id, 7);
                prop_assert!(params.is_empty(), "a static route captures nothing");
            }
            other => prop_assert!(false, "a registered path must match itself, got {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Declared {names} come back as the capture keys, in declaration order.
// ---------------------------------------------------------------------------------------------

/// A path with `names.len()` parameters separated by static segments, plus the concrete path
/// that fills them with `values` — same shape, so a correct router must report one capture per
/// declared name.
fn parametrised_route() -> impl Strategy<Value = (String, String, Vec<String>, Vec<String>)> {
    (
        prop::collection::vec(token(), 1..=4),
        prop::collection::vec(token(), 1..=4),
    )
        .prop_map(|(names, values)| {
            // Distinct names inside one route are required for the assertion to mean anything.
            let names: Vec<String> = names
                .into_iter()
                .enumerate()
                .map(|(i, n)| format!("{n}{i}"))
                .collect();
            let mut pattern = String::from("/api");
            let mut concrete = String::from("/api");
            for (name, value) in names.iter().zip(&values) {
                pattern.push_str(&format!("/{{{name}}}"));
                concrete.push('/');
                concrete.push_str(value);
            }
            (pattern, concrete, names, values)
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside; a shrunk counterexample is already printed in
        // the failure message, and a mutation run must not leave files in the tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn declared_param_names_extract_their_segments_in_order(
        (pattern, concrete, names, values) in parametrised_route()
    ) {
        let mut router = RadixRouter::new();
        // A sibling route descends through the same parameter nodes under different names.
        // Registering it FIRST makes those nodes carry the sibling's names, so a capture key
        // is only correct if the matched leaf restamps it with its own route's names.
        let mut sibling = String::from("/api");
        for name in &names {
            sibling.push_str(&format!("/{{zz{name}}}"));
        }
        sibling.push_str("/tail");
        router.add_route(Method::Get, &sibling, 11);
        router.add_route(Method::Get, &pattern, 9);

        match router.find(Method::Get, &concrete) {
            MatchResult::Found { handler_id, params } => {
                prop_assert_eq!(handler_id, 9);
                let got: Vec<(&str, &str)> = params.iter().map(|p| (p.key, p.value)).collect();
                let expected: Vec<(&str, &str)> = names
                    .iter()
                    .map(String::as_str)
                    .zip(values.iter().map(String::as_str))
                    .collect();
                prop_assert_eq!(got, expected);
            }
            other => prop_assert!(false, "{} must match {}: got {:?}", concrete, pattern, other),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. A static sibling beats a parameter sibling, whatever the registration order.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside; a shrunk counterexample is already printed in
        // the failure message, and a mutation run must not leave files in the tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_static_segment_beats_a_parameter_segment(
        prefix in token(),
        static_segment in token(),
        other in token(),
    ) {
        // The parameter route is registered FIRST, so precedence cannot come from insertion
        // order; the matcher itself must prefer the literal.
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, &format!("/{prefix}/{{value}}"), 1);
        router.add_route(Method::Get, &format!("/{prefix}/{static_segment}"), 2);

        let static_path = format!("/{prefix}/{static_segment}");
        match router.find(Method::Get, &static_path) {
            MatchResult::Found { handler_id, params } => {
                prop_assert_eq!(handler_id, 2, "the literal route must win over the parameter");
                prop_assert!(params.is_empty());
            }
            other => prop_assert!(false, "{} must match: got {:?}", static_path, other),
        }

        // Any segment other than the literal still reaches the parameter route with that
        // segment captured. Keep the two distinct so the two assertions cannot contradict.
        let other = if other == static_segment {
            format!("{other}~")
        } else {
            other
        };
        let param_path = format!("/{prefix}/{other}");
        match router.find(Method::Get, &param_path) {
            MatchResult::Found { handler_id, params } => {
                prop_assert_eq!(handler_id, 1);
                prop_assert_eq!(params.len(), 1);
                prop_assert_eq!(params[0].key, "value");
                prop_assert_eq!(params[0].value, other.as_str());
            }
            other => prop_assert!(false, "{} must match the parameter route: got {:?}", param_path, other),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. 405 iff the path exists, with the exact union of the methods the path serves; 404 otherwise.
// ---------------------------------------------------------------------------------------------

/// Methods the test drives without touching the two registered ones, so every one of them must
/// answer 405 on an existing path.
fn unserved_method() -> impl Strategy<Value = Method> {
    prop_oneof![
        Just(Method::Put),
        Just(Method::Delete),
        Just(Method::Patch),
        Just(Method::Head),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside; a shrunk counterexample is already printed in
        // the failure message, and a mutation run must not leave files in the tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn an_unserved_method_on_an_existing_path_is_405_with_the_union(
        base in token(),
        literal in token(),
        value in token(),
        method in unserved_method(),
    ) {
        // Two routes share one path shape: a static GET and a parameter POST. A 405 on the
        // static path must advertise BOTH methods, even though only the static branch is the
        // one whose path matched last — the union is what RFC 9110 §15.5.6 requires.
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, &format!("/{base}/{literal}"), 1);
        router.add_route(Method::Post, &format!("/{base}/{{p}}"), 2);

        let static_path = format!("/{base}/{literal}");
        match router.find(method, &static_path) {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                prop_assert_eq!(allowed_bitmask, Method::Get.bitmask() | Method::Post.bitmask());
            }
            other => prop_assert!(false, "{} must be 405 for {}: got {:?}", static_path, method, other),
        }

        // Backtracking: POST on the literal path must fall through the GET-only static branch
        // to the POST parameter route rather than answer 405.
        match router.find(Method::Post, &static_path) {
            MatchResult::Found { handler_id, params } => {
                prop_assert_eq!(handler_id, 2);
                prop_assert_eq!(params.len(), 1);
                prop_assert_eq!(params[0].value, literal.as_str());
            }
            other => prop_assert!(false, "POST {} must reach the parameter route: got {:?}", static_path, other),
        }

        // A path under a base that has no route at all answers 404, not 405.
        let absent = format!("/missing-{base}/{value}");
        match router.find(Method::Get, &absent) {
            MatchResult::NotFound => {}
            other => prop_assert!(false, "{} must be 404: got {:?}", absent, other),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 5. `route_listing` returns every registration, under its own names, sorted, and each name it
//    reports is the name the matcher extracts.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside; a shrunk counterexample is already printed in
        // the failure message, and a mutation run must not leave files in the tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn the_route_listing_returns_every_registration_with_its_own_names(
        bases in prop::collection::vec(token(), 1..=4),
    ) {
        let mut router = RadixRouter::new();
        let mut expected: Vec<(Method, String)> = Vec::new();
        // (method, concrete probe, capture key the listing declared, captured value)
        let mut extractions: Vec<(Method, String, String, String)> = Vec::new();

        for (index, base) in bases.iter().enumerate() {
            // The index keeps generated bases from colliding on one path.
            let base = format!("{base}{index}");
            let param = format!("/{base}/{{p{index}}}");
            let param_alias = format!("/{base}/{{q{index}}}");
            let catch_all = format!("/{base}/*c{index}");
            let catch_alias = format!("/{base}/*d{index}");

            // Whichever route registers first owns the shared level's node name, so alternating
            // the order makes an implementation that echoes node prefixes right for even indices
            // and wrong for odd ones. Only reading each leaf's own declared names is always
            // right — and the matcher must extract under those same names.
            let (first, second) = if index % 2 == 0 {
                ((&param, Method::Get), (&param_alias, Method::Post))
            } else {
                (
                    (&param_alias, Method::Post),
                    (&param, Method::Get),
                )
            };
            let (first_catch, second_catch) = if index % 2 == 0 {
                ((&catch_all, Method::Put), (&catch_alias, Method::Delete))
            } else {
                (
                    (&catch_alias, Method::Delete),
                    (&catch_all, Method::Put),
                )
            };
            for (path, method) in [first, second, first_catch, second_catch] {
                router.add_route(method, path, index as u32);
            }

            expected.extend([
                (Method::Get, param.clone()),
                (Method::Post, param_alias.clone()),
                (Method::Put, catch_all.clone()),
                (Method::Delete, catch_alias.clone()),
            ]);
            extractions.push((
                Method::Get,
                format!("/{base}/v{index}"),
                format!("p{index}"),
                format!("v{index}"),
            ));
            // Two segments past the base: only the catch-all can consume the remainder.
            extractions.push((
                Method::Put,
                format!("/{base}/x{index}/y{index}"),
                format!("c{index}"),
                format!("x{index}/y{index}"),
            ));
        }

        let listed: Vec<(Method, String)> = router
            .route_listing()
            .into_iter()
            .map(|route| (route.method, route.path))
            .collect();
        prop_assert_eq!(listed.len(), expected.len(), "one entry per registration");

        // Same set, whatever the order it is compared in.
        let sort = |mut pairs: Vec<(Method, String)>| {
            pairs.sort_by(|a, b| (&a.1, a.0.as_str()).cmp(&(&b.1, b.0.as_str())));
            pairs
        };
        prop_assert_eq!(sort(listed.clone()), sort(expected));

        // Sorted by (path, method) is part of the documented contract, not a test convenience.
        for window in listed.windows(2) {
            prop_assert!(
                (window[0].1.as_str(), window[0].0.as_str())
                    <= (window[1].1.as_str(), window[1].0.as_str()),
                "the listing must come out sorted: {:?}",
                listed
            );
        }

        for (method, probe, key, value) in extractions {
            match router.find(method, &probe) {
                MatchResult::Found { params, .. } => {
                    prop_assert_eq!(params.len(), 1, "{} must capture one value", probe);
                    prop_assert_eq!(
                        params[0].key,
                        key.as_str(),
                        "{}: the name the listing declared must be the capture key",
                        probe
                    );
                    prop_assert_eq!(
                        params[0].value,
                        value.as_str(),
                        "{}: wrong capture value",
                        probe
                    );
                }
                other => prop_assert!(
                    false,
                    "{} must match a listed {:?} route: got {:?}",
                    probe,
                    method,
                    other
                ),
            }
        }
    }
}
