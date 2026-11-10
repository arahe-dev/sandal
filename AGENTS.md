# Driving SANDAL from an agent

SANDAL does not care which model, vendor or agent sits behind it. You can drive it from any agent that can run shell commands and read and write files, and the agent can itself be the model.

## 1. Pick a model (or be one)

Set `[model] provider` in `.sandal/config.toml`, or override it per run with environment variables (`SANDAL_PROVIDER`, `SANDAL_MODEL`, `SANDAL_BASE_URL`, `SANDAL_API_KEY_ENV`).

| provider | what it is | needs |
| --- | --- | --- |
| `handoff` (default) | **You** answer. Prompts are written to files; you write the replies. | nothing |
| `openai` | any OpenAI-compatible endpoint: OpenAI, Ollama, LM Studio, vLLM, OpenRouter, Groq, llama.cpp | `name`; `base_url` for anything but OpenAI; a key only if the server wants one |
| `anthropic` | Anthropic Messages API | `name`, `ANTHROPIC_API_KEY` |
| `command` | any program: prompt on stdin, reply on stdout (a local CLI agent, a script) | `command`, optional `args` |

`api_key_env` names the variable that holds the key, so the key never goes in the file. Local servers need none:

```toml
[model]
provider = "openai"
name = "llama3.1"
base_url = "http://localhost:11434/v1"
```

## 2. Machine-readable output

Add `--json` to any command. stdout is then exactly one JSON document, with `"ok": true` on success and, on failure, `"ok": false` plus `"status"` and `"error"`. Text goes to stderr only for the stats line.

Exit codes: `0` done, `1` error, `3` waiting on model replies.

Ids are 64 hex characters; any unique prefix of 8 or more also works.

## 3. The handoff loop (you are the model)

```text
sandal --json map kitchen --problem-file problem.txt
  -> exit 3, {"status": "pending", "pending": [{"key", "prompt_file", "response_file"}, ...]}
```

1. Read each `prompt_file`. It says what is wanted and the exact JSON shape.
2. Write **only the reply** (a JSON object, no prose) to the matching `response_file`.
3. Run the same command again. Everything already answered is read back from disk and finished steps are memoised, so a rerun only does new work. It may ask for more (a pipeline has several waves); repeat until exit 0.

`sandal --json pending` lists what is waiting at any time. If a reply is not valid JSON for the step, SANDAL asks once more with the parse error attached, as a new pending prompt.

All prompts of one wave (for example one `back` per source) are written in a single run, so you can answer them together.

## 4. A typical session

```bash
sandal init
sandal --json add source --title "..." --body "..." --domain "..." --outcome "..."
sandal --json map myproblem --problem-file problem.txt      # model: break the problem into parts
sandal --json cycle myproblem --k 8 --max-trials 5          # model: notice, back, match, carry, roots, mix
sandal --json try <proposal-id>                             # verifier: runs the proposal's trial
sandal --json read <source-id> --map myproblem              # model: what is this useful for, now
sandal --json stale                                         # readings whose evidence has moved
sandal --json establish <result-id>                         # only a verifier's Confirmed result works
```

Things worth knowing:

- Everything a model writes is a claim. Only a verifier's `confirmed` trial result can make a proposal established; you cannot write one by hand.
- A model never supplies a command. Trials are templates in config; a proposal picks one by name and fills in named parameters, which are validated.
- `uncertain` (timed out, crashed) is not evidence against an idea.
- State is plain files in `.sandal/`: `ledger.jsonl` and `memo.jsonl` are append-only, so you can read, diff and replay them.
