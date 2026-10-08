# A recording proxy in front of a Jev-contract endpoint: jurl talks to it (JURL_JEV_URL), it forwards every POST to
# --upstream and appends each request and response body, with the upstream's latency, as one JSON line to --out.
# jurl never sends the TypeSafe key to JURL_JEV_URL, so to Jev the proxy adds it itself, from TYPESAFE_API_KEY in its
# own environment; to any other upstream it forwards jurl's bearer (JURL_JEV_KEY), if any. No header is ever written.
# Listens on 127.0.0.1 only.
#   TYPESAFE_API_KEY=… python3 record.py --out results/jev-pricing.jsonl     # upstream: Jev, port 18100
#   python3 record.py --upstream http://127.0.0.1:8000/v1/systemone --port 18101 --out results/cand-follow.jsonl
#   python3 record.py --from results/base.jsonl --out results/check.jsonl --port 18102
# --from answers from a recording and never forwards: a POST whose body (canonical JSON) is in base.jsonl gets its
# recorded answer, any other gets 500 {"error": "not in the recording"}, so a jurl that asks Jev something new fails.
# With --from, each line of --out gets "replayed": true|false. Hits and misses go to stderr, with the totals at exit.
# Appending to an existing file continues its `seq`, so several runs (GROUP=pricing, then docs) make one corpus.
import argparse, collections, http.client, json, os, signal, sys, threading, time, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ap = argparse.ArgumentParser()
ap.add_argument("--upstream", default="https://api.typesafe.ai/v1/systemone")
ap.add_argument("--port", type=int, default=18100)
ap.add_argument("--out", required=True)
ap.add_argument("--from", dest="recording", help="answer from this recording instead of forwarding")
# Cloudflare (in front of Jev, and of Runpod's proxy) rejects Python's default client.
ap.add_argument("--user-agent", default="curl/8.7.1")
a = ap.parse_args()
if a.recording and os.path.abspath(a.recording) == os.path.abspath(a.out):
    ap.error("--out is the --from recording itself: appending to it would change the recording")
U = urllib.parse.urlsplit(a.upstream)
JEV_KEY = None
if U.hostname == "api.typesafe.ai" and not a.recording:  # --from never forwards: no key needed
    JEV_KEY = os.environ.get("TYPESAFE_API_KEY")
    if not JEV_KEY:
        raise SystemExit("recording Jev needs TYPESAFE_API_KEY in this proxy's environment")
local = threading.local()
lock = threading.Lock()
seq = 0
try:
    for line in open(a.out):
        seq = max(seq, json.loads(line)["seq"])
except FileNotFoundError:
    pass
out = open(a.out, "a")


def canon(x):  # what a request is matched on: the same body with its keys in another order is the same request
    return json.dumps(x, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def index_of(path):  # canonical request -> (status, response) of its recorded answer; a 200 beats another status
    idx = {}
    for line in open(path):
        row = json.loads(line)
        key, got = canon(row["request"]), (row["status"], row["response"])
        if key not in idx or (idx[key][0] != 200 and got[0] == 200):
            idx[key] = got
    return idx


recorded = index_of(a.recording) if a.recording else None
stats = collections.Counter()


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


def forward(raw, auth):  # to Jev with the proxy's own key; to another upstream with jurl's bearer, if any
    hdr = {"Content-Type": "application/json", "User-Agent": a.user_agent}
    if JEV_KEY:
        hdr["Authorization"] = "Bearer " + JEV_KEY
    elif auth:
        hdr["Authorization"] = auth
    for attempt in (0, 1):
        try:
            c = conn(fresh=attempt == 1)
            c.request("POST", U.path, body=raw, headers=hdr)
            r = c.getresponse()
            return r.status, r.read()
        except (http.client.HTTPException, OSError) as e:
            if attempt == 1:
                return 599, json.dumps({"proxy_error": str(e)}).encode()


def answer(raw):  # --from: the recorded answer to this body, or a 500 for a body the recording doesn't have
    hit = recorded.get(canon(loads(raw)))
    if hit is None:
        return 500, json.dumps({"error": "not in the recording"}).encode(), False
    status, response = hit
    if isinstance(response, str):  # a body that wasn't JSON was saved as its text
        return status, response.encode(), True
    return status, json.dumps(response).encode(), True


def page_of(req):  # for stderr: the page a request is about, and how many questions it asks
    if not isinstance(req, dict):
        return "not a JSON object"
    return f"{req.get('state', {}).get('url')}, {len(req.get('questions', {}))} questions"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    disable_nagle_algorithm = True

    def log_message(self, *_):
        pass

    def do_POST(self):
        global seq
        raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        t0 = time.time()
        if recorded is None:
            status, body = forward(raw, self.headers.get("Authorization"))
        else:
            status, body, hit = answer(raw)
        dt = time.time() - t0
        with lock:
            seq += 1
            row = {"seq": seq, "t": t0, "latency_s": round(dt, 4), "status": status,
                   "request": loads(raw), "response": loads(body)}
            if recorded is not None:
                row["replayed"] = hit
                stats["hit" if hit else "miss"] += 1
                print(f"{'hit' if hit else 'MISS':4} seq {seq} {page_of(row['request'])} "
                      f"(hits {stats['hit']}, misses {stats['miss']} so far)", file=sys.stderr, flush=True)
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


signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))  # ends serve_forever as Ctrl-C does: the totals still print
if recorded is None:
    print(f"recording {a.upstream} on http://127.0.0.1:{a.port}{U.path} into {a.out} (from seq {seq + 1})", flush=True)
else:
    print(f"serving {len(recorded)} distinct recorded bodies from {a.recording} on http://127.0.0.1:{a.port}{U.path} "
          f"into {a.out} (from seq {seq + 1})", flush=True)
try:
    Server(("127.0.0.1", a.port), Handler).serve_forever()
except KeyboardInterrupt:
    pass
finally:
    if recorded is not None:
        print(f"served from {a.recording}: hits {stats['hit']}, misses {stats['miss']}", file=sys.stderr, flush=True)
