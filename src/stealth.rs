//! Rung 5 of the read ladder: a small HTTP sidecar that loads one public page in a
//! stealth browser (Camoufox, MPL-2.0) and returns its rendered text. Used only after
//! rungs 1–4 meet an explicit anti-bot challenge; never to solve a CAPTCHA.
//!
//! The sidecar is `jurl-cloud/infra/stealth/`. jurl talks to it when
//! `JURL_STEALTH_URL` names the service (for example `http://127.0.0.1:8091`).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};
use url::Url;

/// How long one stealth render may take, including the sidecar's own settle window.
const TIMEOUT: Duration = Duration::from_secs(70);

/// What the sidecar may answer. `Captcha` is a hard stop: the cell goes to human review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Content,
    Captcha,
    Blocked,
    Challenge,
    Error,
}

pub(crate) struct Stealth {
    pub outcome: Outcome,
    pub status: Option<u16>,
    pub body: String,
    pub title: String,
    pub reason: Option<String>,
}

/// A CAPTCHA frame or interstitial the page shows the visitor.
const CAPTCHA_MARKERS: &[&str] = &[
    "captcha-delivery.com",
    "geo.captcha",
    "hcaptcha.com",
    "g-recaptcha",
    "challenges.cloudflare.com",
    "px-captcha",
    "funcaptcha",
    "arkoselabs",
];

/// Titles an anti-bot interstitial uses, in English and Spanish.
const CHALLENGE_TITLES: &[&str] = &[
    "just a moment",
    "un momento",
    "checking your browser",
    "attention required",
    "one moment",
    "please wait",
    "verificando",
    "comprobando",
    "unusual traffic",
    "tráfico inusual",
    "trafico inusual",
    "verify you are human",
];

/// Scripts and cookies a thin challenge page carries (DataDome, Incapsula, Akamai, Cloudflare, PerimeterX).
const VENDOR_MARKERS: &[&str] = &[
    "js.datadome.co",
    "captcha-delivery",
    "_incapsula_resource",
    "incap_ses",
    "visid_incap",
    "_abck",
    "bm_sz",
    "ak_bmsc",
    "challenge-platform",
    "cf-chl-",
    "cdn-cgi/challenge",
    "perimeterx",
    "_pxhd",
];

fn has_any(hay: &str, needles: &[&str]) -> bool {
    let hay = hay.to_ascii_lowercase();
    needles.iter().any(|n| hay.contains(n))
}

/// Whether a page is an explicit anti-bot wall rather than ordinary content. The
/// markers match the stealth harness (`research/stealth-tier`): a CAPTCHA frame, a
/// challenge title, or a thin page carrying a vendor's script.
pub(crate) fn looks_like_challenge(status: Option<u16>, title: &str, body: &str) -> bool {
    if has_any(body, CAPTCHA_MARKERS) {
        return true;
    }
    if has_any(title, CHALLENGE_TITLES) {
        return true;
    }
    let thin = body.len() < 1500;
    let vendor = has_any(body, VENDOR_MARKERS);
    if thin && vendor {
        return true;
    }
    matches!(status, Some(401 | 403 | 429 | 503)) && (thin || vendor)
}

/// Ask the stealth sidecar for `url`. `token` is sent as a bearer when the sidecar set
/// `STEALTH_TOKEN`; the proxy credential never travels here — the sidecar owns it.
pub(crate) async fn render(base: &str, token: Option<&str>, url: &Url) -> Result<Stealth> {
    let client = Client::builder().timeout(TIMEOUT).build().context("building the stealth client")?;
    let mut req = client
        .post(format!("{}/render", base.trim_end_matches('/')))
        .json(&json!({ "url": url.as_str() }));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.context("asking the stealth sidecar")?;
    let status = res.status();
    let v: Value = res.json().await.context("the stealth sidecar answered no JSON")?;
    if !status.is_success() {
        bail!("the stealth sidecar refused {url}: {} ({})", status, v["error"].as_str().unwrap_or("error"));
    }
    let outcome = match v["outcome"].as_str().unwrap_or("error") {
        "content" => Outcome::Content,
        "captcha" => Outcome::Captcha,
        "blocked" => Outcome::Blocked,
        "challenge" => Outcome::Challenge,
        _ => Outcome::Error,
    };
    Ok(Stealth {
        outcome,
        status: v["status"].as_u64().map(|n| n as u16),
        body: v["text"].as_str().unwrap_or_default().to_string(),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        reason: v["reason"].as_str().map(str::to_string),
    })
}

/// A CAPTCHA is a hard stop under the rung-5 guardrails; the caller sends the cell to
/// human review rather than trying again. Everything else is a normal failure.
pub(crate) fn bail_on_captcha(s: &Stealth, url: &Url) -> Result<()> {
    if s.outcome == Outcome::Captcha {
        bail!("{url}: an interactive CAPTCHA — not solved; send to human review");
    }
    Ok(())
}

/// When `JURL_STEALTH_URL` is set and `page` is an anti-bot wall, ask the sidecar once
/// and return the rendered page. A CAPTCHA is an error (human review); a sidecar
/// failure is said on stderr and the wall stands.
pub(crate) async fn maybe_render(
    cfg: &crate::config::Config,
    page: &crate::fetch::Page,
) -> Result<Option<crate::fetch::Page>> {
    let Some(base) = cfg.get("JURL_STEALTH_URL") else { return Ok(None) };
    if !looks_like_challenge(None, &html_title(&page.body), &page.body) {
        return Ok(None);
    }
    eprintln!("jurl: anti-bot challenge, asking the stealth sidecar…");
    let token = cfg.get("JURL_STEALTH_TOKEN");
    match render(&base, token.as_deref(), &page.url).await {
        Ok(s) => {
            bail_on_captcha(&s, &page.url)?;
            if s.outcome != Outcome::Content || s.body.trim().is_empty() {
                let why = s.reason.as_deref().unwrap_or("no content");
                eprintln!("jurl: stealth sidecar could not read the page ({why})");
                return Ok(None);
            }
            Ok(Some(crate::fetch::Page {
                url: page.url.clone(),
                body: s.body,
                is_markdown: false,
                via_browser: false,
            }))
        }
        Err(e) => {
            eprintln!("jurl: stealth sidecar unavailable: {e:#}");
            Ok(None)
        }
    }
}

/// A page's `<title>`, or empty. Enough for [`looks_like_challenge`]'s title test.
fn html_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else { return String::new() };
    let rest = &html[start..];
    let Some(open) = rest.find('>') else { return String::new() };
    let after = &rest[open + 1..];
    let Some(close) = after.to_ascii_lowercase().find("</title") else { return String::new() };
    after[..close].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datadome_captcha_page_is_a_challenge() {
        let body = r#"<iframe src="https://geo.captcha-delivery.com/captcha/?initialCid=abc"></iframe>"#;
        assert!(looks_like_challenge(Some(403), "fnac.es", body));
    }

    #[test]
    fn a_thin_akamai_shell_is_a_challenge() {
        let body = "Access denied _abck bm_sz";
        assert!(looks_like_challenge(Some(403), "zara.com", body));
    }

    #[test]
    fn ordinary_content_is_not() {
        let body = "Envío a domicilio 5,90 € hasta 99 €. ".repeat(40);
        assert!(!looks_like_challenge(Some(200), "Envíos y recogida", &body));
    }

    #[test]
    fn a_captcha_outcome_bails() {
        let s = Stealth {
            outcome: Outcome::Captcha,
            status: Some(403),
            body: String::new(),
            title: String::new(),
            reason: None,
        };
        assert!(bail_on_captcha(&s, &Url::parse("https://example.test/").unwrap()).is_err());
    }
}
