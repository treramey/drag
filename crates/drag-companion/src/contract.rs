use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Contract {
    pub(crate) schema_version: u32,
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
    pub(crate) compatibility: CompatibilityContract,
    pub(crate) commands: Vec<CommandContract>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompatibilityContract {
    pub(crate) shim: ShimCompatibility,
    pub(crate) legacy_direct_commands: LegacyCommandCompatibility,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShimCompatibility {
    pub(crate) binary: &'static str,
    pub(crate) available_through: &'static str,
    pub(crate) remove_in: &'static str,
    pub(crate) replacements: Vec<ShimCommandReplacement>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShimCommandReplacement {
    pub(crate) command: &'static str,
    pub(crate) replacement: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyCommandCompatibility {
    pub(crate) available_through: &'static str,
    pub(crate) remove_in: &'static str,
    pub(crate) replacement_prefix: &'static str,
    pub(crate) replacements: Vec<LegacyCommandReplacement>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyCommandReplacement {
    pub(crate) legacy: &'static str,
    pub(crate) replacement: &'static str,
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
        schema_version: TRACKING_MACHINE_CONTRACT_VERSION,
        binary: "drag-tracking",
        default_mode: DEFAULT_MODE,
        config_dir: "$DRAG_TRACKING_DATA/config.json or ~/.drag/tracking/config.json",
        data_dir: "$DRAG_TRACKING_DATA or ~/.drag/tracking",
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
            "automatic execution requires DRAG_TRACKING_LIVE_MUTATION_ROLLOUT=1",
            "execute requires persisted rollout general-autonomy permission",
        ],
        drag_boundary: drag_boundary(),
        compatibility: CompatibilityContract {
            shim: ShimCompatibility {
                binary: "drag-companion",
                available_through: "0.10.x",
                remove_in: "0.11.0",
                replacements: [
                    "setup", "status", "run", "review", "pause", "resume", "uninstall",
                    "sources", "schedule",
                ]
                .into_iter()
                .map(|command| ShimCommandReplacement {
                    command,
                    replacement: format!("drag tracking {command}"),
                })
                .chain(
                    [
                        "collect", "capture", "import", "reconcile", "report", "log", "bundle",
                        "propose", "read", "audit", "preview", "execute", "rollout", "replay",
                        "process-spy", "purge", "retention", "scheduler", "claude-hook",
                    ]
                    .into_iter()
                    .map(|command| ShimCommandReplacement {
                        command,
                        replacement: format!("drag-tracking internal {command}"),
                    }),
                )
                .chain([
                    ShimCommandReplacement {
                        command: "internal",
                        replacement: "drag-tracking internal".to_owned(),
                    },
                    ShimCommandReplacement {
                        command: "contract",
                        replacement: "drag-tracking contract".to_owned(),
                    },
                ])
                .collect(),
            },
            legacy_direct_commands: LegacyCommandCompatibility {
                available_through: "0.10.x",
                remove_in: "0.11.0",
                replacement_prefix: "drag-tracking internal",
                replacements: [
                    "collect", "capture", "import", "reconcile", "report", "log", "bundle",
                    "propose", "read", "audit", "preview", "execute", "rollout", "replay",
                    "process-spy", "purge", "retention", "scheduler", "claude-hook",
                ]
                .into_iter()
                .map(|legacy| LegacyCommandReplacement {
                    legacy,
                    replacement: legacy,
                })
                .collect(),
            },
        },
        commands: vec![
            command(
                "setup",
                false,
                vec!["persist explicit source, schedule, and submission consent", "optionally install owned scheduler and hook files"],
                vec![],
            ),
            command("status", false, vec![], vec![]),
            command(
                "run",
                false,
                vec!["coordinate one complete resumable tracking workflow"],
                vec![],
            ).with_possible_network(vec!["Drag read boundary and conditionally authorized submission"]),
            command(
                "review",
                false,
                vec!["optionally persist approval bound to the current proposal-set digest"],
                vec!["inspect", "approve"],
            ),
            command("pause", false, vec!["disable scheduled tracking while preserving history"], vec![]),
            command("resume", false, vec!["validate configuration and enable scheduled tracking"], vec![]),
            command("uninstall", false, vec!["remove only tracking-owned scheduler and hook files"], vec![]),
            command(
                "sources",
                false,
                vec![
                    "inspect supported and configured local evidence sources",
                    "persist validated explicitly selected source settings",
                    "run bounded redacted collector checks without persisting evidence or worklogs",
                ],
                vec!["list", "configure", "test"],
            ),
            command("schedule", false, vec!["persist and install an explicitly configured weekday schedule"], vec!["show", "update", "pause", "resume"]),
            command(
                "internal",
                false,
                vec!["diagnostic and recovery effects vary by selected operation"],
                vec!["collect", "capture", "import", "reconcile", "resume", "report", "log", "bundle", "propose", "read", "audit", "preview", "execute", "rollout", "replay", "process-spy", "purge", "retention", "scheduler", "claude-hook"],
            ).with_possible_network(vec!["read, audit, preview, and guarded execute operations use the Drag process boundary"])
             .with_possible_live_mutation(vec!["internal execute retains every authorization, rollout, duplicate, uncertainty, and kill-switch gate"]),
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
