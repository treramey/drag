---
name: recipe-audit-day
description: "Audit one day of Tempo worklogs with Drag. Use when the user asks for a read-only, evidence-qualified daily review and proposed corrections."
---

# Audit Day

This is a portable, host-neutral recipe. Use coding-agent context only as evidence and follow the installed Drag contract.

## Required Drag skills

- [`drag`](../drag/SKILL.md)
- [`drag-list`](../drag-list/SKILL.md)

## Evidence requirements

- Resolve the selected date with Drag's configured local time zone and documented date syntax. Ask when the intended date is unclear.
- Use only evidence available from explicit user input, host session context, the active repository, Git history, and issue context. Missing Git activity is not proof that no work occurred.
- Request only the worklog, schedule, and pagination fields needed for comparison and correction planning.

## Workflow

1. **Inspect and select the day** Run `drag --output json schema`, resolve one selected local date, and state the resolved `YYYY-MM-DD` value before analysis.
2. **Traverse the complete bounded day** Run `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,schedule.dayRequiredDuration,pagination.next,pagination.selectedDate,pagination.totalsComplete`. Follow every `pagination.next` unchanged with `--continue-from`, reusing `pagination.selectedDate`, until the day is complete. Across segments, accumulate `worklogs.duration` to calculate the selected day's logged total and ignore segment-local `schedule.dayLoggedDuration`; never add segment-local schedule totals together.
3. **Compare entries with available evidence** Check duplicate entries, overlapping intervals, missing or weak descriptions, unexpected issue allocation, and differences between logged and required time. Do not turn missing repository evidence into a claim that time is missing.
4. **Qualify every finding** Label each discrepancy `verified` when directly established, `likely` when evidence supports but does not prove it, or `unknown` when evidence is insufficient. Explain the evidence and limitation behind the label.
5. **Produce the audit report** Separate `observed worklogs`, `supporting evidence`, `discrepancies`, and `proposed corrections`. When justified, a proposal names the exact worklog ID and complete replacement values; otherwise identify the missing evidence or user decision needed.

## Stop conditions

- The selected date is ambiguous.
- Pagination fails or ends without enough metadata to establish that the bounded day is complete; report the audit as incomplete.
- A proposed correction would require inventing issue, time, date, description, or allocation evidence.

## Authorization policy

This recipe is read-only. An audit request never authorizes `drag log`, `drag delete`, a Tempo mutation, or execution of a proposed correction.

## Safety notes

- Do not execute deletion, creation, replacement, or any other mutation.
- Keep findings confidence-qualified and preserve unknowns rather than filling gaps to match required time.
