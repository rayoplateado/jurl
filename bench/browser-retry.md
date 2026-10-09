# Browser-fingerprint retry, measured

Two parts, both from one Mac on 2026-10-09:

1. **The fetch layer on 38 public sites** (two passes, no model calls, so Jev and Clef cost nothing). Raw rows: [results/browser-retry-sticky-2026-10-09.json](results/browser-retry-sticky-2026-10-09.json) for the current code, and [results/browser-retry-2026-10-09.json](results/browser-retry-2026-10-09.json) for the first pass, made before the host memo and the Retry-After rule.
2. **`--follow` on a walled site**, before and after the host memo, with the Jev cost. Raw: [results/follow-walled-2026-10-09.json](results/follow-walled-2026-10-09.json). After the side-file rule (838f6c0): [results/follow-walled-sitemap-2026-10-09.json](results/follow-walled-sitemap-2026-10-09.json).

- **normal**: jurl's HTTP client, unchanged.
- **browser**: the same request through a client with Chrome 149's TLS and HTTP/2 fingerprint and headers, on its own.
- **retry path**: `fetch()` with the retry on. The normal request goes first; the browser client is asked only after a 403, or a 503 without a Retry-After.

Run the fetch layer with `cargo test --release bench_browser_retry -- --ignored --nocapture`. The sites are in [browser-retry/sites.json](browser-retry/sites.json), and the run prints one `BENCH` line per site. The categories are ours; we did not check which bot protection each site uses.

## Success by category (current code, two passes)

Counts are per pass, run 1 / run 2. "2xx" counts any success status, including the soft blocks below.

| category | sites | normal 2xx | browser 2xx | retry path ok |
| --- | ---: | ---: | ---: | ---: |
| news (Spain) | 3 | 1 / 1 | 2 / 2 | 3 / 2 |
| news (France) | 2 | 2 / 2 | 2 / 2 | 2 / 2 |
| news (Germany) | 2 | 2 / 2 | 2 / 2 | 2 / 2 |
| news (Italy) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (Japan) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (US/UK) | 5 | 2 / 2 | 3 / 3 | 3 / 3 |
| news (Argentina) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (Hong Kong) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| e-commerce | 3 | 3 / 3 | 3 / 3 | 3 / 3 |
| airline | 3 | 0 / 0 | 3 / 3 | 2 / 2 |
| hotel / booking | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| ticketing | 2 | 1 / 1 | 1 / 1 | 1 / 1 |
| CDN-fronted (guess) | 3 | 1 / 1 | 2 / 2 | 2 / 2 |
| control | 10 | 10 / 10 | 10 / 10 | 10 / 10 |
| **all** | **38** | **27 / 27** | **33 / 33** | **33 / 32** |

The first pass, made before the memo and the Retry-After rule, read 28 / 28 normal, 34 / 33 browser and 33 / 33 retry. The gap is British Airways (below) and one elpais.com answer.

## What the retry recovers

Five sites refuse the normal client with 403, and the browser client reads them in both passes:

| site | normal | browser (bytes) |
| --- | --- | --- |
| eleconomista.es | 403 | 200 (599,770) |
| nytimes.com | 403 | 200 (1,620,373) |
| aa.com | 403 | 200 (73,010) |
| easyjet.com | 403 | 200 (2,497) |
| indeed.com | 403 | 200 (596,177) |

## What it does not recover

| site | normal | browser | why |
| --- | --- | --- | --- |
| wsj.com, reuters.com | 401 | 401 | 401 is not 403 or 503, so the rule never retries it |
| stubhub.com, g2.com | 403 | 403 | both fingerprints refused |
| elpais.com | 403 | 200 or 403, depending on the pass | flaky: the same site answers differently between requests |

## Soft blocks the status rule cannot see

These answer with a success status, so jurl reads them as the page:

| site | normal | browser |
| --- | --- | --- |
| britishairways.com | 200, 8,998 bytes (first pass) | 200, 434,864 bytes (both passes) |
| booking.com | 202, 7,033 or 8,410 bytes | 200, 511,958 bytes, or 202, 8,410 bytes |
| amazon.com | 202, 0 bytes | 202, 2,007 bytes |

We did not look at what the short bodies say.

British Airways also timed out on the plain client in both current passes (a 20 s timeout). Minutes later the same plain request returned 200 in about 0.1 s from both curl and the pre-retry baseline binary, and the new binary's plain path did too. We read the timeouts as transient and counted them as misses in the table.

## Latency

- The five sites above, medians: the refused plain request 128 ms, the browser request 351 ms, the retry path 348 ms. The retry path is lower than the first pass's 498 ms because its browser request reuses the connection that the bench's own browser call opened a moment before; in a real run the shared client does the same across hops.
- Pages the normal client reads are never asked again, so they cost what they did before: 162 ms median for a plain GET (54 samples across both passes).

## `--follow` on a walled site, before and after

The start page is eleconomista.es's front door, which the plain client is refused (403). The question is "How many Starbucks stores are there worldwide?". The answer, "unos 41.000 establecimientos propios y franquiciados en todo el mundo", was in an article linked from the front page, not on the front page itself, when these runs were made (checked with Chrome impersonation, which spends no Jev). The front page has since rotated; the next section has the current state.

Command: `jurl --follow 5 --precise -q "How many Starbucks stores are there worldwide?" https://www.eleconomista.es/ -t --json`

| run | build | wall | phases total | "2 more" batch | plain refusals | host switch lines | pages read | answer | Jev requests / input tokens |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| before 1 | per-hop retry | 2.3 s | 2,218 ms | 1,017 ms | 3 | 0 | 3 | 41.000 (p 0.97) | 10 / 77,288 |
| after 1 | host memo | 1.8 s | 1,750 ms | 594 ms | 1 | 1 | 3 | 41.000 (p 0.98) | 10 / 77,015 |
| before 2 | per-hop retry | 2.1 s | 2,047 ms | 666 ms | 3 | 0 | 3 | 41.000 (p 0.98) | 10 / 76,999 |
| after 2 | host memo | 1.8 s | 1,787 ms | 630 ms | 1 | 1 | 3 | 41.000 (p 0.98) | 10 / 77,128 |
| after 3 (final commit's binary) | host memo | 2.3 s | 1,745 ms | 574 ms | 1 | 1 | 3 | 41.000 (p 0.98) | 10 / 77,004 |

- **Hops:** the front page, then, in the first batch, the front page's second section and the article. The path is front page → article in every run.
- **Plain refusals:** before, every page is asked plain first and refused (three). After, only the start page is (one); the host is then on the browser client, so the batch's two pages go straight there. The `-t` output shows the change: three retry lines before, one "using the browser client" line after.
- **Latency:** "page 1 + site map" is the same in both builds (about 0.9 s), because the start page is still plain first. The saving is in the two-page batch. Over the two pairs the phase total is 17% lower on average (2,133 ms before, 1,769 ms after); the wall clock is 2.2 s before and 1.8 s after.
- **Site map (54738cb):** llms.txt and sitemap.xml are read alongside the start page, so they run before its retry can teach the host. They were plain and listed no pages in any run. From 838f6c0 they follow the same retry rule; see the next section.
- **Cost:** 385,434 Jev input tokens across the five runs, about $0.016 at $0.042 per million, under the $0.02 cap. Three one-page checks against britishairways.com added 2,171 tokens.

## `--follow` on a walled site, after the side-file rule (838f6c0)

Re-run on 2026-10-09, after the front page had rotated. The Starbucks article is no longer linked from the front page (checked with Chrome impersonation after the runs). It is still on the site: eleconomista lists it in its news sitemaps, `sitemap_news_ssl.php` (entry 260 of 607) and `sitemap_noticias.php` (entry 262 of 1474), both named in `robots.txt`. jurl does not read those. It reads `/sitemap.xml`, an index of 14 sitemaps, and only the first three: company lists (`/empresa/…`, 2,336 pages, none with "starbucks" in the address). So the sitemap adds 300 company pages as hints (the `most_relevant` cap), and the article is not among the candidates in 838f6c0. 54738cb reads no sitemap at all.

Command: `jurl --follow 5 --precise -q "How many Starbucks stores are there worldwide?" https://www.eleconomista.es/ -t --json`

| run | build | total | page 1 + site map | two 2-page batches | plain refusals | browser requests | site map | pages read | answer | Jev requests / input tokens |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | --- | ---: |
| before 1 | 54738cb | 11,122 ms | 916 ms | 1,024 ms, 8,503 ms | not counted | not counted | 1 page | 5 | none | 15 / 107,839 |
| before 2 | 54738cb | 10,230 ms | 912 ms | 446 ms, 8,422 ms | not counted | not counted | 1 page | 5 | none | 14 / 106,523 |
| after 1 | 838f6c0 | 10,696 ms | 1,079 ms | 8,518 ms, 427 ms | 4 | 11 | 301 pages | 5 | none | 16 / 138,834 |
| after 2 | 838f6c0 | 10,336 ms | 1,106 ms | 8,425 ms, 381 ms | 4 | 11 | 301 pages | 5 | none | 16 / 132,029 |

- **Sitemap:** the first run now gets the site map: 301 pages listed (300 hints and the start page), where 54738cb listed one. The sitemap is refused on the plain client and read on the browser client.
- **Plain refusals:** four in each after run: the start page, `robots.txt`, `llms.txt` and `sitemap.xml`. Each is a 403 to the plain client (checked with curl) and is retried. The 11 browser requests are the four retries, the three child sitemaps (read on the browser client, because the host is learned) and four further pages. 54738cb has no counters; by its code path it makes the same four plain requests, and its refused sitemap listed nothing.
- **Latency:** the totals average 10,516 ms after and 10,676 ms before. "Page 1 + site map" is about 0.18 s longer after, because the sitemap is now read. One 2-page batch took 8.4 to 8.5 s in all four runs: a page with no text without JavaScript, rendered with Lightpanda. That render is about 80% of each total, so the totals barely move. What changes is which pages the five-page budget reads.
- **Answer:** none within five pages, in either build. Neither reached the article.
- **Cost:** 485,225 Jev input tokens across the four runs, about $0.020 at the $0.042 per million rate used above, under the $0.03 cap.
- **Not done:** reading `robots.txt`'s `Sitemap:` lines as well as `/sitemap.xml` would reach the news sitemaps. That changes which files jurl reads, so it is left for a decision.

The full phase strings and the pages read in each run are in [results/follow-walled-sitemap-2026-10-09.json](results/follow-walled-sitemap-2026-10-09.json).

## Binary

Stripped release build (`cargo build --release --locked`): 7,212,992 bytes before the retry, 10,841,360 with it (+3.6 MB, +50%), 10,857,872 for 54738cb (the host memo), and 10,857,904 at 838f6c0 with the side-file retry (+3.6 MB, +50%). The size increase was accepted by the owner. Estimated from unstripped symbols: BoringSSL about 1.0 MB, the Rust brotli crate about 0.9 MB (TLS certificate compression that Chrome's fingerprint advertises), wreq and its HTTP/2 code about 0.4 MB, zstd about 0.14 MB.
