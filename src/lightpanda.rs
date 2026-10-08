//! Lightpanda runs pages' JavaScript. It is AGPL-3.0 and ~90–190 MB, so jurl never
//! bundles it: the first time a page needs it, jurl downloads a pinned release from
//! the official GitHub repo and checks its SHA-256 before ever running it.

use std::{
    env, fs,
    io::{IsTerminal, Write, stderr},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::Client;
use sha2::{Digest, Sha256};

pub const VERSION: &str = "1.0.0";

/// Release asset and its SHA-256 for this platform (from the 1.0.0 release digests).
fn asset() -> Option<(&'static str, &'static str)> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => {
            ("lightpanda-aarch64-macos", "955440053a84754dd64c62f970449a56a2b350cdf43ea5f2e809a73047b8173d")
        }
        ("macos", "x86_64") => {
            ("lightpanda-x86_64-macos", "e510299683b37a203912eac0ee00732224b2ef9b07fe58e69c467f5255be45e2")
        }
        ("linux", "aarch64") => {
            ("lightpanda-aarch64-linux", "69791924bcee43b13b224af4c845622c5fe66fdbc1b8143bfaa39ca8f85244f5")
        }
        ("linux", "x86_64") => {
            ("lightpanda-x86_64-linux", "aa5a4b8ed53d1e38b3c73f5b2647d0a84a82e6744557f45f9a9c85858aa031c3")
        }
        _ => return None,
    })
}

/// `~/Library/Caches/jurl` on macOS, `$XDG_CACHE_HOME/jurl` or `~/.cache/jurl` elsewhere.
fn cache_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from);
    if env::consts::OS == "macos" {
        return home.map(|h| h.join("Library/Caches/jurl"));
    }
    env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".cache"))).map(|d| d.join("jurl"))
}

fn cached() -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("lightpanda-{VERSION}")))
}

/// An existing Lightpanda: `$JURL_LIGHTPANDA`, `lightpanda` on PATH, `~/.local/bin`, or jurl's cache.
pub fn find(configured: Option<String>) -> Option<PathBuf> {
    if let Some(p) = configured {
        return Some(PathBuf::from(p));
    }
    let mut dirs: Vec<PathBuf> = env::var_os("PATH").map(|p| env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join("lightpanda")).chain(cached()).find(|p| p.is_file())
}

/// Find Lightpanda, downloading it once if needed.
pub async fn ensure(configured: Option<String>) -> Result<PathBuf> {
    if let Some(p) = find(configured) {
        return Ok(p);
    }
    let Some((name, sha)) = asset() else {
        bail!(
            "this page needs JavaScript, and Lightpanda has no build for {}/{}; see https://lightpanda.io",
            env::consts::OS,
            env::consts::ARCH
        );
    };
    if env::var_os("JURL_NO_DOWNLOAD").is_some() {
        bail!("this page needs JavaScript: install Lightpanda (https://lightpanda.io) or unset JURL_NO_DOWNLOAD");
    }
    let dest = cached().context("no cache directory ($HOME is not set)")?;
    download(name, sha, &dest).await?;
    Ok(dest)
}

/// Where a download is written before it is verified: `lightpanda-1.0.0.part`. `with_extension` would replace the `.0`.
fn part_path(dest: &Path) -> PathBuf {
    dest.with_added_extension("part")
}

async fn download(name: &str, sha: &str, dest: &PathBuf) -> Result<()> {
    let url = format!("https://github.com/lightpanda-io/browser/releases/download/{VERSION}/{name}");
    // No overall timeout: this is a big file on an unknown connection.
    let client = Client::builder().connect_timeout(Duration::from_secs(15)).build()?;
    let mut res = client.get(&url).send().await?.error_for_status().with_context(|| format!("downloading {url}"))?;
    let total = res.content_length().unwrap_or(0);
    let tty = stderr().is_terminal();
    let mb = |b: u64| b / 1_000_000;
    eprint!("jurl: this page needs JavaScript; downloading Lightpanda {VERSION} ({} MB, once)…", mb(total));

    fs::create_dir_all(dest.parent().unwrap())?;
    let part = part_path(dest);
    let mut file = fs::File::create(&part)?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    while let Some(chunk) = res.chunk().await? {
        hash.update(&chunk);
        file.write_all(&chunk)?;
        got += chunk.len() as u64;
        if tty && total > 0 {
            eprint!(
                "\rjurl: this page needs JavaScript; downloading Lightpanda {VERSION} ({} MB, once)… {:>3}%",
                mb(total),
                got * 100 / total
            );
        }
    }
    drop(file);

    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != sha {
        let _ = fs::remove_file(&part);
        eprintln!();
        bail!("Lightpanda download failed verification (sha256 {digest}, expected {sha}); nothing was installed");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&part, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&part, dest)?;
    eprintln!(" done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_download_keeps_the_full_name() {
        assert_eq!(part_path(Path::new("/cache/lightpanda-1.0.0")), PathBuf::from("/cache/lightpanda-1.0.0.part"));
    }
}
