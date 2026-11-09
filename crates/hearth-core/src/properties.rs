//! `server.properties` editing that keeps comments, blank lines and key order intact.
//! Both Minecraft Java and Bedrock use this format.

use std::path::Path;

use crate::error::{IoContext, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Entry { key: String, value: String },
    Other(String),
}

impl Properties {
    pub fn parse(text: &str) -> Self {
        let lines = text
            .lines()
            .map(|raw| {
                let trimmed = raw.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                    return Line::Other(raw.to_owned());
                }
                match raw.split_once('=') {
                    Some((k, v)) => Line::Entry {
                        key: k.trim().to_owned(),
                        value: v.to_owned(),
                    },
                    None => Line::Other(raw.to_owned()),
                }
            })
            .collect();
        Self { lines }
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(Self::parse(&std::fs::read_to_string(path).at(path)?))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.render()).at(path)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            Line::Entry { key: k, value } if k == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// Updates the existing entry in place, or appends a new one.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = sanitize(value.into());
        for l in &mut self.lines {
            if let Line::Entry { key: k, value: v } = l {
                if k == key {
                    *v = value;
                    return;
                }
            }
        }
        self.lines.push(Line::Entry {
            key: key.to_owned(),
            value,
        });
    }

    pub fn entries(&self) -> Vec<(String, String)> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                Line::Entry { key, value } => Some((key.clone(), value.clone())),
                Line::Other(_) => None,
            })
            .collect()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            match l {
                Line::Entry { key, value } => {
                    out.push_str(key);
                    out.push('=');
                    out.push_str(value);
                }
                Line::Other(s) => out.push_str(s),
            }
            out.push('\n');
        }
        out
    }
}

/// A newline in a value would inject extra keys.
fn sanitize(v: String) -> String {
    if v.contains(['\n', '\r']) {
        v.replace(['\n', '\r'], " ")
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "#Minecraft server properties\n#Sat Nov 08\nmotd=A Minecraft Server\n\nserver-port=25565\nlevel-seed=\n";

    #[test]
    fn round_trips_unchanged() {
        assert_eq!(Properties::parse(SAMPLE).render(), SAMPLE);
    }

    #[test]
    fn get_and_set_in_place() {
        let mut p = Properties::parse(SAMPLE);
        assert_eq!(p.get("server-port"), Some("25565"));
        assert_eq!(p.get("level-seed"), Some(""));
        p.set("server-port", "25570");
        p.set("white-list", "true");
        let out = p.render();
        assert!(out.contains("server-port=25570\n"));
        assert!(out.ends_with("white-list=true\n"));
        assert!(out.starts_with("#Minecraft server properties\n"));
    }

    #[test]
    fn values_may_contain_equals() {
        let p = Properties::parse("motd=a=b\n");
        assert_eq!(p.get("motd"), Some("a=b"));
    }

    #[test]
    fn newlines_cannot_inject_keys() {
        let mut p = Properties::default();
        p.set("motd", "hi\nop-permission-level=4");
        assert_eq!(p.entries().len(), 1);
        assert_eq!(Properties::parse(&p.render()).entries().len(), 1);
    }
}
