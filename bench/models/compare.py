# Agreement with Jev, side by side, on replays (replay.py) of one recording: one column per NAME=FILE. A replay to
# Jev itself is the noise ceiling: no other model can agree with Jev more than Jev agrees with itself.
#   python3 compare.py "Jev again=results/jev2.jsonl" "H2O=results/h2o.jsonl" [--pairs results/pairs.jsonl]
# Only requests every file answered (HTTP 200) are compared, so the columns are about the same questions.
# Thresholds are jurl's (src/main.rs, src/links.rs): a block is searched for the answer from 0.1 (top 3), a page is
# a block page from 0.8 ("blocked"), a link is kept from 0.5, links rank by p + (1 - p) * 0.3 * f. `f` has no
# threshold in jurl (0.5 here). Choice questions compare the top choice only.
# NAME=FILE:KEY reads the answers in field KEY (default: the one field with a KEY_latency_s next to it, not jev).
import argparse, collections, json, math, statistics as st

THRESHOLD = {"b": 0.1, "l": 0.5, "f": 0.5, "blocked": 0.8, "img": 0.5}
CHOICES = ["page_kind", "pick", "next", "tighter"]
FIELD_WEIGHT = 0.3
BLOCK_FLOOR, BLOCK_TOP = 0.1, 3

ap = argparse.ArgumentParser()
ap.add_argument("columns", nargs="+", metavar="NAME=FILE[:KEY]")
ap.add_argument("--pairs", help="write every question's two answers here, most different first")
a = ap.parse_args()


def kind(qid):
    # b12 → b, l7 → l, f7 → f, img3 → img; page_kind, pick, next, tighter, blocked stay as they are
    return qid.rstrip("0123456789")


def tokens(resp):
    return resp.get("usage", {}).get("input_tokens", 0)


def q(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else float("nan")


def rank(xs):
    order = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and xs[order[j + 1]] == xs[order[i]]:
            j += 1
        for k in range(i, j + 1):
            r[order[k]] = (i + j) / 2
        i = j + 1
    return r


def spearman(x, y):
    if len(x) < 3:
        return None
    rx, ry = rank(x), rank(y)
    mx, my = st.mean(rx), st.mean(ry)
    den = math.sqrt(sum((u - mx) ** 2 for u in rx) * sum((v - my) ** 2 for v in ry))
    return sum((u - mx) * (v - my) for u, v in zip(rx, ry)) / den if den else None


def load(spec):
    name, rest = spec.split("=", 1)
    path, key = rest.rsplit(":", 1) if ":" in rest else (rest, "")
    rows = [json.loads(line) for line in open(path)]
    if not key:
        keys = {k[: -len("_latency_s")] for r in rows[:1] for k in r if k.endswith("_latency_s")} - {"jev"}
        if len(keys) != 1:
            raise SystemExit(f"{path}: say which field holds the answers (NAME=FILE:KEY), found {sorted(keys)}")
        key = keys.pop()
    ok = {r["seq"]: r for r in rows if r["status"] == 200 and isinstance(r.get(key), dict) and "answers" in r[key]}
    return name, key, len(rows), ok


cols = [load(c) for c in a.columns]
common = sorted(set.intersection(*(set(ok) for *_, ok in cols)))
print(f"{len(common)} requests answered in every file")
pairs = []


def metrics(name, key, n_rows, ok):
    rows = [ok[s] for s in common]
    m = {"requests ok": f"{len(ok)}/{n_rows}"}
    noul, choice, per_req = collections.defaultdict(list), collections.defaultdict(list), []
    for r in rows:
        ja, ca = r["jev"]["answers"], r[key]["answers"]
        req = {"l": [], "b": [], "f": []}
        for qid, qq in r["request"]["questions"].items():
            j, c = ja.get(qid), ca.get(qid)
            if j is None or c is None:
                continue
            if qq.get("type") == "noul":
                k = kind(qid)
                noul[k].append((j["noul"], c["noul"]))
                if k in req:
                    req[k].append((qid, j["noul"], c["noul"]))
                pairs.append({"name": name, "seq": r["seq"], "qid": qid, "jev": j["noul"], name: c["noul"],
                              "diff": abs(j["noul"] - c["noul"]), "instructions": qq.get("instructions")})
            else:
                same = j.get("choice") == c.get("choice")
                choice[qid].append(same)
                pairs.append({"name": name, "seq": r["seq"], "qid": qid, "jev": j.get("probabilities"),
                              name: c.get("probabilities"), "diff": 0.0 if same else 1.0,
                              "instructions": qq.get("instructions"), "criteria": qq.get("criteria")})
        per_req.append(req)
    for k in sorted(noul, key=lambda k: (k not in THRESHOLD, list(THRESHOLD).index(k) if k in THRESHOLD else 0, k)):
        v, t = noul[k], THRESHOLD.get(k, 0.5)
        m[f"{k}: MAE"] = f"{st.mean(abs(x - y) for x, y in v):.3f}"
        m[f"{k}: same side of {t}"] = f"{sum((x >= t) == (y >= t) for x, y in v) / len(v):.1%}"
        m[f"{k}: n >= {t} (jev {sum(x >= t for x, _ in v)})"] = str(sum(y >= t for _, y in v))
        m[f"{k}: mean (jev {st.mean(x for x, _ in v):.3f})"] = f"{st.mean(y for _, y in v):.3f}"
    # what jurl does with the scores: links ranked (the best are followed), blocks' top 3 from 0.1 searched
    for k, label in (("l", "links"), ("b", "blocks")):
        top1, jaccard, rho, n = 0, [], [], 0
        for req in per_req:
            xs = req[k]
            if len(xs) < 3:
                continue
            n += 1
            if k == "l" and req["f"]:
                f = {qid[1:]: (x, y) for qid, x, y in req["f"]}
                xs = [(qid, x + (1 - x) * FIELD_WEIGHT * f.get(qid[1:], (0, 0))[0],
                       y + (1 - y) * FIELD_WEIGHT * f.get(qid[1:], (0, 0))[1]) for qid, x, y in xs]
            jt, ct = sorted(xs, key=lambda x: -x[1]), sorted(xs, key=lambda x: -x[2])
            if k == "b":
                jt, ct = [x for x in jt if x[1] >= BLOCK_FLOOR], [x for x in ct if x[2] >= BLOCK_FLOOR]
            j3, c3 = {x[0] for x in jt[:BLOCK_TOP]}, {x[0] for x in ct[:BLOCK_TOP]}
            top1 += bool(jt and ct and jt[0][0] == ct[0][0]) or (not jt and not ct)
            jaccard.append(len(j3 & c3) / len(j3 | c3) if j3 | c3 else 1.0)
            s = spearman([x[1] for x in xs], [x[2] for x in xs])
            if s is not None:
                rho.append(s)
        if n:
            m[f"{label}: same top-1 (n={n})"] = f"{top1}/{n} ({top1 / n:.0%})"
            m[f"{label}: top-3 Jaccard"] = f"{st.mean(jaccard):.2f}"
            m[f"{label}: Spearman median"] = f"{st.median(rho):.2f}" if rho else "-"
    for c in sorted(choice, key=lambda c: (CHOICES.index(c) if c in CHOICES else len(CHOICES), c)):
        m[f"choice {c}: same top"] = f"{sum(choice[c])}/{len(choice[c])}"
    lat = [r[f"{key}_latency_s"] for r in rows]
    if lat:
        m["latency p50"], m["latency p95"], m["latency max"] = f"{q(lat, .5):.2f}s", f"{q(lat, .95):.2f}s", f"{max(lat):.1f}s"
    size = collections.defaultdict(list)
    for r, s in zip(rows, lat):
        size[min(tokens(r["jev"]) // 5000, 3)].append(s)
    for b in sorted(size):
        m[f"latency p50, Jev tokens {b * 5}k{'+' if b == 3 else '-' + str(b * 5 + 5) + 'k'}"] = f"{st.median(size[b]):.2f}s (n={len(size[b])})"
    jt, ct = sum(tokens(r["jev"]) for r in rows), sum(tokens(r[key]) for r in rows)
    m[f"input tokens sum (Jev {jt})"] = f"{ct} ({ct / jt:.2f}x)" if jt else str(ct)
    if sum(lat):
        m["throughput tok/s"] = f"{ct / sum(lat):.0f}"
    return m


table = [(name, metrics(name, key, n, ok)) for name, key, n, ok in cols]
labels = list(dict.fromkeys(k for _, m in table for k in m))
w = max(map(len, labels)) + 2
cw = max(20, max(len(n) for n, _ in table) + 2)
print(f"{'':{w}}" + "".join(f"{n:>{cw}}" for n, _ in table))
for k in labels:
    print(f"{k:{w}}" + "".join(f"{m.get(k, '-'):>{cw}}" for _, m in table))

if a.pairs:
    with open(a.pairs, "w") as f:
        for p in sorted(pairs, key=lambda p: -p["diff"]):
            f.write(json.dumps(p, ensure_ascii=False) + "\n")
