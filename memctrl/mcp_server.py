"""MemCtrl — MCP server for AI assistant integration.

Exposes memory operations as MCP tools:
    memctrl_query  — Retrieve relevant memories with trace
    memctrl_add    — Store a new memory
    memctrl_trigger — Fire a trigger event
    memctrl_tree   — Get full memory tree
    memctrl_audit  — Get trigger audit log

MCP config for Claude Code:
    {
      "mcpServers": {
        "memctrl": {
          "command": "memctrl",
          "args": ["serve"],
          "env": {}
        }
      }
    }
"""

from __future__ import annotations

import asyncio
import json
import os

# ---------------------------------------------------------------------------
# MCP imports (optional — graceful degradation if mcp not installed)
# ---------------------------------------------------------------------------

try:
    from mcp.server import Server
    from mcp.server.stdio import stdio_server
    from mcp.types import Tool, TextContent

    HAS_MCP = True
except ImportError:
    HAS_MCP = False

    # Stub classes for type checking
    class Tool:  # type: ignore
        def __init__(self, **kwargs):
            pass

    class TextContent:  # type: ignore
        def __init__(self, type="", text=""):
            pass

    class Server:  # type: ignore
        def __init__(self, name):
            pass


# ---------------------------------------------------------------------------
# Tool schemas
# ---------------------------------------------------------------------------

MCP_TOOLS = [
    Tool(
        name="memctrl_query",
        description=(
            "Query the memory tree for relevant facts about the project, "
            "session, or user preferences. Returns facts with reasoning trace."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Natural language query"},
                "layer": {
                    "type": "string",
                    "description": "Optional layer filter (project/session/user)",
                },
            },
            "required": ["query"],
        },
    ),
    Tool(
        name="memctrl_add",
        description="Add a memory to the store",
        inputSchema={
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Memory content"},
                "layer": {
                    "type": "string",
                    "description": "Target layer",
                    "enum": ["project", "session", "user"],
                },
                "source": {"type": "string", "default": "mcp"},
            },
            "required": ["content", "layer"],
        },
    ),
    Tool(
        name="memctrl_trigger",
        description="Fire a trigger event (e.g., on_session_end)",
        inputSchema={
            "type": "object",
            "properties": {
                "event": {"type": "string", "description": "Event name"},
                "context": {"type": "object", "default": {}},
            },
            "required": ["event"],
        },
    ),
    Tool(
        name="memctrl_tree",
        description="Get the full memory tree as JSON",
        inputSchema={"type": "object", "properties": {}},
    ),
    Tool(
        name="memctrl_audit",
        description="Get trigger audit log",
        inputSchema={
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "default": 50},
            },
        },
    ),
]


# ---------------------------------------------------------------------------
# Server
# ---------------------------------------------------------------------------


async def serve_mcp() -> None:
    """Start MCP server using stdio transport.

    Designed for Claude Code / Cursor MCP integration:

    ```json
    {
      "mcpServers": {
        "memctrl": {
          "command": "memctrl",
          "args": ["serve"],
          "env": {}
        }
      }
    }
    ```
    """
    if not HAS_MCP:
        print("ERROR: MCP package not installed. Install: pip install mcp")
        return

    from memctrl.store import MemoryStore
    from memctrl.rules import RuleEngine
    from memctrl.tree import MemoryTreeBuilder, get_or_build_tree
    from memctrl.retriever import MemoryRetriever

    server = Server("memctrl")

    @server.list_tools()
    async def list_tools() -> list[Tool]:
        return MCP_TOOLS

    @server.call_tool()
    async def call_tool(name: str, arguments: dict) -> list[TextContent]:
        db_path = os.environ.get("MEMCTRL_DB_PATH")
        store = MemoryStore(db_path)
        engine = RuleEngine()

        try:
            if name == "memctrl_add":
                mid = store.insert_memory(
                    layer=arguments["layer"],
                    content=arguments["content"],
                    source=arguments.get("source", "mcp"),
                )
                return [
                    TextContent(
                        type="text",
                        text=json.dumps({"id": mid, "status": "stored"}),
                    )
                ]

            elif name == "memctrl_query":
                memories = store.list_memories(arguments.get("layer"))
                if not memories:
                    return [
                        TextContent(
                            type="text",
                            text=json.dumps({"facts": [], "trace": ["no_memories"]}),
                        )
                    ]

                mem_dicts = [m.to_dict() for m in memories]
                memory_lookup = {m.id: m.to_dict() for m in memories}

                builder = MemoryTreeBuilder()
                tree = await get_or_build_tree(store, mem_dicts, builder)
                tree_dict = tree.to_dict()

                retriever = MemoryRetriever()
                result = await retriever.retrieve(
                    arguments["query"],
                    tree_dict,
                    memory_lookup=memory_lookup,
                )
                return [
                    TextContent(
                        type="text",
                        text=json.dumps(result.to_dict()),
                    )
                ]

            elif name == "memctrl_tree":
                memories = store.list_memories()
                mem_dicts = [m.to_dict() for m in memories]
                builder = MemoryTreeBuilder()
                tree = await get_or_build_tree(store, mem_dicts, builder)
                return [
                    TextContent(
                        type="text",
                        text=json.dumps(tree.to_dict()),
                    )
                ]

            elif name == "memctrl_audit":
                logs = store.get_trigger_log(arguments.get("limit", 50))
                return [
                    TextContent(
                        type="text",
                        text=json.dumps(
                            {
                                "logs": [log.to_dict() for log in logs],
                            }
                        ),
                    )
                ]

            elif name == "memctrl_trigger":
                engine.load()
                ids = engine.fire_trigger(
                    arguments["event"],
                    arguments.get("context", {}),
                    store,
                )
                return [
                    TextContent(
                        type="text",
                        text=json.dumps(
                            {
                                "status": "fired",
                                "event": arguments["event"],
                                "affected": len(ids),
                            }
                        ),
                    )
                ]

            else:
                return [TextContent(type="text", text="Unknown tool")]

        except Exception as exc:
            return [
                TextContent(
                    type="text",
                    text=json.dumps({"error": str(exc)}),
                )
            ]

    # Use stdio transport (standard for MCP). stdio_server() takes optional
    # stdin/stdout; passing no args uses sys.stdin / sys.stdout.
    async with stdio_server() as (read_stream, write_stream):
        init_options = server.create_initialization_options()
        await server.run(read_stream, write_stream, init_options)


if __name__ == "__main__":
    asyncio.run(serve_mcp())
