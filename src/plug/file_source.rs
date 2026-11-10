use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;

use super::{Source, SourceQuery};
use crate::error::{Error, Result};
use crate::thing::SourceItem;

/// Reads a JSONL file of `SourceItem`. Drops items whose domain is excluded.
/// Ranks by the number of distinct lowercase words of four or more letters
/// shared between the item's title plus body and the query's conditions, ties
/// by title. Returns the top k.
pub struct FileSource {
    name: String,
    path: PathBuf,
}

impl FileSource {
    pub fn new(name: &str, path: impl Into<PathBuf>) -> Self {
        FileSource {
            name: name.to_string(),
            path: path.into(),
        }
    }

    fn load(&self) -> Result<Vec<SourceItem>> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| Error::Plug(format!("file source {}: {e}", self.path.display())))?;
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| {
                serde_json::from_str(l).map_err(|e| {
                    Error::Plug(format!("{}: line {}: {e}", self.path.display(), i + 1))
                })
            })
            .collect()
    }
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphabetic())
        .filter(|w| w.chars().count() >= 4)
        .map(|w| w.to_lowercase())
        .collect()
}

#[async_trait]
impl Source for FileSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch(&self, q: &SourceQuery) -> Result<Vec<SourceItem>> {
        let excluded: BTreeSet<String> =
            q.exclude_domains.iter().map(|d| d.to_lowercase()).collect();
        let wanted = words(&q.conditions.join(" "));
        let mut scored: Vec<(usize, SourceItem)> = self
            .load()?
            .into_iter()
            .filter(|i| !excluded.contains(&i.domain.to_lowercase()))
            .map(|i| {
                let shared = words(&format!("{} {}", i.title, i.body))
                    .intersection(&wanted)
                    .count();
                (shared, i)
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
        Ok(scored.into_iter().take(q.k).map(|(_, i)| i).collect())
    }
}
