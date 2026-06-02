# Agent Prompt Building Flow

```mermaid
graph TD
    A["Agent.turn() called<br/>with user_message"] --> B{"System prompt<br/>exists in history?"}
    B -->|No| C["build_system_prompt()"]
    B -->|Yes| D["refresh_prompt_datetime()"]

    C --> E["Collect tool hints<br/>from tools"]
    E --> F["build_system_prompt_with_mode()"]

    F --> G["Add Identity Section<br/>- SOUL.md + IDENTITY.md (if present)"]
    G --> G2["Add Runtime Section<br/>- Host, OS"]
    G2 --> H["Add Your Task Section<br/>- Instructions for LLM"]
    H --> I["Add Safety Section<br/>- General safety rules"]
    I --> J{"security_summary<br/>provided?"}
    J -->|Yes| K["Add Active Security Policy<br/>subsection"]
    J -->|No| L["Add Tools Section<br/>- Tool names & descriptions"]
    K --> L

    L --> M["Add Hardware Section<br/>if hardware tools present"]
    M --> N["Add Skills Section<br/>- Authorization + full or compact mode"]

    N --> O["Add Workspace Section<br/>- Working directory"]
    O --> P{"skip_bootstrap?"}
    P -->|No| Q{"AIEOS identity<br/>configured?"}
    P -->|Yes| R["Skip Project Context"]
    Q -->|Yes| S["Load AIEOS Identity"]
    Q -->|No| T["load_openclaw_bootstrap_files()"]

    S --> U["Convert to system prompt<br/>or fallback to OpenClaw"]
    T --> U
    U --> V["Add Project Context<br/>- AGENTS.md, TOOLS.md, USER.md, etc"]
    V --> R

    R --> W["Add Channel Capabilities<br/>- Response delivery info"]
    W --> X["Add Current Date & Time<br/>- Formatted with timezone"]
    X --> Z["Append tool_instructions<br/>if not native tools"]
    Z --> AA["Append shell_policy_instructions<br/>from autonomy_config"]

    AA --> AB["Return final system prompt"]
    AB --> AC["Add to history as<br/>system ChatMessage"]

    D --> AD["Update only datetime<br/>in existing prompt"]
    AD --> AC

```

## Key Flow Points:

1. **Initialization Check** — On first turn, builds full system prompt; on subsequent turns, just refreshes the datetime
2. **Prompt Construction** — 10 main sections built in sequence:
   - Identity (SOUL.md + IDENTITY.md — frames everything)
   - Runtime info (host/OS — stable, cache-friendly, placed early)
   - Task instructions
   - Safety guidelines + active policy
   - Tools & Hardware access
   - Skills (authorization + definitions, contiguous)
   - Workspace context
   - Project context (bootstrap files or AIEOS)
   - Channel capabilities
   - Current date/time (dynamic tail for prompt cache stability)
3. **Conditional Sections** — Security policy, hardware access, and bootstrap files are conditionally injected
4. **Post-processing** — Tool and shell policy instructions appended if applicable
5. **History Management** — Final prompt stored as system message in conversation history

The prompt is modular and respects configuration options (AIEOS vs OpenClaw, native tools, skills mode, etc.).

## Source Code References

- **Agent.turn()**: `src/agent/agent.rs:594`
- **Agent.build_system_prompt()**: `src/agent/agent.rs:486`
- **build_system_prompt_with_mode()**: `src/channels/mod.rs:4688`
- **refresh_prompt_datetime()**: `src/agent/prompt.rs:11`
