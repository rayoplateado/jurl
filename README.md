# jurl

**curl that reads the page for you.**

```console
$ jurl --find "a cathedral" en.wikipedia.org/wiki/Cologne
https://thumb.wikimedia.org/wikipedia/commons/thumb/2/27/Kdom.jpg/250px-Kdom.jpg
```

There are 73 images on that page, and jurl looked at every one of them. The cathedral came back in **2.3 seconds** for **$0.002**.

jurl fetches a URL and gives you only what matters: the paragraphs worth reading, the answer to your question, the code you came for, the links worth following, or the photo that matches your description.

It does this with **decision models**, not a chatbot. [Jev](https://docs.typesafe.ai/introduction) (TypeSafe) judges text and [Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/) (Cloudflare) judges pixels. Neither writes a word: each returns probabilities over options that jurl hands it.

> **Everything jurl prints is literally on the page.** No summaries that drift, no invented quotes, no hallucinated URLs. A model can rank things badly, but it cannot make things up.

It is also fast (most pages in under a second) and cheap (roughly a thousand pages per dollar).

---

- [What it does](#what-it-does)
- [How it works](#how-it-works)
- [Install](#install)
- [Usage](#usage): [read](#read-a-page) · [ask](#ask-a-question) · [code](#grab-the-code) · [links](#links-worth-following) · [images](#content-images) · [find](#find-a-photo-by-description) · [JavaScript apps](#javascript-apps) · [scripting](#scripting)
- [Flags](#flags)
- [Speed](#speed)
- [Cost](#cost)
- [Design notes](#design-notes)
- [Limits](#limits)

## What it does

| You want | Run | You get |
| --- | --- | --- |
| The gist of a page | `jurl <url>` | Title, page type, and the 12 blocks that carry the page, in reading order |
| One answer | `jurl -q "how do I install it?" <url>` | The 5 blocks that best answer it |
| The code | `jurl -c <url>` | Code blocks only, under their headings |
| Where to go next | `jurl -l <url>` | Content links, best first, with no navigation or share buttons |
| The real images | `jurl -i <url>` | Content images, with no logos, icons, avatars or trackers |
| One specific photo | `jurl -f "a bridge at night" <url>` | The image that best matches the description |
| A JavaScript app | `jurl -r <url>` | All of the above, after running the page in a headless browser |

## How it works

```text
            ┌──────────── Accept: text/markdown ─────────────┐
url ─fetch─▶│ HTML ──scraper──▶ blocks · links · images        │
            │  └─ no text + app shell? ─▶ Lightpanda render   │
            └────────────────────────┬────────────────────────┘
                                     │  one typed question per candidate
                     ┌───────────────┴───────────────┐
                     ▼                               ▼
              Jev (TypeSafe)                 Clef-flash (Cloudflare)
      "does block 12 carry the page?"      "does this image show: …?"
               → p = 0.97                          → p = 0.96
                     └───────────────┬───────────────┘
                                     ▼
                  rank · threshold · keep page order · print
```

1. **Fetch.** jurl asks for `text/markdown` first, so sites using Cloudflare's *Markdown for Agents* send it already converted. Otherwise jurl's own extractor parses the HTML. It drops navigation, footers, asides and scripts, and splits the rest into blocks.
2. **Ask.** Each candidate block, link or image becomes one yes/no question. They all go to Jev in a single request (a few parallel requests for huge pages), so asking 100 questions takes about as long as asking one.
3. **Look.** With `--vision` or `--find`, Clef-flash looks at the actual pixels of each image while Jev reads the text.
4. **Print.** jurl keeps what clears the threshold, caps the count, and prints it verbatim.

## Install

```sh
# macOS or Linux, with Homebrew
brew install rayoplateado/tap/jurl

# macOS or Linux, without Homebrew
curl -LsSf https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.ps1 | iex"

# From source
cargo install --git https://github.com/rayoplateado/jurl
```

Then just run it. The first time, jurl asks for a [TypeSafe](https://console.typesafe.ai) API key, checks it against the API and saves it:

```console
$ jurl example.com
jurl reads pages with Jev, TypeSafe's decision model. It needs your API key, once.
Get one at https://console.typesafe.ai

    TypeSafe API key (hidden):
    Checking… ok. Saved to ~/.config/jurl/env

# Example Domain

<https://example.com/> · docs (0.86)

This domain is for use in documentation examples without needing permission. …
```

`jurl init` adds the optional Cloudflare token for `--vision` and `--find`, or replaces either key. The keys are saved to `~/.config/jurl/env` with `0600` permissions. You can also use environment variables:

| Variable | What it's for |
| --- | --- |
| `TYPESAFE_API_KEY` | Everything. Get one at [console.typesafe.ai](https://console.typesafe.ai) |
| `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_AI_TOKEN` | `--vision` and `--find` (a token with the *Workers AI* permission) |
| `JURL_LIGHTPANDA` | Use this Lightpanda binary instead of the one jurl manages |
| `JURL_NO_DOWNLOAD` | Never download Lightpanda |

**JavaScript rendering needs no setup.** The first time a page needs JavaScript, jurl downloads [Lightpanda](https://lightpanda.io) 1.0.0 (about 90 MB on macOS, once). It comes from Lightpanda's official GitHub release, and jurl checks its SHA-256 before running it and caches it in `~/Library/Caches/jurl` (or `~/.cache/jurl` on Linux). Lightpanda is AGPL-3.0, which is why jurl never bundles it. If you already have it on `PATH`, jurl uses that one. Lightpanda has no Windows build, so `--render` isn't available on Windows.

## Usage

All outputs below are real. They were captured on 2026-10-04 and trimmed with `…` where long.

### Read a page

```console
$ jurl -n 4 en.wikipedia.org/wiki/William_Stanley_Jevons
# William Stanley Jevons - Wikipedia

<https://en.wikipedia.org/wiki/William_Stanley_Jevons> · article (0.97)

# William Stanley Jevons

| William Stanley Jevons FRS |
|  |
| Born | 1 September 1835 Liverpool, Lancashire, England |
| Died | 13 August 1882 (aged 46) Bexhill-on-Sea, Sussex, England |
…
| Known for | Marginal utility theory Jevons paradox |
…

William Stanley Jevons FRS (/ˈdʒɛvənz/;[2] 1 September 1835 – 13 August 1882) was an English economist and logician.

Irving Fisher described Jevons's book The Theory of Political Economy (1871) as the start of the mathematical method in economics.[3] …

Jevons broke off his studies of the natural sciences in London in 1854 to work as an assayer in Sydney, …
```

The page has about 240 blocks. Jev kept the infobox and the lede, threw away the navigation, the references and the footers, and classified the page as an `article` with 0.97 confidence. A heading only comes back when something in its section survives. It took **868 ms** end to end, 395 ms of which was Jev.

### Ask a question

````console
$ jurl -q "how do I install it on macOS?" github.com/BurntSushi/ripgrep
# GitHub - BurntSushi/ripgrep: ripgrep recursively searches directories for a regex pattern …

<https://github.com/BurntSushi/ripgrep> · docs (0.51)

### Installation

Archives of precompiled binaries for ripgrep are available for Windows, macOS and Linux. …

If you're a macOS Homebrew or a Linuxbrew user, then you can install ripgrep from homebrew-core:

```
$ brew install ripgrep
```

If you're a MacPorts user, then you can install ripgrep from the official ports:

```
$ sudo port install ripgrep
```
````

That's exactly the macOS part of a long README. On Wikipedia, *"What is the Jevons paradox?"* returns the two paragraphs about *The Coal Question* (p = 0.96 and 0.97) and none of the biography.

`--ask` works with every mode: `-c -q "…"` keeps the matching code, `-l -q "…"` the matching links, and `-i -q "…"` the matching images.

### Grab the code

````console
$ jurl -c -n 3 github.com/BurntSushi/ripgrep
…
### Installation

```
$ brew install ripgrep
```

```
$ sudo port install ripgrep
```

### Building

```
$ rustup target add x86_64-unknown-linux-musl
$ cargo build --release --target x86_64-unknown-linux-musl
```
````

### Links worth following

```console
$ jurl -l -n 5 news.ycombinator.com
https://www.youtube.com/watch?v=UOuxo6SA8Uc
https://www.da.vidbuchanan.co.uk/blog/hacking-time.html
https://www.phoronix.com/news/XDC-2026-Valve-Timur-AMDGPU
https://www.theatlantic.com/technology/2026/10/openai-safety-team-resignation/688881/?gift=…
https://news.ycombinator.com/from?site=royapakzad.substack.com
```

You get the stories, not `login`, `submit` or `hide`. Add a question to filter them:

```console
$ jurl -l -n 5 -q "official installation instructions" github.com/BurntSushi/ripgrep
https://chocolatey.org/packages/ripgrep
https://www.macports.org/ports.php?by=name&substr=ripgrep
https://packages.gentoo.org/packages/sys-apps/ripgrep
https://github.com/ScoopInstaller/Main/blob/master/bucket/ripgrep.json
https://www.freshports.org/textproc/ripgrep/
```

### Content images

`--image` judges each image by its file name, alt text, caption and size. It never downloads the image, costs almost nothing, and is usually enough:

```console
$ jurl -i en.wikipedia.org/wiki/William_Stanley_Jevons
https://upload.wikimedia.org/wikipedia/commons/0/0a/William_Stanley_Jevons.jpg?…
https://thumb.wikimedia.org/wikipedia/commons/thumb/1/14/William_Stanley_Jevons_Logic_Piano.jpg/500px-William_Stanley_Jevons_Logic_Piano.jpg?…
https://thumb.wikimedia.org/wikipedia/commons/thumb/2/26/William_Stanley_Jevons_1858_extract.jpg/500px-William_Stanley_Jevons_1858_extract.jpg?…
…
```

The captions do the work here: *"Portrait of W. Stanley Jevons at 42"* (p = 0.97), *"Jevons's Logic Piano in the Sydney Powerhouse Museum"* (0.97), *"Jevons in Sydney (age 22)"* (0.96).

When the alt text says nothing, `--vision` lets Clef look at the pixels too. These scores come from `--json` on a Cloudflare blog post:

| Image | Alt text | `--image` (text only) | `--vision` (pixels + text) |
| --- | --- | --- | --- |
| Stacked area chart | `BLOG-3162 4` | 0.64 | **0.82** |
| Diagram | `BLOG-3162 3` | 0.63 | **0.82** |
| Hero illustration | `BLOG-3162 1` | 0.62 | **0.78** |
| Author avatar | `Will Allen` | 0.38 | **0.21** |
| Author avatar | `Celso Martinho` | 0.36 | **0.21** |
| Company logo | `Cloudflare` | 0.09 | **0.08** |

From text alone, a chart named `BLOG-3162 4` and an author's avatar look about the same. The pixels tell them apart.

### Find a photo by description

`--find` asks Clef the same question about every image on the page: *"The attached image shows: …"*. You get the best match, or the top N with `-n`:

```console
$ jurl -f "a cathedral" -n 3 --json en.wikipedia.org/wiki/Cologne
```

| # | Image | What's in it | p |
| --- | --- | --- | --- |
| 1 | `Kdom.jpg` | Cologne Cathedral (alt: "Cologne Cathedral") | 0.96 |
| 2 | `Köln_um_1890.jpg` | Cologne from the Rhine in 1890, the cathedral towering over the skyline | 0.95 |
| 3 | `Kranhäuser_Cologne_April_2018.jpg` | The Rhine at dusk; the lit cathedral sits on the right. No alt, no caption | 0.95 |

Number 3 has no text at all, so only the pixels could have found it.

```console
$ jurl -f "a bridge over a river" en.wikipedia.org/wiki/Cologne
https://thumb.wikimedia.org/wikipedia/commons/thumb/6/69/Bundesarchiv_Bild_183-R27436%2C_K%C3%B6ln%2C_R%C3%BCckkkehr_deutscher_Truppen.jpg/500px-…
```

The caption of that photo: *"German Army troops return to the right bank of the Rhine via the Deutz Suspension Bridge"*.

```console
$ jurl -f "a carnival parade with costumes" en.wikipedia.org/wiki/Cologne
jurl: no image in https://en.wikipedia.org/wiki/Cologne looks like "a carnival parade with costumes" (closest: …, p=0.02)
```

When nothing matches, jurl tells you so instead of handing over the least-bad photo.

The output is plain URLs, so you can pipe it straight into curl:

```sh
jurl -f "a stacked area chart" blog.cloudflare.com/markdown-for-agents/ | xargs curl -sO
```

### JavaScript apps

Some pages arrive as an empty app shell: scripts plus a `#root` element, a `<noscript>` tag or a heavy bundle, and no text. jurl renders those in [Lightpanda](https://lightpanda.io) automatically, and downloads Lightpanda the first time it's needed:

```console
$ jurl -n 3 hn.algolia.com
jurl: no text without JavaScript, rendering with Lightpanda…
# HN Search powered by Algolia

<https://hn.algolia.com/> · listing (1.00)

Stephen Hawking has died(http://www.bbc.com/news/uk-43396008)

6015 points|Cogito|9 years ago|436 comments

OpenAI's board has fired Sam Altman(https://openai.com/blog/openai-announces-leadership-transition)
```

Add `-r` to force it:

```console
$ jurl -r -n 3 bsky.app/profile/bsky.app
# Bluesky (@bsky.app)

<https://bsky.app/profile/bsky.app> · listing (0.92)

35.1M followers15 following

official Bluesky account (check username👆) Bugs, feature requests, feedback: support@bsky.app

👋 Bluesky is an open social network that gives creators independence from platforms, …
```

### Scripting

`--json` gives you every probability. `-t` prints where the time went to stderr, so pipes stay clean. The JSON below has been collapsed to one line per block:

```console
$ jurl --json -n 2 -q "What is the Jevons paradox?" en.wikipedia.org/wiki/William_Stanley_Jevons
{
  "ask": "What is the Jevons paradox?",
  "blocks": [
    { "i": 0, "kind": "heading", "level": 1, "text": "William Stanley Jevons" },
    { "i": 49, "kind": "para", "p": 0.96, "text": "Jevons received public recognition for his work on The Coal Question (1865), …" },
    { "i": 63, "kind": "heading", "level": 2, "text": "Practical economics" },
    { "i": 67, "kind": "para", "p": 0.97, "text": "In The Coal Question, Jevons covered a breadth of concepts on energy depletion …" }
  ],
  "kind": { "choice": "article", "confidence": 0.97 },
  "title": "William Stanley Jevons - Wikipedia",
  "url": "https://en.wikipedia.org/wiki/William_Stanley_Jevons"
}

$ jurl -t -f "a cathedral" en.wikipedia.org/wiki/Cologne > /dev/null
⏱  fetch 343ms · extract 26ms · jev(1 req, 8619 tok) ‖ clef(73 img) 2478ms · total 2850ms
```

## Flags

| Flag | |
| --- | --- |
| `-q, --ask "…"` | Keep what answers the question instead of a general summary. Works with every mode |
| `-c, --code` | Code blocks only |
| `-l, --links` | Content links, best first |
| `-i, --image` | Content images, best first, judged by file name, alt text, caption and size |
| `--vision` | Like `--image`, but Clef also looks at the pixels of the first 12 images |
| `-f, --find "…"` | The image that best matches the description. Clef looks at up to 80 images; on bigger pages Jev shortlists them from their text first |
| `-r, --render` | Run the page's JavaScript in Lightpanda first. This happens automatically for empty app shells |
| `-n, --max N` | Number of results. Defaults: 12 blocks, 5 with `--ask`, 8 code blocks, 20 links, every image, 1 with `--find` |
| `-a, --all` | No cap: keep everything above the threshold |
| `--threshold P` | Minimum probability to keep a result (default 0.5) |
| `--json` | Machine-readable output, including every `p` |
| `-t, --timing` | Per-phase timings on stderr |

URLs without a scheme get `https://` added.

## Speed

Median of 3 runs on 2026-10-04:

| Page | Fetch | Jev | Total |
| --- | --- | --- | --- |
| blog.cloudflare.com (served as markdown) | 122 ms | 440 ms | **563 ms** |
| Wikipedia, ~240 blocks (2 parallel requests) | 247 ms | 416 ms | **672 ms** |
| GitHub README | 242 ms | 373 ms | **647 ms** |
| The Rust Book | 104 ms | 504 ms | **605 ms** |
| Hacker News | 732 ms | 245 ms | **982 ms** |

| Mode | Typical total |
| --- | --- |
| `--image` | 0.5–0.7 s |
| `--vision` (6–8 images) | 1.3–1.9 s |
| `--find` (73 images) | 2.2–2.9 s |
| `--render` (JavaScript apps) | 2.5–5 s, mostly spent waiting on the page itself |

Parsing never takes more than a few tens of milliseconds; the rest is the network and the models. A few things keep it that way:

- **The API connections open while the page downloads.** The TLS handshakes are done before the first question goes out.
- **All the questions go in one request.** Jev evaluates them in parallel, so 100 blocks take about as long as one.
- **Clef runs at the same time as Jev.** So `--vision` costs whichever of the two is slower, not their sum.
- **Each image gets its own HTTP/1 connection to Clef.** Sending them all over one HTTP/2 connection made the slowest calls about twice as slow.
- **Slow Clef calls get a backup.** After 700 ms jurl sends a duplicate and takes whichever answers first. An image still waiting at 2.5 s keeps its text-only score from Jev.
- **Images are shrunk before upload.** jurl downloads the smallest `srcset` variant and resizes it to 384 px, which costs about 255 vision tokens per image.

## Cost

Only input tokens are billed: Jev costs $0.042 per million and Clef-flash $0.09 per million. Measured on the pages above:

| Call | Tokens | Cost | Calls per dollar |
| --- | --- | --- | --- |
| `jurl <url>` (article) | 10k–32k Jev | $0.0004–0.0013 | ~750–2,400 |
| `jurl -l` (Hacker News) | ~23k Jev | ~$0.001 | ~1,000 |
| `jurl -i` | ~1k Jev | ~$0.00004 | ~24,000 |
| `jurl --vision` (8 images) | ~1k Jev + 8 × 255 Clef | ~$0.0002 | ~4,300 |
| `jurl -f` (73 images) | ~9k Jev + 73 × 255 Clef | ~$0.002 | ~500 |

A Clef call that gets a backup is paid twice, so in the worst case the Clef part of the bill doubles. `-t` and `--json` show how many tokens Jev used on each call.

## Design notes

- **Models choose, they don't write.** Every block, link and image is a candidate. The model returns P(yes) and the code decides what to keep. That is why the output is safe to pipe into `xargs curl`.
- **The extractor is jurl's own**, not a readability port: `scraper` plus a small walker. It drops `nav`, `footer`, `aside` and scripts, and splits the rest into headings, paragraphs, list items, code, quotes and tables.
  - Layout tables (tables that contain tables) are treated as containers, not as data.
  - Lazy-loaded images (`data-src`, `srcset`) are resolved to real URLs.
  - Obvious noise never reaches a model: SVGs, tracking pixels, sprites and tiny images are dropped up front.
  - Jev takes care of the subtler boilerplate.
- **Text comes out in page order, not score order.** A summary should read like the page.
- **Headings aren't judged.** A heading prints when something in its section survives, so the structure comes for free.
- **Clef is better at "what is this?" than at "does this matter?"** Asked whether a chart was "meaningful content", Clef said 0.12. Asked *what it shows* (photo, chart, diagram, screenshot, logo, avatar, ad…), it got it right.
  - `--vision` adds up the probabilities of the content classes and averages the result with Jev's judgement of the page context.
  - The context matters because a portrait is content on Wikipedia and noise in an author box.
- **For `--find`, Clef's answer is final.** It sees the pixels *and* the alt text and caption, so its probability already combines both.
- **Rendering waits for text, not for silence.** Single-page apps never stop talking to the network, so Lightpanda waits for `networkalmostidle` *and* for `innerText` to pass 1,500 characters, up to 5 s.

## Limits

- **Jev and Clef are new** (launched September and October 2026), so prices, limits and calibration may change. jurl pins `jev-1.13.0` to keep its thresholds meaningful.
- **Logged-in pages are out of scope**: jurl sends no cookies.
- **The output is only as good as the page's markup.** Text inside a canvas or baked into an image is invisible to the text modes.
- **`--render` isn't available on Windows** (Lightpanda has no Windows build). Pages that only show content after you interact with them will still come back thin.
- **The speed and cost numbers come from one machine on one day.** Run with `-t` to see your own.

## Development

```sh
cargo test             # extraction: layout tables, lazy images, markdown, links, app-shell detection
cargo build --release
./target/release/jurl -t <url>
```

The code is deliberately small:

| File | What's in it |
| --- | --- |
| `src/fetch.rs` | HTTP fetching and Lightpanda rendering |
| `src/lightpanda.rs` | Finding Lightpanda, or downloading and verifying it on first use |
| `src/setup.rs` | First-run key prompt and `jurl init` |
| `src/config.rs` | Keys from the environment and `~/.config/jurl/env` |
| `src/extract.rs` | HTML and markdown → blocks, links and images |
| `src/decide.rs` | Jev and Clef clients (same `{state, questions} → answers` contract) |
| `src/main.rs` | The modes, chunking, hedging and output |

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

[Lightpanda](https://lightpanda.io), which jurl downloads on first use for JavaScript rendering, is a separate program under AGPL-3.0. It is not bundled with jurl.
