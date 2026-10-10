# The front-doors bench: every cell of sheets.json is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`, with Jev
# behind jev_proxy.py (the proxy must be running; see its header). Each round writes one JSON file per cell to
# results/ROUND/SHEET-ROW-Q.json (stdout, stderr, exit code, answer, pages, tokens used and billed, time), so a
# rerun skips the cells already done. A row's questions run one after the other; up to --parallel rows at once.
#
#   TYPESAFE_API_KEY=… python3 jev_proxy.py --cache SCRATCH/front-doors-cache.jsonl --port 18200 --cap-usd 1.50 &
#   JURL=/path/to/jurl python3 run.py step0 --sheets S1
#
# The runner never holds the TypeSafe key: jurl gets JURL_JEV_URL pointing at the proxy, and no key.
import argparse, concurrent.futures as cf, json, os, pathlib, re, subprocess, sys, threading, time, urllib.request

HERE = pathlib.Path(__file__).resolve().parent
ap = argparse.ArgumentParser()
ap.add_argument("round", help="the round's name: its results go to results/ROUND/")
ap.add_argument("--sheets", default="", help="comma-separated sheet ids (default: every sheet)")
ap.add_argument("--jurl", default=os.environ.get("JURL", "jurl"))
ap.add_argument("--port", type=int, default=18200, help="where jev_proxy.py listens")
ap.add_argument("--parallel", type=int, default=4, help="rows at once (at most 4)")
ap.add_argument("--timeout", type=int, default=900, help="seconds per cell")
ap.add_argument("--accept-language", help="JURL_ACCEPT_LANGUAGE for this round (default: unset, no header)")
ap.add_argument("--only", default="", help="comma-separated cells as their file names give them, e.g. S1-0-0,R5-2-1")
ap.add_argument("--force", action="store_true", help="rerun cells that already have a result")
a = ap.parse_args()
a.parallel = max(1, min(a.parallel, 4))
PROXY = f"http://127.0.0.1:{a.port}"
OUT = HERE / "results" / a.round
OUT.mkdir(parents=True, exist_ok=True)
lock = threading.Lock()


def proxy(path):
    with urllib.request.urlopen(PROXY + path, timeout=30) as r:
        return json.loads(r.read())


def cells():
    sheets = json.loads((HERE / "sheets.json").read_text(encoding="utf-8"))["sheets"]
    wanted = [s for s in a.sheets.split(",") if s]
    only = {c for c in a.only.split(",") if c}
    for sheet in sheets:
        if wanted and sheet["id"] not in wanted:
            continue
        for ri, row in enumerate(sheet["rows"]):
            for qi, question in enumerate(sheet["questions"]):
                if only and f"{sheet['id']}-{ri}-{qi}" not in only:
                    continue
                yield sheet, ri, row, qi, question


def run_cell(sheet, ri, row, qi, question):
    tag = f"{a.round}.{sheet['id']}.{ri}.{qi}"
    path = OUT / f"{sheet['id']}-{ri}-{qi}.json"
    if path.exists() and not a.force:
        return "skipped"
    st = proxy("/stats")
    if st["billed_tokens"] >= st["cap_tokens"]:
        return "budget"
    env = {k: v for k, v in os.environ.items() if k not in ("TYPESAFE_API_KEY", "JURL_JEV_KEY")}
    env["JURL_JEV_URL"] = f"{PROXY}/{tag}/v1/systemone"
    env.pop("JURL_ACCEPT_LANGUAGE", None)  # a round without --accept-language sends no header, whatever the shell has
    if a.accept_language:
        env["JURL_ACCEPT_LANGUAGE"] = a.accept_language
    argv = [a.jurl, "-t", "--json", "--precise", "--follow", "5", "-q", question, row["front"]]
    t0 = time.monotonic()
    try:
        p = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=a.timeout, stdin=subprocess.DEVNULL)
        code, out, err, timed_out = p.returncode, p.stdout, p.stderr, False
    except subprocess.TimeoutExpired as e:
        code, out, err, timed_out = None, (e.stdout or b"").decode(errors="replace"), (e.stderr or b"").decode(errors="replace"), True
    ms = int((time.monotonic() - t0) * 1000)
    doc = None
    try:
        doc = json.loads(out) if out.strip().startswith("{") else None
    except ValueError:
        doc = None
    m = re.search(r"(\d+) pages · (\d+) tokens", err)
    jurl_pages = (doc or {}).get("usage", {}).get("pages") if doc else None
    pages = jurl_pages if jurl_pages is not None else (int(m.group(1)) if m else None)
    counts = proxy(f"/stats/{tag}")
    errors = [l for l in err.splitlines() if l.startswith("jurl: ")]
    rec = {
        "round": a.round, "sheet": sheet["id"], "lang": sheet["lang"], "row": ri, "name": row["name"], "front": row["front"],
        "question": question, "q": qi, "tag": tag, "exit": code, "timed_out": timed_out, "ms": ms,
        "answer": (doc or {}).get("answer") if doc else None, "closest": (doc or {}).get("closest") if doc else None,
        "quote": (doc or {}).get("quote") if doc else None, "link": (doc or {}).get("link") if doc else None,
        "p": (doc or {}).get("p") if doc else None, "path": (doc or {}).get("path") if doc else None,
        "pages": pages, "tokens_used": counts["served_tokens"], "tokens_billed": counts["billed_tokens"],
        "jurl_tokens": ((doc or {}).get("usage") or {}).get("jev", {}).get("input_tokens") if doc else None,
        "requests": {"hits": counts["hits"], "misses": counts["misses"], "errors": counts["errors"]},
        "error": errors[-1][len("jurl: "):] if errors else None,
        "budget_stop": "budget cap reached" in err or "budget cap reached" in out,
        "accept_language": a.accept_language or "(none)",
        "stdout": out, "stderr": err,
    }
    with lock:
        path.write_text(json.dumps(rec, ensure_ascii=False, indent=1), encoding="utf-8")
        ans = (rec["answer"] or "")[:70].replace("\n", " ")
        status = "ANSWER " if rec["answer"] else ("TIMEOUT" if timed_out else f"exit {code}")
        print(f"{sheet['id']:4} {row['name'][:22]:22} q{qi} {status:8} p={rec['p'] if rec['p'] is not None else '-':<5} "
              f"pages={pages!s:>3} used={counts['served_tokens']:>8} billed={counts['billed_tokens']:>8} {ms/1000:6.1f}s | {ans}",
              flush=True)
    return "done"


def do_row(item):
    results = []
    for sheet, ri, row, qi, question in item:
        r = run_cell(sheet, ri, row, qi, question)
        results.append(r)
        if r == "budget":
            break
    return results


by_row = {}
for sheet, ri, row, qi, question in cells():
    by_row.setdefault((sheet["id"], ri), []).append((sheet, ri, row, qi, question))
print(f"round {a.round}: {sum(len(v) for v in by_row.values())} cells in {len(by_row)} rows, {a.parallel} rows at once", flush=True)
with cf.ThreadPoolExecutor(max_workers=a.parallel) as pool:
    outcomes = [r for res in pool.map(do_row, by_row.values()) for r in res]
st = proxy("/stats")
print(f"done: {outcomes.count('done')} run, {outcomes.count('skipped')} already done, {outcomes.count('budget')} not run (budget). "
      f"Billed so far: {st['billed_tokens']} tokens = ${st['billed_usd']:.4f} of ${st['cap_usd']:.2f} (served {st['served_tokens']} tokens).")
