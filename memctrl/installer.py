"""MemCtrl — SKILL.md installer for AI coding assistants.

Replicates Graphify's install pattern:
  - `uv tool install graphifyy` → `pip install memctrl`
  - `graphify install` → `memctrl install`
  - Writes SKILL.md to ~/.claude/agent/skills/memctrl/SKILL.md etc.
  - Auto-detects installed tools by checking config dir existence

Research: Graphify writes to ~/.claude/, .claude/, ~/.cursor/, .cursor/,
~/.codex/, ~/.axga/, ~/.pi/ directories. Uses YAML frontmatter SKILL.md.
"""

from __future__ import annotations

import shutil
from pathlib import Path
from typing import List, Optional

# ---------------------------------------------------------------------------
# Tool paths
# ---------------------------------------------------------------------------

TOOL_PATHS = {
    "claude_code": [
        "~/.claude/agent/skills/memctrl/SKILL.md",
        ".claude/agent/skills/memctrl/SKILL.md",
    ],
    "cursor": [
        "~/.cursor/skills/memctrl/SKILL.md",
        ".cursor/skills/memctrl/SKILL.md",
    ],
    "codex": [
        "~/.codex/skills/memctrl/SKILL.md",
    ],
    "axga": [
        "~/.axga/agent/skills/memctrl/SKILL.md",
    ],
    "pi": [
        "~/.pi/agent/skills/memctrl/SKILL.md",
    ],
    "kimi": [
        "~/.kimi/skills/memctrl/SKILL.md",
        ".kimi/skills/memctrl/SKILL.md",
    ],
}


# ---------------------------------------------------------------------------
# Install logic
# ---------------------------------------------------------------------------


def detect_installed_tools() -> List[str]:
    """Check which tool config directories exist. Returns tool names."""
    installed = []
    for tool_name, paths in TOOL_PATHS.items():
        for path in paths:
            expanded = Path(path).expanduser().resolve()
            if expanded.parent.exists():
                installed.append(tool_name)
                break
    return installed


def install_skill(
    tool: Optional[str] = None,
    project: bool = False,
    verbose: bool = True,
) -> List[str]:
    """Install SKILL.md for specified tool or all detected tools.

    Args:
        tool: Specific tool name (claude_code, cursor, codex, kimi, etc.)
        project: If True, install to project-level paths (e.g., .claude/)
        verbose: Print summary

    Returns:
        List of paths where SKILL.md was installed.
    """
    skill_template = Path(__file__).parent / "templates" / "SKILL.md"
    if not skill_template.exists():
        if verbose:
            print(f"[memctrl] ERROR: SKILL.md template not found at {skill_template}")
        return []

    targets = [tool] if tool else detect_installed_tools()
    installed_paths: List[str] = []
    summary: List[str] = []

    for target in targets:
        if target not in TOOL_PATHS:
            if verbose:
                print(f"[memctrl] Unknown tool: {target}")
            continue

        paths = TOOL_PATHS[target]
        if project:
            project_paths = [p for p in paths if not p.startswith("~/")]
            for path in project_paths:
                dest = Path(path)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(skill_template, dest)
                installed_paths.append(str(dest))
                summary.append(f"  {target} (project): {dest}")
        else:
            user_paths = [p for p in paths if p.startswith("~/")]
            for path in user_paths:
                dest = Path(path).expanduser()
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(skill_template, dest)
                installed_paths.append(str(dest))
                summary.append(f"  {target} (user): {dest}")

    if verbose:
        if installed_paths:
            print("[memctrl] SKILL.md installed to:")
            for line in summary:
                print(line)
        else:
            print("[memctrl] No tools detected. Install paths checked:")
            for tool_name, paths in TOOL_PATHS.items():
                for p in paths:
                    print(f"  {tool_name}: {p}")
            print("\nTo force install for a specific tool, use:")
            print("  memctrl install --tool kimi")

    return installed_paths
