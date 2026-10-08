# Rewrites the question wording of a recording (record.py), so replay.py can send Jev the same questions worded another
# way and compare.py can show whether its answers depend on the wording. Only the per-item yes/no questions change, and
# only their `instructions`: l<n> (links, and a site map's), f<n> (link fields) and b<n> (blocks), each asked with the
# user's question. Ids, seq, the recorded response, status and latency stay; a request with none of these is copied
# byte for byte, so replay.py draws the same sample from both files.
#   python3 rewrite.py IN.jsonl OUT.jsonl --variant short|ref
# short: the question stays inline, with its owner note or pointer. The boilerplate shrinks by 30 chars for an l, 44 for
#   a site map's l, 44 for an f, 23 for a b.
# ref: the question is stated once, as state["question"] (a block's owner note included), and each instruction says
#   `question` instead. A link's "(read it as `asked_about` says)" becomes the sentence "Read `question` as
#   `asked_about` says.". Per question the boilerplate shrinks by 7 (l), 31 (site map, f) or 13 (b) chars without a
#   pointer; with one, l's stays as long and site map and f shrink by 24. The question's own chars leave each question
#   and come back once per request, with 14 chars of JSON.
# Measured on base-all.jsonl (1,193 requests, 2026-10-08): request chars -8.5% with short, -15.4% with ref.
# Purposes come from cost.classify on the original questions. Run cost.py on the recording, not on an output: a site
#   map's requests then count as links (their phrase is gone), and user_question reads no question from most questions.
#   Stdlib only; canon and classify are cost.py's.
import argparse, collections, json, os, re, sys

import cost

# The templates of src/links.rs (score) and src/main.rs (question). The id is the kind's letter and then i.
TEMPLATE = {
    "l": re.compile(r"Following the link in `links` with i=(\d+) is the page that answers this question, "
                    r"or leads to it: "),
    "site": re.compile(r"The page at the URL in `links` with i=(\d+) is the page that answers this question, "
                       r"or leads to it: "),
    "f": re.compile(r"Following the link in `links` with i=(\d+) is about the same field of knowledge as the answer "
                    r"to this question \(chemistry, astronomy, literature, medicine…\): "),
    "b": re.compile(r"The block in `blocks` with i=(\d+) helps answer this question: "),
}
LETTER = {"l": "l", "site": "l", "f": "f", "b": "b"}
SHORT = {
    "l": "Link i={i} in `links` leads to the answer, or is the page with it: ",
    "site": "Page i={i} in `links` has the answer, or leads to it: ",
    "f": "Link i={i} in `links` is in the field of knowledge of the answer "
         "(chemistry, astronomy, literature, medicine…): ",
    "b": "Block i={i} in `blocks` helps answer: ",
}
REF = {
    "l": "Following link i={i} in `links` leads to the answer to `question`, or is the page with it.",
    "site": "Page i={i} in `links` has the answer to `question`, or leads to it.",
    "f": "Link i={i} in `links` is in the field of knowledge of the answer to `question` "
         "(chemistry, astronomy, literature, medicine…).",
    "b": "Block i={i} in `blocks` helps answer `question`.",
}
POINTER = " (read it as `asked_about` says)"  # Ctx::ask_per_link: a link's question when the site has an owner
ASKED = " Read `question` as `asked_about` says."
# A question of ours (by these phrases) that no template fits is counted and left as it is.
UNMATCHED = re.compile(r"helps answer this question: |leads to it: |field of knowledge")


def template_of(text):  # (kind, match) or None
    for kind, rx in TEMPLATE.items():
        m = rx.match(text)
        if m:
            return kind, m
    return None


def rewrite(req, variant, seq):
    """Rewrites req's per-item questions in place. Returns the rewritten ones as (qid, kind, i, rest) and the number of
    questions that look like ours but match no template, or whose id is not their letter and i."""
    qs, found, odd = req["questions"], [], 0
    for qid, qq in qs.items():
        text = qq.get("instructions", "") if qq.get("type") == "noul" else ""
        hit = template_of(text)
        if hit is None:
            odd += bool(UNMATCHED.search(text))
        elif qid != LETTER[hit[0]] + hit[1].group(1):
            odd += 1
        else:
            found.append((qid, hit[0], hit[1].group(1), text[hit[1].end():]))
    if not found:
        return found, odd
    if variant == "short":
        for qid, kind, i, rest in found:
            qs[qid]["instructions"] = SHORT[kind].format(i=i) + rest
        return found, odd
    # ref: the question is stated once, so the rewritten questions of a request must all ask the same one
    ptr = {qid: kind != "b" and rest.endswith(POINTER) for qid, kind, _, rest in found}
    ask = {qid: rest[: -len(POINTER)] if ptr[qid] else rest for qid, _, _, rest in found}
    if len(set(ask.values())) > 1 or "question" in req["state"]:
        raise SystemExit(f"rewrite.py: request seq {seq}: its questions do not ask one question, or its state has one")
    for qid, kind, i, _ in found:
        qs[qid]["instructions"] = REF[kind].format(i=i) + (ASKED if ptr[qid] else "")
    req["state"]["question"] = next(iter(ask.values()))
    return found, odd


def report(a, read, rewritten, kinds, odd, per):
    tot = collections.Counter()
    for p in per.values():
        tot.update(p)
    overall = tot["tokens"] / tot["token_chars"] if tot["token_chars"] else 0.0
    print(f"rewrite.py --variant {a.variant}: {a.inp} -> {a.out}", file=sys.stderr)
    print(f"requests read {read:,}, rewritten {rewritten:,}; questions rewritten: "
          + (", ".join(f"{k} {n:,}" for k, n in kinds.items()) or "none"), file=sys.stderr)
    if odd:
        print(f"{odd:,} questions look like jurl's but match no template, or their id is not their i: left as they are",
              file=sys.stderr)
    print(f"request chars (canonical JSON): {tot['before']:,} -> {tot['after']:,} "
          f"({tot['after'] / tot['before'] - 1:+.1%})", file=sys.stderr)
    rows, est, star = [], 0.0, False
    for name, p in per.items():
        if p["requests"]:
            tpc = p["tokens"] / p["token_chars"] if p["token_chars"] else overall
            change = (p["after"] - p["before"]) * tpc
            est += change
            star |= not p["token_chars"]
            rows.append((name + ("*" if not p["token_chars"] else ""), p, tpc, change))
    rows.append(("all", tot, overall, est))
    print(f"{'purpose':10}{'requests':>10}{'rewritten':>11}{'chars before':>15}{'chars after':>13}{'tok/char':>10}"
          f"{'est. tokens':>14}", file=sys.stderr)
    for name, p, tpc, change in rows:
        print(f"{name:10}{p['requests']:>10,}{p['rewritten']:>11,}{p['before']:>15,}{p['after']:>13,}{tpc:>10.4f}"
              f"{change:>+14,.0f}", file=sys.stderr)
    if tot["tokens"]:
        print(f"recorded input tokens {tot['tokens']:,}; estimated change {est:+,.0f} ({est / tot['tokens']:+.1%}); "
              "tok/char is each purpose's own rate in the recording" + ("; * no token counts there, so the overall rate"
                                                                         if star else ""), file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description="Rewrites a recording's per-item question wording (see the header).")
    ap.add_argument("inp", metavar="IN.jsonl")
    ap.add_argument("out", metavar="OUT.jsonl")
    ap.add_argument("--variant", required=True, choices=["short", "ref"])
    a = ap.parse_args()
    if os.path.abspath(a.inp) == os.path.abspath(a.out):
        raise SystemExit("rewrite.py: OUT must not be IN")
    # per purpose: requests, rewritten, chars before and after, and tokens with the chars of the requests they count
    per = {p: collections.Counter() for p in cost.PURPOSES}
    kinds, read, rewritten, odd = collections.Counter(), 0, 0, 0
    with open(a.inp, encoding="utf-8", newline="\n") as fin, open(a.out, "w", encoding="utf-8", newline="\n") as fout:
        for line in fin:
            read += 1
            row = json.loads(line)
            req = row["request"]
            p = per[cost.classify(req["questions"])]  # before the rewrite: a site map's phrase goes with it
            before = len(cost.canon(req))
            found, bad = rewrite(req, a.variant, row["seq"])
            odd += bad
            p["requests"] += 1
            p["before"] += before
            p["after"] += len(cost.canon(req))
            if found:
                rewritten += 1
                p["rewritten"] += 1
                kinds.update(kind for _, kind, _, _ in found)
                fout.write(json.dumps(row, ensure_ascii=False) + "\n")
            else:
                fout.write(line)
            resp = row.get("response")
            usage = resp.get("usage") if isinstance(resp, dict) else None
            tokens = usage.get("input_tokens") if isinstance(usage, dict) else None
            if row.get("status") == 200 and isinstance(tokens, int):
                p["tokens"] += tokens
                p["token_chars"] += before
    if not read:
        raise SystemExit(f"rewrite.py: {a.inp} has no requests")
    report(a, read, rewritten, kinds, odd, per)


if __name__ == "__main__":
    main()
