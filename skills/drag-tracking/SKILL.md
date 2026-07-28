---
name: drag-tracking
description: "Operate Drag automatic time tracking. Use when the user asks to configure approved local sources, inspect tracking health, run or review a day, manage the schedule, or change submission policy."
---

# drag tracking

## Shared Drag rules

- Run `drag doctor` when configuration state is uncertain. Never print credential values.
- Prefer explicit structured output for automation. Use `drag --output json`, or NDJSON only with `list`.
- Inspect unfamiliar arguments with `drag <command> --help` and inspect the machine contract with `drag schema`.
- Use `drag setup --from-env --dry-run` to validate unattended configuration without writing it.
- Preview mutations with `--dry-run`; execute them only when the user's request explicitly authorizes the change.
- Successful JSON uses `{"ok":true,"data":...}`. Errors use `{"ok":false,"error":{...}}` on stderr.

Manage automatic time tracking.

## Usage

```text
Usage: drag tracking [OPTIONS] <COMMAND>
```

## Tracking safety policy

Use intent-level `drag tracking setup|status|run|review|pause|resume|uninstall`, `drag tracking sources ...`, and `drag tracking schedule ...` commands. Never invoke the hidden `drag-tracking internal` pipeline for normal work. Draft mode cannot submit. Review mode requires `drag tracking review approve DATE` for the current proposal-set digest. Automatic mode requires separate setup authorization and every runtime safety gate.

## Inspect the contract

```bash
drag tracking --help
drag schema
```
