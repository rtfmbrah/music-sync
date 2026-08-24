#!/usr/bin/env python3
"""Serve one deterministic MusicBrainz recording lookup on loopback."""

import http.server
import pathlib
import socketserver
import sys


RECORDING_ID = "f59c5520-5f46-4d2c-b2c4-822eabf53419"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if not self.path.startswith(f"/ws/2/recording/{RECORDING_ID}?"):
            self.send_error(404, "Unexpected fixture path")
            return
        if "music-sync-lxc-test/" not in self.headers.get("User-Agent", ""):
            self.send_error(400, "Missing fixture User-Agent")
            return
        body = pathlib.Path(__file__).with_name("recording.json").read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


with socketserver.TCPServer(("127.0.0.1", int(sys.argv[1])), Handler) as server:
    server.handle_request()
