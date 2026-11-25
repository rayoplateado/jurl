//! Frozen pages: a run that writes down the pages it reads, and a run that reads them back with no network (docs/reference.md).
//!
//! `JURL_FROZEN_DIR` names a directory and `JURL_FROZEN_MODE` says what the run does with it:
//!
//! - `record`: the run reads the web as it always does, and writes each page as it reaches the extractor (after the proxy, the
//!   stealth sidecar and the render, so a rendered page is its DOM), and each small file it asks for (robots.txt, llms.txt, a
//!   sitemap, a site's search description) with the ones the site does not have. A PDF, or any body that is not text, is kept as
//!   the bytes that came.
//! - `replay`: the run reads from the directory and never asks the network. It renders nothing (a page that was rendered when it
//!   was recorded is given as its DOM, as it was), and it skips the checks of a public address (`reach`, the DNS lookup). An
//!   address that is not in the directory is [`NotFrozen`], never a live read.
//!
//! Without the variable nothing here runs. The directory holds, for each address read, `<sha256(url + Accept-Language)>.json`
//! (what the entry is) and, beside it, `<same name>.html` (or `.pdf`) with the body. A small file's entry is `<same name>.small`
//! instead, so a page and a file at one address are two entries.

use std::{
    collections::HashSet,
    fmt, fs,
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

/// The exit code of a run that asked for an address the frozen directory does not have (see [`NotFrozen`]). 1 is a read that
/// found nothing and 2 any other failure (see `output::exit_code`).
pub(crate) const EXIT_NOT_FROZEN: u8 = 3;

/// What the run does with the frozen directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Record,
    Replay,
}

struct Store {
    dir: PathBuf,
    mode: Mode,
    /// The keys this run has written, so a page read twice in a run is read the second time from what was written.
    written: Mutex<HashSet<String>>,
    /// The addresses a replay asked for that the directory does not have, in the order they were asked.
    missed: Mutex<Vec<String>>,
}

static STORE: OnceLock<Option<Store>> = OnceLock::new();

fn store() -> Option<&'static Store> {
    STORE.get().and_then(Option::as_ref)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads `JURL_FROZEN_DIR` and `JURL_FROZEN_MODE` once, at the start of a run. Neither set: the run is as it always was. A
/// directory with no mode, a mode with no directory or a mode that is not `record` or `replay` is an error, so a run that was
/// meant to be frozen is never a live one.
pub(crate) fn init() -> Result<()> {
    let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let found = from_env(env("JURL_FROZEN_DIR"), env("JURL_FROZEN_MODE"))?;
    let _ =
        STORE.set(found.map(|(dir, mode)| Store { dir, mode, written: Mutex::default(), missed: Mutex::default() }));
    Ok(())
}

/// The directory and mode the two variables name, or none when neither is set.
fn from_env(dir: Option<String>, mode: Option<String>) -> Result<Option<(PathBuf, Mode)>> {
    let mode = match (dir.as_deref(), mode.as_deref()) {
        (None, None) => return Ok(None),
        (None, Some(_)) => {
            bail!("JURL_FROZEN_MODE is set but JURL_FROZEN_DIR is not: it names the directory of the frozen pages")
        }
        (Some(_), None) => bail!("JURL_FROZEN_DIR is set but JURL_FROZEN_MODE is not: it is `record` or `replay`"),
        (Some(_), Some("record")) => Mode::Record,
        (Some(_), Some("replay")) => Mode::Replay,
        (Some(_), Some(other)) => bail!("JURL_FROZEN_MODE={other}: it is `record` or `replay`"),
    };
    let dir = PathBuf::from(dir.unwrap_or_default());
    match mode {
        Mode::Record => {
            fs::create_dir_all(&dir).with_context(|| format!("creating JURL_FROZEN_DIR {}", dir.display()))?
        }
        Mode::Replay if !dir.is_dir() => {
            bail!("JURL_FROZEN_DIR {} is not a directory: there is nothing to replay", dir.display())
        }
        Mode::Replay => {}
    }
    Ok(Some((dir, mode)))
}

pub(crate) fn recording() -> bool {
    store().is_some_and(|s| s.mode == Mode::Record)
}

pub(crate) fn replaying() -> bool {
    store().is_some_and(|s| s.mode == Mode::Replay)
}

/// The addresses a replay asked for that are not frozen, each once.
pub(crate) fn misses() -> Vec<String> {
    store().map(|s| lock(&s.missed).clone()).unwrap_or_default()
}

/// The error of an address the frozen directory does not have, said by a replay in place of a read of the web.
#[derive(Debug)]
pub(crate) struct NotFrozen {
    pub(crate) url: String,
}

impl fmt::Display for NotFrozen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not frozen: {} is not in JURL_FROZEN_DIR, and a replay never reads the web", self.url)
    }
}

impl std::error::Error for NotFrozen {}

/// The error a page's read ended with when it was recorded, said again by the replay. `unreached` is whether the site gave no
/// answer at all, which is what tries the `www.` alternate of a bare domain (see `fetch::unreached`).
#[derive(Debug)]
pub(crate) struct Recorded {
    pub(crate) message: String,
    pub(crate) unreached: bool,
}

impl fmt::Display for Recorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Recorded {}

/// Whether `err` says an address is not frozen.
pub(crate) fn is_not_frozen(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<NotFrozen>())
}

/// Whether `err` is a recorded failure of a site that gave no answer at all.
pub(crate) fn recorded_unreached(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<Recorded>().is_some_and(|r| r.unreached))
}

/// The error of a read the replay cannot make: the address is said as not frozen, and remembered, so a run that goes on without
/// the page (a `--follow` skips the page it cannot read) still ends with the exit code of a miss.
pub(crate) fn not_frozen(url: &str) -> anyhow::Error {
    if let Some(s) = store() {
        let mut missed = lock(&s.missed);
        if !missed.iter().any(|u| u == url) {
            missed.push(url.to_string());
        }
    }
    NotFrozen { url: url.to_string() }.into()
}

/// The error of a request that would use the network, in a replay. The clients and the render go through it.
pub(crate) fn no_network(url: &str) -> anyhow::Error {
    not_frozen(url)
}

/// An entry's key: the SHA-256 of the address and the Accept-Language the run asks with (nothing when it names none), as hex.
/// The address is as `Url::as_str` writes it.
pub(crate) fn key(url: &str, language: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(url.as_bytes());
    hash.update(language.unwrap_or_default().as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn key_of(url: &str) -> String {
    key(url, crate::fetch::accept_language().as_deref())
}

/// The name of a small file's entry. A page and a small file may be the same address (a site's `llms.txt` is a run's start
/// page, or a platform's endpoint is probed as a file and then read as a page), and they are read differently, so they are
/// kept apart: a page is `<key>`, as above, and a file is `<key>.small`.
fn small_of(url: &str) -> String {
    format!("{}.small", key_of(url))
}

/// What a page was when it reached the extractor.
pub(crate) struct Frozen {
    /// The address of the page as the run goes on with it: where redirects ended, and the HTML twin of a markdown page.
    pub(crate) url: Url,
    pub(crate) status: u16,
    pub(crate) content_type: String,
    /// The route the page was read on, as `fetch::Route::name` says it.
    pub(crate) route: String,
    /// Whether Lightpanda rendered it: its body is then the DOM, and the run does not render it again.
    pub(crate) rendered: bool,
    /// The body: the bytes that came, or the text the extractor was given when no bytes came (a render, the sidecar).
    pub(crate) body: Vec<u8>,
    /// Whether `body` is the text the extractor reads already (a DOM, the sidecar's page), not bytes to decode with the content type.
    pub(crate) decoded: bool,
}

fn extension(content_type: &str) -> &'static str {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    if essence.eq_ignore_ascii_case("application/pdf") { "pdf" } else { "html" }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    // A name of its own for each write: pages load side by side under `--follow`, and a page may be written twice at once.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!("tmp{}-{}", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// A UTC date and time as ISO 8601, from seconds since the epoch (Howard Hinnant's days-to-civil).
fn iso8601(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

fn now() -> String {
    iso8601(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// Writes the entry `key`: `meta` as its JSON, and `body` beside it. Both are written whole or not at all; the JSON goes last, so
/// an entry with a JSON has its body.
fn write_entry(s: &Store, key: &str, extension: &str, mut meta: Value, body: Option<&[u8]>) -> Result<()> {
    meta["frozen_at"] = json!(now());
    if let Some(body) = body {
        let file = format!("{key}.{extension}");
        atomic_write(&s.dir.join(&file), body)?;
        meta["body_file"] = json!(file);
    }
    atomic_write(&s.dir.join(format!("{key}.json")), serde_json::to_string_pretty(&meta)?.as_bytes())?;
    lock(&s.written).insert(key.to_string());
    Ok(())
}

fn read_meta(s: &Store, key: &str) -> Option<Value> {
    serde_json::from_slice(&fs::read(s.dir.join(format!("{key}.json"))).ok()?).ok()
}

fn read_body(s: &Store, meta: &Value) -> Result<Vec<u8>> {
    let file = meta["body_file"].as_str().context("a frozen entry with no body")?;
    // A name in the directory, never a path out of it.
    if file.contains(['/', '\\']) {
        bail!("a frozen entry names a file outside the directory: {file}");
    }
    fs::read(s.dir.join(file)).with_context(|| format!("reading the frozen body {file}"))
}

/// Records the page a load read, as `url` asked for it: what reached the extractor, with `served`'s route and render. A failure to
/// write is said on stderr and the run goes on.
pub(crate) fn record_page(url: &Url, page: &Frozen) {
    let Some(s) = store().filter(|s| s.mode == Mode::Record) else { return };
    let meta = json!({
        "kind": "page",
        "url": url.as_str(),
        "accept_language": crate::fetch::accept_language(),
        "final_url": page.url.as_str(),
        "status": page.status,
        "content_type": page.content_type,
        "route": page.route,
        "rendered": page.rendered,
        "decoded": page.decoded,
    });
    if let Err(e) = write_entry(s, &key_of(url.as_str()), extension(&page.content_type), meta, Some(&page.body)) {
        eprintln!("jurl: could not freeze {url}: {e:#}");
    }
}

/// Records that the read of `url` failed, with its message, so a replay fails the same way.
pub(crate) fn record_error(url: &Url, error: &anyhow::Error, unreached: bool) {
    let Some(s) = store().filter(|s| s.mode == Mode::Record) else { return };
    let meta = json!({
        "kind": "page",
        "url": url.as_str(),
        "accept_language": crate::fetch::accept_language(),
        "error": format!("{error:#}"),
        "unreached": unreached,
    });
    if let Err(e) = write_entry(s, &key_of(url.as_str()), "html", meta, None) {
        eprintln!("jurl: could not freeze the failure of {url}: {e:#}");
    }
}

/// The frozen page for `url`, in a replay: the page, the error its read ended with, or [`NotFrozen`] when the directory has no
/// entry for it. `None` when the run is not a replay.
pub(crate) fn replay_page(url: &Url) -> Option<Result<Frozen>> {
    let s = store().filter(|s| s.mode == Mode::Replay)?;
    Some(read_page(s, url))
}

/// The page `url` was frozen as in this run, when this run has written it (a record run), and the frozen page when it replays: a
/// second read of a page in one run is the page the extractor had, not another read of the web.
pub(crate) fn again_page(url: &Url) -> Option<Result<Frozen>> {
    let s = store()?;
    if s.mode == Mode::Record && !lock(&s.written).contains(&key_of(url.as_str())) {
        return None;
    }
    Some(read_page(s, url))
}

fn read_page(s: &Store, url: &Url) -> Result<Frozen> {
    let key = key_of(url.as_str());
    let Some(meta) = read_meta(s, &key).filter(|m| m["kind"] == "page") else { return Err(not_frozen(url.as_str())) };
    if let Some(message) = meta["error"].as_str() {
        return Err(
            Recorded { message: message.to_string(), unreached: meta["unreached"].as_bool().unwrap_or(false) }.into()
        );
    }
    let text = |name: &str| meta[name].as_str().unwrap_or_default().to_string();
    Ok(Frozen {
        url: Url::parse(&text("final_url")).with_context(|| format!("the frozen entry of {url} names no address"))?,
        status: meta["status"].as_u64().and_then(|n| u16::try_from(n).ok()).unwrap_or(200),
        content_type: text("content_type"),
        route: text("route"),
        rendered: meta["rendered"].as_bool().unwrap_or(false),
        decoded: meta["decoded"].as_bool().unwrap_or(false),
        body: read_body(s, &meta)?,
    })
}

/// Records a small file (robots.txt, a sitemap, a search description): its text, or that the site has none, with the status the
/// site answered when there is one.
pub(crate) fn record_small(url: &Url, status: Option<u16>, body: Option<&str>) {
    let Some(s) = store().filter(|s| s.mode == Mode::Record) else { return };
    let meta = json!({
        "kind": "small",
        "url": url.as_str(),
        "accept_language": crate::fetch::accept_language(),
        "status": status,
        "absent": body.is_none(),
    });
    if let Err(e) = write_entry(s, &small_of(url.as_str()), "html", meta, body.map(str::as_bytes)) {
        eprintln!("jurl: could not freeze {url}: {e:#}");
    }
}

/// The small file frozen for `url`, in a replay: its text, or none when the site had none. A file that is not frozen is none too,
/// said on stderr and remembered, so the run ends as a miss does (see [`not_frozen`]). `None` outside a replay.
pub(crate) fn replay_small(url: &Url) -> Option<Option<String>> {
    let s = store().filter(|s| s.mode == Mode::Replay)?;
    let found = read_meta(s, &small_of(url.as_str())).filter(|m| m["kind"] == "small");
    let Some(meta) = found else {
        eprintln!("jurl: {:#}", not_frozen(url.as_str()));
        return Some(None);
    };
    if meta["absent"].as_bool().unwrap_or(false) {
        return Some(None);
    }
    match read_body(s, &meta) {
        Ok(body) => Some(Some(String::from_utf8_lossy(&body).into_owned())),
        Err(e) => {
            eprintln!("jurl: {e:#}");
            Some(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_the_sha256_of_the_address_and_the_language() {
        // sha256("abc")
        assert_eq!(key("a", Some("bc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(key("abc", None), key("a", Some("bc")));
        assert_ne!(key("https://x.com/", None), key("https://x.com/", Some("es")));
    }

    #[test]
    fn the_mode_is_one_of_two_and_comes_with_a_directory() {
        let dir = std::env::temp_dir();
        let dir_s = || Some(dir.to_string_lossy().into_owned());
        assert!(from_env(None, None).unwrap().is_none());
        assert_eq!(from_env(dir_s(), Some("replay".into())).unwrap().map(|(_, m)| m), Some(Mode::Replay));
        assert!(from_env(dir_s(), None).is_err(), "a directory with no mode must not be a live run");
        assert!(from_env(None, Some("record".into())).is_err());
        assert!(from_env(dir_s(), Some("live".into())).is_err());
        assert!(from_env(Some("/no/such/dir/for/jurl".into()), Some("replay".into())).is_err());
    }

    #[test]
    fn a_date_is_written_in_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_764_000_000 + 3_661), "2025-11-24T17:01:01Z");
    }

    #[test]
    fn a_pdf_is_kept_with_its_own_extension() {
        assert_eq!(extension("application/pdf"), "pdf");
        assert_eq!(extension("Application/PDF; charset=binary"), "pdf");
        assert_eq!(extension("text/html; charset=utf-8"), "html");
        assert_eq!(extension(""), "html");
    }
}
