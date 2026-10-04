use std::{collections::HashMap, env, fs, path::PathBuf};

use anyhow::{Context, Result};

/// Keys come from the environment first, then `~/.config/jurl/env`, then `./.env`
/// (simple `KEY=value` lines).
pub struct Config {
    file: HashMap<String, String>,
}

impl Config {
    pub fn load() -> Self {
        let mut file = HashMap::new();
        let mut paths = vec![PathBuf::from(".env")];
        if let Some(p) = Self::path() {
            paths.insert(0, p);
        }
        for path in paths {
            let Ok(text) = fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let line = line.trim().trim_start_matches("export ");
                if line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                    file.entry(k.trim().to_string()).or_insert_with(|| v.to_string());
                }
            }
        }
        Self { file }
    }

    pub fn path() -> Option<PathBuf> {
        let base = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| {
            env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".config"))
        })?;
        Some(base.join("jurl/env"))
    }

    pub fn get(&self, key: &str) -> Option<String> {
        env::var(key).ok().filter(|v| !v.is_empty()).or_else(|| self.file.get(key).cloned())
    }

    /// Set `key` in `~/.config/jurl/env`, keeping every other line. The file is private (0600).
    pub fn save(&mut self, key: &str, value: &str) -> Result<PathBuf> {
        let path = Self::path().context("no config directory ($HOME is not set)")?;
        let old = fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<String> = old
            .lines()
            .filter(|l| l.trim().trim_start_matches("export ").split_once('=').is_none_or(|(k, _)| k.trim() != key))
            .map(String::from)
            .collect();
        lines.push(format!("{key}={value}"));
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, lines.join("\n") + "\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        self.file.insert(key.to_string(), value.to_string());
        Ok(path)
    }
}
