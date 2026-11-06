"""Hand-framed HTTP/2 client: the check a cap-aware library client cannot make.

Node's http2 client reads SETTINGS_MAX_CONCURRENT_STREAMS and queues the overflow itself, so
launching 110 streams through it proves only that Node behaved. This client ignores the advertised
cap and puts 110 HEADERS frames on the wire at once, then decodes the server's own frames: the
answer is 100 responses, 10 RST_STREAM(REFUSED_STREAM) and no GOAWAY.

Not part of the cargo build (cargo reads only *.rs here). Requires `hyperframe` 5.2 and `hpack` 3.0.

Usage: python framework/tests/examples/h2_handframed_cap.py <path-to-h2_interop_server.exe> [port]
"""
import socket
import ssl
import subprocess
import sys
import threading
import time

from hpack import Decoder, Encoder
from hyperframe.frame import Frame, HeadersFrame, SettingsFrame

PREFACE = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
REFUSED_STREAM = 0x7
N_STREAMS = 110


def launch(exe, port, mode):
    proc = subprocess.Popen([exe, str(port), mode], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
    buf = b""
    deadline = time.time() + 40
    while time.time() < deadline:
        chunk = proc.stdout.readline()
        if not chunk:
            break
        buf += chunk
        if b"READY" in buf:
            return proc, buf
    raise RuntimeError("server never became ready: " + buf.decode(errors="replace"))


def main():
    exe = sys.argv[1]
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 18954
    proc, _ = launch(exe, port, "h2c")
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=20)
        sock.sendall(PREFACE)
        sock.sendall(SettingsFrame(stream_id=0).serialize())

        enc = Encoder()
        head = enc.encode([(b":method", b"GET"), (b":scheme", b"http"), (b":authority", b"localhost"),
                          (b":path", b"/slow"), (b"x-marker", b"cap")])
        for i in range(N_STREAMS):
            sid = 1 + 2 * i
            f = HeadersFrame(stream_id=sid, data=head, flags=['END_HEADERS', 'END_STREAM'])
            sock.sendall(f.serialize())

        decoder = Decoder()
        got = {"200": 0, "rst": {}, "other_status": {}, "goaway": 0, "frames": 0}
        opened = N_STREAMS
        stop = time.time() + 25
        sock.settimeout(2.0)
        while time.time() < stop:
            try:
                hdr = sock.recv(9)
            except socket.timeout:
                continue
            except OSError:
                break
            if len(hdr) < 9:
                break
            frame, length = Frame.parse_frame_header(hdr)
            payload = b""
            while len(payload) < length:
                chunk = sock.recv(length - len(payload))
                if not chunk:
                    break
                payload += chunk
            frame.parse_body(memoryview(payload))
            got["frames"] += 1
            t = type(frame).__name__
            if t == "HeadersFrame":
                status = str(dict(decoder.decode(frame.data)).get(":status", "?"))
                if status == "200":
                    got["200"] += 1
                else:
                    got["other_status"][status] = got["other_status"].get(status, 0) + 1
                opened -= 1
            elif t == "RstStreamFrame":
                code = int(frame.error_code)
                got["rst"][code] = got["rst"].get(code, 0) + 1
                opened -= 1
            elif t == "GoAwayFrame":
                got["goaway"] += 1
                break
            elif t == "SettingsFrame" and frame.stream_id == 0 and not frame.flags:
                # drain the body of any DATA frame the server still sends
                pass
            elif t == "DataFrame":
                pass
        # A fresh stream after the storm: the connection must still serve.
        after = None
        try:
            h2 = enc.encode([(b":method", b"GET"), (b":scheme", b"http"), (b":authority", b"localhost"),
                             (b":path", b"/ping")])
            sock.sendall(HeadersFrame(stream_id=1 + 2 * N_STREAMS, data=h2,
                                      flags=['END_HEADERS', 'END_STREAM']).serialize())
            body = ""
            deadline = time.time() + 10
            while time.time() < deadline:
                hdr = sock.recv(9)
                if len(hdr) < 9:
                    break
                frame, length = Frame.parse_frame_header(hdr)
                payload = b""
                while len(payload) < length:
                    chunk = sock.recv(length - len(payload))
                    if not chunk:
                        break
                    payload += chunk
                frame.parse_body(memoryview(payload))
                if type(frame).__name__ == "HeadersFrame":
                    body += "|" + str(dict(decoder.decode(frame.data)).get(":status", "?"))
                if type(frame).__name__ == "DataFrame":
                    body += bytes(frame.data).decode(errors="replace")
                    break
            after = body
        except OSError as exc:
            after = "socket error %s" % exc
        sock.close()

        refused = got["rst"].get(REFUSED_STREAM, 0)
        other_rst = {k: v for k, v in got["rst"].items() if k != REFUSED_STREAM}
        print("frames seen            : %d" % got["frames"])
        print("HEADERS :status 200    : %d" % got["200"])
        print("other statuses         : %s" % got["other_status"])
        print("RST_STREAM codes       : %s (7 = REFUSED_STREAM)" % got["rst"])
        print("GOAWAY frames          : %d" % got["goaway"])
        print("streams unresolved     : %d" % opened)
        print("post-storm /ping       : %s" % after)
        ok = (got["200"] == 100 and refused == 10 and got["goaway"] == 0
              and not other_rst and "|200" in (after or ""))
        print("%s  hand-framed 110 streams -> 100 answered, 10 REFUSED_STREAM, no GOAWAY, connection still serving"
              % ("PASS" if ok else "FAIL"))
        return 0 if ok else 1
    finally:
        proc.kill()


if __name__ == "__main__":
    sys.exit(main())
