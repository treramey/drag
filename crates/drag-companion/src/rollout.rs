use crate::*;

pub(crate) const ROLLOUT_STAGES: [&str; 6] = [
    "capture-only",
    "historical-replay",
    "shadow",
    "reviewed-batches",
    "restricted-autonomy",
    "general-autonomy",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutState {
    pub(crate) stage: String,
    pub(crate) fixture: RolloutGate,
    pub(crate) replay: RolloutGate,
    pub(crate) shadow: RolloutGate,
    pub(crate) reviewed: RolloutGate,
    pub(crate) restricted: RolloutGate,
    pub(crate) general: RolloutGate,
    pub(crate) general_expansions: Vec<String>,
    pub(crate) last_reset_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutGate {
    pub(crate) eligible_days: u64,
    pub(crate) proposals: u64,
    pub(crate) issue_attribution_precision: f64,
    pub(crate) supported_duration_precision: f64,
    pub(crate) schema_valid: bool,
    pub(crate) provenance_retained: bool,
    pub(crate) secrets_redacted: bool,
    pub(crate) reviewed_batches: u64,
    pub(crate) incorrect_creates: u64,
    pub(crate) duplicates: u64,
    pub(crate) overlap_violations: u64,
    pub(crate) uncertain_outcome_retries: u64,
    pub(crate) privacy_incidents: u64,
    #[serde(default)]
    pub(crate) fabricated_material_fields: u64,
    #[serde(default)]
    pub(crate) unsafe_retries: u64,
    pub(crate) passed: bool,
}

impl Default for RolloutGate {
    fn default() -> Self {
        Self {
            eligible_days: 0,
            proposals: 0,
            issue_attribution_precision: 1.0,
            supported_duration_precision: 1.0,
            schema_valid: true,
            provenance_retained: true,
            secrets_redacted: true,
            reviewed_batches: 0,
            incorrect_creates: 0,
            duplicates: 0,
            overlap_violations: 0,
            uncertain_outcome_retries: 0,
            privacy_incidents: 0,
            fabricated_material_fields: 0,
            unsafe_retries: 0,
            passed: false,
        }
    }
}

impl Default for RolloutState {
    fn default() -> Self {
        Self {
            stage: "capture-only".to_owned(),
            fixture: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            replay: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            shadow: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            reviewed: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            restricted: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            general: RolloutGate {
                issue_attribution_precision: 1.0,
                supported_duration_precision: 1.0,
                schema_valid: true,
                provenance_retained: true,
                secrets_redacted: true,
                ..Default::default()
            },
            general_expansions: Vec::new(),
            last_reset_reason: None,
        }
    }
}

pub(crate) fn rollout_path(data_dir: &Path) -> PathBuf {
    data_dir.join("rollout-state.json")
}

pub(crate) fn load_rollout_state(data_dir: &Path) -> Result<RolloutState, CompanionError> {
    let path = rollout_path(data_dir);
    if !path.exists() {
        return Ok(RolloutState::default());
    }
    let text = fs::read_to_string(&path).map_err(|source| CompanionError::Read { path, source })?;
    serde_json::from_str(&text)
        .map_err(|error| CompanionError::Proposal(format!("rollout state schema: {error}")))
}

pub(crate) fn save_rollout_state(
    data_dir: &Path,
    state: &RolloutState,
) -> Result<(), CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let path = rollout_path(data_dir);
    let text = serde_json::to_string_pretty(state).map_err(CompanionError::Serialize)?;
    atomic_write(&path, text.as_bytes())
}

pub(crate) fn handle_rollout(data_dir: &Path, args: RolloutArgs) -> Result<(), CompanionError> {
    let mut state = load_rollout_state(data_dir)?;
    match args.operation {
        RolloutOperation::Status => print_json(&rollout_status_value(&state, None)),
        RolloutOperation::EffectiveMode(args) => {
            let reason = force_shadow_reason(&args);
            print_json(&rollout_status_value(&state, reason.as_deref()))
        }
        RolloutOperation::Record(args) => {
            if let Some(reason) = args.unsafe_reason.filter(|s| !s.is_empty()) {
                state.last_reset_reason = Some(reason);
                let gate = args
                    .gate
                    .unwrap_or_else(|| stage_gate_name(&state.stage).to_owned());
                *gate_mut(&mut state, &gate)? = RolloutGate::default();
                demote_after_unsafe_reset(&mut state, &gate);
            } else if let Some(expansion) = args.expansion {
                if !state.general_expansions.contains(&expansion) {
                    state.general_expansions.push(expansion);
                }
            } else {
                let gate = args
                    .gate
                    .unwrap_or_else(|| stage_gate_name(&state.stage).to_owned());
                let target = gate_mut(&mut state, &gate)?;
                target.eligible_days += args.eligible_days;
                target.proposals += args.proposals;
                target.issue_attribution_precision = target
                    .issue_attribution_precision
                    .min(args.issue_attribution_precision);
                target.supported_duration_precision = target
                    .supported_duration_precision
                    .min(args.supported_duration_precision);
                target.schema_valid &= args.schema_valid;
                target.provenance_retained &= args.provenance_retained;
                target.secrets_redacted &= args.secrets_redacted;
                target.reviewed_batches += args.reviewed_batches;
                target.incorrect_creates += args.incorrect_creates;
                target.duplicates += args.duplicates;
                target.overlap_violations += args.overlap_violations;
                target.uncertain_outcome_retries += args.uncertain_outcome_retries;
                target.privacy_incidents += args.privacy_incidents;
                target.fabricated_material_fields += args.fabricated_material_fields;
                target.unsafe_retries += args.unsafe_retries;
                target.passed = gate_passed(&gate, target);
            }
            save_rollout_state(data_dir, &state)?;
            print_json(&rollout_status_value(&state, None))
        }
        RolloutOperation::Promote => {
            promote_one_stage(&mut state);
            save_rollout_state(data_dir, &state)?;
            print_json(&rollout_status_value(&state, None))
        }
    }
}

pub(crate) fn gate_mut<'a>(
    state: &'a mut RolloutState,
    gate: &str,
) -> Result<&'a mut RolloutGate, CompanionError> {
    match gate {
        "fixture" => Ok(&mut state.fixture),
        "replay" | "historical-replay" => Ok(&mut state.replay),
        "shadow" => Ok(&mut state.shadow),
        "reviewed" | "reviewed-batches" => Ok(&mut state.reviewed),
        "restricted" | "restricted-autonomy" => Ok(&mut state.restricted),
        "general" | "general-autonomy" => Ok(&mut state.general),
        other => Err(CompanionError::Proposal(format!(
            "unknown rollout gate {other}"
        ))),
    }
}

pub(crate) fn stage_gate_name(stage: &str) -> &str {
    match stage {
        "capture-only" => "fixture",
        "historical-replay" => "replay",
        "shadow" => "shadow",
        "reviewed-batches" => "reviewed",
        "restricted-autonomy" => "restricted",
        _ => "general",
    }
}

pub(crate) fn demote_after_unsafe_reset(state: &mut RolloutState, gate: &str) {
    let reset_stage = match gate {
        "fixture" => "capture-only",
        "replay" | "historical-replay" => "historical-replay",
        _ => "shadow",
    };
    let current_index = ROLLOUT_STAGES
        .iter()
        .position(|stage| *stage == state.stage)
        .unwrap_or(0);
    let reset_index = ROLLOUT_STAGES
        .iter()
        .position(|stage| *stage == reset_stage)
        .unwrap_or(0);
    if current_index > reset_index {
        state.stage = reset_stage.to_owned();
    }
}

pub(crate) fn gate_passed(gate: &str, g: &RolloutGate) -> bool {
    match gate {
        "fixture" => g.schema_valid && g.provenance_retained && g.secrets_redacted,
        "replay" | "historical-replay" => {
            g.eligible_days >= 30
                && g.fabricated_material_fields == 0
                && g.duplicates == 0
                && g.overlap_violations == 0
                && g.unsafe_retries == 0
                && g.privacy_incidents == 0
        }
        "shadow" => {
            g.eligible_days >= 20
                && g.proposals >= 100
                && g.issue_attribution_precision >= 0.99
                && g.supported_duration_precision >= 0.99
        }
        "reviewed" | "reviewed-batches" => g.eligible_days >= 10 && g.reviewed_batches >= 10,
        "restricted" | "restricted-autonomy" => {
            g.eligible_days >= 20
                && g.incorrect_creates == 0
                && g.duplicates == 0
                && g.overlap_violations == 0
                && g.uncertain_outcome_retries == 0
                && g.privacy_incidents == 0
        }
        "general" | "general-autonomy" => true,
        _ => false,
    }
}

pub(crate) fn promote_one_stage(state: &mut RolloutState) {
    let next = match state.stage.as_str() {
        "capture-only" if state.fixture.passed => "historical-replay",
        "historical-replay" if state.replay.passed => "shadow",
        "shadow" if state.shadow.passed => "reviewed-batches",
        "reviewed-batches" if state.reviewed.passed => "restricted-autonomy",
        "restricted-autonomy" if state.restricted.passed => "general-autonomy",
        "general-autonomy" => "general-autonomy",
        current => current,
    };
    state.stage = next.to_owned();
}

pub(crate) fn force_shadow_reason(args: &RolloutEffectiveModeArgs) -> Option<String> {
    [
        (args.collector_health_failure, "collector-health"),
        (args.schema_compatibility_failure, "schema-compatibility"),
        (args.lock_failure, "lock-failure"),
        (args.incomplete_day, "incomplete-day"),
        (args.mutation_uncertainty, "mutation-uncertainty"),
    ]
    .into_iter()
    .find_map(|(hit, reason)| hit.then(|| reason.to_owned()))
}

pub(crate) fn rollout_status_value(state: &RolloutState, forced: Option<&str>) -> Value {
    let effective = if forced.is_some() {
        "shadow"
    } else {
        state.stage.as_str()
    };
    serde_json::json!({ "status": "ok", "stage": state.stage, "stages": ROLLOUT_STAGES, "effectiveMode": effective, "forcedShadowReason": forced, "liveMutationAllowed": forced.is_none() && state.stage == "general-autonomy" && state.restricted.passed, "lastResetReason": state.last_reset_reason, "gates": state })
}

pub(crate) fn persisted_live_mutation_allowed(data_dir: &Path) -> Result<bool, CompanionError> {
    let state = load_rollout_state(data_dir)?;
    Ok(state.stage == "general-autonomy" && state.restricted.passed)
}
