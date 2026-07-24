---
name: drag-list
description: "List and inspect Tempo Cloud worklogs with Drag. Use when the user asks to review time entries, retrieve worklogs for a date, paginate results, or select structured output fields."
---

# drag list

## Shared Drag rules

- Run `drag doctor` when configuration state is uncertain. Never print credential values.
- Prefer explicit structured output for automation. Use `drag --output json`, or NDJSON only with `list`.
- Inspect unfamiliar arguments with `drag <command> --help` and inspect the machine contract with `drag schema`.
- Use `drag setup --from-env --dry-run` to validate unattended configuration without writing it.
- Preview mutations with `--dry-run`; execute them only when the user's request explicitly authorizes the change.
- Successful JSON uses `{"ok":true,"data":...}`. Errors use `{"ok":false,"error":{...}}` on stderr.

List worklogs for a date without changing Jira or Tempo.

## Usage

```text
Usage: drag list [OPTIONS] [DATE]
```

## Arguments

| Argument | Required | Default | Description |
|---|---|---|---|
| `<when>` | no | todayInConfiguredLocalTimeZone | Optional date (defaults to today): YYYY-MM-DD, y, yesterday, t±N, or today±N |
| `--verbose` | no | false | Include descriptions and Jira URLs |
| `--fields` | no | — | Comma-delimited result fields to include in structured output |
| `--limit` | no | 100 | Maximum worklogs to retrieve and return (1-1000; default: 100) |
| `--page-limit` | no | 1 | Maximum Tempo pages to retrieve (1-100; default: 1) |
| `--continue-from` | no | — | Resume from the opaque continuation token returned by a prior list result |
| `--all-pages` | no | false | Retrieve every page, subject to the 100-page safety ceiling |

## Automation policy

Use `drag --output json list` explicitly so an interactive terminal never opens. Each worklog includes its Tempo work attributes in `attributes`; use `--fields worklogs.attributes` to select only them. Do not make a second Tempo request to discover attributes already returned by `list`. Use `--fields` to reduce structured output, and preserve `pagination.next` when another segment may be needed. `list` is read-only; its interactive human view can open a Jira URL only after an explicit keypress.

## Inspect the contract

```bash
drag list --help
drag schema
```
