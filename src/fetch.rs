use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{Client, header};
use tokio::process::Command;
use url::Url;

pub struct Page {
    pub url: Url,
    pub body: String,
    pub is_markdown: bool,
}

pub async fn fetch(client: &Client, url: &str) -> Result<Page> {
    let res = client
        .get(url)
        .header(header::ACCEPT, "text/markdown, text/html;q=0.9, */*;q=0.5")
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = res.status();
    if !status.is_success() {
        bail!("{url} returned HTTP {status}");
    }
    let final_url = res.url().clone();
    let is_markdown = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("markdown"));
    let body = res.text().await?;
    Ok(Page { url: final_url, body, is_markdown })
}

/// Run the page's JavaScript in Lightpanda and return the resulting DOM.
/// Waits for the network to settle and for real visible text to appear, capped at 5s:
/// SPAs keep background traffic going and often paint content after the network calms down.
pub async fn render(bin: &Path, url: &Url) -> Result<Page> {
    let run = Command::new(bin)
        .args(["fetch", "--dump", "html", "--wait-until", "networkalmostidle", "--wait-ms", "5000"])
        .args(["--wait-script", "document.body && (document.body.innerText || '').trim().length > 1500"])
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .context("lightpanda timed out")?
        .with_context(|| format!("running {}", bin.display()))?;
    if out.stdout.is_empty() {
        bail!("lightpanda rendered nothing for {url} (exit {})", out.status);
    }
    Ok(Page { url: url.clone(), body: String::from_utf8_lossy(&out.stdout).into_owned(), is_markdown: false })
}

pub async fn fetch_bytes(client: &Client, url: &Url) -> Result<Vec<u8>> {
    let res = client.get(url.as_str()).send().await?.error_for_status()?;
    Ok(res.bytes().await?.to_vec())
}
