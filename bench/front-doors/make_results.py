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
ROUND2 = ["r2a", "r2a2", "r2a3", "r2a4", "r2a5"]  # round 2's navigation-flag variants, tried and reverted, in the order they were run
FINAL = "final"  # step 4 + step 5, no navigation flag; measured with Accept-Language en (see round2.md for the default now)
TITLE = {
    "baseline": "Baseline: #50 + #51 as merged (no change of ours)",
    "step1": "Step 1: robots.txt `Sitemap:` lines; a sitemap index read newest-first by `<lastmod>`",
    "step2": "Step 2: llms.txt of the front door's linked hosts on the same registrable domain (PSL)",
    "step3": "Step 3: links to other registrable domains (explicit links and bare URLs in the text)",
    "step4": "Step 4 (round 1): Accept-Language `en` by default, `JURL_ACCEPT_LANGUAGE` overrides",
    "step5": "Step 5, round 1 (no navigation flag): the sitemaps of linked hosts, read through their robots.txt",
    "r2a": "Round 2a, tried and reverted: navigation-like blocks dropped from the page (link share 0.8, one link), banner role",
    "r2a2": "Round 2a2, tried and reverted: navigation-like blocks dropped (two or more links)",
    "r2a3": "Round 2a3, tried and reverted: navigation flag, step 5, banner role",
    "r2a4": "Round 2a4, tried and reverted: navigation flag and step 5 (EIC's 2026 deadline is correct only with the flag)",
    "r2a5": "Round 2a5, tried and reverted: navigation flag, no step 5",
    FINAL: "FINAL: step 4 + step 5 (the linked hosts' sitemaps), no navigation flag; measured with `en`",
}
def norm(s): return re.sub(r"\s+", " ", (s or "").strip())
ALL = ROUNDS + ROUND2 + [FINAL]
cells = {rd: m.cells(rd, earlier=ALL[:i]) for i, rd in enumerate(ALL)}
REAL = ("R1", "R2", "R3", "R4", "R5", "R6", "R6es")
DEV = ("S1", "S2", "S3")
def nf_reason(cell):  # walks the final round, then every earlier round, newest first
    """The reason for a not-found cell of the final round: its own, else the one from the latest earlier round in which
    the same cell was not found (the same walk as summary.cells, marked as carried)."""
    for rd in [FINAL] + list(reversed(ALL[:-1])):
        e = NF.get(f"{rd}|{cell}")
        if e:
            return e["why"] if rd == FINAL else f"(carried from {rd}) {e['why']}"
    return "no reason recorded"


def count(items, pred): return sum(1 for r in items if pred(r))
out = []
out.append("# --follow from a front door: benchmark\n")
out.append("Measured 2026-10-10 with `bench/front-doors/run.py`. Every cell is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`; "
           "`-t` only adds stderr. Jev is behind `jev_proxy.py`, which answers an identical request from a cache and forwards the rest. "
           "Real-world sheets (R1-R6, R6es) come first everywhere.\n")
out.append("**Final state = step 4 + step 5**: the robots `Sitemap:` lines, the newest-first sitemap index, llms.txt and cross-domain "
           "links, and the sitemaps of linked hosts. No navigation flag: it was tried in round 2 and reverted (round2.md). "
           "The final round ran with Accept-Language `en`; the default is now none (the caller sets `JURL_ACCEPT_LANGUAGE`), and the "
           "three cells that depend on the old default were re-run with none (round2.md). Binary 12,065,040 bytes.\n")
out.append("## Headline\n")
out.append("| set | round | cells | correct | wrong | not found | not run |\n|---|---|---:|---:|---:|---:|---:|")
LABEL = {"baseline": "baseline", "step4": "step 4 (round 1)", "r2a4": "r2a4: flag + step 5 (tried, reverted)", FINAL: "**final: step 4 + step 5**"}
for label, sel in [("real-world", REAL), ("developer", DEV), ("all", REAL + DEV)]:
    for rd in ("baseline", "step4", "r2a4", FINAL):
        items = [r for r in cells[rd] if r["sheet"] in sel]
        out.append(f"| {label} | {LABEL[rd]} | {len(items)} | "
                   f"{count(items, lambda r: r['verdict'] == 'correct')} | {count(items, lambda r: r['verdict'] == 'wrong')} | "
                   f"{count(items, lambda r: r['verdict'] == 'not_found')} | {count(items, lambda r: r['verdict'] == 'not_run')} |")
out.append("")
out.append("## Rounds\n")
out.append("Columns: correct / wrong / not found per sheet; mean pages read; mean tokens used (what each run's JSON reports, cached answers included); "
           "billed tokens (what Jev charged for that round's cells: the proxy's forwarded answers) and its cost at $0.042 per million input tokens.\n")
for rd in ALL:
    out.append(m.table(rd, cells[rd], TITLE[rd]))
    out.append("")
    out.append(f"Failures by class ({rd}): {m.classes(rd, cells[rd])}.\n")
out.append("## Final round, cell by cell\n")
out.append("Verdict per cell. A correct or wrong answer is judged by its quote, with the one-line reason from `judgements.json`. "
           "A cell with no answer gives its class (see the legend) and the reason from `notfound.json`.\n")
out.append("| sheet | row | question | answer (closest) | verdict | reason |\n|---|---|---|---|---|---|")
for r in cells[FINAL]:
    ans = (r["answer"] or r["closest"] or "").replace("|", "/")[:70]
    if r["answer"]:
        why = J.get(f"{r['front']}|{r['question']}|{norm(r['answer'])}", {}).get("why", "")
    elif r["verdict"] == "not_run":
        why = f"{r['class']}. jurl made no call: no verdict until the cell is re-run."
    else:
        why = f"{r['class']}. " + nf_reason(f"{r['sheet']}-{r['row']}-{r['q']}")  # the final round's own reason, else carried
    out.append(f"| {r['sheet']} | {r['name']} | {r['question']} | {ans} | {r['verdict']} | {why.replace('|', '/')} |")
out.append("")
hand = HERE / "round2.md"
if hand.exists():
    out.append(hand.read_text(encoding="utf-8"))
(HERE / "RESULTS.md").write_text("\n".join(out) + "\n", encoding="utf-8")
print("RESULTS.md written:", sum(len(x) for x in out), "chars")
