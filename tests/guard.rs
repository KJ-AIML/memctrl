//! The write guard against the red-team corpus from the 2.1.0 review: 73 injection attempts and
//! 50 ordinary developer notes. Floors only ever rise, like the retrieval floors in eval.rs.
use memctrl::guard::{inspect, Policy};
use serde_json::Value;

const CAUGHT_AT_LEAST: usize = 26;
const BENIGN_FLAGGED_AT_MOST: usize = 4;

#[test]
fn guard_corpus_meets_floors() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/guard-corpus.json")).expect("valid corpus");
    let policy = Policy::default();
    let (mut caught, mut lost, mut flagged) = (Vec::new(), Vec::new(), Vec::new());
    for case in corpus["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let quarantined = inspect(case["text"].as_str().unwrap(), "manual", &policy).quarantined();
        match case["kind"].as_str().unwrap() {
            "attack" if quarantined => caught.push(id),
            "attack" if case["caught_in_2_1_0"] == true => lost.push(id),
            "benign" if quarantined => flagged.push(id),
            _ => {}
        }
    }
    eprintln!(
        "caught {} attacks; benign flagged {}: {flagged:?}",
        caught.len(),
        flagged.len()
    );
    assert!(
        lost.is_empty(),
        "attacks caught by 2.1.0 now pass: {lost:?}"
    );
    assert!(caught.len() >= CAUGHT_AT_LEAST, "caught only {caught:?}");
    assert!(
        flagged.len() <= BENIGN_FLAGGED_AT_MOST,
        "ordinary notes held for review: {flagged:?}"
    );
}
