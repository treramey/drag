---
name: recipe-audit-week
description: "Audit one week of Tempo worklogs with Drag. Use when the user asks for a read-only weekly review of daily totals, issue allocation, evidence, and proposed corrections."
---

# Audit Week

This is a portable, host-neutral recipe. Use coding-agent context only as evidence and follow the installed Drag contract.

## Required Drag skills

- [`drag`](../drag/SKILL.md)
- [`drag-list`](../drag-list/SKILL.md)

## Evidence requirements

- Require an explicit selected week, then resolve every date in that week using Drag's configured local time zone.
- Use only explicit user, host session, repository, Git, and issue evidence. Coding activity may support a likely finding but its absence does not prove no work occurred.
- Keep daily observations separate so weekly totals are based only on completely traversed days.

## Workflow

1. **Resolve the week** Run `drag --output json schema`. State the explicit selected week, its local-time-zone boundary, and each `YYYY-MM-DD` date that will be audited.
2. **Retrieve each date independently** For every date, run `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,schedule.dayRequiredDuration,schedule.dayLoggedDuration,pagination.next,pagination.selectedDate`. Follow each day's `pagination.next` unchanged with `--continue-from` until that day is complete before moving on.
3. **Analyze daily and weekly totals** Calculate required and logged totals by day and for the selected week. Group logged time by Jira issue and day. Highlight missing or underlogged days, duplicate or overlapping entries, unusual daily duration, weak descriptions, and coding activity that may lack a corresponding worklog.
4. **Qualify findings from evidence** Label every discrepancy `verified`, `likely`, or `unknown`, and state supporting evidence and limitations. Never create entries merely to balance the week to its required total.
5. **Produce the weekly report** Separate `daily summary`, `issue allocation`, `supporting evidence`, `evidence limitations`, `discrepancies`, and `proposed corrections`. Exact proposals include worklog IDs and complete replacement values only where evidence justifies them.

## Stop conditions

- The selected week or its local date boundary is ambiguous.
- Any day is not completely traversed; identify it and do not present weekly totals as complete.
- A finding or proposal would require invented work, time, issue allocation, or description facts.

## Authorization policy

This recipe is read-only. A weekly audit cannot execute proposed corrections or authorize any create, delete, replacement, or Tempo mutation.

## Safety notes

- Do not mutate Tempo to make logged totals equal required totals.
- Keep observed data, inferred evidence, and unknowns visibly separate.
