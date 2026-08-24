# ADR 0005: Repository is agent-native source of truth

- Status: Accepted
- Date: 2026-08-23

## Context

Development spans coding agents and context windows. Chat history and model-specific
instruction files are unreliable shared memory.

## Decision

Keep concise navigation/rules in `AGENTS.md`, domain detail in focused docs, binding
choices in ADRs, product sequencing in one roadmap, current task state in execution
plans, and behavior in code/tests. All repository content and emitted output is
English. Do not create duplicated model-specific guide files.

## Consequences

Behavior changes include relevant doc and plan updates. Completed execution plans are
retained. Agents read only the map, relevant architecture documents, active plan, and
code/tests needed for the task.

