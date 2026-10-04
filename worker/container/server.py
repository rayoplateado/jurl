"""POST /run {"mode", "url", "q"} -> runs jurl and returns its JSON output.

jurl is always run with an argument list, never through a shell, so nothing
in the URL or the question can become a command. The Worker in front of this
already validated the request; the checks here are a second line of defence.
"""

import json
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TIMEOUT = 20
MAX_Q = 200

# Fixed result counts per mode; the client never chooses them.
MODES = {
    "gist": lambda q: ["-n", "5"] + ([f"--ask={q}"] if q else []),
    "ask": lambda q: ["-n", "3", f"--ask={q}"],
    "code": lambda q: ["--code", "-n", "3"] + ([f"--ask={q}"] if q else []),
    "links": lambda q: ["--links", "-n", "5"] + ([f"--ask={q}"] if q else []),
    "images": lambda q: ["--image", "-n", "5"] + ([f"--ask={q}"] if q else []),
    "find": lambda q: ["-n", "3", f"--find={q}"],
}
NEEDS_Q = {"ask", "find"}


def run(mode, url, q):
    if mode not in MODES:
        return 400, {"error": "unknown mode"}
    if not isinstance(url, str) or not url.startswith(("http://", "https://")) or len(url) > 2000:
        return 400, {"error": "bad url"}
    q = (q or "").strip() if isinstance(q, str) else ""
    if len(q) > MAX_Q or (mode in NEEDS_Q and not q):
        return 400, {"error": "bad question"}
    argv = ["jurl", "--json", "-t", *MODES[mode](q), "--", url]
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=TIMEOUT, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return 200, {"code": -1, "stdout": "", "stderr": "jurl: timed out"}
    return 200, {"code": p.returncode, "stdout": p.stdout[:200_000], "stderr": p.stderr[-4000:]}


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != "/run":
            return self.reply(404, {"error": "not found"})
        try:
            body = json.loads(self.rfile.read(min(int(self.headers.get("Content-Length", 0)), 10_000)))
            status, out = run(body.get("mode"), body.get("url"), body.get("q"))
        except (ValueError, AttributeError):
            status, out = 400, {"error": "bad request"}
        self.reply(status, out)

    def do_GET(self):
        self.reply(200, {"ok": True})

    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
