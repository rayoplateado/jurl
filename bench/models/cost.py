# Where jurl's Jev input tokens go, in recordings from record.py: per request purpose (blocks, precise, refine,
# shortlist, links, sitemap, images), the characters of each request (state and its big list, page text, questions,
# the user's question and the owner note repeated across questions, the f<n> entries that copy l<n> entries), tokens
# per char, and tokens per search (by the user's question). Purposes come from the question ids jurl sends (src/main.rs,
# links.rs, follow.rs); a chunk past the first is a request of its own with only item ids (b<n>, l<n>, f<n>).
#   python3 cost.py results/jev-pricing.jsonl [more.jsonl ...] [--json]
# Only lines with status 200 and usage count. Python's standard library only, like the rest of bench/.
import argparse, collections, json, re, statistics as st

PURPOSES = ["blocks", "precise", "refine", "shortlist", "links", "sitemap", "images", "other"]
CHARS = ["total", "state", "list", "page_text", "questions", "user", "note", "fdup"]
# Where the user's question starts in a question's instructions (the templates of src/main.rs, links.rs, follow.rs);
# the leftmost delimiter wins. OWNER_TAIL strips the owner suffix (Ctx::ask, Ctx::ask_per_link) from the end.
DELIMS = ["helps answer this question: ", "leads to it: ", "medicine…): ", "leads to it? ", "nothing extra? ",
          "around it? ", "shows: "]
OWNER_TAIL = re.compile(r" \((?:Asked about .*|read it as `asked_about` says)\)$", re.S)
OWNER_NOTE = re.compile(r"Asked about \S+?: unless [^\n]*?writes about\.")  # Ctx::owner_note
SITEMAP = "The page at the URL in `links`"  # follow.rs run
USD_PER_MILLION = 0.042  # bench/models/README.md


def canon(x):
    return json.dumps(x, ensure_ascii=False, separators=(",", ":"))


def strings(x):  # every string inside x
    if isinstance(x, str):
        yield x
    elif isinstance(x, dict):
        yield from (s for v in x.values() for s in strings(v))
    elif isinstance(x, list):
        yield from (s for v in x for s in strings(v))


def nearest(xs, p):  # the nearest-rank pick of compare.py
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def classify(qs):
    ids = [k for k in qs if k != "blocked"]
    if "page_kind" in qs or any(re.fullmatch(r"b\d+", k) for k in ids):  # b<n> alone: a later chunk of a blocks page
        return "blocks"
    if "pick" in qs:
        return "precise"
    if "refine" in qs or "tighter" in qs:
        return "refine"
    if "next" in qs:
        return "shortlist"
    if any(re.fullmatch(r"img\d+", k) for k in ids):
        return "images"
    if any(re.fullmatch(r"[lf]\d+", k) for k in ids):  # an f-only chunk has no l question; a site map never asks f
        asked = [v.get("instructions", "") for k, v in qs.items() if re.fullmatch(r"[lf]\d+", k)]
        return "sitemap" if any(SITEMAP in t for t in asked) else "links"
    return "other"


def user_question(text):
    hits = [(text.index(d), d) for d in DELIMS if d in text]
    if not hits:
        return None
    start, d = min(hits)
    return OWNER_TAIL.sub("", text[start + len(d):]) or None


def analyze(label, r, tokens):
    req = r["request"]
    st_, qs = req.get("state", {}), req.get("questions", {})
    instr = [v.get("instructions", "") for v in qs.values()]
    users = [u for u in map(user_question, instr) if u]
    user = collections.Counter(users).most_common(1)[0][0] if users else None
    note = next((m.group(0) for s in strings(req) if (m := OWNER_NOTE.search(s))), None)
    texts = [t for v in qs.values() for t in [v.get("instructions", "")] + list(strings(v.get("criteria", {})))]
    big = max([k for k, v in st_.items() if isinstance(v, list)], key=lambda k: len(canon(st_[k])), default=None)
    entries = st_[big] if big else []
    fs = [e for e in entries if "context" not in e] if any(re.fullmatch(r"f\d+", k) for k in qs) else []
    url = st_.get("url")
    chars = {"total": len(canon(req)), "state": len(canon(st_)), "list": len(canon(st_[big])) if big else 0,
             "page_text": len(canon(st_["page_text"])) if "page_text" in st_ else 0, "questions": len(canon(qs)),
             "user": sum(t.count(user) for t in texts) * len(user) if user else 0,
             "note": sum(s.count(note) for s in strings(req)) * len(note) if note else 0,
             "fdup": sum(len(canon(e)) for e in fs)}
    return {"label": label, "purpose": classify(qs), "user": user, "mixed": len(set(users)) > 1, "tokens": tokens,
            "questions": sum(k != "blocked" for k in qs), "kinds": sorted({re.sub(r"\d+", "", k) for k in qs}),
            "first": next((v.get("instructions", "") for k, v in qs.items() if k != "blocked"), ""), "chars": chars,
            "l_keys": [(url, e.get("i"), e.get("text"), e.get("path")) for e in entries
                       if big == "links" and "context" in e],
            "f_keys": [(url, e.get("i"), e.get("text"), e.get("path")) for e in fs]}


def summarise(rows):
    toks = [r["tokens"] for r in rows]
    return {"requests": len(rows), "tokens": sum(toks), "median": st.median(toks) if toks else None,
            "p90": nearest(toks, 0.9), "questions": st.mean(r["questions"] for r in rows) if rows else None,
            "chars": {k: sum(r["chars"][k] for r in rows) for k in CHARS}}


def build(paths):
    lines, skipped, direct, rows = 0, 0, 0, []
    for path in paths:
        for n, line in enumerate(open(path), 1):
            lines += 1
            r = json.loads(line)
            resp = r.get("response")
            usage = resp.get("usage") if isinstance(resp, dict) else None
            tokens = usage.get("input_tokens") if isinstance(usage, dict) else None
            if r.get("status") != 200 or not isinstance(tokens, int) or not isinstance(r.get("request"), dict):
                skipped += 1
                continue
            direct += tokens
            rows.append(analyze(f"{path}#{r.get('seq', n)}", r, tokens))
    total = summarise(rows)
    purposes = {}
    for p in PURPOSES:
        purposes[p] = summarise([x for x in rows if x["purpose"] == p])
        purposes[p]["share"] = purposes[p]["tokens"] / total["tokens"] if total["tokens"] else 0.0
    groups = collections.defaultdict(list)
    for x in rows:
        groups[x["user"] or "(no question)"].append(x)
    searches = {}
    for q, xs in sorted(groups.items(), key=lambda kv: -sum(x["tokens"] for x in kv[1])):
        s = summarise(xs)
        s["shares"] = {p: sum(x["tokens"] for x in xs if x["purpose"] == p) / s["tokens"]
                       for p in PURPOSES if any(x["purpose"] == p for x in xs)} if s["tokens"] else {}
        s["usd"] = s["tokens"] * USD_PER_MILLION / 1e6
        searches[q] = s
    l_keys = {k for x in rows for k in x["l_keys"]}
    f_keys = [k for x in rows for k in x["f_keys"]]
    other = [x for x in rows if x["purpose"] == "other"]
    checks = {
        "purposes add up to the file's tokens": sum(purposes[p]["tokens"] for p in PURPOSES) == direct,
        "searches add up to the file's tokens": sum(s["tokens"] for s in searches.values()) == direct,
        "token shares sum to 100%": not rows or abs(sum(d["share"] for d in purposes.values()) - 1) < 1e-9,
        "shares by search sum to 100%": all(abs(sum(s["shares"].values()) - 1) < 1e-9
                                            for s in searches.values() if s["tokens"]),
    }
    return {"files": paths, "lines": lines, "kept": len(rows), "skipped": skipped, "tokens": direct,
            "usd": direct * USD_PER_MILLION / 1e6, "all": {**total, "share": 1.0 if rows else 0.0},
            "purposes": purposes, "searches": searches,
            "f_entries": {"total": len(f_keys), "matched_to_an_l_entry": sum(k in l_keys for k in f_keys)},
            "other": {"requests": len(other), "kinds": sorted({k for x in other for k in x["kinds"]}),
                      "examples": [[x["label"], x["first"][:120]] for x in other[:3]]},
            "mixed_user_requests": sum(x["mixed"] for x in rows), "checks": checks}


def pc(x):
    return "-" if x is None else f"{100 * x:.1f}%"


def num(x, fmt="{:,.0f}"):
    return "-" if x is None else fmt.format(x)


def ratio(a, b, fmt):
    return fmt.format(a / b) if b else "-"


def show(title, cols, metrics):  # metrics: (label, f(column) -> text), one row each
    w = max(len(label) for label, _ in metrics) + 2
    cw = max([11] + [len(c) + 2 for c in cols])
    print(f"\n{title}\n{'':{w}}" + "".join(f"{c:>{cw}}" for c in cols))
    for label, f in metrics:
        print(f"{label:{w}}" + "".join(f"{f(c):>{cw}}" for c in cols))


def render(rep):
    if not rep["kept"]:
        print(f"no request with status 200 and usage in {len(rep['files'])} file(s): nothing to count")
        return
    D = {**rep["purposes"], "all": rep["all"]}  # one column per purpose, then all
    cols = [k for k in PURPOSES if rep["purposes"][k]["requests"]] + ["all"]

    def frac(c, f, base="total"):  # f(chars) as a share of the column's chars[base]
        ch = D[c]["chars"]
        return pc(f(ch) / ch[base]) if ch[base] else "-"

    show("1. requests and input tokens by purpose", cols, [
        ("requests", lambda c: f"{D[c]['requests']:,}"),
        ("input tokens", lambda c: f"{D[c]['tokens']:,}"),
        ("share of all tokens", lambda c: pc(D[c]["share"])),
        ("median tokens per request", lambda c: num(D[c]["median"])),
        ("p90 tokens per request", lambda c: num(D[c]["p90"])),
        ("questions per request (not blocked)", lambda c: num(D[c]["questions"], "{:.1f}")),
    ])
    show("2. request characters (canonical JSON; share of the purpose's request chars)", cols, [
        ("request chars", lambda c: f"{D[c]['chars']['total']:,}"),
        ("state", lambda c: frac(c, lambda ch: ch["state"])),
        ("  big list (blocks, links, images)", lambda c: frac(c, lambda ch: ch["list"])),
        ("  rest of state", lambda c: frac(c, lambda ch: ch["state"] - ch["list"])),
        ("    of which page_text", lambda c: frac(c, lambda ch: ch["page_text"])),
        ("questions", lambda c: frac(c, lambda ch: ch["questions"])),
        ("wrapper (model, keys; remainder)", lambda c: frac(c, lambda ch: ch["total"] - ch["state"] - ch["questions"])),
    ])
    show("2b. repeats (user question: % of question chars; the rest: % of request chars)", cols, [
        ("user question in the questions", lambda c: frac(c, lambda ch: ch["user"], "questions")),
        ("owner note, state or questions", lambda c: frac(c, lambda ch: ch["note"])),
        ("f<n> entries copying an l<n> entry", lambda c: frac(c, lambda ch: ch["fdup"])),
    ])
    show("3. tokens per character (saved chars times this is saved tokens)", cols, [
        ("input tokens", lambda c: f"{D[c]['tokens']:,}"),
        ("request chars", lambda c: f"{D[c]['chars']['total']:,}"),
        ("tokens per char", lambda c: ratio(D[c]["tokens"], D[c]["chars"]["total"], "{:.4f}")),
        ("chars per token", lambda c: ratio(D[c]["chars"]["total"], D[c]["tokens"], "{:.2f}")),
    ])
    shown = [k for k in PURPOSES if rep["purposes"][k]["requests"]]

    def cell(s, c):  # one search's cell in table 4
        if c in ("requests", "tokens"):
            return f"{s[c]:,}"
        return f"${s['usd']:.4f}" if c == "usd" else pc(s["shares"].get(c))

    show("4. by search (the user's question; share of its tokens by purpose)", ["requests", "tokens", "usd"] + shown,
         [(q[:60], lambda c, s=s: cell(s, c)) for q, s in rep["searches"].items()])
    fe, other = rep["f_entries"], rep["other"]
    print(f"\nfiles: {len(rep['files'])}, lines {rep['lines']}, kept {rep['kept']} (status 200 with usage), "
          f"skipped {rep['skipped']}")
    print(f"total: {rep['all']['requests']:,} requests, {rep['tokens']:,} input tokens, "
          f"${rep['usd']:.4f} at ${USD_PER_MILLION} per million")
    print(f"f<n> entries: {fe['total']:,}, {fe['matched_to_an_l_entry']:,} copy an l<n> entry of the same page")
    if other["requests"]:
        print(f"other: {other['requests']} requests, id kinds {other['kinds']}; e.g. {other['examples']}")
    if rep["mixed_user_requests"]:
        print(f"{rep['mixed_user_requests']} requests have more than one user question; the most common is used")
    for name, ok in rep["checks"].items():
        print(f"check: {name}: {'ok' if ok else 'FAILED'}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--json", action="store_true", help="print the same numbers as JSON")
    a = ap.parse_args()
    rep = build(a.files)
    if a.json:
        print(json.dumps(rep, ensure_ascii=False, indent=1))
    else:
        render(rep)
    if not all(rep["checks"].values()):
        raise SystemExit("cost.py: a check failed")


if __name__ == "__main__":
    main()
