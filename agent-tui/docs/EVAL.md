# Evaluation harness

`agent-tui eval` runs the SWE-bench evaluation harness. It measures the
agent's ability to produce correct patches for real-world GitHub issues and
reports both a standard `pass@1` and a mutation-strengthened
`semantic_pass@1` that catches patches that pass tests but are semantically
incorrect.

**Research basis:** SWE-ABS (March 2026,
[arxiv 2603.00520](https://arxiv.org/abs/2603.00520)) — 19.78% of
"solved" SWE-bench tasks are semantically incorrect under strengthened tests.

---

## Quick start

```bash
# Smoke test (5 instances, fully offline, no network required)
agent-tui eval --subset 5 --dry-run

# Real evaluation (50 instances, Anthropic Opus)
agent-tui eval \
  --bench swe-verified \
  --subset 50 \
  --provider anthropic \
  --model claude-opus-4-7

# With RTV test-time scaling (4 rollouts per instance)
agent-tui eval --subset 50 --scale 4

# Specific instances
agent-tui eval --subset-ids django__django-12345,flask__flask-789
```

---

## How it works

### Step 1: fetch instances

The harness loads SWE-bench Verified instances. In `--dry-run` mode, two
synthetic instances are used (`pass_instance` and `fail_instance`) so the
entire run completes offline in seconds.

For real runs, instances are fetched from the HuggingFace `princeton-nlp/SWE-bench_Verified`
dataset. This requires network access and `huggingface_hub` to be installed
(or the dataset pre-downloaded).

### Step 2: isolate the workspace

For each instance, the harness either:

- **Docker (preferred):** spins up the official SWE-bench Docker image for
  that instance and runs the agent inside it.
- **Git clone fallback:** clones the target repo at the issue's base commit
  into a temp directory. Used when Docker is unavailable.

The active isolation mode is logged at the start of each run.

### Step 3: run the agent

`agent-tui fix --yolo --headless` runs against the instance's issue text
within the isolated workspace. The harness captures the final unified diff,
total turns, total tokens, estimated cost (USD), and wall-clock time.

When `--scale N > 1` is given, N independent rollouts run in parallel.
Each rollout is summarised into a structured `<summary>` block. Recursive
Tournament Voting (RTV) selects the winner:

1. Group rollouts into batches of 3–4.
2. A judge model picks the best summary from each group.
3. Recurse until one winner remains.

The winner's patch is used for evaluation. **Note:** voting is over
structured summaries, not raw bash transcripts — raw transcripts are too
noisy for reliable comparison (Meta RTV paper,
[arxiv 2604.16529](https://arxiv.org/abs/2604.16529)).

### Step 4: apply and test

The patch is applied to the workspace. The instance's official test suite is
run to determine `pass@1`.

### Step 5: mutation strengthening

For each instance where `pass@1 = true`, the harness additionally:

1. Generates N mutants (default 5) of the test file using basic mutation
   operators:
   - Return-value flips (`return true` → `return false`)
   - Comparison-operator swaps (`>` → `>=`, `==` → `!=`)
   - Conditional negations (`if cond` → `if !cond`)
   - Off-by-one offsets (integer literals ± 1)

2. Runs the patched code against each mutant test. If any mutant test
   **passes** (meaning the original test didn't detect the mutation), the
   instance is marked `semantic_pass = false` — the patch is likely not a
   complete fix.

3. `semantic_pass@1` = fraction of instances that pass both the original
   test **and** a majority (≥ 3/5) of mutant tests.

The mutation engine is intentionally simple: it uses tree-sitter rewrites,
not a full mutation testing framework. This keeps it dependency-free and
fast, at the cost of missing some mutation classes.

---

## Output format

Each run writes a JSON file to `eval-runs/<timestamp>.json`:

```json
{
  "config": {
    "bench": "swe-verified",
    "provider": "anthropic",
    "model": "claude-opus-4-7",
    "scale": 1,
    "time_budget_secs": 600,
    "dry_run": false
  },
  "summary": {
    "pass_at_1": 0.41,
    "semantic_pass_at_1": 0.35,
    "mean_cost_usd": 1.18,
    "mean_turns": 14.2,
    "mean_wallclock_s": 287.0
  },
  "instances": [
    {
      "id": "django__django-12345",
      "pass": true,
      "semantic_pass": true,
      "cost_usd": 0.94,
      "turns": 11,
      "wallclock_s": 243.0,
      "patch": "diff --git a/django/... ..."
    }
  ]
}
```

---

## Comparing runs

```bash
agent-tui eval --compare eval-runs/run-a.json eval-runs/run-b.json
```

Produces a markdown table:

```
| Instance | pass A | pass B | sem A | sem B | cost A | cost B |
|---|---|---|---|---|---|---|
| django__12345 | ✓ | ✓ | ✓ | ✗ | $0.94 | $1.02 |
| flask__789    | ✗ | ✓ | ✗ | ✓ | $0.61 | $0.87 |
...
Summary: A→B flips: 3 gained, 1 lost. Cost delta: +$0.21/instance avg.
```

---

## Interpreting results

- **`pass@1`** — standard SWE-bench metric. A patch "passes" if the
  instance's official test suite passes after applying it.
- **`semantic_pass@1`** — more conservative. A patch only counts if it
  also survives mutation-strengthened tests. Expect this to be 10–20%
  lower than `pass@1`.
- **`mean_cost_usd`** — average dollar cost per instance. Strongly
  correlated with `--scale`.
- **`mean_turns`** — average number of model turns per instance. Higher
  turns usually indicate the agent is retrying after failures.
- **`mean_wallclock_s`** — average wall-clock time per instance. Dominated
  by the `--time-budget` cap and parallelism.

A healthy run shows `semantic_pass@1 / pass@1 ≥ 0.85`. Ratios below 0.80
suggest the agent is producing patches that happen to pass tests but miss
the underlying bug (over-fitting to the test oracle).

---

## Known limitations

- **Docker availability.** Without Docker, the git-clone fallback is used.
  The fallback does not provide a clean runtime environment, which can
  cause false failures on instances with unusual build requirements.
- **Eval harness is not run in CI.** CI only validates `cargo test
  --workspace`. Running `eval` in CI would require Docker-in-Docker and
  is too slow for every push.
- **SWE-bench dataset access.** Real runs require network access to
  HuggingFace. Use `--dry-run` for offline testing.
- **Mutation coverage.** The mutation engine covers common operators but
  not all possible mutations (e.g. string mutations, API call mutations).
  This means `semantic_pass@1` is a lower bound on true semantic correctness,
  not an upper bound.
- **RTV cost.** `--scale N` multiplies cost roughly by N. Use only in
  headless batch mode; the CLI guards against `--scale N > 1` when
  `AGENT_TUI_INTERACTIVE` is set.
