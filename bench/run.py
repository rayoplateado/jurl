# Runs every reader on tasks.json and saves what each one hands an agent to results/.
#
#   uv run --with tiktoken --with requests bench/run.py jurl [runs]   # needs TYPESAFE_API_KEY (or `jurl init`)
#   uv run --with tiktoken --with requests bench/run.py whole-page    # the page as markdown, via Jina Reader
#   uv run --with tiktoken --with requests bench/run.py exa           # needs EXA_API_KEY
#   uv run --with tiktoken --with requests bench/run.py tavily        # needs TAVILY_API_KEY
#
# Claude Code's WebFetch can't be called from a script: results/webfetch.json holds what it answered when called
# from Claude Code with the task's URL and its question as the prompt.

import json, os, re, subprocess, sys, time
from pathlib import Path
from urllib.parse import urlparse

import requests, tiktoken

HERE = Path(__file__).parent
TASKS = json.loads((HERE / "tasks.json").read_text())
enc = tiktoken.get_encoding("cl100k_base")
norm = lambda s: re.sub(r"\s+", " ", s).lower()


def scored(t, text, ms, **extra):
    """What every reader is judged on: is the exact answer in it, and how many tokens does the agent read."""
    hit = bool(text) and all(norm(s) in norm(text) for s in t["truth"])
    return {"hit": hit, "tokens": len(enc.encode(text)), "ms": ms, "text": text, **extra}


def timed(fn):
    t0 = time.time()
    out = fn()
    return out, round((time.time() - t0) * 1000)


def jurl(runs=3):
    exe = os.environ.get("JURL", "jurl")
    out = {"runs": []}
    for _ in range(runs):
        run = {}
        for t in TASKS:
            p, ms = timed(lambda: subprocess.run([exe, "-q", t["q"], "-t", t["url"]], capture_output=True, text=True, timeout=120))
            m = re.search(r"(\d+) tok", p.stderr)
            run[t["id"]] = scored(t, p.stdout, ms, jev_tokens=int(m.group(1)) if m else None)
        out["runs"].append(run)
    return out


def whole_page():
    out = {}
    for t in TASKS:
        res, ms = timed(lambda: requests.get("https://r.jina.ai/" + t["url"], headers={"X-Return-Format": "markdown"}, timeout=90))
        row = scored(t, res.text, ms)
        del row["text"]  # the whole page is large; it's re-fetched when scoring
        out[t["id"]] = row
    return out


def exa():
    h = {"x-api-key": os.environ["EXA_API_KEY"], "content-type": "application/json"}
    view = lambda results: "\n\n".join(f'# {r.get("title", "")}\n<{r.get("url", "")}>\n\n' + "\n\n".join(r.get("highlights") or []) for r in results)
    out = {}
    for t in TASKS:
        hl = {"query": t["q"], "numSentences": 3, "highlightsPerUrl": 3}
        row = {}
        for name, path, body in [
            ("contents", "contents", {"urls": [t["url"]], "highlights": hl, "livecrawl": "fallback"}),
            ("search", "search", {"query": t["q"], "includeDomains": [urlparse(t["url"]).hostname], "numResults": 3, "contents": {"highlights": hl}}),
        ]:
            res, ms = timed(lambda: requests.post(f"https://api.exa.ai/{path}", headers=h, json=body, timeout=60))
            data = res.json()
            row[name] = scored(t, view(data.get("results", [])) if res.ok else "", ms, cost=(data.get("costDollars") or {}).get("total"))
        out[t["id"]] = row
    return out


def tavily():
    h = {"authorization": f'Bearer {os.environ["TAVILY_API_KEY"]}', "content-type": "application/json"}
    out = {}
    for t in TASKS:
        res, ms = timed(lambda: requests.post("https://api.tavily.com/extract", headers=h, timeout=90,
                                              json={"urls": [t["url"]], "query": t["q"], "chunks_per_source": 3, "format": "markdown"}))
        extract = scored(t, "\n\n".join(r.get("raw_content", "") for r in res.json().get("results", [])), ms)
        res, ms = timed(lambda: requests.post("https://api.tavily.com/search", headers=h, timeout=90,
                                              json={"query": t["q"], "include_domains": [urlparse(t["url"]).hostname], "max_results": 3}))
        text = "\n\n".join(f'# {r.get("title", "")}\n<{r.get("url", "")}>\n\n{r.get("content", "")}' for r in res.json().get("results", []))
        out[t["id"]] = {"extract": extract, "search": scored(t, text, ms)}
    return out


if __name__ == "__main__":
    which = sys.argv[1]
    fn = {"jurl": lambda: jurl(int(sys.argv[2]) if len(sys.argv) > 2 else 3), "whole-page": whole_page, "exa": exa, "tavily": tavily}[which]
    (HERE / "results").mkdir(exist_ok=True)
    (HERE / "results" / f"{which}.json").write_text(json.dumps(fn(), indent=1))
