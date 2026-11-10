# SANDAL

SANDAL carries ideas that worked in one place into a new problem, proposes trials for them, and re-reads its own conclusions when new evidence arrives.

It is a tool for harnesses, not a harness: **no model API, no API key**. Whatever drives it (Claude Code, Codex, Cursor, a script, you) is the model. SANDAL does the bookkeeping a model is bad at: every derived thing records what it was derived from, results are cached, and new evidence marks stale only the readings that depended on it.

v0 is built to show two things, either of which could fail:

1. **Transfer.** On historical transfers with the outcome hidden, the pipeline picks the right source and reconstructs the transfer more often than the same model asked once.
2. **Incremental re-reading.** After new evidence arrives, refreshing gives the same readings as recomputing everything, using fewer model calls.

## How a harness uses it

When a step needs a model, SANDAL writes the prompt down and exits with code 3. The harness answers and runs the same command again:

```bash
sandal --json cycle kitchen        # exit 3: {"status":"pending","pending":[{"key":"9f2c…","prompt":"…"}]}
sandal answer 9f2c < reply.json    # the harness's reply, JSON only
sandal --json cycle kitchen        # picks up where it left off; repeat until exit 0
```

`--json` works on every command. [AGENTS.md](AGENTS.md) has the full protocol, exit codes and a typical session. If a hand-off per step is too slow, `provider = "command"` makes SANDAL run a program of yours for each model step (prompt on stdin, reply on stdout).

## Build and test

```bash
cargo test                          # every test runs against fakes; nothing touches a network
cargo clippy --all-targets -- -D warnings
cargo run --example try_and_read    # the reference for writing a new operation
```

The compile-fail tests in `tests/ui/` pin the compiler's wording of two errors. If a newer compiler words them differently, refresh them with `TRYBUILD=overwrite cargo test --test compile`.

## Using the CLI

```bash
sandal init                                  # creates .sandal/ with a default config
sandal add source --title "..." --body "..." --domain "..." --outcome "..."
sandal map kitchen --problem-file problem.txt
sandal cycle kitchen --k 8 --max-trials 5    # notice, back, match, carry, roots, mix
sandal try <proposal-id>                     # runs the proposal's trial through the verifier
sandal read <thing-id> --map kitchen         # what is this useful for, given the evidence so far
sandal stale                                 # readings whose evidence has moved
sandal forward <thing-id>                    # what a thing led to, as a tree
sandal pending                               # model prompts waiting for a reply
sandal answer <key>                          # reply to one (stdin, or --file)
sandal establish <result-id>                 # only a verifier's Confirmed result can do this
```

Ids may be given as any unique prefix of at least 8 hex characters. Trials are command templates in `.sandal/config.toml`; see the default file `sandal init` writes.

### Judging it

```bash
sandal bench bench/fixtures.jsonl --n 3      # SANDAL arm vs the model asked once; writes bench-report.json
sandal bench-incremental bench/fixtures.jsonl
```

`bench/fixtures.jsonl` holds three famous transfers (annealing, kingfisher to bullet train, burrs to Velcro). They exist to exercise the format. A model will recall them, so they prove nothing. The real set is 20 to 40 hand-written cases with obscure or recent transfers.

With the handoff provider the bench is answered by the harness too, including the judge. For a fair comparison the judge should be a different, stronger model than the one under test, so point `[judge]` at a `command` that runs it.

## How it fits together

| Part | Where |
| --- | --- |
| Ids, things, payloads | `src/id.rs`, `src/thing.rs` |
| Append-only ledger, slots and versions | `src/ledger.rs` |
| Memo entries and validity | `src/memo.rs` |
| Engine, budget, own-append rule | `src/engine.rs`, `src/ctx.rs` |
| Effect markers (checked at compile time) | `src/fx.rs` |
| `Op` trait, `then` / `each` / `iterate` | `src/op.rs` |
| `Verified<T>` (only `verify::confirm` makes one) | `src/verify.rs` |
| The ten operations and `Cycle` | `src/ops/` |
| Prompts, one per model operation | `src/prompts/` |
| Model, Source and Verifier plug-ins, with fakes | `src/plug/` (models: `handoff`, `command_model`) |
| Config, bench, CLI | `src/config.rs`, `src/bench.rs`, `src/bin/sandal.rs` |

State lives in `.sandal/`: `ledger.jsonl` and `memo.jsonl` (both append-only), `config.toml`, and `handoff/` for prompts and replies.

A model must never be able to supply a command. The command verifier only lets a model pick a template by name and fill in named parameters, which are checked against a regex and handed to the program as `SANDAL_PARAM_<NAME>` environment variables, never through a shell.

## Where this departs from the letter of the design

Places where the design was silent or needed a small hook are marked `// DECISION:` in the source:

- The design asks for an Anthropic adapter. It is gone on purpose: SANDAL calls no model APIs, and the harness is the model. The `Model` trait remains as the seam; `handoff` and `command` implement it.
- The error enum gained `Pending`, for a model reply that is waiting on the harness (`src/error.rs`).
- `Op::normalize`, a defaulted hook, lets `Roots` sort its input before the memo key is computed (`src/op.rs`).
- Concurrency is bounded around model, source and verifier calls rather than around whole operations, so nested `run_all` calls cannot deadlock (`src/ctx.rs`).
- `Cycle` skips failed items but never `BudgetExhausted` or `Pending` (`keep_ok` in `src/op.rs`).
- A torn last line in `ledger.jsonl` or `memo.jsonl` is cut off the file when it is opened, so the next append cannot turn it into a bad middle line (`src/ledger.rs`).
- `Pair` (cross-root pairing) is an addition the design does not name. It pairs group heads from different roots and leaves `Mix` unchanged. Acceptance: `bench/fixture4_pairing.json`, run by `tests/pairing.rs`.
- `Mix` reads "the remaining members in the same group order" as: the head of every group first, then the rest of each group in turn (`src/ops/mix.rs`).
- `fetch` spends one world call per source and only then truncates to `k`. Capping calls at `k` would silently skip sources, since a source's contribution is known only after calling it. The cost is bounded by the number of configured sources, and `max_world_calls` is the limit. Pinned by `fetch_spends_one_call_per_source_whatever_k_is` (`src/ctx.rs`, `tests/ops.rs`).
- `sandal stats` counters are per process; it also summarises the ledger. With no verifier templates, `sandal bench` uses a placeholder trial kind, since it runs no trials (`src/bin/sandal.rs`).

Still out of scope for v0, with reasons in the design: collision finder across maps, prompt tuning, a ledger-reading source, in-flight deduplication, a written syntax, multiple verifiers, multi-process access, macros, a user interface.
