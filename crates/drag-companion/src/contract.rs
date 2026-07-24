use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Contract {
    pub(crate) binary: &'static str,
    pub(crate) default_mode: &'static str,
    pub(crate) config_dir: &'static str,
    pub(crate) data_dir: &'static str,
    pub(crate) adapters: Adapters,
    pub(crate) default_network_access: bool,
    pub(crate) possible_network_access: bool,
    pub(crate) conditional_network_access: Vec<&'static str>,
    pub(crate) default_live_mutation_allowed: bool,
    pub(crate) possible_live_mutation_allowed: bool,
    pub(crate) conditional_live_mutation_allowed: Vec<&'static str>,
    pub(crate) drag_boundary: DragBoundary,
    pub(crate) commands: Vec<CommandContract>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Adapters {
    pub(crate) collector: &'static str,
    pub(crate) mutator: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DragBoundary {
    pub(crate) invocation: &'static str,
    pub(crate) schema_contract: &'static str,
    pub(crate) process_boundary: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandContract {
    pub(crate) name: &'static str,
    pub(crate) requires_explicit_date: bool,
    pub(crate) side_effects: Vec<&'static str>,
    pub(crate) default_network_access: bool,
    pub(crate) possible_network_access: bool,
    pub(crate) conditional_network_access: Vec<&'static str>,
    pub(crate) default_live_mutation_allowed: bool,
    pub(crate) possible_live_mutation_allowed: bool,
    pub(crate) conditional_live_mutation_allowed: Vec<&'static str>,
    pub(crate) operations: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResult {
    pub(crate) date: NaiveDate,
    pub(crate) status: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) adapters: Adapters,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
    pub(crate) drag_boundary: DragBoundary,
    pub(crate) observations: Vec<FakeObservation>,
}

pub(crate) fn contract() -> Contract {
    Contract {
        binary: "drag-companion",
        default_mode: DEFAULT_MODE,
        config_dir: "$DRAG_COMPANION_CONFIG or .drag-companion/config.json",
        data_dir: "$DRAG_COMPANION_DATA or .drag-companion",
        adapters: adapters(),
        default_network_access: false,
        possible_network_access: true,
        conditional_network_access: vec![
            "read/audit/preview/execute may invoke Drag public CLI operations that contact Tempo depending on Drag cache/config",
        ],
        default_live_mutation_allowed: false,
        possible_live_mutation_allowed: true,
        conditional_live_mutation_allowed: vec![
            "execute requires --authorize-live",
            "execute requires DRAG_COMPANION_LIVE_MUTATION_ROLLOUT=1",
            "execute requires persisted rollout general-autonomy permission",
        ],
        drag_boundary: drag_boundary(),
        commands: vec![
            command("status", false, vec![], vec![]),
            command("collect", false, vec!["capture fake observations"], vec![]),
            command(
                "capture",
                true,
                vec!["append one immutable evidence event to journal"],
                vec![],
            ),
            command(
                "import",
                false,
                vec!["migrate sqlite store", "import journal events idempotently"],
                vec![],
            ),
            command("reconcile", true, vec!["write terminal run result"], vec![]),
            command("resume", true, vec!["write terminal run result"], vec![]),
            command("report", true, vec![], vec![]),
            command("log", true, vec!["emit secret-safe structured operator status"], vec![]),
            command(
                "bundle",
                true,
                vec!["read imported evidence and print minimized daily bundle"],
                vec![],
            ),
            command(
                "propose",
                true,
                vec!["read minimized bundle", "persist schema-valid proposals and safe provider metadata"],
                vec![],
            ),
            command("read", true, vec![], vec!["drag list through public CLI"]).with_possible_network(vec!["drag public CLI may contact Tempo depending on Drag cache/config"]),
            command(
                "audit",
                true,
                vec![],
                vec![
                    "drag list through public CLI",
                    "local duplicate and overlap comparison",
                    "deterministic unattended policy decisions require --authorize-unattended before approval",
                ],
            ).with_possible_network(vec!["drag list through public CLI may contact Tempo depending on Drag cache/config"]),
            command("preview", true, vec![], vec!["drag log --json - --dry-run through public CLI"]).with_possible_network(vec!["drag dry-run public CLI may perform schema/client validation without creating worklogs"]),
            command(
                "execute",
                true,
                vec![
                    "persist exact payload and submitting intent before Drag invocation",
                    "persist durable mutation operation ledger",
                ],
                vec![
                    "drag list complete day before create",
                    "drag log --json - only when --authorize-live and rollout env are enabled",
                ],
            ).with_possible_network(vec!["drag list complete day before create", "drag log submission when live mutation conditions pass"]).with_possible_live_mutation(vec!["--authorize-live", "DRAG_COMPANION_LIVE_MUTATION_ROLLOUT=1", "persisted rollout permits general-autonomy"]),
            command(
                "rollout",
                false,
                vec!["persist staged autonomy promotion evidence and reset reasons"],
                vec!["status", "record", "promote", "effective-mode"],
            ),
            command("replay", false, vec!["read recorded fixtures and compare deterministic outputs"], vec![]),
            command(
                "process-spy",
                true,
                vec![],
                vec!["inspect durable mutation operation ledger"],
            ),
            command(
                "purge",
                false,
                vec!["delete companion data directory"],
                vec![],
            ),
            command(
                "retention",
                false,
                vec!["compact journal and canonical store according to configured retention windows"],
                vec!["enforce"],
            ),
            command(
                "scheduler",
                false,
                vec![
                    "write only owned host scheduler files",
                    "persist scheduler state atomically with backup",
                    "run one scheduler-safe explicit-date reconciliation command",
                    "kill switch forces shadow mode before mutation",
                ],
                vec!["install", "enable", "disable", "uninstall", "status", "catch-up", "run"],
            ),
            command(
                "claude-hook",
                false,
                vec![
                    "install SessionStart and SessionEnd capture hooks while preserving unrelated Claude settings",
                    "remove only drag-companion Claude hook commands",
                    "append local Claude lifecycle metadata from stdin without transcript capture",
                ],
                vec!["install", "remove", "capture"],
            ),
            command("contract", false, vec![], vec![]),
        ],
    }
}

pub(crate) fn command(
    name: &'static str,
    requires_explicit_date: bool,
    side_effects: Vec<&'static str>,
    operations: Vec<&'static str>,
) -> CommandContract {
    CommandContract {
        name,
        requires_explicit_date,
        side_effects,
        default_network_access: false,
        possible_network_access: false,
        conditional_network_access: Vec::new(),
        default_live_mutation_allowed: false,
        possible_live_mutation_allowed: false,
        conditional_live_mutation_allowed: Vec::new(),
        operations,
    }
}

impl CommandContract {
    fn with_possible_network(mut self, conditions: Vec<&'static str>) -> Self {
        self.possible_network_access = true;
        self.conditional_network_access = conditions;
        self
    }

    fn with_possible_live_mutation(mut self, conditions: Vec<&'static str>) -> Self {
        self.possible_live_mutation_allowed = true;
        self.conditional_live_mutation_allowed = conditions;
        self
    }
}

pub(crate) fn adapters() -> Adapters {
    Adapters {
        collector: COLLECTOR_ADAPTER,
        mutator: MUTATOR_ADAPTER,
    }
}
pub(crate) fn drag_boundary() -> DragBoundary {
    DragBoundary {
        invocation: "drag public CLI process",
        schema_contract: "drag schema",
        process_boundary: true,
    }
}
