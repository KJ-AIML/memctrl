//! Lexical retrieval over SQLite FTS5.
//!
//! Space-delimited text is matched word by word with Porter stemming and BM25 ranking, so rare
//! words outweigh common ones. Thai, CJK, kana, Hangul and other scripts written without spaces
//! are matched by character trigrams. Every hit records which query terms matched it.
use crate::model::{Memory, Why};
use crate::store::MemoryStore;
use anyhow::Result;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

/// Upper bounds that keep one query's cost predictable regardless of input size.
const MAX_TERMS: usize = 16;
const MAX_TERM_CHARS: usize = 64;
const MAX_GRAMS_PER_RUN: usize = 48;
const HITS_PER_TERM: usize = 500;
const CANDIDATES: usize = 200;
/// Relative weight of evidence that is weaker than a direct word match.
const ALIAS_WEIGHT: f64 = 0.8;
const SUBSTRING_WEIGHT: f64 = 0.6;
const SHORT_RUN_SCORE: f64 = 1.5;
/// A trigram run must cover at least this fraction of its trigrams to count as matched.
const MIN_RUN_COVERAGE: f64 = 0.25;
/// Results must score at least this fraction of the best result.
const RELATIVE_GATE: f64 = 0.35;

const STOP_WORDS: &str = "a an the and or but if then else of to in on at by for from with \
without into onto over under about as is are was were be been being am do does did doing done \
have has had having i me my we our us you your he she it its they them their this that these \
those there here what which who whom whose when where why how can could should would will shall \
may might must not no nor so than too very just also only own same such some any each every all \
both few more most other again further once please tell know want need";

fn stop_words() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| STOP_WORDS.split_whitespace().collect())
}

/// Scripts usually written without spaces between words. Thai and Lao tone marks fall inside
/// their blocks, so they stay attached to the run instead of splitting it.
fn spaceless(c: char) -> bool {
    matches!(c as u32,
        0x0E00..=0x0EFF      // Thai, Lao
        | 0x1000..=0x109F    // Myanmar
        | 0x1100..=0x11FF    // Hangul Jamo
        | 0x1780..=0x17FF    // Khmer
        | 0x3040..=0x30FF    // Hiragana, Katakana
        | 0x31F0..=0x31FF    // Katakana extensions
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xAC00..=0xD7AF    // Hangul syllables
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFF66..=0xFF9F    // Halfwidth katakana
        | 0x20000..=0x2FA1F  // CJK extensions B–F and compatibility supplement
    )
}

/// Combining marks belong to the word they modify, as in FTS5's unicode61 tokenizer.
fn combining(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05C7 | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x0900..=0x0DFF
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// Splits text into lower-cased words (space-delimited scripts) and spaceless-script runs.
/// Underscores and punctuation separate words, matching the index tokenizer.
pub fn tokens(text: &str) -> (Vec<String>, Vec<String>) {
    let mut words = Vec::new();
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut in_run = false;
    let mut flush = |current: &mut String, in_run: bool| {
        if !current.is_empty() {
            let token = std::mem::take(current);
            if in_run {
                runs.push(token);
            } else {
                words.push(token);
            }
        }
    };
    for c in text.chars() {
        if spaceless(c) {
            if !in_run {
                flush(&mut current, false);
            }
            in_run = true;
            current.push(c);
        } else if c.is_alphanumeric() || combining(c) {
            if in_run {
                flush(&mut current, true);
            }
            in_run = false;
            current.extend(c.to_lowercase());
        } else {
            flush(&mut current, in_run);
        }
    }
    flush(&mut current, in_run);
    (words, runs)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub word: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    /// Character trigrams of the run. Empty for runs shorter than three characters, which are
    /// matched as plain substrings.
    pub grams: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
    pub runs: Vec<Run>,
}
impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.runs.is_empty()
    }
    fn units(&self) -> usize {
        self.terms.len() + self.runs.len()
    }
}

fn trigrams(run: &str) -> Vec<String> {
    let chars: Vec<char> = run.chars().collect();
    if chars.len() < 3 {
        return Vec::new();
    }
    let windows: Vec<String> = chars.windows(3).map(|w| w.iter().collect()).collect();
    if windows.len() <= MAX_GRAMS_PER_RUN {
        let mut seen = HashSet::new();
        return windows
            .into_iter()
            .filter(|g| seen.insert(g.clone()))
            .collect();
    }
    // Long runs: sample evenly so the whole run still contributes.
    let step = windows.len() as f64 / MAX_GRAMS_PER_RUN as f64;
    let mut seen = HashSet::new();
    (0..MAX_GRAMS_PER_RUN)
        .map(|i| windows[(i as f64 * step) as usize].clone())
        .filter(|g| seen.insert(g.clone()))
        .collect()
}

/// Turns a query into deduplicated search terms with their aliases, plus spaceless runs.
pub fn parse_query(query: &str, alias_groups: &[Vec<String>]) -> ParsedQuery {
    let (words, runs) = tokens(query);
    let stop = stop_words();
    let mut parsed = ParsedQuery::default();
    let mut seen = HashSet::new();
    for word in words {
        let length = word.chars().count();
        if !(2..=MAX_TERM_CHARS).contains(&length)
            || stop.contains(word.as_str())
            || !seen.insert(word.clone())
        {
            continue;
        }
        let mut aliases: Vec<String> = Vec::new();
        for group in alias_groups.iter().filter(|g| g.contains(&word)) {
            for alias in group {
                if *alias != word && !aliases.contains(alias) {
                    aliases.push(alias.clone());
                }
            }
        }
        parsed.terms.push(Term { word, aliases });
        if parsed.terms.len() == MAX_TERMS {
            break;
        }
    }
    let mut seen = HashSet::new();
    for run in runs {
        if run.chars().count() > 256 || !seen.insert(run.clone()) {
            continue;
        }
        parsed.runs.push(Run {
            grams: trigrams(&run),
            text: run,
        });
        if parsed.runs.len() == MAX_TERMS {
            break;
        }
    }
    parsed
}

/// A double-quoted FTS5 string: matched as a token (or phrase), never parsed as query syntax.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Eligibility in SQL, applied before any limit so expired or not-yet-valid memories can't
/// crowd valid ones out of the shortlist. `julianday` reads RFC 3339 and legacy naive
/// timestamps alike and yields NULL (ineligible) for anything unparseable, matching
/// `Memory::eligible`, which still has the final say.
const ELIGIBLE: &str = "m.lifecycle_state='accepted' AND m.verification_state<>'refuted' \
     AND (?2 IS NULL OR m.layer=?2) \
     AND (m.expires_at IS NULL OR julianday(m.expires_at) >= julianday('now')) \
     AND (m.valid_from IS NULL OR julianday(m.valid_from) <= julianday('now')) \
     AND (m.valid_until IS NULL OR julianday(m.valid_until) > julianday('now'))";

/// BM25 hits for one FTS5 expression, as (rowid, score) with larger scores better.
fn fts_hits(
    store: &MemoryStore,
    table: &str,
    weights: &str,
    expression: &str,
    layer: Option<&str>,
) -> Result<Vec<(i64, f64)>> {
    let sql = format!(
        "SELECT m.rowid, bm25({table}{weights}) FROM {table} JOIN memories m ON m.rowid={table}.rowid \
         WHERE {table} MATCH ?1 AND {ELIGIBLE} ORDER BY 2 LIMIT {HITS_PER_TERM}"
    );
    let mut statement = store.conn.prepare_cached(&sql)?;
    let hits = statement
        .query_map(params![expression, layer], |r| {
            Ok((r.get::<_, i64>(0)?, -r.get::<_, f64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(hits)
}

fn porter(store: &MemoryStore, expression: &str, layer: Option<&str>) -> Result<Vec<(i64, f64)>> {
    fts_hits(store, "memory_fts", ", 1.0, 0.5", expression, layer)
}

fn trigram(store: &MemoryStore, expression: &str, layer: Option<&str>) -> Result<Vec<(i64, f64)>> {
    fts_hits(store, "memory_fts_tri", "", expression, layer)
}

/// Substring matches for needles too short for trigrams (one or two CJK characters).
fn substring(store: &MemoryStore, needle: &str, layer: Option<&str>) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT m.rowid FROM memories m WHERE m.content LIKE ?1 ESCAPE '\\' AND {ELIGIBLE} LIMIT {HITS_PER_TERM}"
    );
    let mut statement = store.conn.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params![format!("%{}%", escape_like(needle)), layer], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Default)]
struct Evidence {
    raw: f64,
    /// Match strength per query unit (term or run index), 0–1.
    units: BTreeMap<usize, f64>,
    matched: Vec<String>,
}
impl Evidence {
    fn add(&mut self, unit: usize, strength: f64, score: f64, label: String) {
        self.raw += score;
        let entry = self.units.entry(unit).or_insert(0.0);
        *entry = entry.max(strength);
        if !self.matched.contains(&label) {
            self.matched.push(label);
        }
    }
}

pub struct Hit {
    pub memory: Memory,
    pub why: Why,
}

/// BM25 hits of one trigram run, kept until memory text is available to measure coverage.
struct RunHits<'a> {
    unit: usize,
    run: &'a Run,
    hits: Vec<(i64, f64)>,
}

fn layer_boost(layer: &str) -> f64 {
    match layer {
        "project" => 1.5,
        "user" => 1.2,
        _ => 1.0,
    }
}

/// Ranked hits for `parsed`, best first, at most `top_k`. Returns the hits and how many
/// candidates were considered.
pub fn search(
    store: &MemoryStore,
    parsed: &ParsedQuery,
    layer: Option<&str>,
    top_k: usize,
) -> Result<(Vec<Hit>, usize)> {
    if parsed.is_empty() || top_k == 0 {
        return Ok((Vec::new(), 0));
    }
    let mut evidence: HashMap<i64, Evidence> = HashMap::new();
    for (unit, term) in parsed.terms.iter().enumerate() {
        let mut direct = HashSet::new();
        for (rowid, score) in porter(store, &quoted(&term.word), layer)? {
            direct.insert(rowid);
            evidence
                .entry(rowid)
                .or_default()
                .add(unit, 1.0, score, term.word.clone());
        }
        let mut via_alias = false;
        for alias in &term.aliases {
            for (rowid, score) in porter(store, &quoted(alias), layer)? {
                via_alias = true;
                if !direct.contains(&rowid) {
                    evidence.entry(rowid).or_default().add(
                        unit,
                        1.0,
                        score * ALIAS_WEIGHT,
                        format!("{} → {alias} (alias)", term.word),
                    );
                }
            }
        }
        // A word glued to CJK text, or a prefix like `auth` for `authentication`, has no
        // token of its own; fall back to a substring match only when nothing else matched.
        if direct.is_empty() && !via_alias && term.word.chars().count() >= 3 {
            for (rowid, score) in trigram(store, &quoted(&term.word), layer)? {
                evidence.entry(rowid).or_default().add(
                    unit,
                    1.0,
                    score * SUBSTRING_WEIGHT,
                    format!("{} (substring)", term.word),
                );
            }
        }
    }
    // Trigram run hits need memory content to measure coverage, so collect them first.
    let mut run_hits: Vec<RunHits> = Vec::new();
    for (index, run) in parsed.runs.iter().enumerate() {
        let unit = parsed.terms.len() + index;
        if run.grams.is_empty() {
            for rowid in substring(store, &run.text, layer)? {
                evidence.entry(rowid).or_default().add(
                    unit,
                    1.0,
                    SHORT_RUN_SCORE,
                    format!("{} (substring)", run.text),
                );
            }
        } else {
            let expression = run
                .grams
                .iter()
                .map(|g| quoted(g))
                .collect::<Vec<_>>()
                .join(" OR ");
            run_hits.push(RunHits {
                unit,
                run,
                hits: trigram(store, &expression, layer)?,
            });
        }
    }
    // Shortlist by preliminary score before reading and sanitizing any memory text.
    let mut preliminary: HashMap<i64, f64> = evidence
        .iter()
        .map(|(rowid, e)| (*rowid, e.raw * (1.0 + e.units.len() as f64)))
        .collect();
    for RunHits { hits, .. } in &run_hits {
        for (rowid, score) in hits {
            *preliminary.entry(*rowid).or_insert(0.0) += score;
        }
    }
    let mut shortlist: Vec<(i64, f64)> = preliminary.into_iter().collect();
    shortlist.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    shortlist.truncate(CANDIDATES);
    let rowids: Vec<i64> = shortlist.iter().map(|(rowid, _)| *rowid).collect();
    let mut memories = store.memories_by_rowid(&rowids)?;
    memories.retain(|_, m| m.eligible());
    for RunHits { unit, run, hits } in &run_hits {
        let total = run.grams.len();
        for (rowid, score) in hits {
            let Some(memory) = memories.get(rowid) else {
                continue;
            };
            let content = memory.content.to_lowercase();
            let found = run
                .grams
                .iter()
                .filter(|g| content.contains(g.as_str()))
                .count();
            let fraction = found as f64 / total as f64;
            if fraction >= MIN_RUN_COVERAGE {
                evidence.entry(*rowid).or_default().add(
                    *unit,
                    fraction,
                    score * fraction,
                    format!("{} ({found}/{total})", run.text),
                );
            }
        }
    }
    let units = parsed.units() as f64;
    let mut scored: Vec<(f64, f64, i64)> = Vec::new();
    for (rowid, memory) in &memories {
        let Some(e) = evidence.get(rowid) else {
            continue;
        };
        let coverage = e.units.values().sum::<f64>() / units;
        if coverage <= 0.0 {
            continue;
        }
        let score = e.raw * (0.5 + 0.5 * coverage) * layer_boost(&memory.layer) * memory.confidence;
        scored.push((score, coverage, *rowid));
    }
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| memories[&a.2].id.cmp(&memories[&b.2].id))
    });
    let candidates = scored.len();
    let Some(&(best, _, _)) = scored.first() else {
        return Ok((Vec::new(), 0));
    };
    let min_coverage = if parsed.units() >= 3 { 0.3 } else { 0.0 };
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for (score, coverage, rowid) in scored {
        if best <= 0.0 || score < best * RELATIVE_GATE || coverage < min_coverage {
            continue;
        }
        let memory = memories
            .remove(&rowid)
            .expect("scored rowids come from memories");
        if !seen.insert(memory.content.clone()) {
            continue;
        }
        let e = &evidence[&rowid];
        hits.push(Hit {
            why: Why {
                memory_id: memory.id.clone(),
                score: (score * 1000.0).round() / 1000.0,
                coverage: (coverage * 100.0).round() / 100.0,
                matched: e.matched.clone(),
                freshness: None,
            },
            memory,
        });
        if hits.len() == top_k {
            break;
        }
    }
    Ok((hits, candidates))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_by_script_and_keep_thai_marks_attached() {
        let (words, runs) = tokens("ใช้ pnpm แทน npm; UserService.getById snake_case");
        assert_eq!(
            words,
            ["pnpm", "npm", "userservice", "getbyid", "snake", "case"]
        );
        assert_eq!(runs, ["ใช้", "แทน"]);
        let (words, runs) = tokens("我们选择了PostgreSQL因为需要事务");
        assert_eq!(words, ["postgresql"]);
        assert_eq!(runs, ["我们选择了", "因为需要事务"]);
        let (words, runs) = tokens("デプロイの前にマイグレーション、then deploy");
        assert_eq!(runs, ["デプロイの前にマイグレーション"]);
        assert_eq!(words, ["then", "deploy"]);
    }

    #[test]
    fn parse_drops_stop_words_and_expands_aliases() {
        let groups = vec![vec!["prod".to_string(), "production".to_string()]];
        let parsed = parse_query("How do I release to prod?", &groups);
        let words: Vec<&str> = parsed.terms.iter().map(|t| t.word.as_str()).collect();
        assert_eq!(words, ["release", "prod"]);
        assert_eq!(parsed.terms[1].aliases, ["production"]);
        assert!(parse_query("what is it?", &groups).is_empty());
        let parsed = parse_query("ไลเซนส์ 事务", &[]);
        assert_eq!(parsed.runs[0].grams.len(), 5);
        assert!(
            parsed.runs[1].grams.is_empty(),
            "two characters use substring matching"
        );
    }

    #[test]
    fn long_runs_are_sampled_not_truncated() {
        let long: String = "กขคงจฉชซฌญฎฏฐฑฒณดตถทธนบปผฝพฟภมยรลวศษสหฬอฮ".repeat(3);
        let grams = trigrams(&long);
        assert!(grams.len() <= MAX_GRAMS_PER_RUN);
        assert!(grams.iter().any(|g| g.starts_with('ก')));
    }

    #[test]
    fn quoting_neutralizes_fts_syntax() {
        assert_eq!(quoted(r#"a" OR "b"#), r#""a"" OR ""b""#);
        assert_eq!(escape_like("50%_off\\"), "50\\%\\_off\\\\");
    }
}
