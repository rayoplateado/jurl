# --follow from a front door: benchmark

Measured 2026-10-10 with `bench/front-doors/run.py`. Every cell is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`; `-t` only adds stderr. Jev is behind `jev_proxy.py`, which answers an identical request from a cache and forwards the rest. Real-world sheets (R1-R6, R6es) come first everywhere.

**Final state = step 4.** Its binary is byte-identical to the final build (12,048,496 bytes). Step 5 was measured and reverted.

## Headline

| set | round | cells | correct | wrong | not found | not run |
|---|---|---:|---:|---:|---:|---:|
| real-world | baseline | 85 | 45 | 19 | 21 | 0 |
| real-world | step 4 (round 1 final) | 85 | 46 | 14 | 25 | 0 |
| real-world | r2a4 (round 2 final) | 85 | 46 | 12 | 24 | 3 |
| developer | baseline | 25 | 16 | 3 | 6 | 0 |
| developer | step 4 (round 1 final) | 25 | 17 | 3 | 5 | 0 |
| developer | r2a4 (round 2 final) | 25 | 18 | 3 | 4 | 0 |
| all | baseline | 110 | 61 | 22 | 27 | 0 |
| all | step 4 (round 1 final) | 110 | 63 | 17 | 30 | 0 |
| all | r2a4 (round 2 final) | 110 | 64 | 15 | 28 | 3 |

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

**Step 4 (FINAL): Accept-Language `en` by default, `JURL_ACCEPT_LANGUAGE` overrides**

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

**Step 5 (tried, REVERTED): the sitemaps of linked hosts, read through their robots.txt**

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

**Round 2a: navigation-like blocks dropped (link share 0.8, one link included), with banner**

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

**Round 2a2: navigation-like blocks dropped (two or more links), with banner**

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

**Round 2a5: navigation-like blocks are no precise answer (flagged, warmth kept), no step 5**

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
| B1 | 1 | 0 | 0 | 1 | 5.0 | 97,097 | 97,097 | 0.0041 |
| B2 | 1 | 0 | 0 | 0 | 5.0 | 83,326 | 83,326 | 0.0035 |
| B3 | 1 | 0 | 1 | 0 | 5.0 | 107,880 | 107,880 | 0.0045 |
| B4 | 1 | 0 | 0 | 1 | 2.0 | 84,686 | 84,686 | 0.0036 |
| B5 | 1 | 0 | 0 | 0 (+1 not run) | 0.0 | 0 | 0 | 0.0000 |
| B6 | 1 | 1 | 0 | 0 | 3.0 | 68,281 | 68,281 | 0.0029 |
| B7 | 1 | 1 | 0 | 0 | 5.0 | 110,477 | 110,477 | 0.0046 |
| K1 | 1 | 0 | 0 | 1 | 5.0 | 92,935 | 88,515 | 0.0037 |
| K2 | 1 | 0 | 0 | 1 | 5.0 | 93,411 | 93,411 | 0.0039 |
| **all** | 119 | 64 | 17 | 33 (+4 not run) | 3.7 | 71,098 | 755,258 | 0.0317 |

Failures by class (r2a5): wrong_answer 17, not_reached (carried) 14, not_on_site (carried) 6, unclassified 4, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 2.

**Round 2a3: as 2a5 with step 5 and the banner role**

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

**Round 2a4 = FINAL: navigation-like blocks are no precise answer, with step 5 (linked hosts' sitemaps)**

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

## Final round, cell by cell

Verdict per cell. A correct or wrong answer is judged by its quote, with the one-line reason from `judgements.json`. A cell with no answer gives its class (see the legend) and the reason from `notfound.json`.

| sheet | row | question | answer (closest) | verdict | reason |
|---|---|---|---|---|---|
| R1 | Impact Hub Madrid | What is the monthly price of a flexible (hot) desk? | 120€
Contratar online | not_found | not_on_site. Re-verified in round 2a4: the price pages read give private-office rates ("Oficina flexible ... €/mes"), no hot-desk price. |
| R1 | Impact Hub Madrid | Can you buy a day pass? | Ven cuando lo necesites. Adquiere tu pase de un día o un bono de 10 dí | correct | "Adquiere tu pase de un dia o un bono de 10 dias": a day pass can be bought. |
| R1 | Impact Hub Madrid | Is it open 24/7? | 24/7 | correct | The plans list "Acceso 24/7" (24/7 access for members), so the space is open round the clock for that plan. |
| R1 | Utopicus | What is the monthly price of a flexible (hot) desk? | 259€/mes | correct | The Utopicus Passport, its flexible workstation, is listed at 259 EUR/mes ("Comfortable workstation 24/7 access"). |
| R1 | Utopicus | Can you buy a day pass? | 10 pases de día | correct | "Bono 10 pases de dia Working Pass": a pack of 10 day passes is sold. |
| R1 | Utopicus | Is it open 24/7? | 4-8 | not_found | missed_on_read (carried). (carried from step4) The same hours page as in step 3: read, not picked. |
| R1 | Talent Garden Madrid | What is the monthly price of a flexible (hot) desk? |  | not_found | not_on_site. Re-verified in round 2a4: 7 pages read (coworking and knowledge-base pages), no desk prices in them. |
| R1 | Talent Garden Madrid | Can you buy a day pass? | Fixed desks, hourly packages or access to the digital community | not_found | not_on_site. Re-verified in round 2a4: 10 pages read, no day-pass text in them. |
| R1 | Talent Garden Madrid | Is it open 24/7? | 24/7 access in all TAGs | correct | "24/7 access in all TAGs" (TAG = Talent Garden campus) on the front page. |
| R1 | Lexington | What is the monthly price of a flexible (hot) desk? | Alquiler zonas comunes
€
Electricidad
€
Comunidad
€
Mantenimiento
€
In | not_found | not_on_site. Re-verified in round 2a4: the prices read are for private offices, not for a hot desk. |
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
| R3 | EIC | When is the application deadline? | 17 December 2026 17:00:00 Brussels time | correct | The EIC Accelerator call HORIZON-EIC-2026-ACCELERATOR-01 closes 17 December 2026 17:00 Brussels time, on the EU funding portal. |
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
| R4 | VivaTech | What are the dates of the next edition? |  | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (information, practical information, get-your-pass) |
| R4 | VivaTech | How much is a general ticket? |  | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (get-your-pass) |
| R4 | VivaTech | In which city is it held? | Paris | correct | "VivaTech, the Paris tech conference". |
| R4 | Slush | What are the dates of the next edition? | Nov 18–19, Helsinki | correct | "Slush 2026 Nov 18-19, Helsinki": the next edition. |
| R4 | Slush | How much is a general ticket? | 395€* | wrong | EUR 395 is the startup ticket ("Tickets to Slush 2026 startup 395 EUR"), not the general ticket. |
| R4 | Slush | In which city is it held? | Helsinki | correct | "Helsinki is the home of Slush since 2008". |
| R5 | IE Business School | What is the price of the flagship MBA or bootcamp? | The cost can vary depending on the program | not_found | not_on_site. Re-verified in round 2a4: 9 pages read, no MBA or bootcamp fee in them; the fee page was not reached. |
| R5 | IE Business School | How long does it last? | TWO-WEEK MODULE OPTIONS | wrong | "TWO-WEEK MODULE OPTIONS" is a menu label; the duration of the MBA is not given. |
| R5 | IE Business School | Is there an online version? | EVENT FORMAT Online events | wrong | "EVENT FORMAT Online events" is an event-format label, not an online MBA. |
| R5 | ESADE | What is the price of the flagship MBA or bootcamp? |  | not_run | not run: fetching https://esade.edu/: error sending request for url (. jurl made no call: no verdict until the cell is re-run. |
| R5 | ESADE | How long does it last? |  | not_run | not run: fetching https://esade.edu/: error sending request for url (. jurl made no call: no verdict until the cell is re-run. |
| R5 | ESADE | Is there an online version? |  | not_run | not run: fetching https://esade.edu/: error sending request for url (. jurl made no call: no verdict until the cell is re-run. |
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
| S1 | shopify.com | What is the primary rate limit for authenticated requests? |  | not_found | not_reached. Re-verified in round 2a4: the hub /docs/api/usage/limits is read; its table names the REST Admin API but gives no figure; the REST Admin API page (one hop past the hub) is not reached. --follow 10 takes the Storefront pages instead and answers wrongly. |
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

## Round 2: the wrong answers, the navigation fix, the linked sitemaps, Accept-Language

Round 2 had a cap of $0.50 billed on top of round 1's $0.54. It spent $0.12 (the proxy's log: $0.666 cumulative against $1.04). Every round ran all 110 cells; the sharpened and risk sheets are separate. ESADE (esade.edu) was unreachable from this machine in every round-2 run (timeouts, curl included): its three main cells and its sharpened cell are **not run**, not wrong or not found. They were not re-run when the site was reachable again, because it never was.

### Headline

| set | baseline | step 4 (round 1 final) | **round 2 final (r2a4)** |
|---|---|---|---|
| all 110 cells (C / W / NF) | 61 / 22 / 27 | 63 / 17 / 30 | **64 / 15 / 28** (+3 not run: ESADE) |
| same 107 cells in all three (ESADE left out) | 61 / 20 / 26 | 62 / 15 / 30 | **64 / 15 / 28** |
| real-world, 85 cells | 45 / 19 / 21 | 46 / 14 / 25 | **46 / 12 / 24** (+3 not run) |
| developer, 25 cells | 16 / 3 / 6 | 17 / 3 / 5 | **18 / 3 / 4** |
| S1 (≥ 4/5 target) | 1/5 | 2/5 | **3/5: not met** (github, bitbucket, stripe; gitlab wrong, shopify not found) |

Targets: S1 at least 4/5 is **not met** (3/5). Total correct clearly above baseline is **met only narrowly** (61 → 64). Wrong answers not increasing: **met** (22 → 15; real-world 19 → 12).

### Step 1: the 17 wrong answers of step 4

For each wrong cell: the class (a) menu or navigation label, (b) right type with the wrong entity, (c) stale content, (d) other non-answer; whether the correct answer was on a page jurl read (`path` and the page text sent to Jev, from front-doors-cache.jsonl); and what round 2's final binary says.

| sheet, front door | question | step 4 answer | class | correct answer on a page read? | r2a4 |
|---|---|---|---|---|---|
| R2 K Fund | How do you send them a pitch? | the 2020 K Founders call form | (c) stale | no: only the 2020 blog; no current pitch route on the pages read | wrong (unchanged) |
| R2 Nauta | How do you send them a pitch? | "Fale conosco" | (a) menu label | no: a JS front page; its only text is the menu copy | wrong (unchanged) |
| R3 ENISA | What is the maximum amount? | "9.964" (loans disbursed) | (d) non-answer | no: a count on the home page; the maximum is not on it | wrong (unchanged) |
| R3 Acelerapyme (Kit Digital) | What is the maximum amount? | "40 millones de euros" (AI initiative budget) | (b) wrong entity | no: the per-company maximum is not on the page read | wrong (unchanged); the sharpened question: not found |
| R3 EIC | What is the maximum amount? | "€6,5 billion" (total support) | (b) wrong entity | no: the per-project maximum is not on the pages read | wrong (unchanged); sharpened: not found |
| R4 South Summit | What are the dates of the next edition? | "De 3 a 5 de junho de 2026" (a past edition) | (c) stale | no: the next edition's dates are not on the pages read | wrong (unchanged) |
| R4 South Summit | How much is a general ticket? | "299€" (Startup Pass) | (b) wrong entity | no: only the startup pass is priced on the pages read | wrong (unchanged); sharpened: still 299€ |
| R4 Slush | How much is a general ticket? | "395€*" (startup ticket) | (b) wrong entity | no: only the startup ticket is priced on the pages read | wrong (unchanged); sharpened: not found (closest 1195€, not accepted) |
| R5 ESADE | Is there an online version? | "Formato: Online, 2.900 €" (an executive programme) | (b) wrong entity | no: the flagship MBA's online version is not on the pages read | not run (ESADE unreachable) |
| R5 ESADE | How long does it last? | "Weeks of April 13th and April 20th" (campus dates) | (d) non-answer | no: the pages read give no duration | not run |
| R5 IE Business School | How long does it last? | "TWO-WEEK MODULE OPTIONS" (a card label) | (a) label | no: no programme duration on the pages read | wrong (unchanged) |
| R5 IE Business School | Is there an online version? | "Liquid Learning ..." (a slogan) | (d) non-answer | **yes**: the ranking badge "WORLDWIDE - ONLINE MBA" is on ie.edu/business-school, read | wrong (now "EVENT FORMAT Online events", a label: (a)) |
| R5 IESE | Is there an online version? | "Online" (a link-list label, then a card label) | (a) label | **yes**: "IESE Online Programs" (iese.edu/online-programs) was read | wrong (the card label "Online" still, see the fix below) |
| R5 Ironhack | How long does it last? | "bis zu 1 Jahr nach Abschluss" (the German FAQ's career support) | (d) non-answer | no: no bootcamp length on the pages read | wrong (unchanged) |
| S2 figma.com | the monthly price of the cheapest paid plan | "$3/mo" (a collab seat) | (b) wrong entity | **yes**: "Professional, Full seat, $16/mo" is on the pricing page read | wrong (unchanged); sharpened "full seat": **$16/mo, correct** |
| S3 cloudflare.com | Do they have a SOC 2 Type II report? | "Meet SOC 2, PCI DSS … requirements" | (d) non-answer | no: no Type II claim on the 11 pages read | **not found** (fixed) |
| S3 vercel.com | Can customers choose where their data is stored? | Functions region ("Hobby customers can select … for Serverless Functions") | (b) wrong entity | no: no data-location text on the pages read | wrong (unchanged); sharpened "data, not Functions": **correct (Vercel Blob regions)** |

Count: (a) 3, (b) 7, (c) 2, (d) 5. Of the 17: 14 are still wrong in the final round (one changed text), 1 became not found (Cloudflare), 2 are not run (ESADE). Where the correct answer was on a read page (IE's badge, IESE's programme page, Figma's price), the failure is `--precise`'s pick, not the search.

### Step 2: class (a), and the fix that was kept

What the extractor already did: it skips `<nav>`, `<footer>`, `<aside>`, `<form>`, the masthead `<header>`, elements with roles navigation, complementary, contentinfo, search, menu and menubar, `aria-hidden` and `hidden` (src/extract/html.rs). The leaks are outside those landmarks: a link list inside a content paragraph (IESE's program types: "Focused Programs | Online | …" are five links with separators), a card label (IESE's "Online", IE's "TWO-WEEK MODULE OPTIONS"), and a Wix `role="region"` copy of the footer menu (Nauta, which the search read from its rendered page).

Tried, measured as rounds:
- **r2a** (navigation-like blocks dropped from the page, link share ≥ 0.8, one link included, banner role): lost correct answers (Lexington's day pass, Ironhack's remote format, bitbucket's path): the dropped blocks were the page's warmth too, so the search went elsewhere. Net worse.
- **r2a2** (dropped, two or more links): same kind of loss (Lexington, bitbucket). Net worse.
- **r2a5** (flagged, not dropped; `--precise` takes no answer from a flagged block, the warmth is unchanged; no step 5): 62 / 16 / 29, the only verdict changes being GitLab (not found → a wrong sentence) and the three ESADE cells (not run). IESE's list block was no longer the pick, but the card label "Online" took its place (still wrong).
- **banner** (ARIA landmark added to the chrome roles): r2a3 and r2a4 differ only in Ironhack's text: no verdict moves, so it is dropped.

Kept: **r2a4** = the flag, plus step 5. The rule: a paragraph or list item whose text is at least 80% link text and holds two or more links is navigation (`Block::nav`); `--precise`'s candidate answers exclude it (`answerable`, src/follow.rs). Its links are still candidates; its text still counts for the warmth. Known limits: a list of single-link items is not flagged (the rule is per element); a label in a card is no link list, so IE's and IESE's card labels are not touched. Those need a rule over the whole list, or Jev's pick to refuse labels, and neither is a standard in the HTML sense; left open.

### Step 3: the linked hosts' sitemaps, with the fix (r2a4 against r2a5)

Reinstated: a linked same-registrable-domain host's `/sitemap.xml` and the sitemaps its `robots.txt` names are read, as the start site's own are. Measured with the fix: +2 correct (Stripe's rate-limit table, "100 requests per second", from docs.stripe.com's own sitemap; EIC's call deadline, "17 December 2026 17:00 Brussels time"), −1 wrong (Cloudflare's non-answer becomes not found). No new wrong answers. The round-1 step-5 round had three new wrong answers (EIC's 2024 deadline, VivaTech's 2026 edition, GitLab's sentence): with the fix, EIC is right, VivaTech is not reached, and GitLab's pick is still a wrong sentence from the page reached (the figure is not on it). So step 5 is kept.

### Step 4 (round 1's (b) question): sharpened questions

Re-run only, not a change to jurl. Final binary, English question, one front door each (sheets B1–B7 in sheets.json):

| cell | sharpened question | answer | verdict | does the specificity fix it? |
|---|---|---|---|---|
| Kit Digital | What is the maximum Kit Digital aid per company? | not found (no page read has it) | not found | wrong → not found: removes the 40 million, no right answer found |
| EIC | What is the maximum funding per project? | not found (closest "€10 million", p 0.37) | not found | wrong → not found |
| South Summit | How much is the general admission ticket (not startup or investor passes)? | "299€" (still the startup pass) | wrong | no |
| Slush | How much is the general admission ticket (not startup or investor passes)? | not found (closest "1195€", p 0.27) | not found | wrong → not found |
| ESADE | Can you take the full-time MBA online? | not run (ESADE unreachable) | not run | not measured |
| Figma | What is the monthly price per full seat of the cheapest paid plan (not a collab or dev seat)? | "$16/mo" (Professional, full seat) | **correct** | **yes**: wrong → correct |
| Vercel | Can customers choose the region where their data is stored, not only where Serverless Functions run? | "You can create Blob stores in any of the 19 regions" | **correct** (Vercel Blob storage) | **yes**: wrong → correct |

So specificity fixed 2 of 6 scored cells (Figma, Vercel), turned 3 wrong answers into not found (Kit Digital, EIC, Slush: the right entity was not on the pages read), and left South Summit wrong (its only ticket on the pages read is the startup pass). For the planner prompt: the question has to name the entity (a full seat, the data rather than the compute, the flagship programme) and the unit; a sharper question does not make a missing page appear.

### Accept-Language: the risk test (not decided by the owner)

Kept as its own commit (019ed1d) so it can be dropped. Stripe, a site that honours the header, asked in Spanish, final binary:

| setting | answer | language |
|---|---|---|
| default `en` | "100 requests per second" (the English table) | English, for a Spanish question |
| `JURL_ACCEPT_LANGUAGE=es` | "100 peticiones por segundo" (the Spanish table, "Límite de frecuencia de la API global") | Spanish, matches the question |

The risk is real: with the `en` default a site that serves by the header answers a Spanish question in English. Ironhack (also bilingual) asked in Spanish returns not found under both settings ("¿Cuánto dura el bootcamp?": the length is not on the pages read), so it shows no difference. The coordinator's proposal (no header by default, the caller sends the question's language) is the safer one for a multilingual site; the cost is that a site that answers by GeoIP without the header gets its local language (stripe's docs gave Spanish without the header from a Spanish IP).

### Reach: Shopify and GitLab

- **Shopify**: the search reaches `shopify.dev/docs/api/usage/limits` (the redirect of the rate-limits URL), a hub whose table names the REST Admin API ("Request-based bucket and headers") but gives no figure. The figure is on the REST Admin API page, one hop past the hub. With the default budget (5 pages) it is not reached. With `--follow 10` (probe, billed ~$0.006) it is not reached either: the search takes the Storefront pages (warmth 0.65) and answers "None for buyer traffic" (wrong). What would reach it, as a general rule: a hub's links ranked by how their text and path answer the question, which is already what the Jev lead score does; the REST link scores too low for it. Not implemented: the only rule that would do it is a word match, which the owner forbids.
- **GitLab**: the search reaches `docs.gitlab.com/user/gitlab_com/rate_limits/` (through a blog link, a cross-domain candidate, in every round since r2a5). That page gives the rules (an hourly limit per plan, a per-minute burst limit, "Authenticated requests receive your plan's full allowance") and no figure; the two wrong picks in the rounds are "full allowance" (r2a5) and "A sustained limit, measured each hour" (r2a4). The figure, if GitLab states one, is on another page this search did not reach; the front door's static HTML links no docs host.

### Changed-reach not-found cells, re-verified (round 2a4 against step 4)

Five not-found cells read different pages in the final round. Each was re-checked against the page text it was given:
- Talent Garden, hot desk price and day pass: seven and ten pages read; no desk price or day pass in them (`not_on_site` in the pages read).
- Lexington, hot desk price: six pages read; the prices on them are for private offices, not hot desks (`not_on_site`).
- IE, flagship MBA price: nine pages read; no fee in them (`not_on_site`).
- Shopify: `not_reached`, as above.

The other not-found classes are carried from the round they were first lost in (RESULTS.md marks them), unverified beyond jurl's own traces.

### Cost, CI and size

- Billed in round 2: $0.124 (cumulative $0.666 of the $1.04 cap for rounds 1 and 2). The round-2 cap of $0.50 was not reached.
- Binary: 12,065,040 bytes for the final build (step 4: 12,048,496; the navigation flag and the linked sitemaps add 16,544 bytes; the banner variant has the same size).
- Tests: 261 passed, 1 ignored; fmt and clippy `-D warnings` clean.

