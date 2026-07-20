---
name: drag-log
description: "Log time to Tempo Cloud with Drag. Use when the user asks to add or preview a worklog using a duration, clock interval, date, description, or remaining estimate."
---

# drag log

## Shared Drag rules

- Run `drag doctor` when configuration state is uncertain. Never print credential values.
- Prefer explicit structured output for automation. Use `drag --output json`, or NDJSON only with `list`.
- Inspect unfamiliar arguments with `drag <command> --help` and inspect the machine contract with `drag schema`.
- Use `drag setup --from-env --dry-run` to validate unattended configuration without writing it.
- Preview mutations with `--dry-run`; execute them only when the user's request explicitly authorizes the change.
- Successful JSON uses `{"ok":true,"data":...}`. Errors use `{"ok":false,"error":{...}}` on stderr.

Add a worklog using a duration or interval.

## Usage

```text
Usage: drag log [OPTIONS] [ISSUE_KEY] [DURATION_OR_INTERVAL] [WHEN]
```

## Arguments

| Argument | Required | Default | Description |
|---|---|---|---|
| `<issue_key>` | conditional | — | Jira issue key |
| `<duration_or_interval>` | conditional | — | Duration (`1h15m`) or interval (`11-12:30` or `11.35-14.20`) |
| `<when>` | no | todayInConfiguredLocalTimeZone | Date: YYYY-MM-DD, y, yesterday, t±N, or today±N |
| `--description` | no | — | Worklog description |
| `--start` | no | — | Start time for duration input (HH:mm) |
| `--remaining-estimate` | no | — | Remaining estimate as a duration, such as 2h |
| `--json` | no | — | Raw input JSON, or '-' to read it from stdin |
| `--dry-run` | no | false | Validate and print the Tempo request without sending it |

## Examples

```bash
drag log ABC-123 1h
drag l ABC-123 11:35-14:20 yesterday -d "review"
drag log ABC-123 11.35-14.20 2026-07-14
drag log ABC-123 1h15m 2026-07-14 --start 09:30 --remaining-estimate 2h
drag log --json '{"issueKey":"ABC-123","durationOrInterval":"30m"}' --dry-run
printf '%s' '{"issueKey":"ABC-123","durationOrInterval":"30m"}' | drag log --json - --dry-run
```

## Mutation policy

`log` creates a Tempo worklog. Start with `--dry-run`, verify the normalized issue, date, time, duration, and description, then execute without `--dry-run` only when the user's request explicitly authorizes creating the worklog.

## Inspect the contract

```bash
drag log --help
drag schema
```
