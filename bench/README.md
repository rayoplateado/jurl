# Reading docs for one answer

An agent has a URL and a question. What it gets back from each way of reading the page, on 10 documentation pages.
Measured on 2026-10-07.

| Reader | Exact answer | Code lines not on the page | Cost per 10,000 pages | Median tokens | Median time |
| --- | --- | --- | --- | --- | --- |
| jurl -q | 10/10 (30/30 over 3 runs) | 0 of 60 | $4 | 375 | 0.8 s |
| Claude Code WebFetch | 10/10 | 33 of 63 | ~$150 | 148 | — |
| Exa contents + highlights | 8/10 | 0 of 59 | $10 | 351 | 0.3 s |
| Exa search + highlights | 8/10 | — | $70 | 1,529 | 1.9 s |
| Tavily extract (query) | 5/10 | 8 of 36 | $16 | 524 | 0.5 s |
| Tavily search | 3/10 | — | $80 | 942 | 3.4 s |
| Whole page as markdown | 10/10 | 0 | — | 15,352 | 1.8 s |

- **Exact answer:** the answer's text (`truth` in [tasks.json](tasks.json)) is in what the reader returned, word for word. This is strict on purpose: an explanation in other words counts as a miss. Exa's React answer, for example, explains cleanup correctly in prose but has no `return () =>`.
- **Code lines not on the page:** every line inside a code block of the answer, looked up (ignoring whitespace, and a heading's `#`) in the page as served. Claude Code's WebFetch rewrites what it reads, adding comments and examples of its own, e.g. `const subscription = createSubscription(data);` for React, which isn't on react.dev. Search rows are left out: their results are other pages.
- **Median tokens:** what the agent reads, counted with `cl100k_base`.

## What it costs

- **jurl:** $0.0004 per page (median), $0.0086 at most, for Node's 450 KB `fs` page. Jev is $0.042 per million input tokens.
- **Claude Code's WebFetch:** included in Claude Code; each call has a small model read the page, up to 100,000 characters. At Claude Haiku 4.5's $1 per million input tokens that's ~$0.015 for the median page here.
- **Exa:** $0.001 per contents call, $0.007 per search, as reported by the API.
- **Tavily:** $0.008 per credit; extract is 1 credit per 5 pages, search 1 credit.

## What it shows, and what it doesn't

- On these pages, jurl is the only reader that had the exact answer every time without changing a word of it.
- Exa is faster (it serves pages from its index) and costs about the same per page, but missed both answers that were code.
- Search returns other pages of the same site, uses more tokens and misses more.
- These are 10 tasks we picked, all developer documentation, each with one known answer. They say nothing about prose, news or shopping pages.

## The tasks

| Page | Question | Answer must contain |
| --- | --- | --- |
| github.com/BurntSushi/ripgrep | how do I install it on macOS? | `brew install ripgrep` |
| docs.python.org/3/tutorial/inputoutput.html | how do I read a file line by line? | `for line in f` |
| github.com/astral-sh/uv | how do I install uv on macOS or Linux? | `curl -LsSf https://astral.sh/uv/install.sh \| sh` |
| en.wikipedia.org/wiki/Jevons_paradox | who first described the paradox and in which book? | `The Coal Question` |
| developer.mozilla.org/…/Array/at | what happens when the index is negative? | `count back from the last item` |
| developers.cloudflare.com/workers/platform/pricing/ | how many requests per month are included in the Workers Paid plan? | `10 million` |
| docs.github.com/…/rate-limits-for-the-rest-api | what is the primary rate limit for authenticated users? | `5,000 requests per hour` |
| nodejs.org/api/fs.html | how do I read a whole file as a string with the promises API? | `fsPromises.readFile` |
| postgresql.org/docs/current/datatype-numeric.html | what is the range of bigint? | `9223372036854775807` |
| react.dev/reference/react/useEffect | how do I clean up a subscription when the component unmounts? | `return () =>` |

## Run it yourself

Everything each reader returned is in [results/](results). To measure again:

```sh
uv run --with tiktoken --with requests bench/run.py jurl          # 3 runs; TYPESAFE_API_KEY or `jurl init`
uv run --with tiktoken --with requests bench/run.py whole-page
EXA_API_KEY=… uv run --with tiktoken --with requests bench/run.py exa
TAVILY_API_KEY=… uv run --with tiktoken --with requests bench/run.py tavily
uv run --with tiktoken --with requests bench/table.py              # the table above
```

WebFetch only runs inside Claude Code: [results/webfetch.json](results/webfetch.json) holds its answers, asked with each task's URL and question.

## `--follow`: finding the page on a site

[follow.json](follow.json) holds 18 searches that start from a site's front door: the cheapest paid plan from a bare domain (`linear.app`), an answer in docs from the docs' root, short and long Wikipedia games, and 3 questions the site doesn't answer (right = left empty). [follow.py](follow.py) runs each 3 times with `--precise --follow` and saves `results/follow-<label>.json`.

| | Right | Pricing | Docs | Wikipedia | Not on the site | Median pages (pricing) | Tokens (all 54 runs) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| First version | 44/54 | 21/24 | 3/9 | 11/12 | 9/9 | 4 | 7.5M ($0.32) |
| With the first column | 49/54 | 24/24 | 7/9 | 9/12 | 9/9 | 3 | 8.4M ($0.35) |

What changed: the start page is weighed like any other candidate page (a FAQ line on the home page no longer beats `/pricing`); Jev compares the best leads side by side; a docs index counts as warm when it links to the answer; on pages with more links than get scored, those sharing words with the question get in first (Cloudflare's Workers index lists hundreds); table cells reach `--precise` with their row and column ("30 s (CPU time · Paid)"). Warmth is relative too: far from the answer every page is cold, so a page counts as warmer when its best link looks better than the link that led to it; without that, a long game never left the start page's links (Paris → Aspirin went from 2 in 3 to none; with relative warmth it reached the formula in 5 of 10 runs, the long game is still hit or miss). A wiki's own pages (`Wikipedia:`, `Special:`, `Help:`) aren't followed (by a list of their prefixes then; by generic signals now, see below). A table's first column is a candidate in `--precise` too, labelled by the cell next to it: in Wikipedia's glossary of formulae the formula is the first cell (`HC9H7O4`, the page's own spelling), and without it jurl picked the name next to it. Kubernetes is the one miss that stays: the answer page is found, but `--precise` picks another field on it ("NotRequired").

#### Long games

Five games 3 to 5 clicks apart (`--follow 15`, group `long`) and six more that played no part in tuning (group `heldout`), each run 3 times. `GROUP=long python3 follow.py <label>` runs one group.

| | long | heldout |
|---|---|---|
| Links scored only by "does it lead to the answer?" | 10/15 | 15/18 |
| Plus "is it in the answer's field of knowledge?" (`--follow 10` and up) | 15/15 | 16/18 |

Far from the answer every link scores ~0.02–0.08 on "leads to the answer", so the pick is noise: from Tennis, jurl opened Berdych, Deutsche Bank, Kiev, then Freddie Mercury. Asked whether each link is in the answer's field, Jev gives vulcanized rubber and polyester 0.9 (chemistry), and the search reaches Mercury (element) in 10 pages. Asking about the question's subject instead (not the answer's field) made Jev judge links by the page they're on: tennis players, jazz musicians. The miss that stays is Bicycle → the Titanic: "what year" pulls toward calendar pages. A wiki's own pages (`Wikipedia:…/…`, `Main_Page`) are never followed (see the next section for how).

#### `--follow` scores links the way `--links -q` does

`--follow` is now `--precise` and `--links -q` in a loop: both use one function to score links (`src/links.rs`). On the follow path, what Jev is asked didn't change, except that a link to another host (a subdomain, or `www.` when the page has none) now says its host. Measured against 0.1.10 the same day, all 84 runs each ([0.1.10](results/follow-0.1.10.json), [after](results/follow-links-q.json)):

| | All | Pricing | Docs | Wikipedia | long | heldout | Not on the site | Tokens (all 84 runs) |
|---|---|---|---|---|---|---|---|---|
| 0.1.10 | 79/84 | 24/24 | 7/9 | 9/9 | 15/15 | 15/18 | 9/9 | 32.4M ($1.36) |
| `--links -q` scoring | 79/84 | 24/24 | 9/9 | 9/9 | 15/15 | 13/18 | 9/9 | 34.3M ($1.44) |

The two groups that moved were run again with both binaries (0.1.10, then after): docs 8/9 and 8/9, heldout 15/18 and 16/18. On Wikipedia (heldout) Jev is asked exactly what it was asked before, so the gap there can only be noise. Coffee → The Magic Flute missed twice in the full run and never in the rerun; Bicycle → the Titanic still misses (1 right in 6 runs after, 0 in 6 before).

#### A site's own pages, without a list of them

Up to here `--follow` never opened a wiki's own pages by a list of MediaWiki prefixes (`Wikipedia:`, `Special:`, `Main_Page`…). Every site has such pages, so the list is gone, replaced by three things any page shows:

- A link inside a footnote mark (`<sup>`: "[clarification needed]", "[when?]") or with only an image (a photo's own `File:` page) isn't a candidate for `--links -q` or `--follow`. That's where `Wikipedia:Please_clarify` and `Wikipedia:Manual_of_Style/Dates_and_numbers` came from.
- A site's menus and footers (links outside the page's text) are scored on the first page they're on, not again on every page. On a page far from the question, "Main page", "Contents" and "Search" outscored everything in its text: Bicycle → American Automobile Association → `Main_Page` → `Wikipedia:Contents`.
- On a long search (`--follow 10` and up) menu links count for 30% of their score. From Pizza, the start page's own menu ("Search" 0.19, "Contents" 0.18, "Main page" 0.14) beat every link in the article; with the menus scored once but not discounted, Pizza → Don Quixote went to `Wikipedia:Contents` and missed once in three.

Telling Jev in the question that "a page about the site itself (help, editing, policies, search, accounts…) does neither" didn't work: on the American Automobile Association page `Special:Search` went from 0.18 to 0.31 and `Main_Page` from 0.16 to 0.29, every score rose about twice, and on linear.app "signup" went from 0.15 to 0.53. The question is unchanged.

Measured the same day, 84 runs each ([before](results/follow-generic-base.json), [first version](results/follow-generic-first.json): `<sup>`/image links and menus scored once, no discount; [final](results/follow-generic-final.json): with the discount):

| | All | Pricing | Docs | Wikipedia | long | heldout | Not on the site | Tokens (all 84 runs) |
|---|---|---|---|---|---|---|---|---|
| Prefix list (before) | 80/84 | 24/24 | 8/9 | 9/9 | 15/15 | 15/18 | 9/9 | 34.3M ($1.44) |
| Generic, no discount | 76/84 | 24/24 | 7/9 | 9/9 | 14/15 | 13/18 | 9/9 | 33.8M ($1.42) |
| Generic, final | 80/84 | 24/24 | 6/9 | 9/9 | 15/15 | 17/18 | 9/9 | 33.4M ($1.40) |

Docs moved, so it was run again with both binaries, back to back ([before](results/follow-generic-base-docs2.json), [final](results/follow-generic-final-docs2.json); 5 runs each: [before](results/follow-generic-base-docs3.json), [final](results/follow-generic-final-docs3.json)): 9/9 and 8/9, then 13/15 and 13/15. Every docs miss is Kubernetes, the known one: when the search lands on the Pod API reference instead of the pod lifecycle page, `--precise` picks "NotRequired". Over all its runs that day it was right 8 of 11 times before and 5 of 11 after, but tied 3 of 5 to 3 of 5 when run back to back; GitHub and Cloudflare were right in every run with both. Bicycle → the Titanic: 0/3 before, 2/3 now, both times through `RMS_Titanic`. In `-t` runs of the final version from Paris (4), Bicycle (2) and Pizza (2), no `Wikipedia:`, `Main_Page`, `Special:` or `File:` page was opened, and no trail in the 84 runs goes through one; with only the `<sup>`/image rule, Bicycle had opened `Main_Page` and five `Wikipedia:Contents` pages.

#### Fixes from real use cases

Short list items kept with their list, markdown served as `text/plain` read as markdown, a page's own URL never an image, `-t` and a non-zero exit on every miss. None of them changes what `--follow --precise` asks Jev, except on a page served as `text/plain`, now read as markdown (`llms.txt` already was). Measured against the binary before them the same day, 84 runs each ([before](results/follow-usecase-base.json), [after](results/follow-usecase-fixes.json)):

| | All | Pricing | Docs | Wikipedia | long | heldout | Not on the site | Tokens (all 84 runs) |
|---|---|---|---|---|---|---|---|---|
| Before | 81/84 | 24/24 | 8/9 | 9/9 | 15/15 | 16/18 | 9/9 | 33.8M ($1.42) |
| After | 81/84 | 24/24 | 9/9 | 9/9 | 15/15 | 15/18 | 9/9 | 33.5M ($1.41) |

The new heldout miss was Chess → the speed of sound, answered "340 m/s" from `Air` once; run again 3 times with each binary, back to back, it was right 3 of 3 with both ([before](results/follow-usecase-base-rerun.json), [after](results/follow-usecase-fixes-rerun.json), docs in the same rerun: 7/9 and 8/9, every miss Kubernetes).

On the 10 docs pages ([before](results/jurl-usecase-base.json), [after](results/jurl-usecase-fixes.json)) both scored 9/10 (27/30): nodejs.org answered HTTP 404 to every request that day, curl included. react.dev answers `Accept: text/markdown` with markdown as `text/plain`, which jurl used to read as HTML, every code block collapsed into one line. Read as markdown, the code jurl returns there keeps its lines (79 code lines checked against the page, 0 not on it, against 30 before), and Jev reads the whole page instead of a few long blocks cut short: 31,380 tokens instead of 5,415 for that task.

#### "The company" is the site's owner

`--follow --precise -q "Where is the company headquartered?" linear.app` answered "San Francisco", then "New York": both from customer stories (`/customers/openai`, `/customers/ramp`), OpenAI's and Ramp's headquarters, not Linear's. On stripe.com it answered from `/es/customers/linear` and `/es/customers/gamma`. On a customer story, "the company" reads as the customer.

When `--follow` starts at a site's front door (`linear.app`, not `en.wikipedia.org/wiki/Paris` or a repo), the question Jev is asked about each block, the `--precise` spans and the side-by-side pick of leads now says so: "(Asked about linear.app: unless the question names someone, "the company", "they", "we" or "it" is the organisation behind linear.app, not a customer or partner it writes about.)". Each link's question only points at that note (`asked_about` in the links' state, once per request): repeated in all 250 link questions it cost 40% more tokens instead of about 25%. What jurl prints is unchanged: still the page's own words. Started from any other page, Jev is asked exactly what it was before.

On the customer stories alone, with `--precise`: `/customers/openai` "Founded San Francisco" (p 0.82–0.84 on main) and `/customers/ramp` "Founded New York" (0.78) became no answer at all, 2 runs of 2 each (`stripe.com/customers/amazon` had none either way). The note had to be in the question: in the state only (once per request) it changed nothing ("Founded San Francisco" 0.85); in the `--precise` question only, `/customers/openai` still passed (0.43, 0.57). Shorter wording without "not a customer or partner" didn't work either (0.80).

What it doesn't fix: Figma's headquarters is still "Berlin", from the German imprint (`/legal/impressum/`, the address of Figma GmbH, p≈0.9 with or without the note). Linear's is now "Singapore" at times: a job listing's location on `/readme`, which main answers too (p 0.97–0.99 on main, 0.79–0.89 after; once the search ended without an answer, closest "North America and Europe" on `/about`).

Measured the same day: the new binary on all 84 runs ([after](results/follow-site-subject.json)) against main's from earlier that day ([main](results/follow-usecase-fixes.json), the same commit), and the groups that start at a front door (pricing, docs, not on the site) run again with main back to back ([main](results/follow-site-subject-base-pricing.json), [docs](results/follow-site-subject-base-docs.json), [trap](results/follow-site-subject-base-trap.json)):

| | All | Pricing | Docs | Wikipedia | long | heldout | Not on the site | Tokens (all 84 runs) |
|---|---|---|---|---|---|---|---|---|
| main | 81/84 | 24/24 | 9/9 | 9/9 | 15/15 | 15/18 | 9/9 | 33.5M ($1.41) |
| main, front-door groups again | | 24/24 | 8/9 | | | | 9/9 | |
| Site's owner in the question | 77/84 | 24/24 | 6/9 | 9/9 | 15/15 | 14/18 | 9/9 | 33.9M ($1.42) |

Every run that moved starts from a page, not a front door, so Jev was asked byte for byte what main asks (the 10 single-page tasks below used the same tokens to the token with both binaries). Run again with both binaries back to back ([main](results/follow-site-subject-rerun-base.json), [after](results/follow-site-subject-rerun.json)): docs 7/9 and 7/9 (Kubernetes, the known miss: 0/3 in the full run, 1/3 and 1/3 here), Coffee → The Magic Flute 3/3 and 2/3, Bicycle → the Titanic 1/3 and 1/3.

Tokens: searches from a front door read about 25% more (pricing 1.73M → 2.12M, not on the site 0.78M → 0.95M, docs.github.com and Cloudflare 0.64M → 0.90M; the block questions carry the note), about $0.001 more a search. The first version, with the note in every link's question too ([pricing](results/follow-site-subject-inline-pricing.json), [docs](results/follow-site-subject-inline-docs.json), [trap](results/follow-site-subject-inline-trap.json)): 24/24, 7/9, 9/9, at 2.44M, 1.52M and 1.09M.

The 10 docs pages ([main](results/jurl-site-subject-base.json), [after](results/jurl-site-subject-new.json)): 27/30 both, 312,438 Jev tokens both (nodejs.org answered 404 again). A single page is never searched "about" its site: on `github.com/BurntSushi/ripgrep`, "how do I install it?" is about ripgrep, not GitHub.

Headquarters and founding year from more front doors, once each with main and after (the use case's own six domains are in its notes):

| | main | after |
|---|---|---|
| stripe.com, headquarters | "San Francisco" from `/es/customers/gamma` (Gamma's); before that, "Área de la Bahía de San Francisco" from `/es/customers/linear` | "Estados Unidos" from `/es/global`; before that, no answer |
| stripe.com, founded | no answer (closest "100 años", a Hertz story) | no answer |
| notion.com, headquarters | "San Francisco" (`/about`) | "San Francisco" (`/about`) |
| notion.com, founded | no answer | no answer |
| vercel.com, headquarters | "San Francisco" (a press release's dateline) | "San Francisco" (the same) |
| vercel.com, founded | no answer | no answer |

#### Real-world searches

[follow-real.json](follow-real.json) holds 30 searches people type at a company's or a project's site, each from its front door or docs root: company facts, support, product limits, docs, Spanish sites, and five questions the site doesn't answer (expect null). Every answer was checked against its page with curl; [follow-real.notes.md](follow-real.notes.md) has each one's URL, snippet and clicks. The set is held out: nothing in `--follow` was tuned on it, so use it to compare versions, not to tune against. Run it from the repo root with `CASES=bench/follow-real.json python3 bench/follow.py <label>`; `GROUP=es` still runs one group.

## Another decision model in Jev's place

[models/](models) records what jurl asks Jev, replays it to another model with the same contract (`JURL_JEV_URL`) and counts how often they decide alike, against Jev's agreement with itself.
