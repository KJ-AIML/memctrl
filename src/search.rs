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
/// Trigram BM25 adds up over every trigram a run shares with a memory, about three per word, so
/// a run's score is scaled to a word's before it meets word scores in the same query. Without it
/// a scrap of Thai outranked both words of `purge-tenant`.
const RUN_SCALE: f64 = 1.0 / 3.0;
/// Identifiers searched as phrases, at most.
const MAX_IDENTIFIERS: usize = 4;
/// A trigram run must cover at least this fraction of its trigrams to count as matched.
const MIN_RUN_COVERAGE: f64 = 0.25;
/// Results must score at least this fraction of the best result.
const RELATIVE_GATE: f64 = 0.35;

const STOP_WORDS: &str = "a an the and or but if then else of to in on at by for from with \
without into onto over under about as is are was were be been being am do does did doing done \
have has had having i me my we our us you your he she it its they them their this that these \
those there here what which who whom whose when where why how can could should would will shall \
may might must not no nor so than too very just also only own same such some any each every all \
both few more most other again further once please tell know want need \
don doesn didn isn aren wasn weren won wouldn shouldn couldn haven hasn hadn ll ve re";

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
ครับ ค่ะ คะ นะ ให้ แล้ว ว่า ก็ จะ เลย ด้วย สิ ซิ เถอะ หรอ เหรอ ล่ะ น่ะ จ้า จ้ะ คับ ฮะ \
什么 哪些 有 的 吗 呢 是 这个 关于 所有 全部 一切 \
何 について の は を が に です か 全て すべて \
무엇 뭐 의 은 는 이 가 을 를 에 대한 모든 전체";

/// Topic-free words in those scripts (project, information, summary, overview, system, use).
const SPACELESS_GENERIC: &str = "\
ข้อมูล โปรเจกต์ โปรเจค โปรเจ็กต์ โปรเจ็ค โครงการ ระบบ สรุป ภาพรวม รายละเอียด บันทึก ความจำ ใช้ \
信息 资料 项目 概况 概要 总结 系统 \
プロジェクト 情報 まとめ 一覧 メモ システム \
정보 프로젝트 요약 개요 시스템";

/// Thai words that often come right before a verb (ต้อง in ต้องลบ). Like the stop and generic
/// words, they show where a word ends inside a run, so a short alias key may start there.
const BEFORE_VERB: &str = "ต้อง ห้าม อย่า ควร ไม่ ยัง อยาก เคย กำลัง ค่อย เพิ่ง พึ่ง จึง ถึง แต่ ก่อน \
หลัง ถ้า พอ เพื่อ ไป มา";

/// Words that begin with a built-in alias key but mean something else (QR, apple, cashier):
/// read whole, so the key inside them isn't cut out.
const LOOKALIKES: &str = "คิวอาร์ แอปเปิ้ล แอปเปิล แคชเชียร์";

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
fn splits_into(run: &[char], words: &[&(HashSet<&str>, usize)]) -> bool {
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

type WordSet = (HashSet<&'static str>, usize);

/// The built-in stop and generic words of scripts written without spaces.
fn spaceless_words() -> (&'static WordSet, &'static WordSet) {
    static STOP: OnceLock<WordSet> = OnceLock::new();
    static GENERIC: OnceLock<WordSet> = OnceLock::new();
    (
        STOP.get_or_init(|| word_set(SPACELESS_STOP)),
        GENERIC.get_or_init(|| word_set(SPACELESS_GENERIC)),
    )
}

fn run_kind(run: &str) -> RunKind {
    run_kind_with(run, &(HashSet::new(), 0))
}

/// `run_kind`, with `extra` words (your `generic_words`, or recall's request words such as ทำ)
/// counting as generic inside a run too.
fn run_kind_with(run: &str, extra: &(HashSet<&str>, usize)) -> RunKind {
    let (stop, generic) = spaceless_words();
    let chars: Vec<char> = run.chars().collect();
    if stop.0.contains(run) || splits_into(&chars, &[stop]) {
        RunKind::Stop
    } else if generic.0.contains(run)
        || extra.0.contains(run)
        || splits_into(&chars, &[stop, generic, extra])
    {
        RunKind::Generic
    } else {
        RunKind::Topic
    }
}

/// Chunks of the query written as one compound (`project_id`, `project.info`,
/// `GOOGLE_CLOUD_PROJECT`, `reset-slots`), trimmed of surrounding punctuation, with their words.
/// Their words are identifiers, never generic.
fn compounds(query: &str) -> Vec<(String, Vec<String>)> {
    query
        .split_whitespace()
        .filter_map(|chunk| {
            let trimmed = chunk.trim_matches(|c: char| !c.is_alphanumeric());
            let joined = trimmed
                .chars()
                .any(|c| matches!(c, '_' | '.' | '/' | ':' | '-'));
            joined.then(|| (trimmed.to_string(), tokens(trimmed).0))
        })
        .collect()
}

/// Scripts usually written without spaces between words. Thai and Lao tone marks fall inside
/// their blocks, so they stay attached to the run instead of splitting it.
pub(crate) fn spaceless(c: char) -> bool {
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

/// True when `word` begins a word somewhere in `content`: after a character that isn't a letter
/// or digit, after text written without spaces, or at a camelCase hump (`connections` in
/// `maxConnections`). Never in the middle of a word, as `lag` is in `flagged`. `word` is
/// lower-case; `content` is the memory text as written.
fn starts_word(content: &str, word: &str) -> bool {
    let chars: Vec<char> = content.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let needle: Vec<char> = word.chars().collect();
    if needle.is_empty() || needle.len() > lower.len() {
        return false;
    }
    (0..=lower.len() - needle.len()).any(|at| {
        lower[at..at + needle.len()] == needle[..]
            && (at == 0 || {
                let before = chars[at - 1];
                !before.is_alphanumeric()
                    || spaceless(before)
                    || (before.is_lowercase() && chars[at].is_uppercase())
            })
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
    /// An alias key found inside a run of a script written without spaces (ลบ in ลบข้อมูล). Memory
    /// text holds it inside runs too, so it is matched as a substring, not as a token.
    pub spaceless: bool,
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

/// A query chunk of several words written as one (`reset-slots`, `src/db/ledger.ts`): one more
/// thing the query names. A memory holding the words side by side, in order, covers it; one
/// holding them apart only shares words.
#[derive(Debug, Clone, PartialEq)]
pub struct Identifier {
    /// As written in the query.
    pub text: String,
    /// Its words, searched as a phrase.
    pub words: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
    pub runs: Vec<Run>,
    pub identifiers: Vec<Identifier>,
}
impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.runs.is_empty()
    }
    /// The share of the query a memory covers: the strength (0–1) of each unit it matched, over
    /// the query's units.
    fn coverage(&self, matched: &BTreeMap<usize, f64>) -> f64 {
        let units = self.terms.len() + self.runs.len() + self.identifier_units().len();
        matched.values().sum::<f64>() / units.max(1) as f64
    }
    /// Identifiers with a word still searched (recall drops words no memory holds), each a unit
    /// after the terms and runs.
    fn identifier_units(&self) -> Vec<(usize, &Identifier)> {
        let first = self.terms.len() + self.runs.len();
        self.identifiers
            .iter()
            .filter(|i| {
                i.words
                    .iter()
                    .any(|w| self.terms.iter().any(|t| t.word == *w))
            })
            .enumerate()
            .map(|(n, i)| (first + n, i))
            .collect()
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
            None => self
                .runs
                .get(unit - self.terms.len())
                .is_some_and(|r| r.generic),
        }
    }
    /// Units as the coverage gate counts them: a long spaceless run holds several words, so it
    /// counts as one unit per four trigrams, as English already counts each word.
    pub fn gate_units(&self) -> usize {
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

/// True for text written only in scripts without spaces (Thai, CJK, Korean...).
pub(crate) fn spaceless_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(spaceless)
}

/// Alias group members written without spaces, at least two characters, longest first.
fn spaceless_keys(groups: &[Vec<String>]) -> Vec<String> {
    let mut keys: Vec<String> = groups
        .iter()
        .flatten()
        .filter(|w| w.chars().count() >= 2 && spaceless_text(w))
        .cloned()
        .collect();
    keys.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
    keys.dedup();
    keys
}

/// A run cut around the alias keys it holds: `Ok` for a key, `Err` for the text between keys.
/// With no dictionary, the run is read left to right taking the longest word it knows at each
/// point: a key is cut out, and one of the `known` words is kept whole as text, so no key is found
/// inside it. A two-letter key can still straddle two words (ลบ in ข้อมูล|บน), so one cuts only
/// where a word is known to end: at the start of the run, after a key, or after a known word.
fn split_at_keys(
    run: &str,
    keys: &[String],
    known: &[&(HashSet<&str>, usize)],
) -> Vec<Result<String, String>> {
    // A Thai vowel or tone mark belongs to the letter before it, so a word can't end before one.
    let mark = |c: char| matches!(c as u32, 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E);
    let whole = |rest: &str, len: usize| rest[len..].chars().next().is_none_or(|c| !mark(c));
    let longest = known.iter().map(|(_, l)| *l).max().unwrap_or(0);
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut after_word = true;
    let mut rest = run;
    while let Some(c) = rest.chars().next() {
        // The longest known word `rest` starts with, in bytes.
        let word = rest
            .char_indices()
            .take(longest)
            .map(|(at, c)| at + c.len_utf8())
            .filter(|end| {
                whole(rest, *end) && known.iter().any(|(set, _)| set.contains(&rest[..*end]))
            })
            .last()
            .unwrap_or(0);
        let key = keys.iter().find(|key| {
            rest.starts_with(key.as_str())
                && key.len() >= word
                && whole(rest, key.len())
                && (after_word || key.chars().count() > 2)
        });
        if let Some(key) = key {
            if !text.is_empty() {
                pieces.push(Err(std::mem::take(&mut text)));
            }
            pieces.push(Ok(key.clone()));
            rest = &rest[key.len()..];
            after_word = true;
        } else if word > 0 {
            text.push_str(&rest[..word]);
            rest = &rest[word..];
            after_word = true;
        } else {
            text.push(c);
            rest = &rest[c.len_utf8()..];
            after_word = false;
        }
    }
    if !text.is_empty() {
        pieces.push(Err(text));
    }
    pieces
}

/// The other members of every alias group that holds `word`.
fn aliases_of(word: &str, groups: &[Vec<String>]) -> Vec<String> {
    let mut aliases: Vec<String> = Vec::new();
    for group in groups.iter().filter(|g| g.iter().any(|w| w == word)) {
        for alias in group {
            if alias != word && !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
    }
    aliases
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
    let compounds = compounds(query);
    let compound: HashSet<&str> = compounds
        .iter()
        .flat_map(|(_, words)| words.iter().map(String::as_str))
        .collect();
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
        let aliases = aliases_of(&word, alias_groups);
        let generic = is_generic(&word);
        parsed.terms.push(Term {
            word,
            aliases,
            generic,
            spaceless: false,
        });
        if parsed.terms.len() == MAX_TERMS {
            break;
        }
    }
    // A compound of words being searched is also searched whole.
    for (text, words) in compounds.into_iter().take(MAX_IDENTIFIERS) {
        if words.len() >= 2
            && words
                .iter()
                .any(|w| parsed.terms.iter().any(|t| t.word == *w))
        {
            parsed.identifiers.push(Identifier { text, words });
        }
    }
    // Alias keys written without spaces (ลบ, ดีพลอย) mark words inside a run, as a dictionary
    // would: the run is cut around them, and each key searches as a word with its aliases.
    let keys = spaceless_keys(alias_groups);
    // Generic words written without spaces count inside a run, not only as a whole run.
    let extra_spaceless: (HashSet<&str>, usize) = {
        let set: HashSet<&str> = extra_generic
            .iter()
            .map(String::as_str)
            .filter(|w| spaceless_text(w))
            .collect();
        let longest = set.iter().map(|w| w.chars().count()).max().unwrap_or(0);
        (set, longest)
    };
    static BEFORE: OnceLock<WordSet> = OnceLock::new();
    static LOOKALIKE: OnceLock<WordSet> = OnceLock::new();
    let before_verb = BEFORE.get_or_init(|| word_set(BEFORE_VERB));
    let lookalikes = LOOKALIKE.get_or_init(|| word_set(LOOKALIKES));
    let (stop_spaceless, generic_spaceless) = spaceless_words();
    let mut seen_runs = HashSet::new();
    for whole in runs {
        if whole.chars().count() > 256 {
            continue;
        }
        let split = split_at_keys(
            &whole,
            &keys,
            &[
                stop_spaceless,
                generic_spaceless,
                before_verb,
                lookalikes,
                &extra_spaceless,
            ],
        );
        let cut = split.len() > 1;
        for piece in split {
            let run = match piece {
                Ok(key) => {
                    if parsed.terms.len() < MAX_TERMS && seen.insert(key.clone()) {
                        let generic = (builtin_generic && run_kind(&key) != RunKind::Topic)
                            || extra_generic.contains(&key);
                        parsed.terms.push(Term {
                            aliases: aliases_of(&key, alias_groups),
                            word: key,
                            generic,
                            spaceless: true,
                        });
                    }
                    continue;
                }
                // A letter left between two keys is a fragment, not a word.
                Err(text) if cut && text.chars().count() < 2 => continue,
                Err(text) => text,
            };
            if parsed.runs.len() == MAX_TERMS || !seen_runs.insert(run.clone()) {
                continue;
            }
            let kind = if builtin_generic {
                run_kind_with(&run, &extra_spaceless)
            } else if extra_spaceless.0.contains(run.as_str())
                || splits_into(&run.chars().collect::<Vec<_>>(), &[&extra_spaceless])
            {
                RunKind::Generic
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

/// Memories holding `needle`, a word written without spaces, anywhere in their text: its trigrams
/// as a phrase, or a plain substring when it is too short for trigrams.
fn spaceless_hits(
    store: &MemoryStore,
    needle: &str,
    layer: Option<&str>,
) -> Result<Vec<(i64, f64)>> {
    if needle.chars().count() >= 3 {
        trigram(store, &quoted(needle), layer)
    } else {
        Ok(substring(store, needle, layer)?
            .into_iter()
            .map(|rowid| (rowid, SHORT_RUN_SCORE))
            .collect())
    }
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

/// Substring hits of a word with no token of its own (`auth` for `authentication`), kept until
/// memory text shows whether the word starts a word there.
struct PrefixHits<'a> {
    unit: usize,
    word: &'a str,
    hits: Vec<(i64, f64)>,
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
    let mut prefix_hits: Vec<PrefixHits> = Vec::new();
    let mut standalone_hits: Vec<(usize, &str, Vec<i64>)> = Vec::new();
    for (unit, term) in parsed.terms.iter().enumerate() {
        let mut direct = HashSet::new();
        let mut best_word_score: f64 = 0.0;
        let label = if term.generic {
            format!("{} (generic)", term.word)
        } else {
            term.word.clone()
        };
        let direct_hits = if term.spaceless {
            spaceless_hits(store, &term.word, layer)?
        } else {
            porter(store, &quoted(&term.word), layer)?
        };
        for (rowid, score) in direct_hits {
            direct.insert(rowid);
            best_word_score = best_word_score.max(score);
            evidence
                .entry(rowid)
                .or_default()
                .add(unit, 1.0, score, label.clone());
        }
        let mut via_alias = false;
        for alias in &term.aliases {
            // An alias in a script without spaces sits inside runs of memory text.
            let alias_hits = if spaceless_text(alias) {
                spaceless_hits(store, alias, layer)?
            } else {
                porter(store, &quoted(alias), layer)?
            };
            for (rowid, score) in alias_hits {
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
        if term.spaceless {
            continue;
        }
        // A word glued to CJK text, or a prefix like `auth` for `authentication`, has no
        // token of its own; fall back to a substring match only when nothing else matched.
        if direct.is_empty() && !via_alias && term.word.chars().count() >= 3 {
            // Confirmed below against memory text: only where the word starts a word there.
            prefix_hits.push(PrefixHits {
                unit,
                word: &term.word,
                hits: trigram(store, &quoted(&term.word), layer)?,
            });
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
    // Covered, not scored: its words already scored, and a memory repeating the identifier
    // shouldn't outrank one that explains it.
    for (unit, identifier) in parsed.identifier_units() {
        let phrase = quoted(&identifier.words.join(" "));
        for (rowid, _) in porter(store, &phrase, layer)? {
            evidence.entry(rowid).or_default().add(
                unit,
                1.0,
                0.0,
                format!("{} (identifier)", identifier.text),
            );
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
            *preliminary.entry(*rowid).or_insert(0.0) += score * RUN_SCALE;
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
    for PrefixHits { hits, .. } in &prefix_hits {
        for (rowid, score) in hits {
            *preliminary.entry(*rowid).or_insert(0.0) += score * SUBSTRING_WEIGHT;
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
    for PrefixHits { unit, word, hits } in &prefix_hits {
        for (rowid, score) in hits {
            if memories
                .get(rowid)
                .is_some_and(|m| starts_word(&m.content, word))
            {
                evidence.entry(*rowid).or_default().add(
                    *unit,
                    1.0,
                    score * SUBSTRING_WEIGHT,
                    format!("{word} (substring)"),
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
                    score * fraction * RUN_SCALE,
                    format!("{} ({found}/{total})", run.text),
                );
            }
        }
    }
    let mut scored: Vec<(f64, f64, i64)> = Vec::new();
    for (rowid, memory) in &memories {
        let Some(e) = evidence.get(rowid) else {
            continue;
        };
        // Generic words rank, but a memory must match at least one topic word to be returned.
        if !e.units.keys().any(|unit| !parsed.generic_unit(*unit)) {
            continue;
        }
        let coverage = parsed.coverage(&e.units);
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

    #[test]
    fn alias_keys_cut_thai_runs_into_words() {
        let groups = vec![
            vec!["delete".to_string(), "ลบ".to_string()],
            vec![
                "prod".to_string(),
                "โปรดักชัน".to_string(),
                "โปรดักชั่น".to_string(),
            ],
        ];
        let keys = spaceless_keys(&groups);
        assert_eq!(keys, ["โปรดักชั่น", "โปรดักชัน", "ลบ"]);
        assert_eq!(
            split_at_keys("ลบข้อมูลบนโปรดักชัน", &keys, &[]),
            [
                Ok("ลบ".to_string()),
                Err("ข้อมูลบน".to_string()),
                Ok("โปรดักชัน".to_string())
            ]
        );
        // A vowel or tone mark belongs to the letter before it, so a key can't end there.
        assert_eq!(
            split_at_keys("กรุงเทพ", &["กร".to_string()], &[]),
            [Err("กรุงเทพ".to_string())]
        );

        let parsed = parse_query("ลบข้อมูลบนโปรดักชัน", &groups);
        let words: Vec<(&str, bool)> = parsed
            .terms
            .iter()
            .map(|t| (t.word.as_str(), t.spaceless))
            .collect();
        assert_eq!(words, [("ลบ", true), ("โปรดักชัน", true)]);
        assert_eq!(parsed.terms[0].aliases, ["delete"]);
        assert_eq!(parsed.terms[1].aliases, ["prod", "โปรดักชั่น"]);
        assert_eq!(parsed.runs.len(), 1);
        assert_eq!(parsed.runs[0].text, "ข้อมูลบน");
        // Known words show where words end: ลบ after ต้อง is a word, across ข้อมูล|บน it isn't,
        // and คิว inside คิวอาร์ (QR) isn't a queue.
        let words = |query: &str, groups: &[Vec<String>]| -> Vec<String> {
            parse_query(query, groups)
                .terms
                .into_iter()
                .map(|t| t.word)
                .collect()
        };
        assert_eq!(words("ต้องลบไฟล์เก่า", &groups), ["ลบ"]);
        assert!(words("ข้อมูลบนเซิร์ฟเวอร์", &groups).is_empty());
        assert!(words("ผลบวกของเทสต์", &groups).is_empty());
        let queue = vec![vec!["queue".to_string(), "คิว".to_string()]];
        assert!(words("สแกนคิวอาร์โค้ด", &queue).is_empty());
        assert_eq!(words("งานค้างในคิว", &queue), ["คิว"]);
        // English words find the Thai members of their groups too.
        assert_eq!(parse_query("delete", &groups).terms[0].aliases, ["ลบ"]);
    }

    #[test]
    fn identifiers_are_searched_whole() {
        let parsed = parse_query("how do I run make test-int, or src/db/ledger.ts?", &[]);
        let found: Vec<(&str, Vec<&str>)> = parsed
            .identifiers
            .iter()
            .map(|i| {
                (
                    i.text.as_str(),
                    i.words.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("test-int", vec!["test", "int"]),
                ("src/db/ledger.ts", vec!["src", "db", "ledger", "ts"])
            ]
        );
        // Made only of stop words, or of one word: nothing to search as a whole.
        assert!(parse_query("a how-to for deploys", &[])
            .identifiers
            .is_empty());
        assert!(parse_query("check v2.", &[]).identifiers.is_empty());
    }

    #[test]
    fn substrings_count_only_where_a_word_starts() {
        assert!(!starts_word("Retry storms get flagged", "lag"));
        assert!(starts_word("Lagging replicas are paged", "lag"));
        assert!(starts_word("Raise maxConnections to 50", "connections"));
        assert!(!starts_word("RAISE MAXCONNECTIONS", "connections"));
        assert!(starts_word("ใช้pnpmแทน npm", "pnpm"));
        assert!(starts_word("deploy/k8s-manifests", "manifests"));
    }

    #[test]
    fn request_words_inside_a_thai_run_name_no_topic() {
        let extra: Vec<String> = ["ทำ", "ต่อ", "แก้"].map(String::from).to_vec();
        for vague in ["ทำต่อเลย", "แก้ต่อด้วยนะ"] {
            let parsed = parse_query_with(vague, &[], &extra, true);
            assert!(parsed.only_generic(), "{vague}: {parsed:?}");
        }
        let topic = parse_query_with("ทำต่อเรื่องแคช", &[], &extra, true);
        assert!(!topic.only_generic(), "{topic:?}");
        // Without the built-in words, your own generic words still count inside a run.
        let own = parse_query_with("ทำต่อ", &[], &extra, false);
        assert!(own.only_generic(), "{own:?}");
    }
}
