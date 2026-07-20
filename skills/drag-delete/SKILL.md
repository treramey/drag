---
name: drag-delete
description: "Delete Tempo Cloud worklogs with Drag. Use when the user explicitly asks to preview or delete one or more worklogs by numeric ID."
---

# drag delete

## Shared Drag rules

- Run `drag doctor` when configuration state is uncertain. Never print credential values.
- Prefer explicit structured output for automation. Use `drag --output json`, or NDJSON only with `list`.
- Inspect unfamiliar arguments with `drag <command> --help` and inspect the machine contract with `drag schema`.
- Use `drag setup --from-env --dry-run` to validate unattended configuration without writing it.
- Preview mutations with `--dry-run`; execute them only when the user's request explicitly authorizes the change.
- Successful JSON uses `{"ok":true,"data":...}`. Errors use `{"ok":false,"error":{...}}` on stderr.

Delete one or more worklogs.

## Usage

```text
Usage: drag delete [OPTIONS] [WORKLOG_IDS]...
```

## Arguments

| Argument | Required | Default | Description |
|---|---|---|---|
| `<worklog_ids>` | conditional | — | Numeric Tempo worklog IDs |
| `--json` | no | — | Raw input JSON, or '-' to read it from stdin |
| `--dry-run` | no | false | Show what would be deleted without changing Tempo |

## Destructive-operation policy

`delete` permanently removes Tempo worklogs and a multi-ID deletion is not atomic. First run the exact IDs with `--dry-run`. Execute without `--dry-run` only when the user explicitly authorizes deleting those IDs. Never infer IDs from position or stale output.

## Inspect the contract

```bash
drag delete --help
drag schema
```
