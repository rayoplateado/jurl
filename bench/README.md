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
| Now | 49/54 | 24/24 | 7/9 | 9/12 | 9/9 | 3 | 8.4M ($0.35) |

What changed: the start page is weighed like any other candidate page (a FAQ line on the home page no longer beats `/pricing`); Jev compares the best leads side by side; a docs index counts as warm when it links to the answer; on pages with more links than get scored, those sharing words with the question get in first (Cloudflare's Workers index lists hundreds); table cells reach `--precise` with their row and column ("30 s (CPU time · Paid)"). Warmth is relative too: far from the answer every page is cold, so a page counts as warmer when its best link looks better than the link that led to it; without that, a long game never left the start page's links (Paris → Aspirin went from 2 in 3 to none; with relative warmth it reached the formula in 5 of 10 runs, the long game is still hit or miss). A wiki's own pages (`Wikipedia:`, `Special:`, `Help:`) aren't followed. A table's first column is a candidate in `--precise` too, labelled by the cell next to it: in Wikipedia's glossary of formulae the formula is the first cell (`HC9H7O4`, the page's own spelling), and without it jurl picked the name next to it. Kubernetes is the one miss that stays: the answer page is found, but `--precise` picks another field on it ("NotRequired").
