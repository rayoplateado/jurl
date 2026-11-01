<p align="center">
  <img src="docs/jurl.png" alt="Ink drawing of a man in a suit and leopard-print tie, pinching his fingers as if picking something out" width="380">
</p>

<h1 align="center">jurl</h1>

<p align="center"><b>curl that reads the page for you.</b><br>
Tell it what you want. It picks it out of the page.</p>

---

```console
$ jurl --precise -q "what is the primary rate limit for authenticated users?" docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api
5,000 requests per hour

All of these requests count towards your personal rate limit of 5,000 requests per hour. …

<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#:~:text=rate%20limit%20of-,5%2C000%20requests%20per%20hour>
```

The answer, in the page's own words, then the block it came from and a link that opens the page with it highlighted. If the page doesn't say it, jurl says so and exits 1 instead of guessing.

## Why jurl

- **It picks. It doesn't write.** jurl uses *decision models*, not a chatbot: [Jev](https://docs.typesafe.ai/introduction) reads the text and [Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/) looks at the images. They only say how likely each paragraph, link or image is to be what you want, and jurl prints the winners **exactly as they appear on the page**. No summary that drifts, no URL that doesn't exist.
- **It's fast.** Most pages take under a second.
- **It's cheap.** About a thousand pages per dollar.

## Install

```sh
brew install rayoplateado/tap/jurl                                                                # macOS, Linux
curl -LsSf https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.sh | sh   # no Homebrew
```

<sub>Windows: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/rayoplateado/jurl/releases/latest/download/jurl-installer.ps1 | iex"` · From source (Rust 1.98, cmake and libclang, see [CONTRIBUTING](CONTRIBUTING.md#develop)): `cargo install --git https://github.com/rayoplateado/jurl --locked`</sub>

Then run `jurl`. The first time, it asks how you want to read pages: with a free [jurl cloud](#jurl-cloud) account, or with your own [TypeSafe API key](https://console.typesafe.ai). It checks the key before saving it. That's the whole setup. Pages that need JavaScript just work: jurl fetches a headless browser the first time one shows up. `--vision` and `--find` always use your own Cloudflare Workers AI token (jurl cloud doesn't take them): add it with `jurl init`.

`jurl update` gets a newer jurl the same way you installed it. To uninstall: `brew uninstall jurl` or delete `~/.local/bin/jurl`, then `~/.config/jurl` (keys) and `~/Library/Caches/jurl` or `~/.cache/jurl` (the browser); run `jurl logout` first if you use jurl cloud.

## What it does

```console
$ jurl --precise --follow -q "What is the monthly price of the cheapest paid plan?" linear.app
$10 per user/month
…
jurl: found after reading 4 pages: linear.app/ → linear.app/pricing
```

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

- `jurl <url>`: the gist of a page, without menus, banners or footers.
- `-q "…"`: only what answers your question. Add `--precise` for just the answer, and `--follow` to look for it across the site.
- `--links`, `--code`, `--image`: the links worth following, the code blocks, the real images.
- `--find "…"`: the photo that shows what you describe (Clef looks at the pixels).

Everything prints plain text or plain URLs, so it fits in a pipe: `jurl -f "a bridge over a river" en.wikipedia.org/wiki/Cologne | xargs curl -sO`.

## jurl cloud

Don't want to manage API keys? `jurl login` signs this computer in to [jurl cloud](https://cloud.jurl.dev), and jurl's servers do the reading. Signing up is free, with a monthly allowance of page reads; paid plans have more. Same flags and exit codes, except the image modes (`--image`, `--vision`, `--find`) and a few others, which need your own keys.

```console
$ jurl login
To sign in to jurl cloud, open https://cloud.jurl.dev/device and enter this code:

    ABCD-EFGH
```

**Keep an answer up to date.** At a terminal, a jurl cloud answer ends with a link: `jurl: keep it up to date in a sheet: https://cloud.jurl.dev/…`. Open it and save the answer as a sheet: its first row is that answer with its proof. From there you add sites, questions and a schedule, and jurl cloud keeps it fresh. Scripts reading the output don't get the line.

`jurl status` shows the account and what the period has used; `jurl logout` signs out.

## Use it from an agent (MCP)

`jurl mcp` gives any MCP client (Claude Code, Claude Desktop, Cursor…) jurl as a set of web tools: the page's own words with their links, and a plain "Not found" when the page doesn't say it.

```sh
claude mcp add jurl -- jurl mcp
```

Other clients take `{"mcpServers": {"jurl": {"command": "jurl", "args": ["mcp"]}}}`. The tools: [docs/usage.md](docs/usage.md#use-it-from-an-agent-mcp).

## Reference

| Flag | |
| --- | --- |
| `-q, --ask "…"` | Keep what answers the question |
| `-p, --precise` | With `-q`: just the answer, its block and a link to it |
| `--follow [N]` | With `-q`: follow the site's links until it answers (5 pages, or N) |
| `-l, --links` · `-c, --code` · `-i, --image` | Links, code blocks or content images |
| `-f, --find "…"` | The image that best matches the description |
| `-n, --max N` · `-a, --all` | How many results, or all of them |
| `--json` | Machine-readable output, with every probability and the run's usage |
| `-t, --timing` | Where the time went, on stderr |

### Exit codes

As with grep, a script can tell "not there" from "something broke":

| Code | Meaning |
| --- | --- |
| 0 | Something was printed |
| 1 | The page (or the site, with `--follow`) was read and has nothing that answers |
| 2 | An error: the page couldn't be read, the API couldn't be asked, or the arguments are wrong |

## More

- [docs/usage.md](docs/usage.md): every mode with real output, recipes, MCP tools.
- [docs/reference.md](docs/reference.md): all flags, keys, settings, how jurl reaches pages that refuse it.
- [docs/cloud.md](docs/cloud.md): jurl cloud, your keys and your data.
- [docs/how-it-works.md](docs/how-it-works.md): speed, cost, how it compares, design notes.
- [bench/](bench): the measurements behind every number here.
- [CHANGELOG.md](CHANGELOG.md).

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; security problems go to [SECURITY.md](SECURITY.md).

## License

MIT or Apache-2.0, at your option. [Lightpanda](https://lightpanda.io), which jurl downloads for JavaScript pages, is a separate program under AGPL-3.0.
