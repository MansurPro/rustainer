#!/usr/bin/env python3
"""A stand-in for api.osv.dev, for the scan test.

    fake-osv.py <log-file> <port>

Says `busybox` has one vulnerability and nothing else has any, and writes
every request body to the log so the test can assert what was SENT - the
property that matters about a scanner is what it tells a third party.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG, PORT = sys.argv[1], int(sys.argv[2])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, doc):
        body = json.dumps(doc).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        with open(LOG, "ab") as f:
            f.write(raw + b"\n")
        queries = json.loads(raw)["queries"]
        self.reply({"results": [
            {"vulns": [{"id": "FAKE-2026-0001"}]} if q["package"]["name"] == "busybox" else {}
            for q in queries
        ]})

    def do_GET(self):
        eco = "Alpine:v3.21"
        self.reply({
            "id": "FAKE-2026-0001",
            "summary": "a made-up flaw in busybox",
            "upstream": ["CVE-2026-0001"],
            "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
            "affected": [
                {"package": {"name": "busybox", "ecosystem": "Alpine:v3.14"},
                 "ranges": [{"events": [{"introduced": "0"}, {"fixed": "wrong-release"}]}]},
                {"package": {"name": "busybox", "ecosystem": eco},
                 "ranges": [{"events": [{"introduced": "0"}, {"fixed": "9.9.9-r0"}]}]},
            ],
        })


server = HTTPServer(("127.0.0.1", PORT), Handler)
print(f"ready {PORT}", flush=True)
server.serve_forever()
