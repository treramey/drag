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
- Retain the exact original ID, reviewed replacement payload, and new replacement ID throughout execution and recovery reporting.

## Workflow

1. **Retrieve the exact original** Run `drag --output json schema tempo.worklogs.get-worklog-by-id --resolve-refs`, construct the declared path parameters for the exact numeric ID, then run `drag --output json tempo worklogs get-worklog-by-id --params "$PARAMS_JSON"`. Display the current original and stop if its returned ID does not match.
2. **Build and preview the replacement** Build the complete structured replacement as `WORKLOG_JSON`, then run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json - --dry-run`. Verify every normalized material field against the intended correction.
3. **Preview deletion of the original** Build `DELETE_JSON` with only the exact original ID and run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json - --dry-run`. Display the replacement preview and deletion preview together before requesting authorization.
4. **Explain risk and obtain exact authorization** Explain that replacement is non-atomic. Authorization must identify the exact original ID and the complete replacement payload shown in both previews. A general audit, correction suggestion, or approval of different values does not authorize execution.
5. **After authorization, create the replacement first** Run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json -`. If creation fails, the original is not deleted; report that no correction was applied. Do not proceed to deletion.
6. **Verify and retain the replacement ID** Retain the returned replacement worklog ID and verify its normalized fields, retrieving that exact ID when necessary. If creation may have succeeded but verification fails, keep the original and report a possible recoverable duplicate instead of deleting either entry.
7. **Delete the original last** Only after successful creation and verification, run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json -`. If deletion fails, report a recoverable duplicate, return both original and replacement IDs, and give the exact original-ID cleanup proposal without claiming full success. If deletion succeeds, report the replacement ID and confirm removal of the original ID.

## Stop conditions

- The exact numeric original ID is absent, inferred, stale, or does not match the retrieved entry.
- The replacement payload is incomplete, ambiguous, or differs from its dry run.
- Either dry run fails or exact authorization for the displayed ID and payload is absent.
- Replacement creation or verification fails; never delete the original in either case.

## Authorization policy

Execute only the exact create-then-delete pair authorized after both previews. Authorization does not transfer to changed IDs or payloads. Any changed material field requires both previews and authorization again.

## Safety notes

- Create-first ordering prefers a recoverable duplicate over lost time; the two mutations are not atomic.
- Creation failure leaves the original intact. Deletion failure after verified creation leaves a recoverable duplicate and requires explicit cleanup.
- Never claim full success unless the replacement is verified and deletion of the exact original ID succeeds.
