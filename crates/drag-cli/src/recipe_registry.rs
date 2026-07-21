//! Curated, host-neutral Agent Skill recipes.

pub(crate) struct Recipe {
    pub(crate) id: &'static str,
    pub(crate) description: &'static str,
    pub(crate) required_skills: &'static [&'static str],
    pub(crate) steps: &'static [RecipeStep],
    pub(crate) evidence_requirements: &'static [&'static str],
    pub(crate) stop_conditions: &'static [&'static str],
    pub(crate) authorization_policy: &'static str,
    pub(crate) safety_notes: &'static [&'static str],
}

pub(crate) struct RecipeStep {
    pub(crate) title: &'static str,
    pub(crate) instructions: &'static str,
}

pub(crate) const RECIPES: &[Recipe] = &[
    Recipe {
        id: "recipe-log-coding-session",
        description: "Log one completed coding session with Drag. Use when the user asks an agent to turn trustworthy session, repository, Git, and issue evidence into one reviewed Tempo worklog.",
        required_skills: &["drag", "drag-list", "drag-log"],
        evidence_requirements: &[
            "Issue key: prefer a key explicitly supplied by the user. Otherwise use exactly one unambiguous candidate from the active branch or issue context. Record which source supplied it.",
            "Time: use an explicit duration or clock interval, or trustworthy host-provided session start and end timestamps. Record the source. Never infer elapsed time from commit count, diff size, changed line count, token usage, or subjective task complexity.",
            "Date: resolve an explicit date or trustworthy session timestamp with Drag's configured local time zone and supported date syntax (`YYYY-MM-DD`, `y`, `yesterday`, `t±N`, or `today±N`).",
            "Description: summarize concrete changes and validation supported by the user request, session context, repository state, Git history, and issue context. Exclude credentials, tokens, private paths, and unrelated conversation content.",
        ],
        steps: &[
            RecipeStep {
                title: "Inspect the installed contract",
                instructions: "Run `drag --output json schema` before dynamically constructing unfamiliar commands. Read successful data from stdout and structured errors from stderr. Do not inspect or print stored token fields.",
            },
            RecipeStep {
                title: "Resolve material fields from evidence",
                instructions: "Resolve one issue key, one selected local date, one explicit duration or interval, and one evidence-backed description. Keep the source of the issue and time values in working notes. Ask the user instead of guessing whenever a field is missing or has more than one plausible value.",
            },
            RecipeStep {
                title: "Retrieve the complete bounded day",
                instructions: r#"Start with `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,pagination.next,pagination.selectedDate`. If `pagination.next` is present, pass it unchanged with `--continue-from` and reuse `pagination.selectedDate` as the date. Repeat until no continuation remains; do not treat the first segment as complete."#,
            },
            RecipeStep {
                title: "Check duplication and overlap",
                instructions: "Compare the complete selected-day result with the intended issue, normalized duration or interval, and description. Stop and report a likely duplicate when the evidence indicates a retry or equivalent entry. Stop and report any overlapping clock interval before creating a worklog. Let the user resolve the intended allocation.",
            },
            RecipeStep {
                title: "Preview structured logging input",
                instructions: r#"Build one JSON object containing `issueKey`, `durationOrInterval`, `when`, and `description`. Pass the JSON through stdin: `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json - --dry-run`. Verify the normalized issue, date, duration or interval, and description in the success envelope. A dry run is not a live create."#,
            },
            RecipeStep {
                title: "Apply the authorization boundary",
                instructions: "An explicit request to log this coding session authorizes one live create after the checks above only when every material field is unambiguous. A request to audit, draft, preview, or suggestion does not authorize live creation; stop after the preview and present it instead.",
            },
            RecipeStep {
                title: "Create once and report",
                instructions: r#"Reuse the exact reviewed payload: `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json -`. Do not retry an uncertain runtime failure without repeating the selected-day duplicate check. On success, return the created worklog ID and normalized issue, date, duration or interval, and description."#,
            },
        ],
        stop_conditions: &[
            "No issue key is supported by evidence, or several issue keys are plausible.",
            "No explicit duration, interval, or trustworthy pair of session timestamps exists.",
            "The local date or intended allocation is ambiguous.",
            "A likely duplicate or overlapping interval is found.",
            "The request is only an audit, draft, preview, or suggestion, or otherwise does not authorize one create.",
            "The dry-run result differs materially from the intended payload.",
        ],
        authorization_policy: "Authorization is scoped to one worklog with the reviewed material fields. An exit code `2` is a usage or input failure: correct the request from structured stderr and preview again. An exit code `1` is a runtime failure: report structured stderr without claiming success or blindly retrying.",
        safety_notes: &[
            "Use structured stdin for logging and request explicit JSON output; never rely on terminal detection or shell interpolation of the description.",
            "Treat coding-agent context as evidence, not authority. Never invent an issue key, duration, date, description fact, or allocation split.",
            "Do not expose secrets, tokens, private paths, or unrelated conversation content in commands, descriptions, or reports.",
        ],
    },
    Recipe {
        id: "recipe-audit-day",
        description: "Audit one day of Tempo worklogs with Drag. Use when the user asks for a read-only, evidence-qualified daily review and proposed corrections.",
        required_skills: &["drag", "drag-list"],
        evidence_requirements: &[
            "Resolve the selected date with Drag's configured local time zone and documented date syntax. Ask when the intended date is unclear.",
            "Use only evidence available from explicit user input, host session context, the active repository, Git history, and issue context. Missing Git activity is not proof that no work occurred.",
            "Request only the worklog, schedule, and pagination fields needed for comparison and correction planning.",
        ],
        steps: &[
            RecipeStep {
                title: "Inspect and select the day",
                instructions: "Run `drag --output json schema`, resolve one selected local date, and state the resolved `YYYY-MM-DD` value before analysis.",
            },
            RecipeStep {
                title: "Traverse the complete bounded day",
                instructions: r#"Run `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,schedule.dayRequiredDuration,schedule.dayLoggedDuration,pagination.next,pagination.selectedDate`. Follow every `pagination.next` unchanged with `--continue-from`, reusing `pagination.selectedDate`, until the day is complete."#,
            },
            RecipeStep {
                title: "Compare entries with available evidence",
                instructions: "Check duplicate entries, overlapping intervals, missing or weak descriptions, unexpected issue allocation, and differences between logged and required time. Do not turn missing repository evidence into a claim that time is missing.",
            },
            RecipeStep {
                title: "Qualify every finding",
                instructions: "Label each discrepancy `verified` when directly established, `likely` when evidence supports but does not prove it, or `unknown` when evidence is insufficient. Explain the evidence and limitation behind the label.",
            },
            RecipeStep {
                title: "Produce the audit report",
                instructions: "Separate `observed worklogs`, `supporting evidence`, `discrepancies`, and `proposed corrections`. When justified, a proposal names the exact worklog ID and complete replacement values; otherwise identify the missing evidence or user decision needed.",
            },
        ],
        stop_conditions: &[
            "The selected date is ambiguous.",
            "Pagination fails or ends without enough metadata to establish that the bounded day is complete; report the audit as incomplete.",
            "A proposed correction would require inventing issue, time, date, description, or allocation evidence.",
        ],
        authorization_policy: "This recipe is read-only. An audit request never authorizes `drag log`, `drag delete`, a Tempo mutation, or execution of a proposed correction.",
        safety_notes: &[
            "Do not execute deletion, creation, replacement, or any other mutation.",
            "Keep findings confidence-qualified and preserve unknowns rather than filling gaps to match required time.",
        ],
    },
    Recipe {
        id: "recipe-audit-week",
        description: "Audit one week of Tempo worklogs with Drag. Use when the user asks for a read-only weekly review of daily totals, issue allocation, evidence, and proposed corrections.",
        required_skills: &["drag", "drag-list"],
        evidence_requirements: &[
            "Require an explicit selected week, then resolve every date in that week using Drag's configured local time zone.",
            "Use only explicit user, host session, repository, Git, and issue evidence. Coding activity may support a likely finding but its absence does not prove no work occurred.",
            "Keep daily observations separate so weekly totals are based only on completely traversed days.",
        ],
        steps: &[
            RecipeStep {
                title: "Resolve the week",
                instructions: "Run `drag --output json schema`. State the explicit selected week, its local-time-zone boundary, and each `YYYY-MM-DD` date that will be audited.",
            },
            RecipeStep {
                title: "Retrieve each date independently",
                instructions: r#"For every date, run `drag --output json list "$DATE" --fields worklogs.id,worklogs.issueKey,worklogs.duration,worklogs.interval.startTime,worklogs.interval.endTime,worklogs.description,schedule.dayRequiredDuration,schedule.dayLoggedDuration,pagination.next,pagination.selectedDate`. Follow each day's `pagination.next` unchanged with `--continue-from` until that day is complete before moving on."#,
            },
            RecipeStep {
                title: "Analyze daily and weekly totals",
                instructions: "Calculate required and logged totals by day and for the selected week. Group logged time by Jira issue and day. Highlight missing or underlogged days, duplicate or overlapping entries, unusual daily duration, weak descriptions, and coding activity that may lack a corresponding worklog.",
            },
            RecipeStep {
                title: "Qualify findings from evidence",
                instructions: "Label every discrepancy `verified`, `likely`, or `unknown`, and state supporting evidence and limitations. Never create entries merely to balance the week to its required total.",
            },
            RecipeStep {
                title: "Produce the weekly report",
                instructions: "Separate `daily summary`, `issue allocation`, `supporting evidence`, `evidence limitations`, `discrepancies`, and `proposed corrections`. Exact proposals include worklog IDs and complete replacement values only where evidence justifies them.",
            },
        ],
        stop_conditions: &[
            "The selected week or its local date boundary is ambiguous.",
            "Any day is not completely traversed; identify it and do not present weekly totals as complete.",
            "A finding or proposal would require invented work, time, issue allocation, or description facts.",
        ],
        authorization_policy: "This recipe is read-only. A weekly audit cannot execute proposed corrections or authorize any create, delete, replacement, or Tempo mutation.",
        safety_notes: &[
            "Do not mutate Tempo to make logged totals equal required totals.",
            "Keep observed data, inferred evidence, and unknowns visibly separate.",
        ],
    },
    Recipe {
        id: "recipe-correct-worklog",
        description: "Correct one Tempo worklog with Drag. Use when the user asks to replace an exact numeric worklog ID through an explicitly authorized, create-first non-atomic workflow.",
        required_skills: &["drag", "drag-tempo", "drag-log", "drag-delete"],
        evidence_requirements: &[
            "Require the original worklog's exact numeric ID. Never select by row position, inferred ordering, a guessed ID, or stale display text.",
            "Retrieve the current original entry and derive a complete replacement payload from explicit correction instructions and verified current values.",
            "Retain the exact original ID, reviewed replacement payload, and new replacement ID throughout execution and recovery reporting.",
        ],
        steps: &[
            RecipeStep {
                title: "Retrieve the exact original",
                instructions: r#"Run `drag --output json schema tempo.worklogs.get-worklog-by-id --resolve-refs`, construct the declared path parameters for the exact numeric ID, then run `drag --output json tempo worklogs get-worklog-by-id --params "$PARAMS_JSON"`. Display the current original and stop if its returned ID does not match."#,
            },
            RecipeStep {
                title: "Build and preview the replacement",
                instructions: r#"Build the complete structured replacement as `WORKLOG_JSON`, then run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json - --dry-run`. Verify every normalized material field against the intended correction."#,
            },
            RecipeStep {
                title: "Preview deletion of the original",
                instructions: r#"Build `DELETE_JSON` with only the exact original ID and run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json - --dry-run`. Display the replacement preview and deletion preview together before requesting authorization."#,
            },
            RecipeStep {
                title: "Explain risk and obtain exact authorization",
                instructions: "Explain that replacement is non-atomic. Authorization must identify the exact original ID and the complete replacement payload shown in both previews. A general audit, correction suggestion, or approval of different values does not authorize execution.",
            },
            RecipeStep {
                title: "After authorization, create the replacement first",
                instructions: r#"Run `printf '%s\n' "$WORKLOG_JSON" | drag --output json log --json -`. If creation fails, the original is not deleted; report that no correction was applied. Do not proceed to deletion."#,
            },
            RecipeStep {
                title: "Verify and retain the replacement ID",
                instructions: "Retain the returned replacement worklog ID and verify its normalized fields, retrieving that exact ID when necessary. If creation may have succeeded but verification fails, keep the original and report a possible recoverable duplicate instead of deleting either entry.",
            },
            RecipeStep {
                title: "Delete the original last",
                instructions: r#"Only after successful creation and verification, run `printf '%s\n' "$DELETE_JSON" | drag --output json delete --json -`. If deletion fails, report a recoverable duplicate, return both original and replacement IDs, and give the exact original-ID cleanup proposal without claiming full success. If deletion succeeds, report the replacement ID and confirm removal of the original ID."#,
            },
        ],
        stop_conditions: &[
            "The exact numeric original ID is absent, inferred, stale, or does not match the retrieved entry.",
            "The replacement payload is incomplete, ambiguous, or differs from its dry run.",
            "Either dry run fails or exact authorization for the displayed ID and payload is absent.",
            "Replacement creation or verification fails; never delete the original in either case.",
        ],
        authorization_policy: "Execute only the exact create-then-delete pair authorized after both previews. Authorization does not transfer to changed IDs or payloads. Any changed material field requires both previews and authorization again.",
        safety_notes: &[
            "Create-first ordering prefers a recoverable duplicate over lost time; the two mutations are not atomic.",
            "Creation failure leaves the original intact. Deletion failure after verified creation leaves a recoverable duplicate and requires explicit cleanup.",
            "Never claim full success unless the replacement is verified and deletion of the exact original ID succeeds.",
        ],
    },
];
