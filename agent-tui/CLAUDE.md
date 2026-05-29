# Working agreement for all agents

## Source of truth

- The codebase is the source of truth, not anyone's memory.
- One task per worker. Do not start work outside your assigned task or batched spec.
- If a fact is needed and you cannot find it in the briefing or the code, say so in your
  STATUS.md summary. Do not invent.

## Finishing contract (every worker, before completing)

1. Save all changes to disk.
2. Append a 3–5 line summary to STATUS.md (never overwrite) in this format:

   ```
   ### TASK-XXX (completed YYYY-MM-DD)
   - what was created/changed (with paths)
   - any decision that affects other tasks
   ```

3. Flip your task in TASKS.md from `[ ]` to `[x]`.
4. If you batched multiple tasks, flip each one and write one summary block per task.

## Output discipline

- Produce minimal chat output. Do not narrate steps ("I'll start by…", "Now I'll…",
  "Let me verify…").
- The deliverable is the code change plus the STATUS.md summary. Chat output is **one
  sentence** confirming completion.

## Read efficiency

- Use Grep/Glob to locate files before Read. Do not enumerate or read whole directories.
- When reading a known file, use line ranges if the relevant section is identifiable from
  the spec.
- **Do not read PLAN.md, TASKS.md, or STATUS.md at startup.** The orchestrator has briefed
  you with everything you need; if context is missing, say so rather than fishing for it.

## Credential safety

- Never print, log, echo, or commit any token, key, or secret.
- Credentials come from the environment only.
- Add `.env`, token files, and `~/.claude` to `.gitignore` if they are not already there.

---

## Project-specific regulations (agent-tui, v0.1.0)

These are non-negotiable. Apply to every code change.

1. **Stable Rust only.** No `#![feature(...)]`, no `cargo +nightly`, no unstable libraries.
   If a problem seems to need nightly, redesign — do not enable nightly.
2. **No Node or Python runtime dependency** in `agent-tui` itself. Ollama via HTTP is fine;
   shelling out to Python from inside agent-tui is not. (The `agent-tui-eval` harness may
   shell out to test runners — that's external infrastructure, not agent-tui runtime.)
3. **All file paths use `camino::Utf8PathBuf`** in cross-platform code paths. Never
   `std::path::PathBuf`.
4. **Secrets only in `~/.agent-tui/config.toml` with mode 0600 on Unix.** Never written
   elsewhere. Never logged. Never committed.
5. **Tool execution that touches filesystem or shell must go through
   `agent-tui-execpolicy`.** No direct `std::process::Command` in tools. This includes
   REPL subprocess sessions.
6. **Streaming responses display incrementally.** Never buffer a full response before
   rendering.
7. **Tree-sitter syntax check on every edit.** Reject malformed edits at the tool layer
   before they propagate.
8. **`--scale N > 1` is headless-only.** Reject with a clear error if passed in interactive
   mode.
9. **Mutation operators in the eval harness must use tree-sitter AST rewrites, not regex.**
10. **Cite arxiv 2603.00520 (SWE-ABS) in a comment at the top of the mutation-strengthening
    module** explaining why we use mutation-strengthened tests.
11. **All Phase 4 and Phase 5 work lands on `claude/agent-tui-phase-3-final-vE9RD`.**
    No PRs to `main` until v0.1.0 release.
12. **Hard sequence: do not start Phase 5 work until Phase 4 smoke tests pass.** Tasks are
    numbered in dependency order in TASKS.md.
13. **Compaction breaks prefix cache; only the orchestrator decides when to invoke it via
    the context layer. Workers do not run compaction directly.**
14. **Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` on any code you
    write or modify**, before flipping `[ ]` to `[x]`. If clippy flags something you
    deliberately want to keep, add a scoped `#[allow(...)]` with a one-line comment
    explaining why.
15. **Add or update tests for behaviour you add or change.** A `[ ] → [x]` transition
    without test coverage of the new behaviour is incomplete.

## Forbidden actions (route to human pause via orchestrator)

The orchestrator gates these; workers should refuse to perform them autonomously even if
prompted:

- `git push --force` / `git push --force-with-lease` over shared history
- `git branch -D` on any branch with unmerged work
- Deletion of `.git/`, `target/`, or `Cargo.lock` without an explicit instruction in the
  briefing that names the file and gives a reason
- Editing or deleting `~/.agent-tui/config.toml` from a worker (only the user does this
  via the `login` subcommand)
- Tagging or creating a release (TASK-513 explicitly requires a human pause before tagging
  v0.1.0)
- Any rotation of credentials, deletion of remote branches, or operation against a
  production system
- Editing GitHub Actions workflow files in `.github/workflows/` outside the scope of
  TASK-509 / TASK-510

## Phase 4 specific notes (the next work)

- The new crate is `agent-tui-eval`, added to the workspace `Cargo.toml` members list.
- The CLI subcommand is `agent-tui eval` wired through `agent-tui-cli`.
- `--dry-run` mode is **not a stub** — it must exercise every code path including mutation
  strengthening, JSON serialization, and the comparison output, against 2+ synthetic
  instances with known correct patches.
- Docker availability is detected at startup; the fallback to host-side `git clone` is
  fully supported, not a hack. Both paths must be tested.
- The five-instance smoke tests (TASK-412, TASK-413, TASK-414) use `--dry-run` so they
  succeed without network access or Docker.

## Phase 5 specific notes

- Each documentation file must reference real config flags / commands / behaviour that
  exist in the code as of the time it's written. Verify by Grep before writing prose.
- The GitHub Actions workflows must run successfully at least once before TASK-513
  (tagging v0.1.0). If a workflow fails on first run, fix the workflow, do not skip it.
- The npm wrapper's postinstall script downloads from GitHub Releases — it cannot be
  smoke-tested until release.yml has produced its first set of binaries. That's a
  bootstrapping cycle: tag a pre-release, let release.yml run, then verify npm install
  pulls correctly, then tag v0.1.0.
