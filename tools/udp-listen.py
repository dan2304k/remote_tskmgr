#!/usr/bin/env python3
"""Print every RTM packet heard on UDP (and optionally TCP) port 48000 (stdlib only).

    python3 tools/udp-listen.py               # one summary line per packet
    python3 tools/udp-listen.py --raw         # print the JSON
    python3 tools/udp-listen.py --tcp         # also accept TCP line-protocol senders on the same port
    python3 tools/udp-listen.py --secret 'correct horse'   # verify `auth`
"""

import argparse
import hashlib
import hmac
import json
import selectors
import socket
import sys
import time


def canonical(packet):
    body = {k: v for k, v in packet.items() if k != "auth"}
    return json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def fmt_metric(metrics, key):
    m = metrics.get(key)
    if not isinstance(m, dict):
        return "-"
    v = m.get("v")
    if v is None:
        return "null"
    unit = m.get("unit", "")
    return f"{v:.1f}{unit}" if isinstance(v, (int, float)) else str(v)


def handle(raw, src, proto, args):
    stamp = time.strftime("%H:%M:%S") + f".{int(time.time() * 1000) % 1000:03d}"
    try:
        text = raw.decode("utf-8")
        p = json.loads(text)
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        print(f"{stamp} {proto} {src[0]}:{src[1]} INVALID ({e}): {raw[:80]!r}", flush=True)
        return
    auth = ""
    if args.secret:
        want = hmac.new(args.secret.encode(), canonical(p), hashlib.sha256).hexdigest()
        auth = " auth=ok" if p.get("auth") == want else " auth=BAD"
    if args.raw:
        print(f"{stamp} {proto} {src[0]}:{src[1]}{auth} {text.rstrip()}", flush=True)
        return
    if not isinstance(p, dict):
        print(f"{stamp} {proto} {src[0]}:{src[1]} not an object", flush=True)
        return
    metrics = p.get("metrics") if isinstance(p.get("metrics"), dict) else {}
    extra = [k for k in metrics if k not in ("cpu", "gpu", "mem", "disk")]
    print(f"{stamp} {proto} {src[0]}:{src[1]:<5} {p.get('host', '?'):<14} seq={p.get('seq', '?'):<6} "
          f"cpu={fmt_metric(metrics, 'cpu'):<7} gpu={fmt_metric(metrics, 'gpu'):<7} "
          f"mem={fmt_metric(metrics, 'mem'):<7} disk={fmt_metric(metrics, 'disk'):<10}"
          + (f" +{','.join(extra)}" if extra else "") + auth
          + ("" if p.get("v") == 1 else f" (v={p.get('v')!r}: incompatible)"), flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=48000)
    ap.add_argument("--bind", default="0.0.0.0")
    ap.add_argument("--raw", action="store_true", help="print raw JSON instead of a summary")
    ap.add_argument("--tcp", action="store_true", help="also listen on TCP (line protocol) on the same port")
    ap.add_argument("--secret", default="", help="verify auth with this shared secret")
    ap.add_argument("--count", type=int, default=0, help="exit after this many packets (0 = forever)")
    ap.add_argument("--timeout", type=float, default=0, help="exit after this many seconds (0 = forever)")
    args = ap.parse_args()

    sel = selectors.DefaultSelector()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, "SO_REUSEPORT"):
        try:
            udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except OSError:
            pass
    udp.bind((args.bind, args.port))
    sel.register(udp, selectors.EVENT_READ, "udp")
    srv = None
    bufs = {}
    if args.tcp:
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((args.bind, args.port))
        srv.listen(16)
        sel.register(srv, selectors.EVENT_READ, "accept")
    print(f"listening on udp {args.bind}:{args.port}" + (" and tcp" if args.tcp else ""), file=sys.stderr)

    seen = 0
    deadline = time.monotonic() + args.timeout if args.timeout else None
    try:
        while True:
            wait = None if deadline is None else max(0.0, deadline - time.monotonic())
            if deadline is not None and wait == 0:
                break
            for key, _ in sel.select(wait):
                kind = key.data
                if kind == "udp":
                    data, src = udp.recvfrom(65535)
                    handle(data, src, "udp", args)
                    seen += 1
                elif kind == "accept":
                    conn, src = srv.accept()
                    print(f"tcp: connection from {src[0]}:{src[1]}", file=sys.stderr)
                    bufs[conn] = (src, b"")
                    sel.register(conn, selectors.EVENT_READ, "conn")
                else:
                    conn = key.fileobj
                    src, buf = bufs[conn]
                    try:
                        chunk = conn.recv(65536)
                    except OSError:
                        chunk = b""
                    if not chunk:
                        print(f"tcp: {src[0]}:{src[1]} closed", file=sys.stderr)
                        sel.unregister(conn)
                        conn.close()
                        del bufs[conn]
                        continue
                    buf += chunk
                    *lines, buf = buf.split(b"\n")
                    bufs[conn] = (src, buf)
                    for line in lines:
                        if line.strip():
                            handle(line, src, "tcp", args)
                            seen += 1
                if args.count and seen >= args.count:
                    return
    except KeyboardInterrupt:
        pass
    finally:
        for c in list(bufs):
            c.close()
        if srv:
            srv.close()
        udp.close()


if __name__ == "__main__":
    main()
