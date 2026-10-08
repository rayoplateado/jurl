# A recording proxy in front of a Jev-contract endpoint: jurl talks to it (JURL_JEV_URL), it forwards every POST to
# --upstream and appends each request and response body, with the upstream's latency, as one JSON line to --out.
# Headers are forwarded (Jev needs jurl's Authorization) but never written. Listens on 127.0.0.1 only.
#   python3 record.py --out results/jev-pricing.jsonl                 # upstream: Jev, port 18100
#   python3 record.py --upstream http://127.0.0.1:8000/v1/systemone --port 18101 --out results/cand-follow.jsonl
# Appending to an existing file continues its `seq`, so several runs (GROUP=pricing, then docs) make one corpus.
import argparse, http.client, json, threading, time, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ap = argparse.ArgumentParser()
ap.add_argument("--upstream", default="https://api.typesafe.ai/v1/systemone")
ap.add_argument("--port", type=int, default=18100)
ap.add_argument("--out", required=True)
# Cloudflare (in front of Jev, and of Runpod's proxy) rejects Python's default client.
ap.add_argument("--user-agent", default="curl/8.7.1")
a = ap.parse_args()
U = urllib.parse.urlsplit(a.upstream)
local = threading.local()
lock = threading.Lock()
seq = 0
try:
    for line in open(a.out):
        seq = max(seq, json.loads(line)["seq"])
except FileNotFoundError:
    pass
out = open(a.out, "a")


def conn(fresh=False):
    # one keep-alive connection per handler thread; a fresh one after an error
    c = getattr(local, "c", None)
    if c is None or fresh:
        if c is not None:
            c.close()
        cls = http.client.HTTPSConnection if U.scheme == "https" else http.client.HTTPConnection
        c = local.c = cls(U.netloc, timeout=7200)
    return c


def loads(b):
    try:
        return json.loads(b)
    except ValueError:
        return b.decode("utf-8", "replace")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    disable_nagle_algorithm = True

    def log_message(self, *_):
        pass

    def do_POST(self):
        global seq
        raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        hdr = {"Content-Type": "application/json", "User-Agent": a.user_agent}
        if self.headers.get("Authorization"):
            hdr["Authorization"] = self.headers["Authorization"]
        t0 = time.time()
        for attempt in (0, 1):
            try:
                c = conn(fresh=attempt == 1)
                c.request("POST", U.path, body=raw, headers=hdr)
                r = c.getresponse()
                status, body = r.status, r.read()
                break
            except (http.client.HTTPException, OSError) as e:
                if attempt == 1:
                    status, body = 599, json.dumps({"proxy_error": str(e)}).encode()
        dt = time.time() - t0
        with lock:
            seq += 1
            row = {"seq": seq, "t": t0, "latency_s": round(dt, 4), "status": status,
                   "request": loads(raw), "response": loads(body)}
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            out.flush()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Server(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 512


print(f"recording {a.upstream} on http://127.0.0.1:{a.port}{U.path} into {a.out} (from seq {seq + 1})", flush=True)
Server(("127.0.0.1", a.port), Handler).serve_forever()
