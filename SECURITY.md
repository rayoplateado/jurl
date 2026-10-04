# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue:

- **Preferred**: use GitHub's private reporting. On the [Security tab](https://github.com/rayoplateado/jurl/security), select *Report a vulnerability*.
- **Or** email ray@rayoplateado.com.

Include what you found, how to reproduce it and what an attacker could do with it. You'll get an answer within a few days. Once there's a fix, you'll be credited in the release notes unless you'd rather not be.

## Supported versions

Only the latest release gets security fixes.

## What's in scope

jurl handles API keys and runs a downloaded browser, so these matter most:

- **API keys**: how jurl reads, stores (`~/.config/jurl/env`, mode `0600`) and sends your TypeSafe and Cloudflare keys.
- **The Lightpanda download**: jurl fetches a pinned Lightpanda release and checks its SHA-256 before running it.
- **Hostile pages**: a page jurl fetches making it crash, hang, use unbounded memory, or write anything outside its cache.
- **Output**: anything that makes jurl print text that isn't on the page.

Vulnerabilities in TypeSafe, Cloudflare or Lightpanda themselves should go to those projects.
