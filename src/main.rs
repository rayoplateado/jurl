//! The entry point: check the flags, load the page (rendering it when it needs JavaScript), then run the
//! mode they ask for.

mod account;
mod answer;
mod blocks;
mod cli;
mod cloud;
mod config;
mod decide;
mod extract;
mod fallback;
mod fetch;
mod follow;
mod hosts;
mod judge;
mod lightpanda;
mod links;
mod mcp;
#[cfg(test)]
mod mock;
mod output;
mod precise;
mod reach;
mod setup;
mod site_search;
mod sitemaps;
mod stealth;
mod timing;
mod update;
mod vision;

use std::{
    io::{stderr, stdout},
    path::Path,
    process::ExitCode,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser};
use futures::future::join_all;
use reqwest::Client;

use crate::{
    cli::Args,
    config::Config,
    extract::{Extracted, Kind},
    judge::Ctx,
    output::{Rendered, exit_code, stdout_for, write_out},
    setup::{Access, NoArgs},
    timing::Timer,
};

/// One request's whole time: a page, an image or a model call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an unused connection is kept, so the ones `page` warms are still open when the calls come.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// A JS app with less text than this (in bytes, headings aside) is rendered rather than read as it is.
const APP_SHELL_TEXT: usize = 300;

#[tokio::main]
async fn main() -> ExitCode {
    // `jurl` with no arguments at all is the setup, or the help. Anything else goes to clap, as it always has.
    if std::env::args_os().nth(1).is_none() {
        return match no_args().await {
            Ok(code) => code,
            Err(e) => failed(&e),
        };
    }
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            let _ = e.print();
            // A flag this jurl doesn't know may be one a newer jurl does.
            if e.kind() == clap::error::ErrorKind::UnknownArgument
                && let Some(hint) = update::hint().await
            {
                eprintln!("\n{hint}");
            }
            return ExitCode::from(e.exit_code() as u8);
        }
    };
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => failed(&e),
    }
}

/// Says what went wrong, and returns the exit code for it.
fn failed(e: &anyhow::Error) -> ExitCode {
    eprintln!("jurl: {e:#}");
    ExitCode::from(exit_code(e))
}

/// `jurl` with no arguments. With nothing set up and a terminal to ask in: a welcome, the setup `jurl init` does, and
/// what to try next. Otherwise the short help as a usage error (exit 2), and a line about `jurl status` if anything is
/// set up.
async fn no_args() -> Result<ExitCode> {
    let mut cfg = Config::load();
    let client = http_client()?;
    match setup::no_args_action(setup::is_set_up(&cfg)?, setup::interactive()) {
        NoArgs::Setup => {
            setup::first_run(&mut cfg, &client).await?;
            Ok(ExitCode::SUCCESS)
        }
        NoArgs::Help { status_hint } => {
            Args::command().write_help(&mut stderr())?;
            if status_hint {
                eprintln!("\nRun `jurl status` to see which account reads go to.");
            }
            Ok(ExitCode::from(2))
        }
    }
}

/// The largest header list a response may have over HTTP/2, which the client advertises. hyper's default is 16 KB, and some
/// pages send more: backmarket's /es-es sends 31 KB, mostly a Link of preloads, and the stream was reset as a protocol
/// error before the page was read. 256 KB is what Chrome advertises, so the browser client (see `fetch.rs`) matches it.
const HTTP2_MAX_HEADER_LIST: u32 = 256 * 1024;

/// What every jurl client starts from: a browser's user agent, with jurl's version on the end, the Accept-Language a run
/// names, the time limits, and the HTTP/2 header limit above.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(concat!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) jurl/",
            env!("CARGO_PKG_VERSION")
        ))
        .default_headers(fetch::language_headers())
        .timeout(HTTP_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .http2_max_header_list_size(HTTP2_MAX_HEADER_LIST)
}

/// The client for the APIs (Jev, Clef, jurl cloud, the sign-in) and the requests before a run's pages. It is never guarded:
/// those servers are the ones jurl is configured to use, and they may be on a private network. Pages go through the run's
/// reach instead (see `reach.rs`).
pub(crate) fn http_client() -> Result<Client> {
    Ok(client_builder().build()?)
}

async fn run(mut args: Args) -> Result<()> {
    let mut cfg = Config::load();
    let client = http_client()?;
    if args.url == "update" {
        return update::run().await;
    }
    if args.url == "init" {
        return setup::init(&mut cfg, &client).await;
    }
    if args.url == "login" {
        return account::login(&mut cfg, &client).await;
    }
    if args.url == "logout" {
        return account::logout(&mut cfg, &client).await;
    }
    if args.url == "status" {
        return account::status(&cfg, &client).await;
    }
    if args.url == "mcp" {
        return mcp::serve(client).await;
    }
    prepare(&mut args)?;
    reach::set_reach(&mut args).await?;
    args.stealth =
        stealth::Sidecar::configured(|key| cfg.get(key), args.public_only, args.render_sandboxed, args.timing);
    args.fallback = fallback::Fallback::configured(|key| cfg.get(key))?;
    let access = setup::access(&mut cfg, &client).await?;

    let mut t = Timer::new();
    let done = page(&args, &cfg, &client, &access, &mut t).await;
    // A miss costs tokens too: -t reports them either way.
    if args.timing {
        t.report();
    }
    if let Some(text) = stdout_for(&args, &done, &decide::USAGE)? {
        write_out(&mut stdout().lock(), &text)?;
    }
    done.map(|_| ())
}

/// The URL and flags made whole and checked: `--find` is a question for `--vision`, a bare domain gets https://.
pub(crate) fn prepare(args: &mut Args) -> Result<()> {
    if !args.url.contains("://") {
        args.url = format!("https://{}", args.url);
    }
    if let Some(what) = args.find.take() {
        if args.ask.is_some() {
            bail!("--find already is the question; drop --ask");
        }
        args.vision = true;
        args.ask = Some(what);
    }
    let modes = [args.image || args.vision, args.links, args.code].iter().filter(|m| **m).count();
    if modes > 1 {
        bail!("pick one of --image/--vision, --links, --code");
    }
    if args.precise && (args.image || args.vision || args.links) {
        bail!("--precise picks part of a block: use it with -q or --code -q, not images or links");
    }
    if args.precise && args.ask.is_none() {
        bail!("--precise needs a question: add -q \"…\"");
    }
    if args.follow.is_some() && (args.ask.is_none() || args.image || args.vision || args.links) {
        bail!("--follow looks for an answer: use it with -q (and --precise or --code), not images or links");
    }
    Ok(())
}

/// The page (or with --follow, the site) read in the mode `args` asks for: what the CLI prints and what `jurl mcp`
/// answers. Through jurl cloud when the run reads with it; a read the cloud doesn't take goes to the own keys, when
/// they are set up, and otherwise says why it can't be done.
pub(crate) async fn page(
    args: &Args,
    cfg: &Config,
    client: &Client,
    access: &Access,
    t: &mut Timer,
) -> Result<Rendered> {
    match access {
        Access::Cloud(account) => match cloud::unsupported(args) {
            None => cloud::read(client, account, args, t).await,
            Some(why) => {
                let key = setup::saved_key(cfg)
                    .map_err(|_| anyhow!("{why}. For that, set up your own keys with `jurl init`"))?;
                eprintln!("jurl: {why}, so this read uses your own keys");
                own_page(args, cfg, client, &key, t).await
            }
        },
        Access::Own(key) => own_page(args, cfg, client, key, t).await,
    }
}

/// The page read with the own keys: fetched (rendered when it needs JavaScript), then read in the mode `args` asks for.
async fn own_page(args: &Args, cfg: &Config, client: &Client, key: &str, t: &mut Timer) -> Result<Rendered> {
    // Warm the API connections (TLS handshakes) while the page downloads.
    // Jev's host, or the one `JURL_JEV_URL` names.
    let mut hosts: Vec<String> = url::Url::parse(&decide::jev_url())
        .map(|u| format!("{}/", u.origin().ascii_serialization()))
        .into_iter()
        .collect();
    if args.vision {
        hosts.push("https://api.cloudflare.com/".to_string());
    }
    let warm = tokio::spawn({
        let c = client.clone();
        async move { join_all(hosts.into_iter().map(|h| c.head(h).send())).await }
    });
    let target: url::Url = args.url.parse().with_context(|| format!("bad url {}", args.url))?;
    read(args, cfg, client, key, target, warm, t).await
}

/// The page (or with --follow, the site) read in the mode asked for.
async fn read(
    args: &Args,
    cfg: &Config,
    client: &Client,
    key: &str,
    target: url::Url,
    warm: tokio::task::JoinHandle<impl Sized>,
    t: &mut Timer,
) -> Result<Rendered> {
    if args.follow.is_some() {
        let _ = warm.await;
        return follow::run(args, cfg, client, key, target, t).await;
    }
    let (url, ex, served) = load(args, cfg, &target, t).await?;
    decide::USAGE.record_route(served);
    let _ = warm.await;

    let ctx = Ctx::new(args, client, key, &url, &ex);
    if args.image || args.vision {
        vision::images(&ctx, cfg, &ex, t).await
    } else if args.links {
        links::links(&ctx, &ex, t).await
    } else {
        blocks::blocks(&ctx, &ex, t).await
    }
}

/// Fetch a page (rendering it when it needs JavaScript) and cut it into blocks, links and images. Also says how the page was
/// served, for the usage's `route`.
pub(crate) async fn load(
    args: &Args,
    cfg: &Config,
    target: &url::Url,
    t: &mut Timer,
) -> Result<(url::Url, Extracted, fetch::Served)> {
    // A host switching to the browser client is said on stderr under -t, once, where the switch happens.
    let retry = fetch::Retry::for_run(
        args.no_browser_retry,
        args.timing,
        &args.reach,
        &args.cookies,
        args.stealth.as_ref(),
        &args.fallback,
    );
    let (page, served) = if args.render {
        fetch::render_allowed(target, &args.reach, args.public_only, args.render_sandboxed).await?;
        let bin = lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await?;
        let rendered = render_or_refused(&bin, target, retry).await?;
        t.lap("render");
        rendered
    } else {
        let page = fetch::fetch(target.as_str(), retry).await?;
        t.lap("fetch");
        if page.via_browser() {
            decide::USAGE.browser_retry.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let served = fetch::Served { route: page.route, rendered: false };
        (page, served)
    };
    decide::USAGE.pages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (page, served, mut ex) = read_page(args, cfg, retry, page, served, t).await?;
    // --precise may answer with the page's JSON-LD values (see `extract::json_ld`), read from the HTML the page was read as.
    if args.precise && !page.is_markdown {
        extract::json_ld::add_to(&mut ex, &page.body);
    }
    Ok((page.url, ex, served))
}

/// Whether a page is rendered with Lightpanda after it is served: not when it is rendered already, nor when the stealth sidecar
/// served it (its HTML is a browser's DOM after the scripts ran, and a render would ask again the host that refused the page).
/// Otherwise a page is rendered when the run asks for it (`--render`), or when it is an app shell or has unfilled placeholders.
fn renders_after_fetch(render: bool, served: fetch::Served, shell: bool, placeholders: bool) -> bool {
    !served.rendered && served.route != fetch::Route::Stealth && (render || shell || placeholders)
}

/// The page's text cut into blocks, and whether its template placeholders are unfilled (see `extract::html_with_placeholders`).
/// Only HTML can carry placeholders: a markdown page has no script to have left them, and a rendered page has had its scripts
/// run already.
fn extract_page(page: &fetch::Page, rendered: bool, t: &mut Timer) -> (Extracted, bool) {
    let read = if page.is_markdown {
        (extract::markdown(&page.body, &page.url), false)
    } else if rendered {
        (extract::html(&page.body, &page.url), false)
    } else {
        extract::html_with_placeholders(&page.body, &page.url)
    };
    t.lap(if page.is_markdown { "extract(md)" } else { "extract" });
    read
}

/// Whether the text of a page is an app shell with (almost) no text: it has nothing to read without its render.
fn is_shell(ex: &Extracted) -> bool {
    let text: usize = ex.blocks.iter().filter(|b| b.kind != Kind::Heading).map(|b| b.text.len()).sum();
    ex.app_shell && text < APP_SHELL_TEXT
}

/// Lightpanda's render of `url`. A render that the host refuses with 401, 403 or 429 climbs the rungs a refused fetch climbs
/// (see `fetch::refused_render`): what comes back is the page the proxy gave, not rendered, or the sidecar's page. Any other
/// error is the render's own.
async fn render_or_refused(
    bin: &Path,
    url: &url::Url,
    retry: fetch::Retry<'_>,
) -> Result<(fetch::Page, fetch::Served)> {
    let (page, rendered) = match fetch::render(bin, url, retry.fallback).await {
        Ok(page) => (page, true),
        Err(e) => match e.downcast::<fetch::RenderRefused>() {
            Ok(refused) if fallback::refused_enough(refused.status) => {
                (fetch::refused_render(url, refused.status, refused.retry_after, retry).await?, false)
            }
            Ok(refused) => return Err(refused.into()),
            Err(e) => return Err(e),
        },
    };
    let served = fetch::Served { route: page.route, rendered };
    Ok((page, served))
}

/// The render `read_page` asks for: the page as Lightpanda renders it, or as the rungs after a refused render give it (see
/// `render_or_refused`). None when the page is read as it is. Without the run's permission to render, or without Lightpanda, the
/// failure is said on stderr and the page is read as it is, unless the failure is the answer: the page is an app shell, which has
/// no text without its render, or the run asked for the render (`--render`).
async fn render_page(
    args: &Args,
    cfg: &Config,
    retry: fetch::Retry<'_>,
    page: &fetch::Page,
    shell: bool,
) -> Result<Option<(fetch::Page, fetch::Served)>> {
    let answer = shell || args.render;
    if let Err(e) = fetch::render_allowed(&page.url, &args.reach, args.public_only, args.render_sandboxed).await {
        if answer {
            return Err(e);
        }
        eprintln!("jurl: could not render the page, reading it as it is: {e:#}");
        return Ok(None);
    }
    let bin = match lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await {
        Ok(bin) => bin,
        Err(e) if args.render => return Err(e),
        Err(e) => {
            eprintln!("jurl: {e:#}");
            return Ok(None);
        }
    };
    if !args.render {
        let why = if shell { "no text without JavaScript" } else { "unfilled template placeholders in the text" };
        eprintln!("jurl: {why}, rendering with Lightpanda…");
    }
    // A page the proxy gave is rendered through the proxy, so its host has to be on it: a redirect of its request may have
    // taken it to a host that is not yet (see `fetch::send_get`). Then a refusal of that render goes to the sidecar, not back
    // to the proxy.
    if page.route == fetch::Route::Proxy {
        retry.fallback.put_on_proxy(&page.url, "its page came through the proxy", retry.timing);
    }
    match render_or_refused(&bin, &page.url, retry).await {
        Ok(rendered) => Ok(Some(rendered)),
        Err(e) if answer => Err(e),
        Err(e) => {
            eprintln!("jurl: could not render the page, reading it as it is: {e:#}");
            Ok(None)
        }
    }
}

/// Extracts `page` as `load` reads it, and renders it when it needs JavaScript (see `renders_after_fetch`). The page that a
/// refused render gives is read the same way, and rendered once more at most: a refusal of that render goes to the sidecar.
/// Returns the page read, how it was served, and its text.
async fn read_page(
    args: &Args,
    cfg: &Config,
    retry: fetch::Retry<'_>,
    mut page: fetch::Page,
    mut served: fetch::Served,
    t: &mut Timer,
) -> Result<(fetch::Page, fetch::Served, Extracted)> {
    let mut renders = 0;
    loop {
        let (ex, placeholders) = extract_page(&page, served.rendered, t);
        let shell = is_shell(&ex);
        if renders == 2 || !renders_after_fetch(args.render, served, shell, placeholders) {
            return Ok((page, served, ex));
        }
        renders += 1;
        let Some((next, next_served)) = render_page(args, cfg, retry, &page, shell).await? else {
            return Ok((page, served, ex));
        };
        t.lap("render");
        (page, served) = (next, next_served);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_the_stealth_sidecar_served_is_not_rendered_again() {
        use fetch::{Route, Served};
        let served = |route, rendered| Served { route, rendered };
        assert!(renders_after_fetch(false, served(Route::Direct, false), true, false));
        assert!(renders_after_fetch(false, served(Route::Browser, false), false, true));
        assert!(
            !renders_after_fetch(false, served(Route::Stealth, false), true, true),
            "the sidecar's DOM is already rendered"
        );
        assert!(
            !renders_after_fetch(true, served(Route::Stealth, false), true, true),
            "--render does not render it either"
        );
        assert!(!renders_after_fetch(true, served(Route::Direct, true), true, true), "--render rendered it already");
        assert!(
            renders_after_fetch(true, served(Route::Proxy, false), false, false),
            "--render renders the proxy's page"
        );
        assert!(!renders_after_fetch(false, served(Route::Direct, false), false, false));
    }
}
