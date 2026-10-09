//! The entry point: check the flags, load the page (rendering it when it needs JavaScript), then run the
//! mode they ask for.

mod answer;
mod blocks;
mod cli;
mod config;
mod decide;
mod extract;
mod fetch;
mod follow;
mod hosts;
mod judge;
mod lightpanda;
mod links;
mod mcp;
mod output;
mod precise;
mod setup;
mod sitemaps;
mod timing;
mod update;
mod vision;

use std::{io::stdout, process::ExitCode, time::Duration};

use anyhow::{Context, Result, bail};
use clap::Parser;
use futures::future::join_all;
use reqwest::Client;

use crate::{
    cli::Args,
    config::Config,
    extract::{Extracted, Kind},
    judge::Ctx,
    output::{Rendered, exit_code, stdout_for, write_out},
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
        Err(e) => {
            eprintln!("jurl: {e:#}");
            ExitCode::from(exit_code(&e))
        }
    }
}

/// The client every request goes through, pages and APIs alike: a browser's user agent, with jurl's version on the end.
pub(crate) fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15 (KHTML, like Gecko) jurl/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(HTTP_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .build()?)
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
    if args.url == "mcp" {
        return mcp::serve(client).await;
    }
    prepare(&mut args)?;
    let key = setup::typesafe_key(&mut cfg, &client).await?;

    let mut t = Timer::new();
    let done = page(&args, &cfg, &client, &key, &mut t).await;
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
/// answers.
pub(crate) async fn page(args: &Args, cfg: &Config, client: &Client, key: &str, t: &mut Timer) -> Result<Rendered> {
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
    let (url, ex) = load(args, cfg, client, &target, t).await?;
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

/// Fetch a page (rendering it when it needs JavaScript) and cut it into blocks, links and images.
pub(crate) async fn load(
    args: &Args,
    cfg: &Config,
    client: &Client,
    target: &url::Url,
    t: &mut Timer,
) -> Result<(url::Url, Extracted)> {
    let page = if args.render {
        let bin = lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await?;
        let page = fetch::render(&bin, target).await?;
        t.lap("render");
        page
    } else {
        // A host switching to the browser client is said on stderr under -t, once, where the switch happens.
        let retry = fetch::Retry::for_run(args.no_browser_retry, args.timing);
        let page = fetch::fetch(client, target.as_str(), retry).await?;
        t.lap("fetch");
        if page.via_browser {
            decide::USAGE.browser_retry.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        page
    };
    // Only HTML can carry placeholders: a markdown page has no script to have left them.
    let (mut ex, placeholders) = if page.is_markdown {
        (extract::markdown(&page.body, &page.url), false)
    } else {
        extract::html_with_placeholders(&page.body, &page.url)
    };
    t.lap(if page.is_markdown { "extract(md)" } else { "extract" });
    decide::USAGE.pages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // A JS app with (almost) no server-rendered text, or a page whose script left template placeholders in its text
    // (a long text is no proof that the script ran): render it instead of giving up.
    let text: usize = ex.blocks.iter().filter(|b| b.kind != Kind::Heading).map(|b| b.text.len()).sum();
    let shell = ex.app_shell && text < APP_SHELL_TEXT;
    if !args.render && (shell || placeholders) {
        match lightpanda::ensure(cfg.get("JURL_LIGHTPANDA")).await {
            Ok(bin) => {
                let why =
                    if shell { "no text without JavaScript" } else { "unfilled template placeholders in the text" };
                eprintln!("jurl: {why}, rendering with Lightpanda…");
                match fetch::render(&bin, &page.url).await {
                    Ok(rendered) => {
                        ex = extract::html(&rendered.body, &rendered.url);
                        t.lap("render");
                    }
                    // An app shell has nothing to read without its render; a page with placeholders is readable anyway.
                    Err(e) if shell => return Err(e),
                    Err(e) => eprintln!("jurl: could not render the page, reading it as it is: {e:#}"),
                }
            }
            Err(e) => eprintln!("jurl: {e:#}"),
        }
    }
    Ok((page.url, ex))
}
