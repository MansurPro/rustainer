#!/usr/bin/env python3
"""A stand-in for hub.docker.com, for the signed-in repository list.

    fake-hub.py <port>

Anonymous callers see one public repository. `tester:secret` signs in
and sees a private one too, on a second page - so the test fails unless
the client both sent the token and followed `next`.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, doc):
        body = json.dumps(doc).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        doc = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        if (doc.get("username"), doc.get("password")) == ("tester", "secret"):
            return self.reply(200, {"token": "jwt-for-tester"})
        self.reply(401, {"detail": "Incorrect authentication credentials"})

    def do_GET(self):
        signed_in = self.headers.get("Authorization") == "Bearer jwt-for-tester"
        if "page=2" in self.path:
            if not signed_in:
                return self.reply(400, {"message": "pagination offset too large for anonymous requests"})
            return self.reply(200, {"next": None, "results": [
                {"name": "hidden", "pull_count": 3, "last_updated": "2026-02-02T00:00:00Z", "is_private": True}]})
        self.reply(200, {
            "next": f"http://127.0.0.1:{PORT}/v2/repositories/tester/?page=2",
            "results": [
                {"name": "open", "pull_count": 9, "last_updated": "2026-01-01T00:00:00Z", "is_private": False}]})


server = HTTPServer(("127.0.0.1", PORT), Handler)
print(f"ready {PORT}", flush=True)
server.serve_forever()
