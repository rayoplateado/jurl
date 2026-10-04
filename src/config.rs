use std::{collections::HashMap, env, fs, path::PathBuf};

/// Keys come from the environment first, then `~/.config/jurl/env`, then `./.env`
/// (simple `KEY=value` lines).
pub struct Config {
    file: HashMap<String, String>,
}

impl Config {
    pub fn load() -> Self {
        let mut file = HashMap::new();
        let mut paths = vec![PathBuf::from(".env")];
        if let Some(home) = env::var_os("HOME") {
            paths.insert(0, PathBuf::from(home).join(".config/jurl/env"));
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

    pub fn get(&self, key: &str) -> Option<String> {
        env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.file.get(key).cloned())
    }
}
