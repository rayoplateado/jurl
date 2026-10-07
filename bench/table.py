# Scores the saved results and prints the table in README.md.
#
#   uv run --with tiktoken --with requests bench/table.py
#
# "Exact answer" means the task's truth string (tasks.json) is in what the reader returned, word for word.
# "Code lines not on the page": every line inside a code block of the answer is looked up, ignoring whitespace, in
# the page as served (HTML with tags stripped, the markdown it serves to `Accept: text/markdown`, and Jina's
# markdown). Search rows are left out: their results are other pages.

import json, re, html, statistics as st
from pathlib import Path

import requests, tiktoken

HERE = Path(__file__).parent
TASKS = json.loads((HERE / "tasks.json").read_text())
IDS = [t["id"] for t in TASKS]
R = {p.stem: json.loads(p.read_text()) for p in (HERE / "results").glob("*.json")}
enc = tiktoken.get_encoding("cl100k_base")
norm = lambda s: re.sub(r"\s+", " ", s).lower()
squash = lambda s: re.sub(r"\s+", "", html.unescape(s))
JEV_PER_TOKEN = 0.042 / 1e6


def page(url):
    get = lambda u, **h: requests.get(u, headers={"user-agent": "Mozilla/5.0", **h}, timeout=90).text
    return "\n".join(
        squash(x)
        for x in [re.sub(r"<[^>]+>", "", get(url)), get(url, accept="text/markdown"), get("https://r.jina.ai/" + url)]
    )


def code_lines(md):
    return [l.strip() for b in re.findall(r"```[^\n]*\n(.*?)```", md, re.S) for l in b.split("\n") if l.strip()]


def truth_hit(i, text):
    t = next(t for t in TASKS if t["id"] == i)
    return bool(text) and all(norm(s) in norm(text) for s in t["truth"])


jurl_runs = R["jurl"]["runs"]
readers = {
    "jurl -q": {i: jurl_runs[0][i] for i in IDS},
    "Claude Code WebFetch": {i: {"text": R["webfetch"][i]["text"], "ms": None} for i in IDS},
    "Exa contents + highlights": {i: R["exa"][i]["contents"] for i in IDS},
    "Exa search + highlights": {i: R["exa"][i]["search"] for i in IDS},
    "Tavily extract (query)": {i: R["tavily"][i]["extract"] for i in IDS},
    "Tavily search": {i: R["tavily"][i]["search"] for i in IDS},
}
pages = {t["id"]: page(t["url"]) for t in TASKS}

print("| Reader | Exact answer | Code lines not on the page | Median tokens | Median time |")
print("| --- | --- | --- | --- | --- |")
for name, rows in readers.items():
    hits = sum(truth_hit(i, rows[i]["text"]) for i in IDS)
    if name == "jurl -q":
        all_hits = sum(truth_hit(i, run[i]["text"]) for run in jurl_runs for i in IDS)
        hit_cell = f"{hits}/10 ({all_hits}/{10 * len(jurl_runs)} over {len(jurl_runs)} runs)"
    else:
        hit_cell = f"{hits}/10"
    if "search" in name:
        off = "—"
    else:
        lines = [(i, l) for i in IDS for l in code_lines(rows[i]["text"])]
        off = f"{sum(squash(l) not in pages[i] for i, l in lines)} of {len(lines)}"
    toks = int(st.median(len(enc.encode(rows[i]["text"])) for i in IDS))
    ms = [rows[i]["ms"] for i in IDS if rows[i].get("ms")]
    time_cell = f"{st.median(ms) / 1000:.1f} s" if ms else "—"
    print(f"| {name} | {hit_cell} | {off} | {toks:,} | {time_cell} |")
whole = R["whole-page"]
print(f'| Whole page as markdown | {sum(whole[i]["hit"] for i in IDS)}/10 | 0 | {int(st.median(whole[i]["tokens"] for i in IDS)):,} | {st.median(whole[i]["ms"] for i in IDS) / 1000:.1f} s |')

cost = [jurl_runs[0][i]["jev_tokens"] * JEV_PER_TOKEN for i in IDS]
print(f"\njurl cost per page: median ${st.median(cost):.4f}, max ${max(cost):.4f}")
print("Exa cost per call:", sorted({R["exa"][i][k]["cost"] for i in IDS for k in ("contents", "search")}))
