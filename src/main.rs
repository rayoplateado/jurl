//! The entry point: check the flags, load the page (rendering it when it needs JavaScript), then run the
//! mode they ask for.

mod account;
mod answer;
mod blocks;
mod cli;
mod cloud;
mod config;
mod country;
mod decide;
mod extract;
mod fallback;
mod fetch;
mod follow;
mod hosts;
mod judge;
mod langtag;
mod lightpanda;
mod links;
mod mcp;
mod meter;
#[cfg(test)]
mod mock;
mod output;
mod paging;
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
    collections::{HashMap, HashSet},
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
    args.country = country::configured(|key| cfg.get(key), args.timing);
    args.stealth =
        stealth::Sidecar::configured(|key| cfg.get(key), args.public_only, args.render_sandboxed, args.timing);
    args.fallback = fallback::Fallback::configured(|key| cfg.get(key))?;
    args.jev_budget = follow::jev_budget(|key| cfg.get(key), args.timing);
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
    if args.pages == 0 || args.pages > paging::MAX_PAGES {
        bail!("--pages takes 1 to {}", paging::MAX_PAGES);
    }
    if args.pages > 1 && (args.follow.is_some() || args.links || args.image || args.vision) {
        bail!(
            "--pages reads the next pages of a list: use it with -q or a plain read, not --follow, --links or --image"
        );
    }
    if args.follow.is_some() && (args.ask.is_none() || args.image || args.vision || args.links) {
        bail!("--follow looks for an answer: use it with -q (and --precise or --code), not images or links");
    }
    if args.with_image.is_some() && (args.ask.is_none() || args.image || args.vision || args.links) {
        bail!("--with-image adds the image of the answer's page: use it with -q, not --image, --find or --links");
    }
    if matches!(&args.with_image, Some(Some(what)) if what.trim().is_empty()) {
        bail!("--with-image=WHAT needs what to find; for the page's best image, drop the =");
    }
    if args.with_image.is_some() && args.pages > 1 {
        bail!("--with-image looks at one page, the answer's: not with --pages");
    }
    if args.with_link.is_some() && (args.ask.is_none() || args.image || args.vision || args.links) {
        bail!("--with-link adds a link of the answer's page: use it with -q, not --image, --find or --links");
    }
    if args.with_link.as_deref().is_some_and(|what| what.trim().is_empty()) {
        bail!("--with-link=WHAT needs what the link is for, such as \"buy tickets\"");
    }
    if args.with_link.is_some() && args.pages > 1 {
        bail!("--with-link looks at one page, the answer's: not with --pages");
    }
    if args.with_pages && !args.json {
        bail!("--with-pages puts the pages in the JSON: add --json");
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
    // `--with-image=WHAT` needs Clef's keys: missing ones fail the run before its pages are read, not after.
    let clef_later = matches!(args.with_image, Some(Some(_)));
    if clef_later {
        vision::check_keys(cfg)?;
    }
    if args.vision || clef_later {
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
    // The start's page, or its `www.` alternate when the bare domain gives no answer (see `fetch::start_with_alternate`).
    let (url, ex, served) =
        fetch::start_with_alternate(&target, args.timing, async |u| load_start(args, cfg, u, t).await).await?;
    decide::USAGE.record_route(served);
    let _ = warm.await;

    let ex = if args.pages > 1 { list_pages(args, cfg, url.clone(), ex, t).await } else { ex };
    let ctx = Ctx::new(args, client, key, &url, &ex);
    if args.image || args.vision {
        vision::images(&ctx, cfg, &ex, t).await
    } else if args.links {
        links::links(&ctx, &ex, t).await
    } else {
        let answer = blocks::blocks(&ctx, &ex, t).await?;
        let answer = vision::with_image(&ctx, cfg, &ex, answer, t).await?;
        links::with_link(&ctx, &ex, answer, t).await
    }
}

/// `--pages`: the start page, and the pages after it one after another, each the next page of the one before (see `paging`),
/// as one list (`paging::merge`). A page that the site's robots.txt does not let jurl read ends the list, as does a page
/// already read or one with no text that is new (under a --recipe, no row that is new). A page that fails ends it too, with a
/// note on stderr: the pages before it are judged as they are. A page that links no next page may name it in its data (see
/// `extract::data_next`).
async fn list_pages(args: &Args, cfg: &Config, start: url::Url, ex: Extracted, t: &mut Timer) -> Extracted {
    let retry = fetch::Retry::for_run(
        args.no_browser_retry,
        args.timing,
        &args.reach,
        &args.cookies,
        args.stealth.as_ref(),
        &args.fallback,
    );
    let mut robots: HashMap<String, follow::Robots> = HashMap::new();
    // Under a --recipe a page is new by its rows, and its next page may be guessed (see `paging::guessed_next`), since many
    // lists' pages are only reached by their scripts: a guess that gives no row, or only rows already read, ends the list.
    let novelty = |url: &url::Url, ex: &Extracted| -> Vec<String> {
        if args.recipe.is_some() {
            args.pages_log.rows_of(url)
        } else {
            ex.blocks.iter().map(|b| b.text.clone()).collect()
        }
    };
    let mut seen: HashSet<String> = novelty(&start, &ex).into_iter().collect();
    let mut pages = vec![(start, ex)];
    // Whether the last page read is a batch its list's data named next (see `extract::data_next`).
    let mut last_batch = false;
    'pages: while pages.len() < args.pages {
        let Some((last, last_ex)) = pages.last() else { break };
        // The page the last one links next; else the pages its data names next, nearest its list first, each of which must hold
        // the list's things (a data link can name another feed's next page); else, under a recipe, a guess.
        let (candidates, keys): (Vec<url::Url>, Option<&[String]>) = match (&last_ex.next_page, &last_ex.data_next) {
            (Some(next), _) => (vec![next.clone()], None),
            (None, Some(data)) => (data.urls.clone(), Some(data.keys.as_slice())),
            // A batch that its data names no next after is the list's last: its address is the data's, not a page's to guess at.
            (None, None) if last_batch => break,
            (None, None) => {
                let guessed =
                    args.recipe.as_ref().filter(|_| !seen.is_empty()).and_then(|_| paging::guessed_next(last));
                (guessed.into_iter().collect(), None)
            }
        };
        let keys = keys.map(<[String]>::to_vec);
        for next in candidates {
            // A page already read ends the list, unless the data named it: its next link may be another one.
            if pages.iter().any(|(u, _)| *u == next) {
                if keys.is_some() {
                    continue;
                }
                break 'pages;
            }
            let host = next.host_str().unwrap_or_default().to_string();
            if !robots.contains_key(&host) {
                robots.insert(host.clone(), follow::Robots::load(&next, retry).await);
            }
            if !robots[&host].allows(&next) {
                eprintln!("jurl: {next} is not read: its robots.txt does not let jurl read it");
                break 'pages;
            }
            let (url, next_ex) = match load(args, cfg, &next, t).await {
                Ok((url, next_ex, _)) => (url, next_ex),
                // A page the data named that fails leaves its other links to try.
                Err(e) if keys.is_some() => {
                    if args.timing {
                        eprintln!("jurl: {next}, which the page's data names next, is not read: {e:#}");
                    }
                    continue;
                }
                Err(e) => {
                    eprintln!("jurl: {next} is not read, so the list is {} pages: {e:#}", pages.len());
                    break 'pages;
                }
            };
            if let Some(keys) = &keys
                && !extract::data_next::holds_like(&next_ex.data_shapes, keys)
            {
                if args.timing {
                    eprintln!("jurl: {url}, which the page's data names next, holds none of its list's things");
                }
                continue;
            }
            let new = novelty(&url, &next_ex);
            if new.iter().all(|n| seen.contains(n)) {
                if args.recipe.is_some() && args.timing {
                    eprintln!(
                        "jurl: {url} gives no row the pages before it didn't, so the list is {} pages",
                        pages.len()
                    );
                }
                break 'pages;
            }
            seen.extend(new);
            last_batch = keys.is_some();
            pages.push((url, next_ex));
            continue 'pages;
        }
        break;
    }
    // Every page given was read: the page after the last one, when the page itself links it or its data names it (its first such link),
    // is where the list goes on, for a later read to start at. A guessed page is not: nothing shows the list goes on (the pages read after
    // a guess are checked for new rows, and a later read starting there could not be). Only `--json` prints it.
    if args.json
        && pages.len() >= args.pages
        && let Some(next) = pages.last().and_then(|(_, last_ex)| {
            last_ex.next_page.clone().or_else(|| last_ex.data_next.as_ref().and_then(|d| d.urls.first().cloned()))
        })
        && !pages.iter().any(|(u, _)| *u == next)
    {
        let host = next.host_str().unwrap_or_default().to_string();
        if !robots.contains_key(&host) {
            robots.insert(host.clone(), follow::Robots::load(&next, retry).await);
        }
        if robots[&host].allows(&next) {
            args.list_next.set(next);
        }
    }
    paging::merge(pages)
}

/// Fetch a page (rendering it when it needs JavaScript) and cut it into blocks, links and images. Also says how the page was
/// served, for the usage's `route`.
pub(crate) async fn load(
    args: &Args,
    cfg: &Config,
    target: &url::Url,
    t: &mut Timer,
) -> Result<(url::Url, Extracted, fetch::Served)> {
    load_page(args, cfg, target, false, t).await
}

/// [`load`] for the run's start page: when the site sent its direct read to a page of another country, the start page is read
/// again through the fallback proxy, and the run goes on from that read (see `country::reread`).
pub(crate) async fn load_start(
    args: &Args,
    cfg: &Config,
    target: &url::Url,
    t: &mut Timer,
) -> Result<(url::Url, Extracted, fetch::Served)> {
    load_page(args, cfg, target, true, t).await
}

async fn load_page(
    args: &Args,
    cfg: &Config,
    target: &url::Url,
    start: bool,
    t: &mut Timer,
) -> Result<(url::Url, Extracted, fetch::Served)> {
    // Numbered before anything waits, so the loads a `--follow` batch starts together are listed in the order they start.
    let number = args.pages_log.start();
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
        let page = if start { country::reread(args, retry, target, page, t).await } else { page };
        let served = fetch::Served { route: page.route, rendered: false };
        (page, served)
    };
    decide::USAGE.pages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (page, served, mut ex) = read_page(args, cfg, retry, page, served, t).await?;
    // --precise may answer with the page's JSON-LD values (see `extract::json_ld`), read from the HTML the page was read as.
    if args.precise && !page.is_markdown {
        extract::json_ld::add_to(&mut ex, &page.body);
    }
    // `--with-pages` lists the page as the run reads it: the blocks a -q read is given, and apart from them the things its
    // JSON-LD lists (see `extract::json_ld::items`), so a list read can take its rows from the page's own data, and under
    // --with-sources and --recipe the lists its data holds and the rows a recipe takes from them (see `extract::sources`).
    // A page that is JSON (a batch a data link names) is its own data; a list read over pages takes the page after it from its
    // data when it links none (see `extract::data_next`).
    let document = extract::data_next::document(&page.body, &page.url);
    let is_document = document.is_some();
    // A list read over pages looks into the data of a page that links no next page (a page that links one is read on from there).
    let pages_by_data = args.pages > 1 && ex.next_page.is_none();
    let reads_data = pages_by_data || (args.with_pages && (args.with_sources || args.recipe.is_some()));
    let own = match document {
        Some(own) => Some(own),
        None if reads_data && !page.is_markdown => Some(extract::sources::data_of(&page.body)),
        None => None,
    };
    if pages_by_data && let Some(own) = &own {
        ex.data_next = extract::data_next::next_of(own, &page.url, is_document);
        ex.data_shapes = extract::data_next::shapes_of(own);
    }
    if args.with_pages {
        let mut data = output::PageData::default();
        if let Some(own) = own.as_ref().filter(|_| is_document) {
            data.items = extract::data_next::items(own, &page.url, 0);
        } else if !page.is_markdown {
            data.items = extract::json_ld::items(&page.body, 0);
        }
        if let Some(own) = own.as_ref().filter(|_| args.with_sources || args.recipe.is_some()) {
            if args.with_sources {
                data.sources = extract::sources::lists(own);
            }
            data.recipe = args.recipe.as_ref().map(|r| match extract::sources::apply(own, r) {
                extract::sources::Applied::RecipeError(_) if is_document => extract::data_next::apply_by_shape(own, r),
                applied => applied,
            });
        }
        args.pages_log.record(number, &page.url, served, &ex.blocks, data);
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

    #[test]
    fn pages_is_a_count_from_one_to_the_most_allowed_and_not_for_a_follow_or_a_link() {
        let check = |argv: &[&str]| prepare(&mut Args::parse_from(argv));
        assert!(check(&["jurl", "-q", "x?", "--pages", "13", "x.com"]).is_ok());
        assert!(check(&["jurl", "--pages", "1", "x.com"]).is_ok(), "one page is the default");
        assert!(check(&["jurl", "-q", "x?", "--pages", "0", "x.com"]).is_err());
        let too_many = paging::MAX_PAGES + 1;
        assert!(check(&["jurl", "-q", "x?", "--pages", &too_many.to_string(), "x.com"]).is_err());
        assert!(check(&["jurl", "-q", "x?", "--pages", "2", "--follow", "5", "x.com"]).is_err());
        assert!(check(&["jurl", "-q", "x?", "--pages", "2", "--links", "x.com"]).is_err());
        assert!(check(&["jurl", "--pages", "2", "--image", "x.com"]).is_err());
        assert!(
            check(&["jurl", "-q", "x?", "--pages", "2", "--precise", "x.com"]).is_ok(),
            "a precise answer may be on any page"
        );
    }

    #[test]
    fn with_image_goes_with_an_answer_on_one_page_or_a_follow() {
        let check = |argv: &[&str]| prepare(&mut Args::parse_from(argv));
        assert!(check(&["jurl", "-p", "-q", "x?", "--with-image", "x.com"]).is_ok());
        assert!(check(&["jurl", "-p", "-q", "x?", "--follow", "5", "--with-image=a facade", "x.com"]).is_ok());
        assert!(check(&["jurl", "-q", "x?", "--with-image", "x.com"]).is_ok(), "blocks answer too");
        assert!(check(&["jurl", "--with-image", "x.com"]).is_err(), "no question, no answer to go with");
        assert!(check(&["jurl", "-q", "x?", "--image", "--with-image", "x.com"]).is_err());
        assert!(check(&["jurl", "--find", "a cat", "--with-image", "x.com"]).is_err());
        assert!(check(&["jurl", "-q", "x?", "--links", "--with-image", "x.com"]).is_err());
        assert!(
            check(&["jurl", "-q", "x?", "--pages", "2", "--with-image", "x.com"]).is_err(),
            "one page: the answer's"
        );
    }

    #[test]
    fn with_image_takes_what_to_find_only_after_an_equals_sign() {
        // Without `=`, the next word is the URL, not what to find.
        let bare = Args::parse_from(["jurl", "-q", "x?", "--with-image", "x.com"]);
        assert_eq!((bare.with_image, bare.url.as_str()), (Some(None), "x.com"));
        let find = Args::parse_from(["jurl", "-q", "x?", "--with-image=a facade", "x.com"]);
        assert_eq!(find.with_image, Some(Some("a facade".to_string())));
        assert_eq!(Args::parse_from(["jurl", "-q", "x?", "x.com"]).with_image, None);
    }

    #[test]
    fn with_link_goes_with_an_answer_on_one_page_or_a_follow() {
        let check = |argv: &[&str]| prepare(&mut Args::parse_from(argv));
        assert!(check(&["jurl", "-p", "-q", "x?", "--with-link=buy tickets", "x.com"]).is_ok());
        assert!(check(&["jurl", "-q", "x?", "--follow", "5", "--with-image", "--with-link=buy", "x.com"]).is_ok());
        assert!(check(&["jurl", "--with-link=buy", "x.com"]).is_err(), "no question, no answer to go with");
        assert!(check(&["jurl", "-q", "x?", "--links", "--with-link=buy", "x.com"]).is_err());
        assert!(check(&["jurl", "-q", "x?", "--with-link= ", "x.com"]).is_err(), "what the link is for");
        assert!(check(&["jurl", "-q", "x?", "--pages", "2", "--with-link=buy", "x.com"]).is_err());
    }

    #[test]
    fn with_pages_is_part_of_the_json() {
        let check = |argv: &[&str]| prepare(&mut Args::parse_from(argv));
        assert!(check(&["jurl", "--with-pages", "x.com"]).is_err(), "the pages are in the JSON only");
        assert!(check(&["jurl", "--json", "--with-pages", "x.com"]).is_ok());
        assert!(check(&["jurl", "--json", "x.com"]).is_ok(), "without it, the JSON is as it was");
    }
}
