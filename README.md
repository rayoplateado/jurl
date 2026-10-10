<p align="center">
  <img src="docs/jurl.png" alt="Ink drawing of a man in a suit and leopard-print tie, pinching his fingers as if picking something out" width="380">
</p>

<h1 align="center">jurl</h1>

<p align="center"><b>curl that reads the page for you.</b><br>
Tell it what you want. It picks it out of the page.</p>

---

````console
$ jurl -q "how do I install it on macOS?" github.com/BurntSushi/ripgrep
…
### Installation

…

If you're a macOS Homebrew or a Linuxbrew user, then you can install ripgrep from homebrew-core:

```
$ brew install ripgrep
```

If you're a MacPorts user, then you can install ripgrep from the official ports:

```
$ sudo port install ripgrep
```
````

```console
$ jurl --find "a cathedral" en.wikipedia.org/wiki/Cologne
https://thumb.wikimedia.org/wikipedia/commons/thumb/2/27/Kdom.jpg/250px-Kdom.jpg
```

```console
$ jurl --links -n 3 news.ycombinator.com
https://github.com/PowderworksCode/headstart
https://www.da.vidbuchanan.co.uk/blog/hacking-time.html
https://gamehistory.org/5k-magazines/
```

## Why jurl

- **It picks. It doesn't write.** jurl doesn't use a chatbot. It uses *decision models*: [Jev](https://docs.typesafe.ai/introduction) reads the text and [Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/) looks at the images. Neither writes a word. They only say how likely each paragraph, link or image is to be what you want, and jurl prints the winners **exactly as they appear on the page**. You never get a summary that drifts or a URL that doesn't exist.
- **It's fast.** Most pages take under a second, end to end.
- **It's cheap.** About a thousand pages per dollar of API usage.

## Install

```sh
brew install rayoplateado/tap/jurl                                                   # macOS, Linux
curl -LsSf https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.sh | sh   # no Homebrew
```

<sub>Windows: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.ps1 | iex"` · From source (needs Rust 1.98, cmake and libclang: see [CONTRIBUTING](CONTRIBUTING.md#develop)): `cargo install --git https://github.com/rayoplateado/jurl`</sub>

Then run `jurl`. With nothing set up, it asks how you want to read pages: with your own [TypeSafe API key](https://console.typesafe.ai), which it checks and saves, or with a [jurl cloud](#jurl-cloud) account. Reading a page first asks the same question. That's the whole setup. Pages that need JavaScript just work too: jurl fetches a headless browser the first time one shows up.

To use `--vision` and `--find`, run `jurl init` and add a Cloudflare Workers AI token. To get a newer jurl, run `jurl update`: it updates the same way you installed it (Homebrew, the installer or cargo).

## What you can do

### Get the gist

```console
$ jurl -n 3 blog.cloudflare.com/markdown-for-agents/
# Introducing Markdown for Agents

<https://blog.cloudflare.com/markdown-for-agents/> · article (1.00)

The way content and businesses are discovered online is changing rapidly. In the past, traffic originated from traditional search engines, and SEO determined who got found first. …

## Convert HTML to markdown, automatically

Cloudflare's network now supports real-time content conversion at the source, …

Here’s how it works. To fetch the markdown version of any page from a zone with Markdown for Agents enabled, the client needs to add the **Accept** negotiation header …
```

You get the title, what kind of page it is, and the blocks that carry it, in reading order. Navigation, cookie banners, sign-up prompts, author bios and footers are left out.

### Ask one question

```console
$ jurl -q "What is the Jevons paradox?" -n 2 en.wikipedia.org/wiki/William_Stanley_Jevons
…
Jevons received public recognition for his work on The Coal Question (1865), in which he called attention to the gradual exhaustion of Britain's coal supplies and also put forth the view that increases in energy production efficiency leads to more, not less, consumption. …

## Practical economics

In The Coal Question, Jevons covered a breadth of concepts on energy depletion …
```

Out of a page with about 240 blocks, you get the two paragraphs that answer it.

### Just the answer

```console
$ jurl --precise -q "what is the primary rate limit for authenticated users?" docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api
5,000 requests per hour

All of these requests count towards your personal rate limit of 5,000 requests per hour. …

<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#:~:text=rate%20limit%20of-,5%2C000%20requests%20per%20hour>
```

The first line is the answer, still in the page's own words: jurl splits the best blocks into candidates (values, names, clauses, sentences, lines of code) and Jev picks the one that is exactly the answer, then, among the shorter ones inside it, the one with nothing extra ("2009", not "2009; 17 years ago"). Then the block it came from, and a link that opens the page with the answer highlighted. If no part of the page is the answer, jurl says so and exits with an error instead of guessing, so a script gets an empty cell, not a wrong one. It never computes: if the page says "$8 a month", it won't tell you the yearly price. On 30 SaaS pricing pages ("What is the monthly price of the cheapest paid plan?") it answered 15, all of them right (Notion's as `Plus*€9.50`, with the plan's name stuck to it on the page), and left the rest empty.

### Not on this page? Follow the site

```console
$ jurl --precise --follow -q "What is the monthly price of the cheapest paid plan?" linear.app
$10 per user/month
…
jurl: found after reading 4 pages: linear.app/ → linear.app/pricing
```

With `--follow`, a page that doesn't answer isn't the end. It is `--precise` and `--links -q` in a loop: each page is asked for the answer, and its links are scored by how likely they lead to it. jurl reads the site's own map (`llms.txt`, `sitemap.xml`, and the sitemaps its `robots.txt` names: copies of a page in other languages are skipped; the newest sitemaps of an index first) and the `llms.txt` of the hosts the page links on its own registrable domain (`docs.stripe.com` from `stripe.com`), while it reads the first page, then compares the best ten links side by side and opens the two it likes most, keeping on from whichever page looks closest, the way people play the Wikipedia game. On a long search (`--follow 10` and up) Jev also asks of each link whether its page is in the answer's field of knowledge: far from the answer, "leads to the answer" is noise (from Tennis: Birmingham, Philadelphia), while the field points the way (vulcanized rubber, polyester: chemistry, then the elements, then mercury). It plays hot and cold: a link counts for as much as its page is close to the question (a docs index with a link straight to the answer counts as close), so a wrong turn is dropped and the search goes back to the page that was getting warmer. A page that answers is ranked by how sure Jev is of the answer and of the page, and jurl stops only when no page left could beat it by much: the pricing page beats both a home-page FAQ line and an old blog post quoting last year's price. Started from a site's front door (`linear.app`), the question is asked about that site: "the company", "they", "we" or "it" is whoever runs it, not a customer in one of its stories. It follows the links a page gives, to another site too when the page names one (a link or a URL in its text, never one it makes up), scored like any other link; each host's `robots.txt` is checked, and it stops after 5 pages (`--follow 10` for more; longer searches open three pages per step). The path comes last, on stderr, or as `path` in `--json`; `-t` shows how warm each page was.

On [28 searches](bench/follow.json), run once each on 2026-10-08: 27 right, against 25 for the build before (7e68b67). The eight pricing pages are found from the bare domain, 8/8 (about 3 pages, median ~2.6 s, ~$0.0035 each). Answers in docs from the docs' root: 3/3. Short Wikipedia games (Medicine → Aspirin: `C9H8O4`): 3/3. Questions the site doesn't answer, left empty: 3/3. Long games with `--follow 15`, 3 to 5 clicks apart: 10/11, the one miss being Bicycle → the Titanic. Five were tuned on (Paris → Aspirin, Tennis → the boiling point of mercury, Jazz → the moons of Mars, among them) and six never were (Volcano → Mona Lisa, Chess → the speed of sound, among them). Median ~5 s; ~$0.031 each on average (before: ~$0.035). Three runs of each, on earlier builds, are in [bench/README.md](bench/README.md).

### Just the code

````console
$ jurl --code -n 2 github.com/BurntSushi/ripgrep
### Installation

```
$ brew install ripgrep
```

```
$ cargo install ripgrep
```
````

### Links worth following

```console
$ jurl --links -n 3 github.com/BurntSushi/ripgrep
https://beyondgrep.com/feature-comparison/
https://docs.rs/regex/1/regex/
https://dandavison.github.io/delta/grep.html
```

The ranking puts content first, ahead of `login`, `share` or `privacy policy`.

```console
$ jurl --links -n 2 -q "What is the monthly price of the cheapest paid plan?" linear.app
https://linear.app/pricing
https://linear.app/plan
```

With `-q`, each link is scored by how likely following it leads to the answer. Menus and footers count too: a nav bar's `Pricing` is often the way to a price. These are the links `--follow` opens.

### The real images

`--image` picks the content images by their file name, alt text and caption, with no logos, icons or tracking pixels. `--vision` also has Clef look at the pixels, which matters when the alt text says nothing:

| Image on a Cloudflare blog post | Alt text | `--image` | `--vision` |
| --- | --- | --- | --- |
| Stacked area chart | `BLOG-3162 4` | 0.64 | **0.82** |
| Diagram | `BLOG-3162 3` | 0.63 | **0.82** |
| Author avatar | `Will Allen` | 0.38 | **0.21** |
| Company logo | `Cloudflare` | 0.09 | **0.08** |

### Find a photo by description

```console
$ jurl --find "a cathedral" -n 3 en.wikipedia.org/wiki/Cologne
```

The page has 73 images, and Clef looks at every one of them in parallel. It took 2.3 seconds:

| | Image | Why it matched | p |
| --- | --- | --- | --- |
| 1 | `Kdom.jpg` | Cologne Cathedral | 0.96 |
| 2 | `Köln_um_1890.jpg` | The 1890 skyline, the cathedral towering over it | 0.95 |
| 3 | `Kranhäuser_Cologne_April_2018.jpg` | The Rhine at dusk, the lit cathedral on the right. **No alt text, no caption:** only the pixels could find it | 0.95 |

If nothing matches, jurl tells you so instead of handing you the least-bad photo:

```console
$ jurl --find "a carnival parade" en.wikipedia.org/wiki/Cologne
jurl: no image in https://en.wikipedia.org/wiki/Cologne looks like "a carnival parade" (closest: …, p=0.02)
```

The same goes for every mode: an HTTP error, a rate-limit or bot-check page served in place of the real one, or nothing above `--threshold` ends with a message on stderr and a non-zero exit code, never an empty answer: 1 when the page has nothing that answers, 2 when something failed (see [Exit codes](#exit-codes)). With `--precise --json` (with or without `--follow`) a miss still prints its JSON, `"answer": null` plus what came `closest` (`null` too when no block came close), and exits 1 all the same: a script that wants the closest passage reads stdout before checking the exit code. Without `--precise`, a miss prints nothing on stdout, `--json` or not. `-t` prints its timings and tokens on a miss too, so a miss can be costed.

Images are picked from `<img>` tags (`src`, `srcset` and the usual lazy-loading attributes) and the page's `og:image`. SVG images aren't candidates: they're mostly icons and logos, and Clef only reads raster images, so a post whose diagrams are all SVG (Stripe's engineering blog) has nothing to find.

### JavaScript apps

Some pages arrive empty because their content is built by JavaScript, or with template placeholders in their text (`!{freePlanStorage}`) that the script would have filled in. jurl spots those and renders them in [Lightpanda](https://lightpanda.io), a fast headless browser:

```console
$ jurl -n 2 hn.algolia.com
jurl: no text without JavaScript, rendering with Lightpanda…
# HN Search powered by Algolia

<https://hn.algolia.com/> · listing (1.00)

Stephen Hawking has died(http://www.bbc.com/news/uk-43396008)

6015 points|Cogito|9 years ago|436 comments
```

Use `-r` to force it.

## Recipes

Everything prints plain text or plain URLs, so jurl fits in a pipe:

```sh
# Read the top 3 Hacker News stories, one key paragraph each
jurl -l -n 3 news.ycombinator.com | xargs -n1 jurl -n 1

# Download the photo that matches
jurl -f "a bridge over a river" en.wikipedia.org/wiki/Cologne | xargs curl -sO

# Every chart in a post
jurl -f "a chart or graph" -n 10 blog.cloudflare.com/markdown-for-agents/ | xargs -n1 curl -sO

# Only the confident answers, for scripts
jurl --json -q "installation" github.com/BurntSushi/ripgrep | jq -r '.blocks[] | select(.p > 0.8) | .text'
```

## Use it from an agent (MCP)

`jurl mcp` gives any MCP client (Claude Code, Claude Desktop, Cursor…) jurl as a set of web tools, over stdio. The agent gets the page's own words with their links, and a plain "Not found" when the page doesn't say it.

```sh
claude mcp add jurl -- jurl mcp
```

Claude Desktop (`claude_desktop_config.json`) and Cursor (`~/.cursor/mcp.json`) take the same entry. If the app can't find `jurl`, use the full path that `which jurl` prints:

```json
{
  "mcpServers": {
    "jurl": { "command": "jurl", "args": ["mcp"] }
  }
}
```

| Tool | Like | Returns |
| --- | --- | --- |
| `answer` | `--precise -q` (`follow`: `--follow`) | The exact answer, the block it's in and a link that highlights it |
| `read_page` | `jurl`, `-q` | The blocks that carry the page, or that answer `question` |
| `find_links` | `--links` | The links worth following, or most likely to lead to the answer |
| `find_code` | `--code` | Code blocks, as written |
| `find_image` | `--find`, `--image`, `--vision` | Image URLs: the one that shows `description`, or the content images |

Ask your agent for the cheapest paid plan on linear.app and it calls `answer` with `{"url": "linear.app", "question": "What is the monthly price of the cheapest paid plan?", "follow": true}`. It gets back `$10 per user/month`, the block, the link, and `Found by following https://linear.app/ → https://linear.app/pricing`.

The tools run the same code as the CLI, with the same keys or jurl cloud sign-in (`jurl init`, `jurl login`, or the environment variables). A miss is a normal result that starts with `Not found:`, as exit code 1 is. A failure (a page that can't be read, a bad key, no credits) is a tool error, as exit code 2 is.

## Reference

| Flag | |
| --- | --- |
| `-q, --ask "…"` | Keep what answers the question. Works with every mode |
| `--follow [N]` | With `-q`: when the page doesn't answer, follow the links the pages give, most promising first, reading up to N pages (default 5) |
| `-p, --precise` | With `-q`: just the answer, in the page's words, then its block and a link to it. Exits 1 when no part of the page is the answer |
| `-c, --code` | Code blocks only |
| `-l, --links` | Content links, best first. With `-q`, the links most likely to lead to the answer, menus included |
| `-i, --image` | Content images, judged by file name, alt text and caption |
| `--vision` | Like `--image`, plus Clef looks at the pixels |
| `-f, --find "…"` | The image that best matches the description |
| `-r, --render` | Run the page's JavaScript first (automatic for empty JavaScript apps and unfilled template placeholders) |
| `--no-browser-retry` | Don't ask a page that answers 403 or 503 again with a browser's TLS fingerprint (see below) |
| `-n, --max N` | How many results (12 blocks, 5 with `--ask`, 8 code blocks, 20 links, 1 with `--find`) |
| `-a, --all` | No limit: everything above the threshold |
| `--threshold P` | Minimum probability (default 0.5; 0.4 for the `--precise` answer) |
| `--json` | Machine-readable output, with every probability and the run's `usage`. A `--precise` answer also gives its block's `kind` (`heading`, `para`, `code`, `quote`, `item`, `table`), and the `level` or `lang` when the block has one |
| `-t, --timing` | Where the time went, on stderr |
| `jurl init` | Set or replace your API keys, or sign in to jurl cloud |
| `jurl login` | Sign this computer in to [jurl cloud](#jurl-cloud) |
| `jurl logout` | Sign out of jurl cloud, and revoke the key there |
| `jurl status` | Which account reads go to, and jurl cloud's usage |
| `jurl update` | Install the latest jurl, the same way this one was installed |
| `jurl mcp` | Serve jurl's tools to an AI agent over MCP (see [above](#use-it-from-an-agent-mcp)) |

Keys live in `~/.config/jurl/env`. Environment variables take precedence over that file: `TYPESAFE_API_KEY`, and for images `CLOUDFLARE_ACCOUNT_ID` plus `CLOUDFLARE_AI_TOKEN`. A `.env` in the current directory is read too, for those API keys only. A jurl cloud sign-in is saved there as `JURL_CLOUD_KEY` (and `JURL_CLOUD_URL` when it isn't the default); it never comes from a `.env`.

`--vision` and `--find` give Clef 2.5 s per image; an image slower than that keeps its text-only score. If Clef looks at none of the images, jurl says so on stderr and the result is from text alone. Images over 15 MB aren't read. For batch use, where a slow host matters more than a second of waiting, raise it with `JURL_VISION_TIMEOUT_MS` (e.g. `10000`).

`JURL_JEV_URL` sends Jev's requests to another server with the same contract (`POST {state, model, questions}` → `{answers, usage}`), e.g. a self-hosted model: `JURL_JEV_URL=http://127.0.0.1:8000/v1/systemone`. The TypeSafe key is never sent there: the bearer is `JURL_JEV_KEY` (environment or `~/.config/jurl/env`), or none, and only over https or to localhost: plain http to another host is refused. [bench/models](bench/models) compares such a server's answers with Jev's.

`JURL_ACCEPT_LANGUAGE` sets the `Accept-Language` every page request sends. Unset, jurl sends none; the browser-fingerprint retry below keeps the line its browser profile sends. jurl doesn't detect the language of a question, and a site may answer in the language asked for or by where the request comes from, so the caller sets it: `JURL_ACCEPT_LANGUAGE=es` for a question in Spanish. [bench/front-doors](bench/front-doors/RESULTS.md) has the measurements: a Spanish question about Stripe's rate limits gets an English answer with `en` and a Spanish one with `es`.

Some bot protection refuses a request by its TLS and HTTP/2 fingerprint, whatever the user agent says. So when a page answers 403, or 503 without a `Retry-After`, jurl asks for the same URL once more, with a client that has Chrome's fingerprint and headers, and reads that answer if it is a page. A 503 with a `Retry-After` is maintenance or backoff and is not retried, and a 429 is a rate limit and is never retried: both are respected.

A host that a retry showed needs that client is asked there first for the next 10 minutes (`STICKY_TTL` in `src/fetch.rs`), with no plain request before it. That covers its later pages (`--follow` hops and `--links`) and its `robots.txt` and sitemaps. If the browser client refuses too, that refusal is the answer. Other hosts are asked plain first, as before. `jurl mcp` is one long-lived process, so the same 10 minutes apply there, and a host is tried plain again after them. A site's `robots.txt`, `llms.txt` and sitemaps follow the same rule, the start page's included: one that the plain client refuses is asked once more of the browser client, and that success teaches the host, so the first search of a blocked site gets its sitemap too.

`--no-browser-retry`, or `JURL_NO_BROWSER_RETRY` set to anything, turns all of this off. `-t` says on stderr once per host when it switches (`jurl: example.com: using the browser client`), and `--json` has `"browser_retry": true` in `usage` when a page was read with the browser client, `"plain_refusals"` for the plain requests that got a refusal, and `"browser_requests"` for the requests sent with the browser client. [bench/browser-retry.md](bench/browser-retry.md) has the numbers.

### Exit codes

As with grep, a script can tell "not there" from "something broke":

| Code | Meaning |
| --- | --- |
| 0 | Something was printed: an answer, blocks, links or images |
| 1 | The page (or the site, with `--follow`) was read and has nothing that answers: no exact answer, no image that looks like that, nothing above `--threshold`. With `--precise --json`, the JSON is still printed (`"answer": null` and what came `closest`); without `--precise`, nothing is printed |
| 2 | An error: the page couldn't be read (HTTP error, timeout, a block page, no readable text, or over 8 MB), the API couldn't be asked (a bad key, no credits), or the arguments are wrong |

Before 0.1.11 every failure exited 1, and a `--json` miss exited 0.

## Speed and cost

| Command | Typical time | Typical cost |
| --- | --- | --- |
| `jurl <url>` | 0.6–1 s | $0.0004–0.0013 |
| `jurl -i <url>` | 0.5–0.7 s | $0.00004 |
| `jurl --vision <url>` | 1.3–1.9 s | $0.0002 |
| `jurl --find "…" <url>` (73 images) | 2.2–2.9 s | $0.002 |
| JavaScript apps | +3–6 s | same |

Measured on 2026-10-04. Run any command with `-t` to see your own numbers.

## How it compares

Ten documentation pages, one question each ([bench/](bench) has the tasks, the scripts and every answer):

| | jurl | Claude Code WebFetch | Exa | Tavily |
| --- | --- | --- | --- | --- |
| Exact answer | **10/10** | **10/10** | 8/10 | 5/10 |
| Code lines not on the page | **0 of 96** | 33 of 63 | **0 of 59** | 8 of 36 |
| Cost per 10,000 pages | **$4** | ~$150 | $10 | $16 |
| Tokens the agent reads | 300 | **148** | 351 | 524 |
| Time per page | 0.7 s | — | **0.3 s** | 0.5 s |
| Finds an image by what it shows | **yes** | no | no | no |
| Picks the links worth following | **yes** | no | no | no |
| Just the code | **yes** | no | no | no |
| Open source, in your terminal | **yes** | no | no | no |

Best in each row in bold. WebFetch hands the agent the fewest tokens because it rewrites what it reads: half the code it hands back isn't on the page, and each call has a small model ($1 per million tokens) read the whole page. Exa is the fastest, but missed both answers that were code. Tavily cut the code out of its extracts. Measured on 2026-10-07; jurl again on 2026-10-08.

## jurl cloud

Don't want to manage API keys? `jurl login` signs this computer in to jurl cloud, and jurl's servers do the reading. The flags and the exit codes stay the same, and the text matches a local run's except for the page's kind line (`· docs (0.84)`), which the server doesn't send, so it isn't printed. A `--precise` answer's block prints with the kind, heading level and code language the server sends, as a local run prints it, and `--json` carries them too. A block whose kind the server doesn't send prints as plain text.

```console
$ jurl login
To sign in to jurl cloud, open https://cloud.jurl.dev/device and enter this code:

    ABCD-EFGH

Waiting for it to be entered (it expires in 15 minutes). Ctrl-C cancels.
Saved the key "laptop" to ~/.config/jurl/env
Signed in as ray@acme.com to Acme. Console: https://cloud.jurl.dev
```

It opens that page in your browser when it can, and the sign-in finishes once the code is entered there.

```console
$ jurl status
jurl cloud · Acme · 1,240 page reads used · 18 days left
key "laptop"
```

`jurl status` says which account reads go to, and for jurl cloud, what the period has used. `jurl logout` signs this computer out and revokes its key on jurl cloud.

A run reads with jurl cloud or with your own keys, by the first of these that applies:

1. `JURL_CLOUD_KEY` in the environment: jurl cloud.
2. `TYPESAFE_API_KEY` or `JURL_JEV_URL` in the environment: your own keys. Set one for a single run to read with your own keys while you're signed in.
3. A sign-in saved by `jurl login`: jurl cloud.
4. Otherwise: your own keys.

jurl cloud takes the same reads as your own keys, except `--image`, `--vision`, `--find`, `-r`, and `--code` with `--precise`. `-n` takes 1 to 50 and `--threshold` 0 to 1; `-a` keeps every result above the threshold. `--follow` takes 5, 10 or 15 pages, with `--precise`. A read it doesn't take uses your own keys when they're set up, and says so on stderr; otherwise it stops and says why. With `--json`, `usage` counts the pages read for you, and its `jev` and `clef` counts stay at zero.

`JURL_CLOUD_URL` points jurl at another jurl cloud, such as a self-hosted one: `JURL_CLOUD_URL=http://127.0.0.1:3211 jurl login` saves it. A key only goes over https, or to this computer.

## Your keys, your data

With your own keys, jurl has no server and no account of its own. It talks to the model APIs directly with **your** keys:

- **Who you pay:** usage is billed by TypeSafe (Jev, $0.042 per million input tokens) and Cloudflare (Clef-flash, $0.09 per million). Output is free on both. Every `--json` result says what its run used, a `--precise` miss included: `"usage": {"pages": 3, "browser_retry": false, "plain_refusals": 0, "browser_requests": 0, "jev": {"requests": 6, "input_tokens": 41250}, "clef": {"requests": 0, "input_tokens": 0, "images": 0}}`. `pages` counts the pages read (with `--follow`, the whole search); `browser_retry` is true when one of them was read with the browser client (after a retry, or because its host needed it); `plain_refusals` and `browser_requests` count the requests sent with each client, a refusal being any non-success status; a request counts once it has answered.
- **What leaves your machine:** the text of the page goes to TypeSafe. With `--vision` or `--find`, the images go to Cloudflare too. Keep that in mind for internal or private pages.
- **What jurl can't read:** it sends no cookies, so pages behind a login are out of reach.

With [jurl cloud](#jurl-cloud), the URL and your question go to jurl cloud, which reads the page on its servers with the same models. It keeps a private log of each read for your organization, with the retention your organization sets. It also keeps an anonymous, de-identified record of reads of public pages (no account, user or key), used to improve jurl. Your organization can opt out of that record.

## Update and uninstall

- **Update:** `brew upgrade jurl`, or run the install script again.
- **Uninstall:** `brew uninstall jurl`, or delete `~/.local/bin/jurl`. To remove everything, run `jurl logout` first if you use jurl cloud (it revokes the key), then delete `~/.config/jurl` (keys) and `~/Library/Caches/jurl` or `~/.cache/jurl` (the browser).

<details>
<summary><b>How it works</b></summary>

```text
url ─▶ fetch (asks for markdown first) ─▶ split into blocks · links · images
          └─ empty JavaScript app or unfilled template? ─▶ render in Lightpanda
                          │
            one yes/no question per candidate
              ┌───────────┴───────────┐
              ▼                       ▼
       Jev reads text          Clef looks at images
   "is block 12 the point?"   "does this show a cathedral?"
          p = 0.97                  p = 0.96
              └───────────┬───────────┘
                          ▼
        rank · threshold · page order · print verbatim
```

- **Fetch.** jurl asks for `text/markdown` first; sites using Cloudflare's *Markdown for Agents* send it already converted (react.dev answers with markdown too, as `text/plain`: jurl reads that as markdown, so code keeps its lines). Otherwise jurl's own extractor drops navigation, footers, asides and scripts, and splits the rest into headings, paragraphs, list items, code, quotes and tables. A list item too short to judge alone ("1 teaspoon baking soda") is printed when an item next to it in the same list is kept.
- **Ask.** Every candidate becomes one yes/no question, and they all go to Jev in a single request. Asking 100 questions takes about as long as asking one.
- **Look.** Clef-flash checks the pixels of each image (downscaled to 384 px) while Jev reads the text, not after.
- **Print.** jurl keeps what clears the threshold, in page order. A heading comes back only when something in its section does.

</details>

<details>
<summary><b>Design notes</b></summary>

- **Models choose, they don't write.** That's why the output is safe to pipe into `xargs curl`.
- **Clef is better at "what is this?" than at "does this matter?"** Asked whether a chart was "meaningful content", Clef said 0.12. Asked *what it shows* (photo, chart, logo, avatar…), it was right. `--vision` adds up the content classes and averages them with Jev's judgement of the page context: a portrait is content on Wikipedia and noise in an author box.
- **For `--find`, Clef decides alone.** It sees the pixels *and* the alt text and caption.
- **Rendering waits for text, not silence.** Single-page apps never stop talking to the network, so Lightpanda waits for the network to calm down *and* for 1,500 characters of visible text, up to 8 s.
- **Several tricks keep `--vision` fast:**
  - Clef runs at the same time as Jev.
  - Each image gets its own HTTP/1 connection: sharing one HTTP/2 connection made the slowest calls about twice as slow.
  - A call that takes longer than 700 ms is sent again, and whichever copy answers first wins.
  - An image still pending at 2.5 s keeps its text-only score.
- **Lightpanda isn't bundled.** It's AGPL-3.0 and about 90 MB. jurl downloads a pinned 1.0.0 from Lightpanda's official release, checks its SHA-256 and caches it. If one is already on your `PATH`, jurl uses that. `JURL_LIGHTPANDA` points to a specific binary, and `JURL_NO_DOWNLOAD` stops the download. Lightpanda has no Windows build, so on Windows rendering is unavailable.

</details>

<details>
<summary><b>Development</b></summary>

```sh
cargo test        # extraction: layout tables, lazy images, markdown, links, app-shell detection
cargo build --release && ./target/release/jurl -t <url>
```

| File | What's in it |
| --- | --- |
| `src/main.rs` | Entry point: flags checked, the page loaded, the mode picked |
| `src/cli.rs` | The flags, as clap parses them and `--help` shows them |
| `src/judge.rs` | Jev's requests: items and questions, chunks that fit the budget, block pages |
| `src/blocks.rs` | The default mode and `-q`/`--code`: the best blocks, in page order |
| `src/answer.rs` | `--precise`: the answer as a span of the best blocks |
| `src/vision.rs` | `--image`, `--vision`, `--find`: content images, and Clef's look at the pixels |
| `src/output.rs` | What a run prints (text or `--json`), exit codes, misses |
| `src/timing.rs` | `-t`: per-phase timings on stderr |
| `src/precise.rs` | `--precise`: candidate spans and the link to them |
| `src/links.rs` | `--links` and `--links -q` (ranked by the answer), `--follow`: which links lead to the answer |
| `src/follow.rs` | `--follow`: site map, best-first search, hot and cold |
| `src/extract/mod.rs` · `src/extract/html.rs` · `src/extract/markdown.rs` · `src/extract/join.rs` | HTML and markdown → blocks, links, images |
| `src/decide.rs` | Jev and Clef clients |
| `src/fetch.rs` · `src/lightpanda.rs` | Fetching (with the retry on a browser's fingerprint), rendering, the browser download |
| `src/setup.rs` · `src/config.rs` | First-run setup (own keys or jurl cloud), `jurl init`, key storage |
| `src/cloud.rs` | jurl cloud: which account a run reads with, the read, the usage |
| `src/account.rs` | `jurl login`, `logout` and `status`: the sign-in in the browser, and signing out |
| `src/mock.rs` | Tests only: a local server that answers as jurl cloud does |
| `src/update.rs` | `jurl update` |
| `src/mcp.rs` | `jurl mcp`: the tools, their schemas, JSON-RPC over stdio |

Releases are built by [cargo-dist](https://opensource.axo.dev/cargo-dist/) when a `v*` tag is pushed.

</details>

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; security problems go to [SECURITY.md](SECURITY.md).

## License

MIT or Apache-2.0, at your option. [Lightpanda](https://lightpanda.io), which jurl downloads for JavaScript pages, is a separate program under AGPL-3.0.
