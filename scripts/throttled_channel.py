#!/usr/bin/env python3
"""Mock HyprFetch update channel with THROTTLED asset serving.

Reproduces the owner's v0.4.6 bug: `hyprfetch update` dies after the
client's 60s total HTTP timeout with
    Error: update channel: asset read: error decoding response body
whenever the network is slow/stalled — exactly his 1m4s run.

Serves:
  /latest.json          -> manifest pointing at /<version>/<asset-name>
  /<version>/<asset>    -> real tarball bytes, throttled (default 40 KB/s)
Supports Range requests so the FIXED downloader's resume can be tested.

Usage: throttled_channel.py <port> <tarball-file> [bytes_per_second] [version]
"""
import json
import os
import re
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TARBALL = sys.argv[2]
SPEED = float(sys.argv[3]) if len(sys.argv) > 3 else 40 * 1024
VERSION = sys.argv[4] if len(sys.argv) > 4 else "0.5.0-test"
PORT = int(sys.argv[1])
SIZE = os.path.getsize(TARBALL)
# Real sha256 of the payload so the manifest is consistent.
import hashlib
SHA = hashlib.sha256(open(TARBALL, "rb").read()).hexdigest()

MANIFEST = {
    "version": VERSION,
    "tag": f"v{VERSION}",
    "published_at": "2026-09-30T00:00:00Z",
    "notes_url": "https://istias.tech/hyprfetch/updates",
    "assets": {
        "x86_64-unknown-linux-gnu": {
            "url": f"http://127.0.0.1:{PORT}/{VERSION}/hyprfetch-{VERSION}-linux-x64.tar.gz",
            "sha256": SHA,
            "size": SIZE,
        }
    },
}


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _asset(self):
        start = 0
        rng = self.headers.get("Range")
        if rng:
            m = re.match(r"bytes=(\d+)-", rng)
            if m:
                start = int(m.group(1))
        end = SIZE  # exclusive
        status = 200 if start == 0 else 206
        if start >= SIZE:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{SIZE}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(status)
        self.send_header("Content-Length", str(end - start))
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end-1}/{SIZE}")
        self.send_header("Content-Type", "application/gzip")
        self.end_headers()
        with open(TARBALL, "rb") as f:
            f.seek(start)
            sent = start
            t0 = time.time()
            while sent < end:
                chunk = f.read(8192)
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return
                sent += len(chunk)
                # throttle
                expect = (sent - start) / SPEED
                delta = expect - (time.time() - t0)
                if delta > 0:
                    time.sleep(delta)

    def do_GET(self):
        if self.path.endswith("latest.json"):
            body = json.dumps(MANIFEST).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)
        else:
            self._asset()


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
