//! First-run setup: choose how to read pages (your own keys, or jurl cloud), check the keys against the real APIs, save
//! them. `jurl init` does the same, and can switch between the two.

use std::io::{BufRead, IsTerminal, Write, stderr, stdin};

use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::{Map, json};

use crate::{
    account,
    cloud::{self, Cloud},
    config::Config,
    decide,
};

const TYPESAFE_URL: &str = "https://console.typesafe.ai";
const CLOUDFLARE_URL: &str = "https://dash.cloudflare.com/profile/api-tokens";
/// How much of an error's first line goes on the "that didn't work" line.
const SHORT_ERROR_CHARS: usize = 80;

/// How a run reads pages: through jurl cloud, or with your own keys.
pub(crate) enum Access {
    Cloud(Cloud),
    /// The TypeSafe key, or an empty one when `JURL_JEV_URL` names a server that takes none.
    Own(String),
}

fn interactive() -> bool {
    stdin().is_terminal() && stderr().is_terminal()
}

/// How this run reads pages: through jurl cloud when it's in use (see `cloud::in_use`), else with the own keys. With
/// nothing set up yet and a terminal to ask in, it asks which of the two to use first.
pub(crate) async fn access(cfg: &mut Config, client: &Client) -> Result<Access> {
    if let Some(cloud) = cloud::in_use(cfg)? {
        return Ok(Access::Cloud(cloud));
    }
    if !interactive() || cfg.get("TYPESAFE_API_KEY").is_some() || decide::custom_jev_url().is_some() {
        return saved_key(cfg).map(Access::Own);
    }
    eprintln!(
        "jurl reads pages with Jev, TypeSafe's decision model. It needs an API key, once, or a jurl cloud account."
    );
    if ask_cloud()? {
        return Ok(Access::Cloud(account::sign_in(cfg, client).await?));
    }
    ask_typesafe(cfg, client).await.map(Access::Own)
}

/// The access for a run that never asks, such as `jurl mcp`: jurl cloud when it's in use, else the saved own keys.
pub(crate) fn saved_access(cfg: &Config) -> Result<Access> {
    match cloud::in_use(cfg)? {
        Some(cloud) => Ok(Access::Cloud(cloud)),
        None => saved_key(cfg).map(Access::Own),
    }
}

/// The TypeSafe key from the environment or `jurl init`, never asking: `jurl mcp` has no terminal to ask in.
pub(crate) fn saved_key(cfg: &Config) -> Result<String> {
    key_for(decide::custom_jev_url().is_some(), |k| cfg.get(k))
}

/// The bearer for Jev's requests. Another server (`JURL_JEV_URL`) never gets the TypeSafe key: only `JURL_JEV_KEY`,
/// or no bearer at all.
fn key_for(custom_url: bool, get: impl Fn(&str) -> Option<String>) -> Result<String> {
    if custom_url {
        return Ok(get("JURL_JEV_KEY").unwrap_or_default());
    }
    match get("TYPESAFE_API_KEY") {
        Some(k) => Ok(k),
        None => bail!("missing TypeSafe API key: run `jurl init`, or set TYPESAFE_API_KEY (get one at {TYPESAFE_URL})"),
    }
}

/// `jurl init`: choose how to read pages. Your own keys: set or replace both. jurl cloud: sign in.
pub(crate) async fn init(cfg: &mut Config, client: &Client) -> Result<()> {
    if !interactive() {
        bail!(
            "`jurl init` needs a terminal; otherwise set TYPESAFE_API_KEY (and CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_AI_TOKEN) in the environment"
        );
    }
    eprintln!("How should jurl read pages?");
    if ask_cloud()? {
        account::sign_in(cfg, client).await?;
        eprintln!("Reads go to jurl cloud now. `jurl logout` switches back to your own keys.");
        return Ok(());
    }
    if cfg.saved("JURL_CLOUD_KEY").is_some() {
        if confirm("    This computer is signed in to jurl cloud. Sign out, so reads use your own keys?")? {
            account::sign_out(cfg, client).await?;
        } else {
            eprintln!("    Kept. Reads still go to jurl cloud until you run `jurl logout`.");
        }
    }
    eprintln!("\n1/2 · TypeSafe (Jev), required. Get a key at {TYPESAFE_URL}");
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

/// Which way to read pages: your own keys (`false`) or jurl cloud (`true`). Asked until the answer is 1 or 2.
fn ask_cloud() -> Result<bool> {
    eprintln!("  1) Your own keys: a TypeSafe API key, from {TYPESAFE_URL}");
    eprintln!("  2) jurl cloud: sign in with your browser, and jurl's servers read the pages");
    loop {
        if let Some(cloud) = cloud_choice(&line("Choose 1 or 2 [1]: ")?) {
            return Ok(cloud);
        }
        eprintln!("    Enter 1 or 2.");
    }
}

/// What an answer to "1 or 2" picks: 2 is jurl cloud, 1 (or Enter) your own keys, anything else isn't an answer.
fn cloud_choice(answer: &str) -> Option<bool> {
    match answer {
        "" | "1" => Some(false),
        "2" => Some(true),
        _ => None,
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
        None => s.lines().next().unwrap_or("").chars().take(SHORT_ERROR_CHARS).collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typesafe_key_never_goes_to_another_server() {
        let both = |k: &str| match k {
            "TYPESAFE_API_KEY" => Some("ts-key".to_string()),
            "JURL_JEV_KEY" => Some("own-key".to_string()),
            _ => None,
        };
        let typesafe_only = |k: &str| (k == "TYPESAFE_API_KEY").then(|| "ts-key".to_string());
        assert_eq!(key_for(false, both).unwrap(), "ts-key");
        assert_eq!(key_for(true, both).unwrap(), "own-key");
        assert_eq!(key_for(true, typesafe_only).unwrap(), "");
        assert!(key_for(false, |_| None).is_err());
    }

    #[test]
    fn enter_and_1_are_your_own_keys_and_2_is_jurl_cloud() {
        assert_eq!(cloud_choice(""), Some(false));
        assert_eq!(cloud_choice("1"), Some(false));
        assert_eq!(cloud_choice("2"), Some(true));
        assert_eq!(cloud_choice("3"), None);
    }
}
