---
name: drag
description: "Operate Tempo Cloud worklogs with Drag. Use when an agent needs to configure Drag, choose a command, inspect its contract, or follow shared automation and safety rules."
---

# Drag CLI

Use `drag` to work with Tempo Cloud through stable structured output and explicit dry-run paths.

## Agent workflow

1. Run `drag doctor` when configuration state is uncertain. Do not print credential values.
2. Prefer `drag --output json <command>` for automation. NDJSON is supported only by `list`.
3. Inspect `drag <command> --help` before constructing unfamiliar arguments.
4. Inspect the complete machine contract with `drag schema`.
5. Preview mutations with `--dry-run`; execute them only when the user's request explicitly authorizes the change.

## Task skills

| Skill | Command | Description |
|---|---|---|
| [`drag-log`](../drag-log/SKILL.md) | `drag log` | Add a worklog using a duration or interval |
| [`drag-list`](../drag-list/SKILL.md) | `drag list` | List worklogs for a date without changing Jira or Tempo |
| [`drag-delete`](../drag-delete/SKILL.md) | `drag delete` | Delete one or more worklogs |

## Configuration and secrets

- Use interactive `drag setup` only when the user can complete terminal prompts.
- For unattended setup, supply credentials through documented environment variables and use `drag setup --from-env`.
- Never echo, log, summarize, or include Atlassian or Tempo tokens in output.
- Use `drag setup --from-env --dry-run` to validate unattended configuration without writing it.

## Output contract

Successful JSON uses `{"ok":true,"data":...}`. Errors use `{"ok":false,"error":{"code":...,"message":...}}` on stderr. Treat exit code 2 as invalid input or usage and exit code 1 as a runtime failure.
