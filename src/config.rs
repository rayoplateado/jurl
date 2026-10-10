use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

/// Keys come from the environment first, then `~/.config/jurl/env`, then `./.env` (simple `KEY=value` lines).
/// `./.env` supplies only the `DOTENV_KEYS`.
#[derive(Default)]
pub(crate) struct Config {
    file: HashMap<String, String>,
}

/// The keys `./.env` may supply. A project's `.env` is not the user's: it must not choose the programs jurl runs
/// (`JURL_LIGHTPANDA`), the servers a key is sent to (`JURL_JEV_URL`, `JURL_CLOUD_URL`), or the jurl cloud account the
/// reads go to (`JURL_CLOUD_KEY`).
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
        Self::from_env(key).or_else(|| self.saved(key))
    }

    /// `key` from the environment, if it's set and not empty.
    pub(crate) fn from_env(key: &str) -> Option<String> {
        env::var(key).ok().filter(|v| !v.is_empty())
    }

    /// `key` as saved in `~/.config/jurl/env` (or in `./.env`, for the keys it may supply), not from the environment.
    pub(crate) fn saved(&self, key: &str) -> Option<String> {
        self.file.get(key).cloned()
    }

    /// Set `key` in `~/.config/jurl/env`, keeping every other line. The file is private (0600).
    pub(crate) fn save(&mut self, key: &str, value: &str) -> Result<PathBuf> {
        let path = Self::path().context("no config directory ($HOME is not set)")?;
        let old = fs::read_to_string(&path).unwrap_or_default();
        let mut lines = drop_keys(&old, &[key]);
        lines.push(format!("{key}={value}"));
        fs::create_dir_all(path.parent().unwrap())?;
        write_private(&path, &(lines.join("\n") + "\n"))?;
        self.file.insert(key.to_string(), value.to_string());
        Ok(path)
    }

    /// Remove `keys` from `~/.config/jurl/env`, keeping every other line. With no file, there is nothing to remove.
    pub(crate) fn remove(&mut self, keys: &[&str]) -> Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        let Ok(old) = fs::read_to_string(&path) else { return Ok(()) };
        let lines = drop_keys(&old, keys);
        let text = if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" };
        write_private(&path, &text)?;
        for key in keys {
            self.file.remove(*key);
        }
        Ok(())
    }
}

/// The lines of a config file that don't set one of `keys`, read the way `parse` reads them.
fn drop_keys(text: &str, keys: &[&str]) -> Vec<String> {
    text.lines()
        .filter(|l| {
            l.trim().trim_start_matches("export ").split_once('=').is_none_or(|(k, _)| !keys.contains(&k.trim()))
        })
        .map(String::from)
        .collect()
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

    #[test]
    fn a_project_env_never_chooses_where_a_key_goes() {
        let file =
            merge("", "JURL_CLOUD_URL=https://evil.example\nJURL_JEV_URL=https://evil.example\nJURL_CLOUD_KEY=k");
        assert!(!file.contains_key("JURL_CLOUD_URL"));
        assert!(!file.contains_key("JURL_JEV_URL"));
        assert!(!file.contains_key("JURL_CLOUD_KEY"));
    }

    #[test]
    fn dropping_keys_keeps_every_other_line() {
        let text = "export JURL_CLOUD_KEY=old\n# a note\nTYPESAFE_API_KEY=k\nno equals sign\nJURL_CLOUD_URL=http://x";
        assert_eq!(
            drop_keys(text, &["JURL_CLOUD_KEY", "JURL_CLOUD_URL"]),
            ["# a note", "TYPESAFE_API_KEY=k", "no equals sign"]
        );
        assert!(drop_keys("", &["X"]).is_empty());
    }
}
