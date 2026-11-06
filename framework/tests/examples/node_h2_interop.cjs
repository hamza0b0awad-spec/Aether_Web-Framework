// Independent HTTP/2 client probe for `h2_interop_server.rs` in this directory.
// Uses only Node's built-in http2/tls/net/child_process modules - it shares no code with Aether,
// which is the point: RFC 9113 behaviour is judged by an implementation that is not ours.
// Not part of the cargo build (cargo reads only *.rs here). The extension is `.cjs` because the
// repository root used to carry a package.json declaring "type": "module", which made a `.js` file
// here parse as ESM with `require` undefined; track 111 removed that manifest (2026-11-02) and the
// explicit extension stays so the answer does not depend on a root manifest. Run it by hand:
//   cargo build --release -p aether-tests --example h2_interop_server
//   node framework/tests/examples/node_h2_interop.cjs <path-to>/h2_interop_server.exe
// It spawns the three listeners itself (TLS 18943, cleartext h2c 18944, strict 18945), owns their
// lifetime, and exits non-zero if any assertion fails. The measured results are recorded in
// `framework/docs/PRODUCTION_READINESS_REPORT.md` §11.
const net = require('net');
const tls = require('tls');
const http2 = require('http2');
const { spawn } = require('child_process');

const EXE = process.argv[2];
const PORTS = { tls: 18943, h2c: 18944, strict: 18945 };
const results = [];
function check(name, ok, detail) {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '   [' + detail + ']' : ''}`);
}

function launch(port, mode) {
  return new Promise((resolve, reject) => {
    const p = spawn(EXE, [String(port), mode], { stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '';
    const timer = setTimeout(() => reject(new Error(`no READY for ${mode}:${port}: ${out}`)), 30000);
    p.stdout.on('data', (b) => {
      out += b.toString();
      if (out.includes('READY')) {
        clearTimeout(timer);
        const m = /CA-PEM-BEGIN([\s\S]*?)CA-PEM-END/.exec(out);
        resolve({ proc: p, ca: m ? m[1] : null });
      }
    });
    p.stderr.on('data', (b) => (out += b.toString()));
    p.on('exit', (c) => clearTimeout(timer));
  });
}

function request(session, path, opts = {}, body = null, deadline = 20000) {
  return new Promise((resolve, reject) => {
    const req = session.request({ ':path': path, ...opts });
    const chunks = [];
    let status = null;
    const timer = setTimeout(() => { req.close(http2.constants.NGHTTP2_CANCEL); reject(new Error('deadline ' + path)); }, deadline);
    req.on('response', (h) => { status = h[':status']; });
    req.on('data', (d) => chunks.push(d));
    req.on('end', () => { clearTimeout(timer); resolve({ status, body: Buffer.concat(chunks) }); });
    req.on('error', (e) => { clearTimeout(timer); reject(e); });
    if (body) req.end(body); else req.end();
  });
}

// Reads the server's own SETTINGS frame off a TLS stream that already negotiated h2, then decodes
// it here rather than trusting the client library's notion of what the peer advertised.
function serverSettings(ca) {
  return new Promise((resolve, reject) => {
    const sock = tls.connect({ host: '127.0.0.1', port: PORTS.tls, ca, servername: 'localhost', ALPNProtocols: ['h2'] });
    let buf = Buffer.alloc(0);
    const settings = {};
    const timer = setTimeout(() => { sock.destroy(); resolve({ alpn: sock.alpnProtocol, settings }); }, 6000);
    sock.on('secureConnect', () => sock.write(Buffer.from([0, 0, 0, 4, 0, 0, 0, 0, 0])));
    sock.on('data', (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      while (buf.length >= 9) {
        const len = (buf[0] << 16) | (buf[1] << 8) | buf[2];
        const type = buf[3];
        if (buf.length < 9 + len) return;
        if (type === 0x4 && (buf[4] & 1) === 0) {
          for (let i = 9; i < 9 + len; i += 6) {
            settings[buf.slice(i, i + 2).readUInt16BE(0)] = buf.slice(i + 2, i + 6).readUInt32BE(0);
          }
        }
        buf = buf.slice(9 + len);
        if (Object.keys(settings).length) { clearTimeout(timer); sock.destroy(); resolve({ alpn: sock.alpnProtocol, settings }); return; }
      }
    });
    sock.on('error', (e) => { clearTimeout(timer); reject(e); });
  });
}

async function main() {
  const servers = {};
  for (const [mode, port] of Object.entries(PORTS)) servers[mode] = await launch(port, mode);
  const ca = servers.tls.ca;

  // --- 1. ALPN and advertised SETTINGS on the TLS listener ---
  const s = http2.connect(`https://127.0.0.1:${PORTS.tls}`, { ca, servername: 'localhost' });
  s.on('error', () => {});
  await new Promise((r) => (s.once('connect', r), setTimeout(r, 2000)));
  check('ALPN negotiates h2 over TLS', s.alpnProtocol === 'h2', 'alpn=' + s.alpnProtocol);
  const wire = await serverSettings(ca);
  check('SETTINGS frame on the wire advertises MAX_CONCURRENT_STREAMS 100', wire.settings[0x3] === 100,
    'id 0x3 = ' + wire.settings[0x3] + ' (all: ' + JSON.stringify(wire.settings) + ')');
  check('SETTINGS frame on the wire advertises MAX_HEADER_LIST_SIZE 8192 (setting id 0x6)', wire.settings[0x6] === 8192,
    'id 0x6 = ' + wire.settings[0x6]);
  const rs = s.remoteSettings;
  console.log(`INFO  Node's remoteSettings says maxConcurrentStreams=${rs.maxConcurrentStreams}, ` +
    `maxHeaderListSize=${rs.maxHeaderListSize} - compare with the wire values above.`);
  const ping = await request(s, '/ping');
  check('GET /ping over h2 is 200 pong', ping.status === 200 && ping.body.toString() === 'pong', `${ping.status}/${ping.body}`);
  const ws = await request(s, '/live');
  check('WebSocket route over h2 is 400 (RFC 9113 has no Upgrade)', ws.status === 400, `${ws.status}`);

  // --- 2. flow control both directions ---
  const oneMiB = Buffer.alloc(1024 * 1024, 0x61);
  const up = await request(s, '/echo', { ':method': 'POST' }, oneMiB, 60000);
  const sawBackpressure = up.status === 200 && up.body.toString().includes('body=' + 'a'.repeat(1024 * 1024));
  check('1 MiB upload completes under real flow control', sawBackpressure, `status=${up.status} bytes=${up.body.length}`);
  const big = await request(s, '/big', {}, null, 60000);
  check('5 MiB download arrives intact', big.status === 200 && big.body.length === 5 * 1024 * 1024, `${big.status}/${big.body.length}`);

  // --- 3. multiplexing: 50 streams on one socket, 10 behind a 300 ms handler ---
  const t0 = Date.now();
  const mix = await Promise.all(
    Array.from({ length: 40 }, () => request(s, '/ping')).concat(Array.from({ length: 10 }, () => request(s, '/slow', {}, null, 10000)))
  );
  const el = Date.now() - t0;
  check('50 concurrent streams answered on one connection', mix.length === 50 && mix.every((r) => r.status === 200), `elapsed=${el}ms`);

  // --- 4. header budgets ---
  const wide = {};
  for (let i = 0; i < 60; i++) wide['x-pad-' + i] = 'b'.repeat(200);
  let wideRes;
  try { wideRes = await request(s, '/ping', wide, null, 15000); } catch (e) { wideRes = { status: 'stream-error:' + e.code }; }
  check('>8 KiB header list is refused (advertised MAX_HEADER_LIST_SIZE)',
    String(wideRes.status) === '431' || String(wideRes.status).startsWith('stream-error'), JSON.stringify(wideRes.status));
  const many = {};
  for (let i = 0; i < 120; i++) many['x-' + i] = '1';
  let manyRes;
  try { manyRes = await request(s, '/ping', many, null, 15000); } catch (e) { manyRes = { status: 'stream-error:' + e.code }; }
  check('>100 header lines is refused at the per-line budget',
    String(manyRes.status) === '431' || String(manyRes.status).startsWith('stream-error'), JSON.stringify(manyRes.status));
  const stillUp = await request(s, '/ping');
  check('connection still serves after both refusals', stillUp.status === 200, `${stillUp.status}`);

  // --- 5. oversized body mid-stream, then the connection keeps serving ---
  const tooBig = Buffer.alloc(2 * 1024 * 1024 + 49152, 0x62);
  let over;
  try { over = await request(s, '/echo', { ':method': 'POST' }, tooBig, 30000); }
  catch (e) { over = { status: 'stream-error:' + e.code, body: Buffer.from('') }; }
  const refused = over.status === 413 || String(over.status).startsWith('stream-error');
  check('2,146,304 B against the 2 MiB default is refused mid-stream',
    refused && over.body.length < tooBig.length, `${over.status} echoed=${over.body.length}`);
  const after = await request(s, '/ping');
  check('connection still serves after the oversized body', after.status === 200, `${after.status}`);

  // --- 6. cancellation: reset a stream inside its handler, connection keeps serving ---
  const slowReq = s.request({ ':path': '/slow' });
  slowReq.on('error', () => {});
  slowReq.end();
  await new Promise((r) => setTimeout(r, 60));
  slowReq.close(http2.constants.NGHTTP2_CANCEL);
  const cancelled = await request(s, '/ping');
  check('RST_STREAM mid-handler leaves the connection serving', cancelled.status === 200, `${cancelled.status}`);

  // --- 7. the advertised cap is real on the wire: 110 streams at once ---
  let goaway = null;
  s.on('goaway', (h) => { goaway = h; });
  let answered = 0, capRefused = 0, other = 0;
  const codes = {};
  const capT0 = Date.now();
  await Promise.all(Array.from({ length: 110 }, () =>
    request(s, '/slow', {}, null, 120000)
      .then((r) => { if (r.status === 200) answered++; else other++; })
      .catch((e) => { capRefused++; const k = String(e.code || e.message); codes[k] = (codes[k] || 0) + 1; })
  ));
  const capElapsed = Date.now() - capT0;
  // Node is cap-aware: it reads the advertised 100 and queues the rest, so nothing is refused here.
  // The two-wave wall clock (110 handlers of 300 ms could not all overlap) is the indirect proof that
  // the advertisement constrained the schedule; the direct measurement is h2_handframed_cap.py.
  check('110 streams through a cap-aware client: 110 answered, none refused, two waves',
    answered === 110 && capRefused === 0 && capElapsed >= 500,
    `answered=${answered} refused=${capRefused} other=${other} elapsed=${capElapsed}ms codes=${JSON.stringify(codes)}`);
  check('no GOAWAY was received on the h2 session', goaway === null, 'goaway=' + JSON.stringify(goaway));
  s.close();

  // --- 8. cleartext-on-strict refusals (raw sockets) ---
  async function strictProbe(label, bytes) {
    return await new Promise((resolve) => {
      const sock = net.connect(PORTS.strict, '127.0.0.1', () => sock.write(bytes));
      let got = 0;
      sock.on('data', (b) => { got += b.length; });
      const t = setTimeout(() => { sock.destroy(); resolve({ closed: false, got }); }, 4000);
      sock.on('close', () => { clearTimeout(t); resolve({ closed: true, got }); });
      sock.on('error', () => { clearTimeout(t); resolve({ closed: true, got }); });
    });
  }
  const h1 = await strictProbe('http/1.1', Buffer.from('GET /ping HTTP/1.1\r\nHost: localhost\r\nAccept: */*\r\n\r\n'));
  check('cleartext HTTP/1.1 on the strict port is closed without an answer', h1.closed && h1.got === 0, JSON.stringify(h1));
  const preface = Buffer.concat([
    Buffer.from('PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n'),
    Buffer.from([0, 0, 12, 4, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 100, 0, 4, 0, 0, 0, 32]),
  ]);
  const h2c = await strictProbe('h2 preface', preface);
  check('cleartext h2 preface on the strict port is closed without an answer', h2c.closed && h2c.got === 0, JSON.stringify(h2c));
  const silent = await new Promise((resolve) => {
    const sock = net.connect(PORTS.strict, '127.0.0.1');
    let got = 0;
    sock.on('data', (b) => (got += b.length));
    const t = setTimeout(() => { sock.destroy(); resolve({ closed: false, got }); }, 6000);
    sock.on('close', () => { clearTimeout(t); resolve({ closed: true, got }); });
    sock.on('error', () => { clearTimeout(t); resolve({ closed: true, got }); });
  });
  check('silent cleartext connection on the strict port is closed', silent.closed && silent.got === 0, JSON.stringify(silent));

  // --- 9. TLS without ALPN, and with http/1.1 only ---
  function rawTls(alpn, payload) {
    return new Promise((resolve, reject) => {
      const opts = { host: '127.0.0.1', port: PORTS.tls, ca, servername: 'localhost', rejectUnauthorized: true };
      if (alpn) opts.ALPNProtocols = alpn;
      const sock = tls.connect(opts);
      const chunks = [];
      sock.on('data', (b) => chunks.push(b));
      sock.on('secureConnect', () => sock.write(payload));
      const t = setTimeout(() => { sock.destroy(); resolve({ alpn: sock.alpnProtocol, text: Buffer.concat(chunks).toString('latin1') }); }, 4000);
      sock.on('error', (e) => { clearTimeout(t); reject(e); });
    });
  }
  const h1overTls = await rawTls(null, Buffer.from('GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n'));
  check('TLS with no ALPN offered is served over HTTP/1.1',
    !h1overTls.alpn && h1overTls.text.startsWith('HTTP/1.1 200'), `alpn=${JSON.stringify(h1overTls.alpn)} first=${h1overTls.text.slice(0, 14).trim()}`);
  const h11alpn = await rawTls(['http/1.1'], Buffer.from('GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n'));
  check('TLS negotiating http/1.1 serves HTTP/1.1',
    h11alpn.alpn === 'http/1.1' && h11alpn.text.startsWith('HTTP/1.1 200'), `alpn=${h11alpn.alpn}`);

  // --- 10. Node against the cleartext h2c port (informational, not a §11 row) ---
  const c = http2.connect(`http://127.0.0.1:${PORTS.h2c}`);
  c.on('error', () => {});
  let cleartext;
  try { cleartext = await request(c, '/ping', {}, null, 8000); }
  catch (e) { cleartext = { status: 'client-error:' + (e.code || e.message) }; }
  check('cleartext prior-knowledge h2 serves /ping with 200', cleartext.status === 200 && cleartext.body.toString() === 'pong',
    `${cleartext.status}/${cleartext.body ? cleartext.body.length : 0}`);
  let cleartextWs;
  try { cleartextWs = await request(c, '/live', {}, null, 8000); }
  catch (e) { cleartextWs = { status: 'client-error:' + (e.code || e.message), body: Buffer.from('') }; }
  check('WebSocket route over cleartext h2 is 400', cleartextWs.status === 400, JSON.stringify(cleartextWs.status));
  console.log(`INFO  Node against the cleartext h2c listener: ${JSON.stringify(cleartext.status)} ` +
    `(body=${cleartext.body ? cleartext.body.length : 0}) — which bytes Node put on the wire is measured in section 11.`);
  c.close();

  // --- 11. what Node actually puts on a cleartext h2 connection, and whether the server honours
  //         the `Upgrade: h2c` exchange (docs say it is a deliberate non-goal) ---
  const firstBytes = await new Promise((resolve) => {
    const listener = net.createServer((sock) => {
      sock.once('data', (b) => { listener.close(); sock.destroy(); resolve(b.slice(0, 32)); });
    });
    listener.listen(18999, '127.0.0.1', () => {
      const c = http2.connect('http://127.0.0.1:18999');
      c.on('error', () => {});
      setTimeout(() => { c.destroy(); resolve(Buffer.alloc(0)); }, 3000);
    });
  });
  const isPreface = firstBytes.subarray(0, 24).toString('latin1').startsWith('PRI * HTTP/2.0');
  const isUpgrade = /\r?\nupgrade:\s*h2c/i.test(firstBytes.toString('latin1')) || firstBytes.toString('latin1').includes(' HTTP/1.1');
  console.log(`INFO  Node's first cleartext bytes: ${JSON.stringify(firstBytes.toString('latin1'))} -> ` +
    `${isPreface ? 'h2 preface (prior knowledge)' : isUpgrade ? 'HTTP/1.1 Upgrade exchange' : 'unknown'}`);

  const upg = await new Promise((resolve) => {
    const sock = net.connect(PORTS.h2c, '127.0.0.1', () => sock.write(Buffer.from(
      'GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade, HTTP2-Settings\r\n' +
      'Upgrade: h2c\r\nHTTP2-Settings: AABAAEIAAAAAAUQACAAIAAoAAAAKAAgACgA=\r\n\r\n')));
    const chunks = [];
    sock.on('data', (b) => chunks.push(b));
    const t = setTimeout(() => { sock.destroy(); resolve(Buffer.concat(chunks).toString('latin1')); }, 4000);
    sock.on('close', () => { clearTimeout(t); resolve(Buffer.concat(chunks).toString('latin1')); });
    sock.on('error', () => { clearTimeout(t); resolve(Buffer.concat(chunks).toString('latin1')); });
  });
  check('an Upgrade: h2c request is served as HTTP/1.1, never switched to h2',
    upg.startsWith('HTTP/1.1 200') && !/101/.test(upg.slice(0, 20)), firstBytes.length ? `first=${upg.slice(0, 16).trim()}` : upg.slice(0, 16));

  for (const { proc } of Object.values(servers)) proc.kill();
  const failed = results.filter((r) => !r.ok);
  console.log(`\n${results.length - failed.length}/${results.length} assertions passed`);
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => { console.error('RUNNER ERROR', e); process.exit(2); });
