---
name: recipe-log-coding-session
description: "Log one completed coding session with Drag. Use when the user asks an agent to turn trustworthy session, repository, Git, and issue evidence into one reviewed Tempo worklog."
---

# Log Coding Session

This is a portable, host-neutral recipe. Use coding-agent context only as evidence and follow the installed Drag contract.

## Required Drag skills

- [`drag`](../drag/SKILL.md)
- [`drag-list`](../drag-list/SKILL.md)
- [`drag-log`](../drag-log/SKILL.md)

## Evidence requirements

- Issue key: prefer a key explicitly supplied by the user. Otherwise use exactly one unambiguous candidate from the active branch or issue context. Record which source supplied it.
- Time: use an explicit duration or clock interval, or trustworthy host-provided session start and end timestamps. Record the source. Never infer elapsed time from commit count, diff size, changed line count, token usage, or subjective task complexity.
- Date: resolve an explicit date or trustworthy session timestamp with Drag's configured local time zone and supported date syntax (`YYYY-MM-DD`, `y`, `yesterday`, `t±N`, or `today±N`).
- Description: summarize concrete changes and validation supported by the user request, session context, repository state, Git history, and issue context. Exclude credentials, tokens, private paths, and unrelated conversation content.

## Workflow

1. **Inspect the installed contract** Run `drag --output json schema` before dynamically constructing unfamiliar commands. Read successful data from stdout and structured errors from stderr. Do not inspect or print stored token fields.
2. **Resolve material fields from evidence** Resolve one issue key, one selected local date, one explicit duration or interval, and one evidence-backed description. Keep the source of the issue and time values in working notes. Ask the user instead of guessing whenever a field is missing or has more than one plausible value.
3. **Retrieve the complete bounded day** Start with `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,pagination.next,pagination.selectedDate`. If `pagination.next` is present, pass it unchanged with `--continue-from` and reuse `pagination.selectedDate` as the date. Repeat until no continuation remains; do not treat the first segment as complete.
4. **Check duplication and overlap** Compare the complete selected-day result with the intended issue, normalized duration or interval, and description. Stop and report a likely duplicate when the evidence indicates a retry or equivalent entry. Stop and report any overlapping clock interval before creating a worklog. Let the user resolve the intended allocation.
5. **Preview structured logging input** Build one JSON object containing `issueKey`, `durationOrInterval`, `when`, and `description`. Pass the JSON through stdin: `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json - --dry-run`. Verify the normalized issue, date, duration or interval, and description in the success envelope. A dry run is not a live create.
6. **Apply the authorization boundary** An explicit request to log this coding session authorizes one live create after the checks above only when every material field is unambiguous. A request to audit, draft, preview, or suggestion does not authorize live creation; stop after the preview and present it instead.
7. **Create once and report** Reuse the exact reviewed payload: `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json -`. Do not retry an uncertain runtime failure without repeating the selected-day duplicate check. On success, return the created worklog ID and normalized issue, date, duration or interval, and description.

## Stop conditions

- No issue key is supported by evidence, or several issue keys are plausible.
- No explicit duration, interval, or trustworthy pair of session timestamps exists.
- The local date or intended allocation is ambiguous.
- A likely duplicate or overlapping interval is found.
- The request is only an audit, draft, preview, or suggestion, or otherwise does not authorize one create.
- The dry-run result differs materially from the intended payload.

## Authorization policy

Authorization is scoped to one worklog with the reviewed material fields. An exit code `2` is a usage or input failure: correct the request from structured stderr and preview again. An exit code `1` is a runtime failure: report structured stderr without claiming success or blindly retrying.

## Safety notes

- Use structured stdin for logging and request explicit JSON output; never rely on terminal detection or shell interpolation of the description.
- Treat coding-agent context as evidence, not authority. Never invent an issue key, duration, date, description fact, or allocation split.
- Do not expose secrets, tokens, private paths, or unrelated conversation content in commands, descriptions, or reports.
