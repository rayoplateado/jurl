# Contributing to jurl

Thanks for helping. jurl is small on purpose, so the best contributions are small too: one fix or one improvement per pull request.

## Before you start

- **Bugs**: open an issue with the command, the URL and what you expected. Add the `-t` line, and the `--json` output if it's short.
- **New features or modes**: open an issue first and describe what you'd use it for. A short discussion saves a pull request that can't be merged.
- **Security problems**: don't open an issue. See [SECURITY.md](SECURITY.md).

## The rules jurl lives by

Any change has to keep these true:

- **Models choose, they don't write.** Every block, link and image jurl prints comes verbatim from the page. No summaries, no rewording, no generated text in the output.
- **Output is pipe-friendly.** Results go to stdout as plain text or plain URLs. Progress, warnings and timings go to stderr.
- **Numbers are measured, not guessed.** If a change affects speed or cost, include before and after `-t` lines on real pages.
- **Small and dependency-light.** Prefer a few lines in the code that's already here over a new crate. Where a crate changes what jurl can do, its cost goes in the PR: the browser-fingerprint retry links BoringSSL, about 3.6 MB on the release binary, and that was accepted.

## Develop

```sh
cargo build
cargo test        # extraction: layout tables, lazy images, markdown, links, app-shell detection
cargo fmt         # CI checks formatting
cargo clippy --all-targets -- -D warnings
cargo build --release && ./target/release/jurl -t <url>   # try a real page
```

The build needs Rust 1.98 or newer, and cmake and libclang: jurl links BoringSSL (for the retry with a browser's TLS fingerprint), which is built from source. On macOS, `brew install cmake` and the Xcode Command Line Tools are enough, unless the BoringSSL build fails on `__builtin_clzg`: then the compiler is older than the SDK, and building with Homebrew's LLVM works (`CC=$(brew --prefix llvm)/bin/clang CXX=$(brew --prefix llvm)/bin/clang++ cargo build`). On Debian or Ubuntu: `sudo apt install cmake libclang-dev`. On Windows: NASM and LLVM.

The tests don't need API keys. To try real pages you need a [TypeSafe API key](https://console.typesafe.ai) for Jev. `--vision` and `--find` also need a Cloudflare Workers AI token. Set them in `~/.config/jurl/env` with `jurl init`, or as environment variables (see [docs/reference.md](docs/reference.md#keys-and-settings)).

### Code map

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
| `src/feeds.rs` | A feed (RSS, Atom) → the page of its entries, one list item each with its link, date, summary and image |
| `src/decide.rs` | Jev and Clef clients |
| `src/fetch.rs` · `src/lightpanda.rs` | Fetching (the retry on a browser's fingerprint, the run's cookies), rendering, the browser download |
| `src/fallback.rs` | Rung 3: the fallback proxy, per run, and what a run sends through it |
| `src/meter.rs` | The loopback forwarder that counts, on the wire, the bytes a run sends through the proxy |
| `src/setup.rs` · `src/config.rs` | First-run setup (own keys or jurl cloud), `jurl init`, key storage |
| `src/cloud.rs` | jurl cloud: which account a run reads with, the read, the usage |
| `src/account.rs` | `jurl login`, `logout` and `status`: the sign-in in the browser, and signing out |
| `src/mock.rs` | Tests only: a local server that answers as jurl cloud does |
| `src/update.rs` | `jurl update` |
| `src/mcp.rs` | `jurl mcp`: the tools, their schemas, JSON-RPC over stdio |

Releases are built by [cargo-dist](https://opensource.axo.dev/cargo-dist/) when a `v*` tag is pushed.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Keep the change focused. Add or update tests when you touch extraction or ranking.
3. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
4. In the description, say what changed and why. Paste real output for anything user-visible.

CI runs rustfmt, clippy and the tests on every pull request; all three must pass to merge. The first time you contribute, a maintainer approves the CI run before it starts. Pull requests are squash-merged.

Releases, tags and the Homebrew tap are handled by the maintainer.

## License

jurl is licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in jurl by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
