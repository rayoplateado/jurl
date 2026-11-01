# Changelog

## Unreleased

- **A `-q --follow` read through jurl cloud links the page that answers**, as a local run does, not the page the search started on.
- **`jurl update` on a copy installed with cargo builds the release it announces** (`--tag vX.Y.Z --locked`), not main; when the build fails, it says what is usually missing and gives the ready-built installer command.
- **`--with-line`** (with `--precise -q`): the line of the answer's page a second reading points at, as `line` in `--json`, so a program can check the answer against it.

## 0.2.1

Fixes for `--follow`, with your own keys and with jurl cloud.

- **`--follow` no longer spends its pages on other languages.** A link that says its page is in another language (`<a hreflang>`, as a wiki's links to the same article in a hundred languages) is no longer opened: such a page was skipped anyway, but each one took a step of the `--follow` budget, so a search could end after reading three pages.
- **jurl cloud follows links with `-q` too**, not only with `--precise`. A read jurl cloud doesn't take says it runs on this computer with your own TypeSafe key.

## 0.2.0

jurl can now read with [jurl cloud](https://cloud.jurl.dev) instead of your own API keys, and an answer can become a sheet that jurl cloud keeps up to date. Reads with your own keys need no change.

### New: jurl cloud

- **`jurl login`** signs this computer in to jurl cloud with a code you enter in the browser. From then on, jurl's servers do the reading: same flags, same exit codes, and the same text, except the page's kind line (`· docs (0.84)`), which the server doesn't send. Signing up is free, with a monthly allowance of page reads; paid plans have more. `jurl init` and the first run offer the choice between your own keys and jurl cloud.
- **`jurl status`** says which account reads go to and, for jurl cloud, the plan and the page reads the period has used. **`jurl logout`** signs out and revokes the key.
- **Keep an answer up to date.** At a terminal, a jurl cloud answer to a question (or a miss) ends with a link on stderr: `jurl: keep it up to date in a sheet: …`. It opens the console, where the answer becomes the first row of a sheet, with its proof, and the sheet can add sites, questions and a schedule. A script reading the output doesn't get the line.
- **When the page reads run out**, a read stops with exit code 2 and prints jurl cloud's message, which says how to go on (extra page reads or a bigger plan).
- `jurl mcp` reads with the jurl cloud sign-in too. jurl cloud doesn't take `--image`, `--vision`, `--find`, `--with-image`, `--with-link`, `-r`, `--code` with `--precise`, or `--with-pages`; such a read uses your own keys when they're set up, and says so on stderr.

### Changes that can affect scripts

- **JSON-LD answers.** A `--precise` answer taken from the page's JSON-LD has the kind `structured`, and its `link` is the page itself, with no text fragment.
- **One line per answer.** An answer that spans lines (a table cell's paragraphs, a `<br>`) prints on one line. Code keeps its lines.
- **Markdown pages.** A page served as markdown answers in the words it shows, so `**bold**` and `[links](…)` no longer leak into the answer or its link. When the HTML page it stands for is verified, `url`, `link` and `path` name that HTML page.
- **Default output on lists.** Repeated cards are read as numbered records with their section titles, and with `--pages` the default block count grows with the pages read.
- **Hidden content.** Only `hidden`, `display:none` and `visibility:hidden` hide text now; `aria-hidden` content is read.
- **Cookies.** jurl keeps the cookies a site sets for the length of one run, in memory only, and sends each back only to the host and path that set it.

### Reads more sites

- **A site's own data first.** List reads take what the page's data already lists before reading its text: JSON-LD, a WordPress site's REST API (posts, WooCommerce products, The Events Calendar), app state such as `__NEXT_DATA__`, and the JSON a page fetches from its own site while it renders.
- **Feeds** (RSS, Atom, RDF) read as a page of their entries, each with its link, date, summary and image.
- **Long lists are read to their end** with `--pages`: next pages a page links, numbered pages it doesn't link, offsets (`?offset=24`, `start=`, `skip=`), the next batch a page's data names, and pages that add cards as you scroll.
- **Pages that refuse a plain fetch.** A 403 or 503, or a reset stream, is retried with a browser's TLS fingerprint (`--no-browser-retry` turns that off), a bare domain also tries `www`, and a 202 to a page is treated as a refusal, not as the page. Two optional fallbacks for hosts that still refuse: a residential proxy (`JURL_FALLBACK_PROXY`) and a stealth browser sidecar (`JURL_STEALTH_URL`). See [docs/reference.md](docs/reference.md#reaching-pages-that-refuse-a-plain-fetch) for both.
- **`--follow` searches a site better:** its sitemaps (gzip ones too) and `llms.txt`, the site's own search when nothing is found, and pages in the run's country and languages first (`JURL_COUNTRY`, `JURL_ACCEPT_LANGUAGE`). `JURL_JEV_MAX_TOKENS` caps what a search spends.
- **Cards** are read in more layouts: product cards built as buttons, a photo before its name, fields set on one line, a fee after a "+" beside a price. Links resolve against the page's `<base href>`.

### More precise answers

- **`--precise` answers with the cell** when the answer is in a table, and JSON-LD values come back structured. News front pages answer through their headlines.
- **`--precise --json`** carries the quoted block's kind, level and language, and the whole table or list as `context` when the answer is one of its parts.

### New flags and fields

- **`--with-image`**: the answer and its page's image in one read (`page_images` says how many images the page has). **`--with-link=WHAT`**: the link on the answer's page that WHAT names, under `page_link`.
- **`--with-pages`** puts the text of every page the read loaded in the JSON, with the things its JSON-LD lists as items, its photos, its search forms and its full JSON-LD photo gallery. With it, **`--with-sources`** lists a page's own data, **`--recipe JSON`** takes rows from it with no model, and **`--pages-only`** loads the pages and prints them with no model (the run exits 1, as a miss, with the pages in its JSON).
- **Look at a list of photos:** `-` as the page, with `--image`, `--vision` or `--find`, judges the image addresses given on stdin. One photo served in two formats counts once. `--vision --json` says per image whether its pixels were looked at (`pixels`).
- **`--json` `usage`** gains `bytes` (read directly and through the proxy, and the proxy's wire bytes), the browser-retry and proxy counts, and `jev.capped` when `JURL_JEV_MAX_TOKENS` stopped a search.
- `JURL_PUBLIC_ONLY=1` reads only public addresses, for running jurl as a service.
