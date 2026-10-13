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

/// Words that point at "the project" or "everything" rather than a topic. They still count
/// toward ranking, but a memory must match something else to be returned, so "summarize the
/// project" answers nothing instead of every memory that mentions a project.
const GENERIC_WORDS: &str = "project projects information info overview summary summaries \
summarize summarise everything anything something thing things stuff details detail system \
systems use uses using";

/// Function words in scripts written without spaces. A run made only of these says nothing, like
/// an English stop word, and is dropped.
const SPACELESS_STOP: &str = "\
มี อะไร บ้าง ไหม มั้ย ไหน ยังไง อย่างไร ทำไม เมื่อไร เมื่อไหร่ ใคร ของ ที่ ใน และ หรือ กับ จาก \
เกี่ยวกับ เรื่อง นี้ นั้น นี่ ทุก ทุกอย่าง ทั้งหมด ทั้ง หมด อยู่ คือ เป็น ได้ บอก ช่วย หน่อย ขอ \
ครับ ค่ะ คะ นะ ให้ แล้ว ว่า ก็ จะ \
什么 哪些 有 的 吗 呢 是 这个 关于 所有 全部 一切 \
何 について の は を が に です か 全て すべて \
무엇 뭐 의 은 는 이 가 을 를 에 대한 모든 전체";

/// Topic-free words in those scripts (project, information, summary, overview, system, use).
const SPACELESS_GENERIC: &str = "\
ข้อมูล โปรเจกต์ โปรเจค โปรเจ็กต์ โปรเจ็ค โครงการ ระบบ สรุป ภาพรวม รายละเอียด บันทึก ความจำ ใช้ \
信息 资料 项目 概况 概要 总结 系统 \
プロジェクト 情報 まとめ 一覧 メモ システム \
정보 프로젝트 요약 개요 시스템";

fn word_set(words: &'static str) -> (HashSet<&'static str>, usize) {
    let set: HashSet<&'static str> = words.split_whitespace().collect();
    let longest = set.iter().map(|w| w.chars().count()).max().unwrap_or(0);
    (set, longest)
}

/// How a spaceless run reads once split into known words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    /// Only function words: dropped.
    Stop,
    /// Function words and generic words: kept for ranking, but can't qualify a memory.
    Generic,
    /// Contains something else: a topic.
    Topic,
}

/// True when the whole run splits into words from `words`. A one-character word (a particle
/// like の or 가) may only follow a longer word, so a short topic word such as かに is never
/// mistaken for two particles.
fn splits_into(run: &[char], words: &[&(HashSet<&'static str>, usize)]) -> bool {
    let longest = words.iter().map(|(_, l)| *l).max().unwrap_or(0);
    // reachable[i]: the prefix of length i splits; Some(true) when its last word was longer
    // than one character.
    let mut reachable: Vec<Option<bool>> = vec![None; run.len() + 1];
    reachable[0] = Some(false);
    for start in 0..run.len() {
        let Some(after_long) = reachable[start] else {
            continue;
        };
        for end in start + 1..=(start + longest).min(run.len()) {
            let short = end - start == 1;
            if short && !after_long {
                continue;
            }
            let piece: String = run[start..end].iter().collect();
            if words.iter().any(|(set, _)| set.contains(piece.as_str())) {
                let long = !short;
                reachable[end] = Some(reachable[end].unwrap_or(false) || long);
            }
        }
    }
    reachable[run.len()].is_some()
}

fn run_kind(run: &str) -> RunKind {
    static STOP: OnceLock<(HashSet<&'static str>, usize)> = OnceLock::new();
    static GENERIC: OnceLock<(HashSet<&'static str>, usize)> = OnceLock::new();
    let stop = STOP.get_or_init(|| word_set(SPACELESS_STOP));
    let generic = GENERIC.get_or_init(|| word_set(SPACELESS_GENERIC));
    let chars: Vec<char> = run.chars().collect();
    if stop.0.contains(run) || splits_into(&chars, &[stop]) {
        RunKind::Stop
    } else if generic.0.contains(run) || splits_into(&chars, &[stop, generic]) {
        RunKind::Generic
    } else {
        RunKind::Topic
    }
}

/// Words written as part of a compound in the query (`project_id`, `project.info`,
/// `GOOGLE_CLOUD_PROJECT`, `project-wide`): identifiers, never generic.
fn compound_words(query: &str) -> HashSet<String> {
    let mut found = HashSet::new();
    for chunk in query.split_whitespace() {
        let joined = chunk
            .trim_matches(|c: char| !c.is_alphanumeric())
            .chars()
            .any(|c| matches!(c, '_' | '.' | '/' | ':' | '-'));
        if joined {
            found.extend(tokens(chunk).0);
        }
    }
    found
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

/// Thai, Lao, Myanmar and Khmer letters. A lone one is a whole word only when it stands apart
/// (ณ in หัก ณ ที่จ่าย); inside a word (สาธารณะ) it means nothing.
fn abugida(c: char) -> bool {
    matches!(c as u32, 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

/// True when `letter` occurs in `content` with no letter of its script (or combining mark)
/// directly before or after it.
fn standalone(content: &str, letter: &str) -> bool {
    content.match_indices(letter).any(|(at, _)| {
        let apart = |c: Option<char>| c.is_none_or(|c| !(abugida(c) || combining(c)));
        apart(content[..at].chars().next_back())
            && apart(content[at + letter.len()..].chars().next())
    })
}

/// True when `word` occurs in `content` glued to text written without spaces (ใช้Redisเป็น,
/// 订单服务使用PostgreSQL) and is a whole word on its other side, so `redis` matches inside Thai but
/// not inside `Redisson`. `content` and `word` are lower-case.
fn glued(content: &str, word: &str) -> bool {
    content.match_indices(word).any(|(at, _)| {
        let before = content[..at].chars().next_back();
        let after = content[at + word.len()..].chars().next();
        let script = |c: Option<char>| c.is_some_and(|c| spaceless(c) || combining(c));
        let edge = |c: Option<char>| script(c) || c.is_none_or(|c| !c.is_alphanumeric());
        (script(before) && edge(after)) || (script(after) && edge(before))
    })
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
    /// A topic-free word ("project", "summary"): it ranks, but can't qualify a memory alone.
    pub generic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    /// Character trigrams of the run. Empty for runs shorter than three characters, which are
    /// matched as plain substrings.
    pub grams: Vec<String>,
    /// Made only of function and topic-free words (โปรเจกต์นี้, 项目概况).
    pub generic: bool,
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
    /// True when every unit is generic: the query names no topic, so nothing is returned.
    pub fn only_generic(&self) -> bool {
        !self.is_empty()
            && self.terms.iter().all(|t| t.generic)
            && self.runs.iter().all(|r| r.generic)
    }
    fn generic_unit(&self, unit: usize) -> bool {
        match self.terms.get(unit) {
            Some(term) => term.generic,
            None => self.runs[unit - self.terms.len()].generic,
        }
    }
    /// Units as the coverage gate counts them: a long spaceless run holds several words, so it
    /// counts as one unit per four trigrams, as English already counts each word.
    fn gate_units(&self) -> usize {
        self.terms.len()
            + self
                .runs
                .iter()
                .map(|r| (r.grams.len() / GRAMS_PER_WORD).max(1))
                .sum::<usize>()
    }
}

/// Trigrams in a typical spaceless-script word, for the coverage gate.
const GRAMS_PER_WORD: usize = 4;

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

/// Turns a query into deduplicated search terms with their aliases, plus spaceless runs, using
/// the built-in generic words.
pub fn parse_query(query: &str, alias_groups: &[Vec<String>]) -> ParsedQuery {
    parse_query_with(query, alias_groups, &[], true)
}

/// `parse_query` with your own generic words (`[retrieval] generic_words`) added to, or
/// replacing, the built-in ones.
pub fn parse_query_with(
    query: &str,
    alias_groups: &[Vec<String>],
    extra_generic: &[String],
    builtin_generic: bool,
) -> ParsedQuery {
    static GENERIC: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let builtin = GENERIC.get_or_init(|| GENERIC_WORDS.split_whitespace().collect());
    let compound = compound_words(query);
    let is_generic = |word: &str| {
        !compound.contains(word)
            && ((builtin_generic && builtin.contains(word))
                || extra_generic.iter().any(|w| w == word))
    };
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
        let generic = is_generic(&word);
        parsed.terms.push(Term {
            word,
            aliases,
            generic,
        });
        if parsed.terms.len() == MAX_TERMS {
            break;
        }
    }
    let mut seen = HashSet::new();
    for run in runs {
        if run.chars().count() > 256 || !seen.insert(run.clone()) {
            continue;
        }
        let kind = if builtin_generic {
            run_kind(&run)
        } else {
            RunKind::Topic
        };
        if kind == RunKind::Stop {
            continue;
        }
        // A lone kana, or only marks and repetition signs (ๆ, ฯ), names nothing.
        let chars: Vec<char> = run.chars().collect();
        let kana =
            |c: &char| matches!(*c as u32, 0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F);
        if (chars.len() == 1 && chars.iter().all(kana))
            || chars.iter().all(|c| {
                combining(*c)
                    || matches!(*c, 'ๆ' | 'ฯ')
                    || matches!(*c as u32, 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E)
            })
        {
            continue;
        }
        parsed.runs.push(Run {
            grams: trigrams(&run),
            generic: kind == RunKind::Generic || extra_generic.contains(&run),
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

/// Trigram hits of a word that also matched as a token, kept until memory text shows whether
/// the word is glued to a spaceless script there. They score on the word scale (`word_score`):
/// trigram BM25 runs smaller, which let the relative gate drop confirmed whole-word matches.
struct GluedHits<'a> {
    unit: usize,
    word: &'a str,
    word_score: f64,
    hits: Vec<(i64, f64)>,
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
    // Candidates that can only be confirmed against memory text: words glued to spaceless
    // scripts, and lone Thai-script letters.
    let mut glued_hits: Vec<GluedHits> = Vec::new();
    let mut standalone_hits: Vec<(usize, &str, Vec<i64>)> = Vec::new();
    for (unit, term) in parsed.terms.iter().enumerate() {
        let mut direct = HashSet::new();
        let mut best_word_score: f64 = 0.0;
        let label = if term.generic {
            format!("{} (generic)", term.word)
        } else {
            term.word.clone()
        };
        for (rowid, score) in porter(store, &quoted(&term.word), layer)? {
            direct.insert(rowid);
            best_word_score = best_word_score.max(score);
            evidence
                .entry(rowid)
                .or_default()
                .add(unit, 1.0, score, label.clone());
        }
        let mut via_alias = false;
        for alias in &term.aliases {
            for (rowid, score) in porter(store, &quoted(alias), layer)? {
                via_alias = true;
                best_word_score = best_word_score.max(score * ALIAS_WEIGHT);
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
        } else if term.word.chars().count() >= 3 {
            // The word also matched as a token, which used to skip the substring search and
            // miss memories where it is glued to Thai or CJK text. Confirmed below.
            let extra: Vec<(i64, f64)> = trigram(store, &quoted(&term.word), layer)?
                .into_iter()
                .filter(|(rowid, _)| !direct.contains(rowid))
                .collect();
            if !extra.is_empty() {
                glued_hits.push(GluedHits {
                    unit,
                    word: &term.word,
                    word_score: best_word_score,
                    hits: extra,
                });
            }
        }
    }
    // Trigram run hits need memory content to measure coverage, so collect them first.
    let mut run_hits: Vec<RunHits> = Vec::new();
    for (index, run) in parsed.runs.iter().enumerate() {
        let unit = parsed.terms.len() + index;
        if run.grams.is_empty() {
            let lone_letter = run.text.chars().count() == 1 && run.text.chars().all(abugida);
            if lone_letter {
                standalone_hits.push((unit, &run.text, substring(store, &run.text, layer)?));
                continue;
            }
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
    for GluedHits {
        word_score, hits, ..
    } in &glued_hits
    {
        for (rowid, _) in hits {
            *preliminary.entry(*rowid).or_insert(0.0) += word_score * SUBSTRING_WEIGHT;
        }
    }
    for (_, _, rowids) in &standalone_hits {
        for rowid in rowids {
            *preliminary.entry(*rowid).or_insert(0.0) += SHORT_RUN_SCORE;
        }
    }
    let mut shortlist: Vec<(i64, f64)> = preliminary.into_iter().collect();
    shortlist.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    shortlist.truncate(CANDIDATES);
    let rowids: Vec<i64> = shortlist.iter().map(|(rowid, _)| *rowid).collect();
    let mut memories = store.memories_by_rowid(&rowids)?;
    memories.retain(|_, m| m.eligible());
    for GluedHits {
        unit,
        word,
        word_score,
        hits,
    } in &glued_hits
    {
        for (rowid, _) in hits {
            if memories
                .get(rowid)
                .is_some_and(|m| glued(&m.content.to_lowercase(), word))
            {
                evidence.entry(*rowid).or_default().add(
                    *unit,
                    1.0,
                    word_score * SUBSTRING_WEIGHT,
                    format!("{word} (glued)"),
                );
            }
        }
    }
    for (unit, letter, rowids) in &standalone_hits {
        for rowid in rowids {
            if memories
                .get(rowid)
                .is_some_and(|m| standalone(&m.content, letter))
            {
                evidence.entry(*rowid).or_default().add(
                    *unit,
                    1.0,
                    SHORT_RUN_SCORE,
                    format!("{letter} (standalone)"),
                );
            }
        }
    }
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
        // Generic words rank, but a memory must match at least one topic word to be returned.
        if !e.units.keys().any(|unit| !parsed.generic_unit(*unit)) {
            continue;
        }
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
    let min_coverage = if parsed.gate_units() >= 3 { 0.3 } else { 0.0 };
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
