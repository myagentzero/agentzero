# Consolidation Cron Integration

## Overview

The `src/cron/consolidation.rs` module provides **nightly memory consolidation** functionality for the AgentZero system. It distills operational activity from the past 24 hours into actionable summaries stored in long-term memory.

## Architecture

### Module Structure

- **Location**: `src/cron/consolidation.rs`
- **Declared in**: `src/cron/mod.rs:5`
- **Public API**:
  - `create_consolidation_job(config: &Config) -> Result<CronJob>`
  - `create_consolidation_job_with_schedule(config: &Config, cron_expr: &str, tz: Option<String>) -> Result<CronJob>`

### Job Configuration

- **Job Type**: `JobType::Agent` (not a shell command)
- **Default Schedule**: `0 3 * * *` (3:00 AM daily)
- **Session Target**: `SessionTarget::Isolated` (doesn't interfere with main sessions)
- **Job Name Marker**: `__consolidate_nightly`
- **Recurring**: Yes (`delete_after_run: false`)
- **Light Context**: No (needs full workspace context)

## Execution Flow

```
┌─────────────────────┐
│  CronJob Created    │
│ (consolidation.rs)  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ Scheduler Polling   │
│ (scheduler.rs:36)   │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Due Jobs Query     │
│ (scheduler.rs:41)   │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ execute_and_persist │
│ (scheduler.rs:136)  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│   run_agent_job     │
│ (scheduler.rs:153)  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Agent Execution    │
│ (agent::run)        │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  persist_job_result │
│ (scheduler.rs:219)  │
└─────────────────────┘
```

## Integration Points

### 1. Scheduler (src/cron/scheduler.rs)

- **Line 78-80**: Job type routing - dispatches to `run_agent_job()` for `JobType::Agent`
- **Line 153-217**: `run_agent_job()` - executes the consolidation prompt as an agent task
  - Checks security policy (autonomy level, rate limits)
  - Records action in security budget
  - Prefixes prompt with `[cron:{job_id} {name}]`
  - Supports `light_context` mode for compact runs
  - Invokes `crate::agent::run()` with the prompt

### 2. Cron Store (src/cron/store.rs)

- **`add_agent_job()`**: Persists job configuration to SQLite database
- Job parameters stored:
  - Schedule (cron expression + timezone)
  - Prompt text
  - Session target (Main or Isolated)
  - Model override (optional)
  - Delivery config (optional)
  - Deletion policy

### 3. Module Registration

- **src/cron/mod.rs:5**: `pub mod consolidation;`
- Functions are exported but not re-exported at the crate level
- Must be called explicitly: `crate::cron::consolidation::create_consolidation_job(&config)`

## What the Consolidation Job Does

The consolidation agent is instructed to:

1. **Recall recent memories**
   - Use `memory_recall` with category `'daily'` and `since '24h'`
   - Also recall category `'conversation'` for chat observations
   - Look for patterns, discoveries, and goal progress

2. **Classify findings** into categories:
   - **Recurring errors**: Problems that appeared more than once
   - **Successful strategies**: Approaches that worked well
   - **New discoveries**: Information or capabilities learned
   - **Blocked goals**: Objectives that couldn't be completed and why

3. **Synthesize summary**
   - Create concise summary (max 500 words)
   - Focus on actionable learnings
   - Emphasize what should change going forward

4. **Store results**
   - Use `memory_store` with category `"system"`
   - Key format: `consolidation_YYYY-MM-DD`

5. **Update MEMORY.md** (if exists)
   - Read existing `MEMORY.md` using `file_read`
   - Append dated section with top 3 learnings using `file_write`
   - Format:
     ```markdown
     ## Consolidation — YYYY-MM-DD
     1. <learning 1>
     2. <learning 2>
     3. <learning 3>
     ```

6. **Handle no activity**
   - If no meaningful activity, store brief note confirming check was performed
   - Skip `MEMORY.md` update

## Current State

- **Git Status**: Modified (`M src/cron/consolidation.rs`)
- **Automatic Creation**: Not enabled by default
- **Manual Creation**: Must call `create_consolidation_job(&config)` explicitly
- **Likely Trigger Points** (not yet implemented):
  - CLI command (e.g., `agentzero cron add-consolidation`)
  - Onboarding flow
  - Config-driven auto-setup
  - Dashboard/API endpoint

## Security & Safety

### Security Policy Enforcement

The consolidation job is subject to security checks:

- **Autonomy Level**: Must not be `ReadOnly` (scheduler.rs:158)
- **Rate Limiting**: Respects `max_actions_per_hour` (scheduler.rs:165)
- **Action Budget**: Consumes one action from budget (scheduler.rs:172)
- **Approval**: Agent jobs run with `approved: false` by default (scheduler.rs:146)

### High-Frequency Warning

The scheduler warns if agent jobs run more frequently than every 5 minutes (scheduler.rs:280-305). Consolidation's default 24-hour schedule is well within safe limits.

## Testing

The module includes comprehensive tests (lines 82-178):

- ✅ Valid job creation
- ✅ Correct default schedule (3:00 AM)
- ✅ Custom schedule with timezone
- ✅ Prompt contains required instructions:
  - `memory_recall`
  - `memory_store`
  - `consolidation_YYYY-MM-DD` key format
  - `MEMORY.md` handling

## Related Files

- **src/cron/consolidation.rs**: Job creation and prompt definition
- **src/cron/scheduler.rs**: Execution engine
- **src/cron/store.rs**: Persistence layer
- **src/cron/types.rs**: Job type definitions
- **src/cron/mod.rs**: Module declarations and exports

## Next Steps

To actually use consolidation in production:

1. Add CLI command or auto-initialization
2. Verify `memory_recall`, `memory_store`, and file I/O tools are available to cron agents
3. Consider making consolidation opt-in via config flag
4. Add observability (logging consolidation run results)
5. Implement retry/failure handling for critical consolidation failures
