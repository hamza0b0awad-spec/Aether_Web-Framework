//! TLS termination: certificate loading, ALPN advertisement, and live renewal.
//!
//! One port can carry HTTP/1.1, HTTP/2 and TLS at once only if the TLS layer is decided
//! *per connection* rather than per listener, so the acceptor is built once at `serve` and
//! cloned into every connection task. Two choices in here are load-bearing:
//!
//! - **ALPN advertises only what this build can actually serve.** The HTTP/2 frontend is not
//!   wired yet, so [`TlsAcceptorBuilder::advertise_h2`] is off by default and a TLS client
//!   negotiates `http/1.1` and gets it. Offering `h2` before that would let a browser select a
//!   framing the server cannot speak, and what it reports is a corrupt stream, not a missing
//!   capability — the worst kind of failure to debug.
//! - **Reloading certificates is an atomic `Arc` swap** ([`TlsAcceptor::replace_certificates`]).
//!   A cert renewed by a cron job must not require a restart, and a failed reload must leave
//!   the live certificate running rather than half-applied — so a new set is fully parsed and
//!   key-validated first, and only a complete, named-consistent set replaces the old one.
//!
//! Certificate *validation* (chain of trust, names, expiry) is the client's job, as it is for
//! every public web server; this module only ever loads the server's own material. Revocation of
//! that material is enforced here from operator-supplied, offline sources:
//! [`TlsCertSet::with_crl`] refuses a certificate whose serial a signed, current CRL lists, and
//! [`TlsCertSet::with_ocsp`] refuses a staple that did not answer successfully, does not cover
//! this chain, is not current, or is not signed by the chain's issuer (or by a responder
//! certificate that issuer signed and authorised). Nothing here fetches from the network; refresh
//! means handing a new list or staple to the same load/reload calls.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};

use rustls::ServerConfig;
use rustls::crypto::{CryptoProvider, ring as rustls_ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::io::{AsyncRead, AsyncWrite};
use x509_parser::asn1_rs::{Any, Class, Oid, Tag};
use x509_parser::oid_registry::{OID_HASH_SHA1, OID_NIST_HASH_SHA256};
use x509_parser::prelude::{FromDer, X509Certificate};
use x509_parser::time::ASN1Time;
use x509_parser::verify::verify_signature;
use x509_parser::x509::AlgorithmIdentifier;

/// Why TLS material was refused, before it could break a live handshake.
#[derive(Debug)]
pub enum TlsError {
    /// No PEM certificate block was found in the input.
    NoCertificates,
    /// No usable PEM private key block was found in the input.
    NoPrivateKey,
    /// A key was present but in a form that cannot be used: encrypted (`ENCRYPTED PRIVATE KEY`)
    /// or not a PKCS#1/PKCS#8/SEC1 PEM at all.
    InvalidPrivateKey,
    /// The certificate chain and the private key do not correspond.
    KeyMismatch,
    /// No identity was marked as the catch-all (the one served when SNI does not match).
    NoDefaultIdentity,
    /// An SNI name was supplied twice, or was empty/not a DNS name.
    InvalidServerName(String),
    /// rustls rejected the material or the resulting configuration.
    Config(rustls::Error),
    /// An I/O error reading a file.
    Io(String),
    /// The certificate this set would serve is listed as revoked by the supplied CRL or OCSP
    /// response.
    CertificateRevoked,
    /// Revocation material could not be used: it did not parse, was not signed by this chain's
    /// issuer, did not cover this certificate, or was outside its own validity window.
    RevocationUnusable(String),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::NoCertificates => f.write_str("no PEM certificate found"),
            TlsError::NoPrivateKey => f.write_str("no PEM private key found"),
            TlsError::InvalidPrivateKey => f.write_str(
                "private key is not usable (encrypted, or not a PKCS#8/PKCS#1/SEC1 PEM)",
            ),
            TlsError::KeyMismatch => f.write_str("certificate and private key do not match"),
            TlsError::NoDefaultIdentity => {
                f.write_str("no catch-all identity: supply one certificate with no server name")
            }
            TlsError::InvalidServerName(name) => {
                write!(f, "server name is not usable for SNI: {name:?}")
            }
            TlsError::Config(e) => write!(f, "TLS configuration rejected: {e}"),
            TlsError::Io(e) => write!(f, "TLS file error: {e}"),
            TlsError::CertificateRevoked => {
                f.write_str("the certificate is revoked by the configured CRL or OCSP response")
            }
            TlsError::RevocationUnusable(why) => write!(f, "revocation material unusable: {why}"),
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::Config(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TlsError {
    fn from(e: io::Error) -> Self {
        TlsError::Io(e.to_string())
    }
}

/// The crypto provider every TLS decision in this module is made against.
///
/// Pinned to `ring` rather than taken from the process default: the default provider depends on
/// which rustls features the whole build graph happens to enable, and a framework that reports
/// "TLS 1.2 and 1.3 with ring" must mean that regardless of what else the application links.
fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: std::sync::OnceLock<Arc<CryptoProvider>> = std::sync::OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls_ring::default_provider()))
        .clone()
}

/// A validated certificate chain plus the key that matches it.
///
/// Validation happens at construction, through [`CertifiedKey::from_der`], which loads the key
/// with the provider's key provider and compares the key's `SubjectPublicKeyInfo` against the
/// end-entity certificate. A mismatch is therefore a load error, not a handshake failure that
/// some real client has to discover.
#[derive(Clone, Debug)]
pub struct TlsCertSet {
    certified: Arc<CertifiedKey>,
}

impl TlsCertSet {
    /// Parse a PEM certificate bundle (leaf first) and a PEM private key from memory.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsError> {
        let mut reader = io::BufReader::new(cert_pem);
        let chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
            .map(|r| r.map_err(|_| TlsError::NoCertificates))
            .collect::<Result<_, _>>()?;
        if chain.is_empty() {
            return Err(TlsError::NoCertificates);
        }

        // rustls-pemfile silently skips blocks it does not model, so an encrypted key would look
        // like "no key found". Say which thing actually went wrong.
        if key_pem
            .windows(b"ENCRYPTED PRIVATE KEY".len())
            .any(|w| w == b"ENCRYPTED PRIVATE KEY")
        {
            return Err(TlsError::InvalidPrivateKey);
        }
        let mut kreader = io::BufReader::new(key_pem);
        let key =
            match rustls_pemfile::read_one(&mut kreader).map_err(|_| TlsError::NoPrivateKey)? {
                Some(rustls_pemfile::Item::Pkcs8Key(k)) => PrivateKeyDer::Pkcs8(k),
                Some(rustls_pemfile::Item::Pkcs1Key(k)) => PrivateKeyDer::Pkcs1(k),
                Some(rustls_pemfile::Item::Sec1Key(k)) => PrivateKeyDer::Sec1(k),
                Some(_) => return Err(TlsError::InvalidPrivateKey),
                None => return Err(TlsError::NoPrivateKey),
            };

        let p = provider();
        let certified = CertifiedKey::from_der(chain, key, &p).map_err(classify_key_error)?;
        Ok(TlsCertSet {
            certified: Arc::new(certified),
        })
    }

    /// Read a certificate bundle and key from disk.
    pub fn from_files(cert_path: &str, key_path: &str) -> Result<Self, TlsError> {
        let cert = std::fs::read(cert_path).map_err(TlsError::from)?;
        let key = std::fs::read(key_path).map_err(TlsError::from)?;
        Self::from_pem(&cert, &key)
    }

    /// Validate an OCSP response for this set's certificate and staple it alongside the chain.
    ///
    /// The response must be a *basic* OCSP response whose state is `successful` and which carries
    /// a `SingleResponse` naming this leaf — same serial, and both issuer hashes recomputed from
    /// this chain — with a `good` status and a current validity window (`thisUpdate` not in the
    /// future, `nextUpdate` still ahead). Its signature is checked against the chain's issuer or,
    /// when the response names and includes a responder certificate, against that certificate
    /// after proving the issuer signed it and granted it `id-kp-OCSPSigning`.
    ///
    /// A staple that fails any of these is refused at load time rather than sent: a client that
    /// trusts a stale, foreign or unsigned staple is worse off than one that receives none, and
    /// RFC 6066 §8 leaves the client free to hard-fail on a stapled response it rejects.
    ///
    /// Refresh follows [`Self::with_crl`]: reload the certificate set with a fresh staple through
    /// [`TlsAcceptor::replace_certificates`]. Nothing here contacts a responder.
    pub fn with_ocsp(mut self, ocsp: Vec<u8>) -> Result<Self, TlsError> {
        enforce_ocsp(&ocsp, &self.certified.cert)?;
        let mut certified = (*self.certified).clone();
        certified.ocsp = Some(ocsp);
        self.certified = Arc::new(certified);
        Ok(self)
    }

    /// Enforce a certificate revocation list against this set before it can be served.
    ///
    /// The list is parsed (PEM or DER), its signature is verified with the issuing certificate's
    /// key — the second certificate in the chain, or the leaf itself when the set is self-signed —
    /// and its own validity window is checked against the clock. A leaf whose serial appears in the
    /// list is refused *here*, at load or reload time, so a revoked certificate cannot be
    /// installed and then kept serving by an acceptor that never asks again. A list that is stale,
    /// not yet valid, or signed outside the chain is refused rather than ignored: an unenforced
    /// list is precisely the failure this call exists to prevent.
    ///
    /// Refresh is the caller's loop: reload the certificates with a fresh list through the same
    /// [`TlsAcceptor::replace_certificates`] path. Nothing here contacts a CA, so an offline build
    /// and an offline deployment behave identically.
    pub fn with_crl(self, crl: &[u8]) -> Result<Self, TlsError> {
        let leaf_der = self
            .certified
            .cert
            .first()
            .ok_or(TlsError::NoCertificates)?;
        let leaf = parse_certificate(leaf_der)?;
        let issuer_der = self.certified.cert.get(1).unwrap_or(leaf_der);
        let issuer = parse_certificate(issuer_der)?;

        let der = decode_pem_or_der(crl)?;
        let (_, list) = x509_parser::parse_x509_crl(&der)
            .map_err(|_| TlsError::RevocationUnusable("the CRL did not parse".into()))?;
        list.verify_signature(issuer.public_key()).map_err(|e| {
            TlsError::RevocationUnusable(format!(
                "the CRL signature does not verify against this chain's issuer ({e})"
            ))
        })?;

        let now = now_unix();
        if list.last_update().timestamp() > now {
            return Err(TlsError::RevocationUnusable(
                "the CRL's thisUpdate is in the future".into(),
            ));
        }
        let next = list
            .next_update()
            .ok_or_else(|| TlsError::RevocationUnusable("the CRL declares no nextUpdate".into()))?;
        if next.timestamp() <= now {
            return Err(TlsError::RevocationUnusable(
                "the CRL is stale: its nextUpdate has passed".into(),
            ));
        }

        if list
            .iter_revoked_certificates()
            .any(|entry| entry.serial() == &leaf.tbs_certificate.serial)
        {
            return Err(TlsError::CertificateRevoked);
        }
        Ok(self)
    }
}

/// Parse one DER certificate out of the chain rustls will serve.
fn parse_certificate(der: &[u8]) -> Result<X509Certificate<'_>, TlsError> {
    X509Certificate::from_der(der)
        .map(|(_, cert)| cert)
        .map_err(|_| {
            TlsError::RevocationUnusable("a certificate in the chain did not parse".into())
        })
}

/// Revocation material arrives as PEM or DER from every CA and every tool; accept both.
fn decode_pem_or_der(input: &[u8]) -> Result<Vec<u8>, TlsError> {
    if input.windows(11).any(|w| w == b"-----BEGIN ") {
        let (_, pem) = x509_parser::pem::parse_x509_pem(input)
            .map_err(|_| TlsError::RevocationUnusable("the PEM block did not parse".into()))?;
        return Ok(pem.contents);
    }
    Ok(input.to_vec())
}

/// Seconds since the Unix epoch, the unit `x509_parser::time::ASN1Time` compares against.
///
/// A clock before the epoch reads as `0`, which makes every list look not-yet-valid. That is the
/// fail-closed direction: a machine whose clock is wrong should not be the one that decides a
/// revocation list is current.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ocsp_unusable(why: impl Into<String>) -> TlsError {
    TlsError::RevocationUnusable(why.into())
}

/// Parse one OCSP timestamp. RFC 6960 requires GeneralizedTime (`YYYYMMDDHHMMSSZ`) for all four
/// time fields, so a UTCTime is refused rather than coerced: a responder that cannot follow that
/// rule is not one whose freshness claims are worth reading.
fn ocsp_time(any: &Any<'_>, raw: &[u8], field: &str) -> Result<i64, TlsError> {
    if any.class() != Class::Universal || any.tag() != Tag::GeneralizedTime {
        return Err(ocsp_unusable(format!(
            "the OCSP {field} is not a GeneralizedTime"
        )));
    }
    ASN1Time::from_der(raw)
        .map(|(_, time)| time.timestamp())
        .map_err(|_| ocsp_unusable(format!("the OCSP {field} is not a time")))
}

/// `asn1-rs`' `Any::as_*` accessors reinterpret whatever bytes they are handed instead of checking
/// the tag, so a SEQUENCE sitting where an OID belongs would otherwise be read as an OID. Every
/// field read below states its type first, and these accessors are the only way in.
fn check_tag(any: &Any<'_>, tag: Tag, what: &str) -> Result<(), TlsError> {
    if any.class() == Class::Universal && any.tag() == tag {
        return Ok(());
    }
    Err(ocsp_unusable(format!(
        "the staple's {what} is not {}",
        type_name(tag)
    )))
}

fn type_name(tag: Tag) -> &'static str {
    match tag {
        Tag::Integer => "an INTEGER",
        Tag::BitString => "a BIT STRING",
        Tag::OctetString => "an OCTET STRING",
        Tag::Oid => "an OBJECT IDENTIFIER",
        Tag::Enumerated => "an ENUMERATED",
        Tag::Sequence => "a SEQUENCE",
        Tag::GeneralizedTime => "a GeneralizedTime",
        _ => "of the type the field requires",
    }
}

/// A cursor over DER content that also hands back each element's own encoding, so the signature
/// is verified over exactly the bytes that were parsed rather than a re-encoding.
struct DerReader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        DerReader { input, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.input.len()
    }

    fn element(&mut self) -> Result<(Any<'a>, &'a [u8]), TlsError> {
        let rest = &self.input[self.pos..];
        let (tail, any) =
            Any::from_der(rest).map_err(|_| ocsp_unusable("the staple is not valid DER"))?;
        let len = rest.len() - tail.len();
        self.pos += len;
        Ok((any, &rest[..len]))
    }

    fn sequence(&mut self) -> Result<DerReader<'a>, TlsError> {
        let (any, _) = self.element()?;
        if any.class() != Class::Universal || any.tag() != Tag::Sequence {
            return Err(ocsp_unusable("expected a SEQUENCE in the staple"));
        }
        Ok(DerReader::new(any.data))
    }
}

fn context_tag(any: &Any<'_>) -> Option<u32> {
    if any.class() == Class::ContextSpecific {
        Some(any.tag().0)
    } else {
        None
    }
}

fn responder_state(state: u32) -> &'static str {
    match state {
        1 => "malformedRequest",
        2 => "internalError",
        3 => "tryLater",
        5 => "sigRequired",
        6 => "unauthorized",
        _ => "an unrecognised state",
    }
}

fn ocsp_hash(algorithm: &Oid<'_>, data: &[u8]) -> Result<Vec<u8>, TlsError> {
    let digest = if algorithm == &OID_HASH_SHA1 {
        ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data)
    } else if algorithm == &OID_NIST_HASH_SHA256 {
        ring::digest::digest(&ring::digest::SHA256, data)
    } else {
        return Err(ocsp_unusable(
            "the staple names an unsupported CertId hash algorithm",
        ));
    };
    Ok(digest.as_ref().to_vec())
}

/// RFC 6960 does not name an algorithm for `ResponderID.byKey`, and SHA-1 is what responders and
/// clients settled on, so that is the only key hash compared here.
fn sha1_of_key(cert: &X509Certificate<'_>) -> Vec<u8> {
    ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        cert.public_key().subject_public_key.data.as_ref(),
    )
    .as_ref()
    .to_vec()
}

/// The certificate whose key must have signed the response, or `None` when it is the issuer's.
///
/// The signature alone does not say *who* the responder claims to be: `responderID` does. A
/// response that names a key or name other than the issuer is only usable when that responder is
/// carried in the response, bears an issuer signature, and is authorised for OCSP signing.
fn ocsp_signer<'a, 'cert>(
    responder_id: &Any<'_>,
    responder_cert: Option<&'cert X509Certificate<'a>>,
    issuer: &'cert X509Certificate<'a>,
) -> Result<Option<&'cert X509Certificate<'a>>, TlsError> {
    let by_key = match context_tag(responder_id) {
        Some(2) => Some(responder_id.data),
        Some(1) => None,
        _ => return Err(ocsp_unusable("the staple has no responder identity")),
    };
    let names = |cert: &X509Certificate<'_>| match by_key {
        Some(key) => key == sha1_of_key(cert),
        None => responder_id.data == cert.subject().as_raw(),
    };

    if names(issuer) {
        return Ok(None);
    }
    let Some(cert) = responder_cert else {
        return Err(ocsp_unusable(
            "the staple names a responder that is neither the issuer nor an included certificate",
        ));
    };
    if !names(cert) {
        return Err(ocsp_unusable(
            "the staple names a responder that is neither the issuer nor an included certificate",
        ));
    }
    cert.verify_signature(Some(issuer.public_key()))
        .map_err(|e| {
            ocsp_unusable(format!(
                "the responder certificate is not signed by this chain's issuer ({e})"
            ))
        })?;
    let signs_ocsp = cert
        .extended_key_usage()
        .map_err(|_| ocsp_unusable("the responder certificate's key usage is unusable"))?
        .map(|extension| extension.value.ocsp_signing)
        .unwrap_or(false);
    if !signs_ocsp {
        return Err(ocsp_unusable(
            "the responder certificate is not authorised to sign OCSP responses \
             (no id-kp-OCSPSigning)",
        ));
    }
    Ok(Some(cert))
}

/// Enforce one stapled OCSP response against the chain this set would serve.
fn enforce_ocsp(ocsp: &[u8], certs: &[CertificateDer<'_>]) -> Result<(), TlsError> {
    let leaf_der = certs.first().ok_or(TlsError::NoCertificates)?;
    let leaf = parse_certificate(leaf_der)?;
    let issuer_der = certs.get(1).unwrap_or(leaf_der);
    let issuer = parse_certificate(issuer_der)?;

    let mut envelope = DerReader::new(ocsp);
    let mut response = envelope.sequence()?;
    if !envelope.is_empty() {
        return Err(ocsp_unusable("the staple carries bytes after the response"));
    }

    let (state, _) = response.element()?;
    check_tag(&state, Tag::Enumerated, "response state")?;
    let state = state
        .as_enumerated()
        .map_err(|_| ocsp_unusable("the OCSP response state is malformed"))?
        .0;
    if state != 0 {
        return Err(ocsp_unusable(format!(
            "the responder answered {} rather than successful",
            responder_state(state)
        )));
    }

    let (bytes, _) = response.element()?;
    if context_tag(&bytes) != Some(0) {
        return Err(ocsp_unusable(
            "a successful response carried no responseBytes",
        ));
    }
    let mut bytes = DerReader::new(bytes.data).sequence()?;
    let (kind, _) = bytes.element()?;
    check_tag(&kind, Tag::Oid, "response type")?;
    let kind = kind
        .as_oid()
        .map_err(|_| ocsp_unusable("the OCSP response type is malformed"))?;
    if kind.to_id_string() != "1.3.6.1.5.5.7.48.1.1" {
        return Err(ocsp_unusable("the staple is not a basic OCSP response"));
    }
    let (body, _) = bytes.element()?;
    check_tag(&body, Tag::OctetString, "response body")?;
    let body = body
        .as_octetstring()
        .map_err(|_| ocsp_unusable("the OCSP response body is malformed"))?;
    let mut basic = DerReader::new(body.as_ref());

    let mut basic = basic.sequence()?;
    let (tbs, tbs_raw) = basic.element()?;
    let mut response_data = DerReader::new(tbs.data);
    let (_, sig_alg_raw) = basic.element()?;
    let (sig_bits, _) = basic.element()?;
    check_tag(&sig_bits, Tag::BitString, "signature")?;
    let signature = sig_bits
        .as_bitstring()
        .map_err(|_| ocsp_unusable("the OCSP signature is malformed"))?;

    let responder_cert = if basic.is_empty() {
        None
    } else {
        let (wrapped, _) = basic.element()?;
        if context_tag(&wrapped) != Some(0) {
            return Err(ocsp_unusable("unexpected data after the OCSP signature"));
        }
        let mut list = DerReader::new(wrapped.data).sequence()?;
        let (_, cert_raw) = list.element()?;
        let (_, cert) = X509Certificate::from_der(cert_raw)
            .map_err(|_| ocsp_unusable("the responder certificate did not parse"))?;
        Some(cert)
    };

    let (first, _) = response_data.element()?;
    let responder_id = if context_tag(&first) == Some(0) {
        let (version, _) = DerReader::new(first.data).element()?;
        check_tag(&version, Tag::Integer, "response version")?;
        let version = version
            .as_u32()
            .map_err(|_| ocsp_unusable("the OCSP response version is malformed"))?;
        if version != 0 {
            return Err(ocsp_unusable("unsupported OCSP response version"));
        }
        response_data.element()?.0
    } else {
        first
    };

    let (produced_at, produced_at_raw) = response_data.element()?;
    ocsp_time(&produced_at, produced_at_raw, "producedAt")?;

    let mut responses = response_data.sequence()?;
    let now = now_unix();
    let mut covered = false;
    while !responses.is_empty() {
        let mut single = responses.sequence()?;
        let (cert_id, _) = single.element()?;
        let mut cert_id = DerReader::new(cert_id.data);
        let (_, hash_algorithm) = cert_id.element()?;
        let hash_algorithm = AlgorithmIdentifier::from_der(hash_algorithm)
            .map_err(|_| ocsp_unusable("the staple's CertId hash algorithm did not parse"))?
            .1;
        let (name_hash, _) = cert_id.element()?;
        check_tag(&name_hash, Tag::OctetString, "issuerNameHash")?;
        let name_hash = name_hash
            .as_octetstring()
            .map_err(|_| ocsp_unusable("the staple's issuerNameHash is malformed"))?;
        let (key_hash, _) = cert_id.element()?;
        check_tag(&key_hash, Tag::OctetString, "issuerKeyHash")?;
        let key_hash = key_hash
            .as_octetstring()
            .map_err(|_| ocsp_unusable("the staple's issuerKeyHash is malformed"))?;
        let (serial, _) = cert_id.element()?;
        check_tag(&serial, Tag::Integer, "CertId serial")?;
        let serial = serial
            .as_integer()
            .map_err(|_| ocsp_unusable("the staple's CertId serial is malformed"))?;

        if serial.as_ref() != leaf.tbs_certificate.raw_serial() {
            continue;
        }
        if ocsp_hash(&hash_algorithm.algorithm, issuer.subject().as_raw())?.as_slice()
            != name_hash.as_ref()
            || ocsp_hash(
                &hash_algorithm.algorithm,
                issuer.public_key().subject_public_key.data.as_ref(),
            )?
            .as_slice()
                != key_hash.as_ref()
        {
            continue;
        }
        covered = true;

        let (status, _) = single.element()?;
        match context_tag(&status) {
            Some(0) => {}
            Some(1) => return Err(TlsError::CertificateRevoked),
            Some(2) => {
                return Err(ocsp_unusable(
                    "the responder has no status for this certificate (unknown)",
                ));
            }
            _ => return Err(ocsp_unusable("the OCSP certificate status is not a choice")),
        }

        let (this_update, this_update_raw) = single.element()?;
        let this_update = ocsp_time(&this_update, this_update_raw, "thisUpdate")?;
        if this_update > now {
            return Err(ocsp_unusable("the staple's thisUpdate is in the future"));
        }
        if single.is_empty() {
            return Err(ocsp_unusable("the staple declares no nextUpdate"));
        }
        let (next_update, _) = single.element()?;
        if context_tag(&next_update) != Some(0) {
            return Err(ocsp_unusable("the staple declares no nextUpdate"));
        }
        let (next_update, next_update_raw) = DerReader::new(next_update.data).element()?;
        let next_update = ocsp_time(&next_update, next_update_raw, "nextUpdate")?;
        if next_update <= now {
            return Err(ocsp_unusable(
                "the staple is stale: its nextUpdate has passed",
            ));
        }
    }
    if !covered {
        return Err(ocsp_unusable("the staple does not cover this certificate"));
    }

    let signer = ocsp_signer(&responder_id, responder_cert.as_ref(), &issuer)?;
    let key = match signer {
        Some(cert) => cert.public_key(),
        None => issuer.public_key(),
    };
    let algorithm = AlgorithmIdentifier::from_der(sig_alg_raw)
        .map_err(|_| ocsp_unusable("the OCSP signature algorithm did not parse"))?
        .1;
    verify_signature(key, &algorithm, &signature, tbs_raw)
        .map_err(|e| ocsp_unusable(format!("the OCSP signature does not verify ({e})")))?;
    Ok(())
}

fn classify_key_error(e: rustls::Error) -> TlsError {
    match e {
        rustls::Error::InconsistentKeys(rustls::InconsistentKeys::KeyMismatch) => {
            TlsError::KeyMismatch
        }
        // Anything else — an unparseable key, an unsupported algorithm — is rustls' own
        // diagnosis, and paraphrasing it by string-matching its messages would rot on the next
        // rustls upgrade.
        other => TlsError::Config(other),
    }
}

/// One certificate set together with the SNI name it answers.
///
/// `server_name: None` is the catch-all: the set served when the client sent no SNI or a name
/// this server does not hold. Exactly one catch-all is required, because a TLS server that
/// answers "no certificate" to a client without SNI is a server that cannot be reached by
/// half of the clients in the field.
#[derive(Clone, Debug)]
pub struct TlsIdentity {
    server_name: Option<String>,
    certs: TlsCertSet,
}

impl TlsIdentity {
    /// The identity served when SNI does not match any named identity.
    pub fn default_identity(certs: TlsCertSet) -> Self {
        TlsIdentity {
            server_name: None,
            certs,
        }
    }

    /// The identity served for one DNS name. Comparison is case-insensitive, as SNI is.
    pub fn for_name(name: &str, certs: TlsCertSet) -> Result<Self, TlsError> {
        let normalized = normalize_sni(name).ok_or_else(|| {
            TlsError::InvalidServerName(if name.is_empty() {
                "<empty>".to_string()
            } else {
                name.to_string()
            })
        })?;
        Ok(TlsIdentity {
            server_name: Some(normalized),
            certs,
        })
    }
}

fn normalize_sni(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > 253 {
        return None;
    }
    // An IP literal can never be selected by SNI: rustls parses the SNI extension as a DNS name
    // only. An identity keyed on an IP would be installed but unreachable, which is worse than a
    // load-time error.
    if trimmed.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// The certificate sets a live acceptor serves, reachable through one `Arc` so a reload is a
/// pointer swap rather than an in-place edit a concurrent handshake could read half-done.
#[derive(Debug)]
struct SharedCerts {
    by_name: Vec<(String, Arc<CertifiedKey>)>,
    default: Arc<CertifiedKey>,
}

impl SharedCerts {
    fn from_identities(identities: Vec<TlsIdentity>) -> Result<Arc<Self>, TlsError> {
        let mut by_name: Vec<(String, Arc<CertifiedKey>)> = Vec::with_capacity(identities.len());
        let mut default: Option<Arc<CertifiedKey>> = None;
        for identity in identities {
            match identity.server_name {
                None => {
                    if default.is_some() {
                        // Second catch-all: silently ignoring it would mean one identity is
                        // installed but unreachable, which is a confusing way to fail.
                        return Err(TlsError::NoDefaultIdentity);
                    }
                    default = Some(identity.certs.certified);
                }
                Some(name) => {
                    if by_name.iter().any(|(existing, _)| *existing == name) {
                        return Err(TlsError::InvalidServerName(name));
                    }
                    by_name.push((name, identity.certs.certified));
                }
            }
        }
        let default = default.ok_or(TlsError::NoDefaultIdentity)?;
        Ok(Arc::new(SharedCerts { by_name, default }))
    }

    /// The certificate set selected for one SNI name, falling back to the catch-all.
    fn pick(&self, server_name: Option<&str>) -> Arc<CertifiedKey> {
        match server_name {
            Some(wanted) => self
                .by_name
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, key)| key.clone())
                .unwrap_or_else(|| self.default.clone()),
            None => self.default.clone(),
        }
    }
}

/// SNI-aware certificate resolution with an atomic reload path.
#[derive(Debug)]
struct AetherResolves {
    live: Mutex<Arc<SharedCerts>>,
}

impl ResolvesServerCert for AetherResolves {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        // One lock, one Arc clone: the snapshot cannot change underneath this handshake, and a
        // reload that lands while it runs applies to the next connection instead.
        let shared = self.live.lock().ok()?;
        Some(shared.pick(client_hello.server_name()))
    }
}

/// The per-server TLS state, cloned into every connection task.
#[derive(Clone)]
pub struct TlsAcceptor {
    config: Arc<ServerConfig>,
    resolver: Arc<AetherResolves>,
    /// Whether a plaintext connection must be refused. Off means "TLS is available", not
    /// "required", which is what lets HTTP/1.1 and h2c keep working on the same port.
    require_tls: bool,
}

impl TlsAcceptor {
    /// Build an acceptor that serves one certificate set to every client.
    pub fn new(set: TlsCertSet) -> Result<Self, TlsError> {
        Self::builder()
            .insert(TlsIdentity::default_identity(set))
            .build()
    }

    /// Start a multi-certificate (SNI) acceptor.
    pub fn builder() -> TlsAcceptorBuilder {
        TlsAcceptorBuilder::default()
    }

    /// Replace every certificate the live acceptor serves, atomically.
    ///
    /// A failed reload is returned and changes nothing: the previous certificates keep serving,
    /// because "renewal crashed the server" is a worse outcome than "renewal did not happen".
    /// Handshakes already in flight finish with the set they started from.
    pub fn replace_certificates(&self, identities: Vec<TlsIdentity>) -> Result<(), TlsError> {
        let shared = SharedCerts::from_identities(identities)?;
        let mut guard = self
            .resolver
            .live
            .lock()
            .map_err(|_| TlsError::Config(rustls::Error::General("lock poisoned".into())))?;
        *guard = shared;
        Ok(())
    }

    /// Refuse plaintext connections instead of serving them alongside TLS.
    pub fn set_require_tls(&mut self, require: bool) {
        self.require_tls = require;
    }

    pub fn requires_tls(&self) -> bool {
        self.require_tls
    }

    /// Upgrade an already-accepted connection into a TLS one.
    ///
    /// The caller must bound this future with a timeout: a client that completes a TCP handshake
    /// and then dribbles ClientHello bytes holds a task and a buffer otherwise, which is the same
    /// slowloris class the HTTP/1.1 read path already defends against.
    pub async fn accept<S>(&self, stream: S) -> io::Result<tokio_rustls::server::TlsStream<S>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        tokio_rustls::TlsAcceptor::from(Arc::clone(&self.config))
            .accept(stream)
            .await
    }

    /// The ALPN protocols advertised to clients, in preference order.
    pub fn alpn(&self) -> &[Vec<u8>] {
        &self.config.alpn_protocols
    }
}

/// Accumulates identities for one listener.
pub struct TlsAcceptorBuilder {
    identities: Vec<TlsIdentity>,
    require_tls: bool,
    advertise_h2: bool,
}

impl Default for TlsAcceptorBuilder {
    fn default() -> Self {
        Self {
            identities: Vec::new(),
            require_tls: false,
            // On, because the HTTP/2 frontend is wired and an ALPN list that hides it would
            // hold every h2 client back to HTTP/1.1. Off is still available for an operator who
            // wants a port that provably answers one protocol.
            advertise_h2: true,
        }
    }
}

impl TlsAcceptorBuilder {
    /// Add one identity (named or catch-all). Validation happens when the acceptor is built,
    /// because that is where the whole set — names, catch-all, duplicates — is visible at once.
    pub fn insert(mut self, identity: TlsIdentity) -> Self {
        self.identities.push(identity);
        self
    }

    /// Refuse plaintext connections instead of serving them alongside TLS.
    pub fn require_tls(mut self, require: bool) -> Self {
        self.require_tls = require;
        self
    }

    /// Whether to offer `h2` through ALPN. On by default now that the HTTP/2 frontend exists;
    /// off pins this port to HTTP/1.1 for clients that negotiate TLS, which is a thing an
    /// operator may want, but is not what "HTTP/2 is not wired up here" used to mean.
    ///
    /// Plaintext is unaffected either way: HTTP/2 over cleartext (h2c) is a separate negotiation
    /// and is refused on this build.
    pub fn advertise_h2(mut self, advertise: bool) -> Self {
        self.advertise_h2 = advertise;
        self
    }

    pub fn build(self) -> Result<TlsAcceptor, TlsError> {
        let shared = SharedCerts::from_identities(self.identities)?;
        let resolver = Arc::new(AetherResolves {
            live: Mutex::new(shared),
        });
        let mut alpn_protocols = Vec::new();
        if self.advertise_h2 {
            alpn_protocols.push(b"h2".to_vec());
        }
        alpn_protocols.push(b"http/1.1".to_vec());

        let mut cfg = ServerConfig::builder_with_provider(provider())
            // TLS 1.2 alongside 1.3: a 1.3-only server cannot be reached by older clients that
            // are still common on Windows and macOS installs, and cross-platform parity is a
            // requirement, not a preference.
            .with_safe_default_protocol_versions()
            .map_err(TlsError::Config)?
            // Public web servers make client certificates optional; mutual TLS needs a
            // `ClientCertVerifier`, which is deliberately not silently half-implemented here.
            .with_no_client_auth()
            .with_cert_resolver(resolver.clone());
        cfg.alpn_protocols = alpn_protocols;

        Ok(TlsAcceptor {
            config: Arc::new(cfg),
            resolver,
            require_tls: self.require_tls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{Issuer, KeyPair};

    /// A CA plus one leaf for `localhost`, minted per test run so no key material lives in the
    /// repository and no certificate can expire in someone else's CI.
    struct TestPki {
        cert_pem: Vec<u8>,
        key_pem: Vec<u8>,
        ca_pem: Vec<u8>,
        ca_der: Vec<u8>,
        ca_key_pem: String,
        issuer: Issuer<'static, KeyPair>,
    }

    /// The serial every test leaf is minted with, so a revocation list can name it without
    /// reading it back out of the certificate under test.
    const LEAF_SERIAL: &[u8] = &[0x2a];

    /// A throwaway CA plus one leaf for `name`, minted per test run: no key material ends up in
    /// the repository and no certificate can expire in someone else's checkout. Validity comes
    /// from rcgen's defaults (1975..4096), so no clock arithmetic is needed here.
    fn test_pki(name: &str) -> TestPki {
        use rcgen::{
            BasicConstraints, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
            KeyUsagePurpose,
        };

        let mut ca_params =
            rcgen::CertificateParams::new(Vec::<String>::new()).expect("empty SAN is valid");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Aether test CA");
        ca_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
        ];
        let ca_key = KeyPair::generate().expect("key generation");
        let ca_key_pem = ca_key.serialize_pem();
        let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed CA");
        let ca_der = ca_cert.der().as_ref().to_vec();
        let issuer = Issuer::new(ca_params, ca_key);

        let mut leaf_params =
            rcgen::CertificateParams::new(vec![name.to_string()]).expect("name is a DNS name");
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, name);
        leaf_params.use_authority_key_identifier_extension = true;
        leaf_params
            .key_usages
            .push(KeyUsagePurpose::DigitalSignature);
        leaf_params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        let leaf_key = KeyPair::generate().expect("key generation");
        leaf_params.serial_number = Some(rcgen::SerialNumber::from_slice(LEAF_SERIAL));
        let leaf = leaf_params
            .signed_by(&leaf_key, &issuer)
            .expect("leaf signed by CA");

        // Leaf first: that is the order TLS requires a chain to be sent in, and a reversed
        // bundle must not be quietly accepted.
        let mut cert_pem = leaf.pem().into_bytes();
        cert_pem.extend_from_slice(ca_cert.pem().as_bytes());
        TestPki {
            ca_pem: ca_cert.pem().into_bytes(),
            ca_der,
            ca_key_pem,
            cert_pem,
            key_pem: leaf_key.serialize_pem().into_bytes(),
            issuer,
        }
    }

    impl TestPki {
        /// A CRL this PKI's CA signs, revoking exactly the serials given, valid from `this` to
        /// `next`. Everything is relative to the caller's clock so both freshness directions can
        /// be driven without committing a fixture.
        fn crl(
            &self,
            revoked: &[&[u8]],
            this: time::OffsetDateTime,
            next: time::OffsetDateTime,
        ) -> rcgen::CertificateRevocationList {
            signed_crl(&self.issuer, revoked, this, next)
        }

        fn set(&self) -> TlsCertSet {
            TlsCertSet::from_pem(&self.cert_pem, &self.key_pem).expect("valid material")
        }
    }

    fn signed_crl(
        issuer: &Issuer<'static, KeyPair>,
        revoked: &[&[u8]],
        this: time::OffsetDateTime,
        next: time::OffsetDateTime,
    ) -> rcgen::CertificateRevocationList {
        let params = rcgen::CertificateRevocationListParams {
            this_update: this,
            next_update: next,
            crl_number: rcgen::SerialNumber::from_slice(&[1]),
            issuing_distribution_point: None,
            revoked_certs: revoked
                .iter()
                .map(|serial| rcgen::RevokedCertParams {
                    serial_number: rcgen::SerialNumber::from_slice(serial),
                    revocation_time: this,
                    reason_code: Some(rcgen::RevocationReason::KeyCompromise),
                    invalidity_date: None,
                })
                .collect(),
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };
        params.signed_by(issuer).expect("CRL signed by the CA")
    }

    /// Whole-second "now": RFC 6960 wants `YYYYMMDDHHMMSSZ` and yasna would otherwise encode the
    /// sub-second part, which no conformant responder emits.
    fn now() -> time::OffsetDateTime {
        time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .expect("zero nanoseconds are representable")
    }

    #[test]
    fn loads_a_ca_issued_chain() {
        let pki = test_pki("localhost");
        let set = TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("valid material");
        assert_eq!(set.certified.cert.len(), 2);
    }

    #[test]
    fn rejects_a_key_that_does_not_belong_to_the_certificate() {
        let pki = test_pki("localhost");
        let other = test_pki("localhost");
        let err = TlsCertSet::from_pem(&pki.cert_pem, &other.key_pem)
            .expect_err("mismatched pair must not load");
        assert!(matches!(err, TlsError::KeyMismatch), "{err}");
    }

    #[test]
    fn rejects_missing_and_unusable_key_material() {
        let pki = test_pki("localhost");
        assert!(matches!(
            TlsCertSet::from_pem(&pki.cert_pem, b"not a pem").unwrap_err(),
            TlsError::NoPrivateKey
        ));
        let encrypted = String::from_utf8(pki.key_pem.clone())
            .expect("PEM is ascii")
            .replace("PRIVATE KEY", "ENCRYPTED PRIVATE KEY")
            .into_bytes();
        assert!(matches!(
            TlsCertSet::from_pem(&pki.cert_pem, &encrypted).unwrap_err(),
            TlsError::InvalidPrivateKey
        ));
        assert!(matches!(
            TlsCertSet::from_pem(b"", &pki.key_pem).unwrap_err(),
            TlsError::NoCertificates
        ));
    }

    /// The private key's DER, so a test can search a rendering for it. The key is consumed when
    /// the set is built, which is exactly why this reads it back out of the PEM the test minted.
    fn private_key_der(pem: &[u8]) -> Vec<u8> {
        let mut reader = io::BufReader::new(pem);
        match rustls_pemfile::read_one(&mut reader).expect("a readable PEM") {
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => key.secret_pkcs8_der().to_vec(),
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => key.secret_pkcs1_der().to_vec(),
            Some(rustls_pemfile::Item::Sec1Key(key)) => key.secret_sec1_der().to_vec(),
            other => panic!("expected one private key block, got {other:?}"),
        }
    }

    /// `Der`'s `Debug` prints `0x` once and then lowercase hex for the whole payload; a plain
    /// `Vec<u8>` prints decimal numbers in brackets. Both shapes are searched below, so the
    /// windows here carry no `0x` of their own: inside a rendering it only appears at the front.
    fn hex(bytes: &[u8]) -> String {
        let mut out = String::new();
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// The derived `Debug` on the TLS types renders the certificate chain — public data, and the
    /// positive control below insists it is still there — plus rustls' signing key, whose `Debug`
    /// prints the algorithm only. This pins the whole rendering: no window of the private key
    /// appears, in the PEM's base64 text, in DER hex, or in the decimal byte-array shape a
    /// `Vec<u8>` field would produce. A `Debug` that gained the key, or a rustls upgrade whose
    /// backend printed it, fails here.
    #[test]
    fn the_debug_rendering_carries_no_private_key_bytes() {
        let pki = test_pki("localhost");
        let set = pki.set();
        let identity = TlsIdentity::for_name("localhost", set.clone()).expect("a DNS name");

        let leaf = set.certified.cert[0].as_ref();
        let cert_window = &leaf[leaf.len() / 2..leaf.len() / 2 + 12];

        let key_der = private_key_der(&pki.key_pem);
        // Both the key's DER and the PEM bytes the operator supplied: a field that kept either one
        // would print it as decimal numbers in a derived `Debug`. A window that also occurs in the
        // public chain cannot tell a key leak apart from the certificate rendering — the shared
        // ASN.1 algorithm identifiers do exactly that — so only key-unique windows are probes.
        let secret: Vec<u8> = [key_der.as_slice(), pki.key_pem.as_slice()].concat();
        let public: Vec<u8> = set
            .certified
            .cert
            .iter()
            .flat_map(|cert| cert.as_ref().to_vec())
            .collect();
        let probes: Vec<&[u8]> = secret
            .windows(12)
            .step_by(4)
            .filter(|window| !public.windows(12).any(|bytes| bytes == *window))
            .collect();
        assert!(
            probes.len() >= 5,
            "only {} key-unique windows to probe; the scan would be vacuous",
            probes.len()
        );

        let pem_text = String::from_utf8(pki.key_pem.clone()).expect("PEM is ASCII");
        let body: String = pem_text
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let pem_window = &body[body.len() / 2..body.len() / 2 + 24];

        for rendered in [format!("{set:?}"), format!("{identity:?}")] {
            assert!(
                rendered.contains(&hex(cert_window)),
                "positive control: the public certificate must still be rendered: {rendered}"
            );
            assert!(
                !rendered.contains(pem_window),
                "the private key's PEM text must not be rendered: {rendered}"
            );
            for window in &probes {
                // A byte slice renders as `[45, 45, …]`, so a leaked window is surrounded by
                // `[`/`]` or `, ` on each side. Anchoring on those boundaries is what keeps the
                // probe honest: searching for the bare digits would match inside a longer number.
                let digits = window
                    .iter()
                    .map(|byte| byte.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let decimals = [
                    format!("[{digits}, "),
                    format!(", {digits}, "),
                    format!(", {digits}]"),
                ];
                assert!(
                    !decimals.iter().any(|form| rendered.contains(form))
                        && !rendered.contains(&hex(window)),
                    "private key bytes must not be rendered: {window:?}"
                );
            }
        }
    }

    /// The order is the promise: `h2` first means a client offering both gets HTTP/2, and
    /// `http/1.1` last means a client offering only that still gets served.
    #[test]
    fn the_default_acceptor_offers_h2_then_http_1() {
        let pki = test_pki("localhost");
        let acceptor =
            TlsAcceptor::new(TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("loaded"))
                .expect("configured");
        assert_eq!(
            acceptor.alpn(),
            [b"h2".to_vec(), b"http/1.1".to_vec()],
            "the HTTP/2 frontend is wired, so ALPN must offer it"
        );
        assert!(!acceptor.requires_tls());
    }

    /// Opting out has to be real, not cosmetic: a deployment that pins the port to HTTP/1.1 must
    /// not have an `h2` client silently handed a protocol it did not advertise.
    #[test]
    fn advertising_h2_can_be_turned_off_and_pins_the_port_to_http_1() {
        let pki = test_pki("localhost");
        let set = TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("loaded");
        let acceptor = TlsAcceptor::builder()
            .insert(TlsIdentity::default_identity(set))
            .advertise_h2(false)
            .build()
            .expect("configured");
        assert_eq!(acceptor.alpn(), [b"http/1.1".to_vec()]);
    }

    #[test]
    fn a_builder_with_no_catch_all_is_refused_not_silently_repaired() {
        let pki = test_pki("localhost");
        let set = TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("loaded");
        let only_named = TlsAcceptor::builder()
            .insert(TlsIdentity::for_name("example.com", set).expect("a dns name"))
            .build();
        assert!(matches!(
            only_named.err(),
            Some(TlsError::NoDefaultIdentity)
        ));
    }

    #[test]
    fn the_leaf_is_first_and_the_ca_is_a_usable_trust_anchor() {
        let pki = test_pki("localhost");
        let set = TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("loaded");
        let mut reader = std::io::BufReader::new(pki.cert_pem.as_slice());
        let pem_chain: Vec<_> = rustls_pemfile::certs(&mut reader)
            .map(|c| c.expect("one pem block"))
            .collect();
        assert_eq!(pem_chain.len(), 2);
        assert_eq!(set.certified.cert[0].as_ref(), pem_chain[0].as_ref());

        let mut anchors = rustls::RootCertStore::empty();
        let mut ca_reader = std::io::BufReader::new(pki.ca_pem.as_slice());
        let ca = rustls_pemfile::certs(&mut ca_reader)
            .next()
            .expect("one ca block")
            .expect("valid ca pem");
        assert!(
            anchors.add(ca).is_ok(),
            "the test CA must be a trust anchor, or the handshake tests cannot verify our chain"
        );
    }

    #[test]
    fn sni_names_are_normalized_and_selection_falls_back_to_the_catch_all() {
        let pki = test_pki("localhost");
        let set = TlsCertSet::from_pem(&pki.cert_pem, &pki.key_pem).expect("loaded");
        assert_eq!(
            TlsIdentity::for_name("Example.COM.", set.clone())
                .expect("normalized")
                .server_name
                .as_deref(),
            Some("example.com")
        );
        assert!(TlsIdentity::for_name("127.0.0.1", set.clone()).is_err());
        assert!(TlsIdentity::for_name("", set.clone()).is_err());

        let other = test_pki("example.com");
        let other_set = TlsCertSet::from_pem(&other.cert_pem, &other.key_pem).expect("loaded");
        let shared = SharedCerts::from_identities(vec![
            TlsIdentity::default_identity(set.clone()),
            TlsIdentity::for_name("example.com", other_set.clone()).expect("named"),
        ])
        .expect("one default, one named");
        assert_eq!(
            shared.pick(Some("example.com")).cert[0].as_ref(),
            other_set.certified.cert[0].as_ref()
        );
        // A client that sent no SNI, and one that asked for a name this server does not hold,
        // both get the catch-all rather than a failed handshake.
        assert_eq!(
            shared.pick(None).cert[0].as_ref(),
            set.certified.cert[0].as_ref()
        );
        assert_eq!(
            shared.pick(Some("unlisted.example")).cert[0].as_ref(),
            set.certified.cert[0].as_ref()
        );

        let second_default = SharedCerts::from_identities(vec![
            TlsIdentity::default_identity(set.clone()),
            TlsIdentity::default_identity(set),
        ]);
        assert!(matches!(
            second_default.err(),
            Some(TlsError::NoDefaultIdentity)
        ));
    }

    #[test]
    fn replace_certificates_is_visible_to_the_next_resolution_and_leaves_a_failure_untouched() {
        let first = test_pki("localhost");
        let second = test_pki("alt.example.com");
        let mut acceptor = TlsAcceptor::new(
            TlsCertSet::from_pem(&first.cert_pem, &first.key_pem).expect("loaded"),
        )
        .expect("configured");
        acceptor.set_require_tls(true);
        assert!(acceptor.requires_tls());

        let replacement = TlsCertSet::from_pem(&second.cert_pem, &second.key_pem).expect("loaded");
        let before = acceptor.resolver.live.lock().unwrap().clone();
        acceptor
            .replace_certificates(vec![TlsIdentity::default_identity(replacement)])
            .expect("reload");
        let after = acceptor.resolver.live.lock().unwrap().clone();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.default.cert.len(), 2);
        // `before` still holds the old chain: a handshake that read it before the swap keeps the
        // certificate it started with instead of switching mid-flight.
        assert_ne!(
            before.default.cert[0].as_ref(),
            after.default.cert[0].as_ref()
        );
    }

    #[test]
    fn a_failed_reload_does_not_change_what_is_served() {
        let good = test_pki("localhost");
        let acceptor =
            TlsAcceptor::new(TlsCertSet::from_pem(&good.cert_pem, &good.key_pem).expect("loaded"))
                .expect("configured");
        let before = acceptor.resolver.live.lock().unwrap().clone();
        let err = acceptor
            .replace_certificates(Vec::new())
            .expect_err("empty reload");
        assert!(matches!(err, TlsError::NoDefaultIdentity), "{err}");
        assert!(Arc::ptr_eq(
            &before,
            &acceptor.resolver.live.lock().unwrap().clone()
        ));
    }

    /// Enforcement happens at load, the only moment this acceptor consults a list, so a revoked
    /// identity must be unconstructible rather than constructible-and-refused-later.
    #[test]
    fn a_crl_that_lists_the_leaf_refuses_the_set() {
        let pki = test_pki("localhost");
        let list = pki.crl(
            &[LEAF_SERIAL],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(7),
        );
        let err = pki
            .set()
            .with_crl(list.der())
            .expect_err("a revoked leaf must not load");
        assert!(matches!(err, TlsError::CertificateRevoked), "{err}");
    }

    #[test]
    fn a_current_crl_that_lists_other_serials_leaves_the_set_servable() {
        let pki = test_pki("localhost");
        let list = pki.crl(
            &[&[0x2b], &[0x2c]],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(7),
        );
        let set = pki
            .set()
            .with_crl(list.der())
            .expect("a current CRL that does not name the leaf");
        let acceptor = TlsAcceptor::new(set).expect("configured");
        assert_eq!(acceptor.alpn().first().map(Vec::as_slice), Some(&b"h2"[..]));
    }

    /// An ignored list is the failure this call exists to prevent, so both freshness directions
    /// refuse rather than warn.
    #[test]
    fn a_stale_or_not_yet_valid_crl_is_refused() {
        let pki = test_pki("localhost");
        let stale = pki.crl(
            &[],
            now() - time::Duration::days(2),
            now() - time::Duration::days(1),
        );
        let err = pki.set().with_crl(stale.der()).expect_err("stale list");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
        assert!(err.to_string().contains("stale"), "{err}");

        let early = pki.crl(
            &[],
            now() + time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let err = pki.set().with_crl(early.der()).expect_err("not yet valid");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
        assert!(err.to_string().contains("future"), "{err}");
    }

    #[test]
    fn a_crl_signed_outside_the_chain_or_tampered_with_is_unusable() {
        let pki = test_pki("localhost");
        let other = test_pki("localhost");
        let foreign = other.crl(
            &[],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let err = pki.set().with_crl(foreign.der()).expect_err("wrong signer");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");

        let ours = pki.crl(
            &[],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let mut tampered = ours.der().to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let err = pki
            .set()
            .with_crl(&tampered)
            .expect_err("a flipped signature byte must not load");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
    }

    #[test]
    fn the_crl_reads_as_pem_and_garbage_is_named() {
        let pki = test_pki("localhost");
        let list = pki.crl(
            &[],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let pem = list.pem().expect("PEM output");
        pki.set().with_crl(pem.as_bytes()).expect("PEM accepted");

        let err = pki
            .set()
            .with_crl(b"-----BEGIN X509 CRL-----\nnot base64\n-----END X509 CRL-----\n")
            .expect_err("garbage PEM");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
        let err = pki
            .set()
            .with_crl(b"\x30\x03\x01\x01\x00")
            .expect_err("garbage DER");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
    }

    /// A chain of one is self-signed; the leaf's own key is then the only key that can have
    /// signed the list, and the check must still run.
    #[test]
    fn a_self_signed_set_is_checked_against_its_own_key() {
        use rcgen::{BasicConstraints, DnType, IsCa, KeyPair, SerialNumber};

        let mut params =
            rcgen::CertificateParams::new(Vec::<String>::new()).expect("empty SAN is valid");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "self signed");
        params.serial_number = Some(SerialNumber::from_slice(LEAF_SERIAL));
        let key = KeyPair::generate().expect("key generation");
        let key_pem = key.serialize_pem();
        let cert = params.self_signed(&key).expect("self-signed cert");
        let issuer = Issuer::new(params, key);

        let set = TlsCertSet::from_pem(cert.pem().as_bytes(), key_pem.as_bytes())
            .expect("a self-signed set is valid");
        let list = signed_crl(
            &issuer,
            &[LEAF_SERIAL],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let err = set
            .with_crl(list.der())
            .expect_err("revoked self-signed leaf");
        assert!(matches!(err, TlsError::CertificateRevoked), "{err}");
    }

    #[derive(Clone, Copy)]
    enum OcspStatus {
        Good,
        Revoked,
        Unknown,
    }

    /// Who claims to have signed a fixture response.
    ///
    /// `ForeignKey` keeps the issuer's identity — its CertId and responder id — but signs with
    /// another key, which is the forgery a staple validator has to catch. `Delegated` names and
    /// includes a responder certificate by its own subject instead.
    enum OcspSigner<'a> {
        Issuer,
        ForeignKey(&'a KeyPair),
        Delegated { key: &'a KeyPair, cert: &'a [u8] },
    }

    struct OcspSpec<'a> {
        serial: &'a [u8],
        status: OcspStatus,
        this: time::OffsetDateTime,
        next: Option<time::OffsetDateTime>,
        signer: OcspSigner<'a>,
        bind_to: Option<&'a TestPki>,
        response_state: u32,
        /// Stamp `producedAt` as a UTCTime: RFC 6960 requires GeneralizedTime there.
        produced_at_utctime: bool,
    }

    fn good_spec() -> OcspSpec<'static> {
        OcspSpec {
            serial: LEAF_SERIAL,
            status: OcspStatus::Good,
            this: now() - time::Duration::hours(1),
            next: Some(now() + time::Duration::days(1)),
            signer: OcspSigner::Issuer,
            bind_to: None,
            response_state: 0,
            produced_at_utctime: false,
        }
    }

    fn sha1(bytes: &[u8]) -> Vec<u8> {
        ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes)
            .as_ref()
            .to_vec()
    }

    /// The name and key hashes a CertId for this CA must carry, read from the CA certificate the
    /// same way the validator reads them from the served chain.
    fn issuer_hashes(pki: &TestPki) -> (Vec<u8>, Vec<u8>) {
        let (_, ca) = X509Certificate::from_der(&pki.ca_der).expect("CA DER");
        (
            sha1(ca.subject().as_raw()),
            sha1(ca.public_key().subject_public_key.data.as_ref()),
        )
    }

    /// A responder certificate issued by this PKI's CA, with or without `id-kp-OCSPSigning`.
    fn responder(pki: &TestPki, ocsp_signing: bool) -> (KeyPair, Vec<u8>) {
        use rcgen::{DnType, ExtendedKeyUsagePurpose, KeyUsagePurpose};

        let key = KeyPair::generate().expect("responder key");
        let mut params =
            rcgen::CertificateParams::new(vec!["responder.local".to_string()]).expect("a DNS name");
        params
            .distinguished_name
            .push(DnType::CommonName, "Aether test responder");
        params.key_usages.push(KeyUsagePurpose::DigitalSignature);
        params.extended_key_usages.push(if ocsp_signing {
            ExtendedKeyUsagePurpose::OcspSigning
        } else {
            ExtendedKeyUsagePurpose::ServerAuth
        });
        let cert = params
            .signed_by(&key, &pki.issuer)
            .expect("responder certificate signed by the CA");
        (key, cert.der().as_ref().to_vec())
    }

    /// Encode one signed basic OCSP response. Everything is relative to the caller's clock so
    /// every freshness and status rule can be driven without a committed fixture.
    fn ocsp(pki: &TestPki, spec: &OcspSpec<'_>) -> Vec<u8> {
        use rcgen::SigningKey;
        use yasna::Tag;
        use yasna::models::{GeneralizedTime, ObjectIdentifier, UTCTime};

        let (name_hash, key_hash) = issuer_hashes(spec.bind_to.unwrap_or(pki));
        let ca_key = KeyPair::from_pem(&pki.ca_key_pem).expect("CA key from PEM");
        let (signer, included, delegated_subject) = match &spec.signer {
            OcspSigner::Issuer => (&ca_key, None, None),
            OcspSigner::ForeignKey(key) => (*key, None, None),
            OcspSigner::Delegated { key, cert } => {
                let (_, parsed) = X509Certificate::from_der(cert).expect("responder DER");
                (*key, Some(*cert), Some(parsed.subject().as_raw().to_vec()))
            }
        };

        let tbs = yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                match &delegated_subject {
                    Some(subject) => writer
                        .next()
                        .write_tagged(Tag::context(1), |writer| writer.write_der(subject)),
                    None => writer
                        .next()
                        .write_tagged_implicit(Tag::context(2), |writer| {
                            writer.write_bytes(&key_hash)
                        }),
                }
                if spec.produced_at_utctime {
                    writer
                        .next()
                        .write_utctime(&UTCTime::from_datetime(spec.this));
                } else {
                    writer
                        .next()
                        .write_generalized_time(&GeneralizedTime::from_datetime(spec.this));
                }
                writer.next().write_sequence_of(|writer| {
                    writer.next().write_sequence(|writer| {
                        writer.next().write_sequence(|writer| {
                            writer.next().write_sequence(|writer| {
                                writer.next().write_oid(&ObjectIdentifier::from_slice(&[
                                    1, 3, 14, 3, 2, 26,
                                ]));
                                writer.next().write_null();
                            });
                            writer.next().write_bytes(&name_hash);
                            writer.next().write_bytes(&key_hash);
                            writer.next().write_bigint_bytes(spec.serial, true);
                        });
                        match spec.status {
                            OcspStatus::Good => writer
                                .next()
                                .write_tagged_implicit(Tag::context(0), |writer| {
                                    writer.write_bytes(&[])
                                }),
                            OcspStatus::Revoked => {
                                writer
                                    .next()
                                    .write_tagged_implicit(Tag::context(1), |writer| {
                                        writer.write_sequence(|writer| {
                                            writer.next().write_generalized_time(
                                                &GeneralizedTime::from_datetime(spec.this),
                                            );
                                        })
                                    })
                            }
                            OcspStatus::Unknown => writer
                                .next()
                                .write_tagged_implicit(Tag::context(2), |writer| {
                                    writer.write_bytes(&[])
                                }),
                        }
                        writer
                            .next()
                            .write_generalized_time(&GeneralizedTime::from_datetime(spec.this));
                        if let Some(next) = spec.next {
                            writer.next().write_tagged(Tag::context(0), |writer| {
                                writer
                                    .write_generalized_time(&GeneralizedTime::from_datetime(next));
                            });
                        }
                    });
                });
            });
        });

        if spec.response_state != 0 {
            return yasna::construct_der(|writer| {
                writer.write_sequence(|writer| {
                    writer.next().write_enum(spec.response_state as i64);
                });
            });
        }

        let signature = signer.sign(&tbs).expect("sign the response data");
        let basic = yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                writer.next().write_der(&tbs);
                writer.next().write_sequence(|writer| {
                    writer
                        .next()
                        .write_oid(&ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 4, 3, 2]));
                });
                writer
                    .next()
                    .write_bitvec_bytes(&signature, signature.len() * 8);
                if let Some(cert) = included {
                    writer.next().write_tagged(Tag::context(0), |writer| {
                        writer.write_sequence_of(|writer| {
                            writer.next().write_der(cert);
                        });
                    });
                }
            });
        });
        let response_bytes = yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                writer.next().write_oid(&ObjectIdentifier::from_slice(&[
                    1, 3, 6, 1, 5, 5, 7, 48, 1, 1,
                ]));
                writer.next().write_bytes(&basic);
            });
        });
        yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                writer.next().write_enum(0);
                writer
                    .next()
                    .write_tagged(Tag::context(0), |writer| writer.write_der(&response_bytes));
            });
        })
    }

    #[test]
    fn a_current_staple_that_clears_the_leaf_is_attached() {
        let pki = test_pki("localhost");
        let staple = ocsp(&pki, &good_spec());
        let set = pki
            .set()
            .with_ocsp(staple.clone())
            .expect("a current staple for this chain");
        assert_eq!(set.certified.ocsp.as_deref(), Some(staple.as_slice()));
    }

    #[test]
    fn a_staple_that_does_not_name_this_certificate_does_not_cover_it() {
        let pki = test_pki("localhost");
        let wrong_serial = OcspSpec {
            serial: &[0x2b],
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &wrong_serial))
            .expect_err("a staple for another serial");
        assert!(err.to_string().contains("does not cover"), "{err}");

        let other_ca = test_pki("localhost");
        let wrong_issuer = OcspSpec {
            bind_to: Some(&other_ca),
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &wrong_issuer))
            .expect_err("a staple bound to another issuer");
        assert!(err.to_string().contains("does not cover"), "{err}");
    }

    #[test]
    fn a_staple_that_reports_revocation_or_unknown_refuses_the_set() {
        let pki = test_pki("localhost");
        let revoked = OcspSpec {
            status: OcspStatus::Revoked,
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &revoked))
            .expect_err("a revoked status");
        assert!(matches!(err, TlsError::CertificateRevoked), "{err}");

        let unknown = OcspSpec {
            status: OcspStatus::Unknown,
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &unknown))
            .expect_err("an unknown status");
        assert!(err.to_string().contains("unknown"), "{err}");
    }

    #[test]
    fn a_stale_or_not_yet_valid_staple_is_refused() {
        let pki = test_pki("localhost");
        let stale = OcspSpec {
            this: now() - time::Duration::days(2),
            next: Some(now() - time::Duration::days(1)),
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &stale))
            .expect_err("a stale staple");
        assert!(err.to_string().contains("stale"), "{err}");

        let early = OcspSpec {
            this: now() + time::Duration::hours(1),
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &early))
            .expect_err("a staple from the future");
        assert!(err.to_string().contains("future"), "{err}");

        let open_ended = OcspSpec {
            next: None,
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &open_ended))
            .expect_err("a staple with no nextUpdate");
        assert!(err.to_string().contains("nextUpdate"), "{err}");
    }

    #[test]
    fn a_utc_time_where_a_generalized_time_belongs_is_refused() {
        let pki = test_pki("localhost");
        let spec = OcspSpec {
            produced_at_utctime: true,
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &spec))
            .expect_err("producedAt stamped as a UTCTime");
        assert!(err.to_string().contains("GeneralizedTime"), "{err}");
    }

    #[test]
    fn a_forged_or_tampered_signature_is_unusable() {
        let pki = test_pki("localhost");
        let foreign_key = KeyPair::generate().expect("key generation");
        let forged = OcspSpec {
            signer: OcspSigner::ForeignKey(&foreign_key),
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &forged))
            .expect_err("a signature by an unrelated key");
        assert!(err.to_string().contains("does not verify"), "{err}");

        let mut tampered = ocsp(&pki, &good_spec());
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let err = pki
            .set()
            .with_ocsp(tampered)
            .expect_err("a flipped signature byte");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
    }

    #[test]
    fn a_delegated_responder_must_be_authorised_by_this_chain() {
        let pki = test_pki("localhost");
        let (key, cert) = responder(&pki, true);
        let delegated = OcspSpec {
            signer: OcspSigner::Delegated {
                key: &key,
                cert: &cert,
            },
            ..good_spec()
        };
        pki.set()
            .with_ocsp(ocsp(&pki, &delegated))
            .expect("an issuer-signed responder with id-kp-OCSPSigning");

        let (key, cert) = responder(&pki, false);
        let unauthorised = OcspSpec {
            signer: OcspSigner::Delegated {
                key: &key,
                cert: &cert,
            },
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &unauthorised))
            .expect_err("a responder without OCSP signing rights");
        assert!(err.to_string().contains("OCSPSigning"), "{err}");

        let foreign = test_pki("localhost");
        let (key, cert) = responder(&foreign, true);
        let foreign_responder = OcspSpec {
            signer: OcspSigner::Delegated {
                key: &key,
                cert: &cert,
            },
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &foreign_responder))
            .expect_err("a responder certificate from another CA");
        assert!(err.to_string().contains("not signed by"), "{err}");
    }

    #[test]
    fn a_non_successful_responder_answer_is_named() {
        let pki = test_pki("localhost");
        let later = OcspSpec {
            response_state: 3,
            ..good_spec()
        };
        let err = pki
            .set()
            .with_ocsp(ocsp(&pki, &later))
            .expect_err("tryLater is not a staple");
        assert!(err.to_string().contains("tryLater"), "{err}");
    }

    #[test]
    fn garbage_is_named_as_unusable_revocation_material() {
        let pki = test_pki("localhost");
        let err = pki
            .set()
            .with_ocsp(b"\x30\x03\x01\x01\x00".to_vec())
            .expect_err("garbage DER");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");

        let list = pki.crl(
            &[],
            now() - time::Duration::hours(1),
            now() + time::Duration::days(1),
        );
        let err = pki
            .set()
            .with_ocsp(list.der().to_vec())
            .expect_err("a CRL is not an OCSP response");
        assert!(matches!(err, TlsError::RevocationUnusable(_)), "{err}");
    }
}
