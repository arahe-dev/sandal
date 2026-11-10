//! The ten built-in operations and `Cycle`, a composition of them.

use crate::error::{Error, Result};
use crate::thing::{Idea, Payload, Proposal, SourceItem, Thing};

pub mod back;
pub mod carry;
pub mod cycle;
pub mod forward;
pub mod map;
pub mod matching;
pub mod mix;
pub mod notice;
pub mod read;
pub mod roots;
pub mod try_op;

pub use back::{Back, BackIn};
pub use carry::{Carry, CarryIn};
pub use cycle::{Cycle, CycleIn, CycleOut};
pub use forward::{Forward, ForwardIn};
pub use map::{MapIn, MapOp};
pub use matching::{MatchIn, MatchOp};
pub use mix::{Mix, MixIn};
pub use notice::{Notice, NoticeIn};
pub use read::{Read, ReadIn};
pub use roots::{Roots, RootsIn};
pub use try_op::{Try, TryIn};

/// The system prompt every Model operation uses.
pub const SYSTEM: &str = "Return only a JSON object. No prose and no code fences.";

/// Fill `{name}` placeholders in a prompt template in a single pass, so text
/// that is substituted in is never itself scanned for placeholders.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    'outer: while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        for (key, value) in vars {
            let token_len = key.len() + 2;
            if tail.len() >= token_len && &tail[1..1 + key.len()] == *key && tail[1 + key.len()..].starts_with('}') {
                out.push_str(value);
                rest = &tail[token_len..];
                continue 'outer;
            }
        }
        out.push('{');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

pub(crate) fn bullets(items: &[String]) -> String {
    if items.is_empty() {
        return "(none)".to_string();
    }
    items
        .iter()
        .map(|s| format!("- {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn non_empty(value: &str, what: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::BadModelOutput(format!("{what} is empty")))
    } else {
        Ok(())
    }
}

/// A source item, an idea or a proposal, reduced to what the prompts show.
pub(crate) struct Described {
    pub title: String,
    pub body: String,
    pub domain: String,
    pub outcome: Option<String>,
}

impl Described {
    /// The text a prompt shows. The outcome is included only when asked for.
    pub(crate) fn text(&self, with_outcome: bool) -> String {
        let mut s = format!("Title: {}\nDomain: {}\nBody:\n{}", self.title, self.domain, self.body);
        if with_outcome {
            if let Some(o) = &self.outcome {
                s.push_str(&format!("\nOutcome: {o}"));
            }
        }
        s
    }
}

/// Describe a thing that `back`, `carry` or `read` may be pointed at.
pub(crate) fn describe(thing: &Thing) -> Result<Described> {
    match thing.kind.as_str() {
        k if k == SourceItem::KIND => {
            let s: SourceItem = thing.decode()?;
            Ok(Described {
                title: s.title,
                body: s.body,
                domain: s.domain,
                outcome: s.outcome,
            })
        }
        k if k == Idea::KIND => {
            let i: Idea = thing.decode()?;
            Ok(Described {
                title: i.title,
                body: i.body,
                domain: "unspecified".to_string(),
                outcome: None,
            })
        }
        k if k == Proposal::KIND => {
            let p: Proposal = thing.decode()?;
            Ok(Described {
                title: format!("Proposal for part {}", p.part),
                body: format!("{}\nIt rests on: {}", p.rebuilt, p.root),
                domain: "unspecified".to_string(),
                outcome: None,
            })
        }
        other => Err(Error::WrongKind {
            expected: "source_item, idea or proposal".to_string(),
            found: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn render_fills_known_placeholders_only() {
        let t = "a {x} b {\"json\": 1} {y} {x}";
        assert_eq!(render(t, &[("x", "1"), ("y", "2")]), "a 1 b {\"json\": 1} 2 1");
    }

    #[test]
    fn render_does_not_rescan_substituted_text() {
        assert_eq!(render("{a} {b}", &[("a", "{b}"), ("b", "B")]), "{b} B");
    }
}
