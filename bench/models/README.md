# Another decision model in Jev's place

jurl asks Jev two kinds of questions: yes/no (`noul`, a probability) and pick-one (`choice`). Any server with the same contract can stand in for it (`JURL_JEV_URL`):

```
POST /v1/systemone  {state, model, questions}  →  {answers: {id: {noul} | {choice, confidence, probabilities}}, usage: {input_tokens}}
```

These scripts measure how often a candidate (another model, or Jev with other settings) makes the same decisions Jev makes, on the requests jurl really sends. Jev isn't fully deterministic, so the same requests are also sent to Jev again: **Jev against Jev is the noise ceiling**. No candidate can agree with Jev more than that, and a gap to it is the candidate's.

- [record.py](record.py): a proxy between jurl and Jev that saves every request and answer (JSONL).
- [replay.py](replay.py): sends the recorded requests to a candidate and saves its answers next to Jev's.
- [compare.py](compare.py): one column per replay. Python's standard library only, like the rest of `bench/`.

## What compare.py counts

The question's id says what jurl does with the answer, and the thresholds are jurl's own:

| id | Question | Threshold | Where |
|---|---|---|---|
| `b12` | Does block 12 answer the question? | 0.1, then the top 3 are searched for the answer | `PRECISE_BLOCK_FLOOR`, `top(…, 0.1, 3)` in src/main.rs |
| `l7` | Does link 7 lead to the answer? | 0.5 for `--links`; `--follow` ranks by `p + (1 - p) × 0.3 × f` | `--threshold`, `FIELD_WEIGHT` in src/links.rs |
| `f7` | Is link 7 in the answer's field? (`--follow 10` and up) | none in jurl; 0.5 here | src/links.rs |
| `blocked` | Is this a block page (rate limit, bot check)? | 0.8 | `BLOCKED_P` |
| `page_kind`, `pick`, `next`, `tighter` | choices | top choice only | src/main.rs, src/follow.rs |

For each `noul` kind: mean absolute difference, how often both are on the same side of the threshold, and how many pass it. For each request: whether the best link and the best block are the same, the top 3's overlap (Jaccard) and rank correlation (Spearman). Then latency (p50, p95, by request size) and input tokens. Two approximations: `pick` also adds up nested spans and `tighter` needs confidence 0.5 in jurl; here both are just the top choice.

## Workflow

1. **Record** one `follow.py` run through the proxy, only the groups you need (Jev's key goes through, it's never written):

   ```sh
   python3 bench/models/record.py --out bench/models/results/jev-pricing.jsonl &      # → Jev, on port 18100
   JURL_JEV_URL=http://127.0.0.1:18100/v1/systemone JURL=target/release/jurl GROUP=pricing \
     python3 bench/follow.py jev-pricing 1
   ```

   Appending to the same file continues its numbering, so `GROUP=docs` afterwards makes one corpus.

2. **Replay** to the candidate. `--sample N` takes a stratified sample (by question kind and request size, plus up to 40 requests with a choice question); it's seeded, so every candidate gets the same requests. `--same-as` takes exactly another replay's.

   ```sh
   python3 bench/models/replay.py bench/models/results/jev-pricing.jsonl bench/models/results/cand.jsonl \
     http://127.0.0.1:8000/v1/systemone --name cand --sample 160
   ```

   Keep `--concurrency 1` (the default) when latency matters. `--user-agent` is `curl/8.7.1` in both scripts: Cloudflare, in front of Jev and Runpod's proxy, rejects Python's.

3. **The noise ceiling**, once per corpus (this one calls Jev, see below):

   ```sh
   REPLAY_KEY=$TYPESAFE_API_KEY python3 bench/models/replay.py bench/models/results/jev-pricing.jsonl \
     bench/models/results/jev-again.jsonl https://api.typesafe.ai/v1/systemone --name jev2 --same-as bench/models/results/cand.jsonl
   ```

4. **Compare**:

   ```sh
   python3 bench/models/compare.py "Jev-vs-Jev=bench/models/results/jev-again.jsonl" "Jev-vs-cand=bench/models/results/cand.jsonl" \
     --pairs bench/models/results/pairs.jsonl | tee bench/models/results/$(date +%F)-cand.txt
   ```

   `--pairs` writes every question's two answers, most different first: where to look when tuning.

5. **End to end**, when the agreement is close: `follow.py` with jurl pointed at the candidate. A slow self-hosted model needs one search at a time and more time per search (a search that times out counts as wrong):

   ```sh
   env -u TYPESAFE_API_KEY JURL_JEV_URL=http://127.0.0.1:8000/v1/systemone POOL=1 TIMEOUT=1800 \
     JURL=target/release/jurl python3 bench/follow.py cand 1
   ```

   **Unset `TYPESAFE_API_KEY`** whenever `JURL_JEV_URL` points anywhere but your own proxy to Jev: jurl sends the key it has to whatever server that is. A key in `~/.config/jurl/env` or `./.env` is sent too: point `XDG_CONFIG_HOME` at an empty directory and run from one without a `.env`.

Recordings, replays and pairs (`results/*.jsonl`) hold the pages' text and are large (156 MB on 2026-10-08): they're ignored by git. Summaries (`results/*.txt`) are committed.

## What it costs

Jev is $0.042 per million input tokens. Keep each change within **$0.30**.

- One full `follow.py` run (28 searches) through the proxy reads ~11.9M tokens: **~$0.50**, over the budget. Record only the groups the change is about: one run of pricing is 0.70M tokens ($0.03), docs 0.46M, wikipedia 0.56M, trap 0.32M, long 4.1M ($0.17), heldout 5.8M ($0.24).
- Reuse recordings: one corpus serves every candidate and every setting of it.
- Replays to a candidate cost nothing on Jev. The noise-ceiling replay is a Jev call: 160 requests were 1.45M tokens ($0.06). Do it once per corpus and keep the file.
- Never set `REPLAY_KEY` for a candidate.

## Not here

Two servers used on 2026-10-08 stay out, because neither runs without things outside this repo: an MLX look-alike of the four vLLM endpoints H2O's own Jev-contract shim calls (tuned to a 36 GB Mac), and a Jev-contract server around LiquidAI's d1-3B (PyTorch on Apple's MPS, its model's own `system_one`). Any server with the contract above works.

## Results, 2026-10-08

One `follow.py` run (28 searches) recorded through Jev: 1,232 requests. Candidates: H2O (`h2oai/h2o-lightning-4b` behind H2O's Jev-contract shim) and LiquidAI's d1-3B.

The same 160 requests ([table](results/2026-10-08-jev-h2o-mlx-d1.txt); H2O on MLX on a Mac, d1 on MPS, so their latencies are the Mac's):

| | Jev-vs-Jev | Jev-vs-H2O | Jev-vs-d1 |
|---|---|---|---|
| b: same side of 0.1 | 98.7% | 94.1% | 49.5% |
| f: same side of 0.5 | 98.5% | 65.2% | 71.2% |
| links: same top-1 (n=49) | 36/49 (73%) | 21/49 (43%) | 6/49 (12%) |
| links: top-3 Jaccard | 0.60 | 0.22 | 0.13 |
| blocks: same top-1 (n=57) | 52/57 (91%) | 42/57 (74%) | 30/57 (53%) |
| choice pick: same top | 20/20 | 12/20 | 10/20 |
| latency p50 | 0.34s | 24.51s | 15.24s |

H2O on an RTX 4090 (Runpod, vLLM) on the same requests ([table](results/2026-10-08-jev-h2o-gpu.txt)): blocks same top-1 43/57 (75%), links 19/49 (39%), `f` same side 65.0%, pick 12/20; latency p50 5.87s, p95 23.07s, against Jev's 0.31s and 0.45s. (That file's last column, "floored", is a probability floor of 0.801 tried in the shim that day; compare.py has no such column.)

End to end, one run of the 28 searches each ([Jev](../results/follow-models-jev.json), [H2O on the RTX 4090](../results/follow-models-h2o-gpu.json); Jev 6 searches at a time, H2O one):

| | All | Pricing | Docs | Wikipedia | long | heldout | Not on the site | Tokens |
|---|---|---|---|---|---|---|---|---|
| Jev | 25/28 | 8/8 | 2/3 | 3/3 | 5/5 | 4/6 | 3/3 | 11.9M |
| H2O | 5/28 | 2/8 | 1/3 | 0/3 | 0/5 | 0/6 | 2/3 | 1.3M |

The 14 H2O searches that start on Wikipedia (wikipedia, long, heldout) ended without a page count, though every request that reached the server through the proxy got a 200; why wasn't established. Against Jev's own noise, H2O's widest gaps are `f` (65.2% against 98.5%) and the best link (43% against 73%).
