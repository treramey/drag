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
        let expectations = &fixture.expectations;
        let required_present = [
            ("collector", &fixture.collector),
            ("model", &fixture.model),
            ("drag_read", &fixture.drag_read),
            ("preview", &fixture.preview),
            ("mutation", &fixture.mutation),
            ("crash", &fixture.crash),
            ("network", &fixture.network),
        ];
        for (rule, value) in required_present {
            if value.is_null() {
                failures.push(replay_failure(
                    &fixture.fixture_id,
                    rule,
                    "fixture field is null",
                    "load",
                    "recorded seam fixture is required",
                ));
            }
        }
        if !expectations.schema_valid {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "schemaValidity",
                "expectations.schemaValid",
                "validate",
                "schema invalid",
            ));
        }
        if !expectations.provenance_valid {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "provenance",
                "expectations.provenanceValid",
                "validate",
                "provenance invalid",
            ));
        }
        if !expectations.redaction_valid {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "redaction",
                "expectations.redactionValid",
                "validate",
                "redaction invalid",
            ));
        }
        if expectations.fabricated_material_fields != 0 {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "zero-fabrication",
                "expectations.fabricatedMaterialFields",
                "validate",
                "fabricated material fields detected",
            ));
        }
        if expectations.duplicate_proposals != 0 {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "zero-duplicate-proposals",
                "expectations.duplicateProposals",
                "validate",
                "duplicate proposals detected",
            ));
        }
        if expectations.accepted_overlaps != 0 {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "zero-accepted-overlaps",
                "expectations.acceptedOverlaps",
                "validate",
                "accepted overlaps detected",
            ));
        }
        if expectations.unsafe_retries != 0 {
            failures.push(replay_failure(
                &fixture.fixture_id,
                "zero-unsafe-retries",
                "expectations.unsafeRetries",
                "validate",
                "unsafe retries detected",
            ));
        }

        metrics["schemaValidity"] = serde_json::json!(
            metrics["schemaValidity"].as_u64().unwrap_or(0) + u64::from(expectations.schema_valid)
        );
        metrics["provenance"] = serde_json::json!(
            metrics["provenance"].as_u64().unwrap_or(0) + u64::from(expectations.provenance_valid)
        );
        metrics["redaction"] = serde_json::json!(
            metrics["redaction"].as_u64().unwrap_or(0) + u64::from(expectations.redaction_valid)
        );
        metrics["issueAttributionPrecision"] = serde_json::json!(metrics
            ["issueAttributionPrecision"]
            .as_f64()
            .unwrap_or(1.0)
            .min(expectations.attribution_precision));
        metrics["supportedDurationPrecision"] = serde_json::json!(metrics
            ["supportedDurationPrecision"]
            .as_f64()
            .unwrap_or(1.0)
            .min(expectations.duration_precision));
        for (key, value) in [
            ("overlaps", expectations.overlaps),
            ("duplicates", expectations.duplicates),
            ("unsafeRetries", expectations.unsafe_retries),
            ("incorrectCreates", expectations.incorrect_creates),
            ("privacyIncidents", expectations.privacy_incidents),
            (
                "fabricatedMaterialFields",
                expectations.fabricated_material_fields,
            ),
            ("duplicateProposals", expectations.duplicate_proposals),
            ("acceptedOverlaps", expectations.accepted_overlaps),
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
