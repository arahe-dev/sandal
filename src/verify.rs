//! Claimed and verified. Anything a model produces is a claim. A proposal
//! becomes verified only through a Confirmed result that a verifier entered,
//! and the type system keeps the two apart.

use crate::error::{Error, Result};
use crate::id::Id;
use crate::ledger::Ledger;
use crate::thing::{Grade, MadeBy, Proposal, TrialResult};

/// Fields are private. The only constructor is `confirm`.
pub struct Verified<T> {
    value: T,
    evidence: Id<TrialResult>,
}

impl<T> Verified<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    /// The trial result that backs the claim.
    pub fn evidence(&self) -> Id<TrialResult> {
        self.evidence
    }
}

/// Ok only if the result's `made_by` is `Verifier(_)`, its grade is
/// `Confirmed`, and its `proposal` field matches.
pub fn confirm(ledger: &Ledger, result: Id<TrialResult>) -> Result<Verified<Id<Proposal>>> {
    let thing = ledger
        .get(&result.raw())
        .ok_or_else(|| Error::NotFound(format!("thing {}", result.raw())))?;
    let payload: TrialResult = thing.decode()?;
    if !matches!(thing.made_by, MadeBy::Verifier(_)) {
        return Err(Error::NotVerified(format!(
            "result {} was not entered by a verifier (made by {})",
            result.raw().short(),
            thing.made_by
        )));
    }
    if payload.grade != Grade::Confirmed {
        return Err(Error::NotVerified(format!(
            "result {} is graded {}, not confirmed",
            result.raw().short(),
            payload.grade
        )));
    }
    // "Matches" means the result's own proposal field agrees with its parent.
    if thing.parents != [payload.proposal.raw()] {
        return Err(Error::NotVerified(format!(
            "result {} names a proposal that is not its parent",
            result.raw().short()
        )));
    }
    Ok(Verified {
        value: payload.proposal,
        evidence: result,
    })
}
