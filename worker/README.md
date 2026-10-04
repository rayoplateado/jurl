# jurl.dev

One Cloudflare Worker, `jurl-dev`, serves everything on jurl.dev:

- **The landing**: `../site/`, static files, no build.
- **The playground**: `POST /api/try` runs the real jurl binary on a URL and returns its `--json` output. It runs in a Cloudflare Container (`container/`): the released Linux binary behind a small Python server that calls it with an argument list, never a shell.

| Path | What it does |
| --- | --- |
| `src/index.ts` | The Worker: serves `site/`, guards and runs `/api/try`, `/api/status`, `/api/warm` |
| `container/Dockerfile` | The jurl release the playground runs, pinned by version and SHA-256 |
| `container/server.py` | `POST /run {mode, url, q}` → `jurl --json -t …` |
| `wrangler.jsonc` | Bindings, container, limits and the daily budget |

## Deploys

Nothing to do by hand.

- **Site or Worker changes**: merging to `main` anything under `site/` or `worker/` runs [`site.yml`](../.github/workflows/site.yml), which typechecks and deploys. It never builds jurl.
- **New jurl release**: when the `Release` workflow finishes for a `v*` tag, [`playground-jurl.yml`](../.github/workflows/playground-jurl.yml) writes the new version and its SHA-256 into `container/Dockerfile`, commits that to `main` and deploys. Prereleases (`v1.2.3-rc.1`) are skipped.
- **Run an older or specific release**: Actions → *Playground jurl* → *Run workflow*, with the tag.
- **jurl itself** is only built by `release.yml` when a PR touches `src/`, `Cargo.toml`, `Cargo.lock` or `dist-workspace.toml`, and on release tags.

Both deploy workflows use the `CLOUDFLARE_API_TOKEN` repo secret (Workers and Containers edit).

## The playground

A try is checked in this order, cheapest first:

1. **URL**: public `http(s)` only; no IP addresses or local names. URLs over 2,000 characters are refused.
2. **Rate limit**: 10 tries a minute per IP.
3. **Turnstile** (invisible), the cache and DNS, in parallel. Anything resolving to a private, loopback or link-local address is refused.
4. **Cache**: the same mode, URL and question are served from KV for an hour, free.
5. **Budget**: the `Budget` Durable Object reserves an estimate, then settles it to the real cost parsed from jurl's `-t` line. The limits:
   - $2 a day overall;
   - 50 tries per IP a day;
   - 10 photo searches per IP a day.

   They're all `vars` in `wrangler.jsonc`.
6. **Container**: jurl runs with fixed result counts and a 20 s timeout. JavaScript rendering is off (`JURL_NO_DOWNLOAD=1`).

The page calls `/api/warm` when the box scrolls into view, so the container is awake by the time someone clicks.

### Secrets

Set on the Worker with `npx wrangler secret put NAME`, run from this folder:

| Secret | What it is |
| --- | --- |
| `TYPESAFE_API_KEY` | Jev. A key dedicated to the playground |
| `CLOUDFLARE_AI_TOKEN` | Clef. A token with Workers AI read only |
| `CLOUDFLARE_AI_ACCOUNT_ID` | The account that token belongs to |
| `TURNSTILE_SECRET` | The secret of the "jurl.dev playground" Turnstile widget |

**Kill switch**: `npx wrangler secret delete TURNSTILE_SECRET`. `/api/status` then reports the playground as off and the page hides the box. Put the secret back to turn it on.

### Cost

- **Models**: capped at $2 a day. A try costs between $0.0004 and $0.003.
- **Container**: billed only while awake, about $0.03 an hour on the `basic` instance. It sleeps after 5 minutes idle. The Workers Paid plan includes about 25 awake hours a month.

## Local development

```sh
npm install
npx wrangler dev --env-file /path/outside/the/repo/dev.env
```

`dev.env` holds the four secrets. For Turnstile, use the test secret `1x0000000000000000000000000000000AA`; on `localhost` the page uses the matching test site key. Docker must be running for the container.

wrangler's local egress proxy sometimes breaks the container's outbound HTTPS after it restarts. If tries fail with TLS errors locally, restart `wrangler dev`. Production isn't affected.
