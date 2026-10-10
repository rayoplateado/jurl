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
| R4-2-1 | South Summit | correct | 299€ | none | Event | 1 | - | 82 / 5 |
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

