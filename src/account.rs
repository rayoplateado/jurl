//! `jurl login`, `logout` and `status`: sign this computer in to jurl cloud with a code entered in a browser (the device
//! flow of RFC 8628), sign it out, and say what it reads with.

use std::{
    io::{IsTerminal, stderr},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::{
    cloud::{self, Cloud, Usage, Whoami},
    config::Config,
    decide, setup,
};

/// What `jurl login` saves in `~/.config/jurl/env`, and `logout` removes.
const SAVED: &[&str] = &["JURL_CLOUD_KEY", "JURL_CLOUD_URL"];
/// How often to ask for the key when jurl cloud doesn't say (RFC 8628's default).
const DEFAULT_INTERVAL: u64 = 5;
/// What each `slow_down` adds to the wait, as RFC 8628 asks.
const SLOW_DOWN: Duration = Duration::from_secs(5);
/// Network failures in a row before the sign-in gives up.
const MAX_FAILURES: u32 = 3;
const EXPIRED: &str = "the code expired before it was entered: run `jurl login` for a new one";

/// The code jurl cloud gave for this sign-in.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

/// The key jurl cloud approved for this computer, and what it is called there. The account's email and the console's
/// address come along when the reply has them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grant {
    access_token: String,
    organization_name: Option<String>,
    key_name: Option<String>,
    email: Option<String>,
    console_url: Option<String>,
}

/// `jurl login`: signs this computer in to jurl cloud. It works without a terminal too, since the code is printed either way.
pub(crate) async fn login(cfg: &mut Config, client: &Client) -> Result<()> {
    sign_in(cfg, client).await.map(|_| ())
}

/// Signs this computer in to jurl cloud and saves the key, and the address when it isn't the default. Prints the code and
/// where to enter it, then waits until it is entered.
pub(crate) async fn sign_in(cfg: &mut Config, client: &Client) -> Result<Cloud> {
    let base = cloud::base(cfg)?;
    let code = start(client, &base).await?;
    eprintln!(
        "To sign in to jurl cloud, open {} and enter this code:\n\n    {}\n",
        code.verification_uri, code.user_code
    );
    eprintln!("Waiting for it to be entered (it expires in {} minutes). Ctrl-C cancels.", code.expires_in / 60);
    open_browser(code.verification_uri_complete.as_deref().unwrap_or(&code.verification_uri));
    let grant = poll(client, &base, &code).await?;
    if base == cloud::DEFAULT_BASE {
        cfg.remove(&["JURL_CLOUD_URL"])?;
    } else {
        cfg.save("JURL_CLOUD_URL", &base)?;
    }
    let path = cfg.save("JURL_CLOUD_KEY", &grant.access_token)?;
    match &grant.key_name {
        Some(name) => eprintln!("Saved the key \"{name}\" to {}", path.display()),
        None => eprintln!("Saved to {}", path.display()),
    }
    eprintln!("{}", signed_in(&grant, &console(&grant, &base)));
    Ok(Cloud { base, key: grant.access_token })
}

/// The address a sign-in names as the console: the one the reply gives, when it's an address this jurl may send a key
/// to, else the one this jurl reads with.
fn console(grant: &Grant, base: &str) -> String {
    grant.console_url.as_deref().and_then(|url| cloud::checked_base(url).ok()).unwrap_or_else(|| base.to_string())
}

/// The line a sign-in ends with: whose account it is, and where to see it.
fn signed_in(grant: &Grant, console: &str) -> String {
    let whose = match (&grant.email, &grant.organization_name) {
        (Some(email), Some(org)) => format!("as {email} to {org}"),
        (Some(email), None) => format!("as {email}"),
        (None, Some(org)) => format!("to {org}"),
        (None, None) => "to jurl cloud".to_string(),
    };
    format!("Signed in {whose}. Console: {console}")
}

/// Asks jurl cloud for a code to enter in the browser.
async fn start(client: &Client, base: &str) -> Result<DeviceCode> {
    let body = json!({ "client": "jurl-cli", "clientVersion": env!("CARGO_PKG_VERSION"), "deviceName": device_name() });
    let reply = cloud::send(client.post(format!("{base}/api/v1/device/code")).json(&body), base).await?;
    if !reply.status.is_success() {
        return Err(cloud::refused(&reply).context("couldn't start the sign-in"));
    }
    serde_json::from_str(&reply.text)
        .context("couldn't start the sign-in: jurl cloud's code is not one this jurl reads")
}

/// Asks jurl cloud, one interval at a time, whether the code has been entered. Ends when it has (with the key), when it
/// is refused or expires, or after a few failed requests in a row.
async fn poll(client: &Client, base: &str, code: &DeviceCode) -> Result<Grant> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.unwrap_or(DEFAULT_INTERVAL));
    let mut failures = 0;
    loop {
        tokio::time::sleep(interval).await;
        if Instant::now() >= deadline {
            bail!(EXPIRED);
        }
        let body = json!({ "deviceCode": code.device_code });
        let reply = match cloud::send(client.post(format!("{base}/api/v1/device/token")).json(&body), base).await {
            Ok(reply) => reply,
            Err(e) => {
                failures += 1;
                if failures == MAX_FAILURES {
                    return Err(e.context("the sign-in stopped"));
                }
                continue;
            }
        };
        failures = 0;
        match answer(reply.status, &reply.text, interval) {
            Answer::Granted(grant) => return Ok(grant),
            Answer::Again(next) => interval = next,
            Answer::Ended(why) => bail!(why),
        }
    }
}

/// What one answer from the token endpoint means: the key, a wait before the next question, or the end of the sign-in.
#[derive(Debug)]
enum Answer {
    Granted(Grant),
    Again(Duration),
    Ended(String),
}

/// The answer's `error` says which of RFC 8628's cases it is, whatever the HTTP status is.
fn answer(status: StatusCode, text: &str, interval: Duration) -> Answer {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    match body["error"].as_str() {
        Some("authorization_pending") => Answer::Again(interval),
        Some("slow_down") => Answer::Again(interval + SLOW_DOWN),
        Some("access_denied") => Answer::Ended("the sign-in was denied in the browser, so nothing was saved".into()),
        Some("expired_token") => Answer::Ended(EXPIRED.into()),
        Some(other) => Answer::Ended(format!("jurl cloud ended the sign-in: {other}")),
        None if status.is_success() => match serde_json::from_value::<Grant>(body) {
            Ok(grant) => Answer::Granted(grant),
            Err(e) => Answer::Ended(format!("jurl cloud's key is not one this jurl reads: {e}")),
        },
        None => {
            Answer::Ended(format!("jurl cloud answered HTTP {status} during the sign-in: {}", decide::snippet(text)))
        }
    }
}

/// `jurl logout`: signs this computer out of jurl cloud, and says what reads now.
pub(crate) async fn logout(cfg: &mut Config, client: &Client) -> Result<()> {
    if !sign_out(cfg, client).await? {
        eprintln!("Not signed in to jurl cloud on this computer.");
    }
    if Config::from_env("JURL_CLOUD_KEY").is_some() {
        eprintln!("JURL_CLOUD_KEY is still set in the environment: unset it to stop reading with jurl cloud.");
    } else if setup::saved_key(cfg).is_ok() {
        eprintln!("Reads use your own keys.");
    } else {
        eprintln!("No own keys are set up: run `jurl init` to read pages.");
    }
    Ok(())
}

/// Signs out: revokes this computer's key on jurl cloud, if it can be reached, and removes the key from the config. False
/// when there was no sign-in to remove.
pub(crate) async fn sign_out(cfg: &mut Config, client: &Client) -> Result<bool> {
    let Some(key) = cfg.saved("JURL_CLOUD_KEY") else { return Ok(false) };
    if let Ok(base) = cloud::base(cfg)
        && let Err(e) = revoke(client, &base, &key).await
    {
        eprintln!("jurl: signed out here, but jurl cloud didn't confirm the revoke: {e:#}");
    }
    cfg.remove(SAVED)?;
    eprintln!("Signed out of jurl cloud on this computer.");
    Ok(true)
}

/// Revokes the key on jurl cloud. A 404 is fine: there was nothing to revoke.
async fn revoke(client: &Client, base: &str, key: &str) -> Result<()> {
    let req = client.delete(format!("{base}/api/v1/device/session")).bearer_auth(key);
    let reply = cloud::send(req, base).await?;
    if reply.status.is_success() || reply.status == StatusCode::NOT_FOUND {
        return Ok(());
    }
    Err(cloud::refused(&reply))
}

/// `jurl status`: what this computer reads with. For jurl cloud: the organization, the key, and what the period has used,
/// as far as the server says (a server without an endpoint leaves that part out).
pub(crate) async fn status(cfg: &Config, client: &Client) -> Result<()> {
    if let Some(cloud) = cloud::in_use(cfg)? {
        let who: Option<Whoami> = cloud::get(client, &cloud, "whoami").await?;
        let usage: Option<Usage> = cloud::get(client, &cloud, "usage").await?;
        let org = who.as_ref().and_then(|w| w.organization.as_ref()).map(|o| format!(" · {}", o.name));
        let period = usage.as_ref().map(|u| format!(" · {}", cloud::usage_line(u, cloud::now_ms())));
        println!("jurl cloud{}{}", org.unwrap_or_default(), period.unwrap_or_default());
        let mut details = Vec::new();
        if let Some(key) = who.as_ref().and_then(|w| w.key.as_ref()) {
            details.push(format!("key \"{}\"", key.name));
        }
        if let Some(plan) = usage.as_ref().and_then(Usage::plan_name) {
            details.push(format!("{plan} plan"));
        }
        if usage.is_none() {
            details.push("usage not reported by this server".into());
        }
        if cloud.base != cloud::DEFAULT_BASE {
            details.push(cloud.base.clone());
        }
        if !details.is_empty() {
            println!("{}", details.join(" · "));
        }
        return Ok(());
    }
    let jev = match decide::custom_jev_url() {
        Some(url) => format!("Jev at {url}"),
        None if cfg.get("TYPESAFE_API_KEY").is_some() => "TypeSafe (Jev) set".to_string(),
        None => bail!("not set up: run `jurl init` for your own keys, or `jurl login` for jurl cloud"),
    };
    let clef = if cfg.get("CLOUDFLARE_AI_TOKEN").is_some() { "set" } else { "not set" };
    println!("own keys · {jev} · Cloudflare (Clef) {clef}");
    if cfg.saved("JURL_CLOUD_KEY").is_some() {
        println!(
            "jurl cloud is also signed in here, but TYPESAFE_API_KEY or JURL_JEV_URL in the environment is used instead"
        );
    }
    Ok(())
}

/// This computer's name, which jurl cloud shows against the key.
fn device_name() -> String {
    let named = ["HOSTNAME", "COMPUTERNAME"].into_iter().find_map(Config::from_env).or_else(|| {
        let out = Command::new("hostname").output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    named.filter(|n| !n.is_empty()).unwrap_or_else(|| "unknown".into())
}

/// Opens the code's page in the browser, when this is a terminal, not an SSH session, and the address is https (or plain
/// http to this computer). Best effort: the code and the address are printed anyway.
fn open_browser(url: &str) {
    if !stderr().is_terminal() || std::env::var_os("SSH_CONNECTION").is_some() || !openable(url) {
        return;
    }
    let (program, args) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("rundll32", vec!["url.dll,FileProtocolHandler", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Whether a URL may be handed to the browser: https, or plain http to this computer.
fn openable(url: &str) -> bool {
    url.starts_with("https://") || (url.starts_with("http://") && !decide::plain_http_off_loopback(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock;

    fn code(expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "dev-123".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://cloud.jurl.dev/device".into(),
            verification_uri_complete: None,
            expires_in,
            interval: Some(0),
        }
    }

    #[test]
    fn the_answers_follow_rfc_8628() {
        let wait = Duration::from_secs(5);
        let pending = answer(StatusCode::BAD_REQUEST, r#"{"error":"authorization_pending"}"#, wait);
        assert!(matches!(pending, Answer::Again(d) if d == wait));
        let slow = answer(StatusCode::BAD_REQUEST, r#"{"error":"slow_down"}"#, wait);
        assert!(matches!(slow, Answer::Again(d) if d == Duration::from_secs(10)), "{slow:?}");
        let denied = answer(StatusCode::BAD_REQUEST, r#"{"error":"access_denied"}"#, wait);
        assert!(matches!(denied, Answer::Ended(m) if m.contains("denied")));
        let expired = answer(StatusCode::BAD_REQUEST, r#"{"error":"expired_token"}"#, wait);
        assert!(matches!(expired, Answer::Ended(m) if m.contains("expired")));
        let granted =
            answer(StatusCode::OK, r#"{"accessToken":"jurl_abc","organizationName":"Acme","keyName":"laptop"}"#, wait);
        assert!(
            matches!(granted, Answer::Granted(g) if g.access_token == "jurl_abc" && g.key_name.as_deref() == Some("laptop"))
        );
        let with_account = answer(
            StatusCode::OK,
            r#"{"accessToken":"jurl_abc","email":"ray@acme.com","consoleUrl":"https://console.acme.test"}"#,
            wait,
        );
        assert!(matches!(with_account, Answer::Granted(g) if g.email.as_deref() == Some("ray@acme.com")
                && g.console_url.as_deref() == Some("https://console.acme.test")));
        let other = answer(StatusCode::INTERNAL_SERVER_ERROR, "oops", wait);
        assert!(matches!(&other, Answer::Ended(m) if m.contains("HTTP 500 Internal Server Error")), "{other:?}");
    }

    fn sample_grant(email: Option<&str>, org: Option<&str>, console: Option<&str>) -> Grant {
        Grant {
            access_token: "jurl_abc".into(),
            organization_name: org.map(Into::into),
            key_name: Some("laptop".into()),
            email: email.map(Into::into),
            console_url: console.map(Into::into),
        }
    }

    #[test]
    fn a_sign_in_ends_with_whose_account_it_is_and_where_to_see_it() {
        let base = "https://cloud.jurl.dev";
        let line = |email, org| signed_in(&sample_grant(email, org, None), base);
        assert_eq!(
            line(Some("ray@acme.com"), Some("Acme")),
            "Signed in as ray@acme.com to Acme. Console: https://cloud.jurl.dev"
        );
        assert_eq!(line(None, Some("Acme")), "Signed in to Acme. Console: https://cloud.jurl.dev");
        assert_eq!(line(Some("ray@acme.com"), None), "Signed in as ray@acme.com. Console: https://cloud.jurl.dev");
        assert_eq!(
            signed_in(&sample_grant(None, None, None), "http://127.0.0.1:3211"),
            "Signed in to jurl cloud. Console: http://127.0.0.1:3211"
        );
    }

    #[test]
    fn the_console_is_the_replys_address_when_it_is_safe_else_where_jurl_reads() {
        let base = "http://127.0.0.1:3211";
        assert_eq!(console(&sample_grant(None, None, None), base), base);
        assert_eq!(
            console(&sample_grant(None, None, Some("https://console.acme.test/")), base),
            "https://console.acme.test"
        );
        // A plain http address off this computer is never a console to send a key to.
        assert_eq!(console(&sample_grant(None, None, Some("http://cloud.example.com")), base), base);
    }

    #[tokio::test]
    async fn a_sign_in_starts_with_the_client_and_this_computer() {
        let reply = r#"{"deviceCode":"dev-123","userCode":"ABCD-EFGH","verificationUri":"https://cloud.jurl.dev/device","verificationUriComplete":"https://cloud.jurl.dev/device?code=ABCD-EFGH","expiresIn":900,"interval":5}"#;
        let (base, seen) = mock::serve(vec![(200, "", reply)]);
        let started = start(&Client::new(), &base).await.unwrap();
        assert_eq!(started.user_code, "ABCD-EFGH");
        assert_eq!(started.interval, Some(5));
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].path, "/api/v1/device/code");
        assert!(seen[0].body.contains("\"client\":\"jurl-cli\""), "{}", seen[0].body);
        assert!(seen[0].body.contains("\"clientVersion\""), "{}", seen[0].body);
        assert!(seen[0].body.contains("\"deviceName\""), "{}", seen[0].body);
    }

    #[tokio::test]
    async fn a_sign_in_waits_while_the_code_is_pending_then_gets_the_key() {
        let (base, seen) = mock::serve(vec![
            (400, "", r#"{"error":"authorization_pending"}"#),
            (200, "", r#"{"accessToken":"jurl_abc","organizationName":"Acme","keyName":"laptop"}"#),
        ]);
        let grant = poll(&Client::new(), &base, &code(600)).await.unwrap();
        assert_eq!(grant.access_token, "jurl_abc");
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(seen.lock().unwrap()[1].body.contains("dev-123"));
    }

    #[tokio::test]
    async fn a_code_past_its_time_ends_the_sign_in_without_asking_again() {
        let (base, seen) = mock::serve(vec![]);
        let err = poll(&Client::new(), &base, &code(0)).await.unwrap_err();
        assert!(format!("{err:#}").contains("expired"), "{err:#}");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_few_failed_requests_in_a_row_end_the_sign_in() {
        // A port nothing listens on: every poll is refused.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = poll(&Client::new(), &format!("http://127.0.0.1:{port}"), &code(600)).await.unwrap_err();
        assert!(format!("{err:#}").contains("couldn't reach jurl cloud"), "{err:#}");
    }

    #[tokio::test]
    async fn signing_out_revokes_the_key_and_tolerates_a_server_without_the_endpoint() {
        let (base, seen) = mock::serve(vec![(404, "", r#"{"error":"not_found"}"#)]);
        revoke(&Client::new(), &base, "jurl_abc").await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("DELETE", "/api/v1/device/session"));
            assert!(
                seen[0].headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer jurl_abc")
            );
        }
        let (base, _) = mock::serve(vec![(500, "", r#"{"error":"boom","message":"Try later."}"#)]);
        assert!(revoke(&Client::new(), &base, "jurl_abc").await.is_err());
    }

    #[test]
    fn only_https_or_this_computer_goes_to_the_browser() {
        assert!(openable("https://cloud.jurl.dev/device?code=ABCD-EFGH"));
        assert!(openable("http://127.0.0.1:3211/device"));
        assert!(!openable("http://cloud.jurl.dev/device"));
        assert!(!openable("file:///etc/passwd"));
    }

    #[test]
    fn this_computer_has_a_name() {
        assert!(!device_name().is_empty());
    }
}
