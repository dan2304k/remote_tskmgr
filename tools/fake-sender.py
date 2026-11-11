#!/usr/bin/env python3
"""Emit synthetic RTM protocol-v1 packets for several fake hosts (stdlib only).

Default: 3 hosts, UDP broadcast to port 48000 every second.

    python3 tools/fake-sender.py                       # broadcast :48000
    python3 tools/fake-sender.py --udp 192.168.1.50:48000 --tcp 192.168.1.50:48000
    python3 tools/fake-sender.py --no-broadcast --udp 127.0.0.1:48000 --secret 'correct horse'

Field names, labels and units follow protocol/examples/valid_full.json. Host "render-box"
also carries a custom `fan_rpm` metric to exercise the viewer's auto-slot behaviour.
"""

import argparse
import hashlib
import hmac
import json
import random
import socket
import sys
import time
import uuid

BCAST = "255.255.255.255"


def canonical(packet):
    body = {k: v for k, v in packet.items() if k != "auth"}
    return json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sign(packet, secret):
    packet["auth"] = hmac.new(secret.encode("utf-8"), canonical(packet), hashlib.sha256).hexdigest()


class Walk:
    """Bounded random walk."""

    def __init__(self, lo, hi, start=None, step=None):
        self.lo, self.hi = lo, hi
        self.x = start if start is not None else random.uniform(lo, hi)
        self.step = step if step is not None else (hi - lo) * 0.06

    def next(self):
        self.x = min(self.hi, max(self.lo, self.x + random.gauss(0, self.step)))
        return self.x


def r1(x):
    return round(x, 1)


class FakeHost:
    def __init__(self, name, os_name, cores, gpu, mem_total_mb, extra_fan=False, seed_id=None):
        self.id = seed_id or str(uuid.uuid4())
        self.name = name
        self.os = os_name
        self.gpu = gpu  # None | dict(name=, vram_total_mb=, backend=, null=bool)
        self.cores = [Walk(0, 100) for _ in range(cores)]
        self.gpu_util = Walk(0, 100)
        self.gpu_temp = Walk(35, 85)
        self.mem = Walk(20, 90)
        self.mem_total = mem_total_mb
        self.rd = Walk(0, 80, start=2, step=5)
        self.wr = Walk(0, 120, start=5, step=8)
        self.rx = Walk(0, 5, start=0.5, step=0.3)
        self.tx = Walk(0, 2, start=0.1, step=0.1)
        self.cpu_temp = Walk(35, 90)
        self.fan = Walk(600, 2400, step=80) if extra_fan else None
        self.seq = 0

    def packet(self, interval_ms):
        self.seq += 1
        cores = [r1(c.next()) for c in self.cores]
        cpu = r1(sum(cores) / len(cores))
        cpu_temp = round(self.cpu_temp.next())
        mem_pct = self.mem.next()
        rd, wr = self.rd.next(), self.wr.next()
        rx, tx = self.rx.next(), self.tx.next()
        metrics = {
            "cpu": {"v": cpu, "unit": "%", "label": "CPU", "min": 0, "max": 100,
                    "detail": {"cores": cores, "freq_mhz": random.choice([3400, 3600, 3800, 4200]),
                               "temp_c": cpu_temp}},
        }
        if self.gpu is not None:
            g = self.gpu
            if g.get("null"):
                metrics["gpu"] = {"v": None, "unit": "%", "label": "GPU", "min": 0, "max": 100,
                                  "detail": {"name": g["name"], "backend": g["backend"], "approx": True,
                                             "gt_cur_freq_mhz": random.choice([300, 650, 1100, 1300]),
                                             "gt_max_freq_mhz": 1300}}
            else:
                util = r1(self.gpu_util.next())
                metrics["gpu"] = {"v": util, "unit": "%", "label": "GPU", "min": 0, "max": 100,
                                  "detail": {"name": g["name"],
                                             "vram_used_mb": int(g["vram_total_mb"] * (0.2 + util / 160)),
                                             "vram_total_mb": g["vram_total_mb"],
                                             "temp_c": round(self.gpu_temp.next()),
                                             "backend": g["backend"]}}
        metrics["mem"] = {"v": r1(mem_pct), "unit": "%", "label": "MEM", "min": 0, "max": 100,
                          "detail": {"used_mb": int(self.mem_total * mem_pct / 100), "total_mb": self.mem_total,
                                     "swap_used_mb": 0, "swap_total_mb": 2048}}
        metrics["disk"] = {"v": r1(rd + wr), "unit": "MB/s", "label": "DISK", "min": 0, "max": None,
                           "detail": {"read_mbps": r1(rd), "write_mbps": r1(wr)}}
        metrics["net"] = {"v": round(rx + tx, 2), "unit": "MB/s", "label": "NET", "min": 0, "max": None,
                          "detail": {"rx_mbps": round(rx, 2), "tx_mbps": round(tx, 2)}}
        metrics["temp"] = {"v": float(cpu_temp), "unit": "°C", "label": "TEMP", "min": 0, "max": 100}
        if self.fan is not None:
            metrics["fan_rpm"] = {"v": round(self.fan.next()), "unit": "rpm", "label": "FAN", "min": 0,
                                  "max": 3000}
        return {
            "v": 1,
            "id": self.id,
            "host": self.name,
            "os": self.os,
            "ts": int(time.time() * 1000),
            "seq": self.seq,
            "interval_ms": interval_ms,
            "metrics": metrics,
        }


def default_hosts():
    return [
        FakeHost("workstation", "linux", 8,
                 {"name": "NVIDIA GeForce RTX 3070", "vram_total_mb": 8192, "backend": "nvml"}, 32768,
                 seed_id="fake-0001-linux-nvidia"),
        FakeHost("gaming-pc", "windows", 12,
                 {"name": "AMD Radeon RX 6800", "vram_total_mb": 16384, "backend": "pdh"}, 65536,
                 extra_fan=True, seed_id="fake-0002-windows-amd"),
        FakeHost("laptop", "linux", 4,
                 {"name": "Intel Iris Xe", "backend": "intel-sysfs", "null": True}, 16384,
                 seed_id="fake-0003-linux-intel"),
    ]


def make_hosts(n):
    base = default_hosts()
    hosts = base[:n]
    for i in range(len(base), n):
        hosts.append(FakeHost(f"fake-{i + 1}", random.choice(["linux", "windows"]), 4, None, 8192,
                              seed_id=f"fake-{i + 1:04d}"))
    return hosts


def parse_hostport(s):
    host, _, port = s.rpartition(":")
    if not host or not port.isdigit():
        raise argparse.ArgumentTypeError(f"expected HOST:PORT, got {s!r}")
    return host, int(port)


class TcpLine:
    """Reconnecting newline-delimited JSON writer (non-blocking-ish: short timeouts)."""

    def __init__(self, addr):
        self.addr = addr
        self.sock = None
        self.backoff = 0.25
        self.next_try = 0.0
        self.failures = 0

    def send(self, data):
        now = time.monotonic()
        if self.sock is None:
            if now < self.next_try:
                return
            try:
                self.sock = socket.create_connection(self.addr, timeout=2)
                self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                print(f"tcp: connected to {self.addr[0]}:{self.addr[1]}", file=sys.stderr)
                self.backoff, self.failures = 0.25, 0
            except OSError as e:
                self.failures += 1
                if self.failures == 1:
                    print(f"tcp: connect {self.addr[0]}:{self.addr[1]} failed: {e}; retrying", file=sys.stderr)
                self.next_try = now + self.backoff
                self.backoff = min(self.backoff * 2, 10.0)
                return
        try:
            self.sock.sendall(data)
        except OSError as e:
            print(f"tcp: disconnected ({e}); reconnecting", file=sys.stderr)
            self.close()
            self.next_try = now

    def close(self):
        if self.sock is not None:
            try:
                self.sock.close()
            except OSError:
                pass
            self.sock = None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--hosts", type=int, default=3, help="number of fake hosts (default 3)")
    ap.add_argument("--interval", type=float, default=1.0, help="seconds between packets (default 1)")
    ap.add_argument("--port", type=int, default=48000, help="UDP broadcast port (default 48000)")
    ap.add_argument("--no-broadcast", action="store_true", help="disable UDP broadcast")
    ap.add_argument("--bcast-addr", default=BCAST, help="broadcast address (default 255.255.255.255)")
    ap.add_argument("--udp", type=parse_hostport, action="append", default=[], metavar="HOST:PORT",
                    help="also send UDP unicast here (repeatable)")
    ap.add_argument("--tcp", type=parse_hostport, action="append", default=[], metavar="HOST:PORT",
                    help="also stream line-delimited JSON over TCP here, reconnecting (repeatable)")
    ap.add_argument("--secret", default="", help="shared secret; adds `auth` HMAC-SHA256 per PROTOCOL.md")
    ap.add_argument("--count", type=int, default=0, help="stop after this many rounds (0 = forever)")
    ap.add_argument("-q", "--quiet", action="store_true", help="do not print a line per round")
    args = ap.parse_args()
    if args.hosts < 1:
        ap.error("--hosts must be >= 1")

    hosts = make_hosts(args.hosts)
    interval_ms = int(round(args.interval * 1000))
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    tcps = [TcpLine(a) for a in args.tcp]
    udp_dests = ([] if args.no_broadcast else [(args.bcast_addr, args.port)]) + list(args.udp)
    if not udp_dests and not tcps:
        ap.error("no destinations (broadcast disabled and no --udp/--tcp)")

    print(f"fake-sender: {len(hosts)} host(s) every {args.interval}s -> "
          + ", ".join([f"udp {h}:{p}" for h, p in udp_dests] + [f"tcp {h}:{p}" for h, p in args.tcp])
          + (" (signed)" if args.secret else ""), file=sys.stderr)
    errors_logged = {}
    rounds = 0
    next_tick = time.monotonic()
    try:
        while True:
            for h in hosts:
                p = h.packet(interval_ms)
                if args.secret:
                    sign(p, args.secret)
                data = json.dumps(p, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
                if len(data) > 1400:
                    print(f"warning: {h.name} packet is {len(data)} bytes (> 1400)", file=sys.stderr)
                for d in udp_dests:
                    try:
                        udp.sendto(data, d)
                    except OSError as e:
                        last = errors_logged.get(d, 0)
                        if time.monotonic() - last > 30:
                            errors_logged[d] = time.monotonic()
                            print(f"udp {d[0]}:{d[1]}: {e}", file=sys.stderr)
                for t in tcps:
                    t.send(data + b"\n")
            rounds += 1
            if not args.quiet:
                print(f"round {rounds}: sent {len(hosts)} packet(s)", file=sys.stderr)
            if args.count and rounds >= args.count:
                break
            next_tick += args.interval
            time.sleep(max(0.0, next_tick - time.monotonic()))
    except KeyboardInterrupt:
        pass
    finally:
        for t in tcps:
            t.close()
        udp.close()


if __name__ == "__main__":
    main()
