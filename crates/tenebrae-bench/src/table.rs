//! `scene key value` tables: the Umbra reference (`reference/umbra.txt`, measured with Umbra's own optimizer and runtime) and
//! Tenebrae's baseline (`baseline.txt`, written by `bench baseline`).
//!
//! One record per line, `#` starts a comment line, blank lines are ignored. The line
//! `generator N` names the scene generator the table was measured on. Every scene has a `hash`
//! record (16 hex digits, [`crate::Scene::hash`]); values are integers unless named otherwise.

use std::collections::BTreeMap;

/// A parsed table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// The `generator` line, if any.
    pub generator: Option<u32>,
    /// `(scene, key) -> value`.
    pub rows: BTreeMap<(String, String), String>,
}

impl Table {
    /// Parses `text`; a malformed line is an error naming it.
    pub fn parse(text: &str) -> Result<Table, String> {
        let mut t = Table::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["generator", v] => {
                    t.generator = Some(v.parse().map_err(|_| format!("line {}: {line}", n + 1))?);
                }
                [scene, key, value] => {
                    t.rows
                        .insert(((*scene).into(), (*key).into()), (*value).into());
                }
                _ => return Err(format!("line {}: {line}", n + 1)),
            }
        }
        Ok(t)
    }

    /// The value of `key` for `scene`.
    pub fn get(&self, scene: &str, key: &str) -> Option<&str> {
        self.rows
            .get(&(scene.to_owned(), key.to_owned()))
            .map(String::as_str)
    }

    /// The value as an integer.
    pub fn int(&self, scene: &str, key: &str) -> Option<i64> {
        self.get(scene, key).and_then(|v| v.parse().ok())
    }

    /// Sets a record.
    pub fn set(&mut self, scene: &str, key: &str, value: impl ToString) {
        self.rows
            .insert((scene.to_owned(), key.to_owned()), value.to_string());
    }

    /// The table as text: `header` comment lines, the generator, then each scene of `order` in
    /// that order (keys in insertion-independent sorted order, `hash` first).
    pub fn render(&self, header: &str, order: &[&str]) -> String {
        let mut out = String::new();
        for line in header.lines() {
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
        if let Some(g) = self.generator {
            out.push_str(&format!("generator {g}\n"));
        }
        for scene in order {
            let mut keys: Vec<&String> = self
                .rows
                .keys()
                .filter(|(s, _)| s == scene)
                .map(|(_, k)| k)
                .collect();
            keys.sort_by_key(|k| (k.as_str() != "hash", k.as_str()));
            if keys.is_empty() {
                continue;
            }
            out.push('\n');
            for k in keys {
                out.push_str(&format!(
                    "{scene} {k} {}\n",
                    self.rows[&((*scene).into(), k.clone())]
                ));
            }
        }
        out
    }
}
