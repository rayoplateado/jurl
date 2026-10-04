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
- **Small and dependency-light.** Prefer a few lines in the code that's already here over a new crate.

## Develop

```sh
cargo build
cargo test        # extraction: layout tables, lazy images, markdown, links, app-shell detection
cargo fmt         # CI checks formatting
cargo clippy --all-targets -- -D warnings
```

The tests don't need API keys. To try real pages you need a [TypeSafe API key](https://console.typesafe.ai) for Jev. `--vision` and `--find` also need a Cloudflare Workers AI token. Set them in `~/.config/jurl/env` with `jurl init`, or as environment variables (see the README).

A short map of the code is in the README, under *Development*.

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
