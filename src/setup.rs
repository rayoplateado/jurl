//! First-run setup: ask for the API keys, check them against the real APIs, save them.

use std::io::{BufRead, IsTerminal, Write, stderr, stdin};

use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::{Map, json};

use crate::{config::Config, decide};

const TYPESAFE_URL: &str = "https://console.typesafe.ai";
const CLOUDFLARE_URL: &str = "https://dash.cloudflare.com/profile/api-tokens";

pub fn interactive() -> bool {
    stdin().is_terminal() && stderr().is_terminal()
}

/// The TypeSafe key, asking for it when running in a terminal and it is missing.
pub async fn typesafe_key(cfg: &mut Config, client: &Client) -> Result<String> {
    if !interactive()
        || cfg.get("TYPESAFE_API_KEY").is_some()
        || std::env::var("JURL_JEV_URL").is_ok_and(|u| !u.is_empty())
    {
        return saved_key(cfg);
    }
    eprintln!("jurl reads pages with Jev, TypeSafe's decision model. It needs your API key, once.");
    eprintln!("Get one at {TYPESAFE_URL}\n");
    ask_typesafe(cfg, client).await
}

/// The TypeSafe key from the environment or `jurl init`, never asking: `jurl mcp` has no terminal to ask in.
pub fn saved_key(cfg: &Config) -> Result<String> {
    match cfg.get("TYPESAFE_API_KEY") {
        Some(k) => Ok(k),
        // a self-hosted endpoint (`JURL_JEV_URL`) may need no key
        None if std::env::var("JURL_JEV_URL").is_ok_and(|u| !u.is_empty()) => Ok(String::new()),
        None => bail!("missing TypeSafe API key: run `jurl init`, or set TYPESAFE_API_KEY (get one at {TYPESAFE_URL})"),
    }
}

/// `jurl init`: set or replace both keys.
pub async fn init(cfg: &mut Config, client: &Client) -> Result<()> {
    if !interactive() {
        bail!(
            "`jurl init` needs a terminal; otherwise set TYPESAFE_API_KEY (and CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_AI_TOKEN) in the environment"
        );
    }
    eprintln!("1/2 · TypeSafe (Jev), required. Get a key at {TYPESAFE_URL}");
    if cfg.get("TYPESAFE_API_KEY").is_some() && !confirm("    A key is already set. Replace it?")? {
        eprintln!("    Kept.");
    } else {
        ask_typesafe(cfg, client).await?;
    }

    eprintln!("\n2/2 · Cloudflare Workers AI (Clef), optional: only for --vision and --find.");
    eprintln!("    Create a token with the \"Workers AI\" permission at {CLOUDFLARE_URL}");
    if cfg.get("CLOUDFLARE_AI_TOKEN").is_some() && !confirm("    Already set. Replace it?")? {
        eprintln!("    Kept.");
        return Ok(());
    }
    let account = line("    Account ID (Enter to skip): ")?;
    if account.is_empty() {
        eprintln!("    Skipped. Run `jurl init` again whenever you want it.");
        return Ok(());
    }
    loop {
        let token = rpassword::prompt_password("    API token (hidden): ")?.trim().to_string();
        if token.is_empty() {
            eprintln!("    Skipped.");
            return Ok(());
        }
        eprint!("    Checking… ");
        let state = json!("jurl setup check");
        let qs = Map::from_iter([("ok".to_string(), decide::noul("This text mentions setup"))]);
        match decide::clef(client, &account, &token, state, qs, Vec::new()).await {
            Ok(_) => {
                cfg.save("CLOUDFLARE_ACCOUNT_ID", &account)?;
                let path = cfg.save("CLOUDFLARE_AI_TOKEN", &token)?;
                eprintln!("ok. Saved to {}", path.display());
                return Ok(());
            }
            Err(e) => eprintln!("that didn't work ({}). Try again, or press Enter to skip.", short(&e)),
        }
    }
}

async fn ask_typesafe(cfg: &mut Config, client: &Client) -> Result<String> {
    loop {
        let key = rpassword::prompt_password("    TypeSafe API key (hidden): ")?.trim().to_string();
        if key.is_empty() {
            bail!("no key given; run `jurl init` when you have one");
        }
        eprint!("    Checking… ");
        let qs = Map::from_iter([("ok".to_string(), decide::noul("This text mentions setup"))]);
        match decide::jev(client, &key, json!("jurl setup check"), qs).await {
            Ok(_) => {
                let path = cfg.save("TYPESAFE_API_KEY", &key)?;
                eprintln!("ok. Saved to {}\n", path.display());
                return Ok(key);
            }
            Err(e) => eprintln!("that key didn't work ({}). Try again.", short(&e)),
        }
    }
}

/// "HTTP 401 Unauthorized" out of a full API error, or the first line of anything else.
fn short(e: &anyhow::Error) -> String {
    let s = e.to_string();
    match s.find("HTTP ") {
        Some(i) => s[i..].split(':').next().unwrap_or("").to_string(),
        None => s.lines().next().unwrap_or("").chars().take(80).collect(),
    }
}

fn line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    stderr().flush()?;
    let mut s = String::new();
    stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn confirm(prompt: &str) -> Result<bool> {
    Ok(matches!(line(&format!("{prompt} [y/N] "))?.to_ascii_lowercase().as_str(), "y" | "yes"))
}
