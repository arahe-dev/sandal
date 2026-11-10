use sandal::*;

fn forge(proposal: Id<Proposal>, evidence: Id<TrialResult>) -> Verified<Id<Proposal>> {
    // The fields are private: the only way to get a Verified is verify::confirm.
    Verified { value: proposal, evidence }
}

fn main() {}
