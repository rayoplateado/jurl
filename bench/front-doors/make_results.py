# Writes bench/front-doors/RESULTS.md from the rounds (results/), judgements.json and notfound.json: the tables are
# summary.py's, so the numbers in the PR and in this file come from one place.
#   python3 make_results.py   (then edit RESULTS.md by hand only where the text says so)
import importlib.util, json, pathlib, re, sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("summ", HERE / "summary.py")
m = importlib.util.module_from_spec(spec)
sys.argv_saved, sys.argv = sys.argv, ["x"]
spec.loader.exec_module(m)
sys.argv = sys.argv_saved
J = json.loads((HERE / "judgements.json").read_text(encoding="utf-8"))
NF = json.loads((HERE / "notfound.json").read_text(encoding="utf-8"))
ROUNDS = ["baseline", "step1", "step2", "step3", "step4", "step5"]
TITLE = {
    "baseline": "Baseline: #50 + #51 as merged (no change of ours)",
    "step1": "Step 1: robots.txt `Sitemap:` lines; a sitemap index read newest-first by `<lastmod>`",
    "step2": "Step 2: llms.txt of the front door's linked hosts on the same registrable domain (PSL)",
    "step3": "Step 3: links to other registrable domains (explicit links and bare URLs in the text)",
    "step4": "Step 4 (FINAL): Accept-Language `en` by default, `JURL_ACCEPT_LANGUAGE` overrides",
    "step5": "Step 5 (tried, REVERTED): the sitemaps of linked hosts, read through their robots.txt",
}
def norm(s): return re.sub(r"\s+", " ", (s or "").strip())
cells = {rd: m.cells(rd, earlier=ROUNDS[:i]) for i, rd in enumerate(ROUNDS)}
REAL = ("R1", "R2", "R3", "R4", "R5", "R6", "R6es")
DEV = ("S1", "S2", "S3")
def nf_reason(cell):
    """The reason for a not-found cell of the final round: its own, else the one from the latest earlier round in which
    the same cell was not found (the same walk as summary.cells, marked as carried)."""
    for rd in ["step4"] + list(reversed(ROUNDS[:4])):
        e = NF.get(f"{rd}|{cell}")
        if e:
            return e["why"] if rd == "step4" else f"(carried from {rd}) {e['why']}"
    return "no reason recorded"


def count(items, pred): return sum(1 for r in items if pred(r))
out = []
out.append("# --follow from a front door: benchmark\n")
out.append("Measured 2026-10-10 with `bench/front-doors/run.py`. Every cell is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`; "
           "`-t` only adds stderr. Jev is behind `jev_proxy.py`, which answers an identical request from a cache and forwards the rest. "
           "Real-world sheets (R1-R6, R6es) come first everywhere.\n")
out.append("**Final state = step 4.** Its binary is byte-identical to the final build (12,048,496 bytes). Step 5 was measured and reverted.\n")
out.append("## Headline\n")
out.append("| set | round | cells | correct | wrong | not found |\n|---|---|---:|---:|---:|---:|")
for label, sel in [("real-world", REAL), ("developer", DEV), ("all", REAL + DEV)]:
    for rd in ("baseline", "step4"):
        items = [r for r in cells[rd] if r["sheet"] in sel]
        out.append(f"| {label} | {rd}{' (final)' if rd == 'step4' else ''} | {len(items)} | "
                   f"{count(items, lambda r: r['verdict'] == 'correct')} | {count(items, lambda r: r['verdict'] == 'wrong')} | "
                   f"{count(items, lambda r: r['verdict'] == 'not_found')} |")
out.append("")
out.append("## Rounds\n")
out.append("Columns: correct / wrong / not found per sheet; mean pages read; mean tokens used (what each run's JSON reports, cached answers included); "
           "billed tokens (what Jev charged for that round's cells: the proxy's forwarded answers) and its cost at $0.042 per million input tokens.\n")
for rd in ROUNDS:
    out.append(m.table(rd, cells[rd], TITLE[rd]))
    out.append("")
    out.append(f"Failures by class ({rd}): {m.classes(rd, cells[rd])}.\n")
out.append("## Final round, cell by cell\n")
out.append("Verdict per cell. A correct or wrong answer is judged by its quote, with the one-line reason from `judgements.json`. "
           "A cell with no answer gives its class (see the legend) and the reason from `notfound.json`.\n")
out.append("| sheet | row | question | answer (closest) | verdict | reason |\n|---|---|---|---|---|---|")
for r in cells["step4"]:
    ans = (r["answer"] or r["closest"] or "").replace("|", "/")[:70]
    if r["answer"]:
        why = J.get(f"{r['front']}|{r['question']}|{norm(r['answer'])}", {}).get("why", "")
    else:
        why = f"{r['class']}. " + nf_reason(f"{r['sheet']}-{r['row']}-{r['q']}")
    out.append(f"| {r['sheet']} | {r['name']} | {r['question']} | {ans} | {r['verdict']} | {why.replace('|', '/')} |")
out.append("")
(HERE / "RESULTS.md").write_text("\n".join(out) + "\n", encoding="utf-8")
print("RESULTS.md written:", sum(len(x) for x in out), "chars")
