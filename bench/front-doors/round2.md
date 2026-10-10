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

Five not-found cells read different pages in round 2a4, and the final round has the same not-found cells: Talent Garden (hot desk price and day pass: five pages read in each of r2a4 and the final round, per the result files; no desk price or day pass in them, `not_on_site`), Lexington (hot desk price: six pages read; the prices are for private offices, `not_on_site`), IE (flagship MBA price: nine pages read; no fee in them, `not_on_site`), and Shopify (`not_reached`, as above). The other not-found classes are carried from the round they were first lost in (RESULTS.md marks them), unverified beyond jurl's own traces.

### Cost, CI and size

- Billed: round 1 $0.54 (per cell); round 2 about $0.13 (the proxy's count, which includes calls whose results were later discarded and probes). Cumulative $0.675 of the $1.04 cap. The round-2 cap of $0.50 was not reached.
- Binary: 12,065,040 bytes for the final build (step 4: 12,048,496; step 5 adds 16,544). The flag build (r2a4) is the same size with different bytes.
- Gates: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`: 259 passed, 1 ignored (after the Accept-Language change).
