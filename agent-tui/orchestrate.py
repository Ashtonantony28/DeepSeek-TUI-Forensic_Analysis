import asyncio
import os
import sys

from claude_agent_sdk import (
    query,
    ClaudeAgentOptions,
    AgentDefinition,
    AssistantMessage,
    TextBlock,
    ResultMessage,
)

# ── PROFILE: Governed (regulations exist in PLAN.md / CLAUDE.md) ──────────
# Stops for human review after each cycle. Use this profile when:
#  - PLAN.md or CLAUDE.md encode hard constraints (which they do here)
#  - At least one task in TASKS.md is explicitly gated on human confirmation
#    (TASK-513 — tag v0.1.0)
PERMISSION_MODE = "acceptEdits"          # workers may edit; risky shell ops still gated
AUTO_LOOP       = False                  # one cycle per invocation; human reviews STATUS.md
MAX_TURNS       = 40                     # hard cap per cycle — confused agents can't loop
# ──────────────────────────────────────────────────────────────────────────


def check_auth() -> None:
    """Refuse to run on a metered API key (which silently overrides OAuth)."""
    if os.environ.get("ANTHROPIC_API_KEY"):
        sys.exit(
            "ANTHROPIC_API_KEY is set and would override your Claude subscription.\n"
            "Run:  unset ANTHROPIC_API_KEY   then re-run.\n"
            "Authenticate the subscription with:  claude setup-token"
        )
    if not os.environ.get("CLAUDE_CODE_OAUTH_TOKEN"):
        print(
            "Note: CLAUDE_CODE_OAUTH_TOKEN not in env. If you haven't run "
            "`claude setup-token`, do so before running unattended.",
            file=sys.stderr,
        )


# Orchestrator system prompt. KEEP LEAN. The 5-minute prompt-cache TTL means
# this gets re-billed on most cycle gaps; worker-applicable rules live in
# CLAUDE.md (auto-loaded via setting_sources=["project"]), NOT here.
ORCHESTRATOR_SYSTEM = """
You orchestrate; you do NOT implement. Optimize for correctness first, then for
minimum rate-limit consumption.

Each cycle:
1. Read PLAN.md, TASKS.md, STATUS.md. Read nothing else unless a decision requires it.
2. Evaluate the latest STATUS.md entries against PLAN.md's definition of done.
3. Select next task(s) whose dependencies are complete.

Dispatching rules:
- Brief each worker by EMBEDDING the relevant slice of PLAN.md inline as a quoted
  block, plus exact file paths the worker should touch. NEVER tell a worker to
  "read PLAN.md" — that forces the whole document to be re-billed per worker.
- NEVER dispatch the same task or same slice to two workers. Parallelism within
  one task is allowed only if each worker gets a different, non-overlapping part.
- Never parallelize two workers editing the same file.
- BATCH small sequential tasks that touch overlapping files into ONE worker —
  paying ~20k startup overhead once instead of N times. Doc-writing tasks
  (TASK-501..TASK-507) are prime batching candidates.
- Keep concurrency at 3–5.
- Route by cost: `implementer` (Sonnet) for building; `reviewer`/`auditor` (Haiku)
  for read-only checking. Do not spawn a worker you don't need.

After workers return:
- Confirm TASKS.md was updated.
- Write a one-line evaluation per task to STATUS.md.
- Every 5 completed tasks: dispatch `reviewer` to verify STATUS claims match code.
- When STATUS.md exceeds ~3000 tokens / ~12 KB: COMPACT before continuing — move
  entries older than the last 5 tasks to STATUS_archive.md.

Scenario A onboarding: first cycle MUST dispatch `auditor` to verify the [x]
entries in TASKS.md Phases 1–3 against the real codebase before any new
implementation work. Flip any [x] that doesn't match reality back to [ ].

Sequencing (hard):
- TASK-000 (auditor) before any other work.
- Phase 4 (TASK-401..TASK-415) before Phase 5 (TASK-501..TASK-513).
- TASK-513 (tag v0.1.0) requires explicit human confirmation in the run prompt —
  refuse to tag even if all other DoD items pass.

Escalate to Opus ONLY for: ambiguous architectural decisions, Scenario A
reconciliation if the audit reveals major divergence, or plan reconciliation
passes. Default: stay on Sonnet.

Governance:
- This is the Governed profile. After dispatching and integrating workers' output
  for ONE cycle, STOP and let the human review STATUS.md before the next cycle.
- Obey every constraint in PLAN.md and CLAUDE.md.
- Never print or commit credentials.
- Never auto-run an action that is both irreversible and destructive — pause.
"""


WORKER_PROMPT = """
You implement ONE task (or one batched spec). The orchestrator has briefed you
with everything you need. Do not read PLAN/TASKS/STATUS at startup — they will
not give you anything the orchestrator didn't already include. Implement, then
follow the finishing contract in CLAUDE.md. Be terse: no step narration, one
sentence of chat confirming completion.

Before flipping [ ] → [x]: run `cargo fmt`, `cargo clippy --all-targets -- -D
warnings`, and `cargo test` for the affected crate(s). If any of those fail,
fix and re-run. Do not mark the task complete with failing tests or clippy
errors.
"""


REVIEWER_PROMPT = """
Read-only verification. Read the most recent STATUS.md entries and the actual
code they describe. Produce a concise structured assessment: what is correct,
what is missing or wrong, what correction tasks are needed. Do not modify code
(except optionally appending a `### Review (YYYY-MM-DD)` section to STATUS.md).
"""


AUDITOR_PROMPT = """
Read-only inventory of the existing codebase on branch
`claude/agent-tui-phase-3-final-vE9RD` of repo
https://github.com/Ashtonantony28/DeepSeek-TUI-Forensic_Analysis. Produce a
factual snapshot appended to STATUS.md under `## Audit (YYYY-MM-DD)`.

You MUST cover:
1. Current branch and HEAD commit hash (`git branch --show-current` and
   `git rev-parse HEAD`)
2. Workspace root path (where top-level `Cargo.toml` lives)
3. List of actual crate directories present
4. Does `cargo build --release --workspace` succeed? If not, paste the error.
5. Test count from `cargo test --workspace --no-run` compile (or actual count
   if cargo test runs fast)
6. For each [x] task in TASKS.md Phases 1–3: brief evidence it is done (file
   exists / module compiles / test name matches). Flag any [x] that doesn't
   match reality.
7. Status of the four known concerns in STATUS.md baseline:
   a. REPL sessions through execpolicy SandboxedCommand?
   b. Pipeline `repair_attempts > 1` exercised by a test?
   c. FlashCompactor multi-endpoint mock test (`with_provider`) present?
   d. `agent-tui-build-plan.md` present on the branch?
8. Any surprises: missing dependencies, broken builds, untracked files,
   divergence from `origin`.

Use Grep/Glob to navigate. Do not read whole directories. Do not modify any
code other than appending to STATUS.md.
"""


AGENTS = {
    "implementer": AgentDefinition(
        description=(
            "Implements one development task or a batched spec: writes Rust "
            "code, edits files, adds tests, runs cargo fmt/clippy/test before "
            "marking complete."
        ),
        prompt=WORKER_PROMPT,
        tools=["Read", "Write", "Edit", "Bash", "Glob", "Grep"],
        model="sonnet",
    ),
    "auditor": AgentDefinition(
        description=(
            "Read-only. Inventories the existing codebase on the working "
            "branch and writes a factual baseline to STATUS.md. First worker "
            "dispatched in Scenario A."
        ),
        prompt=AUDITOR_PROMPT,
        # Edit is allowed only so the auditor can append to STATUS.md. The
        # auditor prompt forbids modifying anything else.
        tools=["Read", "Glob", "Grep", "Bash", "Edit"],
        model="haiku",
    ),
    "reviewer": AgentDefinition(
        description=(
            "Read-only. Verifies STATUS.md claims match the real code. "
            "Dispatched every 5 completed tasks."
        ),
        prompt=REVIEWER_PROMPT,
        tools=["Read", "Glob", "Grep", "Edit"],
        model="haiku",
    ),
}


async def run_cycle(prompt: str) -> None:
    options = ClaudeAgentOptions(
        system_prompt=ORCHESTRATOR_SYSTEM,
        # Sonnet default for the orchestrator. Opus burns the rate-limit pool
        # ~5× faster than Sonnet for equivalent reasoning. Escalate per-prompt
        # only when reasoning genuinely needs it (architectural decisions,
        # major plan reconciliation).
        model="claude-sonnet-4-6",
        allowed_tools=["Read", "Edit", "Task"],
        agents=AGENTS,
        permission_mode=PERMISSION_MODE,
        setting_sources=["project"],   # loads CLAUDE.md into orchestrator and workers
        max_turns=MAX_TURNS,
        cwd=".",
    )
    async for message in query(prompt=prompt, options=options):
        if isinstance(message, AssistantMessage):
            for block in message.content:
                if isinstance(block, TextBlock):
                    print(block.text)
        elif isinstance(message, ResultMessage):
            print("\n── cycle complete ──")


def has_open_tasks() -> bool:
    try:
        with open("TASKS.md") as f:
            return "- [ ]" in f.read()
    except FileNotFoundError:
        return False


async def main(goal: str | None) -> None:
    check_auth()
    first = goal or (
        "Read PLAN.md, TASKS.md, STATUS.md. Evaluate progress and dispatch the "
        "next appropriate task(s). Brief workers with embedded slices, not file "
        "pointers. Governed profile: stop after this cycle and let the human "
        "review STATUS.md."
    )
    await run_cycle(first)
    if AUTO_LOOP:
        # Governed default is False — this branch is skipped. Kept so the
        # profile can be flipped to Autonomous by editing the constants above.
        while has_open_tasks():
            await asyncio.sleep(3)
            await run_cycle(
                "Read PLAN.md, TASKS.md, STATUS.md. Evaluate latest results "
                "and dispatch the next task(s). Compact STATUS.md if it has "
                "grown past ~3000 tokens."
            )


if __name__ == "__main__":
    goal = " ".join(sys.argv[1:]) if len(sys.argv) > 1 else None
    asyncio.run(main(goal))
