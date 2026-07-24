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
    pub(crate) original_timestamp: Option<String>,
    pub(crate) original_timezone: Option<String>,
    pub(crate) observed_at_utc: Option<String>,
    pub(crate) interval_start_utc: Option<String>,
    pub(crate) interval_end_utc: Option<String>,
    pub(crate) elapsed_seconds: Option<i64>,
    pub(crate) summary: String,
    pub(crate) status: Option<String>,
    pub(crate) unsupported_reason: Option<String>,
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
        let resolved_supersedes = resolve_supersedes(&tx, &event)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO evidence_events (event_id, event_type, observed_at, source_kind, source_adapter, source_reference, collector_name, collector_version, timestamp_source, timezone, explicit_date, privacy_classification, privacy_redacted, retention_policy, retain_until, supersedes, payload_json, integrity_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![event.event_id, event.event_type, event.observed_at, event.source.kind, event.source.adapter, event.source.reference, event.collector.name, event.collector.version, event.timestamp_semantics.observed_at_source, event.timestamp_semantics.timezone, event.timestamp_semantics.explicit_date.to_string(), event.privacy.classification, event.privacy.redacted, event.retention.policy, event.retention.retain_until, resolved_supersedes, event.payload.to_string(), event.integrity_hash],
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
        if exists.is_none() && ics_identity(&event.event_id).is_none() {
            return Err(CompanionError::InvalidJournal {
                line,
                reason: format!("supersedes unknown event {supersedes}"),
            });
        }
    }
    Ok(())
}

fn resolve_supersedes(
    conn: &Connection,
    event: &JournalEvent,
) -> Result<Option<String>, CompanionError> {
    let Some(requested) = event.supersedes.as_deref() else {
        return Ok(None);
    };
    let exact: Option<String> = conn
        .query_row(
            "SELECT event_id FROM evidence_events WHERE event_id = ?1",
            [requested],
            |row| row.get(0),
        )
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }
    let Some((prefix, current_sequence)) = ics_identity(&event.event_id) else {
        return Ok(None);
    };
    let pattern = format!("{prefix}.%");
    let mut stmt = conn
        .prepare("SELECT event_id FROM evidence_events WHERE event_id LIKE ?1 ORDER BY event_id")?;
    let rows = stmt.query_map([pattern], |row| row.get::<_, String>(0))?;
    let mut latest = None;
    for candidate in rows {
        let candidate = candidate?;
        if let Some((candidate_prefix, sequence)) = ics_identity(&candidate) {
            if candidate_prefix == prefix
                && sequence < current_sequence
                && latest
                    .as_ref()
                    .is_none_or(|(_, latest_sequence)| sequence > *latest_sequence)
            {
                latest = Some((candidate, sequence));
            }
        }
    }
    Ok(latest.map(|(event_id, _)| event_id))
}

fn ics_identity(event_id: &str) -> Option<(&str, i64)> {
    let (prefix, sequence) = event_id.rsplit_once('.')?;
    event_id.starts_with("evidence.ics.").then(|| {
        sequence
            .parse::<i64>()
            .ok()
            .map(|sequence| (prefix, sequence))
    })?
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
        let safe_id = provider_evidence_id(&id);
        contradiction_keys.insert(
            safe_id.clone(),
            minimized_reference(reference.split('#').next().unwrap_or(&reference)),
        );
        let safe_supersedes = supersedes.as_deref().map(provider_evidence_id);
        let status = payload
            .get("status")
            .and_then(Value::as_str)
            .filter(|status| matches!(*status, "CONFIRMED" | "TENTATIVE" | "CANCELLED"))
            .map(ToOwned::to_owned);
        let unsupported_reason = payload
            .get("unsupportedReason")
            .and_then(Value::as_str)
            .filter(|reason| {
                matches!(
                    *reason,
                    "missing-calendar-end"
                        | "invalid-calendar-end"
                        | "incompatible-calendar-end"
                        | "endpoint-timezone-mismatch"
                        | "non-positive-calendar-interval"
                        | "calendar-event-cancelled"
                )
            })
            .map(ToOwned::to_owned);
        evidence.push(BundleEvidence {
            id: safe_id,
            source: provider_source(&source),
            reference: minimized_reference(&reference),
            original_timestamp: valid_original_timestamp(&original_timestamp),
            original_timezone: valid_original_timezone(&original_timezone),
            observed_at_utc: normalize_timestamp(&point),
            interval_start_utc: start_utc,
            interval_end_utc: end_utc,
            elapsed_seconds,
            summary: redact_bundle_text(summary),
            status,
            unsupported_reason: unsupported_reason.clone(),
            supersedes: safe_supersedes,
            superseded_by: None,
            contradicted_by: Vec::new(),
            abandoned_session: unsupported_reason.is_none()
                && interval_start.is_some()
                && interval_end.is_none(),
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

fn provider_evidence_id(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let safe_characters = raw.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
    });
    let sensitive = [
        "token",
        "password",
        "passwd",
        "secret",
        "bearer",
        "authorization",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !raw.is_empty() && raw.len() <= 160 && safe_characters && !sensitive {
        raw.to_owned()
    } else {
        format!("evidence:{}", sha256_str(raw))
    }
}

fn provider_source(raw: &str) -> String {
    if matches!(
        raw,
        "fake" | "git-local" | "ics-local" | "claude-code-session-hook" | "fixture"
    ) {
        raw.to_owned()
    } else {
        format!("local-source:{}", sha256_str(raw))
    }
}

fn valid_original_timestamp(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|_| raw.to_owned())
}

fn valid_original_timezone(raw: &str) -> Option<String> {
    if matches!(raw, "UTC" | "all-day" | "from-git-offset") || raw.parse::<Tz>().is_ok() {
        Some(raw.to_owned())
    } else {
        None
    }
}

fn redact_bundle_text(raw: &str) -> String {
    let words = raw.split_whitespace().collect::<Vec<_>>();
    let mut safe = Vec::new();
    let mut skip = 0_usize;
    for word in words {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let normalized = word
            .trim_matches(|character: char| {
                !character.is_ascii_alphanumeric()
                    && character != '_'
                    && character != '-'
                    && character != '='
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
        .find(|label| normalized.starts_with(**label));
        if let Some(label) = secret_label {
            if !normalized.contains('=') {
                skip = if *label == "authorization" { 2 } else { 1 };
            }
            continue;
        }
        if normalized == "bearer" {
            skip = 1;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower.starts_with("bearer")
            || lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("akia")
            || lower.contains("/home/")
            || lower.contains("/users/")
            || lower.contains("\\users\\")
            || lower.contains("transcript")
            || lower.contains("instruction")
            || lower == "ignore"
            || (word.contains('@') && word.contains('.'))
        {
            continue;
        }
        safe.push(word);
    }
    if safe.is_empty() {
        "[redacted]".to_owned()
    } else {
        safe.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar(
        sequence: i64,
        status: &str,
        summary: &str,
    ) -> Result<CalendarEvidence, Box<dyn std::error::Error>> {
        Ok(CalendarEvidence {
            uid: "calendar-person@example.test".to_owned(),
            occurrence_date: NaiveDate::from_ymd_opt(2026, 3, 8).ok_or("valid date")?,
            status: status.to_owned(),
            recurrence_id: Some("2026-03-08".to_owned()),
            last_modified: Some(format!("2026-03-08T12:00:0{sequence}Z")),
            timezone: "America/New_York".to_owned(),
            all_day: false,
            interval_start: (status != "CANCELLED").then(|| "2026-03-08T13:00:00Z".to_owned()),
            interval_end: (status != "CANCELLED").then(|| "2026-03-08T14:00:00Z".to_owned()),
            unsupported_reason: (status == "CANCELLED")
                .then(|| "calendar-event-cancelled".to_owned()),
            summary: summary.to_owned(),
            source_file: "private.ics".to_owned(),
            sequence,
        })
    }

    #[test]
    fn provider_identifiers_are_opaque_when_the_source_contains_private_data() {
        let session = provider_evidence_id("evidence.claude.password=hunter2.SessionStart");
        let calendar = provider_evidence_id("evidence.ics.alice.private@example.com.1");
        assert!(session.starts_with("evidence:sha256:"));
        assert!(calendar.starts_with("evidence:sha256:"));
        assert!(!session.contains("hunter2"));
        assert!(!calendar.contains("alice.private"));
    }

    #[test]
    fn provider_summary_redaction_consumes_entire_authorization_credentials() {
        let redacted = redact_bundle_text(
            "worked Authorization: Bearer abc123xyz password=hunter2 /home/alice/private",
        );
        assert_eq!(redacted, "worked");
        assert!(!redacted.contains("abc123xyz"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("/home/alice"));
    }

    #[test]
    fn latest_only_ics_update_imports_without_an_unknown_predecessor(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let event = calendar_event(&calendar(2, "CONFIRMED", "latest feed")?)?;
        append_journal_event(directory.path(), &event)?;
        assert_eq!(import_journal(directory.path())?, 1);
        let conn = Connection::open(store_path(directory.path()))?;
        let supersedes: Option<String> = conn.query_row(
            "SELECT supersedes FROM evidence_events WHERE event_id = ?1",
            [&event.event_id],
            |row| row.get(0),
        )?;
        assert_eq!(supersedes, None);
        Ok(())
    }

    #[test]
    fn sequence_jump_and_cancellation_supersede_latest_known_event(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let first = calendar_event(&calendar(1, "CONFIRMED", "active meeting")?)?;
        append_journal_event(directory.path(), &first)?;
        import_journal(directory.path())?;

        let cancellation = calendar_event(&calendar(3, "CANCELLED", "cancelled meeting")?)?;
        append_journal_event(directory.path(), &cancellation)?;
        import_journal(directory.path())?;

        let bundle = build_bundle(
            directory.path(),
            NaiveDate::from_ymd_opt(2026, 3, 8).ok_or("valid date")?,
        )?;
        let active = bundle
            .evidence
            .iter()
            .find(|event| event.summary == "active meeting")
            .ok_or("active audit event")?;
        let tombstone = bundle
            .evidence
            .iter()
            .find(|event| event.status.as_deref() == Some("CANCELLED"))
            .ok_or("cancellation tombstone")?;
        assert_eq!(active.superseded_by.as_deref(), Some(tombstone.id.as_str()));
        assert_eq!(tombstone.supersedes.as_deref(), Some(active.id.as_str()));
        assert_eq!(tombstone.interval_start_utc, None);
        assert_eq!(
            tombstone.unsupported_reason.as_deref(),
            Some("calendar-event-cancelled")
        );
        Ok(())
    }
}
