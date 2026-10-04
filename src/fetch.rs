use anyhow::{Context, Result, bail};
use reqwest::{Client, header};
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

pub async fn fetch_bytes(client: &Client, url: &Url) -> Result<Vec<u8>> {
    let res = client.get(url.as_str()).send().await?.error_for_status()?;
    Ok(res.bytes().await?.to_vec())
}
