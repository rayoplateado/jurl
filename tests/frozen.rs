//! Frozen pages (`JURL_FROZEN_DIR`, `JURL_FROZEN_MODE`): the jurl binary reads against a local site, records what it read, and
//! reads it again from the directory with the site gone. Each test runs the binary as a run does, so what it checks is the run.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// One thing a site answers a path with: status, content type and body.
type Route = (u16, &'static str, Vec<u8>);

/// A local site. A path it has no route for is a 404, as a real site's missing robots.txt is. `stop` closes it, so that what is
/// read after it is read from somewhere else.
struct Site {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Site {
    fn start(routes: Vec<(&'static str, Route)>) -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        // `{base}` in a body is the site's own address, which is not known until it is bound.
        let base = format!("http://{addr}");
        let routes: BTreeMap<&str, Route> = routes
            .into_iter()
            .map(|(path, (status, content_type, body))| {
                // Only a text body has the placeholder; a binary one is served as it is.
                let body = match String::from_utf8(body) {
                    Ok(text) => text.replace("{base}", &base).into_bytes(),
                    Err(binary) => binary.into_bytes(),
                };
                (path, (status, content_type, body))
            })
            .collect();
        let (log, stop) = (hits.clone(), stopping.clone());
        let thread = thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut conn) = conn else { continue };
                let (head, _) = read_request(&mut conn);
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(path.clone());
                // A route that ends in `*` answers every path that starts as it does.
                let found = routes.get(path.as_str()).or_else(|| {
                    routes
                        .iter()
                        .find(|(r, _)| r.strip_suffix('*').is_some_and(|prefix| path.starts_with(prefix)))
                        .map(|(_, v)| v)
                });
                let (status, content_type, body) = found.cloned().unwrap_or((404, "text/plain", b"not found".to_vec()));
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(&body);
            }
        });
        Site { addr, hits, stopping, thread: Some(thread) }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    /// Closes the site: its port refuses every connection from now on.
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the site's thread");
        }
    }
}

/// A request's head and body, read from `conn`.
fn read_request(conn: &mut TcpStream) -> (String, Vec<u8>) {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = conn.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break data.len();
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = data[end..].to_vec();
    while body.len() < length {
        let n = conn.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    (head, body)
}

/// A stand-in for the model (Jev): it scores a block 0.9 when it holds `needle`, and a link and a page's kind as a reader would
/// want them for this site, so the same page gives the same requests and the same answer each time. It keeps the requests.
struct Model {
    url: String,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl Model {
    fn start(needle: &'static str) -> Model {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let url = format!("http://{}/v1/systemone", listener.local_addr().expect("its address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let (head, body) = read_request(&mut conn);
                let reply = if head.starts_with("POST") {
                    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    log.lock().unwrap().push(request.clone());
                    answer(&request, needle).to_string()
                } else {
                    String::new()
                };
                let _ = write!(
                    conn,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Model { url, seen }
    }

    /// The requests the model got, each as canonical JSON, sorted: a replay asks the same questions, not necessarily in the
    /// same order.
    fn requests(&self) -> Vec<String> {
        let mut all: Vec<String> = self.seen.lock().unwrap().iter().map(Value::to_string).collect();
        all.sort();
        all
    }
}

fn answer(request: &Value, needle: &str) -> Value {
    let blocks = request["state"]["blocks"].as_array().cloned().unwrap_or_default();
    let mut answers = serde_json::Map::new();
    for (id, question) in request["questions"].as_object().into_iter().flatten() {
        let score = |p: f64| json!({ "noul": p });
        let value = if let Some(criteria) = question["criteria"].as_object() {
            json!({ "choice": criteria.keys().next().cloned().unwrap_or_default(), "confidence": 0.5 })
        } else if let Some(n) = id.strip_prefix('b').and_then(|n| n.parse::<usize>().ok()) {
            let holds = blocks
                .iter()
                .any(|b| b["i"].as_u64() == Some(n as u64) && b["text"].as_str().is_some_and(|t| t.contains(needle)));
            score(if holds { 0.95 } else { 0.02 })
        } else if id.starts_with('l') {
            score(0.9)
        } else {
            score(0.01)
        };
        answers.insert(id.clone(), value);
    }
    json!({ "answers": answers, "usage": { "input_tokens": 10 } })
}

/// A directory of the test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jurl-frozen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a test directory");
    dir
}

/// A run of jurl with nothing in its environment but what the test names, in an empty directory, so nothing a developer has set
/// (keys, proxies, a config file) is in it.
fn jurl(home: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut run = Command::new(env!("CARGO_BIN_EXE_jurl"));
    run.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .current_dir(home);
    for (k, v) in env {
        run.env(k, v);
    }
    run.args(args).output().expect("jurl runs")
}

fn stdout_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("jurl printed JSON ({e}): {}\n{}", String::from_utf8_lossy(&out.stdout), stderr_of(out))
    })
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// What a run printed, without what it counted off the wire (the bytes read, the plain requests the sites refused): a replay
/// reads no bytes and asks no site, so its usage does not have them.
fn printed(out: &Output) -> Value {
    let mut json = stdout_of(out);
    for counter in ["bytes", "plain_refusals"] {
        json["usage"].as_object_mut().expect("a usage").remove(counter);
    }
    json
}

/// The entries of the directory of one `kind` (`page` or `small`), by the address they are for.
fn entries_of(dir: &Path, kind: &str) -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("the frozen directory").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if meta["kind"] == kind {
                assert!(
                    found.insert(meta["url"].as_str().unwrap().to_string(), meta).is_none(),
                    "one entry of a kind per address"
                );
            }
        }
    }
    found
}

fn entries(dir: &Path) -> BTreeMap<String, Value> {
    entries_of(dir, "page")
}

fn key(url: &str) -> String {
    Sha256::digest(url.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A model that is never asked: a run that only loads pages needs the address of one all the same.
const NO_MODEL: &str = "http://127.0.0.1:9/v1/systemone";

const PHONE_PAGE: &str = "<html><head><title>Acme</title></head><body><h1>Acme</h1><p>Welcome to Acme, where anvils and rockets are sold every weekday of the year.</p><p>Phone us on 555-0100 between nine and six.</p></body></html>";

fn html(body: &str) -> Route {
    (200, "text/html", body.as_bytes().to_vec())
}

#[test]
fn a_simple_page_is_recorded_and_replayed_with_the_same_answer_and_the_site_gone() {
    let home = scratch("simple");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let mut site = Site::start(vec![("/", html(PHONE_PAGE))]);
    let url = format!("{}/", site.base());
    let ask = ["--json", "-t", "-q", "what is the phone number?", &url];
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };

    let recorded = jurl(&home, &env("record"), &ask);
    assert!(recorded.status.success(), "{}", stderr_of(&recorded));
    assert_eq!(stdout_of(&recorded)["blocks"][1]["text"], "Phone us on 555-0100 between nine and six.");
    let asked_live = model.requests();
    assert!(!asked_live.is_empty());

    let meta = &entries(&frozen)[&url];
    assert_eq!(meta["status"], 200);
    assert_eq!(meta["content_type"], "text/html");
    assert_eq!(meta["rendered"], false);
    assert!(meta["frozen_at"].as_str().is_some_and(|d| d.ends_with('Z')));
    let name = key(&url);
    assert_eq!(std::fs::read_to_string(frozen.join(format!("{name}.html"))).unwrap(), PHONE_PAGE);

    site.stop();
    model.seen.lock().unwrap().clear();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert!(replayed.status.success(), "{}", stderr_of(&replayed));
    assert_eq!(printed(&replayed), printed(&recorded), "a replay prints what the recording printed");
    assert_eq!(model.requests(), asked_live, "and asks the model the same");
}

#[test]
fn a_run_without_the_variables_is_the_run_it_was_and_writes_nothing() {
    let home = scratch("off");
    let model = Model::start("555-0100");
    let site = Site::start(vec![("/", html(PHONE_PAGE))]);
    let url = format!("{}/", site.base());
    let ask = ["--json", "-q", "what is the phone number?", &url];

    let plain = jurl(&home, &[("JURL_JEV_URL", &model.url)], &ask);
    assert!(plain.status.success(), "{}", stderr_of(&plain));
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "a run with no frozen directory writes no file");

    let frozen = home.join("frozen");
    let env = [
        ("JURL_JEV_URL", model.url.as_str()),
        ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
        ("JURL_FROZEN_MODE", "record"),
    ];
    let recorded = jurl(&home, &env, &ask);
    assert_eq!(printed(&recorded), printed(&plain), "recording changes nothing a run prints");
    assert!(site.hits().iter().all(|p| p == "/"), "{:?}", site.hits());
}

#[test]
fn a_directory_without_a_mode_is_an_error_and_never_a_live_read() {
    let home = scratch("nomode");
    let model = Model::start("x");
    let site = Site::start(vec![("/", html(PHONE_PAGE))]);
    let url = format!("{}/", site.base());
    let frozen = home.join("frozen");
    for env in [
        vec![("JURL_JEV_URL", model.url.as_str()), ("JURL_FROZEN_DIR", frozen.to_str().unwrap())],
        vec![
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", "live"),
        ],
        vec![("JURL_JEV_URL", model.url.as_str()), ("JURL_FROZEN_MODE", "replay")],
    ] {
        let out = jurl(&home, &env, &["--json", "-q", "phone?", &url]);
        assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
        assert!(stderr_of(&out).contains("JURL_FROZEN"), "{}", stderr_of(&out));
    }
    assert!(site.hits().is_empty(), "nothing was read: {:?}", site.hits());
}

#[cfg(unix)]
#[test]
fn a_rendered_page_is_replayed_as_the_dom_it_was_and_is_not_rendered_again() {
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("rendered");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let shell = r#"<html><head><title>App</title></head><body><div id="root"></div><script src="/app.js"></script></body></html>"#;
    let mut site = Site::start(vec![("/", html(shell))]);
    // The stand-in for Lightpanda: it keeps a line for each time it runs, and answers with the page after its scripts ran.
    let log = home.join("renders.log");
    let bin = home.join("lightpanda");
    let dom = format!(r#"{{"http_status":200,"headers":[],"content":"{}"}}"#, PHONE_PAGE.replace('"', "\\\""));
    std::fs::write(&bin, format!("#!/bin/sh\necho run >> '{}'\nprintf '%s' '{dom}'\n", log.display())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let url = format!("{}/", site.base());
    let ask = ["--json", "-q", "what is the phone number?", &url];
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_LIGHTPANDA", bin.to_str().unwrap()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };

    let recorded = jurl(&home, &env("record"), &ask);
    assert!(recorded.status.success(), "{}", stderr_of(&recorded));
    assert_eq!(stdout_of(&recorded)["usage"]["route"], "direct+render");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "the page is rendered once when it is recorded"
    );
    let meta = &entries(&frozen)[&url];
    assert_eq!(meta["rendered"], true);
    assert_eq!(
        std::fs::read_to_string(frozen.join(format!("{}.html", key(&url)))).unwrap(),
        PHONE_PAGE,
        "what is frozen is the DOM the extractor read, not the shell the site sent"
    );

    site.stop();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert!(replayed.status.success(), "{}", stderr_of(&replayed));
    assert_eq!(printed(&replayed), printed(&recorded));
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1, "the replay does not render it again");
}

#[test]
fn a_pdf_is_frozen_as_the_bytes_that_came_and_replayed_the_same() {
    let home = scratch("pdf");
    let frozen = home.join("frozen");
    let mut pdf = b"%PDF-1.4\n1 0 obj\n<< /Length 12 >>\nstream\n".to_vec();
    pdf.extend([0xff, 0xfe, 0x00, 0x80, 0xc3, 0x28, 0x00, 0x0a]);
    pdf.extend(b"Phone us on 555-0100 between nine and six, every weekday of the year.\nendstream\n%%EOF\n");
    let mut site = Site::start(vec![("/menu.pdf", (200, "application/pdf", pdf.clone()))]);
    let url = format!("{}/menu.pdf", site.base());
    let ask = ["--json", "--with-pages", "--pages-only", &url];
    let env =
        |mode| [("JURL_JEV_URL", NO_MODEL), ("JURL_FROZEN_DIR", frozen.to_str().unwrap()), ("JURL_FROZEN_MODE", mode)];

    let recorded = jurl(&home, &env("record"), &ask);
    let pages = stdout_of(&recorded)["pages"].clone();
    assert!(!pages.as_array().unwrap().is_empty(), "the page was read: {}", stderr_of(&recorded));
    let meta = &entries(&frozen)[&url];
    assert_eq!(meta["content_type"], "application/pdf");
    assert_eq!(
        std::fs::read(frozen.join(format!("{}.pdf", key(&url)))).unwrap(),
        pdf,
        "the bytes are frozen as they came, not as the text a decoder made of them"
    );

    site.stop();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert_eq!(replayed.status.code(), recorded.status.code(), "{}", stderr_of(&replayed));
    assert_eq!(printed(&replayed), printed(&recorded));
}

#[test]
fn a_follow_of_two_pages_freezes_the_pages_and_the_files_the_site_does_not_have() {
    let home = scratch("follow");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let start = "<html><head><title>Shop</title></head><body><h1>Acme Shop</h1><p>Welcome to the Acme shop, where we sell anvils and rockets.</p><a href=\"/about\">About us and contact details</a></body></html>";
    let about = "<html><head><title>About</title></head><body><h1>About</h1><p>Phone us on 555-0100 between nine and six, every weekday.</p></body></html>";
    let mut site = Site::start(vec![
        ("/", html(start)),
        ("/about", html(about)),
        ("/robots.txt", (200, "text/plain", b"User-agent: *\nDisallow: /private\n".to_vec())),
    ]);
    let url = format!("{}/", site.base());
    let ask = ["--json", "--follow", "2", "-q", "what is the phone number?", &url];
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };

    let recorded = jurl(&home, &env("record"), &ask);
    assert!(recorded.status.success(), "{}", stderr_of(&recorded));
    let path = stdout_of(&recorded)["path"].clone();
    assert_eq!(path.as_array().unwrap().len(), 2, "the follow read two pages: {path}");
    let asked_live = model.requests();

    let (kept, files) = (entries(&frozen), entries_of(&frozen, "small"));
    let base = site.base();
    for page in ["/", "/about"] {
        assert!(kept.contains_key(&format!("{base}{page}")), "{page}");
    }
    assert_eq!(files[&format!("{base}/robots.txt")]["status"], 200);
    for missing in ["/llms.txt", "/sitemap.xml"] {
        let entry =
            files.get(&format!("{base}{missing}")).unwrap_or_else(|| panic!("{missing} is frozen: {:?}", files.keys()));
        assert_eq!((entry["absent"].as_bool(), entry["status"].as_u64()), (Some(true), Some(404)), "{missing}");
    }
    let hits_when_recorded = site.hits().len();

    site.stop();
    model.seen.lock().unwrap().clear();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert!(replayed.status.success(), "{}", stderr_of(&replayed));
    assert!(!stderr_of(&replayed).contains("not frozen"), "{}", stderr_of(&replayed));
    assert_eq!(printed(&replayed), printed(&recorded), "a followed site is replayed whole");
    assert_eq!(model.requests(), asked_live);
    assert_eq!(site.hits().len(), hits_when_recorded, "the replay asked the site nothing");
}

#[test]
fn an_address_that_is_not_frozen_is_its_own_error_and_never_a_live_read() {
    let home = scratch("absent");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let site = Site::start(vec![("/", html(PHONE_PAGE)), ("/other", html(PHONE_PAGE))]);
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };
    let known = format!("{}/", site.base());
    assert!(jurl(&home, &env("record"), &["--json", "-q", "phone?", &known]).status.success());
    let hits = site.hits().len();

    let other = format!("{}/other", site.base());
    let out = jurl(&home, &env("replay"), &["--json", "-q", "phone?", &other]);
    assert_eq!(out.status.code(), Some(3), "its own exit code: {}", stderr_of(&out));
    let said = stderr_of(&out);
    assert!(said.contains("not frozen") && said.contains(&other), "{said}");
    assert_eq!(site.hits().len(), hits, "the site, which is up, was not asked");

    let empty = home.join("empty");
    std::fs::create_dir(&empty).unwrap();
    let out = jurl(
        &home,
        &[("JURL_JEV_URL", &model.url), ("JURL_FROZEN_DIR", empty.to_str().unwrap()), ("JURL_FROZEN_MODE", "replay")],
        &["--json", "-q", "phone?", &known],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr_of(&out));
    assert_eq!(site.hits().len(), hits);
}

/// Freezes `url` by hand: its entry, and `body` beside it when there is one.
fn freeze_by_hand(dir: &Path, url: &str, mut meta: Value, body: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    let name = key(url);
    meta["kind"] = json!("page");
    meta["url"] = json!(url);
    if let Some(body) = body {
        std::fs::write(dir.join(format!("{name}.html")), body).unwrap();
        meta["body_file"] = json!(format!("{name}.html"));
        meta["final_url"] = json!(url);
        meta["status"] = json!(200);
        meta["content_type"] = json!("text/html");
        meta["route"] = json!("direct");
    }
    std::fs::write(dir.join(format!("{name}.json")), meta.to_string()).unwrap();
}

#[test]
fn a_replay_skips_the_check_of_a_public_address_and_reads_a_site_that_is_gone() {
    let home = scratch("public");
    let frozen = home.join("frozen");
    // A page of a name that resolves nowhere (`.invalid` never does), frozen by hand.
    let url = "https://gone.invalid/";
    freeze_by_hand(&frozen, url, json!({}), Some(PHONE_PAGE));
    let ask = ["--json", "--with-pages", "--pages-only", url];

    let live = jurl(&home, &[("JURL_JEV_URL", NO_MODEL), ("JURL_PUBLIC_ONLY", "1")], &ask);
    assert_eq!(live.status.code(), Some(2), "a run that is public-only cannot read a name that does not resolve");

    let env = [
        ("JURL_JEV_URL", NO_MODEL),
        ("JURL_PUBLIC_ONLY", "1"),
        ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
        ("JURL_FROZEN_MODE", "replay"),
    ];
    let replayed = jurl(&home, &env, &ask);
    let json = stdout_of(&replayed);
    assert_eq!(json["pages"][0]["url"], url, "{}", stderr_of(&replayed));
    assert_eq!(json["pages"][0]["blocks"][2]["text"], "Phone us on 555-0100 between nine and six.");
}

#[test]
fn a_bare_domain_that_gave_no_answer_is_replayed_as_the_www_page_that_was_read_instead() {
    let home = scratch("www");
    let frozen = home.join("frozen");
    freeze_by_hand(
        &frozen,
        "https://acme-frozen.example/",
        json!({ "error": "acme-frozen.example gave no answer", "unreached": true }),
        None,
    );
    freeze_by_hand(&frozen, "https://www.acme-frozen.example/", json!({}), Some(PHONE_PAGE));
    let env =
        [("JURL_JEV_URL", NO_MODEL), ("JURL_FROZEN_DIR", frozen.to_str().unwrap()), ("JURL_FROZEN_MODE", "replay")];

    let out = jurl(&home, &env, &["--json", "--with-pages", "--pages-only", "acme-frozen.example"]);
    assert_eq!(stdout_of(&out)["pages"][0]["url"], "https://www.acme-frozen.example/", "{}", stderr_of(&out));

    // A failure that is not "no answer at all" is the page's error, and the www page is not tried.
    freeze_by_hand(
        &frozen,
        "https://refused-frozen.example/",
        json!({ "error": "HTTP 403", "unreached": false }),
        None,
    );
    freeze_by_hand(&frozen, "https://www.refused-frozen.example/", json!({}), Some(PHONE_PAGE));
    let out = jurl(&home, &env, &["--json", "--with-pages", "--pages-only", "refused-frozen.example"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    assert!(stderr_of(&out).contains("HTTP 403"), "{}", stderr_of(&out));
}

#[test]
fn a_page_and_a_file_at_one_address_are_two_entries() {
    let home = scratch("same-address");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let llms = "Acme. Phone us on 555-0100 between nine and six, every weekday of the year.";
    let mut site = Site::start(vec![("/llms.txt", (200, "text/plain", llms.as_bytes().to_vec()))]);
    // The start page of this run is the site's `llms.txt`, which a `--follow` reads as the site's file too.
    let url = format!("{}/llms.txt", site.base());
    let ask = ["--json", "--follow", "2", "-q", "what is the phone number?", &url];
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };

    let recorded = jurl(&home, &env("record"), &ask);
    assert!(entries(&frozen).contains_key(&url) && entries_of(&frozen, "small").contains_key(&url));

    site.stop();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert!(!stderr_of(&replayed).contains("not frozen"), "{}", stderr_of(&replayed));
    assert_eq!(replayed.status.code(), recorded.status.code());
    assert_eq!(printed(&replayed), printed(&recorded));
}

#[test]
fn a_precise_follow_with_the_sites_search_and_a_page_that_fails_is_replayed_the_same() {
    let home = scratch("precise");
    let frozen = home.join("frozen");
    let model = Model::start("555-0100");
    let start = r#"<html><head><title>Shop</title><link rel="https://api.w.org/" href="{base}/wp-json/"></head><body><h1>Acme Shop</h1><p>Welcome to the Acme shop, where we sell anvils and rockets.</p><a href="/broken">Our broken page about anvils</a><a href="/missing">Another page about rockets</a></body></html>"#;
    let about = "<html><head><title>Contact</title></head><body><h1>Contact</h1><p>Phone us on 555-0100 between nine and six, every weekday.</p></body></html>";
    let mut site = Site::start(vec![
        ("/", html(start)),
        ("/broken", (500, "text/plain", b"boom".to_vec())),
        (
            "/wp-json/wp/v2/search*",
            (200, "application/json", br#"[{"url":"{base}/about","title":"Contact"}]"#.to_vec()),
        ),
        ("/about", html(about)),
    ]);
    let url = format!("{}/", site.base());
    let ask = ["--json", "--precise", "--follow", "6", "-q", "what is the phone number?", &url];
    let env = |mode| {
        [
            ("JURL_JEV_URL", model.url.as_str()),
            ("JURL_FROZEN_DIR", frozen.to_str().unwrap()),
            ("JURL_FROZEN_MODE", mode),
        ]
    };

    let recorded = jurl(&home, &env("record"), &ask);
    let kept = entries(&frozen);
    let base = site.base();
    assert!(
        kept[&format!("{base}/broken")]["error"].as_str().is_some_and(|e| e.contains("500")),
        "a page that fails is frozen with its error: {kept:?}"
    );
    let files = entries_of(&frozen, "small");
    assert!(
        files.keys().any(|u| u.contains("/wp-json/wp/v2/search")),
        "the site's search is frozen: {:?}",
        files.keys()
    );
    assert!(
        kept.contains_key(&format!("{base}/about")),
        "the page its search gave is read: {:?}\n{}",
        kept.keys(),
        stderr_of(&recorded)
    );
    let asked_live = model.requests();

    site.stop();
    model.seen.lock().unwrap().clear();
    let replayed = jurl(&home, &env("replay"), &ask);
    assert!(!stderr_of(&replayed).contains("not frozen"), "{}", stderr_of(&replayed));
    assert_eq!(replayed.status.code(), recorded.status.code(), "{}", stderr_of(&replayed));
    assert_eq!(printed(&replayed), printed(&recorded));
    assert_eq!(model.requests(), asked_live);
}
