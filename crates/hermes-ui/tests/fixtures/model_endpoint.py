"""Loopback-only model metadata fixture. NEVER implements inference.

Use its printed port as a private custom provider's base_url. GET /v1/models
is real HTTP metadata; any POST is rejected and counted at GET /stats.
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

COUNTS = {"metadata": 0, "inference": 0}
LOCK = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def send_json(self, value, status=200):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.rstrip("/") in ("/models", "/v1/models"):
            with LOCK:
                COUNTS["metadata"] += 1
            self.send_json({"object": "list", "data": [
                {"id": "fixture-a", "object": "model", "owned_by": "fixture"},
                {"id": "fixture-b", "object": "model", "owned_by": "fixture"},
            ]})
        elif self.path == "/stats":
            with LOCK:
                value = dict(COUNTS)
            self.send_json(value)
        else:
            self.send_json({"error": "Fixture route not found"}, 404)

    def do_POST(self):
        with LOCK:
            COUNTS["inference"] += 1
        self.send_json({"error": "Inference is forbidden by this fixture"}, 405)


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"MODEL_METADATA_READY port={server.server_port}", flush=True)
    server.serve_forever()
