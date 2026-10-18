//! The built-in retrieval suite is a regression gate: floors sit a little below the measured
//! values, so a change that makes retrieval worse fails CI. Paraphrase cases are reported, not
//! gated; lexical retrieval is not expected to answer queries with no shared words.
mod common;
use memctrl::eval::{self, Retriever, BUILTIN_SUITE};
use memctrl::retriever::SearchOptions;
use memctrl::rules::Rules;
use serde_json::Value;
use tempfile::TempDir;

fn report() -> Value {
    let suite = eval::parse_suite(BUILTIN_SUITE).unwrap();
    let options = SearchOptions::from_rules(&Rules::default());
    eval::run(
        &suite,
        &[Retriever::Lexical, Retriever::Legacy],
        5,
        &options,
    )
    .unwrap()
}

fn metric(report: &Value, retriever: &str, path: &[&str]) -> f64 {
    let mut value = &report["results"][retriever];
    for key in path {
        value = &value[*key];
    }
    value
        .as_f64()
        .unwrap_or_else(|| panic!("{retriever} {path:?} missing: {report}"))
}

#[test]
fn built_in_suite_meets_published_floors() {
    let report = report();
    assert!(
        metric(&report, "lexical", &["hit_at_k"]) >= 0.90,
        "{report}"
    );
    assert!(
        metric(&report, "lexical", &["hit_at_1"]) >= 0.90,
        "{report}"
    );
    assert!(
        metric(&report, "lexical", &["negative_accuracy"]) >= 0.90,
        "{report}"
    );
    for kind in [
        "keyword",
        "stemming",
        "identifier",
        "alias",
        "thai",
        "chinese",
        "japanese",
        "korean",
        "glued",
    ] {
        assert_eq!(
            metric(&report, "lexical", &["by_kind", kind, "hit_at_k"]),
            1.0,
            "{kind}: {report}"
        );
    }
    assert_eq!(
        metric(
            &report,
            "lexical",
            &["by_kind", "generic", "negative_accuracy"]
        ),
        1.0
    );
}

#[test]
fn lexical_beats_the_legacy_scorer_where_it_matters() {
    let report = report();
    for path in [
        &["hit_at_k"][..],
        &["mrr"],
        &["negative_accuracy"],
        &["precision_at_k"],
    ] {
        assert!(
            metric(&report, "lexical", path) > metric(&report, "legacy", path),
            "{path:?}: {report}"
        );
    }
    for kind in ["chinese", "japanese", "korean"] {
        assert_eq!(
            metric(&report, "legacy", &["by_kind", kind, "hit_at_k"]),
            0.0
        );
    }
    assert!(
        metric(&report, "lexical", &["avg_tokens"]) < metric(&report, "legacy", &["avg_tokens"]),
        "fewer, better facts: {report}"
    );
    assert!(
        report["brief"]["approx_tokens"].as_u64().unwrap()
            <= memctrl::context::DEFAULT_BUDGET as u64
    );
}

#[test]
fn cli_eval_never_creates_a_database_and_gates_on_request() {
    let dir = TempDir::new().unwrap();
    let run = |args: &[&str]| {
        common::memctrl()
            .current_dir(dir.path())
            .env_remove("MEMCTRL_DB_PATH")
            .args(args)
            .output()
            .unwrap()
    };
    let out = run(&["eval", "--retriever", "lexical", "--min-hit-rate", "0.9"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(value["results"]["legacy"].is_null());
    assert!(
        !dir.path().join(".memctrl").exists(),
        "eval must not create a store"
    );
    let gated = run(&["eval", "--min-hit-rate", "1.0"]);
    assert!(!gated.status.success());
    assert!(gated.stdout.is_empty());
    assert!(String::from_utf8_lossy(&gated.stderr).contains("below --min-hit-rate"));
    std::fs::write(
        dir.path().join("suite.json"),
        r#"{"name":"mine","memories":[{"key":"a","content":"Builds use Bazel"}],"cases":[{"query":"bazel builds","expect":["a"]}]}"#,
    )
    .unwrap();
    let custom = run(&["eval", "--file", "suite.json"]);
    let value: Value = serde_json::from_slice(&custom.stdout).unwrap();
    assert_eq!(value["suite"], "mine");
    assert_eq!(value["results"]["lexical"]["hit_at_k"], 1.0);
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("tests/fixtures/eval/{name}")).unwrap();
    let suite = eval::parse_suite(&text).unwrap();
    let options = SearchOptions::from_rules(&Rules::default());
    eval::run(&suite, &[Retriever::Lexical], 5, &options).unwrap()
}

/// A reviewer's Thai team suite (not written by the retriever's author). In 2.1.0, 41% of its
/// vague questions returned notes.
#[test]
fn multilingual_suite_meets_floors() {
    let report = fixture("thai-team.json");
    assert!(
        metric(&report, "lexical", &["hit_at_k"]) >= 0.93,
        "{report}"
    );
    assert!(
        metric(&report, "lexical", &["hit_at_1"]) >= 0.90,
        "{report}"
    );
    assert!(
        metric(&report, "lexical", &["negative_accuracy"]) >= 0.95,
        "{report}"
    );
    for kind in ["glued", "th_short", "th_exact", "th_compound"] {
        assert_eq!(
            metric(&report, "lexical", &["by_kind", kind, "hit_at_k"]),
            1.0,
            "{kind}: {report}"
        );
    }
}

/// Held-out questions never used for tuning (2.1.0: 0.769 correctly empty). Fix the retriever,
/// never the word lists, when this fails.
#[test]
fn held_out_vague_questions_do_not_regress() {
    let report = fixture("thai-team-heldout.json");
    assert!(
        metric(&report, "lexical", &["negative_accuracy"]) >= 0.90,
        "{report}"
    );
    assert!(
        metric(&report, "lexical", &["hit_at_k"]) >= 0.91,
        "{report}"
    );
}

/// English vague and off-topic questions (2.1.0: half returned notes).
#[test]
fn english_vague_questions_are_mostly_empty() {
    let report = fixture("en-mirror.json");
    assert!(
        metric(&report, "lexical", &["negative_accuracy"]) >= 0.90,
        "{report}"
    );
}
