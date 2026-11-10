# Using SANDAL from a harness

SANDAL is a tool for a harness (Claude Code, Codex, Cursor, a script, you), not a harness itself. It holds no API keys and calls no model API. The harness that drives it is the model.

What SANDAL does is the part a model is bad at holding in its head: every derived thing is stored with what it was derived from, results are cached, and when new evidence arrives only the readings that depended on it are marked stale. The harness does the thinking steps; SANDAL does the bookkeeping.

## The loop

Some steps need a model: `map`, `back`, `match`, `carry`, `roots`, `read`. When SANDAL reaches one it does not call anything. It writes the prompt down, stops, and exits with code **3**:

```text
sandal --json cycle kitchen
  -> exit 3
     {"ok": false, "status": "pending",
      "pending": [{"key": "9f2c…", "prompt": "TASK: back\n…", "prompt_file": "…", "response_file": "…"}, …]}
```

1. For each pending item, read `prompt`. It says what is wanted and gives the exact JSON shape.
2. Answer with **only the reply** (a JSON object, no prose, no fences):
   `sandal answer <key> --file reply.json`, or pipe it on stdin: `sandal answer <key> < reply.json`.
   The key may be any unique prefix.
3. Run the same command again. Everything already answered is read back, and finished steps are memoised, so a rerun only does new work. A pipeline has several waves (`back`, then `match`, then `carry`, …); repeat until the exit code is 0.

All prompts of one wave (say one `back` per source) are written in a single run, so answer them together. `sandal pending` lists what is waiting at any time.

If a reply is not valid for its step, SANDAL asks once more, with the parse error attached, as a new pending prompt. It never guesses.

## Output and exit codes

Add `--json` to any command: stdout is then exactly one JSON document, `"ok": true` on success, and on failure `"ok": false` with `"status"` (`"error"` or `"pending"`).

| exit | meaning |
| --- | --- |
| 0 | done |
| 1 | error (the message says what is missing) |
| 3 | waiting on model replies |

Ids are 64 hex characters. Any unique prefix of 8 or more works.

## A session

```bash
sandal init
sandal --json add source --title "..." --body "..." --domain "..." --outcome "..."
sandal --json map myproblem --problem-file problem.txt      # model step: break the problem into parts
sandal --json cycle myproblem --k 8 --max-trials 5          # model steps: back, match, carry, roots; then mix
sandal --json try <proposal-id>                             # runs the proposal's trial through the verifier
sandal --json read <source-id> --map myproblem              # model step: what is this useful for, now
sandal --json stale                                         # readings whose evidence has moved
sandal --json establish <result-id>                         # only a verifier's Confirmed result works
```

## Rules the tool enforces

- Everything a model writes is a claim. Only a verifier's `confirmed` trial result can make a proposal established; you cannot write one by hand.
- A model never supplies a command. Trials are templates in `.sandal/config.toml`; a proposal picks one by name and fills in named parameters, which are validated.
- `uncertain` (timed out, crashed) is not evidence against an idea.
- State is plain files in `.sandal/`: `ledger.jsonl` and `memo.jsonl` are append-only, so you can read, diff and replay them.

## When a hand-off per step is too slow

Set `provider = "command"` in `[model]` (or `SANDAL_PROVIDER=command` and `SANDAL_COMMAND=<program>`). SANDAL then runs that program for each model step: the prompt goes to its stdin and the reply is its stdout. Point it at any harness CLI that has a one-shot mode, or at a script. SANDAL still never sees a key; whatever the program needs, the program holds.
