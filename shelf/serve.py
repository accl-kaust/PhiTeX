#!/usr/bin/env python3
"""Serve a Shelf locally, CORS open, as the host would, for a --dev build
of the extension (SHELF_LOCAL=1):

    python3 serve.py [--root DIR] [--port N]     # default public/, 8124

With --root releases-s3, http://127.0.0.1:8124/tl2026/release.json is that
release's, its packs under /tl2026/h/."""
import functools, http.server, sys

args = sys.argv[1:]
opt = lambda k, d: args[args.index(k) + 1] if k in args else d
root, port = opt("--root", "public"), int(opt("--port", "8124"))


class H(http.server.SimpleHTTPRequestHandler):
    # (as the host serves: HTTP/1.1, the connection kept; the default,
    # HTTP/1.0, closes each one after its answer)
    protocol_version = "HTTP/1.1"

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()


class S(http.server.ThreadingHTTPServer):
    # (a client fetches eight packs at once: the default backlog of 5
    # dropped connections, the Shelf release's check, run 41296048903)
    request_queue_size = 128


print(f"Shelf: {root} on http://127.0.0.1:{port}/", flush=True)
S(("127.0.0.1", port), functools.partial(H, directory=root)).serve_forever()
