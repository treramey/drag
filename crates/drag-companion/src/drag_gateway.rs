use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DragReadResult {
    pub(crate) status: &'static str,
    pub(crate) selected_date: NaiveDate,
    pub(crate) pages: usize,
    pub(crate) worklogs: Vec<NormalizedWorklog>,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedWorklog {
    pub(crate) tempo_worklog_id: String,
    pub(crate) issue_key: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) description: String,
    pub(crate) attributes: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuditResult {
    pub(crate) status: &'static str,
    pub(crate) selected_date: NaiveDate,
    pub(crate) existing_worklogs: Vec<NormalizedWorklog>,
    pub(crate) duplicate_proposal_ids: Vec<String>,
    pub(crate) overlapping_proposal_ids: Vec<String>,
    pub(crate) decisions: Vec<PolicyDecision>,
    pub(crate) unsupported_periods: Vec<UnsupportedPeriodDecision>,
    pub(crate) unattended_authorization: UnattendedAuthorization,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PolicyDecision {
    pub(crate) proposal_id: String,
    pub(crate) decision: &'static str,
    pub(crate) reason_codes: Vec<&'static str>,
    pub(crate) evidence_trace: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnsupportedPeriodDecision {
    pub(crate) id: String,
    pub(crate) decision: &'static str,
    pub(crate) reason_codes: Vec<&'static str>,
    pub(crate) evidence_trace: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnattendedAuthorization {
    pub(crate) required_for_approval: bool,
    pub(crate) provided: bool,
    pub(crate) mutation_allowed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProposalPolicyInput {
    pub(crate) id: String,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) issue_key: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) description_facts: Vec<String>,
    pub(crate) limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewResult {
    pub(crate) status: &'static str,
    pub(crate) classification: &'static str,
    pub(crate) selected_date: NaiveDate,
    pub(crate) payload: Value,
    pub(crate) drag_preview: Value,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
}

pub(crate) fn read_drag_day(
    drag_bin: &Path,
    date: NaiveDate,
) -> Result<DragReadResult, CompanionError> {
    verify_drag_contract(drag_bin)?;
    let mut continuation: Option<String> = None;
    let mut seen_continuations = std::collections::BTreeSet::new();
    let mut worklogs = Vec::new();
    let mut pages = 0;
    let mut expected_total = None;
    loop {
        let mut args = vec![
            "--output".to_owned(),
            "json".to_owned(),
            "list".to_owned(),
            date.to_string(),
        ];
        if let Some(next) = &continuation {
            args.push("--continue-from".to_owned());
            args.push(next.clone());
        }
        let page = drag_json(drag_bin, &args, None, false)?;
        pages += 1;
        assert_compatible_drag_page(&page, date)?;
        let items = page
            .get("worklogs")
            .or_else(|| page.get("results"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "missing worklogs/results array",
                )
            })?;
        for item in items {
            worklogs.push(normalize_worklog(item, date)?);
        }
        let page_total = page.get("total").and_then(Value::as_u64);
        if let Some(total) = page_total {
            if expected_total
                .replace(total)
                .is_some_and(|expected| expected != total)
            {
                return Err(reconcile_error(
                    ReconcileErrorKind::IncompleteRead,
                    "worklog total changed between continuation pages",
                ));
            }
        }
        let pagination = page.get("pagination");
        continuation = pagination
            .and_then(|value| value.get("next"))
            .or_else(|| page.get("continuation"))
            .or_else(|| page.get("next"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if continuation.is_none() {
            if pagination.is_some()
                && pagination
                    .and_then(|value| value.get("totalsComplete"))
                    .and_then(Value::as_bool)
                    != Some(true)
            {
                return Err(reconcile_error(
                    ReconcileErrorKind::IncompleteRead,
                    "Drag pagination ended before totals were complete",
                ));
            }
            break;
        }
        if !seen_continuations.insert(continuation.clone().unwrap_or_default()) {
            return Err(reconcile_error(
                ReconcileErrorKind::IncompleteRead,
                "Drag continuation cycle detected",
            ));
        }
        if pages > 128 {
            return Err(reconcile_error(
                ReconcileErrorKind::IncompleteRead,
                "page-bound exhaustion",
            ));
        }
    }
    if expected_total.is_some_and(|total| worklogs.len() as u64 != total) {
        return Err(reconcile_error(
            ReconcileErrorKind::IncompleteRead,
            "Drag returned an incomplete worklog total",
        ));
    }
    Ok(DragReadResult {
        status: "read",
        selected_date: date,
        pages,
        worklogs,
        network_access: true,
        live_mutation_allowed: false,
    })
}

pub(crate) fn verify_drag_contract(drag_bin: &Path) -> Result<(), CompanionError> {
    let schema = drag_json(
        drag_bin,
        &[
            "--output".to_owned(),
            "json".to_owned(),
            "schema".to_owned(),
        ],
        None,
        true,
    )?;
    let version = schema
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag schema response omitted schemaVersion",
            )
        })?;
    if version != u64::from(DRAG_MACHINE_CONTRACT_VERSION) {
        return Err(reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!(
                "unsupported Drag schemaVersion {version}; expected {DRAG_MACHINE_CONTRACT_VERSION}"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn audit_drag_day(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
    authorize_unattended: bool,
) -> Result<AuditResult, CompanionError> {
    ensure_proposal_drag_resolutions(data_dir, drag_bin, date)?;
    let read = read_drag_day(drag_bin, date)?;
    let proposals = proposal_payloads(data_dir, date, None)?;
    let policy_inputs = proposal_policy_inputs(data_dir, date)?;
    let mut duplicate_proposal_ids = Vec::new();
    let mut overlapping_proposal_ids = Vec::new();
    for (id, payload) in &proposals {
        let candidate = normalize_payload_worklog(payload, id)?;
        if read
            .worklogs
            .iter()
            .any(|existing| same_worklog(existing, &candidate))
        {
            duplicate_proposal_ids.push(id.clone());
        }
        if read.worklogs.iter().any(|existing| {
            overlaps(
                &existing.start,
                &existing.end,
                &candidate.start,
                &candidate.end,
            )
            .unwrap_or(false)
        }) {
            overlapping_proposal_ids.push(id.clone());
        }
    }
    duplicate_proposal_ids.sort();
    overlapping_proposal_ids.sort();
    let decisions = evaluate_policy_decisions(
        &policy_inputs,
        &read.worklogs,
        &duplicate_proposal_ids,
        &overlapping_proposal_ids,
        authorize_unattended,
    );
    persist_policy_decisions(data_dir, &decisions)?;
    let unsupported_periods = unsupported_period_decisions(data_dir, date)?;
    Ok(AuditResult {
        status: "audited",
        selected_date: date,
        existing_worklogs: read.worklogs,
        duplicate_proposal_ids,
        overlapping_proposal_ids,
        decisions,
        unsupported_periods,
        unattended_authorization: UnattendedAuthorization {
            required_for_approval: true,
            provided: authorize_unattended,
            mutation_allowed: false,
        },
        network_access: true,
        live_mutation_allowed: false,
    })
}

pub(crate) fn persist_policy_decisions(
    data_dir: &Path,
    decisions: &[PolicyDecision],
) -> Result<(), CompanionError> {
    let conn = Connection::open(store_path(data_dir))?;
    let tx = conn.unchecked_transaction()?;
    for decision in decisions {
        let decision_id = format!("policy.v{POLICY_SCHEMA_VERSION}.{}", decision.proposal_id);
        let reason_codes =
            serde_json::to_string(&decision.reason_codes).map_err(CompanionError::Serialize)?;
        let evidence_trace =
            serde_json::to_string(&decision.evidence_trace).map_err(CompanionError::Serialize)?;
        tx.execute(
            "INSERT INTO policy_decisions (id, proposal_id, decision, decided_at, reason_codes_json, evidence_trace_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO UPDATE SET decision = excluded.decision, decided_at = excluded.decided_at, reason_codes_json = excluded.reason_codes_json, evidence_trace_json = excluded.evidence_trace_json",
            params![
                decision_id,
                decision.proposal_id,
                decision.decision,
                now_string(),
                reason_codes,
                evidence_trace,
            ],
        )?;
        tx.execute(
            "UPDATE proposals SET state = ?1 WHERE id = ?2",
            params![decision.decision, decision.proposal_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct ProposalResolutionSource {
    pub(crate) id: String,
    pub(crate) issue_key: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) description_facts: Vec<String>,
}

pub(crate) fn ensure_proposal_drag_resolutions(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
) -> Result<(), CompanionError> {
    verify_drag_contract(drag_bin)?;
    let mut conn = Connection::open(store_path(data_dir))?;
    let proposals = proposal_resolution_sources(&conn, date)?;
    if proposals.is_empty() {
        return Ok(());
    }
    let configured_attributes = configured_tempo_work_attributes()?;
    let tx = conn.transaction()?;
    for proposal in proposals {
        if proposal_drag_resolution_complete(&tx, &proposal.id)? {
            continue;
        }
        let resolution = drag_json(
            drag_bin,
            &[
                "--output".to_owned(),
                "json".to_owned(),
                "resolve".to_owned(),
                "--issue-key".to_owned(),
                proposal.issue_key.clone(),
            ],
            None,
            true,
        )?;
        let resolved_issue = str_field(
            resolution.get("issue").ok_or_else(|| {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "Drag resolve omitted issue",
                )
            })?,
            &["key"],
        )?;
        let attributes = resolved_required_attribute_values(&resolution, &configured_attributes)?;
        let tempo_account_id = resolved_tempo_account_id(&resolution)?;
        let description = if proposal.description_facts.is_empty() {
            "Companion proposed worklog".to_owned()
        } else {
            proposal.description_facts.join("; ")
        };
        for (name, value) in [
            ("tempoAccountId", tempo_account_id),
            ("issueKey", resolved_issue),
            ("start", proposal.start),
            ("end", proposal.end),
            ("description", description),
            (
                "attributes",
                serde_json::to_string(&attributes).map_err(CompanionError::Serialize)?,
            ),
        ] {
            tx.execute(
                "INSERT INTO proposal_drag_resolutions (proposal_id, name, value) VALUES (?1, ?2, ?3) ON CONFLICT(proposal_id, name) DO UPDATE SET value = excluded.value",
                params![proposal.id, name, value],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn proposal_resolution_sources(
    conn: &Connection,
    date: NaiveDate,
) -> Result<Vec<ProposalResolutionSource>, CompanionError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, f.issue_key, f.supported_start, f.supported_end, f.description_facts_json FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id JOIN proposal_policy_fields f ON f.proposal_id = p.id WHERE b.explicit_date = ?1 ORDER BY p.id",
    )?;
    let rows = stmt.query_map([date.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut proposals = Vec::new();
    for row in rows {
        let (id, issue_key, start, end, description_facts_json) = row?;
        proposals.push(ProposalResolutionSource {
            id,
            issue_key,
            start,
            end,
            description_facts: serde_json::from_str(&description_facts_json)
                .map_err(CompanionError::Serialize)?,
        });
    }
    Ok(proposals)
}

pub(crate) fn proposal_drag_resolution_complete(
    conn: &Connection,
    proposal_id: &str,
) -> Result<bool, CompanionError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM proposal_drag_resolutions WHERE proposal_id = ?1 AND name IN ('tempoAccountId','issueKey','start','end','description','attributes') AND TRIM(value) != ''",
        [proposal_id],
        |row| row.get(0),
    )?;
    Ok(count == 6)
}

pub(crate) fn resolved_tempo_account_id(resolution: &Value) -> Result<String, CompanionError> {
    resolution
        .get("tempo")
        .and_then(|tempo| tempo.get("authenticatedAccountId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag resolve omitted authenticated Tempo account id",
            )
        })
}

pub(crate) fn proposal_tempo_account(
    conn: &Connection,
    proposal_id: &str,
) -> Result<String, CompanionError> {
    conn
        .query_row(
            "SELECT value FROM proposal_drag_resolutions WHERE proposal_id = ?1 AND name = 'tempoAccountId'",
            [proposal_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("proposal {proposal_id} missing authenticated Tempo account id"),
            )
        })
}

pub(crate) fn configured_tempo_work_attributes(
) -> Result<std::collections::BTreeMap<String, String>, CompanionError> {
    let Some(raw) = std::env::var_os(TEMPO_WORK_ATTRIBUTES_ENV) else {
        return Ok(std::collections::BTreeMap::new());
    };
    let raw = raw.into_string().map_err(|_| {
        reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!("{TEMPO_WORK_ATTRIBUTES_ENV} must be valid UTF-8 JSON"),
        )
    })?;
    let value: Value = serde_json::from_str(&raw).map_err(|error| {
        reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!("{TEMPO_WORK_ATTRIBUTES_ENV} must be a JSON object: {error}"),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!("{TEMPO_WORK_ATTRIBUTES_ENV} must be a JSON object"),
        )
    })?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(|value| (key.clone(), value.trim().to_owned()))
                .ok_or_else(|| {
                    reconcile_error(
                        ReconcileErrorKind::SchemaIncompatibility,
                        format!("{TEMPO_WORK_ATTRIBUTES_ENV}.{key} must be a non-empty string"),
                    )
                })
        })
        .collect()
}

pub(crate) fn resolved_required_attribute_values(
    resolution: &Value,
    configured_attributes: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, String>, CompanionError> {
    let required = resolution
        .get("tempo")
        .and_then(|tempo| tempo.get("requiredWorkAttributes"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag resolve omitted required Tempo work attributes",
            )
        })?;
    let mut attributes = std::collections::BTreeMap::new();
    for attribute in required {
        let key = str_field(attribute, &["key"])?;
        let Some(value) = configured_attributes.get(&key) else {
            return Err(reconcile_error(
                ReconcileErrorKind::IncompleteRead,
                format!(
                    "missing required Tempo work attribute {key}; set {TEMPO_WORK_ATTRIBUTES_ENV}"
                ),
            ));
        };
        attributes.insert(key, value.clone());
    }
    Ok(attributes)
}

pub(crate) fn evaluate_policy_decisions(
    proposals: &[ProposalPolicyInput],
    existing_worklogs: &[NormalizedWorklog],
    duplicate_ids: &[String],
    overlap_ids: &[String],
    authorize_unattended: bool,
) -> Vec<PolicyDecision> {
    let proposal_ids = proposals
        .iter()
        .map(|proposal| proposal.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    proposals
        .iter()
        .map(|proposal| {
            let mut reason_codes = Vec::new();
            let mut trace = proposal.evidence_refs.clone();
            trace.sort();
            trace.dedup();
            if proposal.evidence_refs.is_empty() {
                reason_codes.push("evidence.missing");
            }
            if proposal
                .evidence_refs
                .iter()
                .any(|reference| !reference.starts_with("evidence."))
            {
                reason_codes.push("evidence.provenance.unsupported");
            }
            if proposal.evidence_refs.len() != 1 {
                reason_codes.push("evidence.direct.single_issue_required");
            }
            if proposal.issue_key.trim().is_empty() || !proposal.issue_key.contains('-') {
                reason_codes.push("issue.verification.failed");
            }
            if proposal.description_facts.is_empty()
                || proposal.limitations.is_empty()
                || proposal.start.trim().is_empty()
                || proposal.end.trim().is_empty()
            {
                reason_codes.push("material_fields.missing");
            }
            if normalize_timestamp(&proposal.start).is_none()
                || normalize_timestamp(&proposal.end).is_none()
                || elapsed(&proposal.start, &proposal.end).is_none_or(|seconds| seconds <= 0)
            {
                reason_codes.push("supported_time.invalid");
            }
            if duplicate_ids.iter().any(|id| id == &proposal.id) {
                reason_codes.push("tempo.duplicate");
            }
            if overlap_ids.iter().any(|id| id == &proposal.id) {
                reason_codes.push("tempo.overlap");
            }
            if proposals.iter().any(|other| {
                other.id != proposal.id
                    && periods_overlap(&proposal.start, &proposal.end, &other.start, &other.end)
                        .unwrap_or(false)
            }) {
                reason_codes.push("proposal.overlap");
            }
            if proposals
                .iter()
                .filter(|other| other.issue_key == proposal.issue_key)
                .count()
                > 1
            {
                reason_codes.push("allocation.multiple_candidates");
            }
            if existing_worklogs
                .iter()
                .any(|worklog| worklog.issue_key == proposal.issue_key)
            {
                reason_codes.push("tempo.current_state.has_issue_worklog");
            }
            if proposal
                .limitations
                .iter()
                .chain(proposal.description_facts.iter())
                .any(|value| {
                    value.to_ascii_lowercase().contains("conflict")
                        || value.to_ascii_lowercase().contains("contradict")
                })
            {
                reason_codes.push("evidence.contradiction");
            }
            if !authorize_unattended {
                reason_codes.push("authorization.unattended.required");
            }
            reason_codes.sort();
            reason_codes.dedup();
            let decision =
                if !proposal_ids.contains(proposal.id.as_str()) || reason_codes.is_empty() {
                    "approved"
                } else if reason_codes
                    .iter()
                    .any(|code| code.starts_with("authorization."))
                {
                    "skipped"
                } else {
                    "rejected"
                };
            PolicyDecision {
                proposal_id: proposal.id.clone(),
                decision,
                reason_codes,
                evidence_trace: trace,
            }
        })
        .collect()
}

pub(crate) fn unsupported_period_decisions(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<Vec<UnsupportedPeriodDecision>, CompanionError> {
    let conn = Connection::open(store_path(data_dir))?;
    let mut stmt =
        conn.prepare("SELECT id FROM unsupported_periods WHERE explicit_date = ?1 ORDER BY id")?;
    let rows = stmt.query_map([date.to_string()], |row| row.get::<_, String>(0))?;
    let mut periods = Vec::new();
    for id in rows {
        periods.push(UnsupportedPeriodDecision {
            id: id?,
            decision: "skipped",
            reason_codes: vec![
                "unsupported_period.preserved",
                "required_time.informational",
            ],
            evidence_trace: Vec::new(),
        });
    }
    Ok(periods)
}

pub(crate) fn preview_drag_payload(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
    proposal_id: Option<&str>,
) -> Result<PreviewResult, CompanionError> {
    ensure_proposal_drag_resolutions(data_dir, drag_bin, date)?;
    let payloads = proposal_payloads(data_dir, date, proposal_id)?;
    let (_, payload) = payloads.into_iter().next().ok_or_else(|| {
        reconcile_error(
            ReconcileErrorKind::IncompleteRead,
            "no proposal payload available",
        )
    })?;
    let preview = drag_json(
        drag_bin,
        &[
            "--output".into(),
            "json".into(),
            "log".into(),
            "--json".into(),
            "-".into(),
            "--dry-run".into(),
        ],
        Some(&payload),
        true,
    )?;
    Ok(PreviewResult {
        status: "previewed",
        classification: "local-normalization",
        selected_date: date,
        payload,
        drag_preview: preview,
        network_access: true,
        live_mutation_allowed: false,
    })
}

pub(crate) fn drag_json(
    drag_bin: &Path,
    args: &[String],
    stdin_json: Option<&Value>,
    dry_run: bool,
) -> Result<Value, CompanionError> {
    let mut command = ProcessCommand::new(drag_bin);
    command.args(args);
    if stdin_json.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| {
        reconcile_error(
            ReconcileErrorKind::TransportAmbiguity,
            format!("failed to start Drag: {e}"),
        )
    })?;
    if let Some(payload) = stdin_json {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            reconcile_error(ReconcileErrorKind::TransportAmbiguity, "missing Drag stdin")
        })?;
        stdin
            .write_all(
                serde_json::to_string(payload)
                    .map_err(CompanionError::Serialize)?
                    .as_bytes(),
            )
            .map_err(|e| {
                reconcile_error(
                    ReconcileErrorKind::TransportAmbiguity,
                    format!("failed to write Drag stdin: {e}"),
                )
            })?;
    }
    let output = child.wait_with_output().map_err(|e| {
        reconcile_error(
            ReconcileErrorKind::TransportAmbiguity,
            format!("Drag transport failed: {e}"),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let kind = if dry_run || output.status.code() == Some(2) {
            ReconcileErrorKind::DefiniteFailure
        } else {
            ReconcileErrorKind::TransportAmbiguity
        };
        let message = redact(stderr.trim());
        return Err(reconcile_error(
            kind,
            if message.is_empty() {
                "Drag command failed with redacted diagnostics".to_owned()
            } else {
                message
            },
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!("invalid Drag JSON: {e}"),
        )
    })?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return value.get("data").cloned().ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag success envelope omitted data",
            )
        });
    }
    Ok(value)
}

pub(crate) fn assert_compatible_drag_page(
    page: &Value,
    date: NaiveDate,
) -> Result<(), CompanionError> {
    let legacy_schema = page
        .get("schemaVersion")
        .or_else(|| page.get("schema_version"))
        .and_then(Value::as_u64);
    if legacy_schema.is_some_and(|schema| schema != 1) {
        return Err(reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!(
                "unsupported legacy page schemaVersion {}",
                legacy_schema.unwrap_or_default()
            ),
        ));
    }
    let selected = page
        .get("pagination")
        .and_then(|pagination| pagination.get("selectedDate"))
        .or_else(|| page.get("selectedDate"))
        .or_else(|| page.get("date"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "missing selected date",
            )
        })?;
    if selected != date.to_string() {
        return Err(reconcile_error(
            ReconcileErrorKind::IncompleteRead,
            "continuation/date mismatch",
        ));
    }
    if page.get("partial").and_then(Value::as_bool) == Some(true) {
        return Err(reconcile_error(
            ReconcileErrorKind::IncompleteRead,
            "partial output",
        ));
    }
    Ok(())
}

pub(crate) fn normalize_worklog(
    item: &Value,
    selected_date: NaiveDate,
) -> Result<NormalizedWorklog, CompanionError> {
    let id = str_field(item, &["tempoWorklogId", "id"])?;
    let issue_key = str_field(item, &["issueKey", "issue"])?;
    let (start, end) = if let Some(interval) = item.get("interval") {
        let start_time = str_field(interval, &["startTime"])?;
        let end_time = str_field(interval, &["endTime"])?;
        canonical_wall_interval(selected_date, &start_time, &end_time)?
    } else if item.get("durationOrInterval").is_some() {
        normalize_drag_log_input(item)?
    } else {
        let start = normalize_timestamp(&str_field(item, &["start", "started", "intervalStart"])?)
            .ok_or_else(|| {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "invalid worklog start",
                )
            })?;
        let end =
            normalize_timestamp(&str_field(item, &["end", "intervalEnd"])?).ok_or_else(|| {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "invalid worklog end",
                )
            })?;
        (start, end)
    };
    validate_period(&start, &end)
        .map_err(|message| reconcile_error(ReconcileErrorKind::SchemaIncompatibility, message))?;
    let description = item
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let attributes = normalize_attributes(item.get("attributes"))?;
    Ok(NormalizedWorklog {
        tempo_worklog_id: id,
        issue_key,
        start,
        end,
        description,
        attributes,
    })
}

pub(crate) fn normalize_payload_worklog(
    payload: &Value,
    id: &str,
) -> Result<NormalizedWorklog, CompanionError> {
    let (start, end) = if payload.get("durationOrInterval").is_some() {
        normalize_drag_log_input(payload)?
    } else {
        let start = normalize_timestamp(&str_field(payload, &["start", "intervalStart"])?)
            .ok_or_else(|| {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "invalid payload start",
                )
            })?;
        let end = normalize_timestamp(&str_field(payload, &["end", "intervalEnd"])?).ok_or_else(
            || {
                reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    "invalid payload end",
                )
            },
        )?;
        (start, end)
    };
    validate_period(&start, &end)
        .map_err(|message| reconcile_error(ReconcileErrorKind::SchemaIncompatibility, message))?;
    Ok(NormalizedWorklog {
        tempo_worklog_id: id.to_owned(),
        issue_key: str_field(payload, &["issueKey"])?,
        start,
        end,
        description: payload
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        attributes: normalize_attributes(payload.get("attributes"))?,
    })
}

pub(crate) fn normalize_attributes(
    value: Option<&Value>,
) -> Result<std::collections::BTreeMap<String, String>, CompanionError> {
    let Some(value) = value else {
        return Ok(std::collections::BTreeMap::new());
    };
    if value.is_null() {
        return Ok(std::collections::BTreeMap::new());
    }
    if let Some(attributes) = value.as_object() {
        return attributes
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.trim().to_owned()))
                    .ok_or_else(|| {
                        reconcile_error(
                            ReconcileErrorKind::SchemaIncompatibility,
                            format!("attribute {key} must be a string"),
                        )
                    })
            })
            .collect();
    }
    if let Some(attributes) = value.as_array() {
        let mut normalized = std::collections::BTreeMap::new();
        for attribute in attributes {
            let key = str_field(attribute, &["key"])?;
            let value = str_field(attribute, &["value"])?;
            if normalized
                .insert(key.clone(), value.trim().to_owned())
                .is_some()
            {
                return Err(reconcile_error(
                    ReconcileErrorKind::SchemaIncompatibility,
                    format!("duplicate attribute {key}"),
                ));
            }
        }
        return Ok(normalized);
    }
    Err(reconcile_error(
        ReconcileErrorKind::SchemaIncompatibility,
        "attributes must be an object or key/value array",
    ))
}

pub(crate) fn proposal_payloads(
    data_dir: &Path,
    date: NaiveDate,
    only: Option<&str>,
) -> Result<Vec<(String, Value)>, CompanionError> {
    Ok(proposal_payload_records(data_dir, date, only)?
        .into_iter()
        .map(|record| (record.proposal_id, record.payload))
        .collect())
}

#[derive(Debug)]
pub(crate) struct ProposalPayloadRecord {
    pub(crate) proposal_id: String,
    pub(crate) tempo_account: String,
    pub(crate) payload: Value,
}

pub(crate) fn proposal_payload_records(
    data_dir: &Path,
    date: NaiveDate,
    only: Option<&str>,
) -> Result<Vec<ProposalPayloadRecord>, CompanionError> {
    let conn = Connection::open(store_path(data_dir))?;
    let mut stmt = conn.prepare("SELECT p.id FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1 ORDER BY p.id")?;
    let ids = stmt.query_map([date.to_string()], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for id in ids {
        let id = id?;
        if only.is_some_and(|wanted| wanted != id) {
            continue;
        }
        let tempo_account = proposal_tempo_account(&conn, &id)?;
        let issue = resolve_drag_required_text(&conn, &id, "issueKey")?;
        let start = resolve_drag_required_text(&conn, &id, "start")?;
        let end = resolve_drag_required_text(&conn, &id, "end")?;
        let description = resolve_drag_required_text(&conn, &id, "description")?;
        let attributes: Value =
            serde_json::from_str(&resolve_drag_required_text(&conn, &id, "attributes")?).map_err(
                |error| {
                    reconcile_error(
                        ReconcileErrorKind::SchemaIncompatibility,
                        format!("invalid resolved attributes for {id}: {error}"),
                    )
                },
            )?;
        let start = DateTime::parse_from_rfc3339(&start).map_err(|_| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("invalid resolved start for {id}"),
            )
        })?;
        let end = DateTime::parse_from_rfc3339(&end).map_err(|_| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("invalid resolved end for {id}"),
            )
        })?;
        let duration_seconds = end.signed_duration_since(start).num_seconds();
        if duration_seconds <= 0 || duration_seconds % 60 != 0 {
            return Err(reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("resolved interval for {id} must be positive and minute-aligned"),
            ));
        }
        if start.date_naive() != date
            || start.time().second() != 0
            || start.time().nanosecond() != 0
        {
            return Err(reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!(
                    "resolved start for {id} must use the selected local date and minute precision"
                ),
            ));
        }
        out.push(ProposalPayloadRecord {
            proposal_id: id,
            tempo_account,
            payload: serde_json::json!({
                "issueKey": issue,
                "durationOrInterval": format!("{}m", duration_seconds / 60),
                "when": date,
                "start": start.format("%H:%M").to_string(),
                "description": description,
                "attributes": attributes,
            }),
        });
    }
    Ok(out)
}

pub(crate) fn normalize_drag_log_input(
    payload: &Value,
) -> Result<(String, String), CompanionError> {
    let date =
        NaiveDate::parse_from_str(&str_field(payload, &["when"])?, "%Y-%m-%d").map_err(|_| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "invalid Drag log input date",
            )
        })?;
    let start = parse_clock(&str_field(payload, &["start"])?)?;
    let duration = str_field(payload, &["durationOrInterval"])?;
    let minutes = duration
        .strip_suffix('m')
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag log input duration must be positive whole minutes",
            )
        })?;
    let start = NaiveDateTime::new(date, start);
    let end = start
        .checked_add_signed(Duration::minutes(minutes))
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "Drag log input interval overflow",
            )
        })?;
    Ok((
        canonical_wall_timestamp(start),
        canonical_wall_timestamp(end),
    ))
}

pub(crate) fn canonical_wall_interval(
    date: NaiveDate,
    start: &str,
    end: &str,
) -> Result<(String, String), CompanionError> {
    let start = NaiveDateTime::new(date, parse_clock(start)?);
    let mut end = NaiveDateTime::new(date, parse_clock(end)?);
    if end <= start {
        end = end.checked_add_signed(Duration::days(1)).ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                "worklog interval overflow",
            )
        })?;
    }
    Ok((
        canonical_wall_timestamp(start),
        canonical_wall_timestamp(end),
    ))
}

pub(crate) fn parse_clock(raw: &str) -> Result<chrono::NaiveTime, CompanionError> {
    ["%H:%M:%S", "%H:%M"]
        .into_iter()
        .find_map(|format| chrono::NaiveTime::parse_from_str(raw, format).ok())
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("invalid worklog clock time {raw}"),
            )
        })
}

pub(crate) fn canonical_wall_timestamp(value: NaiveDateTime) -> String {
    format!("{}Z", value.format("%Y-%m-%dT%H:%M:%S"))
}

pub(crate) fn proposal_policy_inputs(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<Vec<ProposalPolicyInput>, CompanionError> {
    let conn = Connection::open(store_path(data_dir))?;
    let mut stmt = conn.prepare(
        "SELECT p.id, f.evidence_refs_json, f.issue_key, f.supported_start, f.supported_end, f.description_facts_json, f.limitations_json FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id JOIN proposal_policy_fields f ON f.proposal_id = p.id WHERE b.explicit_date = ?1 ORDER BY p.id",
    )?;
    let rows = stmt.query_map([date.to_string()], |row| {
        let evidence_refs_json: String = row.get(1)?;
        let description_facts_json: String = row.get(5)?;
        let limitations_json: String = row.get(6)?;
        Ok((
            row.get::<_, String>(0)?,
            evidence_refs_json,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            description_facts_json,
            limitations_json,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            evidence_refs_json,
            issue_key,
            start,
            end,
            description_facts_json,
            limitations_json,
        ) = row?;
        out.push(ProposalPolicyInput {
            id,
            evidence_refs: serde_json::from_str(&evidence_refs_json)
                .map_err(CompanionError::Serialize)?,
            issue_key,
            start,
            end,
            description_facts: serde_json::from_str(&description_facts_json)
                .map_err(CompanionError::Serialize)?,
            limitations: serde_json::from_str(&limitations_json)
                .map_err(CompanionError::Serialize)?,
        });
    }
    Ok(out)
}

pub(crate) fn resolve_drag_required_text(
    conn: &Connection,
    proposal: &str,
    name: &str,
) -> Result<String, CompanionError> {
    let mut stmt = conn.prepare(
        "SELECT value FROM proposal_drag_resolutions WHERE proposal_id = ?1 AND name = ?2",
    )?;
    stmt.query_row(params![proposal, name], |row| row.get::<_, String>(0))
        .optional()?
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::IncompleteRead,
                format!("missing Drag-resolved {name} for {proposal}"),
            )
        })
}

pub(crate) fn str_field(item: &Value, names: &[&str]) -> Result<String, CompanionError> {
    names
        .iter()
        .find_map(|name| item.get(*name).and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            reconcile_error(
                ReconcileErrorKind::SchemaIncompatibility,
                format!("missing {}", names[0]),
            )
        })
}

pub(crate) fn same_worklog(a: &NormalizedWorklog, b: &NormalizedWorklog) -> bool {
    a.issue_key == b.issue_key
        && a.start == b.start
        && a.end == b.end
        && a.description == b.description
        && a.attributes == b.attributes
}

pub(crate) fn overlaps(
    a_start: &str,
    a_end: &str,
    b_start: &str,
    b_end: &str,
) -> Result<bool, String> {
    periods_overlap(a_start, a_end, b_start, b_end)
}

pub(crate) fn reconcile_error(
    kind: ReconcileErrorKind,
    message: impl Into<String>,
) -> CompanionError {
    CompanionError::DragReconcile {
        kind,
        message: message.into(),
    }
}

pub(crate) fn sha256_json(bytes: &[u8]) -> Result<String, CompanionError> {
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
pub(crate) fn sha256_str(raw: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(raw.as_bytes()))
}

pub(crate) fn normalize_timestamp(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw).ok().map(|timestamp| {
        timestamp
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

pub(crate) fn elapsed(start: &str, end: &str) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(start).ok()?;
    let end = DateTime::parse_from_rfc3339(end).ok()?;
    Some((end - start).num_seconds())
}

pub(crate) fn redact(raw: &str) -> String {
    let words = raw.split_whitespace().collect::<Vec<_>>();
    let mut safe = Vec::new();
    let mut skip_next = false;
    for word in words {
        if skip_next {
            skip_next = false;
            continue;
        }
        let lower = word
            .trim_matches(|character: char| {
                matches!(
                    character,
                    '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
            .to_ascii_lowercase();
        let secret_label = [
            "token",
            "password",
            "passwd",
            "api_key",
            "api-key",
            "apikey",
            "authorization",
            "client_secret",
            "access_token",
            "refresh_token",
        ]
        .iter()
        .find(|label| lower.starts_with(**label));
        if secret_label.is_some() {
            skip_next = lower.ends_with(':') || lower.ends_with('=');
            continue;
        }
        if lower == "bearer" {
            skip_next = true;
            continue;
        }
        if lower.starts_with("bearer")
            || lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("akia")
            || lower.contains("secret")
            || lower.contains("/home/")
            || lower.contains("/users/")
            || lower.contains("\\users\\")
            || lower.contains("transcript")
            || lower.contains("ignore")
            || lower.contains("instruction")
        {
            continue;
        }
        safe.push(word);
    }
    safe.join(" ")
}

pub(crate) fn minimized_reference(reference: &str) -> String {
    format!("local-reference:{}", sha256_str(reference))
}
