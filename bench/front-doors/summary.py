# Markdown tables of the rounds, for RESULTS.md and the PR: per round, per sheet (the real-world sheets first), with
# correct / wrong / not found, mean pages, mean tokens used and billed, and the cost. Also the not-found classes.
#   python3 summary.py baseline step1 step2 ...
import collections, json, pathlib, re, statistics as st, sys

HERE = pathlib.Path(__file__).resolve().parent
JUDGE = json.loads((HERE / "judgements.json").read_text(encoding="utf-8"))
NF = json.loads((HERE / "notfound.json").read_text(encoding="utf-8"))
ORDER = ["R1", "R2", "R3", "R4", "R5", "R6", "R6es", "S1", "S2", "S3"]


def norm(s):
    return re.sub(r"\s+", " ", (s or "").strip())


def cells(round_name, earlier=()):
    """The cells of a round, judged. A not-found cell takes its class from this round's notes, else from the latest earlier
    round in which the same cell was not found (the same reason, unless the notes say the round changed it)."""
    out = []
    for p in sorted((HERE / "results" / round_name).glob("*.json")):
        r = json.loads(p.read_text(encoding="utf-8"))
        cell = f"{r['sheet']}-{r['row']}-{r['q']}"
        if r["answer"]:
            j = JUDGE.get(f"{r['front']}|{r['question']}|{norm(r['answer'])}")
            v = j["verdict"] if j else "unjudged"
            cls = "wrong_answer" if v == "wrong" else ""
        else:
            v = "not_found"
            cls = NF.get(f"{round_name}|{cell}", {}).get("class")
            for earlier_round in reversed(earlier):
                if cls:
                    break
                cls = NF.get(f"{earlier_round}|{cell}", {}).get("class")
                if cls:
                    cls = cls + " (carried)"
            cls = cls or "unclassified"
        out.append({**r, "verdict": v, "class": cls})
    return out


def table(round_name, items, title):
    lines = [f"**{title}**", "", "| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    total = []
    for s in ORDER + sorted({r["sheet"] for r in items} - set(ORDER)):
        rs = [r for r in items if r["sheet"] == s]
        if not rs:
            continue
        total += rs
        lines.append(row(s, rs))
    lines.append(row("**all**", total))
    return "\n".join(lines)


def row(name, rs):
    c = sum(r["verdict"] == "correct" for r in rs)
    w = sum(r["verdict"] == "wrong" for r in rs)
    n = sum(r["verdict"] == "not_found" for r in rs)
    pages = [r["pages"] for r in rs if r["pages"] is not None]
    used = [r["tokens_used"] for r in rs]
    billed = sum(r["tokens_billed"] for r in rs)
    return (f"| {name} | {len(rs)} | {c} | {w} | {n} | {st.mean(pages) if pages else 0:.1f} | "
            f"{st.mean(used) if used else 0:,.0f} | {billed:,} | {billed * 0.042 / 1e6:.4f} |")


def classes(round_name, items):
    counts = collections.Counter(r["class"] for r in items if r["verdict"] in ("not_found", "wrong"))
    return ", ".join(f"{k} {v}" for k, v in sorted(counts.items(), key=lambda kv: -kv[1]))


if __name__ == "__main__":
    rounds = sys.argv[1:]
    for i, rd in enumerate(rounds):
        items = cells(rd, earlier=rounds[:i])
        print(table(rd, items, f"round {rd}"))
        print()
        print(f"failures by class ({rd}): {classes(rd, items)}")
        print()
