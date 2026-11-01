# jurl cloud

Don't want to manage API keys? `jurl login` signs this computer in to jurl cloud, and jurl's servers do the reading. Signing up is free: [cloud.jurl.dev](https://cloud.jurl.dev) has a free plan with a monthly allowance of page reads, and paid plans with more. The flags and the exit codes stay the same, and the text matches a local run's except for the page's kind line (`· docs (0.84)`), which the server doesn't send, so it isn't printed. A `--precise` answer's block prints with the kind, heading level and code language the server sends, as a local run prints it, and `--json` carries them too. A block whose kind the server doesn't send prints as plain text.

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

When the period's page reads run out, a read stops with exit code 2 and prints jurl cloud's message, which says how to go on (extra page reads or a bigger plan).

An answer to a question (`--precise`, or `-q`), or a miss, can be kept up to date: at a terminal, jurl cloud's answer ends with a link on stderr, `jurl: keep it up to date in a sheet: https://cloud.jurl.dev/…`. It opens a console page where the person who signed this computer in saves the answer as a sheet. The sheet's first row is that answer with its proof (or the miss), without reading the page again, and from there it can add sites, questions and a schedule. A script reading the output doesn't get the line.

`jurl status` says which account reads go to, and for jurl cloud, what the period has used. `jurl logout` signs this computer out and revokes its key on jurl cloud.

A run reads with jurl cloud or with your own keys, by the first of these that applies:

1. `JURL_CLOUD_KEY` in the environment: jurl cloud.
2. `TYPESAFE_API_KEY` or `JURL_JEV_URL` in the environment: your own keys. Set one for a single run to read with your own keys while you're signed in.
3. A sign-in saved by `jurl login`: jurl cloud.
4. Otherwise: your own keys.

jurl cloud takes the same reads as your own keys, except `--image`, `--vision`, `--find`, `--with-image`, `--with-link`, `--with-line`, `-r`, `--code` with `--precise`, and `--with-pages` (so `--pages-only` too). `-n` takes 1 to 50 and `--threshold` 0 to 1; `-a` keeps every result above the threshold. `--follow` takes 5, 10 or 15 pages. A read it doesn't take uses your own keys when they're set up, and says so on stderr; otherwise it stops and says why. With `--json`, `usage` counts the pages read for you, and its `jev` and `clef` counts stay at zero.

`JURL_CLOUD_URL` points jurl at another jurl cloud, such as a self-hosted one: `JURL_CLOUD_URL=http://127.0.0.1:3211 jurl login` saves it. A key only goes over https, or to this computer.

## Your keys, your data

With your own keys, jurl has no server and no account of its own. It talks to the model APIs directly with **your** keys:

- **Who you pay:** usage is billed by TypeSafe (Jev, $0.042 per million input tokens) and Cloudflare (Clef-flash, $0.09 per million). Output is free on both. Every `--json` result says what its run used, a `--precise` miss included: `"usage": {"pages": 3, "browser_retry": false, "plain_refusals": 0, "browser_requests": 0, "route": "direct", "bytes": {"direct": 420000, "proxy": 0, "proxy_wire": {"up": 0, "down": 0}, "proxy_render_wire": {"up": 0, "down": 0}}, "proxied_renders": 0, "proxied_hosts": [], "jev": {"requests": 6, "input_tokens": 41250, "capped": false}, "clef": {"requests": 0, "input_tokens": 0, "images": 0}}`. `pages` counts the pages read (with `--follow`, the whole search); `browser_retry` is true when one of them was read with the browser client (after a retry, or because its host needed it); `plain_refusals` and `browser_requests` count the requests sent with each client, a refusal being any non-success status; a request counts once it has answered. `bytes`, `proxied_renders` and `proxied_hosts` are described under the fallback proxy in [reference.md](reference.md#what-a-run-sent-and-where). `jev.capped` is true when `JURL_JEV_MAX_TOKENS` stopped the run's search (see [reference.md](reference.md#keys-and-settings)).
- **What leaves your machine:** the text of the page goes to TypeSafe. With `--vision` or `--find`, the images go to Cloudflare too. Keep that in mind for internal or private pages.
- **What jurl can't read:** pages behind a login. jurl keeps the cookies a site sets for the length of one run, in memory, and sends each one back only to the host and path that set it (RFC 6265). It never reads your browser's cookies and never saves any to disk, so a page that needs your login stays out of reach.

With [jurl cloud](#jurl-cloud), the URL and your question go to jurl cloud, which reads the page on its servers with the same models. It keeps a private log of each read for your organization, with the retention your organization sets. It also keeps an anonymous, de-identified record of reads of public pages (no account, user or key), used to improve jurl. Your organization can opt out of that record.

