# Browser-fingerprint retry, measured

The fetch layer only, on 38 public sites, two passes from one Mac on 2026-10-09 (raw rows: [results/browser-retry-2026-10-09.json](results/browser-retry-2026-10-09.json)). No model is called, so Jev and Clef cost nothing here.

- **normal**: jurl's HTTP client, unchanged.
- **browser**: the same request through a client with Chrome 149's TLS and HTTP/2 fingerprint and headers, on its own.
- **retry path**: `fetch()` with the retry on. The normal request goes first, and the browser client is asked only after a 403 or 503.

Run it with `cargo test --release bench_browser_retry -- --ignored --nocapture`. The sites are in [browser-retry/sites.json](browser-retry/sites.json), and it prints one `BENCH` line per site. The categories are ours; we did not check which bot protection each site uses.

## Success by category

Counts are per pass, run 1 / run 2. "2xx" counts any success status, including the soft blocks below.

| category | sites | normal 2xx | browser 2xx | retry path ok |
| --- | ---: | ---: | ---: | ---: |
| news (Spain) | 3 | 1 / 1 | 3 / 2 | 2 / 2 |
| news (France) | 2 | 2 / 2 | 2 / 2 | 2 / 2 |
| news (Germany) | 2 | 2 / 2 | 2 / 2 | 2 / 2 |
| news (Italy) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (Japan) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (US/UK) | 5 | 2 / 2 | 3 / 3 | 3 / 3 |
| news (Argentina) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| news (Hong Kong) | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| e-commerce | 3 | 3 / 3 | 3 / 3 | 3 / 3 |
| airline | 3 | 1 / 1 | 3 / 3 | 3 / 3 |
| hotel / booking | 1 | 1 / 1 | 1 / 1 | 1 / 1 |
| ticketing | 2 | 1 / 1 | 1 / 1 | 1 / 1 |
| CDN-fronted (guess) | 3 | 1 / 1 | 2 / 2 | 2 / 2 |
| control | 10 | 10 / 10 | 10 / 10 | 10 / 10 |
| **all** | **38** | **28 / 28** | **34 / 33** | **33 / 33** |

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
| elpais.com | 403 | 200 (run 1), 403 (run 2) | flaky: the retry call in run 1 got 403 too |

## Soft blocks the status rule cannot see

These answer with a success status, so jurl reads them as the page:

| site | normal | browser |
| --- | --- | --- |
| britishairways.com | 200, 8,998 bytes (both passes) | 200, 434,864 bytes (both passes) |
| booking.com | 202, 7,033 or 8,410 bytes | 200, 511,958 bytes, or 202, 8,410 bytes |
| amazon.com | 202, 0 bytes | 202, 2,007 bytes |

We did not look at what the short bodies say.

## Latency

- The five sites above: the refused normal request takes 138 ms (median), the browser request 390 ms, and the retry path 498 ms in all (range 197 to 1,971 ms): the refusal, then the browser request.
- A page the normal client reads is never asked again, so it costs what it did before: 172 ms median for a plain GET (56 samples across both passes).

## Binary

Stripped release build (`cargo build --release --locked`): 7,212,992 bytes before, 10,841,360 after (+3.6 MB, +50%). We did not break the increase down by crate.
