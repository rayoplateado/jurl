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
