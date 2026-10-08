# Compares the requests of two recordings from record.py: how many each has, how many distinct request bodies are in
# both (canonical JSON, matched the way record.py --from matches them), and for each body the two ask a different number
# of times: what it asks (purpose by question ids, as cost.py classifies them), the page and the number of questions.
# Exits 1 when they differ. Status is ignored: a request that failed was still asked.
#   python3 diff.py results/base.jsonl results/check.jsonl
# Stdlib only; imports classify from cost.py, next to it.
import argparse, collections, json
from cost import classify

ap = argparse.ArgumentParser()
ap.add_argument("base")
ap.add_argument("other")
a = ap.parse_args()


def canon(x):  # record.py's canonical JSON
    return json.dumps(x, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def describe(req):  # purpose, page, number of questions
    if not isinstance(req, dict):
        return "not a JSON object"
    qs = req.get("questions", {})
    return f"{classify(qs)}, {req.get('state', {}).get('url')}, {len(qs)} questions"


def load(path):  # canonical body -> how many times it was asked, and what it asks
    counts, what = collections.Counter(), {}
    for line in open(path):
        req = json.loads(line)["request"]
        key = canon(req)
        counts[key] += 1
        what.setdefault(key, describe(req))
    return counts, what


(ca, da), (cb, db) = load(a.base), load(a.other)
print(f"{a.base}: {sum(ca.values())} requests, {len(ca)} distinct bodies")
print(f"{a.other}: {sum(cb.values())} requests, {len(cb)} distinct bodies")
print(f"in both: {len(ca.keys() & cb.keys())} bodies")
differ = [k for k in {**ca, **cb} if ca[k] != cb[k]]
for k in differ:
    print(f"  {a.base} {ca[k]}x, {a.other} {cb[k]}x: {(da if k in da else db)[k]}")
print("differ" if differ else "same")
raise SystemExit(1 if differ else 0)
