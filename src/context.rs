//! Session brief: the highest-trust memories that fit a token budget, loaded before an agent's
//! first turn so it never has to remember to ask.
use crate::model::{ClaimType, Memory, VerificationState};
use crate::sanitize::validate_identifier;
use crate::store::{AtomicWrite, MemoryStore};
use anyhow::Result;
use serde::Serialize;

pub const DEFAULT_BUDGET: usize = 1500;
pub const DEFAULT_LAYERS: [&str; 2] = ["project", "user"];
pub const BEGIN_MARKER: &str = "<!-- memctrl:context:begin -->";
pub const END_MARKER: &str = "<!-- memctrl:context:end -->";
const MAX_ENTRY_CHARS: usize = 400;
const TITLE: &str = "# Project memory (memctrl)";

/// Rough token estimate that errs high: about four ASCII characters per token, and one token per
/// other character, which is conservative for CJK and Thai.
pub fn approx_tokens(text: &str) -> usize {
    let (ascii, other) =
        text.chars().fold(
            (0, 0),
            |(a, o), c| {
                if c.is_ascii() {
                    (a + 1, o)
                } else {
                    (a, o + 1)
                }
            },
        );
    usize::div_ceil(ascii, 4) + other
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Decisions,
    Facts,
    Lessons,
    Hypotheses,
    CheckFirst,
}
impl Section {
    fn heading(self) -> &'static str {
        match self {
            Self::Decisions => "Decisions",
            Self::Facts => "Facts and conventions",
            Self::Lessons => "Lessons",
            Self::Hypotheses => "Open hypotheses",
            Self::CheckFirst => "Check before trusting",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BriefEntry {
    pub id: String,
    pub content: String,
    pub layer: String,
    pub claim_type: ClaimType,
    pub verification_state: VerificationState,
    pub confidence: f64,
    pub score: f64,
    pub section: Section,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub entries: Vec<BriefEntry>,
    pub eligible: usize,
    pub approx_tokens: usize,
    pub budget: usize,
    pub layers: Vec<String>,
    pub markdown: String,
}

/// Trust-weighted priority. Confidence is scaled by verification, claim type and layer, with a
/// small, capped bonus for memories that retrieval keeps returning.
pub fn score(memory: &Memory) -> f64 {
    let verification = match memory.verification_state {
        VerificationState::Supported => 1.5,
        VerificationState::Unverified => 1.0,
        VerificationState::Disputed => 0.5,
        VerificationState::Refuted => 0.0,
    };
    let claim = match memory.claim_type {
        ClaimType::Decision => 1.3,
        ClaimType::DerivedLesson => 1.1,
        ClaimType::Assertion => 1.0,
        ClaimType::Observation => 0.9,
        ClaimType::Hypothesis => 0.6,
    };
    let layer = if memory.layer == "project" { 1.2 } else { 1.0 };
    let reinforcement = 1.0 + 0.05 * (memory.access_count as f64 + 1.0).log2().min(6.0);
    memory.confidence * verification * claim * layer * reinforcement
}

fn section(memory: &Memory, stale: bool) -> Section {
    if stale || memory.verification_state == VerificationState::Disputed {
        return Section::CheckFirst;
    }
    match memory.claim_type {
        ClaimType::Decision => Section::Decisions,
        ClaimType::DerivedLesson => Section::Lessons,
        ClaimType::Hypothesis => Section::Hypotheses,
        ClaimType::Assertion | ClaimType::Observation => Section::Facts,
    }
}

fn one_line(content: &str) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_ENTRY_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_ENTRY_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

fn render_entry(entry: &BriefEntry) -> String {
    let short: String = entry.id.chars().take(8).collect();
    let mut notes = vec![format!("`{short}`")];
    match entry.verification_state {
        VerificationState::Supported => notes.push("verified".into()),
        VerificationState::Disputed => notes.push("disputed".into()),
        _ => {}
    }
    if let Some(stale) = &entry.stale {
        notes.push(format!("stale: {stale}"));
    }
    format!("- {} · {}", entry.content, notes.join(" · "))
}

fn header(shown: usize, eligible: usize, layers: &[String], tokens: usize) -> String {
    format!(
        "{TITLE}\n\n{shown} of {eligible} memories from {} · ≈{tokens} tokens · highest trust first.\n\
         Look up anything else with `memctrl query \"<specific terms>\"`; names, paths and error text work best.\n",
        layers.join(", ")
    )
}

const FOOTER: &str = "\nSave new durable facts with `memctrl add \"<fact>\" --layer project` \
(add `--claim-type decision` for decisions and `--cite path:line` for the code they describe).\n";

/// Picks memories greedily by score until the budget is spent, then groups them by section.
/// `stale` returns a short note for memories whose cited code changed; those sink and are
/// labelled instead of being presented as settled facts.
pub fn build(
    memories: &[Memory],
    layers: &[String],
    budget: usize,
    stale: &dyn Fn(&Memory) -> Option<String>,
) -> Brief {
    if memories.is_empty() {
        let markdown = format!(
            "{TITLE}\n\nNo saved memories yet in {}. Save durable facts with \
             `memctrl add \"<fact>\" --layer project`.\n",
            layers.join(", ")
        );
        return Brief {
            entries: vec![],
            eligible: 0,
            approx_tokens: approx_tokens(&markdown),
            budget,
            layers: layers.to_vec(),
            markdown,
        };
    }
    let mut ranked: Vec<BriefEntry> = memories
        .iter()
        .map(|m| {
            let note = stale(m);
            let factor = if note.is_some() { 0.5 } else { 1.0 };
            BriefEntry {
                id: m.id.clone(),
                content: one_line(&m.content),
                layer: m.layer.clone(),
                claim_type: m.claim_type,
                verification_state: m.verification_state,
                confidence: m.confidence,
                score: (score(m) * factor * 1000.0).round() / 1000.0,
                section: section(m, note.is_some()),
                stale: note,
            }
        })
        .collect();
    // Highest score first; newest first among equals (list order is created_at DESC); id last.
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Reserve room for the header (with worst-case counts) and footer before filling.
    let reserved = approx_tokens(&header(memories.len(), memories.len(), layers, budget))
        + approx_tokens(FOOTER)
        + 8;
    let mut spent = reserved;
    let mut chosen = Vec::new();
    for entry in ranked {
        let cost = approx_tokens(&render_entry(&entry)) + 1;
        if spent + cost <= budget {
            spent += cost;
            chosen.push(entry);
        }
    }
    let mut body = String::new();
    let mut sections: Vec<Section> = chosen.iter().map(|e| e.section).collect();
    sections.sort();
    sections.dedup();
    for current in sections {
        body.push_str(&format!("\n## {}\n\n", current.heading()));
        for entry in chosen.iter().filter(|e| e.section == current) {
            body.push_str(&render_entry(entry));
            body.push('\n');
        }
    }
    let mut markdown = String::new();
    let mut tokens = spent;
    // The header states its own size; settle the number in a couple of passes.
    for _ in 0..3 {
        markdown = format!(
            "{}{body}{FOOTER}",
            header(chosen.len(), memories.len(), layers, tokens)
        );
        let measured = approx_tokens(&markdown);
        if measured == tokens {
            break;
        }
        tokens = measured;
    }
    Brief {
        approx_tokens: approx_tokens(&markdown),
        entries: chosen,
        eligible: memories.len(),
        budget,
        layers: layers.to_vec(),
        markdown,
    }
}

/// Reads eligible memories and records which ones the brief selected, in one writer-locked
/// snapshot. Access counts are deliberately left alone: a brief that reinforced its own entries
/// would keep promoting them forever.
pub fn session_brief(
    store: &MemoryStore,
    layers: &[String],
    budget: usize,
    stale: &dyn Fn(&Memory) -> Option<String>,
) -> Result<Brief> {
    let mut unique: Vec<String> = Vec::new();
    for layer in layers {
        validate_identifier(layer, "layer")?;
        if !unique.contains(layer) {
            unique.push(layer.clone());
        }
    }
    let guard = AtomicWrite::new(&store.conn)?;
    let mut memories = Vec::new();
    for layer in &unique {
        memories.extend(store.list_memories(Some(layer), false)?);
    }
    let brief = build(&memories, &unique, budget, stale);
    let ids: Vec<String> = brief.entries.iter().map(|e| e.id.clone()).collect();
    let average = if brief.entries.is_empty() {
        0.0
    } else {
        brief.entries.iter().map(|e| e.confidence).sum::<f64>() / brief.entries.len() as f64
    };
    store.save_provenance_ids(
        "session context brief",
        &ids,
        memories.len(),
        "context_brief",
        average,
    )?;
    guard.commit()?;
    Ok(brief)
}

/// The brief wrapped in markers so `--write` can refresh it in place inside AGENTS.md.
pub fn marked_block(brief: &Brief) -> String {
    format!(
        "{BEGIN_MARKER}\n<!-- Generated by `memctrl context --write`. Re-run it to refresh; edits here are overwritten. -->\n{}{END_MARKER}",
        brief.markdown
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LifecycleState, NewMemory};

    fn memory(content: &str, claim: ClaimType, verification: VerificationState) -> NewMemory {
        NewMemory {
            layer: "project".into(),
            content: content.into(),
            claim_type: claim,
            verification_state: verification,
            lifecycle_state: LifecycleState::Accepted,
            ..Default::default()
        }
    }

    #[test]
    fn token_estimate_is_conservative_for_non_ascii() {
        assert_eq!(approx_tokens("abcd"), 1);
        assert_eq!(approx_tokens("abcde"), 2);
        assert_eq!(approx_tokens("ใช้ pnpm"), 3 + 2);
    }

    #[test]
    fn brief_ranks_by_trust_groups_sections_and_respects_budget() {
        let store = MemoryStore::open(":memory:").unwrap();
        store
            .insert_memory(memory(
                "We chose SQLx over Diesel",
                ClaimType::Decision,
                VerificationState::Supported,
            ))
            .unwrap();
        store
            .insert_memory(memory(
                "Tests need Postgres running",
                ClaimType::Assertion,
                VerificationState::Unverified,
            ))
            .unwrap();
        store
            .insert_memory(memory(
                "Maybe switch to Bun",
                ClaimType::Hypothesis,
                VerificationState::Unverified,
            ))
            .unwrap();
        store
            .insert_memory(memory(
                "CI uses Node 18",
                ClaimType::Assertion,
                VerificationState::Disputed,
            ))
            .unwrap();
        let layers = vec!["project".to_string()];
        let brief = session_brief(&store, &layers, 2000, &|_| None).unwrap();
        assert_eq!(brief.entries.len(), 4);
        assert_eq!(brief.entries[0].content, "We chose SQLx over Diesel");
        let md = &brief.markdown;
        let order = [
            "## Decisions",
            "## Facts and conventions",
            "## Open hypotheses",
            "## Check before trusting",
        ];
        let positions: Vec<usize> = order.iter().map(|h| md.find(h).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{md}");
        assert!(md.contains("verified") && md.contains("disputed"));
        assert!(brief.approx_tokens <= 2000);

        let tiny = session_brief(&store, &layers, 120, &|_| None).unwrap();
        assert!(tiny.entries.len() < 4);
        assert!(tiny.approx_tokens <= 120, "{}", tiny.markdown);
    }

    #[test]
    fn brief_never_bumps_access_and_records_provenance() {
        let store = MemoryStore::open(":memory:").unwrap();
        let id = store
            .insert_memory(memory(
                "Deploys need manual approval",
                ClaimType::Assertion,
                VerificationState::Unverified,
            ))
            .unwrap();
        let layers = vec!["project".to_string()];
        session_brief(&store, &layers, 1500, &|_| None).unwrap();
        session_brief(&store, &layers, 1500, &|_| None).unwrap();
        assert_eq!(
            store.get_memory(&id, true).unwrap().unwrap().access_count,
            0
        );
        let provenance = store.provenance(5).unwrap();
        assert_eq!(provenance.len(), 2);
        assert_eq!(provenance[0]["method"], "context_brief");
        assert_eq!(provenance[0]["sources"][0]["memory_id"], id);
    }

    #[test]
    fn stale_memories_sink_into_check_before_trusting() {
        let store = MemoryStore::open(":memory:").unwrap();
        let id = store
            .insert_memory(memory(
                "UserService.getById returns null",
                ClaimType::Decision,
                VerificationState::Supported,
            ))
            .unwrap();
        let layers = vec!["project".to_string()];
        let stale_id = id.clone();
        let brief = session_brief(&store, &layers, 1500, &move |m| {
            (m.id == stale_id).then(|| "src/user.rs changed".to_string())
        })
        .unwrap();
        assert_eq!(brief.entries[0].section, Section::CheckFirst);
        assert!(brief.markdown.contains("stale: src/user.rs changed"));
        assert!(!brief.markdown.contains("## Decisions"));
    }

    #[test]
    fn empty_store_still_explains_how_to_save() {
        let store = MemoryStore::open(":memory:").unwrap();
        let brief = session_brief(&store, &["project".to_string()], 1500, &|_| None).unwrap();
        assert!(brief.entries.is_empty());
        assert!(brief.markdown.contains("memctrl add"));
    }
}
