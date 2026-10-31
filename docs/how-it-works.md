# How jurl works

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

Ten documentation pages, one question each ([bench/](../bench) has the tasks, the scripts and every answer):

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

## The pipeline

```text
url ─▶ fetch (asks for markdown first) ─▶ split into blocks · links · images
          └─ empty JavaScript app or unfilled template? ─▶ render in Lightpanda
          └─ refused (202, 401, 403 or 429, or a stream reset) by the direct clients, or (401, 403 or 429) by its render, and JURL_FALLBACK_PROXY set? ─▶ read again through the proxy
          └─ still refused (202, 401, 403 or 429, or a stream reset), and JURL_STEALTH_URL set? ─▶ stealth sidecar
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

- **Fetch.** jurl asks for `text/markdown` first (a `--with-pages` run asks for the HTML); sites using Cloudflare's *Markdown for Agents* send it already converted (react.dev answers with markdown too, as `text/plain`: jurl reads that as markdown, so code keeps its lines). Otherwise jurl's own extractor drops navigation, footers, asides and scripts, and splits the rest into headings, paragraphs, list items, code, quotes and tables. A list item too short to judge alone ("1 teaspoon baking soda") is printed when an item next to it in the same list is kept.
- **Ask.** Every candidate becomes one yes/no question, and they all go to Jev in a single request. Asking 100 questions takes about as long as asking one.
- **Look.** Clef-flash checks the pixels of each image (downscaled to 384 px) while Jev reads the text, not after.
- **Print.** jurl keeps what clears the threshold, in page order. A heading comes back only when something in its section does.


## Design notes

- **Models choose, they don't write.** That's why the output is safe to pipe into `xargs curl`.
- **Clef is better at "what is this?" than at "does this matter?"** Asked whether a chart was "meaningful content", Clef said 0.12. Asked *what it shows* (photo, chart, logo, avatar…), it was right. `--vision` adds up the content classes and averages them with Jev's judgement of the page context: a portrait is content on Wikipedia and noise in an author box.
- **For `--find`, Clef decides alone.** It sees the pixels *and* the alt text and caption.
- **Rendering waits for text, not silence.** Single-page apps never stop talking to the network, so Lightpanda waits for the network to calm down *and* for 1,500 characters of visible text, up to 8 s. A page that adds its cards or images as you scroll (an `IntersectionObserver` on them, or a scroll listener) is scrolled to its end first: every element the page watches is told it is in view, and the window scrolls, every quarter second, until the page stops growing with none of its requests in flight (a request open for over 2 s, a stream, is not waited for), or after 12 tries (about 3 s). Those are requests the page makes itself, as it would for a person scrolling, and a render through the fallback proxy counts them in `proxy_render_wire`. A page that loads nothing more costs about 1 s more to render, the time it takes to see that it has stopped. A list that both loads more as it scrolls and links its next pages by offset may hold more records after scrolling than one page's step, so jurl may not follow those links: what the scrolling loaded is read instead.
- **Several tricks keep `--vision` fast:**
  - Clef runs at the same time as Jev.
  - Each image gets its own HTTP/1 connection: sharing one HTTP/2 connection made the slowest calls about twice as slow.
  - A call that takes longer than 700 ms is sent again, and whichever copy answers first wins.
  - An image still pending at 2.5 s keeps its text-only score.
- **Lightpanda isn't bundled.** It's AGPL-3.0 and about 90 MB. jurl downloads a pinned 1.0.0 from Lightpanda's official release, checks its SHA-256 and caches it. If one is already on your `PATH`, jurl uses that. `JURL_LIGHTPANDA` points to a specific binary, and `JURL_NO_DOWNLOAD` stops the download. With both set, as on a host that has Lightpanda preinstalled, that binary must be the pinned 1.0.0 release for this platform: jurl checks its SHA-256 and refuses any other. Lightpanda has no Windows build, so on Windows rendering is unavailable.
- **Stealth is a sidecar, for a page a host still refuses.** When a page is still refused with 202, 401, 403 or 429, or its stream is reset, after the browser retry and, if one is set, the fallback proxy, and `JURL_STEALTH_URL` names a local service, jurl asks that service once for the page. The sidecar loads it in Camoufox (MPL-2.0) and returns the rendered HTML, which jurl reads like any page. The sidecar says what it found: content, a CAPTCHA, a block or a challenge. jurl never solves a CAPTCHA. That page is an error and goes to human review. A host whose call fails is not asked again in the same run, and a run makes at most `JURL_STEALTH_MAX_PAGES` calls (3 by default). Under `JURL_PUBLIC_ONLY=1` the sidecar is asked only with `JURL_RENDER_SANDBOXED=1`, because it runs the page's JavaScript. It resolves the host again, so that check is not proof against DNS rebinding.

