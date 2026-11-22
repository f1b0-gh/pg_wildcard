// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "hot")]
use libwildcard::antalya::SweepStrategy;

// A downstream exhaustive match must keep compiling when the caller uses only
// the original sweep API. Opt-in dispatch diagnostics have their own type.
#[test]
fn original_strategy_remains_exhaustive() {
    let name = match SweepStrategy::ConstantCandidates {
        SweepStrategy::ConstantCandidates => "constant_candidates",
    };
    assert_eq!(name, "constant_candidates");
}
