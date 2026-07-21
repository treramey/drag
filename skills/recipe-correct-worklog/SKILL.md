---
name: recipe-correct-worklog
description: "Correct one Tempo worklog with Drag. Use when the user asks to replace an exact numeric worklog ID through an explicitly authorized, create-first non-atomic workflow."
---

# Correct Worklog

This is a portable, host-neutral recipe. Use coding-agent context only as evidence and follow the installed Drag contract.

## Required Drag skills

- [`drag`](../drag/SKILL.md)
- [`drag-tempo`](../drag-tempo/SKILL.md)
- [`drag-log`](../drag-log/SKILL.md)
- [`drag-delete`](../drag-delete/SKILL.md)

## Evidence requirements

- Require the original worklog's exact numeric ID. Never select by row position, inferred ordering, a guessed ID, or stale display text.
- Retrieve the current original entry and derive a complete replacement payload from explicit correction instructions and verified current values.
- Confirm that the original has no work attributes, billable seconds, or other accounting fields that `drag log` cannot represent. If any are present, stop rather than silently discard them and use a separately reviewed generic Tempo create workflow capable of preserving the complete payload.
- Retain the exact original ID, reviewed replacement payload, and new replacement ID throughout execution and recovery reporting.

## Workflow

1. **Retrieve the exact original** Run `drag --output json schema tempo.worklogs.get-worklog-by-id --resolve-refs`, construct the declared path parameters for the exact numeric ID, then run `drag --output json tempo worklogs get-worklog-by-id --params "$PARAMS_JSON"`. Display and retain the complete current original as the reviewed snapshot. Stop if its returned ID does not match or if it contains work attributes, billable seconds, or another field that `drag log` cannot preserve.
2. **Build and preview the replacement** Build the complete structured replacement as `WORKLOG_JSON`, then run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json - --dry-run`. Verify every normalized material field against the intended correction.
3. **Preview deletion of the original** Build `DELETE_JSON` with only the exact original ID and run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json - --dry-run`. Display the replacement preview and deletion preview together before requesting authorization.
4. **Explain risk and obtain exact authorization** Explain that replacement is non-atomic. Authorization must identify the exact original ID and the complete replacement payload shown in both previews. A general audit, correction suggestion, or approval of different values does not authorize execution.
5. **Revalidate, then create the replacement first** Immediately before creation, retrieve the exact original ID again and compare every returned field with the reviewed snapshot. If it changed, stop and require new previews and authorization. Record the exact IDs of any existing selected-day worklogs that already match the replacement payload, then run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json -`. An exit code `2` or another confirmed pre-request failure means the original is not deleted and no correction was applied. An exit code `1`, timeout, lost response, or other uncertain runtime failure may have committed the create: do not delete the original or claim failure; search the selected day again and retrieve by exact ID only a newly observed matching candidate to reconcile the state.
6. **Verify and retain the replacement ID** Retain the returned or reconciled replacement worklog ID and retrieve that exact ID to verify its normalized fields. If an uncertain creation cannot be reconciled to exactly one verified replacement, keep the original and report the observed candidates as a possible recoverable duplicate instead of deleting any entry.
7. **Revalidate again, then delete the original last** Immediately before deletion, retrieve the exact original ID again and compare every field with the reviewed snapshot. If it changed, do not delete it; report the verified replacement and changed original as a recoverable duplicate requiring a newly reviewed cleanup. If unchanged, run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json -`. After any deletion error or uncertain response, re-fetch both exact IDs before reporting: if only the replacement exists, report that the correction completed despite the response; if both exist, report a recoverable duplicate with both IDs and an exact cleanup proposal; for any other observed state, report it without claiming full success. If deletion succeeds, still confirm that the replacement exists and the original does not before reporting success.

## Stop conditions

- The exact numeric original ID is absent, inferred, stale, or does not match the retrieved entry.
- The original contains work attributes, billable seconds, or another field that `drag log` cannot preserve.
- The replacement payload is incomplete, ambiguous, or differs from its dry run.
- Either dry run fails or exact authorization for the displayed ID and payload is absent.
- The original changes after review, or replacement creation cannot be reconciled and verified; never delete the original in either case.

## Authorization policy

Execute only the exact create-then-delete pair authorized after both previews. Authorization does not transfer to changed IDs or payloads. Any changed material field requires both previews and authorization again.

## Safety notes

- Create-first ordering prefers a recoverable duplicate over lost time; the two mutations are not atomic.
- A confirmed pre-request creation failure leaves the original intact. An uncertain create or delete response must be reconciled from exact-ID reads before describing the resulting state.
- Never claim full success unless the replacement is verified and deletion of the exact original ID succeeds.
