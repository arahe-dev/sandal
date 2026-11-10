//! Compile-fail tests: the effect markers and `Verified` are enforced by the
//! compiler, so the wrong programs must not build.
//!
//! The expected messages in tests/ui/*.stderr come from the current stable
//! compiler. If a new compiler words them differently, refresh them with
//! `TRYBUILD=overwrite cargo test --test compile`.

#[test]
fn effects_and_verified_are_checked_by_the_compiler() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/pure_cannot_ask.rs");
    t.compile_fail("tests/ui/pure_cannot_call_world.rs");
    t.compile_fail("tests/ui/verified_cannot_be_forged.rs");
}
