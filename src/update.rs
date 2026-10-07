//! `jurl update`: install the latest release the same way this copy was installed. jurl never checks for
//! updates on its own; it only looks when asked, or when it doesn't know a flag it was given.

use std::{path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{Client, redirect::Policy};

const REPO: &str = "https://github.com/rayoplateado/jurl";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How this copy was installed, read from where the binary lives.
#[derive(Debug, PartialEq)]
enum Install {
    Homebrew,
    Cargo,
    Installer,
}

impl Install {
    fn detect(exe: &Path) -> Self {
        let path = exe.to_string_lossy().replace('\\', "/");
        if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/") {
            Install::Homebrew
        } else if path.contains("/.cargo/bin/") {
            Install::Cargo
        } else {
            Install::Installer
        }
    }

    fn command(&self) -> (&'static str, Vec<String>) {
        match self {
            Install::Homebrew => ("brew", vec!["upgrade".into(), "rayoplateado/tap/jurl".into()]),
            Install::Cargo => ("cargo", vec!["install".into(), "--git".into(), REPO.into(), "--force".into()]),
            Install::Installer if cfg!(windows) => (
                "powershell",
                vec![
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-c".into(),
                    format!("irm {REPO}/releases/latest/download/jurl-installer.ps1 | iex"),
                ],
            ),
            Install::Installer => (
                "sh",
                vec![
                    "-c".into(),
                    format!("curl --proto '=https' --tlsv1.2 -LsSf {REPO}/releases/latest/download/jurl-installer.sh | sh"),
                ],
            ),
        }
    }
}

/// The latest release's version, from where GitHub's /releases/latest redirects to (no API, no rate limit).
pub async fn latest(timeout: Duration) -> Result<String> {
    let client = Client::builder().redirect(Policy::none()).timeout(timeout).user_agent(concat!("jurl/", env!("CARGO_PKG_VERSION"))).build()?;
    let res = client.get(format!("{REPO}/releases/latest")).send().await?;
    let to = res.headers().get("location").and_then(|l| l.to_str().ok()).context("no latest release")?;
    let tag = to.rsplit('/').next().unwrap_or_default();
    Ok(tag.trim_start_matches('v').to_string())
}

/// Whether `a` is a later version than `b` ("0.1.10" > "0.1.9").
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a) > parts(b)
}

/// A line to add to an "unexpected argument" error when a newer jurl is out, which may know the flag.
pub async fn hint() -> Option<String> {
    let latest = latest(Duration::from_secs(3)).await.ok()?;
    newer(&latest, CURRENT).then(|| format!("you have jurl {CURRENT} and {latest} is out, which may have it: run `jurl update`"))
}

pub async fn run() -> Result<()> {
    let latest = latest(Duration::from_secs(10)).await.context("couldn't reach GitHub to check the latest release")?;
    if !newer(&latest, CURRENT) {
        eprintln!("jurl {CURRENT} is the latest.");
        return Ok(());
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).context("couldn't find where jurl is installed")?;
    let install = Install::detect(&exe);
    let (program, args) = install.command();
    eprintln!("jurl {CURRENT} → {latest}");
    eprintln!("$ {program} {}", args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" "));
    let status = Command::new(program).args(&args).status().with_context(|| format!("couldn't run {program}"))?;
    if !status.success() {
        bail!("the update didn't finish ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.1.4", "0.1.0"));
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("0.1.4", "0.1.4"));
        assert!(!newer("0.1.3", "0.1.4"));
    }

    #[test]
    fn install_method_from_path() {
        assert_eq!(Install::detect(Path::new("/opt/homebrew/Cellar/jurl/0.1.4/bin/jurl")), Install::Homebrew);
        assert_eq!(Install::detect(Path::new("/home/linuxbrew/.linuxbrew/Cellar/jurl/0.1.4/bin/jurl")), Install::Homebrew);
        assert_eq!(Install::detect(Path::new("/Users/me/.cargo/bin/jurl")), Install::Cargo);
        assert_eq!(Install::detect(Path::new("/Users/me/.local/bin/jurl")), Install::Installer);
        assert_eq!(Install::detect(Path::new(r"C:\Users\me\.cargo\bin\jurl.exe")), Install::Cargo);
    }
}
