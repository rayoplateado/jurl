# A caching proxy in front of Jev, for the front-doors bench. jurl sends each Jev request to
# http://127.0.0.1:PORT/<tag>/v1/systemone (JURL_JEV_URL, with no key). An identical request (the same JSON body, keys in any
# order) is answered from CACHE, a JSONL file; any other is forwarded to Jev with the TypeSafe key from this process's own
# environment, and its answer is appended to CACHE. So a rerun of an unchanged search costs nothing, and the answers are
# the same answers: a difference between two rounds is the code's.
#
# Billed tokens are the input_tokens of the forwarded 200s (what Jev charges); jurl's own usage counts replayed answers
# too, so the proxy keeps both: billed (per tag, for the budget) and served (per tag, what jurl's usage reports).
# Once billed tokens reach the cap (USD at --price per million input tokens), nothing more is forwarded: 503.
#
#   TYPESAFE_API_KEY=… python3 jev_proxy.py --cache SCRATCH/front-doors-cache.jsonl --port 18200 --cap-usd 1.50
#   GET /stats/<tag>  → that tag's counts        GET /stats  → the totals and the cap
# Listens on 127.0.0.1 only. The cache holds the pages' text: keep it out of the repository.
import argparse, collections, hashlib, http.server, json, os, re, threading, time, urllib.error, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("--cache", required=True)
ap.add_argument("--port", type=int, default=18200)
ap.add_argument("--cap-usd", type=float, default=1.50)
ap.add_argument("--price", type=float, default=0.042, help="USD per million input tokens")
ap.add_argument("--upstream", default="https://api.typesafe.ai/v1/systemone")
a = ap.parse_args()

KEY = os.environ.get("TYPESAFE_API_KEY")
if not KEY:
    raise SystemExit("TYPESAFE_API_KEY must be set in this proxy's environment")
CAP_TOKENS = a.cap_usd / a.price * 1e6
TAG = re.compile(r"^[A-Za-z0-9._-]{1,120}$")


def canon(x):
    return json.dumps(x, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def input_tokens(resp):
    return int((resp.get("usage") or {}).get("input_tokens") or 0) if isinstance(resp, dict) else 0


lock = threading.Lock()
index = {}  # request key -> the recorded 200 answer
inflight = {}  # request key -> Event, while one thread forwards it
billed_total = 0
stats = collections.defaultdict(collections.Counter)
if os.path.exists(a.cache):
    for line in open(a.cache, encoding="utf-8"):
        row = json.loads(line)
        index[row["key"]] = row["response"]
        billed_total += row["billed"]
cache_out = open(a.cache, "a", encoding="utf-8")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, body):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        parts = self.path.strip("/").split("/")
        with lock:
            if parts == ["stats"]:
                served = sum(s["served_tokens"] for s in stats.values())
                return self._send(200, {
                    "billed_tokens": billed_total, "billed_usd": round(billed_total * a.price / 1e6, 6),
                    "cap_usd": a.cap_usd, "cap_tokens": int(CAP_TOKENS), "served_tokens": served,
                    "hits": sum(s["hits"] for s in stats.values()), "misses": sum(s["misses"] for s in stats.values()),
                })
            if len(parts) == 2 and parts[0] == "stats":
                s = stats[parts[1]]
                return self._send(200, {k: s[k] for k in ("hits", "misses", "billed_tokens", "served_tokens", "errors")})
        self._send(404, {"error": "not found"})

    def do_POST(self):
        global billed_total
        parts = self.path.strip("/").split("/")
        if len(parts) != 3 or parts[1:] != ["v1", "systemone"] or not TAG.match(parts[0]):
            return self._send(404, {"error": "POST /<tag>/v1/systemone"})
        tag = parts[0]
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        try:
            body = json.loads(raw)
        except ValueError:
            return self._send(400, {"error": "the body is not JSON"})
        key = hashlib.sha256(canon(body).encode()).hexdigest()
        mine = None
        while True:
            with lock:
                if key in index:
                    resp = index[key]
                    stats[tag]["hits"] += 1
                    stats[tag]["served_tokens"] += input_tokens(resp)
                    return self._send(200, resp)
                event = inflight.get(key)
                if event is None:
                    if billed_total >= CAP_TOKENS:
                        stats[tag]["errors"] += 1
                        return self._send(503, {"error": "budget cap reached: nothing more is forwarded to Jev"})
                    mine = inflight[key] = threading.Event()
                    break
            event.wait()  # another cell is forwarding this very request: its answer is the cache's next time round
        status, text = 502, b'{"error": "upstream"}'
        try:
            req = urllib.request.Request(a.upstream, data=raw, method="POST", headers={
                "Content-Type": "application/json", "Authorization": f"Bearer {KEY}",
                "User-Agent": "curl/8.7.1",  # Cloudflare in front of Jev rejects Python's default client
            })
            with urllib.request.urlopen(req, timeout=600) as r:
                status, text = r.status, r.read()
        except urllib.error.HTTPError as e:
            status, text = e.code, e.read()
        except Exception as e:  # no reply at all
            text = json.dumps({"error": f"upstream: {e}"}).encode()
        try:
            with lock:
                if status == 200:
                    resp = json.loads(text)
                    tokens = input_tokens(resp)
                    row = {"key": key, "tag": tag, "ts": round(time.time(), 1), "request": body, "response": resp, "billed": tokens}
                    cache_out.write(json.dumps(row, ensure_ascii=False) + "\n")
                    cache_out.flush()
                    index[key] = resp
                    billed_total += tokens
                    stats[tag]["misses"] += 1
                    stats[tag]["billed_tokens"] += tokens
                    stats[tag]["served_tokens"] += tokens
                else:
                    stats[tag]["errors"] += 1
        finally:
            with lock:
                inflight.pop(key, None)
            mine.set()
        self._send(status, text)


if __name__ == "__main__":
    server = http.server.ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"jev_proxy on 127.0.0.1:{a.port}: cache {len(index)} answers, billed {billed_total} tokens, cap {int(CAP_TOKENS)} tokens", flush=True)
    server.serve_forever()
