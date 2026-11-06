# Track 98 threat note — CSRF, bearer authentication, and forwarded client identity

Written before the code, as roadmap §19.7 row 98 requires. It states, per feature, what is being
protected, what the attacker can do, which attacks are mitigated and by what mechanism, and what is
deliberately out of scope. The register entry for track 98 refers back to this file rather than
restating it.

## Common position

The attacker is a remote party with no credentials on this server, able to send any request it
likes: any method, any headers, any body. For the CSRF row it can also make a *victim's browser*
send a request the attacker chooses, with the victim's cookies attached, from an origin the
attacker controls, and can read neither the response nor any cookie it sets on this origin. What it
cannot do is read process memory, obtain the HMAC secret, or modify the server binary. Requests that
arrive through a reverse proxy additionally arrive from the proxy's address, and every header the
client wrote is forwarded unchanged unless the proxy rewrites it.

Two properties hold for all three features: a refusal is a plain status code with no explanation of
which check failed, and a feature that is not configured costs nothing on the request path.

## F-12 — CSRF protection

**Asset.** A state-changing endpoint that authenticates the caller by an ambient cookie. Without
protection the browser attaches that cookie to a request the user never intended.

**Mechanism: signed double-submit, header against cookie.** On a *safe* response the server issues a
token `nonce.mac` in a cookie. An unsafe request must carry the same token in a header. The header
check is what a cross-origin attacker cannot satisfy: reading the cookie is blocked by the
same-origin policy, and setting a custom header on a cross-origin request is blocked without a CORS
preflight, which this server only answers for configured origins.

**Why the token is signed and not just random.** Plain double-submit is broken by cookie tossing: an
attacker who controls any sibling subdomain can set a cookie for the parent domain, choose its
value, and satisfy "cookie equals header" while the victim's browser sends it. With an HMAC over
the nonce, a planted token fails verification because the attacker cannot produce the MAC. The MAC
is compared with `ring`'s constant-time verify, and the header/cookie comparison is
constant-time as well.

**Why not the synchronizer-token (server-side session) pattern.** It requires per-session state the
framework does not have; the JWT validator is stateless by construction, and a middleware that
introduced a session store would be the first stateful component in the request path. The
double-submit form keeps the same stateless property at the cost of relying on the cookie
attribute rules below.

**Cookie rules.** `SameSite=Lax` by default: it does not stop this attack on its own (it is the
token that does), but it removes the easiest cross-site form submits. `Secure` is on by default and
must be switched off explicitly for plaintext local development; the default is the one that is
safe to ship. `Path=/` and no `Domain`, so the cookie is for this origin. An operator who names the
cookie with the `__Host-` prefix gets the browser's own enforcement of Secure + Path + no Domain;
the middleware emits exactly the attributes that prefix requires, so the prefix works when chosen.

**Safe methods are exempt.** RFC 9110 §9.2.1 makes GET/HEAD/OPTIONS/TRACE safe; a server that
enforced tokens on them would break every link and every probe without protecting anything.

**Out of scope.** XSS (an attacker who can run script on this origin reads the cookie, and no CSRF
scheme survives that), endpoints authenticated by something other than ambient cookies (an API on
`Authorization: Bearer` is not a CSRF target), and multiple server instances. The last one is a
deployment fact, not a code gap: a per-process random secret means node A's token fails on node B,
so a multi-node deployment passes one shared secret. The API makes that a constructor argument
rather than a hidden default.

**Failure mode.** 403, empty body. No token is echoed, no reason is given, and no cookie is
refreshed on the refusal.

## F-13 — Bearer authentication

**Asset.** A route whose handler must only run for a caller holding a valid token.

**Mechanism.** The existing `JwtValidator` verifies an HS256 signature and the standard claims.
The new layer takes the token from `Authorization: Bearer <token>`, verifies it with an
`Arc<JwtValidator>` and a configured `JwtValidationOptions`, and inserts a `Principal` (subject plus
the verified claims) into the request's extensions. Only then does the handler run.

**Why not cookie sessions.** The framework has no session store; a bearer token in a header is not
sent by a browser on a cross-site request, so it is also the identity form that does not interact
with F-12.

**Uniform refusal.** Missing header, a non-Bearer scheme, a malformed token, a bad signature, an
expired token and an `alg` mismatch all produce the same `401` with
`WWW-Authenticate: Bearer error="invalid_token"`. Distinguishing "expired" from "forged" tells an
attacker which of their guesses was closer, and telling a legitimate client which is wrong is the
issuer's job via the token itself.

**Attack surface considered.** `alg: none` and algorithm substitution are refused by the validator's
pinned `expected_alg` before any HMAC is computed. Signature comparison is constant-time inside
`ring`. Token size is bounded by the connection's header budget, so a huge token is a `431` from the
parser, not a verification cost. Replay is not mitigated: a stolen token is valid until `exp`, and
revocation is out of scope — it needs state this server does not keep.

**Out of scope.** Asymmetric algorithms and JWKS rotation, refresh tokens, revocation lists,
scope/role checks beyond what a handler can read from the verified claims. A handler that needs
authorisation (not just authentication) reads `Principal::claims`.

## F-14 — Client identity behind a proxy

**Asset.** The rate limiter's per-client budget, and any handler that reasons about the caller's
address. Behind a proxy every request arrives from the proxy, so without this every client shares
one bucket.

**The rule: trust a hop only if the hop that delivered it is trusted.** The transport peer's address
is always true. If it is not in the operator's allow-list of proxy networks, every forwarding header
is ignored — that single rule defeats the trivial spoof (`X-Forwarded-For: 1.2.3.4` sent by the
client itself). If it is trusted, the chain is read right to left: each entry is accepted only while
it names another trusted proxy address; the first address that is not a trusted proxy is the client,
and the walk stops. A client that sends a forged left-hand entry through one honest proxy therefore
ends up with the honest proxy's record of the client, not its own.

**Malformed entries stop the walk.** A port suffix (`1.2.3.4:5678`), an IPv6 bracket form, an
obfuscated identifier (`for=_hidden`), an unknown token or an empty element is not a failure to
retry — it is the point past which nothing is trusted, and the identity is the last accepted hop.
The alternative, skipping the bad element and trusting an entry to its left, would hand the
attacker a way to name their own bucket with one malformed field in the middle of the chain.

**Where the value goes.** The limiter keys on the resolved identity. The transport peer stays
available as `peer_addr` — it is a fact about the socket, and overwriting it with a header's claim
would make the two indistinguishable in logs and handlers. The resolved identity is exposed
separately, as an extension and as `RequestContext::client_addr()`, and only when trusted proxies
are configured, because without them it is the peer by definition.

**Bounded.** At most 32 hops are examined, which is far more than any real deployment and keeps a
pathological header from turning one request into an unbounded parse. Header bytes are already
bounded by the connection's `max_header_bytes`.

**Out of scope.** The PROXY protocol (it is a transport-level preamble this server does not speak),
`Forwarded: for=` with obfuscated or unknown identifiers, and proxies that rewrite rather than
append. The last one is documented in the API: a proxy configured to *replace* the header is
compatible; one configured to *append* is what the right-to-left rule is written for.

## Test obligations this note creates

- F-12: refusal without the header, refusal with a mismatched pair, acceptance with a matching
  pair, safe methods exempt, a planted/forged cookie rejected, constant-time comparison exercised,
  attributes on the issued cookie, exempt-prefix behaviour.
- F-13: 401 shapes (missing, wrong scheme, malformed, bad signature, expired, wrong `alg`), the
  principal reaching the handler, and the layer leaving public routes alone when registered
  per-route.
- F-14: untrusted peer ignores both header forms, trusted peer resolves the rightmost untrusted
  hop, a forged left entry is ignored, malformed entry stops the walk, port and bracket forms
  parsed, the hop cap, and the limiter keying on the resolved identity rather than the proxy.
