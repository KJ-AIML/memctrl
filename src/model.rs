//! Native memory types and semantic validation.
use crate::sanitize::validate_identifier;
use anyhow::{bail, Result};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
macro_rules! string_enum {
    ($name:ident,$default:ident,$($variant:ident=>$text:literal),+ $(,)?) => {
        #[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
        pub enum $name { $(#[serde(rename=$text)] $variant),+ }
        impl Default for $name {fn default()->Self {Self::$default}}
        impl fmt::Display for $name {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str(match self {$(Self::$variant=>$text),+})}}
        impl FromStr for $name {type Err=anyhow::Error;fn from_str(s:&str)->Result<Self>{match s { $($text=>Ok(Self::$variant),)+ _=>bail!("invalid {}",stringify!($name)) }}}
    }
}
string_enum!(ClaimType,Assertion,Observation=>"observation",Assertion=>"assertion",Hypothesis=>"hypothesis",Decision=>"decision",DerivedLesson=>"derived_lesson");
string_enum!(LifecycleState,Accepted,Candidate=>"candidate",Accepted=>"accepted",Superseded=>"superseded",Rejected=>"rejected",Archived=>"archived");
string_enum!(VerificationState,Unverified,Unverified=>"unverified",Supported=>"supported",Disputed=>"disputed",Refuted=>"refuted");
string_enum!(RelationType,DerivedFrom,DerivedFrom=>"derived_from",Supports=>"supports",Contradicts=>"contradicts",Supersedes=>"supersedes",Refutes=>"refutes");
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NewMemory {
    pub layer: String,
    pub content: String,
    pub source: String,
    pub confidence: f64,
    pub tags: Vec<String>,
    pub expires_at: Option<String>,
    pub observed_at: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub claim_type: ClaimType,
    pub lifecycle_state: LifecycleState,
    pub verification_state: VerificationState,
}
impl Default for NewMemory {
    fn default() -> Self {
        Self {
            layer: "session".into(),
            content: String::new(),
            source: "manual".into(),
            confidence: 1.0,
            tags: vec![],
            expires_at: None,
            observed_at: None,
            valid_from: None,
            valid_until: None,
            claim_type: Default::default(),
            lifecycle_state: Default::default(),
            verification_state: Default::default(),
        }
    }
}
/// The largest memory text accepted, in UTF-8 bytes. Memories are short facts; a longer text
/// belongs in a file the memory cites. Larger texts also bloated every log that copied them.
pub const MAX_MEMORY_BYTES: usize = 64 * 1024;

/// `text` cut to fit `MAX_MEMORY_BYTES` at a character boundary, saying that it was cut.
pub fn clip_to_limit(text: &str) -> String {
    if text.len() <= MAX_MEMORY_BYTES {
        return text.to_string();
    }
    const NOTE: &str = " … (cut to fit the 64 KiB memory limit)";
    let mut end = MAX_MEMORY_BYTES - NOTE.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{NOTE}", &text[..end])
}

impl NewMemory {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.layer, "layer")?;
        if self.content.trim().is_empty() {
            bail!("memory content cannot be empty")
        }
        if self.content.len() > MAX_MEMORY_BYTES {
            bail!(
                "memory is {} bytes; the limit is 64 KiB. Split it into several memories, or save the text in a file and cite it with --cite",
                self.content.len()
            )
        }
        validate_confidence(self.confidence)?;
        for t in [
            &self.expires_at,
            &self.observed_at,
            &self.valid_from,
            &self.valid_until,
        ]
        .into_iter()
        .flatten()
        {
            parse_time(t)?;
        }
        if let (Some(start), Some(end)) = (&self.valid_from, &self.valid_until) {
            if parse_time(start)? >= parse_time(end)? {
                bail!("valid_until must follow valid_from")
            }
        }
        if self.lifecycle_state == LifecycleState::Accepted
            && self.verification_state == VerificationState::Refuted
        {
            bail!("refuted memory cannot be accepted")
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub layer: String,
    pub content: String,
    pub source: String,
    pub confidence: f64,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub last_accessed_at: Option<String>,
    pub access_count: u64,
    pub expires_at: Option<String>,
    pub observed_at: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub claim_type: ClaimType,
    pub lifecycle_state: LifecycleState,
    pub verification_state: VerificationState,
    /// When a person pinned it (`memctrl pin`): pinned memories come first in the session brief.
    #[serde(default)]
    pub pinned_at: Option<String>,
}
impl Memory {
    pub fn eligible(&self) -> bool {
        self.lifecycle_state == LifecycleState::Accepted
            && self.verification_state != VerificationState::Refuted
            && temporally_valid(
                self.expires_at.as_deref(),
                self.valid_from.as_deref(),
                self.valid_until.as_deref(),
                Utc::now(),
            )
    }
}
/// True when `at` falls inside the validity window and before expiry. Unparseable timestamps
/// fail closed.
pub fn temporally_valid(
    expires_at: Option<&str>,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
    at: DateTime<Utc>,
) -> bool {
    expires_at.is_none_or(|t| parse_time(t).is_ok_and(|t| t >= at))
        && valid_from.is_none_or(|t| parse_time(t).is_ok_and(|t| t <= at))
        && valid_until.is_none_or(|t| parse_time(t).is_ok_and(|t| t > at))
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeNode {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub layer: Option<String>,
    pub memory_ids: Vec<String>,
    pub children: Vec<TreeNode>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalResult {
    pub facts: Vec<String>,
    pub sources: Vec<String>,
    pub memory_ids: Vec<String>,
    pub trace: Vec<String>,
    pub confidence: f64,
    /// One explanation per fact, in the same order. Empty for strategies that can't explain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub why: Vec<Why>,
}
/// Why a fact was returned: which query terms matched it and how strongly.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Why {
    pub memory_id: String,
    pub score: f64,
    /// Fraction of the query's terms this memory matched (0–1).
    pub coverage: f64,
    /// Matched query terms. "a → b (alias)" means the query word a matched through its alias b;
    /// "(substring)" means a partial-word match; "(glued)" a whole word written against Thai or
    /// CJK text; "(standalone)" a lone Thai-script letter standing apart; "(generic)" a topic-free
    /// word that ranks but can't qualify a memory alone; "(k/n)" counts matched trigrams.
    pub matched: Vec<String>,
    /// For memories with citations: fresh, stale or uncited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<String>,
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
pub fn parse_time(input: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(input, format) {
            return Ok(dt.and_utc());
        }
    }
    bail!("invalid timestamp")
}
pub fn validate_confidence(value: f64) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        bail!("confidence must be finite and between 0 and 1")
    }
    Ok(())
}
