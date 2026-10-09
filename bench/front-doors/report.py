# Prints a round's cells and its per-sheet summary, with the judgements in judgements.json.
#   python3 report.py ROUND [--cells] [--sheets S1,R1] [--markdown]
# A cell with an answer is judged by (front door, question, the answer): judgements.json maps that to
# {"verdict": "correct" | "wrong", "why": "one line"}. A cell with no answer is judged by its round:
# notfound.json maps "ROUND|SHEET-ROW-Q" to {"class": ..., "why": ...}. Classes for not-found and wrong cells:
# blocked (403/429/captcha), js_only (needs JavaScript), not_on_site (the answer is not on the site), not_reached
# (the answer is on the site, the follow did not reach it), wrong_answer (answered, not supported).
import argparse, json, pathlib, re, statistics as st

HERE = pathlib.Path(__file__).resolve().parent
ap = argparse.ArgumentParser()
ap.add_argument("round")
ap.add_argument("--cells", action="store_true")
ap.add_argument("--sheets", default="")
ap.add_argument("--markdown", action="store_true")
a = ap.parse_args()

JUDGE_FILE = HERE / "judgements.json"
NF_FILE = HERE / "notfound.json"
judgements = json.loads(JUDGE_FILE.read_text(encoding="utf-8")) if JUDGE_FILE.exists() else {}
notfound = json.loads(NF_FILE.read_text(encoding="utf-8")) if NF_FILE.exists() else {}
ORDER = ["R1", "R2", "R3", "R4", "R5", "R6", "R6es", "S1", "S2", "S3"]
TITLES = {}


def norm(s):
    return re.sub(r"\s+", " ", (s or "").strip())


def key_of(rec):
    return f"{rec['front']}|{rec['question']}|{norm(rec['answer'])}"


rows = []
for p in sorted((HERE / "results" / a.round).glob("*.json")):
    rows.append(json.loads(p.read_text(encoding="utf-8")))
wanted = [s for s in a.sheets.split(",") if s]
rows = [r for r in rows if not wanted or r["sheet"] in wanted]
rows.sort(key=lambda r: (ORDER.index(r["sheet"]) if r["sheet"] in ORDER else 99, r["row"], r["q"]))


def verdict(r):
    if r["answer"]:
        j = judgements.get(key_of(r))
        return (j["verdict"], j.get("why", "")) if j else ("UNJUDGED", "")
    nf = notfound.get(f"{a.round}|{r['sheet']}-{r['row']}-{r['q']}")
    if nf:
        return ("not_found:" + nf["class"], nf.get("why", ""))
    return ("not_found:UNCLASSIFIED", "")


cells = []
for r in rows:
    v, why = verdict(r)
    cells.append((r, v, why))

sheets = {}
for r, v, why in cells:
    sheets.setdefault(r["sheet"], []).append((r, v, why))

if a.cells:
    for r, v, why in cells:
        ans = (r["answer"] or r["error"] or "")[:60]
        print(f"{r['sheet']:4} {r['name'][:20]:20} q{r['q']} {v:24} p={r['p'] if r['p'] is not None else '-'!s:5} pg={r['pages']!s:>3} "
              f"used={r['tokens_used']:>8} billed={r['tokens_billed']:>8} {r['ms']/1000:5.1f}s | {ans}")
        if r["answer"]:
            print(f"        quote: {norm(r['quote'])[:260]}")
        if why:
            print(f"        why: {why}")

print()
print(f"{'sheet':5} {'cells':>5} {'correct':>7} {'wrong':>5} {'notfound':>8} {'unjudged':>8} {'mean pg':>7} {'mean used':>9} {'mean billed':>11} {'billed $':>9}")
tot = [0, 0, 0, 0, 0, 0, 0, 0]
for s in ORDER:
    if s not in sheets:
        continue
    items = sheets[s]
    c = sum(1 for _, v, _ in items if v == "correct")
    w = sum(1 for _, v, _ in items if v == "wrong")
    n = sum(1 for _, v, _ in items if v.startswith("not_found"))
    u = sum(1 for _, v, _ in items if v == "UNJUDGED")
    pages = [r["pages"] for r, _, _ in items if r["pages"] is not None]
    used = [r["tokens_used"] for r, _, _ in items]
    billed = sum(r["tokens_billed"] for r, _, _ in items)
    print(f"{s:5} {len(items):>5} {c:>7} {w:>5} {n:>8} {u:>8} {st.mean(pages) if pages else 0:>7.1f} "
          f"{st.mean(used) if used else 0:>9.0f} {(sum(used)/len(used) if used else 0):>11.0f} {billed*0.042/1e6:>9.4f}")
    for i, x in enumerate([len(items), c, w, n, u, sum(billed for _ in [0]), 0, 0]):
        tot[i] += x
    tot[5] += billed
allc = [r for r, _, _ in cells]
allv = [v for _, v, _ in cells]
pages = [r["pages"] for r in allc if r["pages"] is not None]
print(f"{'TOTAL':5} {len(cells):>5} {allv.count('correct'):>7} {allv.count('wrong'):>5} "
      f"{sum(v.startswith('not_found') for v in allv):>8} {allv.count('UNJUDGED'):>8} "
      f"{st.mean(pages) if pages else 0:>7.1f} {(sum(r['tokens_used'] for r in allc)/max(1,len(allc))):>9.0f} "
      f"{(sum(r['tokens_billed'] for r in allc)/max(1,len(allc))):>11.0f} {sum(r['tokens_billed'] for r in allc)*0.042/1e6:>9.4f}")
