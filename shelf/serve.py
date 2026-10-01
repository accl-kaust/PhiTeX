#!/usr/bin/env python3
"""Serve public/ on localhost:8124 as the host would (CORS open), for a
--dev build of the extension:  python3 serve.py"""
import functools, http.server

class H(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

http.server.ThreadingHTTPServer(("127.0.0.1", 8124), functools.partial(H, directory="public")).serve_forever()
