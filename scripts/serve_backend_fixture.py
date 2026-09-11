"""Serve a curated test rootfs as one immutable image on loopback only.

Linux backend integration tests use this small registry fixture to exercise real
Docker without depending on Docker Hub or changing the daemon's mirror config.
Not a Hub implementation or an image publisher.
"""
import argparse
import hashlib
import http.server
import io
import json
from pathlib import Path
import tarfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rootfs", type=Path, required=True)
    parser.add_argument("--ready-file", type=Path, required=True)
    args = parser.parse_args()
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        for path in sorted(args.rootfs.iterdir()):
            tar.add(path, arcname=path.name)
    layer = buffer.getvalue()

    def digest(data):
        return "sha256:" + hashlib.sha256(data).hexdigest()

    def encode(data):
        return json.dumps(data, separators=(",", ":")).encode()

    config = encode({"architecture": "amd64", "os": "linux", "config": {},
                     "rootfs": {"type": "layers", "diff_ids": [digest(layer)]}})
    manifest_type = "application/vnd.docker.distribution.manifest.v2+json"
    manifest = encode({"schemaVersion": 2, "mediaType": manifest_type,
        "config": {"mediaType": "application/vnd.docker.container.image.v1+json",
                   "digest": digest(config), "size": len(config)},
        "layers": [{"mediaType": "application/vnd.docker.image.rootfs.diff.tar",
                    "digest": digest(layer), "size": len(layer)}]})
    routes = {"/v2/": (b"{}", "application/json"),
              "/v2/uenv-test/manifests/" + digest(manifest): (manifest, manifest_type),
              "/v2/uenv-test/blobs/" + digest(config): (config, "application/octet-stream"),
              "/v2/uenv-test/blobs/" + digest(layer): (layer, "application/octet-stream")}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_HEAD(self):
            self.reply(False)

        def do_GET(self):
            self.reply(True)

        def reply(self, body):
            if self.path not in routes:
                self.send_error(404)
                return
            data, media_type = routes[self.path]
            self.send_response(200)
            self.send_header("Content-Type", media_type)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Docker-Content-Digest", digest(data))
            self.end_headers()
            if body:
                self.wfile.write(data)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    args.ready_file.write_text(json.dumps({"image": f"127.0.0.1:{server.server_port}/uenv-test@{digest(manifest)}"}), encoding="utf-8")
    server.serve_forever()


if __name__ == "__main__":
    main()
