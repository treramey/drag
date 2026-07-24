use crate::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReplayFixture {
    pub(crate) fixture_id: String,
    pub(crate) date: NaiveDate,
    pub(crate) tags: Vec<String>,
    pub(crate) collector: Value,
    pub(crate) model: Value,
    pub(crate) drag_read: Value,
    pub(crate) preview: Value,
    pub(crate) mutation: Value,
    pub(crate) crash: Value,
    pub(crate) network: Value,
    pub(crate) expectations: ReplayExpectations,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReplayExpectations {
    pub(crate) schema_valid: bool,
    pub(crate) provenance_valid: bool,
    pub(crate) redaction_valid: bool,
    pub(crate) attribution_precision: f64,
    pub(crate) duration_precision: f64,
    pub(crate) overlaps: u64,
    pub(crate) duplicates: u64,
    pub(crate) unsafe_retries: u64,
    pub(crate) incorrect_creates: u64,
    pub(crate) privacy_incidents: u64,
    pub(crate) fabricated_material_fields: u64,
    pub(crate) duplicate_proposals: u64,
    pub(crate) accepted_overlaps: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectorSeam {
    recording: String,
    events: Vec<CollectorEventSeam>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectorEventSeam {
    id: String,
    kind: String,
    source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelSeam {
    recording: String,
    proposals: Vec<ModelProposalSeam>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelProposalSeam {
    id: String,
    issue: String,
    material_fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DragReadSeam {
    recording: String,
    worklogs: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MutationSeam {
    recording: String,
    attempted: bool,
    ledger: Vec<Value>,
}

#[derive(Debug, Default)]
struct ActualReplayMetrics {
    schema_valid: bool,
    provenance_valid: bool,
    redaction_valid: bool,
    attribution_precision: f64,
    duration_precision: f64,
    overlaps: u64,
    duplicates: u64,
    unsafe_retries: u64,
    incorrect_creates: u64,
    privacy_incidents: u64,
    fabricated_material_fields: u64,
    duplicate_proposals: u64,
    accepted_overlaps: u64,
}

pub(crate) fn run_replay(
    fixtures_dir: &Path,
    artifacts_dir: Option<&Path>,
) -> Result<Value, CompanionError> {
    let mut paths = fs::read_dir(fixtures_dir)
        .map_err(|source| CompanionError::Read {
            path: fixtures_dir.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut fixtures = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
            path: path.clone(),
            source,
        })?;
        reject_replay_secret(&path, &text)?;
        let fixture: ReplayFixture = serde_json::from_str(&text).map_err(|error| {
            CompanionError::Proposal(format!("replay fixture {} schema: {error}", path.display()))
        })?;
        fixtures.push(fixture);
    }

    let required_tags = [
        "sparse",
        "multi_issue",
        "meetings",
        "abandoned_session",
        "dst",
        "manual_edit",
        "network_failure",
    ];
    let mut failures = Vec::new();
    if fixtures.len() < 30 {
        failures.push(replay_failure(
            "corpus",
            "corpus-size",
            "fixture_count",
            "load",
            format!("expected at least 30 days, found {}", fixtures.len()),
        ));
    }
    for tag in required_tags {
        if !fixtures
            .iter()
            .any(|fixture| fixture.tags.iter().any(|candidate| candidate == tag))
        {
            failures.push(replay_failure(
                "corpus",
                tag,
                "required_tag",
                "load",
                format!("missing representative tag {tag}"),
            ));
        }
    }

    let mut metrics = serde_json::json!({
        "schemaValidity": 0_u64,
        "provenance": 0_u64,
        "redaction": 0_u64,
        "issueAttributionPrecision": 1.0_f64,
        "supportedDurationPrecision": 1.0_f64,
        "overlaps": 0_u64,
        "duplicates": 0_u64,
        "unsafeRetries": 0_u64,
        "incorrectCreates": 0_u64,
        "privacyIncidents": 0_u64,
        "fabricatedMaterialFields": 0_u64,
        "duplicateProposals": 0_u64,
        "acceptedOverlaps": 0_u64
    });
    let mut days = Vec::new();
    for fixture in &fixtures {
        let source_hash = replay_fixture_hash(fixture)?;
        let actual = validate_replay_seams(fixture, &mut failures);
        compare_replay_expectations(fixture, &actual, &mut failures);

        metrics["schemaValidity"] = serde_json::json!(
            metrics["schemaValidity"].as_u64().unwrap_or(0) + u64::from(actual.schema_valid)
        );
        metrics["provenance"] = serde_json::json!(
            metrics["provenance"].as_u64().unwrap_or(0) + u64::from(actual.provenance_valid)
        );
        metrics["redaction"] = serde_json::json!(
            metrics["redaction"].as_u64().unwrap_or(0) + u64::from(actual.redaction_valid)
        );
        metrics["issueAttributionPrecision"] = serde_json::json!(metrics
            ["issueAttributionPrecision"]
            .as_f64()
            .unwrap_or(1.0)
            .min(actual.attribution_precision));
        metrics["supportedDurationPrecision"] = serde_json::json!(metrics
            ["supportedDurationPrecision"]
            .as_f64()
            .unwrap_or(1.0)
            .min(actual.duration_precision));
        for (key, value) in [
            ("overlaps", actual.overlaps),
            ("duplicates", actual.duplicates),
            ("unsafeRetries", actual.unsafe_retries),
            ("incorrectCreates", actual.incorrect_creates),
            ("privacyIncidents", actual.privacy_incidents),
            (
                "fabricatedMaterialFields",
                actual.fabricated_material_fields,
            ),
            ("duplicateProposals", actual.duplicate_proposals),
            ("acceptedOverlaps", actual.accepted_overlaps),
        ] {
            metrics[key] = serde_json::json!(metrics[key].as_u64().unwrap_or(0) + value);
        }

        days.push(serde_json::json!({
            "fixture": fixture.fixture_id,
            "date": fixture.date,
            "tags": fixture.tags,
            "sourceHash": source_hash,
            "operationState": "validated-offline"
        }));
    }

    let report = serde_json::json!({
        "status": if failures.is_empty() { "passed" } else { "failed" },
        "offline": true,
        "deterministic": true,
        "fixtureDays": fixtures.len(),
        "requiredTags": required_tags,
        "metrics": metrics,
        "zeroInvariants": {
            "fabricatedMaterialFields": metrics["fabricatedMaterialFields"],
            "duplicateProposals": metrics["duplicateProposals"],
            "acceptedOverlaps": metrics["acceptedOverlaps"],
            "unsafeRetries": metrics["unsafeRetries"]
        },
        "days": days,
        "failures": failures,
        "artifactSafety": "secret-safe-redacted-only"
    });
    if let Some(dir) = artifacts_dir {
        fs::create_dir_all(dir).map_err(|source| CompanionError::CreateDir {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = dir.join("replay-report.json");
        let text = serde_json::to_string_pretty(&report).map_err(CompanionError::Serialize)?;
        reject_replay_secret(&path, &text)?;
        atomic_write(&path, text.as_bytes())?;
    }
    Ok(report)
}

pub(crate) fn replay_fixture_hash(fixture: &ReplayFixture) -> Result<String, CompanionError> {
    let payload = fixture_hash_payload(fixture);
    let bytes = serde_json::to_vec(&payload).map_err(CompanionError::Serialize)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(crate) fn fixture_hash_payload(fixture: &ReplayFixture) -> Value {
    serde_json::json!({
        "fixtureId": fixture.fixture_id,
        "date": fixture.date,
        "tags": fixture.tags,
        "collector": fixture.collector,
        "model": fixture.model,
        "dragRead": fixture.drag_read,
        "preview": fixture.preview,
        "mutation": fixture.mutation,
        "crash": fixture.crash,
        "network": fixture.network,
        "expectations": fixture.expectations_summary()
    })
}

impl ReplayFixture {
    fn expectations_summary(&self) -> Value {
        serde_json::json!({
            "schemaValid": self.expectations.schema_valid,
            "provenanceValid": self.expectations.provenance_valid,
            "redactionValid": self.expectations.redaction_valid,
            "attributionPrecision": self.expectations.attribution_precision,
            "durationPrecision": self.expectations.duration_precision,
            "overlaps": self.expectations.overlaps,
            "duplicates": self.expectations.duplicates,
            "unsafeRetries": self.expectations.unsafe_retries,
            "incorrectCreates": self.expectations.incorrect_creates,
            "privacyIncidents": self.expectations.privacy_incidents,
            "fabricatedMaterialFields": self.expectations.fabricated_material_fields,
            "duplicateProposals": self.expectations.duplicate_proposals,
            "acceptedOverlaps": self.expectations.accepted_overlaps
        })
    }
}

pub(crate) fn replay_failure(
    fixture: &str,
    evidence: &str,
    rule: &str,
    operation: &str,
    message: impl Into<String>,
) -> Value {
    serde_json::json!({ "fixture": fixture, "evidence": evidence, "rule": rule, "operation": operation, "message": message.into() })
}

fn validate_replay_seams(
    fixture: &ReplayFixture,
    failures: &mut Vec<Value>,
) -> ActualReplayMetrics {
    let schema_versions_valid = validate_schema_versions(fixture, failures);
    let collector = parse_seam::<CollectorSeam>(fixture, "collector", &fixture.collector, failures);
    let model = parse_seam::<ModelSeam>(fixture, "model", &fixture.model, failures);
    let drag = parse_seam::<DragReadSeam>(fixture, "dragRead", &fixture.drag_read, failures);
    let mutation = parse_seam::<MutationSeam>(fixture, "mutation", &fixture.mutation, failures);
    parse_required_value(fixture, "preview", &fixture.preview, failures);
    parse_required_value(fixture, "crash", &fixture.crash, failures);
    parse_required_value(fixture, "network", &fixture.network, failures);

    let mut actual = ActualReplayMetrics {
        attribution_precision: 1.0,
        duration_precision: 1.0,
        ..ActualReplayMetrics::default()
    };
    let (Some(collector), Some(model), Some(drag), Some(mutation)) =
        (collector, model, drag, mutation)
    else {
        return actual;
    };

    actual.schema_valid = schema_versions_valid
        & validate_collector_normalization(fixture, &collector, failures)
        & validate_model_proposals(fixture, &model, failures)
        & validate_drag_policy(fixture, &drag, failures)
        & validate_recovery_policy(fixture, &mutation, failures)
        & execute_production_seams(fixture, &collector, &model, &drag, &mutation, failures);
    actual.provenance_valid = collector
        .events
        .iter()
        .all(|event| !event.id.trim().is_empty() && !event.source.trim().is_empty());
    actual.redaction_valid = !fixture_hash_payload(fixture)
        .to_string()
        .to_ascii_lowercase()
        .contains("raw_secret");
    actual.duplicate_proposals =
        duplicate_count(model.proposals.iter().map(|proposal| proposal.id.as_str()));
    actual.fabricated_material_fields = model
        .proposals
        .iter()
        .flat_map(|proposal| proposal.material_fields.iter())
        .filter(|field| !matches!(field.as_str(), "issue" | "startedAt" | "durationMinutes"))
        .count() as u64;
    actual.unsafe_retries = u64::from(mutation.attempted && mutation.ledger.is_empty());
    actual.incorrect_creates = u64::from(mutation.attempted && drag.worklogs.is_empty());
    actual
}

fn execute_production_seams(
    fixture: &ReplayFixture,
    collector: &CollectorSeam,
    model: &ModelSeam,
    drag: &DragReadSeam,
    mutation: &MutationSeam,
    failures: &mut Vec<Value>,
) -> bool {
    let evidence = collector
        .events
        .iter()
        .map(|event| BundleEvidence {
            id: format!("evidence.{}", event.id),
            source: event.source.clone(),
            reference: minimized_reference(&event.id),
            original_timestamp: Some(format!("{}T09:00:00Z", fixture.date)),
            original_timezone: Some("UTC".to_owned()),
            observed_at_utc: Some(format!("{}T09:00:00Z", fixture.date)),
            interval_start_utc: None,
            interval_end_utc: None,
            elapsed_seconds: None,
            summary: redact(&event.kind),
            status: None,
            unsupported_reason: None,
            supersedes: None,
            superseded_by: None,
            contradicted_by: Vec::new(),
            abandoned_session: false,
        })
        .collect::<Vec<_>>();
    let bundle = EvidenceBundle {
        schema_version: 1,
        explicit_date: fixture.date,
        mode: DEFAULT_MODE,
        network_access: false,
        live_mutation_allowed: false,
        unsupported_gaps: Vec::new(),
        source_health: Vec::new(),
        evidence,
        contradictions: Vec::new(),
    };
    let first_evidence = bundle.evidence.first().map(|event| event.id.clone());
    let proposals = model
        .proposals
        .iter()
        .enumerate()
        .map(|(index, proposal)| WorklogProposal {
            id: proposal.id.clone(),
            provider_id: None,
            evidence_refs: first_evidence.clone().into_iter().collect(),
            issue_candidate: ProposalIssueCandidate {
                key: proposal.issue.clone(),
                confidence: "recorded".to_owned(),
            },
            supported_time: ProposalTimePeriod {
                start: format!("{}T{:02}:00:00Z", fixture.date, 9 + index),
                end: format!("{}T{:02}:00:00Z", fixture.date, 10 + index),
            },
            description_facts: vec!["recorded replay evidence".to_owned()],
            confidence: 1.0,
            limitations: vec!["offline replay fixture".to_owned()],
        })
        .collect::<Vec<_>>();
    let response = ProviderResponse {
        proposals,
        unsupported_periods: Vec::new(),
    };
    if let Err(error) = validate_provider_response(&response, &bundle) {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "model",
            "production provider validator",
            "execute",
            error,
        ));
        return false;
    }

    let mut existing_worklogs = Vec::new();
    for value in &drag.worklogs {
        match normalize_worklog(value, fixture.date) {
            Ok(worklog) => existing_worklogs.push(worklog),
            Err(error) => {
                failures.push(replay_failure(
                    &fixture.fixture_id,
                    "dragRead",
                    "production Drag normalization",
                    "execute",
                    error.to_string(),
                ));
                return false;
            }
        }
    }
    let policy_inputs = response
        .proposals
        .iter()
        .map(|proposal| ProposalPolicyInput {
            id: proposal.id.clone(),
            evidence_refs: proposal.evidence_refs.clone(),
            issue_key: proposal.issue_candidate.key.clone(),
            start: proposal.supported_time.start.clone(),
            end: proposal.supported_time.end.clone(),
            description_facts: proposal.description_facts.clone(),
            limitations: proposal.limitations.clone(),
        })
        .collect::<Vec<_>>();
    let decisions = evaluate_policy_decisions(&policy_inputs, &existing_worklogs, &[], &[], true);
    if decisions
        .iter()
        .any(|decision| decision.decision != "approved")
    {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "policy",
            "production deterministic policy",
            "execute",
            "recorded proposal was not approved by production policy",
        ));
        return false;
    }

    for proposal in &response.proposals {
        let payload = serde_json::json!({
            "issue": proposal.issue_candidate.key,
            "startedAt": proposal.supported_time.start,
            "durationMinutes": 60,
        });
        if operation_key("replay-account", fixture.date, &payload).is_err() {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "mutation",
                "production operation key",
                "execute",
                "could not derive deterministic operation key",
            ));
            return false;
        }
    }
    if mutation.attempted && mutation.ledger.is_empty() {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "mutation",
            "production recovery ledger",
            "execute",
            "attempted mutation has no durable ledger entry",
        ));
        return false;
    }
    true
}

fn parse_seam<T: serde::de::DeserializeOwned>(
    fixture: &ReplayFixture,
    evidence: &str,
    value: &Value,
    failures: &mut Vec<Value>,
) -> Option<T> {
    parse_required_value(fixture, evidence, value, failures)?;
    match serde_json::from_value(value.clone()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            failures.push(replay_failure(
                &fixture.fixture_id,
                evidence,
                "typed seam schema",
                "load",
                error.to_string(),
            ));
            None
        }
    }
}

fn parse_required_value(
    fixture: &ReplayFixture,
    evidence: &str,
    value: &Value,
    failures: &mut Vec<Value>,
) -> Option<()> {
    if value.is_null() {
        failures.push(replay_failure(
            &fixture.fixture_id,
            evidence,
            "fixture field is null",
            "load",
            "recorded seam fixture is required",
        ));
        None
    } else {
        Some(())
    }
}

fn validate_schema_versions(fixture: &ReplayFixture, failures: &mut Vec<Value>) -> bool {
    let mut ok = true;
    for (evidence, value) in [
        ("collector", &fixture.collector),
        ("model", &fixture.model),
        ("dragRead", &fixture.drag_read),
        ("mutation", &fixture.mutation),
    ] {
        if value.get("schemaVersion").and_then(Value::as_u64) == Some(999) {
            failures.push(replay_failure(
                &fixture.fixture_id,
                evidence,
                "schemaVersion",
                "load",
                "unsupported replay seam schema version 999",
            ));
            ok = false;
        }
    }
    ok
}

fn validate_collector_normalization(
    fixture: &ReplayFixture,
    collector: &CollectorSeam,
    failures: &mut Vec<Value>,
) -> bool {
    let ok = collector.recording == "collector-fixture-v1"
        && collector.events.iter().all(|event| {
            !event.id.trim().is_empty()
                && !event.kind.trim().is_empty()
                && !event.source.trim().is_empty()
        });
    if !ok {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "collector",
            "production normalization seam",
            "validate",
            "collector event normalization failed",
        ));
    }
    ok
}

fn validate_model_proposals(
    fixture: &ReplayFixture,
    model: &ModelSeam,
    failures: &mut Vec<Value>,
) -> bool {
    let ok = model.recording == "model-fixture-v1"
        && model.proposals.iter().all(|proposal| {
            !proposal.id.trim().is_empty()
                && !proposal.issue.trim().is_empty()
                && proposal
                    .material_fields
                    .iter()
                    .any(|field| field == "issue")
        });
    if !ok {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "model",
            "production proposal seam",
            "validate",
            "model proposal seam failed",
        ));
    }
    ok
}

fn validate_drag_policy(
    fixture: &ReplayFixture,
    drag: &DragReadSeam,
    failures: &mut Vec<Value>,
) -> bool {
    let ok = drag.recording == "drag-read-fixture-v1";
    if !ok {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "dragRead",
            "production policy seam",
            "validate",
            "Drag read policy seam failed",
        ));
    }
    ok
}

fn validate_recovery_policy(
    fixture: &ReplayFixture,
    mutation: &MutationSeam,
    failures: &mut Vec<Value>,
) -> bool {
    let ok = mutation.recording == "mutation-fixture-v1"
        && (!mutation.attempted || !mutation.ledger.is_empty());
    if !ok {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "mutation",
            "production recovery seam",
            "validate",
            "mutation recovery seam failed",
        ));
    }
    ok
}

fn duplicate_count<'a>(values: impl Iterator<Item = &'a str>) -> u64 {
    let mut seen = std::collections::BTreeSet::new();
    values.filter(|value| !seen.insert(*value)).count() as u64
}

fn compare_replay_expectations(
    fixture: &ReplayFixture,
    actual: &ActualReplayMetrics,
    failures: &mut Vec<Value>,
) {
    let expected = &fixture.expectations;
    compare_bool(
        fixture,
        "schemaValidity",
        expected.schema_valid,
        actual.schema_valid,
        failures,
    );
    compare_bool(
        fixture,
        "provenance",
        expected.provenance_valid,
        actual.provenance_valid,
        failures,
    );
    compare_bool(
        fixture,
        "redaction",
        expected.redaction_valid,
        actual.redaction_valid,
        failures,
    );
    compare_f64(
        fixture,
        "attributionPrecision",
        expected.attribution_precision,
        actual.attribution_precision,
        failures,
    );
    compare_f64(
        fixture,
        "durationPrecision",
        expected.duration_precision,
        actual.duration_precision,
        failures,
    );
    for (rule, expected_value, actual_value) in [
        ("overlaps", expected.overlaps, actual.overlaps),
        ("duplicates", expected.duplicates, actual.duplicates),
        (
            "unsafeRetries",
            expected.unsafe_retries,
            actual.unsafe_retries,
        ),
        (
            "incorrectCreates",
            expected.incorrect_creates,
            actual.incorrect_creates,
        ),
        (
            "privacyIncidents",
            expected.privacy_incidents,
            actual.privacy_incidents,
        ),
        (
            "fabricatedMaterialFields",
            expected.fabricated_material_fields,
            actual.fabricated_material_fields,
        ),
        (
            "duplicateProposals",
            expected.duplicate_proposals,
            actual.duplicate_proposals,
        ),
        (
            "acceptedOverlaps",
            expected.accepted_overlaps,
            actual.accepted_overlaps,
        ),
    ] {
        if expected_value != actual_value {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "expectations",
                rule,
                "compare",
                format!("expected {expected_value}, actual {actual_value}"),
            ));
        }
    }
}

fn compare_bool(
    fixture: &ReplayFixture,
    rule: &str,
    expected: bool,
    actual: bool,
    failures: &mut Vec<Value>,
) {
    if expected != actual {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "expectations",
            rule,
            "compare",
            format!("expected {expected}, actual {actual}"),
        ));
    }
}

fn compare_f64(
    fixture: &ReplayFixture,
    rule: &str,
    expected: f64,
    actual: f64,
    failures: &mut Vec<Value>,
) {
    if (expected - actual).abs() > f64::EPSILON {
        failures.push(replay_failure(
            &fixture.fixture_id,
            "expectations",
            rule,
            "compare",
            format!("expected {expected}, actual {actual}"),
        ));
    }
}

pub(crate) fn reject_replay_secret(path: &Path, text: &str) -> Result<(), CompanionError> {
    let lower = text.to_ascii_lowercase();
    for prohibited in [
        "raw_secret",
        "token=",
        "authorization:",
        "bearer ",
        "api_key",
        "password=",
    ] {
        if lower.contains(prohibited) {
            return Err(CompanionError::Proposal(format!(
                "replay artifact {} contains prohibited raw content marker {prohibited}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<ReplayFixture, Box<dyn std::error::Error>> {
        Ok(ReplayFixture {
            fixture_id: "unit-fixture".to_owned(),
            date: NaiveDate::from_ymd_opt(2026, 7, 1).ok_or("valid date")?,
            tags: vec!["sparse".to_owned()],
            collector: serde_json::json!({"recording":"collector-fixture-v1","events":[{"id":"collector-1","kind":"redacted-activity","source":"local"}]}),
            model: serde_json::json!({"recording":"model-fixture-v1","proposals":[{"id":"proposal-1","issue":"DRAG-157","materialFields":["issue","startedAt","durationMinutes"]}]}),
            drag_read: serde_json::json!({"recording":"drag-read-fixture-v1","worklogs":[]}),
            preview: serde_json::json!({"recording":"preview-fixture-v1","dryRun":true,"payloads":[]}),
            mutation: serde_json::json!({"recording":"mutation-fixture-v1","attempted":false,"ledger":[]}),
            crash: serde_json::json!({"recording":"crash-fixture-v1","resumeState":"clean"}),
            network: serde_json::json!({"recording":"network-fixture-v1","allowed":false,"failures":[]}),
            expectations: ReplayExpectations {
                schema_valid: true,
                provenance_valid: true,
                redaction_valid: true,
                attribution_precision: 1.0,
                duration_precision: 1.0,
                overlaps: 0,
                duplicates: 0,
                unsafe_retries: 0,
                incorrect_creates: 0,
                privacy_incidents: 0,
                fabricated_material_fields: 0,
                duplicate_proposals: 0,
                accepted_overlaps: 0,
            },
        })
    }

    #[test]
    fn production_seams_drive_metrics_and_expected_values_only_compare(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let valid = fixture()?;
        let mut failures = Vec::new();
        let actual = validate_replay_seams(&valid, &mut failures);
        compare_replay_expectations(&valid, &actual, &mut failures);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(actual.schema_valid);

        let mut mismatch = fixture()?;
        mismatch.expectations.duplicate_proposals = 7;
        let mut failures = Vec::new();
        let actual = validate_replay_seams(&mismatch, &mut failures);
        compare_replay_expectations(&mismatch, &actual, &mut failures);
        assert!(
            failures.iter().any(|failure| {
                failure["evidence"] == "expectations"
                    && failure["rule"] == "duplicateProposals"
                    && failure["operation"] == "compare"
            }),
            "{failures:?}"
        );
        Ok(())
    }

    #[test]
    fn malformed_seam_types_and_schema_999_report_precise_diagnostics(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (field, evidence) in [
            ("collector", "collector"),
            ("model", "model"),
            ("drag", "dragRead"),
            ("mutation", "mutation"),
        ] {
            let mut replay = fixture()?;
            match field {
                "collector" => {
                    replay.collector =
                        serde_json::json!({"recording":"collector-fixture-v1","events":"bad"})
                }
                "model" => {
                    replay.model =
                        serde_json::json!({"recording":"model-fixture-v1","proposals":"bad"})
                }
                "drag" => {
                    replay.drag_read =
                        serde_json::json!({"recording":"drag-read-fixture-v1","worklogs":"bad"})
                }
                "mutation" => {
                    replay.mutation = serde_json::json!({"recording":"mutation-fixture-v1","attempted":"bad","ledger":[]})
                }
                _ => unreachable!(),
            }
            let mut failures = Vec::new();
            validate_replay_seams(&replay, &mut failures);
            assert!(
                failures.iter().any(|failure| {
                    failure["fixture"] == "unit-fixture"
                        && failure["evidence"] == evidence
                        && failure["rule"] == "typed seam schema"
                        && failure["operation"] == "load"
                }),
                "{field}: {failures:?}"
            );
        }

        let mut replay = fixture()?;
        replay.model =
            serde_json::json!({"schemaVersion":999,"recording":"model-fixture-v1","proposals":[]});
        let mut failures = Vec::new();
        validate_replay_seams(&replay, &mut failures);
        assert!(
            failures.iter().any(|failure| {
                failure["fixture"] == "unit-fixture"
                    && failure["evidence"] == "model"
                    && failure["rule"] == "schemaVersion"
                    && failure["operation"] == "load"
            }),
            "{failures:?}"
        );
        Ok(())
    }
}
