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

<sub>Windows: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.ps1 | iex"` · From source: `cargo install --git https://github.com/rayoplateado/jurl`</sub>

Then run it. The first time, jurl asks for a [TypeSafe API key](https://console.typesafe.ai), checks it and saves it. That's the whole setup. Pages that need JavaScript just work too: jurl fetches a headless browser the first time one shows up.

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
$ jurl --links -n 3 -q "official installation instructions" github.com/BurntSushi/ripgrep
https://www.macports.org/ports.php?by=name&substr=ripgrep
https://packages.gentoo.org/packages/sys-apps/ripgrep
https://chocolatey.org/packages/ripgrep
```

The ranking puts content first, ahead of `login`, `share` or `privacy policy`. Add `-q` to keep only the links about something specific.

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

The same goes for every mode: an HTTP error, a rate-limit or bot-check page served in place of the real one, or nothing above `--threshold` ends with a message on stderr and a non-zero exit code, never an empty answer.

### JavaScript apps

Some pages arrive empty because their content is built by JavaScript. jurl spots those and renders them in [Lightpanda](https://lightpanda.io), a fast headless browser:

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

## Reference

| Flag | |
| --- | --- |
| `-q, --ask "…"` | Keep what answers the question. Works with every mode |
| `-p, --precise` | With `-q`: just the answer, in the page's words, then its block and a link to it. Exits with an error when no part of the page is the answer |
| `-c, --code` | Code blocks only |
| `-l, --links` | Content links, best first |
| `-i, --image` | Content images, judged by file name, alt text and caption |
| `--vision` | Like `--image`, plus Clef looks at the pixels |
| `-f, --find "…"` | The image that best matches the description |
| `-r, --render` | Run the page's JavaScript first (automatic for empty JavaScript apps) |
| `-n, --max N` | How many results (12 blocks, 5 with `--ask`, 8 code blocks, 20 links, 1 with `--find`) |
| `-a, --all` | No limit: everything above the threshold |
| `--threshold P` | Minimum probability (default 0.5; 0.4 for the `--precise` answer) |
| `--json` | Machine-readable output, with every probability |
| `-t, --timing` | Where the time went, on stderr |
| `jurl init` | Set or replace your API keys |
| `jurl update` | Install the latest jurl, the same way this one was installed |

Keys live in `~/.config/jurl/env`. Environment variables take precedence over that file: `TYPESAFE_API_KEY`, and for images `CLOUDFLARE_ACCOUNT_ID` plus `CLOUDFLARE_AI_TOKEN`.

`--vision` and `--find` give Clef 2.5 s per image; an image slower than that keeps its text-only score. For batch use, where a slow host matters more than a second of waiting, raise it with `JURL_VISION_TIMEOUT_MS` (e.g. `10000`).

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
| Code lines not on the page | **0 of 60** | 33 of 63 | **0 of 59** | 8 of 36 |
| Cost per 10,000 pages | **$4** | ~$150 | $10 | $16 |
| Tokens the agent reads | 375 | **148** | 351 | 524 |
| Time per page | 0.8 s | — | **0.3 s** | 0.5 s |
| Finds an image by what it shows | **yes** | no | no | no |
| Picks the links worth following | **yes** | no | no | no |
| Just the code | **yes** | no | no | no |
| Open source, in your terminal | **yes** | no | no | no |

Best in each row in bold. WebFetch hands the agent the fewest tokens because it rewrites what it reads: half the code it hands back isn't on the page, and each call has a small model ($1 per million tokens) read the whole page. Exa is the fastest, but missed both answers that were code. Tavily cut the code out of its extracts. Measured on 2026-10-07.

## Your keys, your data

jurl has no server and no account of its own. It talks to the model APIs directly with **your** keys:

- **Who you pay:** usage is billed by TypeSafe (Jev, $0.042 per million input tokens) and Cloudflare (Clef-flash, $0.09 per million). Output is free on both.
- **What leaves your machine:** the text of the page goes to TypeSafe. With `--vision` or `--find`, the images go to Cloudflare too. Keep that in mind for internal or private pages.
- **What jurl can't read:** it sends no cookies, so pages behind a login are out of reach.

## Update and uninstall

- **Update:** `brew upgrade jurl`, or run the install script again.
- **Uninstall:** `brew uninstall jurl`, or delete `~/.local/bin/jurl`. To remove everything, also delete `~/.config/jurl` (keys) and `~/Library/Caches/jurl` or `~/.cache/jurl` (the browser).

<details>
<summary><b>How it works</b></summary>

```text
url ─▶ fetch (asks for markdown first) ─▶ split into blocks · links · images
          └─ empty JavaScript app? ─▶ render in Lightpanda
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

- **Fetch.** jurl asks for `text/markdown` first; sites using Cloudflare's *Markdown for Agents* send it already converted. Otherwise jurl's own extractor drops navigation, footers, asides and scripts, and splits the rest into headings, paragraphs, list items, code, quotes and tables.
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
| `src/main.rs` | Modes, chunking, hedging, output |
| `src/precise.rs` | `--precise`: candidate spans and the link to them |
| `src/extract.rs` | HTML and markdown → blocks, links, images |
| `src/decide.rs` | Jev and Clef clients |
| `src/fetch.rs` · `src/lightpanda.rs` | Fetching, rendering, the browser download |
| `src/setup.rs` · `src/config.rs` | First-run key prompt, `jurl init`, key storage |
| `src/update.rs` | `jurl update` |

Releases are built by [cargo-dist](https://opensource.axo.dev/cargo-dist/) when a `v*` tag is pushed.

</details>

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; security problems go to [SECURITY.md](SECURITY.md).

## License

MIT or Apache-2.0, at your option. [Lightpanda](https://lightpanda.io), which jurl downloads for JavaScript pages, is a separate program under AGPL-3.0.
