//! Who a write comes from: the person, or a coding agent acting for them.
//!
//! A coding agent's shell tool sets a variable for every command it runs (Claude Code sets
//! `CLAUDECODE=1`), so memctrl can tell the agent's `memctrl add` from yours and record it under
//! the agent's name. MCP tools are only ever called by an agent. Either way the write can't claim
//! a source the guard trusts, which keeps an agent's ordinary writes out of "From the user". That
//! isn't proof of authorship: an agent that clears the variable, or feeds a hook forged input, can
//! still write as you.
use crate::guard::Policy;

/// Variables coding agents set for the commands their agent runs, and the source memctrl records
/// for writes made under each.
pub const AGENT_MARKERS: &[(&str, &str)] = &[("CLAUDECODE", "claude-code")];

/// The source of an MCP write that asked for one the guard trusts.
pub const MCP: &str = "mcp";

/// The coding agent running this process, as the source its writes record.
pub fn agent(env: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    AGENT_MARKERS
        .iter()
        .find(|(var, _)| env(var).is_some_and(|v| !v.is_empty()))
        .map(|(_, source)| *source)
}

/// The source a write records, and why when it isn't the one `requested`. `writer` is the agent
/// making the write, or `None` for a person, who may record any source (default `manual`).
pub fn source(
    requested: Option<&str>,
    writer: Option<&str>,
    policy: &Policy,
) -> (String, Option<String>) {
    let Some(writer) = writer else {
        return (requested.unwrap_or("manual").to_string(), None);
    };
    match requested {
        Some(asked) if asked != writer && policy.trusted_sources.iter().any(|s| s == asked) => (
            writer.to_string(),
            Some(format!(
                "recorded as {writer}, not {asked}: an agent's writes count as the agent's, and only the user writes as themselves (by starting a prompt with `remember:`, or with `memctrl add` in their own terminal)"
            )),
        ),
        Some(asked) => (asked.to_string(), None),
        None => (writer.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn claude_code_is_recognised_by_its_variable() {
        assert_eq!(agent(env(&[("CLAUDECODE", "1")])), Some("claude-code"));
        assert_eq!(agent(env(&[("CLAUDECODE", "")])), None);
        assert_eq!(agent(env(&[])), None);
    }

    #[test]
    fn an_agent_can_name_any_source_except_one_the_guard_trusts() {
        let policy = Policy::default();
        assert_eq!(source(None, None, &policy), ("manual".into(), None));
        assert_eq!(source(Some("user"), None, &policy), ("user".into(), None));
        assert_eq!(
            source(None, Some("claude-code"), &policy),
            ("claude-code".into(), None)
        );
        let (recorded, note) = source(Some("user"), Some("claude-code"), &policy);
        assert_eq!(recorded, "claude-code");
        assert!(note.unwrap().contains("remember:"));
        assert_eq!(
            source(Some("docs"), Some("claude-code"), &policy),
            ("docs".into(), None)
        );
        assert_eq!(source(Some("manual"), Some(MCP), &policy).0, "mcp");
        // A user config that trusts the agent changes what counts, not what is recorded.
        let trusting = Policy {
            trusted_sources: vec!["user".into(), "claude-code".into()],
            ..Policy::default()
        };
        assert_eq!(
            source(Some("claude-code"), Some("claude-code"), &trusting),
            ("claude-code".into(), None)
        );
    }
}
