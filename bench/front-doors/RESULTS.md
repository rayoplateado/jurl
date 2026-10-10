# --follow from a front door: benchmark

Measured 2026-10-10 with `bench/front-doors/run.py`. Every cell is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`; `-t` only adds stderr. Jev is behind `jev_proxy.py`, which answers an identical request from a cache and forwards the rest. Real-world sheets (R1-R6, R6es) come first everywhere.

**Final state = step 4 + step 5**: the robots `Sitemap:` lines, the newest-first sitemap index, llms.txt and cross-domain links, and the sitemaps of linked hosts. No navigation flag: it was tried in round 2 and reverted (round2.md). The final round ran with Accept-Language `en`; the default is now none (the caller sets `JURL_ACCEPT_LANGUAGE`), and the three cells that depend on the old default were re-run with none (round2.md). Binary 12,065,040 bytes.

## Headline

| set | round | cells | correct | wrong | not found | not run |
|---|---|---:|---:|---:|---:|---:|
| real-world | baseline | 85 | 45 | 19 | 21 | 0 |
| real-world | step 4 (round 1) | 85 | 46 | 14 | 25 | 0 |
| real-world | r2a4: flag + step 5 (tried, reverted) | 85 | 46 | 12 | 24 | 3 |
| real-world | **final: step 4 + step 5** | 85 | 46 | 15 | 24 | 0 |
| developer | baseline | 25 | 16 | 3 | 6 | 0 |
| developer | step 4 (round 1) | 25 | 17 | 3 | 5 | 0 |
| developer | r2a4: flag + step 5 (tried, reverted) | 25 | 18 | 3 | 4 | 0 |
| developer | **final: step 4 + step 5** | 25 | 18 | 3 | 4 | 0 |
| all | baseline | 110 | 61 | 22 | 27 | 0 |
| all | step 4 (round 1) | 110 | 63 | 17 | 30 | 0 |
| all | r2a4: flag + step 5 (tried, reverted) | 110 | 64 | 15 | 28 | 3 |
| all | **final: step 4 + step 5** | 110 | 64 | 18 | 28 | 0 |

## Rounds

Columns: correct / wrong / not found per sheet; mean pages read; mean tokens used (what each run's JSON reports, cached answers included); billed tokens (what Jev charged for that round's cells: the proxy's forwarded answers) and its cost at $0.042 per million input tokens.

**Baseline: #50 + #51 as merged (no change of ours)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,961 | 894,784 | 0.0376 |
| R2 | 18 | 7 | 3 | 8 | 3.5 | 50,921 | 916,586 | 0.0385 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 303,161 | 0.0127 |
| R4 | 15 | 8 | 4 | 3 | 3.2 | 45,712 | 659,687 | 0.0277 |
| R5 | 12 | 2 | 7 | 3 | 4.5 | 85,416 | 1,010,527 | 0.0424 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 82,119 | 821,188 | 0.0345 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 86,975 | 869,750 | 0.0365 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 89,874 | 449,368 | 0.0189 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,282 | 619,200 | 0.0260 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 76,080 | 912,959 | 0.0383 |
| **all** | 110 | 61 | 22 | 27 | 3.6 | 68,341 | 7,457,210 | 0.3132 |

Failures by class (baseline): wrong_answer 22, not_reached 13, not_on_site 5, js_only 4, missed_on_read 3, blocked 2.

**Step 1: robots.txt `Sitemap:` lines; a sitemap index read newest-first by `<lastmod>`**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,692 | 119,306 | 0.0050 |
| R2 | 18 | 6 | 3 | 9 | 3.5 | 49,151 | 82,370 | 0.0035 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 0 | 0.0000 |
| R4 | 15 | 8 | 2 | 5 | 3.3 | 59,084 | 461,767 | 0.0194 |
| R5 | 12 | 4 | 6 | 2 | 4.2 | 81,956 | 356,478 | 0.0150 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 87,716 | 63,872 | 0.0027 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 92,740 | 65,689 | 0.0028 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 89,874 | 0 | 0.0000 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,863 | 114,933 | 0.0048 |
| S3 | 12 | 10 | 2 | 0 | 4.1 | 75,731 | 169,476 | 0.0071 |
| **all** | 110 | 62 | 19 | 29 | 3.6 | 70,505 | 1,433,891 | 0.0602 |

Failures by class (step1): wrong_answer 19, not_reached (carried) 13, not_on_site (carried) 5, js_only (carried) 4, not_reached 3, missed_on_read (carried) 2, blocked (carried) 2.

**Step 2: llms.txt of the front door's linked hosts on the same registrable domain (PSL)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,692 | 0 | 0.0000 |
| R2 | 18 | 6 | 3 | 9 | 3.5 | 49,151 | 1,112 | 0.0000 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 0 | 0.0000 |
| R4 | 15 | 8 | 2 | 5 | 3.3 | 58,318 | 0 | 0.0000 |
| R5 | 12 | 3 | 7 | 2 | 4.2 | 81,184 | 0 | 0.0000 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 87,716 | 0 | 0.0000 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 92,740 | 0 | 0.0000 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 90,250 | 65,486 | 0.0028 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,863 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.1 | 76,596 | 105,183 | 0.0044 |
| **all** | 110 | 61 | 20 | 29 | 3.6 | 70,428 | 171,781 | 0.0072 |

Failures by class (step2): wrong_answer 20, not_reached (carried) 13, not_on_site (carried) 5, js_only (carried) 4, not_reached 3, missed_on_read (carried) 2, blocked (carried) 2.

**Step 3: links to other registrable domains (explicit links and bare URLs in the text)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 76,022 | 184,457 | 0.0077 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,753 | 326,659 | 0.0137 |
| R3 | 8 | 3 | 3 | 2 | 2.5 | 42,119 | 100,323 | 0.0042 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,162 | 153,146 | 0.0064 |
| R5 | 12 | 4 | 6 | 2 | 4.3 | 90,159 | 404,972 | 0.0170 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,404 | 185,468 | 0.0078 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 204,011 | 0.0086 |
| S1 | 5 | 2 | 0 | 3 | 5.0 | 106,662 | 195,645 | 0.0082 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 69,356 | 0.0029 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,036 | 256,996 | 0.0108 |
| **all** | 110 | 64 | 17 | 29 | 3.7 | 72,990 | 2,081,033 | 0.0874 |

Failures by class (step3): wrong_answer 17, not_reached (carried) 13, js_only (carried) 4, not_on_site (carried) 4, missed_on_read (carried) 2, not_on_site 2, blocked (carried) 2, missed_on_read 1, not_reached 1.

**Step 4 (round 1): Accept-Language `en` by default, `JURL_ACCEPT_LANGUAGE` overrides**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 76,011 | 2,919 | 0.0001 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,753 | 0 | 0.0000 |
| R3 | 8 | 2 | 3 | 3 | 2.5 | 40,201 | 4,202 | 0.0002 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,163 | 14,750 | 0.0006 |
| R5 | 12 | 4 | 6 | 2 | 4.3 | 90,477 | 8,930 | 0.0004 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 30,200 | 0.0013 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 2 | 0 | 3 | 5.0 | 106,088 | 82,468 | 0.0035 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,036 | 0 | 0.0000 |
| **all** | 110 | 63 | 17 | 30 | 3.7 | 72,849 | 143,469 | 0.0060 |

Failures by class (step4): wrong_answer 17, not_reached (carried) 13, not_on_site (carried) 6, js_only (carried) 4, missed_on_read (carried) 2, not_reached 2, blocked (carried) 2, missed_on_read 1.

**Step 5, round 1 (no navigation flag): the sitemaps of linked hosts, read through their robots.txt**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 81,227 | 262,801 | 0.0110 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 58,772 | 139,344 | 0.0059 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 42,494 | 90,352 | 0.0038 |
| R4 | 15 | 8 | 4 | 3 | 3.6 | 66,691 | 158,399 | 0.0067 |
| R5 | 12 | 4 | 6 | 2 | 4.5 | 100,304 | 483,819 | 0.0203 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 3 | 1 | 1 | 4.6 | 98,730 | 219,759 | 0.0092 |
| S2 | 8 | 5 | 1 | 2 | 3.8 | 84,368 | 118,922 | 0.0050 |
| S3 | 12 | 10 | 1 | 1 | 3.8 | 75,929 | 137,637 | 0.0058 |
| **all** | 110 | 64 | 19 | 27 | 3.8 | 76,356 | 1,611,033 | 0.0677 |

Failures by class (step5): wrong_answer 19, not_reached (carried) 12, not_on_site (carried) 6, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 1, not_on_site 1.

**Round 2a, tried and reverted: navigation-like blocks dropped from the page (link share 0.8, one link), banner role**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 6 | 1 | 5 | 4.2 | 75,381 | 306,220 | 0.0129 |
| R2 | 18 | 4 | 2 | 12 | 3.9 | 39,906 | 172,015 | 0.0072 |
| R3 | 8 | 2 | 3 | 3 | 2.5 | 40,025 | 36,996 | 0.0016 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,005 | 33,126 | 0.0014 |
| R5 | 12 | 2 | 5 | 2 (+3 not run) | 4.3 | 66,896 | 336,503 | 0.0141 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 84,782 | 113,164 | 0.0048 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,408 | 129,246 | 0.0054 |
| S1 | 5 | 2 | 1 | 2 | 5.0 | 102,024 | 95,754 | 0.0040 |
| S2 | 8 | 5 | 2 | 1 | 3.5 | 80,371 | 31,788 | 0.0013 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 76,924 | 36,990 | 0.0016 |
| **all** | 110 | 59 | 19 | 29 (+3 not run) | 3.7 | 67,763 | 1,291,802 | 0.0543 |

Failures by class (r2a): wrong_answer 19, not_reached (carried) 14, not_on_site (carried) 5, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 2, not_on_site 1.

**Round 2a2, tried and reverted: navigation-like blocks dropped (two or more links)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 6 | 0 | 5 | 4.3 | 81,811 | 298,554 | 0.0125 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,726 | 1,269 | 0.0001 |
| R3 | 8 | 2 | 3 | 3 | 2.5 | 40,186 | 0 | 0.0000 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,162 | 0 | 0.0000 |
| R5 | 12 | 3 | 4 | 2 (+3 not run) | 4.3 | 66,730 | 29,915 | 0.0013 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,301 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,955 | 0 | 0.0000 |
| S1 | 5 | 1 | 1 | 2 (+1 not run) | 5.0 | 90,741 | 41,203 | 0.0017 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,033 | 0 | 0.0000 |
| **all** | 110 | 60 | 16 | 29 (+4 not run) | 3.7 | 70,186 | 370,941 | 0.0156 |

Failures by class (r2a2): wrong_answer 16, not_reached (carried) 14, not_on_site (carried) 6, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 2.

**Round 2a3, tried and reverted: navigation flag, step 5, banner role**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 81,467 | 17,720 | 0.0007 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 58,751 | 0 | 0.0000 |
| R3 | 8 | 3 | 3 | 2 | 2.5 | 44,413 | 0 | 0.0000 |
| R4 | 15 | 8 | 3 | 4 | 3.7 | 66,619 | 7,084 | 0.0003 |
| R5 | 12 | 3 | 4 | 2 (+3 not run) | 4.6 | 75,406 | 97,800 | 0.0041 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 3 | 1 | 1 | 4.6 | 98,730 | 0 | 0.0000 |
| S2 | 8 | 5 | 1 | 2 | 3.8 | 84,368 | 0 | 0.0000 |
| S3 | 12 | 10 | 1 | 1 | 3.8 | 75,929 | 0 | 0.0000 |
| **all** | 110 | 64 | 15 | 28 (+3 not run) | 3.8 | 73,792 | 122,604 | 0.0051 |

Failures by class (r2a3): wrong_answer 15, not_reached (carried) 12, not_on_site (carried) 7, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 2.

**Round 2a4, tried and reverted: navigation flag and step 5 (EIC's 2026 deadline is correct only with the flag)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 81,363 | 9,920 | 0.0004 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 58,772 | 0 | 0.0000 |
| R3 | 8 | 3 | 3 | 2 | 2.5 | 44,413 | 0 | 0.0000 |
| R4 | 15 | 8 | 3 | 4 | 3.7 | 66,192 | 0 | 0.0000 |
| R5 | 12 | 3 | 4 | 2 (+3 not run) | 4.6 | 75,913 | 65,629 | 0.0028 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 3 | 1 | 1 | 4.6 | 98,730 | 0 | 0.0000 |
| S2 | 8 | 5 | 1 | 2 | 3.8 | 84,368 | 0 | 0.0000 |
| S3 | 12 | 10 | 1 | 1 | 3.8 | 75,929 | 0 | 0.0000 |
| **all** | 110 | 64 | 15 | 28 (+3 not run) | 3.8 | 73,781 | 75,549 | 0.0032 |

Failures by class (r2a4): wrong_answer 15, not_reached (carried) 7, not_on_site (carried) 7, not_on_site 5, js_only (carried) 4, missed_on_read (carried) 2, blocked (carried) 2, not_reached 1.

**Round 2a5, tried and reverted: navigation flag, no step 5**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 76,133 | 0 | 0.0000 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,753 | 0 | 0.0000 |
| R3 | 8 | 2 | 3 | 3 | 2.5 | 40,201 | 0 | 0.0000 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,162 | 0 | 0.0000 |
| R5 | 12 | 3 | 4 | 2 (+3 not run) | 4.3 | 66,783 | 0 | 0.0000 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 2 | 1 | 2 | 5.0 | 104,491 | 21,585 | 0.0009 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,033 | 0 | 0.0000 |
| **all** | 110 | 62 | 16 | 29 (+3 not run) | 3.7 | 70,205 | 21,585 | 0.0009 |

Failures by class (r2a5): wrong_answer 16, not_on_site (carried) 11, not_reached (carried) 10, js_only (carried) 4, missed_on_read (carried) 2, blocked (carried) 2.

**FINAL: step 4 + step 5 (the linked hosts' sitemaps), no navigation flag; measured with `en`**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 81,184 | 4,939 | 0.0002 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 58,772 | 0 | 0.0000 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 42,494 | 0 | 0.0000 |
| R4 | 15 | 8 | 3 | 4 | 3.7 | 66,998 | 3,856 | 0.0002 |
| R5 | 12 | 4 | 6 | 2 | 4.5 | 99,906 | 5,646 | 0.0002 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 3 | 1 | 1 | 4.6 | 98,730 | 0 | 0.0000 |
| S2 | 8 | 5 | 1 | 2 | 3.8 | 84,368 | 11,776 | 0.0005 |
| S3 | 12 | 10 | 1 | 1 | 3.8 | 77,050 | 42,240 | 0.0018 |
| **all** | 110 | 64 | 18 | 28 | 3.8 | 76,472 | 68,457 | 0.0029 |

Failures by class (final): wrong_answer 18, not_on_site (carried) 12, not_reached (carried) 8, js_only (carried) 4, missed_on_read (carried) 2, blocked (carried) 2.

## Final round, cell by cell

Verdict per cell. A correct or wrong answer is judged by its quote, with the one-line reason from `judgements.json`. A cell with no answer gives its class (see the legend) and the reason from `notfound.json`.

| sheet | row | question | answer (closest) | verdict | reason |
|---|---|---|---|---|---|
| R1 | Impact Hub Madrid | What is the monthly price of a flexible (hot) desk? | 120€
Contratar online | not_found | not_on_site (carried). (carried from r2a4) Re-verified in round 2a4: the price pages read give private-office rates ("Oficina flexible ... €/mes"), no hot-desk price. |
| R1 | Impact Hub Madrid | Can you buy a day pass? | Ven cuando lo necesites. Adquiere tu pase de un día o un bono de 10 dí | correct | "Adquiere tu pase de un dia o un bono de 10 dias": a day pass can be bought. |
| R1 | Impact Hub Madrid | Is it open 24/7? | 24/7 | correct | The plans list "Acceso 24/7" (24/7 access for members), so the space is open round the clock for that plan. |
| R1 | Utopicus | What is the monthly price of a flexible (hot) desk? | 259€/mes | correct | The Utopicus Passport, its flexible workstation, is listed at 259 EUR/mes ("Comfortable workstation 24/7 access"). |
| R1 | Utopicus | Can you buy a day pass? | Bono 10 pases de día Working Pass a utilizar en 3 meses | correct | "Bono 10 pases de dia Working Pass": a pack of 10 day passes is sold. |
| R1 | Utopicus | Is it open 24/7? | 4-8 | not_found | missed_on_read (carried). (carried from step4) The same hours page as in step 3: read, not picked. |
| R1 | Talent Garden Madrid | What is the monthly price of a flexible (hot) desk? |  | not_found | not_on_site (carried). (carried from r2a4) Re-verified in round 2a4: 7 pages read (coworking and knowledge-base pages), no desk prices in them. |
| R1 | Talent Garden Madrid | Can you buy a day pass? | Fixed desks, hourly packages or access to the digital community | not_found | not_on_site (carried). (carried from r2a4) Re-verified in round 2a4: 10 pages read, no day-pass text in them. |
| R1 | Talent Garden Madrid | Is it open 24/7? | 24/7 access in all TAGs | correct | "24/7 access in all TAGs" (TAG = Talent Garden campus) on the front page. |
| R1 | Lexington | What is the monthly price of a flexible (hot) desk? | Alquiler zonas comunes
€
Electricidad
€
Comunidad
€
Mantenimiento
€
In | not_found | not_on_site (carried). (carried from r2a4) Re-verified in round 2a4: the prices read are for private offices, not for a hot desk. |
| R1 | Lexington | Can you buy a day pass? | espacios compartidos (escritorios en oficinas compartidas o pases diar | correct | Lexington sells "pases diarios a zonas de trabajo comun" (daily passes to shared areas). |
| R1 | Lexington | Is it open 24/7? | 24/7 | correct | "podreis acceder a la oficina alquilada de manera ilimitada, 24/7" (members' access, 24/7). |
| R2 | seaya.vc | Do they invest at pre-seed? | Your founder-friendly financing solution | not_found | not_reached (carried). (carried from step2) Regression: the 2023 fund article that answered in baseline sits in a sitemap that newest-first no longer reads. |
| R2 | seaya.vc | What is their typical first ticket? | €300m | not_found | js_only (carried). (carried from baseline) the front door needs rendering (Lightpanda); no first-ticket text in the rendered pages |
| R2 | seaya.vc | How do you send them a pitch? | dealflow.website@seaya.vc | correct | The Contact page has a "Pitch" heading with dealflow.website@seaya.vc under it. |
| R2 | Kibo Ventures | Do they invest at pre-seed? | We lead or co-lead pre-Series A or Series A rounds | not_found | not_on_site (carried). (carried from baseline) the site says 'pre-Series A or Series A rounds', not pre-seed, on the pages read |
| R2 | Kibo Ventures | What is their typical first ticket? | No | not_found | not_reached (carried). (carried from step3) Regression: a cross-domain typeform (a whistleblowing form) was followed and the search lost the first-ticket page. |
| R2 | Kibo Ventures | How do you send them a pitch? | Tiene derecho a acceder, rectificar y suprimir los datos, así como otr | not_found | not_reached (carried). (carried from baseline) no pitch or contact text on the 5 pages read; the contact page was not reached (unverified) |
| R2 | K Fund | Do they invest at pre-seed? | This enables us to invest in companies from 100k€ to 10M€ and in seed, | correct | "invest in companies from 100k EUR to 10M EUR and in seed, pre-seed and Series A+". |
| R2 | K Fund | What is their typical first ticket? | €100k | correct | The K Founders first check is EUR 100k, the same as the fund's smallest ticket (100k to 10M); the quote is the program's, close to the fund's. |
| R2 | K Fund | How do you send them a pitch? | We have already launched the first call and you can join filling out t | wrong | A 2020 K Founders call ("open until September 15th 2020"): outdated, and not how to pitch the fund. |
| R2 | Samaipata | Do they invest at pre-seed? | Investment Thesis | not_found | not_on_site (carried). (carried from baseline) no pre-seed text in the 3 pages read; the investment-thesis link returns 404 |
| R2 | Samaipata | What is their typical first ticket? | €8M | not_found | not_on_site (carried). (carried from step3) The pages read have no first-ticket figure (the thesis page 404s). |
| R2 | Samaipata | How do you send them a pitch? | By answering the questions and sending us your deck | correct | The founders' typeform: "By answering the questions and sending us your deck" is the way to pitch. |
| R2 | JME Ventures | Do they invest at pre-seed? | We are seed stage investors in some of Spain´s biggest success stories | not_found | not_on_site (carried). (carried from baseline) the site says 'seed stage investors' on the pages read; no pre-seed text |
| R2 | JME Ventures | What is their typical first ticket? | €100k to €3m | correct | "We invest from EUR 100k to EUR 3m". |
| R2 | JME Ventures | How do you send them a pitch? | We prioritize speed & clarity, offering a transparent process that res | not_found | not_on_site (carried). (carried from step3) The pages read give the investment range (0.5-2m) but no pitch address or method. |
| R2 | Nauta | Do they invest at pre-seed? |  | not_found | js_only (carried). (carried from baseline) front door rendered with Lightpanda; one page read and no link scored (warmth 0.06) |
| R2 | Nauta | What is their typical first ticket? |  | not_found | js_only (carried). (carried from baseline) front door rendered with Lightpanda; one page read and no link scored (warmth 0.05) |
| R2 | Nauta | How do you send them a pitch? | Fale conosco | wrong | "Fale conosco" is a menu label (contact us); no pitch instructions. |
| R3 | ENISA | When is the application deadline? | Las empresas de base tecnológica e innovadora constituidas a partir de | correct | The call for companies founded from 2023 closes 22 January 2027 (the same call as before). |
| R3 | ENISA | What is the maximum amount? | 9.964 | wrong | "9.964 prestamos desembolsados" is a count of loans disbursed, not a maximum amount. |
| R3 | CDTI | When is the application deadline? |  | not_found | not_reached (carried). (carried from baseline) front door links to Ayuda and Actualidad but none scored (warmth 0.00); grant pages not reached |
| R3 | CDTI | What is the maximum amount? |  | not_found | not_reached (carried). (carried from baseline) front door links not scored (warmth 0.00); grant pages not reached |
| R3 | Acelerapyme (Kit Digital) | When is the application deadline? | 11 de septiembre de 2026 a las 11:00 horas | correct | "El plazo de inscripcion finaliza el 11 de septiembre de 2026 a las 11:00 horas". |
| R3 | Acelerapyme (Kit Digital) | What is the maximum amount? | 40 millones de euros | wrong | EUR 40 million is the budget of an AI initiative, not a maximum amount per applicant. |
| R3 | EIC | When is the application deadline? | 13 September 2024 - 10:00 CEST | wrong | "New deadline for receipt of tenders: 13 September 2024": a tender deadline from 2024, not a grant application deadline. |
| R3 | EIC | What is the maximum amount? | €6,5 billion | wrong | EUR 6.5 billion is the total support offered, not the maximum amount per project. |
| R4 | Web Summit | What are the dates of the next edition? | 9-12, 2026 | correct | "Lisbon - November 9-12, 2026": the next edition. |
| R4 | Web Summit | How much is a general ticket? | €809 Excl. sales tax | correct | The General attendee ticket is shown at EUR 1,595 / 995 / 809 excl. sales tax; 809 is one of its prices (the lowest shown, the current one is not dated). |
| R4 | Web Summit | In which city is it held? | Lisbon | correct | "Lisbon - November 9-12, 2026". |
| R4 | 4YFN | What are the dates of the next edition? | 2027 | not_found | not_reached (carried). (carried from step2) Regression: the 4YFN 2027 dates article sits in a sitemap that newest-first no longer reads. |
| R4 | 4YFN | How much is a general ticket? |  | not_found | not_reached (carried). (carried from baseline) no ticket text in the pages read; the tickets page was not reached (unverified) |
| R4 | 4YFN | In which city is it held? | Barcelona | correct | The page says 4YFN is held in Barcelona ("4YFN26 Barcelona ... Fira de Barcelona"). |
| R4 | South Summit | What are the dates of the next edition? | De 3 a 5 de junho de 2026 | wrong | The 3-5 June 2026 dates are the Madrid 2026 edition, already past: not the next edition. |
| R4 | South Summit | How much is a general ticket? | 299€ | wrong | 299 EUR is the Startup Pass in the FAQ, not the general ticket. |
| R4 | South Summit | In which city is it held? | Madrid | correct | "WHERE: La Nave (C/ Cifuentes, 5), Madrid": the event is held in Madrid. |
| R4 | VivaTech | What are the dates of the next edition? | June 16 | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (information, practical information, get-your-pass) |
| R4 | VivaTech | How much is a general ticket? |  | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (get-your-pass) |
| R4 | VivaTech | In which city is it held? | Paris | correct | "VivaTech, the Paris tech conference". |
| R4 | Slush | What are the dates of the next edition? | Nov 18–19, Helsinki | correct | "Slush 2026 Nov 18-19, Helsinki": the next edition. |
| R4 | Slush | How much is a general ticket? | 395€* | wrong | EUR 395 is the startup ticket ("Tickets to Slush 2026 startup 395 EUR"), not the general ticket. |
| R4 | Slush | In which city is it held? | Helsinki | correct | "Helsinki is the home of Slush since 2008". |
| R5 | IE Business School | What is the price of the flagship MBA or bootcamp? | The cost can vary depending on the program | not_found | not_on_site (carried). (carried from r2a4) Re-verified in round 2a4: 9 pages read, no MBA or bootcamp fee in them; the fee page was not reached. |
| R5 | IE Business School | How long does it last? | TWO-WEEK MODULE OPTIONS | wrong | "TWO-WEEK MODULE OPTIONS" is a menu label; the duration of the MBA is not given. |
| R5 | IE Business School | Is there an online version? | EVENT FORMAT Online events | wrong | "EVENT FORMAT Online events" is an event-format label, not an online MBA. |
| R5 | ESADE | What is the price of the flagship MBA or bootcamp? | €39,650 | correct | "1st year at Esade: EUR 39,650" from the Full-Time MBA fees page (the first-year fee; the total is not stated). |
| R5 | ESADE | How long does it last? | Weeks of April 13th and April 20th - Pedralbes and Sant Cugat campuses | wrong | "Weeks of April 13th and April 20th - Pedralbes and Sant Cugat campuses": dates, not a duration. |
| R5 | ESADE | Is there an online version? | Online | wrong | "Format: Online, Fees 2.900 EUR" is an online executive program, not the flagship MBA. |
| R5 | IESE | What is the price of the flagship MBA or bootcamp? | €117,000 | correct | Table: "September 2027 intake ... Total EUR 117,000" for the MBA (fees over the two years). |
| R5 | IESE | How long does it last? | 18 months | correct | "The content of the EMBA program takes place over 18 months": the EMBA's length (not the full-time MBA's). |
| R5 | IESE | Is there an online version? | Online | wrong | "Online" is a category label among Focused Programs, not a statement about an online MBA or bootcamp. |
| R5 | Ironhack | What is the price of the flagship MBA or bootcamp? | 6.750€ | not_found | js_only (carried). (carried from baseline) pages read needed rendering (Lightpanda); no price text in them |
| R5 | Ironhack | How long does it last? | bis zu 1 Jahr nach Abschluss (oder bis du einen Job hast) | wrong | "Dauer: bis zu 1 Jahr nach Abschluss" is career support in the German FAQ, not the bootcamp's length. |
| R5 | Ironhack | Is there an online version? | Ironhack Remoto | correct | "Ironhack Remoto" is its remote (online) bootcamp format. |
| R6 | Ecoalf | From what order amount is shipping free? | 150€ | correct | "Envios gratis a partir de 150 EUR". |
| R6 | Ecoalf | How many days do you have to return an item? | 30 días | correct | "El plazo de devolucion y cambios es de 30 dias". |
| R6 | Hawkers | From what order amount is shipping free? | 49 € | correct | "El envio estandar es gratuito para compras superiores a 49 EUR". |
| R6 | Hawkers | How many days do you have to return an item? | 15 días naturales | correct | "Dispones de 15 dias naturales ... para devolver". |
| R6 | Pompeii | From what order amount is shipping free? | 100€ | correct | "gratuitos a partir de 100 EUR". |
| R6 | Pompeii | How many days do you have to return an item? | 365 días | correct | "Dispones de 365 dias para realizar cambios o devoluciones". |
| R6 | Scalpers | From what order amount is shipping free? | 40€ | correct | "Para pedidos superiores a 40 EUR, el envio a domicilio es gratuito". |
| R6 | Scalpers | How many days do you have to return an item? | 30 calendar days | correct | The English returns text: "30 calendar days" from receipt of the order (the same policy as the Spanish page). |
| R6 | Mr Wonderful | From what order amount is shipping free? | 30 € | correct | "Los pedidos superiores a 30 EUR disfrutan de envio gratuito" (peninsular Spain). |
| R6 | Mr Wonderful | How many days do you have to return an item? | 14 | correct | The legal withdrawal period is 14 natural days (art. 71 Ley 3/2014); the site's returns text says 15 (see the Spanish run). |
| R6es | Ecoalf | ¿A partir de qué importe de pedido el envío es gratis? | 150€ | correct | "Envios gratis a partir de 150 EUR". |
| R6es | Ecoalf | ¿Cuántos días tengo para devolver un artículo? | 30 días | correct | "El plazo de devolucion y cambios es de 30 dias". |
| R6es | Hawkers | ¿A partir de qué importe de pedido el envío es gratis? | 49 € | correct | "El envio estandar es gratuito para compras superiores a 49 EUR". |
| R6es | Hawkers | ¿Cuántos días tengo para devolver un artículo? | 15 días naturales | correct | "Dispones de 15 dias naturales ... para devolver". |
| R6es | Pompeii | ¿A partir de qué importe de pedido el envío es gratis? | 100€ | correct | "gratuitos a partir de 100 EUR". |
| R6es | Pompeii | ¿Cuántos días tengo para devolver un artículo? | 365 días | correct | "Dispones de 365 dias para realizar cambios o devoluciones". |
| R6es | Scalpers | ¿A partir de qué importe de pedido el envío es gratis? | 40€ | correct | "Si tu pedido no supera los 40 EUR, los gastos de envio ascienden a 3,95 EUR": above 40 EUR shipping is free. |
| R6es | Scalpers | ¿Cuántos días tengo para devolver un artículo? | Tienes 30 días naturales desde la recepción de tu pedido para realizar | correct | "Tienes 30 dias naturales desde la recepcion de tu pedido para realizar una devolucion". |
| R6es | Mr Wonderful | ¿A partir de qué importe de pedido el envío es gratis? | 30 € | correct | "Los pedidos superiores a 30 EUR disfrutan de envio gratuito". |
| R6es | Mr Wonderful | ¿Cuántos días tengo para devolver un artículo? | 15 días naturales | correct | "El USUARIO dispone de 15 dias naturales para la devolucion". |
| S1 | github.com | What is the primary rate limit for authenticated requests? | 5,000 requests per hour | correct | "All of these requests count towards your personal rate limit of 5,000 requests per hour". |
| S1 | gitlab.com | What is the primary rate limit for authenticated requests? | A sustained limit, measured each hour. Use this number to plan your us | wrong | "A sustained limit, measured each hour. Use this number to plan your usage." states no number. |
| S1 | bitbucket.org | What is the primary rate limit for authenticated requests? | 1,000 requests per hour | correct | Atlassian's API limits page: "The default rate limit is 1,000 requests per hour" (the page the owner's own check answered from). |
| S1 | stripe.com | What is the primary rate limit for authenticated requests? | 100 requests per second | correct | The API rate-limit table: "Global API rate limit, Live mode: 100 requests per second". |
| S1 | shopify.com | What is the primary rate limit for authenticated requests? |  | not_found | not_reached (carried). (carried from r2a4) Re-verified in round 2a4: the hub /docs/api/usage/limits is read; its table names the REST Admin API but gives no figure; the REST Admin API page (one hop past the hub) is not reached. --follow 10 takes the Storefront pages instead and answers wrongly. |
| S2 | linear.app | What is the monthly price of the cheapest paid plan? | $10 per user/month | correct | "$10 per user/month, billed yearly": the cheapest paid plan. |
| S2 | notion.com | What is the monthly price of the cheapest paid plan? | €9.50 | correct | "Plus: EUR 9.50 per seat/month": Plus is the cheapest paid plan. |
| S2 | clickup.com | What is the monthly price of the cheapest paid plan? | $8user / mo | correct | "Core $8 user/mo": the cheapest paid plan (Free is $0). |
| S2 | vercel.com | What is the monthly price of the cheapest paid plan? | $20/mo | correct | Table: "Hobby $0/mo ... Pro $20/mo": Pro is the cheapest paid plan. |
| S2 | supabase.com | What is the monthly price of the cheapest paid plan? | $25/month | correct | "Pro: from $25/month": the cheapest paid plan. |
| S2 | posthog.com | What is the monthly price of the cheapest paid plan? | $0.000015/row | not_found | not_on_site (carried). (carried from baseline) PostHog's paid plans are usage-based on the pricing page read: no monthly price for a paid plan |
| S2 | figma.com | What is the monthly price of the cheapest paid plan? | US$3/bulan | wrong | "Lisensi Kolaborasi US$3/bulan": the collaborator seat, not the cheapest plan (as before). |
| S2 | asana.com | What is the monthly price of the cheapest paid plan? |  | not_found | missed_on_read (carried). (carried from baseline) the pricing page is read as markdown (no prices in it); the HTML has them ('Asana: $10.99 ... billed monthly') |
| S3 | vercel.com | Do they have a SOC 2 Type II report? | Yes | correct | "Yes. Vercel holds a SOC 2 Type 2 attestation". |
| S3 | vercel.com | Can customers choose where their data is stored? | Previously, Hobby customers could only choose US East (iad1) regardles | wrong | Function co-location with data for region choice of Functions; not a statement that customers choose where their data is stored. |
| S3 | supabase.com | Do they have a SOC 2 Type II report? | Update (2023-05-22): Supabase is now SOC2 Type 2 compliant | correct | "Supabase is now SOC2 Type 2 compliant". |
| S3 | supabase.com | Can customers choose where their data is stored? | Your database is hosted in the AWS region you select when you create y | correct | "Your database is hosted in the AWS region you select when you create your project." |
| S3 | posthog.com | Do they have a SOC 2 Type II report? | PostHog is certified as SOC 2 Type 2 compliant, following an external  | correct | "PostHog is certified as SOC 2 Type 2 compliant". |
| S3 | posthog.com | Can customers choose where their data is stored? | Control where your data is physically stored for GDPR compliance | correct | The feature table: "Data storage location: Control where your data is physically stored for GDPR compliance". |
| S3 | sentry.io | Do they have a SOC 2 Type II report? | SOC2 Type II | correct | The attestations list includes "SOC2 Type II". |
| S3 | sentry.io | Can customers choose where their data is stored? | customers have the option to store and process event data exclusively  | correct | "customers have the option to store and process event data exclusively in the European Union" (EU Data Residency). |
| S3 | resend.com | Do they have a SOC 2 Type II report? | Resend is SOC 2 Type II compliant | correct | "Resend is SOC 2 Type II compliant." |
| S3 | resend.com | Can customers choose where their data is stored? | All of our customers' data is still stored in the United States | correct | "All of our customers' data is still stored in the United States": the answer is no, no region choice. |
| S3 | cloudflare.com | Do they have a SOC 2 Type II report? |  | not_found | not_on_site (carried). (carried from step5) The pages read say 'meet SOC 2 requirements' but no SOC 2 Type II report (the answer that was wrong before). |
| S3 | cloudflare.com | Can customers choose where their data is stored? | The Data Localization Suite (DLS) is a collection of tools that enable | correct | The Data Localization Suite lets customers "choose the location where Cloudflare inspects and stores data". |

## Round 2: the wrong answers, the navigation flag (tried, reverted), the linked sitemaps, Accept-Language

Round 2 spent about $0.13 billed (the proxy's count; $0.675 cumulative of the $1.04 cap for both rounds, counting calls whose results were later discarded and probes). Its cap of $0.50 was not reached. Every round ran the 110 main cells; the sharpened and risk cells are separate sheets. ESADE (esade.edu) was unreachable from this machine during the navigation rounds (timeouts), so those cells are **not run** there. It was reachable in the final round, and its cells are measured there.

### Headline

| set | baseline | round 1 (step 4) | r2a4: flag + step 5 (reverted) | **final: step 4 + step 5** |
|---|---|---|---|---|
| all 110 cells (C / W / NF) | 61 / 22 / 27 | 63 / 17 / 30 | 64 / 15 / 28 (+3 not run) | **64 / 18 / 28** |
| the same 107 cells (ESADE left out) | 61 / 20 / 26 | 62 / 15 / 30 | 64 / 15 / 28 | **63 / 16 / 28** |
| real-world, 85 cells | 45 / 19 / 21 | 46 / 14 / 25 | 46 / 12 / 24 (+3 not run) | **46 / 15 / 24** |
| developer, 25 cells | 16 / 3 / 6 | 17 / 3 / 5 | 18 / 3 / 4 | **18 / 3 / 4** |
| S1 (≥ 4/5 target) | 1/5 | 2/5 | 3/5 | **3/5: not met** (github, bitbucket, stripe; gitlab wrong; shopify not found) |

Targets: S1 at least 4/5 is **not met** (3/5). Total correct clearly above baseline is **not clearly met** (61 → 64). Wrong answers not increasing is **met** (22 → 18; real-world 19 → 15).

The final has one more wrong answer than r2a4 on the cells both ran: EIC q0 (see step 2). Its other two extra wrong answers are ESADE's, which ran only in the final round.

### Step 1: the 17 wrong answers of round 1 (step 4)

For each wrong cell: the class (a) menu or navigation label, (b) right type with the wrong entity, (c) stale content, (d) other non-answer; whether the correct answer was on a page jurl read; and the final round's verdict.

| sheet, front door | question | step 4 answer | class | correct answer on a page read? | final |
|---|---|---|---|---|---|
| R2 K Fund | How do you send them a pitch? | the 2020 K Founders call form | (c) stale | no: only the 2020 blog; no current pitch route on the pages read | wrong (unchanged) |
| R2 Nauta | How do you send them a pitch? | "Fale conosco" | (a) menu label | no: a JS front page; its only text is the menu copy | wrong (unchanged) |
| R3 ENISA | What is the maximum amount? | "9.964" (loans disbursed) | (d) non-answer | no: a count on the home page; the maximum is not on it | wrong (unchanged) |
| R3 Acelerapyme (Kit Digital) | What is the maximum amount? | "40 millones de euros" (AI initiative budget) | (b) wrong entity | no: the per-company maximum is not on the page read | wrong (unchanged); sharpened: not found |
| R3 EIC | What is the maximum amount? | "€6,5 billion" (total support) | (b) wrong entity | no: the per-project maximum is not on the pages read | wrong (unchanged); sharpened: not found |
| R4 South Summit | What are the dates of the next edition? | "De 3 a 5 de junho de 2026" (a past edition) | (c) stale | no: the next edition's dates are not on the pages read | wrong (unchanged) |
| R4 South Summit | How much is a general ticket? | "299€" (Startup Pass) | (b) wrong entity | no: only the startup pass is priced on the pages read | wrong (unchanged); sharpened: still 299€ |
| R4 Slush | How much is a general ticket? | "395€*" (startup ticket) | (b) wrong entity | no: only the startup ticket is priced on the pages read | wrong (unchanged); sharpened: not found (closest 1195€, not accepted) |
| R5 ESADE | Is there an online version? | "Formato: Online, 2.900 €" (an executive programme) | (b) wrong entity | no: the flagship MBA's online version is not on the pages read | not run in round 2; final: wrong ("Online", a label) |
| R5 ESADE | How long does it last? | "Weeks of April 13th and April 20th" (campus dates) | (d) non-answer | no: the pages read give no duration | not run in round 2; final: wrong (unchanged) |
| R5 IE Business School | How long does it last? | "TWO-WEEK MODULE OPTIONS" (a card label) | (a) label | no: no programme duration on the pages read | wrong (unchanged) |
| R5 IE Business School | Is there an online version? | "Liquid Learning ..." (a slogan) | (d) non-answer | **yes**: the ranking badge "WORLDWIDE - ONLINE MBA" is on ie.edu/business-school, read | wrong (final: "EVENT FORMAT Online events", a label: (a)) |
| R5 IESE | Is there an online version? | "Online" (a link-list label, then a card label) | (a) label | **no**: the page read (iese.edu/online-programs) is about online executive programmes; no page states the flagship MBA's online status | wrong (unchanged) |
| R5 Ironhack | How long does it last? | "bis zu 1 Jahr nach Abschluss" (the German FAQ's career support) | (d) non-answer | no: no bootcamp length on the pages read | wrong (unchanged) |
| S2 figma.com | the monthly price of the cheapest paid plan | "$3/mo" (a collab seat) | (b) wrong entity | **yes**: "Professional, Full seat, $16/mo" is on the pricing page read | wrong (final: "US$3/bulan", the collaborator seat again); sharpened "full seat": **$16/mo, correct** |
| S3 cloudflare.com | Do they have a SOC 2 Type II report? | "Meet SOC 2, PCI DSS … requirements" | (d) non-answer | no: no Type II claim on the 11 pages read | **not found** (fixed) |
| S3 vercel.com | Can customers choose where their data is stored? | Functions region ("Hobby customers can select … for Serverless Functions") | (b) wrong entity | no: no data-location text on the pages read | wrong (unchanged); sharpened "data, not Functions": **correct (Vercel Blob regions)** |

Count: (a) 3, (b) 7, (c) 2, (d) 5. Of the 17, 16 are still wrong in the final round (ESADE's two are measured now) and 1 is not found (Cloudflare). Where the correct answer was on a read page (IE's badge, Figma's price), the failure is `--precise`'s pick, not the search.

### Step 2: class (a), the navigation flag: tried, reverted

What the extractor already did: it skips `<nav>`, `<footer>`, `<aside>`, `<form>`, the masthead `<header>`, elements with roles navigation, complementary, contentinfo, search, menu and menubar, `aria-hidden` and `hidden` (src/extract/html.rs). The leaks are outside those landmarks: a link list inside a content paragraph (IESE's program types: "Focused Programs | Online | …" are five links with separators), a card label (IESE's "Online", IE's "TWO-WEEK MODULE OPTIONS"), and a Wix `role="region"` copy of the footer menu (Nauta, which the search read from its rendered page).

Tried, measured as rounds, all reverted:
- **r2a** (navigation-like blocks dropped from the page: link share ≥ 0.8, one link included, banner role): lost correct answers (Lexington's day pass, Ironhack's remote format, bitbucket's path). Dropping blocks changed the page's warmth too, so the search went elsewhere. Net worse.
- **r2a2** (dropped, two or more links): the same kind of loss (Lexington, bitbucket). Net worse.
- **r2a5** (flagged, not dropped: `--precise` takes no answer from a flagged block; no step 5): 62 / 16 / 29. Its verdict moves against step 4 were GitLab (not found → a wrong sentence) and the three ESADE cells (not run). IESE's list block was no longer the pick, but the card label "Online" took its place (still wrong).
- **r2a3** (flag, step 5, banner role) and **r2a4** (flag, step 5): the banner moved no verdict (Ironhack's text only), so it was dropped. r2a4 is 64 / 15 / 28, with ESADE not run.

What the flag did once step 5 was in: EIC q0 is **correct** with the flag (the 17 December 2026 deadline) and **wrong** without it (the 13 September 2024 tender deadline). The flag alone (r2a5) gives not found, and step 5 alone gives the 2024 deadline, so the correct answer needs both. Utopicus q1's pick also depends on the flag ("10 pases de día" with it, "Bono 10 pases de día Working Pass a utilizar" without; both correct).

Why it was dropped. The coordinator's review of round 2 asked for the flag to go. Its premise was that the flag measured zero; it did not, and the EIC cell above is the measured effect. The coordinator dropped it anyway: the 80% link share and the two-link guard are chosen values, not a standard (the owner's no-ad-hoc rule), so the cost of dropping it is that one answer. The final is step 4 + step 5 without it. The navigation leaks of class (a) stay open: IESE's "Online", IE's card label, and Nauta's footer copy. A rule over whole lists, or Jev refusing labels, would be the next step; neither is a standard in the HTML sense yet.

### Step 3: the linked hosts' sitemaps (step 5), kept

Step 5: a linked same-registrable-domain host's `/sitemap.xml` and the sitemaps its `robots.txt` names are read, as the start site's own are.

- With the flag (r2a4 against r2a5): +2 correct (Stripe's rate-limit table, "100 requests per second", from docs.stripe.com's own sitemap; EIC's deadline), −1 wrong (Cloudflare's non-answer becomes not found).
- Without the flag (final against round 1's step 4): +1 correct (Stripe: not found → "100 requests per second"), +1 wrong (EIC: not found → the 2024 tender deadline), −1 wrong (Cloudflare: wrong → not found). GitLab moved from not found to a wrong sentence, but that is site drift and not step 5: the round-1 step-4 binary, run today, gives a wrong GitLab sentence too.
- VivaTech q0: round 1's step 5 answered "Du 17 au 20 juin" (the 2026 edition, wrong) from the editorial page, which returned HTTP 200 then. In the final run that page returned HTTP 403, so jurl answered not found. The site's answers vary between identical requests (VivaTech's page titles change from one request to the next), so this cell is not stable.
- Text changes with the same verdict: Utopicus q1 (the flag, above), IE q2 and ESADE q2 (labels, both wrong), Figma q0 ("$3/mo" → "US$3/bulan", a collaborator seat, wrong; the cause is not isolated).

### Step 4 (round 1's (b) question): sharpened questions

Re-run only, not a change to jurl. Final binary, English question, one front door each (sheets B1–B7 in sheets.json; run folder `sharp-final`, the same verdicts as the earlier sharpened run except ESADE, which was not run then):

| cell | sharpened question | answer | verdict | does the specificity fix it? |
|---|---|---|---|---|
| Kit Digital | What is the maximum Kit Digital aid per company? | not found (no page read has it) | not found | wrong → not found: removes the 40 million, no right answer found |
| EIC | What is the maximum funding per project? | not found (closest "€10 million", p 0.37) | not found | wrong → not found |
| South Summit | How much is the general admission ticket (not startup or investor passes)? | "299€" (still the startup pass) | wrong | no |
| Slush | How much is the general admission ticket (not startup or investor passes)? | not found (closest "1195€", p 0.27) | not found | wrong → not found |
| ESADE | Can you take the full-time MBA online? | "This is a full-time program, … dedicate yourself full-time to your studies" | wrong | not measured before (not run); a non-answer about time, not format. The page says the full-time MBA is on campus; the quote does not |
| Figma | What is the monthly price per full seat of the cheapest paid plan (not a collab or dev seat)? | "$16/mo" (Professional, full seat) | **correct** | **yes**: wrong → correct |
| Vercel | Can customers choose the region where their data is stored, not only where Serverless Functions run? | "You can create Blob stores in any of the 19 regions" | **correct** (Vercel Blob storage) | **yes**: wrong → correct |

So specificity fixed 2 of the 6 cells that were wrong (Figma, Vercel), turned 3 wrong answers into not found (Kit Digital, EIC, Slush: the right entity was not on the pages read), and left South Summit wrong (its only ticket on the pages read is the startup pass). For the planner prompt: the question has to name the entity (a full seat, the data rather than the compute, the flagship programme) and the unit; a sharper question does not make a missing page appear.

### Accept-Language: the default is none (the owner's decision)

After round 2 the owner decided: jurl sends **no** Accept-Language unless the run sets `JURL_ACCEPT_LANGUAGE`, and the caller says which language the question is in (the cloud runner will pass it). The round-1 default was `en` (commit 019ed1d, now changed in place). The browser-fingerprint retry keeps the line its emulation profile sends, as it did before this option existed; jurl adds nothing to it. Tests cover both cases: unset sends no header from the plain client, and a set value is sent exactly once by each client.

The evidence is the Stripe pair, all through the final binary (`risk-en-final`, `risk-es-final`, `risk-none`, `final-none`):

| cell | `JURL_ACCEPT_LANGUAGE=en` | none (the default now) | `es` |
|---|---|---|---|
| K1 Stripe, Spanish question | "100 requests per second" (English): correct, but an English answer to a Spanish question | "100 peticiones por segundo" (Spanish, from the Spanish docs this IP is served without a header): correct | "100 peticiones por segundo": correct |
| S1 Stripe, English question (the English question's answer) | "100 requests per second" (English docs): correct | "100 peticiones por segundo" (Spanish docs): correct, the same fact | not run |
| S1 Shopify (`shopify.com` → `/es-es`) | not found (the English Shopify Spain page) | not found (the Spanish Shopify España page) | not run |

Which cells depended on the old `en` default was measured, not assumed. A screen fetched every page the `en`-run cells read (and the front doors), with and without the header, and compared the final URL, status, `<html lang>`, title and visible-text length. Three cells differ for real, and sequential repeats confirm it: S1 Stripe q0 and K1 (docs.stripe.com and stripe.com answer in Spanish without the header, from this IP), and S1 Shopify q0 (shopify.com goes to the Spanish storefront without the header). The screen also flagged ten cells that differ only through noise: VivaTech's page titles and Hawkers' text length changed between parallel requests and not between sequential ones. The robots.txt, sitemap.xml and llms.txt of all 44 front doors were checked too; none depends on the header. The Lightpanda-rendered cells (40) get no header from jurl (the renderer is called without one), so only jurl's own plain fetch, which decides whether to render, could depend on it; that fetch was in the screen. K2 (Ironhack, Spanish question) reads pages that are the same under both settings: not re-run.

The three re-runs (`final-none`, `risk-none`) move no verdict: S1 Stripe q0 correct, S1 Shopify not found, K1 correct. They do change the language of two answers. Without a header, a site that answers by where the request comes from (Stripe and Shopify from a Spanish IP) answers in Spanish, which suits K1 and not the English question. That is the trade-off the caller resolves by setting the language.

### Reach: Shopify and GitLab

- **Shopify** (open question, not implemented): the search reaches `shopify.dev/docs/api/usage/limits`, a hub whose table names the REST Admin API ("Request-based bucket and headers") but gives no figure. The figure is on the REST Admin API page, one hop past the hub. With the default budget (5 pages) it is not reached. With `--follow 10` (a probe, about $0.006) it is not reached either: the search takes the Storefront pages and answers "None for buyer traffic" (wrong). What would reach it: read the robots.txt sitemaps of any host the search reaches, not only the same-domain hosts the front door links (step 5, generalised). shopify.dev's sitemap lists `/docs/api/admin-rest/usage/rate-limits`, which the existing overlap preselection (`links::most_relevant`) and Jev's lead scoring would then rank. Unmeasured; not implemented.
- **GitLab**: the search reaches `docs.gitlab.com/user/gitlab_com/rate_limits/` (through a blog link, a cross-domain candidate, in every round since r2a5). That page gives the rules (an hourly limit per plan, a per-minute burst limit, "Authenticated requests receive your plan's full allowance") and no figure. The picks are sentences: "A sustained limit, measured each hour" (final), "full allowance" (r2a5), "Authenticated requests receive…" (the round-1 step-4 binary, run today). The figure, if GitLab states one, is on another page this search does not reach.

### Changed-reach not-found cells (round 2a4 against step 4)

Five not-found cells read different pages in round 2a4, and the final round has the same not-found cells: Talent Garden (hot desk price and day pass: seven and ten pages read; no desk price or day pass in them, `not_on_site`), Lexington (hot desk price: six pages read; the prices are for private offices, `not_on_site`), IE (flagship MBA price: nine pages read; no fee in them, `not_on_site`), and Shopify (`not_reached`, as above). The other not-found classes are carried from the round they were first lost in (RESULTS.md marks them), unverified beyond jurl's own traces.

### Cost, CI and size

- Billed: round 1 $0.54 (per cell); round 2 about $0.13 (the proxy's count, which includes calls whose results were later discarded and probes). Cumulative $0.675 of the $1.04 cap. The round-2 cap of $0.50 was not reached.
- Binary: 12,065,040 bytes for the final build (step 4: 12,048,496; step 5 adds 16,544). The flag build (r2a4) is the same size with different bytes.
- Gates: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`: 259 passed, 1 ignored (after the Accept-Language change).

## Round 3: the holdout, the step-0 diagnosis, and the changes measured

Round 3 starts from #52's final head (`8ceed0e`) and is measured with the same bench, the same cache proxy and the same judging. Budget $1.00 billed (cumulative $0.675 before it; the cap was raised to $1.675). Round 3 spent about $0.24 in all: the bench rounds (each mostly cache hits), the two holdout runs, and the probes.

### Holdout (45 cells: four real-world sheets of ten, one developer sheet of five)

`holdout.json`, written and committed before any code change. The front doors were chosen for reachability and category (coworkings in Barcelona, EU accelerators, Spanish e-commerce returns and shipping, tech conferences, SaaS pricing), none of them in the bench. Judged by hand at the start (the baseline binary) and at the end (the final binary), and not used to tune. Zalando (H3-0-0) timed out in the start run and was answered not found on the second try.

| holdout | start (baseline) C / W / NF | end (final) C / W / NF |
|---|---:|---:|
| real-world, 40 cells (H1–H4) | 9 / 10 / 21 | 10 / 12 / 18 |
| developer, 5 cells (H5) | 4 / 1 / 0 | 4 / 1 / 0 |
| all 45 | 13 / 11 / 21 | 14 / 13 / 18 |

Cells that moved on the holdout (start → end):

- H1-4-1 betahaus (Can you buy a day pass?): wrong → wrong. betahaus's Kreuzberg (Berlin) day pass, wrong for a Barcelona sheet in both runs
- H2-4-0 Rockstart (How much equity does the programme take?): not_found → correct. the programme's own terms: 6% equity for USD 100k (site search second pass)
- H4-3-0 GITEX Europe (What are the dates of the next edition?): not_found → wrong. a JSON-LD event of the 2026 edition, which ended on 1 July 2026: the fallback takes the first dated event, with no check that it is still ahead
- H4-3-1 GITEX Europe (How much is a general admission ticket?): not_found → wrong. the 2026 edition's visitor pass (JSON-LD offer), the same stale-edition problem

The holdout gains less than the bench and takes two wrong answers, both the same flaw: the JSON-LD fallback accepts a dated event without asking whether it is still ahead. That is reported as the remaining failure, not fixed on the holdout.

### Step 0: diagnosis of the 46 cells that are wrong or not found today (no code)

Counts over the 46 cells (wrong 18, not found 28):

- (a) a declared search (OpenSearch, SearchAction or WordPress REST): 5 cells (SearchAction, SearchAction,WP-REST); OpenSearch none.
- (b) a relevant JSON-LD type on a read page (Event, Offer, Product, Course, SoftwareApplication, Service): 11 cells.
- (c) a PDF linked from a read page: 15 cells (42 links).
- (d) a read page in another language than the question: 14 cells.
- (e) 20 or more candidates (sitemap URLs plus front-page links) against five pages read: 39 cells; median 92.

Per cell (candidates = sitemap URLs listed + links on the front page; read = pages read):

| cell | site | verdict | answer (closest) | (a) search | (b) JSON-LD on read pages | (c) PDFs on read pages | (d) page lang ≠ question | (e) candidates / read |
|---|---|---|---|---|---|---:|---|---:|
| R1-0-0 | Impact Hub Madrid | not_found | 120€ Contratar online | SearchAction,WP-REST | Offer,Service | 0 | es | 84 / 5 |
| R1-1-2 | Utopicus | not_found | 4-8 | none | - | 0 | es | 595 / 5 |
| R1-2-0 | Talent Garden Madrid | not_found |  | none | - | 0 | - | 267 / 5 |
| R1-2-1 | Talent Garden Madrid | not_found | Fixed desks, hourly packages or ac | none | - | 0 | - | 267 / 5 |
| R1-3-0 | Lexington | not_found | Alquiler zonas comunes € Electrici | none | Offer,Service | 0 | es | 92 / 5 |
| R2-0-0 | seaya.vc | not_found | Your founder-friendly financing so | none | - | 0 | - | 0 / 5 |
| R2-0-1 | seaya.vc | not_found | €300m | none | - | 0 | - | 0 / 5 |
| R2-1-0 | Kibo Ventures | not_found | We lead or co-lead pre-Series A or | none | - | 2 | - | 20 / 5 |
| R2-1-1 | Kibo Ventures | not_found | No | none | - | 2 | - | 20 / 5 |
| R2-1-2 | Kibo Ventures | not_found | Tiene derecho a acceder, rectifica | none | - | 2 | - | 20 / 5 |
| R2-2-2 | K Fund | wrong | We have already launched the first | none | - | 0 | - | 369 / 3 |
| R2-3-0 | Samaipata | not_found | Investment Thesis | none | - | 0 | - | 442 / 3 |
| R2-3-1 | Samaipata | not_found | €8M | none | - | 0 | - | 442 / 4 |
| R2-4-0 | JME Ventures | not_found | We are seed stage investors in som | none | - | 1 | - | 47 / 5 |
| R2-4-2 | JME Ventures | not_found | We prioritize speed & clarity, off | none | - | 1 | - | 47 / 5 |
| R2-5-0 | Nauta | not_found |  | none | - | 5 | pt | 12 / 4 |
| R2-5-1 | Nauta | not_found |  | none | - | 3 | pt | 12 / 4 |
| R2-5-2 | Nauta | wrong | Fale conosco | none | - | 0 | pt | 12 / 1 |
| R3-0-1 | ENISA | wrong | 9.964 | none | - | 0 | - | 36 / 1 |
| R3-1-0 | CDTI | not_found |  | none | - | 0 | - | 0 / 1 |
| R3-1-1 | CDTI | not_found |  | none | - | 0 | - | 0 / 1 |
| R3-2-1 | Acelerapyme (Kit Digit | wrong | 40 millones de euros | none | - | 0 | es | 2310 / 1 |
| R3-3-0 | EIC | wrong | 13 September 2024 - 10:00 CEST | none | CreativeWork,Offer | 3 | - | 1818 / 5 |
| R3-3-1 | EIC | wrong | €6,5 billion | none | - | 0 | - | 1818 / 1 |
| R4-1-0 | 4YFN | not_found | 2027 | none | Event | 0 | - | 66 / 5 |
| R4-1-1 | 4YFN | not_found |  | none | Event | 1 | - | 66 / 5 |
| R4-2-0 | South Summit | wrong | De 3 a 5 de junho de 2026 | none | - | 0 | es,pt | 82 / 5 |
| R4-2-1 | South Summit | wrong | 299€ | none | Event | 1 | - | 82 / 5 |
| R4-3-0 | VivaTech | not_found |  | SearchAction | Event | 0 | - | 41 / 5 |
| R4-3-1 | VivaTech | not_found |  | SearchAction | Event | 0 | - | 41 / 5 |
| R4-4-1 | Slush | wrong | 395€* | none | - | 1 | - | 394 / 2 |
| R5-0-0 | IE Business School | not_found | The cost can vary depending on the | none | - | 0 | - | 98 / 5 |
| R5-0-1 | IE Business School | wrong | TWO-WEEK MODULE OPTIONS | none | - | 2 | - | 98 / 5 |
| R5-0-2 | IE Business School | wrong | EVENT FORMAT Online events | none | - | 0 | - | 98 / 5 |
| R5-1-1 | ESADE | wrong | Weeks of April 13th and April 20th | none | - | 15 | es | 627 / 5 |
| R5-1-2 | ESADE | wrong | Online | none | - | 0 | - | 627 / 5 |
| R5-2-2 | IESE | wrong | Online | SearchAction,WP-REST | - | 0 | - | 142 / 3 |
| R5-3-0 | Ironhack | not_found | 6.750€ | none | - | 2 | es | 24 / 5 |
| R5-3-1 | Ironhack | wrong | bis zu 1 Jahr nach Abschluss (oder | none | - | 0 | de | 24 / 3 |
| S1-1-0 | gitlab.com | wrong | A sustained limit, measured each h | none | Article | 0 | de | 9581 / 5 |
| S1-4-0 | shopify.com | not_found |  | none | - | 0 | es | 728 / 5 |
| S2-5-0 | posthog.com | not_found | $0.000015/row | none | Offer,SoftwareApplication | 0 | - | 52 / 5 |
| S2-6-0 | figma.com | wrong | US$3/bulan | none | - | 1 | id | 1356 / 5 |
| S2-7-0 | asana.com | not_found |  | none | Offer,Product | 0 | - | 136 / 5 |
| S3-0-1 | vercel.com | wrong | Previously, Hobby customers could  | none | Offer,Service,SoftwareApplication | 0 | - | 8696 / 3 |
| S3-5-0 | cloudflare.com | not_found |  | SearchAction | - | 0 | - | 970 / 5 |

Order of the candidate changes, by expected gain from this table. Candidates, not gains: the first two are cheap, standard, and each touches cells that the table marks.

1. **JSON-LD values** (5): 11 cells have a relevant type on a read page, and the answer is a plain value in it for some (VivaTech's and 4YFN's dates, Asana's price). Cheap, standard, measured below.
2. **The site's own search** (1): 5 cells declare one (VivaTech, Impact Hub, IESE, Cloudflare). Measured below.
3. **Skim many, read few** (2): 39 cells have 20 or more candidates against five pages read. The largest population, but the least certain payoff, and it changes the order of leads for every cell. Measured below as a second pass.
4. **PDFs** (4): 15 cells link PDFs from read pages. Measured on the linked PDFs of nine failing cells (below): mostly decks and terms; jurl has no PDF reader, so this needs a new dependency. Not done.
5. **Labelled lists** (6): the ticket questions (South Summit, Slush, VivaTech, 4YFN, MWC, GITEX). Not implemented in this round; see the remaining failures.
6. **The question in the page's language** (3): 14 cells read a page in another language. jurl has no translation path, and Jev is a decision model, not a translator; no dictionaries are allowed. Said plainly: no clean way, so it is not done.

### Rounds

| round | what it is | real-world C / W / NF | developer C / W / NF | all 110 C / W / NF | correct | decision |
|---|---|---|---|---|---|---|
| r3-base | same-day baseline (8ceed0e) | 46 / 15 / 24 | 18 / 3 / 4 | 64 / 18 / 28 | 58.2% | baseline |
| r3-jsonld | JSON-LD values scored with the text pool (first attempt) | 47 / 16 / 22 | 18 / 4 / 3 | 65 / 20 / 25 | 59.1% | rejected: wrong +2 |
| r3-json3 | JSON-LD values when the text gives no answer | 47 / 15 / 23 | 18 / 3 / 4 | 65 / 18 / 27 | 59.1% | kept |
| r3-search2 | site search in the first pass (on top of JSON-LD) | 45 / 16 / 24 | 17 / 3 / 5 | 62 / 19 / 29 | 56.4% | rejected: correct −3 |
| r3-sp1 | site search as a second pass when nothing is found (on top of JSON-LD) | 50 / 14 / 21 | 18 / 3 / 4 | 68 / 17 / 25 | 61.8% | kept |
| r3-sk1 | skim: titles of the best unread leads, second pass (on top of site search) | 49 / 17 / 19 | 19 / 3 / 3 | 68 / 20 / 22 | 61.8% | rejected: wrong +3 |

Cells that moved, and why (each round against the round it was built on):

**r3-jsonld** against **r3-base**:
- R1-0-0 Impact Hub Madrid (What is the monthly price of a flexible (hot) desk): not_found → wrong; answer 'Jornada Completa
220€
Contratar online
Mañana
150€
Contratar'
- R4-3-0 VivaTech (What are the dates of the next edition?): not_found → correct; answer 'startDate 2027-06-16, endDate 2027-06-19'
- S2-5-0 posthog.com (What is the monthly price of the cheapest paid pla): not_found → wrong; answer '$0.000015/row'

**r3-json3** against **r3-base**:
- R4-3-0 VivaTech (What are the dates of the next edition?): not_found → correct; answer 'Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate'

**r3-search2** against **r3-json3**:
- R1-0-0 Impact Hub Madrid (What is the monthly price of a flexible (hot) desk): not_found → wrong; answer '120€'
- R6-3-1 Scalpers (How many days do you have to return an item?): correct → not_found; answer '15 días laborables'
- R6es-3-1 Scalpers (¿Cuántos días tengo para devolver un artículo?): correct → not_found; answer '15 días laborables'
- S1-2-0 bitbucket.org (What is the primary rate limit for authenticated r): correct → not_found; answer ''

**r3-sp1** against **r3-json3**:
- R1-0-0 Impact Hub Madrid (What is the monthly price of a flexible (hot) desk): not_found → correct; answer '120€/month'
- R2-0-1 seaya.vc (What is their typical first ticket?): not_found → correct; answer '€10-40m'
- R3-3-0 EIC (When is the application deadline?): wrong → correct; answer '17 December 2026 17:00:00 Brussels time'

**r3-sk1** against **r3-sp1**:
- R1-0-0 Impact Hub Madrid (What is the monthly price of a flexible (hot) desk): correct → wrong; answer '120€'
- R1-1-2 Utopicus (Is it open 24/7?): not_found → correct; answer '24/7'
- R2-0-1 seaya.vc (What is their typical first ticket?): correct → not_found; answer '€22 million'
- R2-1-1 Kibo Ventures (What is their typical first ticket?): not_found → correct; answer '€2m - €6m'
- R3-3-0 EIC (When is the application deadline?): correct → wrong; answer '13 September 2024 - 10:00 CEST'
- R4-1-0 4YFN (What are the dates of the next edition?): not_found → wrong; answer '2 to March 5'
- S2-7-0 asana.com (What is the monthly price of the cheapest paid pla): not_found → correct; answer '$13.49 per user'

### Remaining failures (today's kept head, round r3-sp1: 68 correct, 17 wrong, 25 not found)

- Not found, in the groups the table names: reach (the answer is on a page never read: Lexington, Talent Garden, Samaipata, JME, Kibo, IE's price, Ironhack's price, Antler and Seedcamp equity, Acelerapyme, Slush tickets, GitLab's figure); a page without the answer's words (CDTI, ENISA's maximum); JSON-LD with a value that is not the answer (posthog's usage price, Impact Hub's offer).
- Wrong: label or menu picks (IE's "TWO-WEEK MODULE OPTIONS" and "EVENT FORMAT Online events", IESE's and ESADE's "Online", Nauta's "Fale conosco"); the wrong entity (South Summit's startup pass, Slush's startup ticket, Acelerapyme's AI budget, EIC's total support, figma's collaborator seat); stale pages (EIC's 2024 tender deadline, 4YFN's dates, GITEX on the holdout).
- The navigation-flag sweep is not done. The diagnosis has five label picks (above), up from one (EIC) in round 2, which the coordinator's note asked to reconsider with a measured sweep. A sweep would need the flag back (it is in the history of #52) and at least three thresholds; not run in this round.

### Findings outside the changes

- **Hand-written lists that break the no-list rule**, already in `main` or in #46's lineage, not added in round 3: `STOP` in `src/links.rs` (`overlap`'s stop words, used to rank links and sitemap URLs), `ASSETS` in `src/follow.rs` (file extensions) and `LOCALES` in `src/follow.rs` (locale path segments). Each would be replaced by a standard: the response's Content-Type for `ASSETS`, `hreflang` alternates for `LOCALES`, and a data-driven filter for `STOP`. Not changed here, so that round 3's results stay comparable.
- The two kept changes have chosen parameters: the JSON-LD threshold is the precise threshold (0.4); the second pass opens three more pages (`SECOND_PASS`). Neither was swept.

### Tests and gates

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked` pass on the kept head: 268 passed, 1 ignored (the new tests: JSON-LD parsing, the fallback's storage apart from the text, the WordPress, OpenSearch and SearchAction parsers, and the template fill).


