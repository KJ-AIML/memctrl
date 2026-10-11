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
impl NewMemory {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.layer, "layer")?;
        if self.content.trim().is_empty() {
            bail!("memory content cannot be empty")
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
}
impl Memory {
    pub fn eligible(&self) -> bool {
        let current = Utc::now();
        self.lifecycle_state == LifecycleState::Accepted
            && self.verification_state != VerificationState::Refuted
            && self
                .expires_at
                .as_ref()
                .is_none_or(|t| parse_time(t).is_ok_and(|t| t >= current))
            && self
                .valid_from
                .as_ref()
                .is_none_or(|t| parse_time(t).is_ok_and(|t| t <= current))
            && self
                .valid_until
                .as_ref()
                .is_none_or(|t| parse_time(t).is_ok_and(|t| t > current))
    }
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
