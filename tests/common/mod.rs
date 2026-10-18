//! Shared by the integration tests that run the memctrl executable.
use std::process::Command;

/// The memctrl executable under test: `MEMCTRL_TEST_BIN` when set (the release workflow points it
/// at the binary it's about to ship), else the one this build produced. Variables a coding agent
/// sets are removed, so writes count as a person's whether the tests run in CI or under an agent.
pub fn memctrl() -> Command {
    let bin = std::env::var_os("MEMCTRL_TEST_BIN")
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_memctrl").into());
    let mut command = Command::new(bin);
    for (var, _) in memctrl::origin::AGENT_MARKERS {
        command.env_remove(var);
    }
    command
}
