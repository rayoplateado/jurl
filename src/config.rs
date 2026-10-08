use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

/// Keys come from the environment first, then `~/.config/jurl/env`, then `./.env` (simple `KEY=value` lines).
/// `./.env` supplies only the `DOTENV_KEYS`.
pub(crate) struct Config {
    file: HashMap<String, String>,
}

/// The keys `./.env` may supply. A project's `.env` is not the user's: it must not choose the programs jurl runs
/// (`JURL_LIGHTPANDA`).
const DOTENV_KEYS: &[&str] = &["TYPESAFE_API_KEY", "JURL_JEV_KEY", "CLOUDFLARE_ACCOUNT_ID", "CLOUDFLARE_AI_TOKEN"];

impl Config {
    pub(crate) fn load() -> Self {
        let user = Self::path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let dotenv = fs::read_to_string(".env").unwrap_or_default();
        Self { file: merge(&user, &dotenv) }
    }

    fn path() -> Option<PathBuf> {
        let base = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| {
            env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".config"))
        })?;
        Some(base.join("jurl/env"))
    }

    pub(crate) fn get(&self, key: &str) -> Option<String> {
        env::var(key).ok().filter(|v| !v.is_empty()).or_else(|| self.file.get(key).cloned())
    }

    /// Set `key` in `~/.config/jurl/env`, keeping every other line. The file is private (0600).
    pub(crate) fn save(&mut self, key: &str, value: &str) -> Result<PathBuf> {
        let path = Self::path().context("no config directory ($HOME is not set)")?;
        let old = fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<String> = old
            .lines()
            .filter(|l| l.trim().trim_start_matches("export ").split_once('=').is_none_or(|(k, _)| k.trim() != key))
            .map(String::from)
            .collect();
        lines.push(format!("{key}={value}"));
        fs::create_dir_all(path.parent().unwrap())?;
        write_private(&path, &(lines.join("\n") + "\n"))?;
        self.file.insert(key.to_string(), value.to_string());
        Ok(path)
    }
}

/// `KEY=value` lines, with `export`, quotes and `#` comments handled. Any other line is skipped.
fn parse(text: &str) -> impl Iterator<Item = (String, String)> + '_ {
    text.lines().filter_map(|line| {
        let line = line.trim().trim_start_matches("export ");
        if line.starts_with('#') {
            return None;
        }
        let (k, v) = line.split_once('=')?;
        Some((k.trim().to_string(), v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()))
    })
}

/// The user's file wins for a key both files set; within a file, the first line wins.
fn merge(user: &str, dotenv: &str) -> HashMap<String, String> {
    let mut file = HashMap::new();
    for (k, v) in parse(user) {
        file.entry(k).or_insert(v);
    }
    for (k, v) in parse(dotenv).filter(|(k, _)| DOTENV_KEYS.contains(&k.as_str())) {
        file.entry(k).or_insert(v);
    }
    file
}

/// Write `text` to `path`, readable only by its owner (0600 on Unix). The mode is set before the text goes in, so
/// the key is never readable by others, not even briefly.
fn write_private(path: &Path, text: &str) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` only applies to a new file: an older jurl may have left this one looser.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn writes_0600_whether_the_file_is_new_or_looser() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("jurl-config-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let loose = dir.join("loose");
        fs::write(&loose, "OLD=1\n").unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
        for path in [loose, dir.join("fresh")] {
            write_private(&path, "KEY=value\n").unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), "KEY=value\n");
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dotenv_supplies_only_the_api_keys() {
        let file = merge("", "JURL_LIGHTPANDA=/bin/echo\nTYPESAFE_API_KEY=k\nCLOUDFLARE_AI_TOKEN=t");
        assert!(!file.contains_key("JURL_LIGHTPANDA"));
        assert_eq!(file["TYPESAFE_API_KEY"], "k");
        assert_eq!(file["CLOUDFLARE_AI_TOKEN"], "t");
    }

    #[test]
    fn user_file_sets_any_key_and_wins_over_dotenv() {
        let file = merge("JURL_LIGHTPANDA=/opt/lp\nTYPESAFE_API_KEY=user", "TYPESAFE_API_KEY=dotenv");
        assert_eq!(file["JURL_LIGHTPANDA"], "/opt/lp");
        assert_eq!(file["TYPESAFE_API_KEY"], "user");
    }

    #[test]
    fn parses_export_quotes_and_comments() {
        let file = merge("# a comment\nexport TYPESAFE_API_KEY=\"quoted\"\n\nno equals sign\n", "");
        assert_eq!(file.len(), 1);
        assert_eq!(file["TYPESAFE_API_KEY"], "quoted");
    }
}
