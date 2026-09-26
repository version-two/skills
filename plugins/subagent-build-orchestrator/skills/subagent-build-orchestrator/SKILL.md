---
name: subagent-build-orchestrator
description: "Plan and execute a large, multi-month software build as a file-based plan driven by parallel model-tiered subagents. Use when the task is to PLAN a big project (read a spec, turn it into a granular implementation plan), set up multi-stream orchestration, delegate implementation to Haiku/Sonnet/Opus subagents, or run an ongoing build where the parent orchestrates and subagents implement. Triggers: prepare/write/rewrite a file-based plan (plan.md), break a PROJECT.md/spec into trackable items, parallel subagent streams, model tiering (haiku=mechanical, sonnet=implementation, opus=planning/review), a binding concurrency contract that fixes the number of parallel subagents per phase and is re-read after every context compaction so the build never silently drops to a single agent, per-stream review gates, performance/compatibility checks mid-implementation, QA + review agents, THE PLAN IS SACRED, no halfassing, commit periodically, parent orchestrates subagents implement. Stack-agnostic: the tech stack is read from PROJECT.md or elicited via a question round, never assumed. Applies to any project too large to hold in one context."
---

# Subagent Build Orchestrator

A methodology for planning and executing a software build that is too large for one
context window and one linear pass: you turn a product spec into a **granular,
file-based plan**, then drive it with **parallel, model-tiered subagents** where the
parent **orchestrates** and subagents **implement**.

**This skill is stack-agnostic.** Do NOT assume any framework, language, database, or tool.
The tech stack is either already fixed by `PROJECT.md` / the existing codebase, or it is
decided with the user through a structured question round (see A2). Everything below uses
illustrative examples in `(e.g. ...)` form only; substitute the project's actual stack.

This skill encodes the working agreement the user established for a large build. The user's
own words define the spirit (verbatim):

> "read the whole PROJECT.md and prepare a file based plan, that will take 5 subagents
> and will implement backend and web ... and then, when backend is done will implement
> the [app] ... also with 5 subagents. **no halfassing things, no todos, no
> shortcuts, Full implementation with test coverage.** Store information about this and
> about the subagents etc in the plan and in claude.md too so that it is not forgotten.
> **Utilize haiku subagents for smaller tasks, sonnet subagents for implementation tasks
> and opus for planning, thinking etc tasks. Never assume things, always check / research
> / verify things, assumptions lead to costly mistakes, verification takes seconds and is
> cheap. commit periodically.**"

> (after a weak first plan) "i have a bad feeling your plan is halfassed.. it lacks a lot
> of things from the project.md, is not taking into account the 5 subagents and not
> utilizing them correctly, it's not dedicating the agent types"

> "**enter proper planning mode and PROPERLY and FULLY plan the project. real granularity
> with even planning for compatibility and performance checking mid implementation after
> tasks with proper qa and review agents** etc"

> (mid-build correction) "STOP hand-coding the stream work yourself. Dispatch the
> tagged-tier subagent. The parent only briefs, verifies, commits, and updates the plan.
> Put this rule in CLAUDE.md so it persists. **THE PLAN IS SACRED.**"

> "why are you not using 5 parallel subagents" / "why did you stop???"

Treat these as the law. The rest of this document operationalizes them.

## When to use

Use this when ANY of these are true:
- The user asks you to read a spec (`PROJECT.md`, a brief, a doc tree) and **prepare/rewrite
  a file-based plan** for a large build.
- The work spans **multiple subsystems / streams** and many weeks – too big for one context.
- The user wants **parallel subagents**, **model tiering**, or an **orchestrated** build.
- You are resuming an in-flight build that already has a `plan.md` + tiered items.

Do NOT spin up this whole apparatus for a small feature. For a one-file fix, just do it.

## The phased shape

Most large builds decompose into sequential phases, each with several parallel streams
(the user's reference point was ~5 per phase; pick the count from the actual decomposition):
- **Phase 1** – the foundation + the first deliverable surface (e.g. a backend + an admin web).
  One **blocking foundations** stream must go green before the others fan out.
- **Phase 2** – the dependent surface (e.g. a mobile or desktop client), started only after
  Phase 1's exit criteria are met and the integration contract (e.g. the API schema) is **frozen**.

Within a phase: a blocking foundation block first (sequential, because items depend on each
other), then the remaining streams in parallel. The number of phases and streams is derived
from the project, not fixed by this skill.

---

## PART A – Planning PROPERLY (do this before any code)

A halfassed plan is the #1 failure mode. The user will reject it. A proper plan has:

### A1. Investigate first (never assume)
Before asking anything or writing anything, do the homework:
- Read the **entire** spec (`PROJECT.md` and any `docs/` tree), not the first screen.
- Inspect the **actual repo**: what already exists, what stack (if any) is already chosen,
  build files, lockfiles, existing conventions, an existing `CLAUDE.md`. Treat the codebase
  as ground truth that overrides guesses.
- Write down two lists: **what is already decided** (locked by spec or existing code) and
  **what is open** (ambiguous, missing, or contradictory). The open list feeds A2.

### A2. Interrogate the user to lock the stack + decisions
**The tech stack and all product ambiguities are decided here, never assumed.** If `PROJECT.md`
already pins a choice, respect it; otherwise ask.
- Run a structured question round using the **AskUserQuestion** tool. Scale to the project:
  roughly **10–100 questions**, batched in rounds (the tool takes up to 4 at a time; keep
  going round after round until the open list is empty). Ask "do you want more questions?"
  and keep asking as long as the user says yes.
- Cover at least: **tech stack** for every layer (language, framework, datastore, realtime,
  auth, storage, payments, CI, hosting, mobile/desktop toolchain, testing + static-analysis
  tools) when not already fixed; product behavior and edge cases; non-functional targets
  (scale, latency, offline, security/compliance); every unspecified **default** (timeouts,
  caps, thresholds, retention); deployment/distribution model; and house-style/conventions.
- Recommend a default for each question (mark it "(Recommended)") and explain the trade-off,
  but let the user decide. If the user defers ("what do you recommend?"), pick the strongest
  option, state why, and record it as a decision.
- Every answer becomes a locked entry in `decisions.md` (A4). Nothing the user told you may be
  silently dropped or contradicted later.

### A3. Inventory the WHOLE spec, lose nothing
- Inventory **every** feature in the spec. Give each a **stable ID** (`FEAT-001`…). Later you
  grep the plan for every ID to prove nothing was dropped: a spec feature with no `FEAT-NNN`
  in the plan means the plan is incomplete – fix it.
- Use Opus (or parallel research agents) for the inventory + a gap audit + a tech-choice
  reference. Persist their raw output under `briefs/` so it is not lost to compaction.

### A4. Lock the decisions (kill ambiguity)
- Every "X or Y?" – from the spec or from the A2 question round – becomes a dated, numbered
  decision in `decisions.md` (`D-01`…) with **rationale + alternatives considered**. Decisions
  supersede ambiguity in the spec. They are **append-only**; to overturn one, append a new
  dated entry.
- Fill in every spec-unspecified default with an explicit locked value. No "TBD".

### A5. Granularity + per-item contract
Every checkbox item carries everything a subagent needs to execute it blind. Use the project's
real paths/tools; this is only the shape:

```
- [ ] [FEAT-NNN] <short description> [tier:haiku] [owner:S1] [brief:briefs/phase-1-s1.md#item-NNN]
  Files: <exact files this item creates/edits>
  Tests: <named test cases that prove the behavior>
  DoD: <objective done criteria – migrations reversible if any, all branches/states covered,
        coverage target, CI green> + commit `<type>(<scope>): ... [FEAT-NNN]`
```

The `[tier:...]` tag is an **instruction**, not a hint (see Part B). **The default tag is
`[tier:haiku]`.** A sonnet tag must carry its reason inline – `[tier:sonnet why:wire-protocol
design]` – and "this looks hard" is not one. An item that cannot be executed by haiku from its
brief is usually an item that was not decomposed far enough; split it before promoting it.

### A6. The artifact set (write all of them)
Don't put everything in `plan.md`. Spread durable structure across files so each is small,
authoritative, and survives context compaction:

| File | Holds |
|---|---|
| `plan.md` | The tracker: the contract, the orchestration model, the phase registry. Append-only (see A8). Holds the items inline while the plan is small. |
| `decisions.md` | Locked decisions `D-NN` + rationale + alternatives (incl. every A2 answer). |
| `ownership.md` | File-ownership matrix per stream + the shared-file claim protocol. |
| `perf-budgets.md` | Hard perf numbers + bench definitions per feature (and mid-build benches). |
| `risk-register.md` | Known risks `R-NN` + owner + mitigation. |
| `compatibility-matrix.md` | Pinned versions + device/test matrix. Bumps need a decision. |
| `briefs/` | One subagent brief per stream-phase + the research outputs (feature inventory, audit, tech reference). Also where the per-phase item files (`plan-phase-<N>.md`) go once the item list outgrows `plan.md` – see A6.2. |
| `qa/` | Smoke tests, the UI/UX audit checklist, the security-review checklist. |
| `.claude/agents/` | The skill's agent templates (B3.1), instantiated with this project's real commands and repo paths. |
| `CLAUDE.md` | The conventions + orchestration rules + the locked stack, so they persist every session. |

**Document the orchestration model itself (mandatory – "store it in the plan and in claude.md
so it is not forgotten").** `plan.md` must OPEN with an orchestration section, and `CLAUDE.md`
must mirror the durable rules, so a fresh context (or the next session after compaction) can run
the build without re-deriving any of it. That written model must state, concretely for THIS
project:

- **The Concurrency Contract (mandatory, see A6.1)** – a fixed, machine-readable table at the very
  top of `plan.md` stating the exact number of subagents to run in parallel for every phase, the
  named streams, and the foundation-block exception. This is the single line of defense against
  the post-compaction "drift to one agent" failure. It is binding at all times, not a suggestion.
- **How many phases and how many parallel streams/subagents per phase**, and the **name + scope
  of each stream** – exactly what subsystem and which files/areas each stream owns (cross-link to
  `ownership.md`). E.g. "Phase 1 = 5 streams: S1 Foundations (auth/data/RBAC, blocking), S2 …".
- **Where + what + how to dispatch:** the blocking order (which foundation block must be green
  before fan-out, plus any cross-stream sequencing), and the dispatch ceremony per wave – fork vs
  fresh subagent, worktree isolation + per-stream DB/sandbox, and how many subagents run
  concurrently.
- **How to dedicate the tiers (haiku / sonnet / opus):** write the tier semantics with concrete
  per-project examples (haiku = which mechanical tasks here; sonnet = which implementation work;
  opus = which planning/review work), the law that every item's `[tier:...]` tag is binding, and
  that `model:` is **always set explicitly** on every Agent call (omitting it silently inherits
  the parent tier and breaks the cost model).
- **The parent-orchestrates / subagents-implement law** and the **verification cadence + review
  gates** (so no one hand-codes tagged stream work, and nothing is marked `[x]` unverified).

Keep this section in sync as the model evolves (append-only notes); it is the single place a new
session reads to know how many agents to run, where, doing what, and at which tier.

### A6.1. The Concurrency Contract (must survive compaction)
**The #1 runtime failure is silent drift to a single subagent after a context compaction.** A new
context window does not remember "we were running 5 in parallel"; it sees a half-done `plan.md`,
picks the next unchecked item, and dispatches **one** agent. To stop this, the plan must encode
concurrency as hard data the orchestrator re-reads and obeys every session, not as prose buried
mid-file. Make it the **first thing** in `plan.md` (and mirror it verbatim in `CLAUDE.md`):

```
## CONCURRENCY CONTRACT (binding – re-read at the start of every session and after every compaction)

Current phase: <P1>                 ← append a NEW marker line when exit criteria are met; last one wins
                                      (several may be active at once: `Current phase: P1, P2`)
Active concurrency (this phase): N   ← you MUST keep N subagents dispatched at all times until the phase's open items run out

| Phase | Parallel subagents (N) | Streams (owner → scope)                                   | Foundation block (sequential, runs first) |
|-------|------------------------|-----------------------------------------------------------|--------------------------------------------|
| P1    | 5                      | S1 Foundations, S2 …, S3 …, S4 …, S5 …                     | S1 items F-001..F-0xx (1 agent, in order)  |
| P2    | 5                      | M1 …, M2 …, M3 …, M4 …, M5 …                               | M1 contract-binding items (1 agent)        |

Rules:
- While a phase is active and has ≥ N unblocked open items, exactly N subagents must be in flight.
  Fewer than N in flight = a bug. Refill immediately: when one stream's agent returns and is
  verified, dispatch the next item for that stream (or the next unblocked item) so the count
  returns to N. Never let the fleet idle down to 1 because "the next item was convenient to do."
- The ONLY time fewer than N run is: (a) the foundation block, which is intentionally sequential
  (1 agent) until green; (b) fewer than N unblocked open items remain in the phase (then run as
  many as there are); (c) the user explicitly tells you to reduce concurrency.
- N is a property of the PHASE, not of how much context you have left. Compaction does not lower N.
```

Fill the table from the real decomposition (the stream count drives N; the user's reference was
~5). When a phase completes, append a note flipping the `Current phase` marker – never rewrite
history. The dispatch ceremony (A6 / B4) reads `Active concurrency (this phase): N` and fans out
to exactly that many. This table is the authority a fresh post-compaction context obeys; if it is
missing or vague, the plan is incomplete – fix it before executing.

### A6.2. Keep `plan.md` statusline-readable
The user's Claude Code statusline parses `plan.md` and shows `P<phase> - <index>/<total phases>`,
a bar for the active phase's completion, and an overall percentage. That readout is only as
honest as the plan's formatting, so these five conventions are **mandatory** – they cost nothing
and they are what makes progress visible at a glance:

1. **The phase marker starts its own line:** `Current phase: P0`, with nothing before it but
   whitespace. Because the file is append-only (A8), a phase flip is a *new* marker line appended
   inside the dated note – never an edit to the original – and **the last marker line in the file
   wins**, so the history above it stays intact while the readout still tracks reality. Prose that
   merely mentions the marker mid-sentence is ignored, so the flip note must break the marker onto
   its own line:

   ```
   <!-- NOTE: 2026-09-01 - P0 exit criteria met, contracts frozen.
   Current phase: P1
   -->
   ```
2. **More than one phase may be active at once.** Put them all on that one line, separated
   however reads best: `Current phase: P1, P2` or `Current phase: P1 + PT`. Every phase ID on the
   line is active; their items pool into a single active-phase percentage labelled `P1+P2`, and
   the position shows the span (`2-3/13`). The phases need not be adjacent. This is the normal
   shape when a finished phase leaves a long tail running beside the next one – write it honestly
   rather than pretending a single phase is active.
3. **The contract table is the phase registry.** Every phase gets exactly one row whose first
   cell starts with its ID (`| P0 contracts | 1 | ... |`, `| PT tests | 5 | ... |`). The number of
   such rows *is* the number of phases – no phase may exist that has no row. An active phase with
   no row still shows, but without its position in the sequence.
4. **Items live in one of two places, and both are read.** Either inline in `plan.md` under `##`
   headings carrying the phase ID (`## Phase 0 - contracts`, `## Phase T - tests`; streams inside a
   phase are `###` or deeper so they do not close it), or – the shape a large plan ends up in –
   one file per phase named `plan-phase-<N>.md` or `phase-<N>.md` (`<N>` a number or `T`), sitting
   in the plan directory or in `briefs/`, `plan/`, `phases/`. Every item in such a file belongs to
   that phase, whatever its internal headings. An inline item under a `##` heading with no phase ID
   counts toward the overall total but toward no phase.
5. **Example items stay in fenced blocks; real items never live in HTML comments.** Any `- [ ]`
   outside a ``` fence and outside a `<!-- ... -->` block counts as real work. The "How to read an
   item" sample stays fenced; anything parked inside a `<!-- NOTE: ... -->` is deliberately
   uncounted, so when parked work becomes real, append it as a normal item.

Item lines are counted by their checkbox alone (`- [ ]` open, `- [x]` done), at any indent, so
the `[tier:]` / `[owner:]` / `Files:` / `Tests:` / `DoD:` shape from A5 is unaffected. Toggling a
box (A8) is what moves the bars – another reason never to mark an item `[x]` unverified.

### A7. Plan for verification mid-build, not just at the end
This is the part the user specifically demanded. Bake into the plan:
- **Mid-stream perf benches** when a stream hits a perf-relevant milestone (e.g. a fan-out
  path, a hot query, a heavy compute loop) – a dedicated bench agent writes results to
  `perf-budgets.md`.
- **Compatibility checks** at version-pin boundaries.
- **Per-stream review gates** (see B5): code-review → security-review (if auth/billing/PII) →
  UI/UX audit (UI streams) → Opus consolidation review → tag.

### A8. plan.md mutation rules (sacred)
- **Append-only.** Never delete content. The only allowed mutations are: toggle `[ ]`→`[x]`
  **after** an item is fully done+verified, and add `<!-- NOTE: ... -->` inline notes.
- New work discovered mid-build is **appended**, never inserted or reordered.
- Read `plan.md` at the start of every session to know what's done and what's next.
- Toggling a box is also what drives the statusline readout (A6.2); keep the phase marker
  line and the phase headings in the shapes it parses.

### A9. Use plan mode + present before executing
Do the heavy planning in plan mode. Present the structure for approval before writing code.
If the user calls it halfassed, do not patch – **re-enter proper planning mode and rebuild it
fully** with real granularity. Self-check before claiming done (A3's FEAT grep, the audit's
top-fixes, decision coverage, brief coverage).

---

## PART B – Executing: parent ORCHESTRATES, subagents IMPLEMENT

This is the non-negotiable law of the build. Read it before every work session.

### B0. Session-start / post-compaction protocol (run this FIRST, every time)
A fresh context window – including the one handed to you after a compaction – must **re-establish
the fleet before touching any item.** Do this before anything else:
1. Read `plan.md`'s **Concurrency Contract** (A6.1): note `Current phase` and `Active concurrency
   (this phase): N`.
2. Determine how many subagents are actually in flight right now (a fresh context = **zero**).
3. If the active phase is past its foundation block and has ≥ N unblocked open items, you are
   **obligated to dispatch up to N parallel subagents immediately** – not one. Resuming a
   parallel build with a single agent is a contract violation, the exact bug this skill exists to
   prevent. The correct resume action is "fan out to N", never "quietly continue with one".
4. Only then pick items and brief them, one per stream, until N are running.

Compaction never lowers N. "I just woke up with little context so I'll do one safe item" is the
forbidden behavior. Re-read the contract, refill the fleet to N, then proceed.

### B1. Do NOT hand-code stream work
Every `plan.md` item has `[tier:haiku|sonnet|opus]`. When an item is `[tier:sonnet]` you
**dispatch a Sonnet subagent**; `[tier:haiku]`→Haiku; `[tier:opus]`→Opus. The parent (you)
writes briefs, verifies output, runs gates, updates `plan.md`, commits/merges. The parent does
**not** sit and write the implementation files itself.

**The only work the parent may do directly:** (a) reading docs/code to write a brief,
(b) verifying a subagent's diff + re-running gates, (c) updating `plan.md` checkmarks/notes,
(d) committing/merging integration, (e) genuinely cross-cutting design decisions that need the
user – and even then, hand the implementation to the tagged tier. Mechanical build steps
(codegen, formatters, regen-from-contract) are integration, not hand-coding – the parent may
run them.

If you catch yourself opening Edit/Write on a tagged item's implementation/test files:
**STOP.** That belongs to a subagent of the tagged tier. Dispatch it.

### B2. Model tiering – haiku first, always (set `model:` explicitly)

**The ladder is haiku → sonnet → opus and every dispatch starts at the bottom.** Sonnet is not the
default implementation tier; it is an escalation that has to be earned by a haiku attempt that
actually failed. Opening at sonnet because an item "looks hard" is a guess, and it is the single
most expensive habit available in this methodology.

- **Haiku – the default for stream work.** Not merely scaffolds and renames: most items in a
  properly written plan are *execution*, not judgment. The brief names the files, the conventions
  are already in the repo, the acceptance criteria are given, and the item was decomposed by the
  orchestrator precisely so it could be executed without re-deriving the design. That is haiku
  work. Scaffolds, migrations from a given schema, fixtures, seed data, config, renames, string
  sweeps, codegen runs and dependency bumps are the floor of what haiku does, not the ceiling.
  Haiku does **not** spawn further subagents.
- **Sonnet – an escalation, not a starting point.** Reached when a verified haiku attempt came
  back wrong, incomplete, or ignoring the brief. Tagging an item sonnet up front is allowed only
  with a named reason on the item (see A5): genuinely open design inside the item, a protocol or
  concurrency shape that has to be reasoned about rather than transcribed, or a review gate.
- **Opus – the orchestrator itself.** Planning, decomposition, cross-stream contract design,
  consolidation review. Never an item owner.

**"Fails to deliver" means verified-and-wrong, not looks-risky.** Verify before escalating, and
prefer a re-brief at the same tier first: a bad result is more often an underwritten brief than an
underpowered model (B7.2). When you do escalate, say in the new brief exactly what the previous
tier got wrong so the next one does not repeat it. Never skip a rung.

**Always pass `model:` on every Agent call.** Omitting it silently inherits the parent tier – an
opus-priced subagent doing haiku work – and is the most common way this rule fails in practice.

### B3. Fork vs fresh subagent
- **Fork** (Agent without `subagent_type`) – inherits parent context, shares prompt cache. Use
  for research, audits, cross-file lookups where the parent's context makes the work tractable.
- **Fresh** (a purpose-built `subagent_type`, see B3.1 – `general-purpose` only when nothing fits)
  – zero context, must be briefed from scratch. Use for isolated implementation. The brief MUST
  contain: scope, exact files to touch, files to leave alone, conventions to follow (incl. the
  locked stack), the exact tests to add, the commit-message format, and "report back which
  `plan.md` items you completed."
- Max spawn depth = 2 (parent → subagent → one further tier).

### B3.1. Purpose-built developer agents, not `general-purpose`

`general-purpose` is a catch-all: the full tool surface, no domain priming, and a default reporting
style that narrates. The parent pays for that narration twice – once to generate it, once when it
lands in the parent's window and rides along in every subsequent request for the rest of the
session. At the measured 68%-of-usage share (B7), the reporting style of the default agent is a
real line item.

**The agents ship with this skill.** They live in `agents/` beside this file and are not something
to reinvent per project:

| Template | Model | Tools | Role |
|---|---|---|---|
| `rust-dev.md` | haiku | +Edit/Write/Bash | Rust stream items |
| `laravel-dev.md` | haiku | +Edit/Write/Bash | Laravel / Blade / Alpine / Filament items |
| `flutter-dev.md` | haiku | +Edit/Write/Bash | Flutter app items |
| `gate-runner.md` | haiku | read-only + Bash | the B5 gate cadence |
| `code-reviewer.md` | sonnet | read-only | the per-stream review gate |
| `_shared-rules.md` | – | – | the rules + report contract every dev template embeds |

Each pins three things the catch-all leaves open:

- **`model:` in the frontmatter**, so the tier is structural rather than a flag someone forgets.
  Developer agents are `haiku`; only the reviewer is `sonnet` (B2).
- **`tools:` narrowed to the role.** A reviewer that cannot write cannot "helpfully" fix things
  the parent did not ask it to fix, and a gate runner needs no editor at all.
- **An explicit output contract.** This is the point of the exercise. The subagent's final text is
  a report to the orchestrator, not a message to a human: `FILES:` / `GATES:` / `ITEMS:` /
  `TESTS:` / `BLOCKED:`, nothing else. No preamble, no restating the brief, no narrating the
  approach, no echoing diffs the parent will read from git anyway.

**Instantiating them (part of A6, done once per project).** Copy the templates matching the
project's stack into the project's `.claude/agents/`, then replace every `<PLACEHOLDER>` with the
project's real values – the build invocation, the repository list, the gate commands – and delete
the `<!-- INSTANTIATE -->` line. A haiku agent holding the exact command does not go hunting for
it; a placeholder left in place is a brief that underspecifies (B7.2). Add repo-specific rules to
the instantiated copy, never to the template.

A stack with no template gets a new one written into `agents/` in the same shape, so the next
project inherits it. If a project genuinely needs a role no template covers, `general-purpose` is
the fallback, not the default.

**Fleet routing, when a project has more than one.** Some projects run a second, non-Claude agent
fleet with different capabilities (no access to a toolchain, unable to commit inside a worktree,
and so on). Where that is true, routing is by repository and is recorded in `plan.md`, not decided
per item by how hard the work looks. These templates describe the Claude fleet's agents; a
repository routed to another fleet still gets its rules written down somewhere both fleets read.

### B4. Dispatch ceremony + parallelism
1. **Pre-flight Opus pass** (optional): one Opus Fork reviews the schema/design across a stream's
   items and appends `<!-- NOTE: ... -->` findings. No code yet.
2. **Foundations block**: sequential subagent calls (items depend on each other). After each:
   parent runs the full gate suite, reads the diff, spot-checks, marks `[x]`, commits.
3. **After foundations green**: fan out to the Concurrency Contract's `N` (A6.1) – dispatch N
   parallel subagents, one per stream. Don't run one agent at a time when the work is independent
   – the user will ask "why are you not using 5 parallel subagents" and "why did you stop???".
   Keep momentum; don't pause after each wave. **Maintain N continuously:** the moment a stream's
   agent returns and you've verified it, dispatch that stream's next item so in-flight count climbs
   back to N. Treat "fewer than N running while open items remain" as a bug to fix on the spot,
   including right after a compaction (B0).
4. **Isolation for parallel writers**: give each parallel stream its own git **worktree** + branch
   + its own test DB/sandbox, so concurrent file writes don't collide. Integrate branches
   sequentially. (Watch worktree/junction hazards – unlink the stack's dependency dirs
   (e.g. `vendor/`, `node_modules/`) before removing a worktree, or you delete through them.)
5. **Mid-stream benches** at perf milestones (A7).

### B5. Verification cadence (a subagent's "done" describes intent, not result)
- **Per task:** parent reads the diff, runs the affected tests, marks `[x]`. One task = one commit.
- **Per batch (~5 tasks):** full test suite + the project's static analyzer at its strictest
  level + formatter; no coverage regression.
- **Per stream:** the review-gate chain – code-review → security-review (auth/billing/PII/RBAC) →
  UI/UX audit (UI streams) → Opus consolidation review → tag on all-green.
- **Always:** verify before marking `[x]`. Never mark done on a subagent's word alone. When a
  subagent reports done, also clean up its mess (e.g. spurious codegen churn it left in the tree).

### B6. Branch / commit protocol
- Trunk-based per repo; each stream commits to `main` after green CI (bottleneck is one human +
  an agent fleet, not many humans).
- **Shared files** (e.g. route files, a core model, a service provider, the router, shared
  theme/i18n) require an `ownership.md` claim marker before edit; parent enforces no concurrent
  claim and removes it on commit.
- Commit format `<type>(<scope>): <description> [FEAT-NNN]`. Commit **before and after** every
  change set. Never `--no-verify`, never `--no-gpg-sign`, never skip hooks – fix the cause.

### B7. Cost discipline (the fleet is the bill)

A build run this way spends almost all of its tokens inside subagents. A measured profile of
real sessions: **88% of usage from subagent-heavy sessions, 68% of that from `general-purpose`
subagents, 78% of it at over 150k parent context.** That is the shape of this methodology working
as designed – but it makes every sloppy dispatch expensive, and the waste is not where people look
for it. `N` is **not** the lever: it is fixed by the Concurrency Contract, and holding it is what
makes the build finish at all (C9). The levers are what each agent loads and how often it has to
go looking.

1. **Start at haiku, every time (B2).** Every item tagged sonnet that a haiku could have executed
   is a multiple of its cost paid for nothing, and the tag is usually a guess made before anyone
   tried. Default to `[tier:haiku]`, make a sonnet tag justify itself in writing, and escalate only
   on a verified failure. Sweeping a stream's open items back down to haiku is usually the single
   largest saving available at any moment in the build.
2. **A vague brief is the expensive one.** A fresh `general-purpose` subagent starts blind:
   whatever the brief omits, it discovers by re-reading the spec, grepping the repo and opening
   files – and that exploration costs far more than the two hundred words that would have
   prevented it. Every brief carries the exact files to touch, the files to leave alone, the
   conventions verbatim, the acceptance criteria and the commit line. If a subagent had to go
   hunting, the brief was underwritten: fix the brief before the next wave rather than paying the
   same discovery cost five more times.
3. **Batch mechanical items into one dispatch.** Five haiku agents that each load a brief to add
   one migration pay five spawn-and-load cycles for work one agent does in one pass. Group
   same-repo, same-shape items into a single item body; keep them split only when they touch files
   that would collide.
4. **Keep the parent light.** The parent's context is re-sent on every call for the whole session,
   which is what puts 78% of usage above 150k. The parent reads diffs, not implementations; it
   sends research to a Fork so the raw output lands in the fork's window instead of its own (B3);
   it compacts at phase boundaries. The file-based plan exists precisely so the parent can drop
   context and reload from `plan.md` instead of carrying everything.
5. **Don't pay twice for verification.** Gates run at the B5 cadence – per task, per batch, per
   stream – not after every file. A subagent told to re-run the full suite in a loop can outspend
   the implementation it was checking.

None of this licenses dropping below `N`. The target is fewer, cheaper, better-briefed agents at
the contracted width – never a narrower fleet.

---

## PART C – The non-negotiable rules (carry through everything)

1. **No halfassing, no shortcuts, no "mostly fine".** Full implementation with full test
   coverage. If the right answer is more work, do the work. The only acceptable reason to defer a
   step is a hard external blocker (API doesn't exist, server not provisioned, user said no) –
   never effort.
2. **No TODOs in committed code.** If a step can't be done as written, **STOP and ask** – never
   silently improvise a different approach.
3. **Verify, never assume.** This applies to the tech stack too: read the spec, inspect the repo,
   ask the user (A1–A2). Grep the code and run a probe before relying on anything. Assumptions
   cause costly mistakes; verification takes seconds.
4. **Tests are mandatory** and pre-existing failures must be fixed too – green at every gate.
   "It's pre-existing" is not an excuse. (Exception: a failure caused by the user's own intentional
   local dev modification – surface it, don't paper over it.)
5. **Commit periodically.** Each sub-task = one commit. Push at batch/stream boundaries.
6. **THE PLAN IS SACRED.** Execute items as written. If a step is impossible or a better approach
   appears: STOP, surface it ("plan says X, I hit Y, I propose Z – approve?"), and do NOT act until
   the user approves. Silent deviation is a trust-breaking, fireable offense. Never mark a task
   complete with a subject that doesn't match what the plan asked for.
7. **Run the project's UI/UX audit before any UI work** (the `ui-ux-pro-max` skill if available),
   and re-invoke it per UI task.
8. **Carry the house style into every brief.** Pull the project's conventions from its `CLAUDE.md`
   and the user's global config – coding standards, naming, ID strategy, i18n/typography rules,
   framework idioms, commit conventions – and pass them to every subagent. The user's cross-project
   rules always apply: never add Claude Code branding to commits; for Slovak text use full
   diacritics (á é í ó ú ý ä ô č ď ľ ĺ ň ŕ š ť ž) and en dashes with spaces, never em dashes.
9. **Hold the concurrency at N, always.** The Concurrency Contract (A6.1) sets `N` parallel
   subagents per phase. While a phase is active and unblocked open items remain, exactly `N` must
   be in flight – continuously, including immediately after a context compaction (B0). Dropping to
   one agent because the context is fresh or "it was the easy next step" is the signature failure
   this skill exists to prevent. Fewer than `N` is allowed only during the sequential foundation
   block, when fewer than `N` unblocked items remain, or when the user says so. Re-read the
   contract every session, refill the fleet to `N`, then work.
10. **Cost is a design input, not an afterthought.** Almost every token this methodology spends is
   spent inside a subagent, so the tier tag, the quality of the brief and the parent's context size
   are financial decisions as much as technical ones (B7). Tier down by default, brief precisely
   enough that no agent has to go hunting, and keep the parent reading diffs rather than
   implementations – at the contracted `N`, never below it.

---

## Quick reference – order of operations for a fresh build

1. **Investigate** the spec + the existing repo; list what's decided vs open (A1).
2. **Interrogate** the user with 10–100 questions (AskUserQuestion, batched rounds) to lock the
   tech stack and every product decision/default – unless already pinned by PROJECT.md (A2).
3. **Inventory** every feature with `FEAT-NNN` (research agents; persist to `briefs/`) (A3).
4. **Lock** every ambiguity + default into `decisions.md` (`D-NN`, append-only) (A4).
5. Write the artifact set (A6), put the **Concurrency Contract** (A6.1) at the top of `plan.md`,
   and bake the rules + locked stack + the contract into `CLAUDE.md`.
   Keep the phase marker, the contract table and the `##` phase headings in the statusline-readable
   shapes (A6.2).
6. Make every item granular + tiered + DoD'd (A5). Self-check coverage (FEAT grep, audit, decisions, briefs).
7. Present in plan mode; if rejected as halfassed, rebuild fully (don't patch) (A9).
8. Execute: at session start re-read the Concurrency Contract and refill the fleet to `N` (B0) →
   foundations block (sequential) → fan out to `N` parallel streams (worktrees), held at `N`
   continuously → mid benches → per-stream review gates → exit criteria → freeze contract → next phase.
9. Parent orchestrates, subagents implement at `N`-wide concurrency, set `model:` explicitly,
   verify every diff, commit periodically, plan stays sacred. After any compaction, resume at `N`,
   never at 1.
