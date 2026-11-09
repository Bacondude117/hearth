//! `PalWorldSettings.ini`: a section header plus one long line
//! `OptionSettings=(Difficulty=None,ServerName="My, Server",CrossplayPlatforms=(Steam,Xbox),...)`.
//! Values may be quoted strings (which can contain commas) or parenthesized lists.

use std::path::Path;

use crate::error::{Error, IoContext, Result};

const SECTION: &str = "[/Script/Pal.PalGameWorldSettings]";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PalSettings {
    entries: Vec<(String, String)>,
}

impl PalSettings {
    pub fn parse(text: &str) -> Result<Self> {
        let Some(line) = text.lines().map(str::trim).find(|l| l.starts_with("OptionSettings=")) else {
            return Ok(Self::default());
        };
        let body = line["OptionSettings=".len()..].trim();
        let inner = body
            .strip_prefix('(')
            .and_then(|b| b.strip_suffix(')'))
            .ok_or_else(|| Error::InvalidInput("PalWorldSettings.ini: OptionSettings isn't wrapped in ( )".into()))?;
        let mut entries = vec![];
        for part in split_top_level(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| Error::InvalidInput(format!("PalWorldSettings.ini: can't read \"{part}\"")))?;
            entries.push((k.trim().to_owned(), v.trim().to_owned()));
        }
        Ok(Self { entries })
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::parse(&std::fs::read_to_string(path).at(path)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).at(dir)?;
        }
        std::fs::write(path, self.render()).at(path)
    }

    pub fn render(&self) -> String {
        let body: Vec<String> = self.entries.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{SECTION}\nOptionSettings=({})\n", body.join(","))
    }

    /// Raw value as written (strings keep their quotes).
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Value with surrounding quotes removed.
    pub fn get(&self, key: &str) -> Option<String> {
        self.raw(key).map(unquote)
    }

    /// Sets a value given as the user sees it; quotes it if the existing value was quoted
    /// (or if it needs quoting).
    pub fn set(&mut self, key: &str, value: &str) {
        let value = value.replace(['\n', '\r'], " ");
        let quoted = self.raw(key).map(|v| v.starts_with('"')).unwrap_or(false)
            || value.contains([',', '(', ')', '=', ' '])
            || value.is_empty();
        let encoded = if quoted && !(value.starts_with('(') && value.ends_with(')')) {
            format!("\"{}\"", value.replace('"', "'"))
        } else {
            value
        };
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = encoded,
            None => self.entries.push((key.to_owned(), encoded)),
        }
    }

    /// Entries for display, unquoted.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.entries.iter().map(|(k, v)| (k.clone(), unquote(v))).collect()
    }
}

fn unquote(v: &str) -> String {
    v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v).to_owned()
}

/// Splits on commas that are outside quotes and parentheses.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = vec![];
    let (mut depth, mut in_quotes, mut start) = (0i32, false, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '(' if !in_quotes => depth += 1,
            ')' if !in_quotes => depth -= 1,
            ',' if !in_quotes && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; This configuration file is a sample of the default server settings.\n\n[/Script/Pal.PalGameWorldSettings]\nOptionSettings=(Difficulty=None,ExpRate=1.000000,bIsPvP=False,ServerPlayerMaxNum=32,ServerName=\"Default Palworld Server\",ServerDescription=\"\",AdminPassword=\"\",ServerPassword=\"\",PublicPort=8211,RCONEnabled=False,RESTAPIEnabled=False,RESTAPIPort=8212,CrossplayPlatforms=(Steam,Xbox,PS5,Mac),DenyTechnologyList=,ServerDescription2=\"Fun, friendly (casual) server\")\n";

    #[test]
    fn parses_the_default_file() {
        let s = PalSettings::parse(SAMPLE).unwrap();
        assert_eq!(s.get("Difficulty").as_deref(), Some("None"));
        assert_eq!(s.get("ServerName").as_deref(), Some("Default Palworld Server"));
        assert_eq!(s.get("ServerDescription").as_deref(), Some(""));
        assert_eq!(s.get("CrossplayPlatforms").as_deref(), Some("(Steam,Xbox,PS5,Mac)"));
        assert_eq!(s.get("DenyTechnologyList").as_deref(), Some(""));
        assert_eq!(s.get("ServerDescription2").as_deref(), Some("Fun, friendly (casual) server"));
        assert_eq!(s.entries().len(), 15);
    }

    #[test]
    fn round_trips_and_edits_keep_order_and_quoting() {
        let mut s = PalSettings::parse(SAMPLE).unwrap();
        s.set("ServerName", "Ryker's, Pals");
        s.set("ServerPlayerMaxNum", "8");
        s.set("RESTAPIEnabled", "True");
        s.set("AdminPassword", "abc123");
        let again = PalSettings::parse(&s.render()).unwrap();
        assert_eq!(again.get("ServerName").as_deref(), Some("Ryker's, Pals"));
        assert_eq!(again.raw("ServerPlayerMaxNum"), Some("8"));
        assert_eq!(again.raw("RESTAPIEnabled"), Some("True"));
        assert_eq!(again.raw("AdminPassword"), Some("\"abc123\""), "was quoted, stays quoted");
        assert_eq!(again.get("CrossplayPlatforms").as_deref(), Some("(Steam,Xbox,PS5,Mac)"));
        let keys: Vec<String> = again.entries().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys[0], "Difficulty");
        assert!(s.render().starts_with("[/Script/Pal.PalGameWorldSettings]\nOptionSettings=("));
    }

    #[test]
    fn values_cannot_break_the_line() {
        let mut s = PalSettings::parse(SAMPLE).unwrap();
        s.set("ServerName", "a\"),AdminPassword=(\"hacked");
        let again = PalSettings::parse(&s.render()).unwrap();
        assert_eq!(again.get("AdminPassword").as_deref(), Some(""));
        assert_eq!(again.entries().len(), 15);
    }

    #[test]
    fn empty_or_missing_file() {
        assert_eq!(PalSettings::parse("").unwrap().entries().len(), 0);
        assert!(PalSettings::parse("OptionSettings=Difficulty=None").is_err());
    }
}
