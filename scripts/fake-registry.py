#!/usr/bin/env python3
"""A registry serving one hand-built image, for tests that need an image
whose exact layer contents are known.

    python3 scripts/fake-registry.py <workdir> <port>
    # prints "ready <reference>" on stdout when it is serving

Why this exists. The properties worth testing hardest about layer handling
are the ones about what a layer *removes* - a whiteout, an opaque
directory - and no public image is a good test for those, because you
cannot see what its layers contain without reimplementing the thing under
test. So the image is built here, byte by byte, and served over loopback,
where the engine's own `base_url` already allows plain HTTP.

It implements exactly the three endpoints a pull touches, and nothing else:
the API version check, one manifest, and the blobs. There is no auth, which
is itself part of the test - a client that assumes a token dance is required
would fail here, and real registries differ on this.
"""

import gzip
import hashlib
import http.server
import io
import json
import os
import sys
import tarfile
import threading

WORK = sys.argv[1]
PORT = int(sys.argv[2])
# An optional base layer: a real root filesystem tarball, so the image has
# a shell to run the assertions with. Without one the image is only the
# test files, and nothing in it can be executed.
BASE_TGZ = sys.argv[3] if len(sys.argv) > 3 else None
NAME = "test/whiteout"
TAG = "v1"

os.makedirs(WORK, exist_ok=True)
blobs = {}


def blob(data: bytes) -> str:
    d = "sha256:" + hashlib.sha256(data).hexdigest()
    blobs[d] = data
    return d


def layer(entries) -> tuple:
    """A gzipped tar layer. Returns (compressed digest, diff_id, size).

    `entries` is a list of (name, kind, content):
      kind "f"   regular file
      kind "d"   directory
      kind "wh"  an OCI whiteout marker - written as a REGULAR FILE named
                 `.wh.<name>`, which is how the image spec expresses a
                 deletion. Turning it into something overlayfs understands
                 is the engine's job, and the point of this image.
    """
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as tf:
        for name, kind, content in entries:
            if kind == "d":
                ti = tarfile.TarInfo(name)
                ti.type = tarfile.DIRTYPE
                ti.mode = 0o755
                tf.addfile(ti)
                continue
            data = content.encode() if kind == "f" else b""
            if kind == "wh":
                parent, _, base = name.rpartition("/")
                name = (parent + "/" if parent else "") + ".wh." + base
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = 0o644
            tf.addfile(ti, io.BytesIO(data))
    tar_bytes = raw.getvalue()
    diff_id = "sha256:" + hashlib.sha256(tar_bytes).hexdigest()
    gz = gzip.compress(tar_bytes, mtime=0)
    return blob(gz), diff_id, len(gz)


def passthrough(path: str) -> tuple:
    """Serve an existing .tar.gz unchanged. Returns (digest, diff_id, size).

    The compressed bytes go out as-is so the compressed digest the manifest
    advertises is the digest of what the engine actually receives - which
    is the thing the engine checks.
    """
    gz = open(path, "rb").read()
    diff = "sha256:" + hashlib.sha256(gzip.decompress(gz)).hexdigest()
    return blob(gz), diff, len(gz)


# Layer 1 creates files; layer 2 deletes two of them and marks a directory
# opaque. A correct engine shows `keep` and `new` and nothing else.
l1_digest, l1_diff, l1_size = layer(
    [
        ("secret", "f", "SECRET-CONTENT"),
        ("d", "d", ""),
        ("d/keep", "f", "keep"),
        ("d/drop", "f", "drop"),
        ("opq", "d", ""),
        ("opq/old", "f", "old"),
    ]
)
# Layer 2 is built by hand rather than through the helper: the opaque
# marker has to sit INSIDE the directory it applies to, which is a shape
# the helper's (name, kind) pairs cannot express.
raw = io.BytesIO()
with tarfile.open(fileobj=raw, mode="w") as tf:
    for name, data, mode in [
        ("secret", b"", 0o644),
        ("d/drop", b"", 0o644),
    ]:
        parent, _, base = name.rpartition("/")
        ti = tarfile.TarInfo((parent + "/" if parent else "") + ".wh." + base)
        ti.size = 0
        ti.mode = mode
        tf.addfile(ti, io.BytesIO(b""))
    ti = tarfile.TarInfo("opq")
    ti.type = tarfile.DIRTYPE
    ti.mode = 0o755
    tf.addfile(ti)
    ti = tarfile.TarInfo("opq/.wh..wh..opq")
    ti.size = 0
    ti.mode = 0o644
    tf.addfile(ti, io.BytesIO(b""))
    ti = tarfile.TarInfo("opq/new")
    data = b"new"
    ti.size = len(data)
    ti.mode = 0o644
    tf.addfile(ti, io.BytesIO(data))
tar_bytes = raw.getvalue()
l2_diff = "sha256:" + hashlib.sha256(tar_bytes).hexdigest()
gz = gzip.compress(tar_bytes, mtime=0)
l2_digest = blob(gz)
l2_size = len(gz)

layers = []
diff_ids = []
if BASE_TGZ:
    b_digest, b_diff, b_size = passthrough(BASE_TGZ)
    layers.append((b_digest, b_size))
    diff_ids.append(b_diff)
layers.append((l1_digest, l1_size))
diff_ids.append(l1_diff)
layers.append((l2_digest, l2_size))
diff_ids.append(l2_diff)

config = {
    "architecture": os.environ.get("FAKE_ARCH", "amd64"),
    "os": "linux",
    "config": {"Cmd": ["/bin/sh"], "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"]},
    "rootfs": {"type": "layers", "diff_ids": diff_ids},
}
config_bytes = json.dumps(config).encode()
config_digest = blob(config_bytes)

manifest = {
    "schemaVersion": 2,
    "mediaType": "application/vnd.oci.image.manifest.v1+json",
    "config": {
        "mediaType": "application/vnd.oci.image.config.v1+json",
        "digest": config_digest,
        "size": len(config_bytes),
    },
    "layers": [
        {
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": d,
            "size": n,
        }
        for d, n in layers
    ],
}
manifest_bytes = json.dumps(manifest).encode()
manifest_digest = "sha256:" + hashlib.sha256(manifest_bytes).hexdigest()


# Anything pushed to us, kept in memory. A push followed by a pull of
# the same reference is the only way to test that what we uploaded is
# what a registry would hand back - and doing it against a registry we
# wrote ourselves is the only way to do it without a network.
pushed_blobs = {}
pushed_manifests = {}
uploads = {}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, body: bytes, ctype: str, extra=None):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_HEAD(self):
        self.do_GET()

    def _read_body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def do_POST(self):
        # Start a blob upload. The Location carries a session id, and a
        # client that rebuilds the URL instead of using what it was given
        # loses it - which is why we put one here.
        if self.path.endswith("/blobs/uploads/"):
            name = self.path[len("/v2/"):-len("/blobs/uploads/")]
            uid = hashlib.sha256(os.urandom(16)).hexdigest()[:16]
            uploads[uid] = name
            self.send_response(202)
            self.send_header("Location", f"/v2/{name}/blobs/uploads/{uid}?state=abc")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_error(404)

    def do_PUT(self):
        path = self.path.split("?")[0]
        query = self.path.split("?")[1] if "?" in self.path else ""
        if "/blobs/uploads/" in path:
            digest = None
            for kv in query.split("&"):
                if kv.startswith("digest="):
                    digest = kv[len("digest="):]
            body = self._read_body()
            if not digest:
                return self.send_error(400, "no digest")
            actual = "sha256:" + hashlib.sha256(body).hexdigest()
            if actual != digest:
                # A registry that accepted a mismatch would hide exactly
                # the bug this test exists to catch.
                return self.send_error(400, "digest mismatch")
            pushed_blobs[digest] = body
            self.send_response(201)
            self.send_header("Docker-Content-Digest", digest)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if "/manifests/" in path:
            body = self._read_body()
            ref = path.split("/manifests/")[1]
            name = path[len("/v2/"):].split("/manifests/")[0]
            # Blobs first, manifest last: refuse a manifest naming blobs
            # we do not have, which is what a strict registry does and
            # what catches a client that uploads in the wrong order.
            try:
                doc = json.loads(body)
                needed = [doc["config"]["digest"]] + [l["digest"] for l in doc.get("layers", [])]
            except Exception:
                return self.send_error(400, "unparseable manifest")
            missing = [d for d in needed if d not in pushed_blobs and d not in blobs]
            if missing:
                return self.send_error(400, f"manifest references missing blobs: {missing[0]}")
            d = "sha256:" + hashlib.sha256(body).hexdigest()
            pushed_manifests[(name, ref)] = (body, d, self.headers.get("Content-Type", ""))
            pushed_manifests[(name, d)] = (body, d, self.headers.get("Content-Type", ""))
            self.send_response(201)
            self.send_header("Docker-Content-Digest", d)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_error(404)

    def do_GET(self):
        p = self.path
        if p == "/v2/" or p == "/v2":
            return self._send(b"{}", "application/json")
        # Anything that was pushed is served back, so a push can be
        # followed by a pull of the same reference.
        if "/manifests/" in p:
            name = p[len("/v2/"):].split("/manifests/")[0]
            ref = p.split("/manifests/")[1]
            if (name, ref) in pushed_manifests:
                body, d, ctype = pushed_manifests[(name, ref)]
                return self._send(
                    body,
                    ctype or "application/vnd.oci.image.manifest.v1+json",
                    {"Docker-Content-Digest": d},
                )
        if "/blobs/" in p:
            d = p.split("/blobs/")[1]
            if d in pushed_blobs:
                return self._send(pushed_blobs[d], "application/octet-stream")
        prefix = f"/v2/{NAME}/"
        if p.startswith(prefix + "manifests/"):
            ref = p[len(prefix + "manifests/"):]
            if ref in (TAG, manifest_digest):
                return self._send(
                    manifest_bytes,
                    manifest["mediaType"],
                    {"Docker-Content-Digest": manifest_digest},
                )
        if p.startswith(prefix + "blobs/"):
            d = p[len(prefix + "blobs/"):]
            if d in blobs:
                return self._send(blobs[d], "application/octet-stream")
        self.send_error(404)


srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
threading.Thread(target=srv.serve_forever, daemon=True).start()
print(f"ready 127.0.0.1:{PORT}/{NAME}:{TAG}", flush=True)
try:
    threading.Event().wait()
except KeyboardInterrupt:
    pass
