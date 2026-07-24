use crate::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalEvent {
    pub(crate) schema_version: u32,
    pub(crate) event_id: String,
    pub(crate) event_type: String,
    pub(crate) observed_at: String,
    pub(crate) source: SourceProvenance,
    pub(crate) collector: CollectorProvenance,
    pub(crate) timestamp_semantics: TimestampSemantics,
    pub(crate) privacy: PrivacyState,
    pub(crate) retention: RetentionMetadata,
    pub(crate) supersedes: Option<String>,
    pub(crate) payload: Value,
    pub(crate) integrity_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceProvenance {
    pub(crate) kind: String,
    pub(crate) adapter: String,
    pub(crate) reference: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectorProvenance {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimestampSemantics {
    pub(crate) observed_at_source: String,
    pub(crate) timezone: String,
    pub(crate) explicit_date: NaiveDate,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrivacyState {
    pub(crate) classification: String,
    pub(crate) redacted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetentionMetadata {
    pub(crate) policy: String,
    pub(crate) retain_until: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceBundle {
    pub(crate) schema_version: u32,
    pub(crate) explicit_date: NaiveDate,
    pub(crate) mode: &'static str,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
    pub(crate) unsupported_gaps: Vec<&'static str>,
    pub(crate) source_health: Vec<BundleSourceHealth>,
    pub(crate) evidence: Vec<BundleEvidence>,
    pub(crate) contradictions: Vec<BundleContradiction>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleSourceHealth {
    pub(crate) source: String,
    pub(crate) events: usize,
    pub(crate) abandoned_sessions: usize,
    pub(crate) health: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleEvidence {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) reference: String,
    pub(crate) original_timestamp: String,
    pub(crate) original_timezone: String,
    pub(crate) observed_at_utc: Option<String>,
    pub(crate) interval_start_utc: Option<String>,
    pub(crate) interval_end_utc: Option<String>,
    pub(crate) elapsed_seconds: Option<i64>,
    pub(crate) summary: String,
    pub(crate) supersedes: Option<String>,
    pub(crate) superseded_by: Option<String>,
    pub(crate) contradicted_by: Vec<String>,
    pub(crate) abandoned_session: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleContradiction {
    pub(crate) key: String,
    pub(crate) evidence_ids: Vec<String>,
}

pub(crate) fn retention_config() -> Value {
    serde_json::json!({
        "rawEvidenceDays": retention_days("DRAG_COMPANION_RETENTION_RAW_DAYS", RAW_EVIDENCE_RETENTION_DAYS),
        "normalizedEvidenceDays": retention_days("DRAG_COMPANION_RETENTION_NORMALIZED_DAYS", NORMALIZED_EVIDENCE_RETENTION_DAYS),
        "reportsAndLedgerDays": retention_days("DRAG_COMPANION_RETENTION_REPORT_LEDGER_DAYS", REPORT_LEDGER_RETENTION_DAYS),
    })
}

pub(crate) fn retention_days(env_name: &str, default_days: u32) -> u32 {
    std::env::var(env_name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default_days)
}

pub(crate) fn append_journal_event(
    data_dir: &Path,
    event: &JournalEvent,
) -> Result<(), CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    ensure_companion_sentinel(data_dir)?;
    let _journal_lock = acquire_journal_lock(data_dir)?;
    let path = journal_path(data_dir);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|source| CompanionError::Open {
            path: path.clone(),
            source,
        })?;
    file.lock_exclusive()
        .map_err(|source| CompanionError::Open {
            path: path.clone(),
            source,
        })?;
    let mut body = serde_json::to_vec(event).map_err(CompanionError::Serialize)?;
    body.push(b'\n');
    file.write_all(&body)
        .map_err(|source| CompanionError::Write {
            path: path.clone(),
            source,
        })?;
    file.sync_data().map_err(|source| CompanionError::Write {
        path: path.clone(),
        source,
    })?;
    file.unlock()
        .map_err(|source| CompanionError::Write { path, source })
}

pub(crate) struct JournalLock {
    _file: File,
}

pub(crate) fn acquire_journal_lock(data_dir: &Path) -> Result<JournalLock, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    ensure_companion_sentinel(data_dir)?;
    let path = data_dir.join(".journal.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| CompanionError::Open {
            path: path.clone(),
            source,
        })?;
    file.lock_exclusive()
        .map_err(|source| CompanionError::Open { path, source })?;
    Ok(JournalLock { _file: file })
}

pub(crate) fn import_journal(data_dir: &Path) -> Result<usize, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let _journal_lock = acquire_journal_lock(data_dir)?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    let path = journal_path(data_dir);
    if !path.exists() {
        return Ok(0);
    }
    let file = File::open(&path).map_err(|source| CompanionError::Open { path, source })?;
    let tx = conn.transaction()?;
    let mut imported = 0;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|source| CompanionError::Read {
            path: journal_path(data_dir),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event: JournalEvent =
            serde_json::from_str(&line).map_err(|error| CompanionError::InvalidJournal {
                line: line_number,
                reason: error.to_string(),
            })?;
        validate_event(&tx, &event, line_number)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO evidence_events (event_id, event_type, observed_at, source_kind, source_adapter, source_reference, collector_name, collector_version, timestamp_source, timezone, explicit_date, privacy_classification, privacy_redacted, retention_policy, retain_until, supersedes, payload_json, integrity_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![event.event_id, event.event_type, event.observed_at, event.source.kind, event.source.adapter, event.source.reference, event.collector.name, event.collector.version, event.timestamp_semantics.observed_at_source, event.timestamp_semantics.timezone, event.timestamp_semantics.explicit_date.to_string(), event.privacy.classification, event.privacy.redacted, event.retention.policy, event.retention.retain_until, event.supersedes, event.payload.to_string(), event.integrity_hash],
        )?;
        imported += inserted;
    }
    tx.commit()?;
    Ok(imported)
}

pub(crate) fn validate_event(
    conn: &Connection,
    event: &JournalEvent,
    line: usize,
) -> Result<(), CompanionError> {
    if event.schema_version != JOURNAL_SCHEMA_VERSION {
        return Err(CompanionError::InvalidJournal {
            line,
            reason: format!("unsupported schemaVersion {}", event.schema_version),
        });
    }
    let expected = event_hash(event).map_err(CompanionError::Serialize)?;
    if event.integrity_hash != expected {
        return Err(CompanionError::InvalidJournal {
            line,
            reason: "integrity hash mismatch".to_owned(),
        });
    }
    let existing_hash: Option<String> = conn
        .query_row(
            "SELECT integrity_hash FROM evidence_events WHERE event_id = ?1",
            [&event.event_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing_hash) = existing_hash {
        if existing_hash != event.integrity_hash {
            return Err(CompanionError::InvalidJournal {
                line,
                reason: format!(
                    "duplicate eventId {} has different integrity hash",
                    event.event_id
                ),
            });
        }
    }
    if let Some(supersedes) = &event.supersedes {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM evidence_events WHERE event_id = ?1",
                [supersedes],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(CompanionError::InvalidJournal {
                line,
                reason: format!("supersedes unknown event {supersedes}"),
            });
        }
    }
    Ok(())
}

pub(crate) fn migrate(conn: &mut Connection) -> Result<(), CompanionError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS evidence_events (event_id TEXT PRIMARY KEY, event_type TEXT NOT NULL, observed_at TEXT NOT NULL, source_kind TEXT NOT NULL, source_adapter TEXT NOT NULL, source_reference TEXT NOT NULL, collector_name TEXT NOT NULL, collector_version TEXT NOT NULL, timestamp_source TEXT NOT NULL, timezone TEXT NOT NULL, explicit_date TEXT NOT NULL, privacy_classification TEXT NOT NULL, privacy_redacted INTEGER NOT NULL CHECK (privacy_redacted IN (0, 1)), retention_policy TEXT NOT NULL, retain_until TEXT, supersedes TEXT REFERENCES evidence_events(event_id), payload_json TEXT NOT NULL, integrity_hash TEXT NOT NULL UNIQUE);\
         CREATE TABLE IF NOT EXISTS issue_candidates (id TEXT PRIMARY KEY, evidence_event_id TEXT NOT NULL REFERENCES evidence_events(event_id), issue_key TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS daily_bundles (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS proposals (id TEXT PRIMARY KEY, bundle_id TEXT NOT NULL REFERENCES daily_bundles(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS unsupported_periods (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, reason TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','confirmed','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS policy_decisions (id TEXT PRIMARY KEY, proposal_id TEXT REFERENCES proposals(id), decision TEXT NOT NULL CHECK (decision IN ('approved','rejected','skipped','uncertain')), decided_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), started_at TEXT NOT NULL, finished_at TEXT);\
         CREATE TABLE IF NOT EXISTS leases (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','confirmed','rejected','skipped','failed','uncertain')), expires_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS mutation_operations (id TEXT PRIMARY KEY, proposal_id TEXT REFERENCES proposals(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), idempotency_key TEXT NOT NULL UNIQUE);\
         CREATE TABLE IF NOT EXISTS mutation_attempts (id TEXT PRIMARY KEY, operation_id TEXT NOT NULL REFERENCES mutation_operations(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), attempted_at TEXT NOT NULL);\
	         CREATE TABLE IF NOT EXISTS reports (id TEXT PRIMARY KEY, run_id TEXT REFERENCES runs(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','confirmed','rejected','skipped','failed','uncertain')), body_json TEXT NOT NULL);
	         CREATE TABLE IF NOT EXISTS provider_requests (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, adapter TEXT NOT NULL, model TEXT NOT NULL, schema_version INTEGER NOT NULL, request_hash TEXT NOT NULL, response_hash TEXT, state TEXT NOT NULL, attempts INTEGER NOT NULL, timeout_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL, error_kind TEXT);
	         CREATE TABLE IF NOT EXISTS proposal_drag_resolutions (proposal_id TEXT NOT NULL REFERENCES proposals(id), name TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (proposal_id, name));
	         CREATE TABLE IF NOT EXISTS proposal_policy_fields (proposal_id TEXT PRIMARY KEY REFERENCES proposals(id), evidence_refs_json TEXT NOT NULL, issue_key TEXT NOT NULL, supported_start TEXT NOT NULL, supported_end TEXT NOT NULL, description_facts_json TEXT NOT NULL, confidence REAL NOT NULL, limitations_json TEXT NOT NULL);"
	    )?;
    for ddl in [
        "ALTER TABLE policy_decisions ADD COLUMN reason_codes_json TEXT NOT NULL DEFAULT '[]'",
        "ALTER TABLE policy_decisions ADD COLUMN evidence_trace_json TEXT NOT NULL DEFAULT '[]'",
    ] {
        if let Err(error) = tx.execute(ddl, []) {
            if !error.to_string().contains("duplicate column name") {
                return Err(error.into());
            }
        }
    }
    let newest: Option<i64> =
        tx.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    if newest.is_some_and(|version| version > STORE_SCHEMA_VERSION) {
        return Err(CompanionError::Proposal(format!(
            "store schema version {} is newer than supported version {STORE_SCHEMA_VERSION}",
            newest.unwrap_or_default()
        )));
    }
    tx.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
        params![STORE_SCHEMA_VERSION, now_string()],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn evidence_event(date: NaiveDate) -> JournalEvent {
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.fake.{date}"),
        event_type: "evidence.captured".to_owned(),
        observed_at: now_string(),
        source: SourceProvenance {
            kind: "fixture".to_owned(),
            adapter: COLLECTOR_ADAPTER.to_owned(),
            reference: date.to_string(),
        },
        collector: CollectorProvenance {
            name: COLLECTOR_ADAPTER.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "collector-clock".to_owned(),
            timezone: "UTC".to_owned(),
            explicit_date: date,
        },
        privacy: PrivacyState {
            classification: "local-fixture".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: serde_json::json!({ "summary": "fake explicit-date capture completed without network or live mutation", "networkAccess": false, "liveMutationAllowed": false }),
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).unwrap_or_default();
    event
}

pub(crate) fn event_hash(event: &JournalEvent) -> Result<String, serde_json::Error> {
    let hash_body = serde_json::json!({
        "schemaVersion": event.schema_version, "eventId": event.event_id, "eventType": event.event_type,
        "observedAt": event.observed_at, "source": event.source, "collector": event.collector,
        "timestampSemantics": event.timestamp_semantics, "privacy": event.privacy,
        "retention": event.retention, "supersedes": event.supersedes, "payload": event.payload,
    });
    let encoded = serde_json::to_vec(&hash_body)?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}

pub(crate) fn build_bundle(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<EvidenceBundle, CompanionError> {
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    let mut stmt = conn.prepare(
        "SELECT event_id, source_adapter, source_reference, observed_at, timezone, supersedes, payload_json \
         FROM evidence_events WHERE explicit_date = ?1 ORDER BY event_id ASC",
    )?;
    let rows = stmt.query_map([date.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;

    let mut evidence = Vec::new();
    let mut contradiction_keys = std::collections::BTreeMap::new();
    for row in rows {
        let (
            id,
            source,
            reference,
            original_timestamp,
            original_timezone,
            supersedes,
            payload_json,
        ) = row?;
        let payload: Value =
            serde_json::from_str(&payload_json).map_err(CompanionError::Serialize)?;
        let interval_start = payload.get("intervalStart").and_then(Value::as_str);
        let interval_end = payload.get("intervalEnd").and_then(Value::as_str);
        let point = payload
            .get("observedAt")
            .and_then(Value::as_str)
            .unwrap_or(&original_timestamp)
            .to_owned();
        let summary = payload.get("summary").and_then(Value::as_str).unwrap_or("");
        let start_utc = interval_start.and_then(normalize_timestamp);
        let end_utc = interval_end.and_then(normalize_timestamp);
        let elapsed_seconds = match (interval_start, interval_end) {
            (Some(start), Some(end)) => elapsed(start, end),
            _ => None,
        };
        contradiction_keys.insert(
            id.clone(),
            minimized_reference(reference.split('#').next().unwrap_or(&reference)),
        );
        evidence.push(BundleEvidence {
            id,
            source,
            reference: minimized_reference(&reference),
            original_timestamp,
            original_timezone,
            observed_at_utc: normalize_timestamp(&point),
            interval_start_utc: start_utc,
            interval_end_utc: end_utc,
            elapsed_seconds,
            summary: redact(summary),
            supersedes,
            superseded_by: None,
            contradicted_by: Vec::new(),
            abandoned_session: interval_start.is_some() && interval_end.is_none(),
        });
    }
    evidence.sort_by(|left, right| left.id.cmp(&right.id));

    for index in 0..evidence.len() {
        let replacement_id = evidence[index].id.clone();
        if let Some(supersedes) = evidence[index].supersedes.clone() {
            if let Some(target) = evidence.iter_mut().find(|item| item.id == supersedes) {
                target.superseded_by = Some(replacement_id);
            }
        }
    }

    let mut contradictions = Vec::new();
    let mut by_key = std::collections::BTreeMap::<String, Vec<String>>::new();
    for item in &evidence {
        if let Some(key) = contradiction_keys.get(&item.id) {
            by_key.entry(key.clone()).or_default().push(item.id.clone());
        }
    }
    for (key, ids) in by_key.into_iter().filter(|(_, ids)| ids.len() > 1) {
        for id in &ids {
            if let Some(item) = evidence.iter_mut().find(|item| &item.id == id) {
                item.contradicted_by = ids.iter().filter(|other| *other != id).cloned().collect();
            }
        }
        contradictions.push(BundleContradiction {
            key,
            evidence_ids: ids,
        });
    }

    let mut health = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for item in &evidence {
        let entry = health.entry(item.source.clone()).or_default();
        entry.0 += 1;
        if item.abandoned_session {
            entry.1 += 1;
        }
    }
    let source_health = health
        .into_iter()
        .map(
            |(source, (events, abandoned_sessions))| BundleSourceHealth {
                source,
                events,
                abandoned_sessions,
                health: if abandoned_sessions > 0 {
                    "degraded"
                } else {
                    "healthy"
                },
            },
        )
        .collect();

    Ok(EvidenceBundle {
        schema_version: 1,
        explicit_date: date,
        mode: DEFAULT_MODE,
        network_access: false,
        live_mutation_allowed: false,
        unsupported_gaps: vec!["collectors-deferred", "model-export-only"],
        source_health,
        evidence,
        contradictions,
    })
}
