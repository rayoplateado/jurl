# Using jurl

Every mode, with real output. All the flags are in [reference.md](reference.md).

## Get the gist

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

## Ask one question

```console
$ jurl -q "What is the Jevons paradox?" -n 2 en.wikipedia.org/wiki/William_Stanley_Jevons
…
Jevons received public recognition for his work on The Coal Question (1865), in which he called attention to the gradual exhaustion of Britain's coal supplies and also put forth the view that increases in energy production efficiency leads to more, not less, consumption. …

## Practical economics

In The Coal Question, Jevons covered a breadth of concepts on energy depletion …
```

Out of a page with about 240 blocks, you get the two paragraphs that answer it.

## Just the answer

```console
$ jurl --precise -q "what is the primary rate limit for authenticated users?" docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api
5,000 requests per hour

All of these requests count towards your personal rate limit of 5,000 requests per hour. …

<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#:~:text=rate%20limit%20of-,5%2C000%20requests%20per%20hour>
```

The first line is the answer, still in the page's own words: jurl splits the best blocks into candidates (values, names, clauses, sentences, lines of code) and Jev picks the one that is exactly the answer, then, among the shorter ones inside it, the one with nothing extra ("2009", not "2009; 17 years ago"). Then the block it came from, and a link that opens the page with the answer highlighted. If no part of the page is the answer, jurl says so and exits with an error instead of guessing, so a script gets an empty cell, not a wrong one. It never computes: if the page says "$8 a month", it won't tell you the yearly price. On 30 SaaS pricing pages ("What is the monthly price of the cheapest paid plan?") it answered 15, all of them right (Notion's as `Plus*€9.50`, with the plan's name stuck to it on the page), and left the rest empty.

## Not on this page? Follow the site

```console
$ jurl --precise --follow -q "What is the monthly price of the cheapest paid plan?" linear.app
$10 per user/month
…
jurl: found after reading 4 pages: linear.app/ → linear.app/pricing
```

With `--follow`, a page that doesn't answer isn't the end. It is `--precise` and `--links -q` in a loop: each page is asked for the answer, and its links are scored by how likely they lead to it. jurl reads the site's own map (`llms.txt`, `sitemap.xml`, and the sitemaps its `robots.txt` names (a `.xml.gz` sitemap is read as the XML it holds): copies of a page in other languages are skipped, but the copies in the languages the run asks for are kept ([`JURL_ACCEPT_LANGUAGE`](reference.md#keys-and-settings)); a sitemap index's sitemaps for the run's country come first, in the run's language before the others ([`JURL_COUNTRY`](reference.md#keys-and-settings)), then those in the start page's language, then the newest; and when there are more than 300 listed pages, those in the start page's language and in the run's languages take turns first) and the `llms.txt` of the three hosts the page links most, on its own registrable domain (`docs.stripe.com` from `stripe.com`), while it reads the first page, then compares the best ten links side by side and opens the two it likes most, keeping on from whichever page looks closest, the way people play the Wikipedia game. On a long search (`--follow 10` and up) Jev also asks of each link whether its page is in the answer's field of knowledge: far from the answer, "leads to the answer" is noise (from Tennis: Birmingham, Philadelphia), while the field points the way (vulcanized rubber, polyester: chemistry, then the elements, then mercury). It plays hot and cold: a link counts for as much as its page is close to the question (a docs index with a link straight to the answer counts as close), so a wrong turn is dropped and the search goes back to the page that was getting warmer. A page that answers is ranked by how sure Jev is of the answer and of the page, and jurl stops only when no page left could beat it by much: the pricing page beats both a home-page FAQ line and an old blog post quoting last year's price. Started from a site's front door (`linear.app`), the question is asked about that site: "the company", "they", "we" or "it" is whoever runs it, not a customer in one of its stories. It follows the links a page gives, to another site too when the page names one (a link or a URL in its text, never one it makes up), scored like any other link; each host's `robots.txt` is checked, and it stops after 5 pages (`--follow 10` for more; longer searches open three pages per step). A page that declares another language than the start page (its `<html lang>`, compared by primary subtag, so `es-ES` is Spanish) is not read, and `-t` says so, unless that is the language the run asks for ([`JURL_ACCEPT_LANGUAGE`](reference.md#keys-and-settings)): a start page in Spanish, searched for an English question, also reads its English pages. A page that declares none is read as before. The path comes last, on stderr, or as `path` in `--json`; `-t` shows how warm each page was.

With `--precise`, a page whose text gives no answer is read for its JSON-LD values (schema.org): an event's `startDate` and `endDate`, an offer's `price` and `priceCurrency`, a course's `duration`. The best of them answers when Jev is sure of it, and its quote names the type, the field and the source ("Event VivaTech 2027 (JSON-LD): startDate 2027-06-16, endDate 2027-06-19."). Such an answer is marked as the page's structured data, and its link is the page itself, since its words are not on the page. A search that finds no answer also asks the site's own search, when the site declares one (an OpenSearch description, a schema.org `SearchAction`, or the WordPress REST search): its results join the search as leads, and it may open three pages more.

On [28 searches](../bench/follow.json), run once each on 2026-10-08: 27 right, against 25 for the build before (7e68b67). The eight pricing pages are found from the bare domain, 8/8 (about 3 pages, median ~2.6 s, ~$0.0035 each). Answers in docs from the docs' root: 3/3. Short Wikipedia games (Medicine → Aspirin: `C9H8O4`): 3/3. Questions the site doesn't answer, left empty: 3/3. Long games with `--follow 15`, 3 to 5 clicks apart: 10/11, the one miss being Bicycle → the Titanic. Five were tuned on (Paris → Aspirin, Tennis → the boiling point of mercury, Jazz → the moons of Mars, among them) and six never were (Volcano → Mona Lisa, Chess → the speed of sound, among them). Median ~5 s; ~$0.031 each on average (before: ~$0.035). Three runs of each, on earlier builds, are in [bench/README.md](../bench/README.md).

## Just the code

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

## Links worth following

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

## The real images

`--image` picks the content images by their file name, alt text and caption, with no logos, icons or tracking pixels. With `-q`, it also reads the words around an image that name it (its `title`, the link it sits in, or its card's heading) and the page's title. `--vision` also has Clef look at the pixels, which matters when the alt text says nothing:

| Image on a Cloudflare blog post | Alt text | `--image` | `--vision` |
| --- | --- | --- | --- |
| Stacked area chart | `BLOG-3162 4` | 0.64 | **0.82** |
| Diagram | `BLOG-3162 3` | 0.63 | **0.82** |
| Author avatar | `Will Allen` | 0.38 | **0.21** |
| Company logo | `Cloudflare` | 0.09 | **0.08** |

jurl asks for each image the way a browser's `<img>` does: with the page as its Referer, and an Accept without AVIF, which jurl can't decode. A 401, 403 or 429 is asked once more with the browser client, and a host that such a retry showed needs it is asked there first, as a page is. A host that refuses an image gets one or two requests in a run, not one per image: the rest of its images are skipped.

With `--vision` each image in the JSON has a `pixels` field: `looked` when Clef looked at its pixels, `refused` when its host refused the download, and `skipped` when it was not looked at (past the look cap, or after its host refused an earlier image). `--image` has no `pixels`, since it looks at nothing.

## Find a photo by description

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

The same goes for every mode: an HTTP error, a rate-limit or bot-check page served in place of the real one, or nothing above `--threshold` ends with a message on stderr and a non-zero exit code, never an empty answer: 1 when the page has nothing that answers, 2 when something failed (see [Exit codes](reference.md#exit-codes)). With `--precise --json` (with or without `--follow`) a miss still prints its JSON, `"answer": null` plus what came `closest` (`null` too when no block came close), and exits 1 all the same: a script that wants the closest passage reads stdout before checking the exit code. Without `--precise`, a miss prints nothing on stdout, `--json` or not. `-t` prints its timings and tokens on a miss too, so a miss can be costed.

Photos you already have, from many pages, are looked at the same way: give `-` as the page and list their addresses on stdin, one per line, each optionally followed by a space and the page it is on (sent as its Referer, as a browser would). They are judged as one page's images, with no page loaded, so `-a --threshold 0 --json` gives every photo with its `p`:

```console
$ jurl --find "a terrace" -a --threshold 0 --json - < photos.txt
```

`--find` looks at 80 images at most, as on a page: a longer list is shortlisted by its text first, and listed photos have none, so give it 80 or fewer at a time (`--vision` without a description looks at the first 12). An address listed twice is looked at once, and the JSON's `url` is `stdin:`.

Images are picked from `<img>` tags (`src`, `srcset` and the usual lazy-loading attributes) and the page's `og:image`. SVG images aren't candidates: they're mostly icons and logos, and Clef only reads raster images, so a post whose diagrams are all SVG (Stripe's engineering blog) has nothing to find.

## JavaScript apps

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
