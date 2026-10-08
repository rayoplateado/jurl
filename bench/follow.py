# --follow on follow.json, or on the cases file in $CASES (a path as given: CASES=bench/follow-real.json): every case 3 times, in parallel, with the jurl in $JURL (default: jurl on PATH).
# Right = the answer contains one of `expect` (traps: no answer at all). Prints a summary and saves results/follow-<label>.json.
#   JURL=../target/release/jurl python3 follow.py <label> [runs]
import json, os, re, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

EXE = os.environ.get("JURL", "jurl")
HERE = os.path.dirname(os.path.abspath(__file__))
cases = json.load(open(os.environ.get("CASES") or os.path.join(HERE, "follow.json"), encoding="utf-8"))
# GROUP=long runs only that group.
if os.environ.get("GROUP"):
    cases = [c for c in cases if c["group"] == os.environ["GROUP"]]
# POOL searches at a time (6), each given TIMEOUT seconds (180): POOL=1 TIMEOUT=1800 for a slow self-hosted model.
POOL = int(os.environ.get("POOL", "6"))
TIMEOUT = int(os.environ.get("TIMEOUT", "180"))
label = sys.argv[1] if len(sys.argv) > 1 else "run"
runs = int(sys.argv[2]) if len(sys.argv) > 2 else 3


def one(case):
    argv = [EXE, "-t", "--json", "--precise", "--follow", str(case.get("pages", 5)), "-q", case["q"], case["start"]]
    t = time.time()
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=TIMEOUT, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        # a search that runs out of time is wrong, traps included
        return {
            "start": case["start"], "q": case["q"], "group": case["group"], "answer": None, "right": False,
            "pages": None, "tokens": None, "ms": int((time.time() - t) * 1000), "path": [], "timeout": True,
        }
    ms = int((time.time() - t) * 1000)
    answer = None
    path = []
    try:
        d = json.loads(p.stdout)
        answer = d.get("answer")
        path = d.get("path", [])
    except ValueError:
        pass
    pages = re.search(r"(\d+) pages · (\d+) tokens", p.stderr)
    expect = case["expect"]
    right = (answer is None) if expect is None else bool(answer) and any(e in answer for e in expect)
    return {
        "start": case["start"], "q": case["q"], "group": case["group"], "answer": answer, "right": right,
        "pages": int(pages.group(1)) if pages else None, "tokens": int(pages.group(2)) if pages else None,
        "ms": ms, "path": path,
    }


jobs = [c for c in cases for _ in range(runs)]
with ThreadPoolExecutor(POOL) as pool:
    results = list(pool.map(one, jobs))
os.makedirs(os.path.join(HERE, "results"), exist_ok=True)
json.dump(results, open(os.path.join(HERE, "results", f"follow-{label}.json"), "w"), indent=1, ensure_ascii=False)

def med(xs):
    xs = sorted(x for x in xs if x is not None)
    return xs[len(xs) // 2] if xs else None

print(f"{'case':58} right  pages  tokens   time")
for c in cases:
    rs = [r for r in results if r["start"] == c["start"] and r["q"] == c["q"]]
    ok = sum(r["right"] for r in rs)
    ans = " | ".join(sorted({(r["answer"] or "∅").replace("\n", " / ")[:18] for r in rs}))
    print(f"{(c['start'] + ' · ' + c['q'])[:58]:58} {ok}/{len(rs)}   {med([r['pages'] for r in rs])!s:>4}  {med([r['tokens'] for r in rs])!s:>7}  {med([r['ms'] for r in rs])/1000:5.1f}s  {ans}")
for g in dict.fromkeys(c["group"] for c in cases):
    rs = [r for r in results if r["group"] == g]
    if not rs:
        continue
    print(f"{g:10} {sum(r['right'] for r in rs)}/{len(rs)} right · median {med([r['pages'] for r in rs])} pages · {med([r['tokens'] for r in rs])} tokens · {med([r['ms'] for r in rs])/1000:.1f}s")
tokens = sum(r["tokens"] or 0 for r in results)
print(f"all       {sum(r['right'] for r in results)}/{len(results)} right · {tokens} tokens in all (${tokens * 0.042 / 1e6:.3f})")
