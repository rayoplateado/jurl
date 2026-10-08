# Replays a recording (record.py) to another endpoint with Jev's contract and saves, per request, the recorded answer
# (`jev`) next to the new one (`<name>`) for compare.py. Resumes: requests already in OUT are skipped.
#   python3 replay.py CORPUS.jsonl OUT.jsonl URL [--name NAME] [--sample N | --same-as REPLAY.jsonl] [--concurrency C]
# --sample N: a stratified sample (by question kind and size) plus up to 40 requests with a choice question. Seeded,
# so the same corpus and N give the same requests for every candidate. --same-as: exactly the requests of another
# replay. Neither: every recorded request that got a 200. REPLAY_KEY, if set, is sent as the bearer (only Jev needs
# it, and replaying to Jev costs what recording did).
import argparse, collections, json, os, random, threading, time, urllib.error, urllib.request
from concurrent.futures import ThreadPoolExecutor

ap = argparse.ArgumentParser()
ap.add_argument("corpus")
ap.add_argument("out")
ap.add_argument("url")
ap.add_argument("--name", default="candidate", help="the new answers' field (and <name>_latency_s)")
ap.add_argument("--sample", type=int)
ap.add_argument("--same-as")
ap.add_argument("--concurrency", type=int, default=1, help="1 (default) keeps latencies comparable")
ap.add_argument("--timeout", type=float, default=7200)
ap.add_argument("--user-agent", default="curl/8.7.1", help="Cloudflare rejects Python's default client")
a = ap.parse_args()

rows = [json.loads(line) for line in open(a.corpus)]
rows = [r for r in rows if r["status"] == 200 and isinstance(r["response"], dict)]


def stratum(r):
    ids = r["request"]["questions"].keys()
    kind = "links" if any(k.startswith("l") for k in ids) else "blocks" if any(k[0] == "b" and k[1:].isdigit() for k in ids) else "other"
    tokens = r["response"].get("usage", {}).get("input_tokens", 0)
    return kind, min(tokens // 5000, 3)


if a.same_as:
    want = [json.loads(line)["seq"] for line in open(a.same_as)]
    by_seq = {r["seq"]: r for r in rows}
    todo = [by_seq[s] for s in want if s in by_seq]
elif a.sample:
    random.seed(0)
    strata = collections.defaultdict(list)
    for r in rows:
        strata[stratum(r)].append(r)
    todo = []
    for _, v in sorted(strata.items()):
        todo += random.sample(v, min(max(4, round(a.sample * len(v) / len(rows))), len(v)))
    # choice questions (page_kind, pick, next, tighter) are few: up to 40 more requests that carry one
    picked = {r["seq"] for r in todo}
    choice = [r for r in rows if r["seq"] not in picked
              and any(q.get("type") == "choice" for q in r["request"]["questions"].values())]
    todo += random.sample(choice, min(40, len(choice)))
    todo.sort(key=lambda r: r["seq"])
else:
    todo = rows

try:
    done = {json.loads(line)["seq"] for line in open(a.out)}
except FileNotFoundError:
    done = set()
todo = [r for r in todo if r["seq"] not in done]
print(f"{len(rows)} recorded requests; {len(done)} already in {a.out}; {len(todo)} to replay", flush=True)

headers = {"Content-Type": "application/json", "User-Agent": a.user_agent}
if os.environ.get("REPLAY_KEY"):
    headers["Authorization"] = "Bearer " + os.environ["REPLAY_KEY"]
lock = threading.Lock()
out = open(a.out, "a")
n = 0


def one(r):
    global n
    t0 = time.time()
    try:
        req = urllib.request.Request(a.url, json.dumps(r["request"]).encode(), headers)
        resp, status = json.load(urllib.request.urlopen(req, timeout=a.timeout)), 200
    except urllib.error.HTTPError as e:
        resp, status = e.read().decode("utf-8", "replace"), e.code
    except Exception as e:  # timeouts, refused connections, bad JSON: kept as a failed row
        resp, status = f"{type(e).__name__}: {e}", 599
    dt = time.time() - t0
    row = {"seq": r["seq"], "stratum": stratum(r), "jev_latency_s": r["latency_s"], f"{a.name}_latency_s": round(dt, 3),
           "status": status, "request": r["request"], "jev": r["response"], a.name: resp}
    with lock:
        out.write(json.dumps(row, ensure_ascii=False) + "\n")
        out.flush()
        n += 1
        print(n, r["seq"], row["stratum"], status, f"{dt:.1f}s", flush=True)


with ThreadPoolExecutor(a.concurrency) as pool:
    list(pool.map(one, todo))
