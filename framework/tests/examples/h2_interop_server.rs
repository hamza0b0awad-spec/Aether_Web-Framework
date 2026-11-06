//! A live HTTP/2 server, for the manual interop check against a client implementation that is
//! not ours (Node.js's built-in `http2`, and a hand-framed cleartext client).
//!
//! Not a test: it has no assertions, because the point is to be measured by something else. The
//! certificate is minted in memory by `rcgen` and never written to disk, so nothing here is a
//! secret and nothing here is committed.
//!
//! ```text
//! cargo run -p aether-tests --example h2_interop_server -- 18443          # TLS + ALPN h2
//! cargo run -p aether-tests --example h2_interop_server -- 18444 h2c      # cleartext, prior knowledge
//! cargo run -p aether-tests --example h2_interop_server -- 18445 strict   # require_tls + enable_h2c
//! ```
//!
//! `strict` is the mode that answers "can `enable_h2c` open a cleartext door on a port that
//! requires TLS?": it enables the cleartext HTTP/2 frontend and requires TLS at the same time, so
//! an external client can probe a plaintext HTTP/1.1 request and a plaintext HTTP/2 preface
//! against it. The issuing CA certificate is printed between `CA-PEM-BEGIN`/`CA-PEM-END` for the
//! client to trust; a leaf certificate is public information, not a secret, and the private key
//! never leaves this process.

use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use aether::prelude::*;
use aether::tls::{TlsAcceptor, TlsCertSet};

fn pki(name: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    use rcgen::{
        BasicConstraints, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };

    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "Aether interop CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    let issuer = Issuer::new(ca_params, ca_key);

    let mut leaf_params =
        rcgen::CertificateParams::new(vec![name.to_string()]).expect("leaf params");
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, name.to_string());
    leaf_params.use_authority_key_identifier_extension = true;
    leaf_params
        .key_usages
        .push(KeyUsagePurpose::DigitalSignature);
    leaf_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ServerAuth);
    let leaf_key = KeyPair::generate().expect("leaf key");
    let leaf = leaf_params
        .signed_by(&leaf_key, &issuer)
        .expect("signed leaf");

    let mut chain = leaf.pem().into_bytes();
    chain.extend_from_slice(ca_cert.pem().as_bytes());
    (
        chain,
        leaf_key.serialize_pem().into_bytes(),
        ca_cert.pem().into_bytes(),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = env::args()
        .nth(1)
        .map(|a| a.parse().expect("port"))
        .unwrap_or(18443);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("address");
    // `h2c` selects the cleartext prior-knowledge path: the same routes and the same serving
    // core, with no TLS and no ALPN underneath — so what an independent client proves about the
    // framing here, it proves about this server rather than about rustls. `strict` keeps that
    // frontend enabled but puts `require_tls` underneath it, which is the combination a client
    // must not be able to walk through.
    let mode = env::args().nth(2).unwrap_or_else(|| "tls".to_string());
    let cleartext = mode == "h2c";
    let strict = mode == "strict";

    let mut app = AetherApp::new();
    app = app.get("/ping", |_| Response::text("pong"));
    app = app.get_async("/slow", |_req: RequestContext| async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Response::text("slow")
    });
    // Large enough to be split across many DATA frames and to exercise flow control in both
    // directions: 5 MiB is far past any window size.
    app = app.get("/big", |_| {
        Response::text_dynamic("a".repeat(5 * 1024 * 1024))
    });
    app = app.post_async("/echo", |req: RequestContext| async move {
        Response::text_dynamic(format!(
            "method={}|path={}|query={}|x-marker={}|body={}",
            req.method,
            req.path,
            req.query_str.as_deref().unwrap_or(""),
            req.header("x-marker").unwrap_or(""),
            String::from_utf8_lossy(&req.body),
        ))
    });
    // Echoes the three fields an independent client can be made to send and the library under test
    // may or may not forward, so the interop check can tell "refused" from "stripped" from "seen".
    app = app.get("/hdr", |req: RequestContext| {
        Response::text_dynamic(format!(
            "connection={}|te={}|transfer-encoding={}|upgrade={}",
            req.header("connection").unwrap_or("<none>"),
            req.header("te").unwrap_or("<none>"),
            req.header("transfer-encoding").unwrap_or("<none>"),
            req.header("upgrade").unwrap_or("<none>"),
        ))
    });
    // A WebSocket route, so an independent client can check what HTTP/2 does with one: the answer
    // is a 400, because HTTP/2 has no Upgrade mechanism (RFC 9113 §8.1).
    app = app.ws("/live", |_ctx, _socket| async move {});
    app = app.get("/fast", |_| Response::text("fast"));

    let mut ca_pem: Option<Vec<u8>> = None;
    let (server, scheme) = if cleartext {
        (app.enable_h2c().bind(&addr.to_string()).await?, "http")
    } else {
        let (chain_pem, key_pem, ca) = pki("localhost");
        ca_pem = Some(ca);
        let acceptor =
            TlsAcceptor::new(TlsCertSet::from_pem(&chain_pem, &key_pem).expect("material"))
                .expect("configured");
        if strict {
            (
                app.tls(acceptor)
                    .require_tls(true)
                    .enable_h2c()
                    .bind(&addr.to_string())
                    .await?,
                "https",
            )
        } else {
            (app.tls(acceptor).bind(&addr.to_string()).await?, "https")
        }
    };
    if let Some(ca) = ca_pem {
        println!("CA-PEM-BEGIN");
        let _ = std::io::stdout().write_all(&ca);
        println!("CA-PEM-END");
    }
    println!("READY {scheme}://{addr} mode={mode}");
    server.serve().await
}
