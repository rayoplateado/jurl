# The regression suite: regression.json, one jurl run per case, each answer checked against what the page says.
# Pages are read live (jurl has no page cache), so a news case holds only on the day it was recorded (its "recorded" and
# "snippet" say which). Jev's answers replay: every Jev call goes through the recording proxy in JURL_JEV_URL
# (models/record.py), which holds the TypeSafe key, so this runner and jurl never see it.
#   JURL=../target/release/jurl JURL_JEV_URL=http://127.0.0.1:18100/v1/systemone python3 regression.py <label>
#   ... python3 regression.py <label> --only news-eldiario-precise,docs-python-file-lines
# Hosts are read one case at a time, GAP seconds apart (20 by default); different hosts run side by side.
# Every mode reads --json. A case may also require what the run did, as -t reports it: "expect_stderr" (a regex, e.g. a
# render). Saves results/regression-<label>.json (answers cut to 300 characters; RESULTS=<dir> to save elsewhere) and prints
# a table: ok, the Jev input tokens the run's answers report (replayed or live), and the time. `*` marks a volatile case.
import json, os, re, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
EXE = os.environ.get("JURL", "jurl")
GAP = float(os.environ.get("GAP", "20"))  # seconds between two loads of the same host
TIMEOUT = int(os.environ.get("TIMEOUT", "300"))
JEV_URL = os.environ.get("JURL_JEV_URL")
if not JEV_URL:
    sys.exit("regression.py: set JURL_JEV_URL to the recording proxy (bench/models/record.py), so every Jev call is recorded")
label = sys.argv[1] if len(sys.argv) > 1 else "run"
only = set(sys.argv[sys.argv.index("--only") + 1].split(",")) if "--only" in sys.argv else None
cases = json.load(open(os.path.join(HERE, "regression.json"), encoding="utf-8"))
if only:
    cases = [c for c in cases if c["id"] in only]
order = {c["id"]: i for i, c in enumerate(cases)}


def argv_for(c):  # every mode reads --json, which says what the run cost (usage.jev); -t keeps the per-call lines on stderr
    mode, url, q = c["mode"], c["url"], c.get("q")
    if mode == "default":
        return [EXE, "-t", "--json", url]
    if mode == "links":
        return [EXE, "-t", "--json", "--links", "-q", q, url]
    if mode == "ask":
        return [EXE, "-t", "--json", "-q", q, url]
    if mode == "precise":
        return [EXE, "-t", "--json", "--precise", "-q", q, url]
    if mode == "follow":
        return [EXE, "-t", "--json", "--precise", "--follow", str(c.get("pages", 5)), "-q", q, url]
    sys.exit(f"{c['id']}: unknown mode {mode!r}")


def env_for(c):
    # Jev is reached only through the recording proxy: no TypeSafe key, and no jurl cloud key, reaches jurl.
    env = {k: v for k, v in os.environ.items() if k not in ("TYPESAFE_API_KEY", "JURL_CLOUD_KEY")}
    env["JURL_JEV_URL"] = JEV_URL
    env.update(c.get("env", {}))
    return env


def text_of(x):
    return x.decode("utf-8", "replace") if isinstance(x, bytes) else (x or "")


def run_case(c):
    t = time.time()
    try:
        p = subprocess.run(argv_for(c), capture_output=True, text=True, timeout=TIMEOUT, stdin=subprocess.DEVNULL,
                           env=env_for(c), cwd=HERE)
        rc, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as e:
        rc, out, err = None, text_of(e.stdout), text_of(e.stderr) + "\ntimeout"
    ms = int((time.time() - t) * 1000)
    try:
        d = json.loads(out) if out.strip() else {}
    except ValueError:
        d = {}
    answer = d.get("answer") if c["mode"] in ("precise", "follow") else None
    if c["mode"] in ("precise", "follow"):
        text = answer or ""
    elif c["mode"] == "links":
        text = "\n".join(link.get("url", "") for link in d.get("links", [])[: c.get("top", 1)])
    else:  # default and ask: the blocks that were printed
        text = "\n".join(b.get("text", "") for b in d.get("blocks", []))
    usage = d.get("usage") or {}
    pages = usage.get("pages")
    jev = (usage.get("jev") or {}).get("input_tokens")
    if jev is None:  # a miss with no JSON (-q without --precise): the -t lines say what each Jev call read
        jev = sum(int(n) for n in re.findall(r"req, (\d+) tok\)", err))
    expect = c.get("expect")
    if expect is None:
        ok = not text.strip()  # the page has no answer to give
    else:
        ok = any(e in text for e in expect)
    if c.get("expect_stderr"):  # what the run must have done, as -t reports it (a render, say), not only what it answered
        ok = ok and bool(re.search(c["expect_stderr"], err))
    row = {"id": c["id"], "group": c["group"], "mode": c["mode"], "url": c["url"], "q": c.get("q"), "ok": ok,
           "rc": rc, "answer": answer[:300] if answer else answer, "text": text[:300], "expect": expect,
           "jev_tokens": jev, "pages": pages, "ms": ms, "volatile": bool(c.get("volatile")), "note": c.get("note")}
    if rc not in (0, 1):  # a failure (not a miss): keep why
        row["stderr"] = err[-300:]
    return row


def host_of(c):
    u = c["url"] if "://" in c["url"] else "https://" + c["url"]
    return urlparse(u).hostname


def run_host(cs):  # one host's cases, one after another, each start at least GAP seconds after the previous one
    out, last = [], None
    for c in cs:
        if last is not None:
            time.sleep(max(0.0, GAP - (time.time() - last)))
        last = time.time()
        out.append(run_case(c))
    return out


by_host = {}
for c in cases:
    by_host.setdefault(host_of(c), []).append(c)
with ThreadPoolExecutor(max(1, len(by_host))) as pool:
    results = [r for rows in pool.map(run_host, by_host.values()) for r in rows]
results.sort(key=lambda r: order[r["id"]])

out_dir = os.environ.get("RESULTS") or os.path.join(HERE, "results")  # RESULTS=<dir> for runs that are not for the repo
os.makedirs(out_dir, exist_ok=True)
json.dump(results, open(os.path.join(out_dir, f"regression-{label}.json"), "w", encoding="utf-8"),
          indent=1, ensure_ascii=False)

print(f"{'case':40} {'ok':4} {'Jev tok':>9} {'time':>7}  answer or text")
for r in results:
    shown = (r["answer"] if r["answer"] is not None else r["text"]).replace("\n", " / ")[:60]
    print(f"{(r['id'] + ('*' if r['volatile'] else ''))[:40]:40} {'ok' if r['ok'] else 'FAIL':4} "
          f"{r['jev_tokens'] or 0:>9} {r['ms'] / 1000:6.1f}s  {shown}")
for g in dict.fromkeys(r["group"] for r in results):
    rs = [r for r in results if r["group"] == g]
    print(f"{g:12} {sum(r['ok'] for r in rs)}/{len(rs)} ok")
tokens = sum(r["jev_tokens"] or 0 for r in results)
print(f"all          {sum(r['ok'] for r in results)}/{len(results)} ok · {tokens} Jev input tokens reported "
      f"(${tokens * 0.042 / 1e6:.4f} if all were live; the recording proxy says what was live)")
