use clap::{Arg, ArgAction, Command, CommandFactory};
use drag::field_selection::ListField;
use drag::models::{AddWorklogRequest, ListPagination, Worklog};
use drag::pagination::{
    DEFAULT_PAGE_LIMIT, DEFAULT_RECORD_LIMIT, HARD_PAGE_LIMIT, MAX_RECORD_LIMIT,
};
use drag::schedule::ScheduleDetails;
use schemars::{generate::SchemaSettings, schema_for, JsonSchema};
use serde_json::{json, Map, Value};

use crate::cli::{Cli, DeleteInput, LogInput, SchemaArgs};
use crate::config::{
    ATLASSIAN_EMAIL_ENV, ATLASSIAN_HOST_ENV, ATLASSIAN_TOKEN_ENV, DRAG_CONFIG_ENV,
    TEMPO_ACCOUNT_ID_ENV, TEMPO_TOKEN_ENV,
};
use crate::error::CliError;
use crate::list_tui::{
    INTERRUPT_KEY, MOVE_DOWN_KEY, MOVE_UP_KEY, NEXT_DATE_KEY, OPEN_ISSUE_KEY, PREVIOUS_DATE_KEY,
    QUIT_KEY,
};
use crate::output::Rendered;
use crate::setup_tui::REDUCED_MOTION_ENV;
use crate::tempo_openapi::{self, CACHE_DIR_ENV};

const SCHEMA_VERSION: u64 = 6;

pub(crate) fn schema() -> Rendered {
    let mut clap = Cli::command();
    clap.build();

    let commands = clap
        .get_subcommands()
        .map(|command| {
            let path = command.get_name().to_owned();
            (path.clone(), command_contract(command, &path))
        })
        .collect::<Map<_, _>>();

    let data = json!({
        "schemaDialect": "https://json-schema.org/draft/2020-12/schema",
        "schemaVersion": SCHEMA_VERSION,
        "cliVersion": env!("CARGO_PKG_VERSION"),
        "name": "drag",
        "description": "Complete command, input, result, error, and side-effect contract for Drag.",
        "$defs": shared_definitions(),
        "globalOptions": argument_contracts(&clap, "drag", true),
        "commands": commands,
        "output": output_contract(),
        "errors": error_contract(),
        "environment": {
            (DRAG_CONFIG_ENV): {"type": "path", "purpose": "Override the configuration file."},
            (CACHE_DIR_ENV): {"type": "path", "purpose": "Override the OpenAPI cache directory."},
            (REDUCED_MOTION_ENV): {"type": "boolean-like", "purpose": "Reduce interactive setup motion."},
            (TEMPO_TOKEN_ENV): {"type": "secret", "purpose": "Override the stored Tempo token."},
            (TEMPO_ACCOUNT_ID_ENV): {"type": "string", "purpose": "Runtime compatibility override for the Tempo account ID."},
            (ATLASSIAN_EMAIL_ENV): {"type": "string", "purpose": "Override the stored Atlassian email."},
            (ATLASSIAN_TOKEN_ENV): {"type": "secret", "purpose": "Override the stored Atlassian API token."},
            (ATLASSIAN_HOST_ENV): {"type": "https-host", "purpose": "Override the stored Atlassian host."}
        },
        "syntax": {
            "date": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"],
            "duration": ["15m", "1h", "1h15m"],
            "interval": ["11-14", "11-14:30", "11:35-14:20", "11.35-14.20", "23:30-00:30"]
        }
    });

    let human = format!("{data:#}");
    Rendered::new(data, human)
}

pub(crate) async fn run(args: SchemaArgs) -> Result<Rendered, CliError> {
    match args.path {
        Some(path) => tempo_openapi::schema(&path, args.resolve_refs).await,
        None => Ok(schema()),
    }
}

fn command_contract(command: &Command, path: &str) -> Value {
    let Some(identity) = CommandIdentity::from_path(path) else {
        return json!({
            "path": path,
            "metadataError": "missingExplicitCommandIdentity"
        });
    };
    let subcommands = if command.get_name() == "help" {
        Map::new()
    } else {
        command
            .get_subcommands()
            .map(|subcommand| {
                let child_path = format!("{path} {}", subcommand.get_name());
                (
                    subcommand.get_name().to_owned(),
                    command_contract(subcommand, &child_path),
                )
            })
            .collect::<Map<_, _>>()
    };
    let aliases = command
        .get_all_aliases()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let descriptor = command_behavior_descriptor(identity);

    let mut contract = json!({
        "path": path,
        "description": command.get_about().map(ToString::to_string),
        "aliases": aliases,
        "hidden": command.is_hide_set(),
        "arguments": argument_contracts(command, path, false),
        "subcommands": subcommands,
        "success": descriptor.contract.success,
        "errorCodes": descriptor.contract.error_codes,
        "sideEffects": descriptor.contract.side_effects,
        "networkAccess": descriptor.contract.network_access,
        "dryRun": descriptor.contract.dry_run,
        "failureDetails": descriptor.failure_details,
        "behavior": descriptor.behavior
    });
    if identity == CommandIdentity::List {
        contract["successWithFieldSelection"] = schema_ref("ProjectedListResult");
        contract["ndjson"] = list_stream_contract();
    }
    if command.get_name() == "help" {
        contract["helpTargets"] = if path == "help" {
            let mut root = Cli::command();
            root.build();
            json!(help_targets(&root))
        } else {
            json!(help_targets(command))
        };
    }
    contract
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandIdentity {
    Log,
    List,
    Delete,
    Setup,
    Doctor,
    Tempo,
    Schema,
    GenerateSkills,
    Help,
}

impl CommandIdentity {
    fn from_path(path: &str) -> Option<Self> {
        match path {
            "log" => Some(Self::Log),
            "list" => Some(Self::List),
            "delete" => Some(Self::Delete),
            "setup" => Some(Self::Setup),
            "doctor" => Some(Self::Doctor),
            "tempo" => Some(Self::Tempo),
            "schema" => Some(Self::Schema),
            "generate-skills" => Some(Self::GenerateSkills),
            "help" => Some(Self::Help),
            _ => None,
        }
    }
}

fn help_targets(command: &Command) -> Vec<String> {
    fn collect(command: &Command, prefix: &str, targets: &mut Vec<String>) {
        for subcommand in command.get_subcommands() {
            let path = if prefix.is_empty() {
                subcommand.get_name().to_owned()
            } else {
                format!("{prefix} {}", subcommand.get_name())
            };
            targets.push(path.clone());
            for alias in subcommand.get_all_aliases() {
                targets.push(if prefix.is_empty() {
                    alias.to_owned()
                } else {
                    format!("{prefix} {alias}")
                });
            }
            collect(subcommand, &path, targets);
        }
    }

    let mut targets = Vec::new();
    collect(command, "", &mut targets);
    targets.sort();
    targets.dedup();
    targets
}

fn argument_contracts(command: &Command, path: &str, globals_only: bool) -> Vec<Value> {
    command
        .get_arguments()
        .filter(|argument| {
            let built_in = matches!(argument.get_id().as_str(), "help" | "version");
            if globals_only {
                argument.is_global_set() || built_in
            } else {
                !argument.is_global_set() && !built_in
            }
        })
        .map(|argument| argument_contract(command, argument, path))
        .collect()
}

fn argument_contract(command: &Command, argument: &Arg, path: &str) -> Value {
    let id = argument.get_id().as_str();
    let is_global = argument.is_global_set() || matches!(id, "help" | "version");
    let switch = is_switch(argument);
    let possible_values = if switch {
        Vec::new()
    } else {
        argument
            .get_value_parser()
            .possible_values()
            .map(|values| {
                values
                    .map(|value| value.get_name().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let defaults = argument
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut conflicts = command
        .get_arg_conflicts_with(argument)
        .into_iter()
        .filter(|other| other.get_id() != argument.get_id())
        .map(|other| canonical_name(other.get_id().as_str()))
        .collect::<Vec<_>>();
    for other in command.get_arguments() {
        if other.get_id() != argument.get_id()
            && command
                .get_arg_conflicts_with(other)
                .iter()
                .any(|conflict| conflict.get_id() == argument.get_id())
        {
            conflicts.push(canonical_name(other.get_id().as_str()));
        }
    }
    conflicts.sort();
    conflicts.dedup();
    let value_count = argument.get_num_args().map(|range| {
        json!({
            "minimum": range.min_values(),
            "maximum": if range.max_values() == usize::MAX {
                Value::Null
            } else {
                json!(range.max_values())
            }
        })
    });
    let mut contract = json!({
        "id": id,
        "name": canonical_name(id),
        "kind": if argument.is_positional() {"positional"} else {"option"},
        "type": argument_type(path, argument, &possible_values),
        "required": argument.is_required_set(),
        "global": is_global,
        "valueCount": value_count,
        "conflictsWith": conflicts
    });
    if let Some(help) = argument.get_help() {
        contract["description"] = json!(help.to_string());
    }

    if let Some(long) = argument.get_long() {
        contract["long"] = json!(format!("--{long}"));
    }
    if let Some(short) = argument.get_short() {
        contract["short"] = json!(format!("-{short}"));
    }
    if !possible_values.is_empty() {
        contract["enum"] = json!(possible_values);
    }
    if defaults.len() == 1 {
        contract["default"] = if switch {
            json!(defaults[0] == "true")
        } else if argument_type(path, argument, &possible_values) == "unsignedInteger" {
            defaults[0]
                .parse::<u64>()
                .map_or_else(|_| json!(defaults[0]), |value| json!(value))
        } else {
            json!(defaults[0])
        };
    } else if !defaults.is_empty() {
        contract["default"] = json!(defaults);
    } else if let Some(default) = documented_default(path, id) {
        contract["default"] = default;
    }
    if let Some(required_unless) = required_unless(path, id) {
        contract["requiredUnlessPresent"] = json!(required_unless);
    }
    if let Some(required_with) = required_with(path, id) {
        contract["requires"] = json!(required_with);
    }
    if let Some(default) = semantic_default(path, id) {
        contract["semanticDefault"] = json!(default);
    }
    if let Some((minimum, maximum)) = numeric_bounds(path, id) {
        contract["minimum"] = json!(minimum);
        contract["maximum"] = json!(maximum);
    }
    if path == "list" && id == "fields" {
        contract["separator"] = json!(",");
        contract["allowedFields"] = json!(ListField::paths().collect::<Vec<_>>());
    }
    if id == "json" {
        let input_schema = match path {
            "log" => Some(json_schema::<LogInput>()),
            "delete" => Some(json_schema::<DeleteInput>()),
            _ => None,
        };
        if let Some(input_schema) = input_schema {
            contract["stdinValue"] = json!("-");
            contract["jsonSchema"] = input_schema;
        }
    }
    contract
}

fn documented_default(path: &str, id: &str) -> Option<Value> {
    match (path, id) {
        ("list", "limit") => Some(json!(DEFAULT_RECORD_LIMIT)),
        ("list", "page_limit") => Some(json!(DEFAULT_PAGE_LIMIT)),
        _ => None,
    }
}

fn argument_type(path: &str, argument: &Arg, possible_values: &[String]) -> &'static str {
    if is_switch(argument) {
        "boolean"
    } else if path == "list" && argument.get_id() == "fields" {
        "fieldMask"
    } else if (path == "delete" && argument.get_id() == "worklog_ids")
        || (path == "list" && matches!(argument.get_id().as_str(), "limit" | "page_limit"))
    {
        "unsignedInteger"
    } else if argument.get_id() == "config"
        || (path == "generate-skills" && argument.get_id() == "output_dir")
    {
        "path"
    } else if !possible_values.is_empty() {
        "enum"
    } else {
        "string"
    }
}

fn numeric_bounds(path: &str, id: &str) -> Option<(u64, u64)> {
    match (path, id) {
        ("list", "limit") => Some((1, MAX_RECORD_LIMIT as u64)),
        ("list", "page_limit") => Some((1, u64::from(HARD_PAGE_LIMIT))),
        _ => None,
    }
}

fn is_switch(argument: &Arg) -> bool {
    matches!(
        argument.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Help | ArgAction::Version
    )
}

fn required_unless(path: &str, id: &str) -> Option<Vec<&'static str>> {
    match (path, id) {
        ("log", "issue_key" | "duration_or_interval") => Some(vec!["json"]),
        ("delete", "worklog_ids") => Some(vec!["json"]),
        _ => None,
    }
}

fn required_with(path: &str, id: &str) -> Option<Vec<&'static str>> {
    match (path, id) {
        ("setup", "dry_run") => Some(vec!["fromEnv"]),
        ("setup", "verify") => Some(vec!["fromEnv", "dryRun"]),
        _ => None,
    }
}

fn semantic_default(path: &str, id: &str) -> Option<&'static str> {
    match (path, id) {
        ("log" | "list", "when") => Some("todayInConfiguredLocalTimeZone"),
        _ => None,
    }
}

fn canonical_name(id: &str) -> String {
    let mut words = id.split('_');
    let mut name = words.next().unwrap_or_default().to_owned();
    for word in words {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            name.extend(first.to_uppercase());
            name.extend(characters);
        }
    }
    name
}

struct CommandSemantics {
    success: Value,
    error_codes: Vec<&'static str>,
    side_effects: Value,
    network_access: Value,
    dry_run: Value,
}

struct CommandBehaviorDescriptor {
    contract: CommandSemantics,
    behavior: Value,
    failure_details: Value,
    skill_policy: Option<CommandSkillPolicy>,
}

#[derive(Clone, Copy)]
pub(crate) struct CommandSkillPolicy {
    pub(crate) heading: &'static str,
    pub(crate) guidance: &'static str,
}

fn command_behavior_descriptor(identity: CommandIdentity) -> CommandBehaviorDescriptor {
    CommandBehaviorDescriptor {
        contract: command_semantics(identity),
        behavior: command_behavior_contract(identity),
        failure_details: command_failure_details(identity),
        skill_policy: command_skill_policy(identity),
    }
}

pub(crate) fn skill_policy_for_command(command_name: &str) -> Option<CommandSkillPolicy> {
    let identity = CommandIdentity::from_path(command_name)?;
    command_behavior_descriptor(identity).skill_policy
}

fn command_skill_policy(identity: CommandIdentity) -> Option<CommandSkillPolicy> {
    match identity {
        CommandIdentity::Log => Some(CommandSkillPolicy {
            heading: "Mutation policy",
            guidance: "`log` creates a Tempo worklog. Start with `--dry-run`, verify the normalized issue, date, time, duration, and description, then execute without `--dry-run` only when the user's request explicitly authorizes creating the worklog.",
        }),
        CommandIdentity::List => Some(CommandSkillPolicy {
            heading: "Automation policy",
            guidance: "Use `drag --output json list` explicitly so an interactive terminal never opens. Use `--fields` to reduce structured output, and preserve `pagination.next` when another segment may be needed. `list` is read-only; its interactive human view can open a Jira URL only after an explicit keypress.",
        }),
        CommandIdentity::Delete => Some(CommandSkillPolicy {
            heading: "Destructive-operation policy",
            guidance: "`delete` permanently removes Tempo worklogs and a multi-ID deletion is not atomic. First run the exact IDs with `--dry-run`. Execute without `--dry-run` only when the user explicitly authorizes deleting those IDs. Never infer IDs from position or stale output.",
        }),
        CommandIdentity::Setup
        | CommandIdentity::Doctor
        | CommandIdentity::Tempo
        | CommandIdentity::Schema
        | CommandIdentity::GenerateSkills
        | CommandIdentity::Help => None,
    }
}

fn command_semantics(identity: CommandIdentity) -> CommandSemantics {
    let remote_errors = vec![
        "usage",
        "invalid_input",
        "not_configured",
        "config_error",
        "api_error",
        "openapi_document_error",
        "http_error",
        "invalid_url",
        "invalid_json",
        "io_error",
    ];
    let local_errors = vec!["usage", "invalid_input", "config_error", "io_error"];
    match identity {
        CommandIdentity::Help => CommandSemantics {
            success: json!({"type": "string", "format": "clapHelpText", "envelope": false}),
            error_codes: vec!["usage"],
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        CommandIdentity::Log => CommandSemantics {
            success: json!({"oneOf": [schema_ref("Worklog"), object_schema(
                &["dryRun", "issueKey", "request"],
                json!({"dryRun": {"const": true}, "issueKey": {"type": "string"}, "request": schema_ref("AddWorklogRequest")})
            )]}),
            error_codes: [
                remote_errors,
                vec![
                    "invalid_duration",
                    "invalid_date",
                    "invalid_time",
                    "non_positive_duration",
                ],
            ]
            .concat(),
            side_effects: json!({"default": ["createTempoWorklog"], "dryRun": []}),
            network_access: json!({"default": {"jira": "read", "tempo": "write"}, "dryRun": {}}),
            dry_run: json!({"supported": true, "option": "dryRun", "sideEffects": false, "networkAccess": false}),
        },
        CommandIdentity::List => CommandSemantics {
            success: object_schema(
                &["date", "worklogs", "schedule", "pagination"],
                json!({
                    "date": {"type": "string", "format": "date"},
                    "worklogs": {"type": "array", "items": schema_ref("Worklog")},
                    "schedule": schema_ref("ScheduleDetails"),
                    "pagination": schema_ref("ListPagination")
                }),
            ),
            error_codes: [remote_errors, vec!["invalid_date"]].concat(),
            side_effects: json!({
                "default": [],
                "interactive": ["openFocusedJiraUrlInLocalDefaultBrowser"]
            }),
            network_access: json!({
                "default": {"jira": "read", "tempo": "read"},
                "interactive": {"browser": "may-open", "github": "read", "jira": "read", "tempo": "read"}
            }),
            dry_run: unsupported_dry_run(),
        },
        CommandIdentity::Delete => CommandSemantics {
            success: object_schema(
                &["dryRun", "worklogs"],
                json!({"dryRun": {"type": "boolean"}, "worklogs": {"type": "array", "items": schema_ref("Worklog")}}),
            ),
            error_codes: remote_errors,
            side_effects: json!({
                "default": ["deleteTempoWorklogs"],
                "dryRun": [],
                "atomic": false,
                "processingOrder": "worklogIdsInInputOrder",
                "failure": "stopOnFirstError; previously deleted worklogs remain deleted and no success result is emitted"
            }),
            network_access: json!({"default": {"jira": "read", "tempo": "read-write"}, "dryRun": {"jira": "read", "tempo": "read"}}),
            dry_run: json!({"supported": true, "option": "dryRun", "sideEffects": false, "networkAccess": "read-only"}),
        },
        CommandIdentity::Setup => CommandSemantics {
            success: setup_success_schema(),
            error_codes: remote_errors,
            side_effects: json!({
                "default": ["mayOpenBrowserTokenPages", "verifyCredentials", "writeConfiguration"],
                "noOpen": ["verifyCredentials", "writeConfiguration"],
                "fromEnv": ["verifyCredentials", "writeConfiguration"],
                "fromEnvDryRun": [],
                "fromEnvDryRunVerify": ["verifyCredentialsReadOnly"]
            }),
            network_access: json!({
                "default": {"browser": "may-open", "jira": "read", "tempo": "read"},
                "noOpen": {"browser": "none", "jira": "read", "tempo": "read"},
                "fromEnv": {"browser": "none", "jira": "read", "tempo": "read"}
                ,"fromEnvDryRun": {"browser": "none", "jira": "none", "tempo": "none"}
                ,"fromEnvDryRunVerify": {"browser": "none", "jira": "read", "tempo": "read"}
            }),
            dry_run: json!({
                "supported": true,
                "option": "dryRun",
                "requires": ["fromEnv"],
                "verificationOption": "verify",
                "sideEffects": false,
                "networkAccess": {"default": false, "verify": "read-only"}
            }),
        },
        CommandIdentity::Doctor => CommandSemantics {
            success: doctor_success_schema(),
            error_codes: [remote_errors, vec!["remote_check_failed"]].concat(),
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}, "remote": {"jira": "read", "tempo": "read"}}),
            dry_run: unsupported_dry_run(),
        },
        CommandIdentity::Tempo => CommandSemantics {
            success: json!({"oneOf": [
                {"description": "The selected Tempo API response."},
                object_schema(
                    &["dryRun", "operationId", "method", "effect", "url", "body"],
                    json!({
                        "dryRun": {"const": true},
                        "operationId": {"type": "string"},
                        "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"]},
                        "effect": {"type": "string", "enum": ["read", "mutation", "ambiguous"]},
                        "url": {"type": "string", "format": "uri"},
                        "body": {}
                    }),
                )
            ]}),
            error_codes: [
                remote_errors,
                vec!["usage", "invalid_input", "invalid_json"],
            ]
            .concat(),
            side_effects: json!({
                "readOperation": [],
                "mutationOperation": ["selectedTempoApiMutation"],
                "ambiguousOperation": ["selectedTempoApiOperationMayMutate"],
                "dryRun": []
            }),
            network_access: json!({
                "readOperation": {"tempoOpenApi": "readOrCache", "tempo": "read"},
                "mutationOperation": {"tempoOpenApi": "readOrCache", "tempo": "write"},
                "ambiguousOperation": {"tempoOpenApi": "readOrCache", "tempo": "unknown"},
                "dryRun": {"tempoOpenApi": "readOrCache", "tempo": "none"}
            }),
            dry_run: json!({
                "supported": true,
                "scope": "generatedMethod",
                "sideEffects": false,
                "networkAccess": {"tempoOpenApi": "readOrCache", "tempo": false}
            }),
        },
        CommandIdentity::Schema => CommandSemantics {
            success: json!({"oneOf": [
                local_cli_contract_schema(),
                tempo_operation_schema_contract(),
                tempo_component_schema_contract()
            ]}),
            error_codes: [
                local_errors,
                vec![
                    "api_error",
                    "openapi_document_error",
                    "http_error",
                    "invalid_json",
                    "invalid_url",
                ],
            ]
            .concat(),
            side_effects: json!({"default": []}),
            network_access: json!({
                "default": {},
                "tempoPathCacheHit": {},
                "tempoPathCacheMissOrStale": {"tempoOpenApi": "read"}
            }),
            dry_run: unsupported_dry_run(),
        },
        CommandIdentity::GenerateSkills => CommandSemantics {
            success: object_schema(
                &["outputDir", "scope", "skills"],
                json!({
                    "outputDir": {"type": "string"},
                    "scope": {"type": "string", "enum": ["local", "tempo", "all"]},
                    "skills": {"type": "array", "items": {"type": "string"}}
                }),
            ),
            error_codes: [
                local_errors,
                vec!["api_error", "openapi_document_error", "http_error"],
            ]
            .concat(),
            side_effects: json!({"default": ["writeGeneratedSkillFiles"]}),
            network_access: json!({
                "local": {},
                "tempo": {"tempoOpenApi": "readOrCache"},
                "all": {"tempoOpenApi": "readOrCache"}
            }),
            dry_run: unsupported_dry_run(),
        },
    }
}

fn unsupported_dry_run() -> Value {
    json!({"supported": false})
}

fn command_behavior_contract(identity: CommandIdentity) -> Value {
    match identity {
        CommandIdentity::Help => json!({
            "target": "zero or more command names from the surrounding command tree",
            "output": "Clap help text on stdout without a JSON envelope"
        }),
        CommandIdentity::GenerateSkills => json!({
            "sourceOfTruth": {
                "local": "clapAndDragSchema",
                "tempo": "officialTempoOpenApi"
            },
            "failure": "renderBeforeWrite; Tempo discovery failure leaves generated files unchanged",
            "outputDirectory": "relativePathWithinCurrentWorkingDirectory"
        }),
        CommandIdentity::Log => json!({
            "dateDefault": "todayInConfiguredLocalTimeZone",
            "durationOrInterval": {
                "durationSyntax": ["15m", "1h", "1h15m"],
                "intervalSyntax": ["11-14", "11-14:30", "11:35-14:20", "11.35-14.20"],
                "overnight": "endAtOrBeforeStartUsesNextLocalDay"
            }
        }),
        CommandIdentity::List => json!({
            "dateDefault": "todayInConfiguredLocalTimeZone",
            "verbose": "adds descriptions and Jira URLs to human output only",
            "interactive": {
                "outputMode": "human",
                "terminalRequirements": ["stdin", "stdout", "stderr"],
                "allTerminalsRequired": true,
                "entry": "afterCompletedRetrieval",
                "renderStream": "stderr",
                "fallback": "completedPlainTextReport",
                "controls": {
                    "previousDate": PREVIOUS_DATE_KEY,
                    "nextDate": NEXT_DATE_KEY,
                    "moveUp": ["up", MOVE_UP_KEY],
                    "moveDown": ["down", MOVE_DOWN_KEY],
                    "openFocusedJiraIssue": OPEN_ISSUE_KEY,
                    "quit": [QUIT_KEY, "escape", format!("ctrl-{INTERRUPT_KEY}")]
                },
                "browser": {
                    "trigger": "explicitOpenFocusedJiraIssue",
                    "target": "resolvedJiraBrowseUrl",
                    "sideEffect": "openLocalDefaultBrowser",
                    "additionalApiRequestByDrag": false,
                    "remoteMutation": false,
                    "failure": "recoverableRedactedStatus"
                },
                "updateCheck": {
                    "source": "latestStableGitHubRelease",
                    "timing": "nonBlocking",
                    "failure": "silent",
                    "display": "brandHeaderWhenNewer"
                }
            },
            "automation": {
                "recommendation": "useExplicitJsonOutput",
                "arguments": ["--output", "json"],
                "interactive": false,
                "successContract": "unchangedListJsonContract"
            },
            "fieldSelection": {
                "option": "fields",
                "default": "allFields",
                "recommendation": "requestOnlyFieldsNeededForTask",
                "appliesTo": "structuredOutputOnly",
                "projection": "beforeSerialization",
                "ordering": "canonicalResultOrder",
                "separator": ",",
                "parentSelection": "selectsWholeSubtree",
                "overlappingParentsAndDescendants": "rejected",
                "allowedFields": ListField::paths().collect::<Vec<_>>()
            },
            "pagination": {
                "defaultRecordLimit": DEFAULT_RECORD_LIMIT,
                "defaultPageLimit": DEFAULT_PAGE_LIMIT,
                "continuationOption": "continueFrom",
                "allPagesOption": "allPages",
                "allPagesSafetyCeiling": HARD_PAGE_LIMIT,
                "boundedTotals": "schedule calculations use the retrieved segment; totalsComplete reports whether they cover the whole month",
                "selectionBinding": "continueFrom is an opaque token bound to the selected date, month range, and effective pagination plan; omitted bounds are restored and explicit mismatches fail before networking"
            },
            "streaming": {
                "outputMode": "ndjson",
                "eventDiscriminator": "kind",
                "terminalEvent": "pagination"
            }
        }),
        CommandIdentity::Setup => json!({
            "interactive": {
                "interface": "ratatui",
                "terminalRequired": true,
                "renderStream": "stderr",
                "events": "asynchronousCrossterm",
                "stages": ["jiraAccountDetails", "atlassianApiToken", "tempoAccount", "reviewAndSave"],
                "reducedMotionEnvironment": REDUCED_MOTION_ENV
            },
            "fromEnv": {
                "interactive": false,
                "requiredEnvironment": [ATLASSIAN_HOST_ENV, ATLASSIAN_EMAIL_ENV, ATLASSIAN_TOKEN_ENV, TEMPO_TOKEN_ENV],
                "secretTransport": "environmentOnly",
                "dryRun": "validateAndPlanWithoutWriting",
                "dryRunVerification": "plannedUnlessVerifyIsSet",
                "verificationRequires": ["fromEnv", "dryRun"]
            },
            "browser": {
                "default": "openEachTokenPageOnExplicitTokenStageEntry",
                "beforeTokenStage": false,
                "failure": "warning",
                "noOpen": "printLinksWithoutOpening",
                "fromEnv": false
            },
            "accountId": {
                "setup": "derivedFromVerifiedJiraUser",
                "runtimeCompatibilityEnvironment": TEMPO_ACCOUNT_ID_ENV
            },
            "writesConfiguration": "onceAfterVerification"
        }),
        CommandIdentity::Doctor => json!({
            "remote": "opt-in read-only Jira and Tempo checks",
            "remoteStatuses": ["connected", "notConfigured", "failed"]
        }),
        CommandIdentity::Schema => json!({
            "withoutPath": "returns the complete local Drag contract",
            "tempoPath": "returns one fixed-origin Tempo OpenAPI operation",
            "tempoComponentPath": "returns one fixed-origin Tempo OpenAPI component schema",
            "pathSyntax": ["tempo.<Schema>", "tempo.<resource>.<method>"],
            "resolveRefs": "inlines bounded local OpenAPI component references"
        }),
        CommandIdentity::Tempo => json!({
            "commandTree": "generatedAtRuntimeFromOfficialTempoOpenApi",
            "currentMethods": "TempoApiV4Operations",
            "resourceNames": "normalizedOpenApiTags",
            "methodNames": "normalizedOperationIdsWithUnambiguousFriendlyAliases",
            "parameters": "JSON object passed through --params and validated against OpenAPI",
            "requestBody": "application/json body passed through --json and validated against OpenAPI",
            "help": "generatedAfterOpenApiDiscovery",
            "dryRun": "validatesAndPrintsWithoutTempoApiAccess"
        }),
        CommandIdentity::Delete => json!({}),
    }
}

fn tempo_operation_schema_contract() -> Value {
    object_schema(
        &["path", "source", "operation"],
        json!({
            "path": {"type": "string", "pattern": "^tempo\\.[a-z0-9-]+\\.[a-z0-9-]+$"},
            "source": tempo_schema_source_contract(),
            "operation": {"type": "object"}
        }),
    )
}

fn local_cli_contract_schema() -> Value {
    json!({
        "type": "object",
        "description": "The complete local Drag contract when no path is supplied.",
        "required": ["schemaDialect", "schemaVersion", "cliVersion", "name", "commands"],
        "properties": {
            "schemaDialect": {"type": "string"},
            "schemaVersion": {"type": "integer"},
            "cliVersion": {"type": "string"},
            "name": {"const": "drag"},
            "commands": {"type": "object"}
        }
    })
}

fn tempo_component_schema_contract() -> Value {
    object_schema(
        &["path", "source", "schema"],
        json!({
            "path": {"type": "string", "pattern": "^tempo\\.[A-Za-z0-9_-]+$"},
            "source": tempo_schema_source_contract(),
            "schema": {"type": "object"}
        }),
    )
}

fn tempo_schema_source_contract() -> Value {
    object_schema(
        &["kind", "url", "openapi", "cached"],
        json!({
            "kind": {"const": "tempoOpenApi"},
            "url": {"const": "https://apidocs.tempo.io/tempo-openapi.yaml"},
            "openapi": {"type": "string"},
            "cached": {"type": "boolean"}
        }),
    )
}

fn object_schema(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object",
        "required": required,
        "properties": properties,
        "additionalProperties": false
    })
}

fn projected_object_schema(properties: Value) -> Value {
    json!({
        "type": "object",
        "minProperties": 1,
        "properties": properties,
        "additionalProperties": false
    })
}

fn nullable_schema(schema: Value) -> Value {
    json!({"anyOf": [schema, {"type": "null"}]})
}

fn json_schema<T: JsonSchema>() -> Value {
    json!(schema_for!(T))
}

fn serialization_schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    json!(generator.into_root_schema_for::<T>())
}

fn schema_ref(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

fn shared_definitions() -> Value {
    let mut definitions = Map::new();
    add_definition::<Worklog>(&mut definitions, "Worklog");
    add_definition::<AddWorklogRequest>(&mut definitions, "AddWorklogRequest");
    add_definition::<ScheduleDetails>(&mut definitions, "ScheduleDetails");
    add_serialization_definition::<ListPagination>(&mut definitions, "ListPagination");
    add_list_projection_definitions(&mut definitions);
    add_list_stream_definitions(&mut definitions);
    Value::Object(definitions)
}

fn add_list_stream_definitions(definitions: &mut Map<String, Value>) {
    definitions.insert(
        "ListWorklogEvent".to_owned(),
        object_schema(
            &["kind", "worklog"],
            json!({
                "kind": {"const": "worklog"},
                "worklog": schema_ref("ProjectedWorklog")
            }),
        ),
    );
    definitions.insert(
        "ListSummaryEvent".to_owned(),
        object_schema(
            &["kind"],
            json!({
                "kind": {"const": "summary"},
                "date": {"type": "string", "format": "date"},
                "schedule": schema_ref("ProjectedScheduleDetails")
            }),
        ),
    );
    definitions.insert(
        "ListPaginationEvent".to_owned(),
        object_schema(
            &["kind"],
            json!({
                "kind": {"const": "pagination"},
                "pagination": schema_ref("ProjectedListPagination")
            }),
        ),
    );
}

fn list_stream_contract() -> Value {
    json!({
        "outputMode": "ndjson",
        "mediaType": "application/x-ndjson",
        "discriminator": "kind",
        "eventOrder": ["zeroOrMoreWorklog", "summary", "pagination"],
        "events": {
            "worklog": schema_ref("ListWorklogEvent"),
            "summary": schema_ref("ListSummaryEvent"),
            "pagination": schema_ref("ListPaginationEvent")
        },
        "fieldSelection": "projects each event payload before serialization; kind is always present; worklog events and Jira enrichment are omitted when no worklog fields are selected; Tempo-only fields avoid Jira enrichment",
        "pageEmission": "worklog events are flushed before requesting the next Tempo page",
        "emptyResult": ["summary", "pagination"],
        "terminalEvent": "pagination",
        "failureStream": "stderrErrorEnvelope",
        "midStreamFailure": "network or enrichment failure stops without summary or terminal events; prior stdout lines remain valid",
        "brokenPipe": "clean successful termination"
    })
}

fn add_list_projection_definitions(definitions: &mut Map<String, Value>) {
    definitions.insert(
        "ProjectedClockInterval".to_owned(),
        projected_object_schema(json!({
            "startTime": {"type": "string"},
            "endTime": {"type": "string"}
        })),
    );
    definitions.insert(
        "ProjectedWorklog".to_owned(),
        projected_object_schema(json!({
            "id": {"type": "string"},
            "interval": nullable_schema(schema_ref("ProjectedClockInterval")),
            "issueId": {"type": "string"},
            "issueKey": {"type": "string"},
            "duration": {"type": "string"},
            "description": {"type": "string"},
            "link": {"type": "string"}
        })),
    );
    definitions.insert(
        "ProjectedScheduleDetails".to_owned(),
        projected_object_schema(json!({
            "monthRequiredDuration": {"type": "string"},
            "monthLoggedDuration": {"type": "string"},
            "monthCurrentPeriodDuration": {"type": "string"},
            "dayRequiredDuration": {"type": "string"},
            "dayLoggedDuration": {"type": "string"}
        })),
    );
    definitions.insert(
        "ProjectedListPagination".to_owned(),
        projected_object_schema(json!({
            "selectedDate": {"type": "string", "format": "date"},
            "monthStart": {"type": "string", "format": "date"},
            "monthEnd": {"type": "string", "format": "date"},
            "limit": nullable_schema(json!({"type": "integer", "minimum": 1, "maximum": MAX_RECORD_LIMIT})),
            "pageLimit": {"type": "integer", "minimum": 1, "maximum": HARD_PAGE_LIMIT},
            "allPages": {"type": "boolean"},
            "pagesRetrieved": {"type": "integer", "minimum": 1, "maximum": HARD_PAGE_LIMIT},
            "recordsRetrieved": {"type": "integer", "minimum": 0},
            "recordsReturned": {"type": "integer", "minimum": 0},
            "next": nullable_schema(json!({"type": "string"})),
            "complete": {"type": "boolean"},
            "totalsComplete": {"type": "boolean"}
        })),
    );
    definitions.insert(
        "ProjectedListResult".to_owned(),
        projected_object_schema(json!({
            "date": {"type": "string", "format": "date"},
            "worklogs": {"type": "array", "items": schema_ref("ProjectedWorklog")},
            "schedule": schema_ref("ProjectedScheduleDetails"),
            "pagination": schema_ref("ProjectedListPagination")
        })),
    );
}

fn add_definition<T: JsonSchema>(definitions: &mut Map<String, Value>, name: &str) {
    add_schema_definition(definitions, name, json_schema::<T>());
}

fn add_serialization_definition<T: JsonSchema>(definitions: &mut Map<String, Value>, name: &str) {
    add_schema_definition(definitions, name, serialization_schema::<T>());
}

fn add_schema_definition(definitions: &mut Map<String, Value>, name: &str, mut schema: Value) {
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        if let Some(Value::Object(nested)) = object.remove("$defs") {
            for (nested_name, nested_schema) in nested {
                definitions.entry(nested_name).or_insert(nested_schema);
            }
        }
        if name == "Worklog" {
            let required = object.entry("required").or_insert_with(|| json!([]));
            if let Some(required) = required.as_array_mut() {
                let interval = Value::String("interval".to_owned());
                if !required.contains(&interval) {
                    required.push(interval);
                }
            }
        }
    }
    definitions.insert(name.to_owned(), schema);
}

fn setup_success_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "required": ["configured", "path", "source", "verification"],
                "properties": {
                    "configured": {"const": true},
                    "path": {"type": "string"},
                    "source": {"const": "environment"},
                    "verification": {
                        "type": "object",
                        "required": ["jira", "tempo"],
                        "properties": {"jira": {"const": "connected"}, "tempo": {"const": "connected"}},
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false
            },
            {
                "type": "object",
                "required": ["configured", "path", "source", "connection"],
                "properties": {
                    "configured": {"const": true},
                    "path": {"type": "string"},
                    "source": {"const": "interactive"},
                    "connection": {
                        "type": "object",
                        "required": ["jira", "tempo"],
                        "properties": {
                            "jira": {"type": "object", "required": ["status", "hostname", "email"], "properties": {"status": {"const": "connected"}, "hostname": {"type": "string"}, "email": {"type": "string"}}, "additionalProperties": false},
                            "tempo": {"type": "object", "required": ["status"], "properties": {"status": {"const": "connected"}}, "additionalProperties": false}
                        },
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false
            },
            {
                "type": "object",
                "required": ["configured", "dryRun", "path", "source", "localValidation", "remoteVerification", "configuration"],
                "properties": {
                    "configured": {"const": false},
                    "dryRun": {"const": true},
                    "path": {"type": "string"},
                    "source": {"const": "environment"},
                    "localValidation": object_schema(&["status"], json!({"status": {"const": "passed"}})),
                    "remoteVerification": {
                        "oneOf": [
                            object_schema(&["status", "jira", "tempo"], json!({"status": {"const": "planned"}, "jira": {"const": "read-only"}, "tempo": {"const": "read-only"}})),
                            object_schema(&["status", "jira", "tempo"], json!({"status": {"const": "completed"}, "jira": {"const": "connected"}, "tempo": {"const": "connected"}}))
                        ]
                    },
                    "configuration": object_schema(&["status", "credentials"], json!({"status": {"const": "planned"}, "credentials": {"const": "replace"}}))
                },
                "additionalProperties": false
            }
        ]
    })
}

fn doctor_success_schema() -> Value {
    object_schema(
        &[
            "name",
            "version",
            "configPath",
            "configured",
            "timezone",
            "target",
        ],
        json!({
            "name": {"const": "drag"},
            "version": {"type": "string"},
            "configPath": {"type": "string"},
            "configured": {
                "type": "object",
                "required": ["tempoToken", "accountId", "atlassianEmail", "atlassianToken", "atlassianHost"],
                "properties": {
                    "tempoToken": {"type": "boolean"},
                    "accountId": {"type": "boolean"},
                    "atlassianEmail": {"type": "boolean"},
                    "atlassianToken": {"type": "boolean"},
                    "atlassianHost": {"type": "boolean"}
                },
                "additionalProperties": false
            },
            "timezone": {"type": "string"},
            "target": {"type": "object", "required": ["architecture", "operatingSystem"], "properties": {"architecture": {"type": "string"}, "operatingSystem": {"type": "string"}}, "additionalProperties": false},
            "remoteChecks": {"type": "object", "required": ["jira", "tempo"], "properties": {"jira": service_check_schema(), "tempo": service_check_schema()}, "additionalProperties": false}
        }),
    )
}

fn service_check_schema() -> Value {
    json!({
        "oneOf": [
            object_schema(&["status"], json!({"status": {"const": "connected"}})),
            object_schema(&["status"], json!({"status": {"const": "notConfigured"}})),
            object_schema(&["status", "errorCode"], json!({"status": {"const": "failed"}, "errorCode": {"type": "string"}}))
        ]
    })
}

fn command_failure_details(identity: CommandIdentity) -> Value {
    match identity {
        CommandIdentity::Doctor => json!({"remote_check_failed": doctor_success_schema()}),
        CommandIdentity::Log
        | CommandIdentity::List
        | CommandIdentity::Delete
        | CommandIdentity::Setup
        | CommandIdentity::Tempo
        | CommandIdentity::Schema
        | CommandIdentity::GenerateSkills
        | CommandIdentity::Help => Value::Null,
    }
}

fn output_contract() -> Value {
    json!({
        "modes": {
            "auto": "human on a stdout TTY; otherwise json",
            "human": "human-readable text",
            "json": "one JSON document",
            "ndjson": "newline-delimited list events"
        },
        "modeConstraints": {
            "ndjson": {
                "commands": ["list"],
                "otherwise": {"errorCode": "invalid_input", "exitCode": 2}
            }
        },
        "successStream": "stdout",
        "errorStream": "stderr",
        "humanDiagnosticsStream": "stderr",
        "clapHelpAndVersion": "plain text on stdout without a JSON envelope",
        "successEnvelope": {
            "type": "object",
            "required": ["ok", "data"],
            "properties": {"ok": {"const": true}, "data": {}},
            "additionalProperties": false
        },
        "errorEnvelope": {
            "type": "object",
            "required": ["ok", "error"],
            "properties": {
                "ok": {"const": false},
                "error": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {"code": {"type": "string"}, "message": {"type": "string"}, "details": {}},
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }
    })
}

fn error_contract() -> Value {
    json!({
        "exitCodes": {"0": "success", "1": "runtime failure", "2": "usage or invalid input"},
        "codes": {
            "usage": 2,
            "invalid_input": 2,
            "invalid_duration": 2,
            "invalid_date": 2,
            "invalid_time": 2,
            "non_positive_duration": 2,
            "not_configured": 2,
            "invalid_url": 2,
            "invalid_json": 2,
            "config_error": 1,
            "api_error": 1,
            "openapi_document_error": 1,
            "http_error": 1,
            "io_error": 1,
            "remote_check_failed": {
                "exitCodes": [1, 2],
                "selection": "most severe failed remote check"
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};
    use drag::pagination::{DEFAULT_PAGE_LIMIT, DEFAULT_RECORD_LIMIT, HARD_PAGE_LIMIT};
    use serde_json::Value;

    use super::{command_behavior_descriptor, schema, CommandIdentity, SCHEMA_VERSION};
    use crate::cli::{Cli, LogInput};
    use crate::list_tui::{
        INTERRUPT_KEY, MOVE_DOWN_KEY, MOVE_UP_KEY, NEXT_DATE_KEY, OPEN_ISSUE_KEY,
        PREVIOUS_DATE_KEY, QUIT_KEY,
    };

    #[test]
    fn every_clap_command_has_an_explicit_typed_identity() {
        let clap = Cli::command();
        for command in clap.get_subcommands() {
            assert!(
                CommandIdentity::from_path(command.get_name()).is_some(),
                "{} has no command semantics",
                command.get_name()
            );
        }
    }

    #[test]
    fn safety_policy_and_machine_contract_share_command_descriptors() {
        let log = command_behavior_descriptor(CommandIdentity::Log);
        assert_eq!(
            log.contract.side_effects["default"][0],
            "createTempoWorklog"
        );
        assert!(log
            .skill_policy
            .is_some_and(|policy| policy.guidance.contains("Start with `--dry-run`")));

        let list = command_behavior_descriptor(CommandIdentity::List);
        assert_eq!(list.contract.network_access["default"]["tempo"], "read");
        assert!(list
            .skill_policy
            .is_some_and(|policy| policy.guidance.contains("`list` is read-only")));

        let delete = command_behavior_descriptor(CommandIdentity::Delete);
        assert_eq!(delete.contract.side_effects["atomic"], false);
        assert!(delete
            .skill_policy
            .is_some_and(|policy| policy.guidance.contains("not atomic")));
    }

    #[test]
    fn list_behavior_uses_runtime_policy_and_control_constants() {
        let rendered = schema();
        let behavior = &rendered.data["commands"]["list"]["behavior"];

        assert_eq!(
            behavior["pagination"]["defaultRecordLimit"],
            DEFAULT_RECORD_LIMIT
        );
        assert_eq!(
            behavior["pagination"]["defaultPageLimit"],
            DEFAULT_PAGE_LIMIT
        );
        assert_eq!(
            behavior["pagination"]["allPagesSafetyCeiling"],
            HARD_PAGE_LIMIT
        );
        assert_eq!(
            behavior["interactive"]["controls"],
            serde_json::json!({
                "previousDate": PREVIOUS_DATE_KEY,
                "nextDate": NEXT_DATE_KEY,
                "moveUp": ["up", MOVE_UP_KEY],
                "moveDown": ["down", MOVE_DOWN_KEY],
                "openFocusedJiraIssue": OPEN_ISSUE_KEY,
                "quit": [QUIT_KEY, "escape", format!("ctrl-{INTERRUPT_KEY}")]
            })
        );
    }

    #[test]
    fn contract_covers_every_clap_command_and_alias() {
        let rendered = schema();
        let commands = &rendered.data["commands"];
        let clap = Cli::command();
        for command in clap.get_subcommands() {
            let contract = &commands[command.get_name()];
            assert!(
                contract.is_object(),
                "missing {} contract",
                command.get_name()
            );
            let aliases = contract["aliases"].as_array().cloned().unwrap_or_default();
            for alias in command.get_all_aliases() {
                assert!(
                    aliases.contains(&Value::String(alias.to_owned())),
                    "missing alias {alias}"
                );
            }
            if command.get_name() != "help" {
                for subcommand in command.get_subcommands() {
                    assert!(contract["subcommands"][subcommand.get_name()].is_object());
                }
            }
        }
    }

    #[test]
    fn contract_arguments_are_derived_from_clap() {
        let rendered = schema();
        let clap = Cli::command();
        for command in clap.get_subcommands() {
            let declared = rendered.data["commands"][command.get_name()]["arguments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for argument in command.get_arguments().filter(|argument| {
                !argument.is_global_set()
                    && !matches!(argument.get_id().as_str(), "help" | "version")
            }) {
                assert!(
                    declared
                        .iter()
                        .any(|item| item["id"] == argument.get_id().as_str()),
                    "missing {} argument {}",
                    command.get_name(),
                    argument.get_id()
                );
            }
        }
    }

    #[test]
    fn log_json_schema_tracks_serde_fields_and_unknown_field_policy() -> Result<(), String> {
        let rendered = schema();
        let json_argument = rendered.data["commands"]["log"]["arguments"]
            .as_array()
            .and_then(|arguments| arguments.iter().find(|argument| argument["id"] == "json"))
            .ok_or_else(|| "missing log json argument".to_owned())?;
        let input_schema = &json_argument["jsonSchema"];
        assert_eq!(input_schema["additionalProperties"], false);
        assert_eq!(
            input_schema["required"],
            serde_json::json!(["issueKey", "durationOrInterval"])
        );
        for field in ["when", "description", "start", "remainingEstimate"] {
            assert!(
                input_schema["properties"][field].is_object(),
                "missing {field}"
            );
        }
        serde_json::from_value::<LogInput>(serde_json::json!({
            "issueKey": "ABC-1",
            "durationOrInterval": "30m"
        }))
        .map_err(|error| error.to_string())?;
        assert!(serde_json::from_value::<LogInput>(serde_json::json!({
            "issueKey": "ABC-1",
            "durationOrInterval": "30m",
            "unexpected": true
        }))
        .is_err());
        Ok(())
    }

    #[test]
    fn declared_log_conditions_match_accepted_clap_forms() -> Result<(), String> {
        let rendered = schema();
        let arguments = rendered.data["commands"]["log"]["arguments"]
            .as_array()
            .ok_or_else(|| "log arguments are not an array".to_owned())?;
        for id in ["issue_key", "duration_or_interval"] {
            let argument = arguments
                .iter()
                .find(|argument| argument["id"] == id)
                .ok_or_else(|| format!("missing {id}"))?;
            assert_eq!(
                argument["requiredUnlessPresent"],
                serde_json::json!(["json"])
            );
        }
        Cli::try_parse_from(["drag", "log", "ABC-1", "30m"]).map_err(|error| error.to_string())?;
        Cli::try_parse_from([
            "drag",
            "log",
            "--json",
            r#"{"issueKey":"ABC-1","durationOrInterval":"30m"}"#,
        ])
        .map_err(|error| error.to_string())?;
        assert!(Cli::try_parse_from(["drag", "log"]).is_err());
        assert!(Cli::try_parse_from(["drag", "log", "ABC-1", "30m", "--json", "{}"]).is_err());
        Ok(())
    }

    #[test]
    fn contract_has_explicit_versions_results_errors_and_behavior() {
        let rendered = schema();
        assert_eq!(rendered.data["schemaVersion"], SCHEMA_VERSION);
        assert_eq!(rendered.data["cliVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            rendered.data["schemaDialect"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(rendered.data.get("$schema").is_none());
        for command in rendered.data["commands"]
            .as_object()
            .into_iter()
            .flat_map(|commands| commands.values())
        {
            assert!(command.get("success").is_some());
            assert!(command["errorCodes"].is_array());
            assert!(command["sideEffects"].is_object());
            assert!(command["networkAccess"].is_object());
            assert!(command["dryRun"].is_object());
            assert!(command.get("failureDetails").is_some());
        }
    }

    #[test]
    fn shared_result_schema_references_resolve_and_track_serialized_models() {
        let rendered = schema();
        let definitions = &rendered.data["$defs"];
        assert!(definitions["Worklog"]["properties"]["interval"].is_object());
        assert!(definitions["Worklog"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&Value::String("interval".to_owned()))));
        assert!(definitions["Worklog"]["properties"]["interval"]["anyOf"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(|variant| variant["type"] == "null")));
        assert!(definitions["AddWorklogRequest"]["properties"]["timeSpentSeconds"].is_object());
        assert!(definitions["ScheduleDetails"]["properties"]["dayLoggedDuration"].is_object());
        assert_eq!(
            rendered.data["commands"]["list"]["success"]["properties"]["worklogs"]["items"]["$ref"],
            "#/$defs/Worklog"
        );
    }

    #[test]
    fn declared_numeric_and_enum_types_match_clap_parsing() -> Result<(), String> {
        let rendered = schema();
        let delete_arguments = rendered.data["commands"]["delete"]["arguments"]
            .as_array()
            .ok_or_else(|| "delete arguments are not an array".to_owned())?;
        let ids = delete_arguments
            .iter()
            .find(|argument| argument["id"] == "worklog_ids")
            .ok_or_else(|| "missing worklog IDs".to_owned())?;
        assert_eq!(ids["type"], "unsignedInteger");
        assert_eq!(ids["required"], false);
        assert_eq!(ids["requiredUnlessPresent"], serde_json::json!(["json"]));
        assert_eq!(ids["valueCount"]["minimum"], 1);
        assert!(ids["valueCount"]["maximum"].is_null());
        Cli::try_parse_from(["drag", "delete", "1", "2"]).map_err(|error| error.to_string())?;
        Cli::try_parse_from(["drag", "delete", "--json", r#"{"worklogIds":[1,2]}"#])
            .map_err(|error| error.to_string())?;
        assert!(Cli::try_parse_from(["drag", "delete", "not-a-number"]).is_err());

        let output = rendered.data["globalOptions"]
            .as_array()
            .and_then(|arguments| arguments.iter().find(|argument| argument["id"] == "output"))
            .ok_or_else(|| "missing output option".to_owned())?;
        assert_eq!(
            output["enum"],
            serde_json::json!(["auto", "human", "json", "ndjson"])
        );
        assert_eq!(output["default"], "auto");
        assert!(Cli::try_parse_from(["drag", "--output", "xml", "schema"]).is_err());
        Ok(())
    }

    #[test]
    fn switches_use_boolean_defaults_without_value_enums() -> Result<(), String> {
        let rendered = schema();
        let dry_run = rendered.data["commands"]["log"]["arguments"]
            .as_array()
            .and_then(|arguments| {
                arguments
                    .iter()
                    .find(|argument| argument["id"] == "dry_run")
            })
            .ok_or_else(|| "missing log dry-run switch".to_owned())?;
        assert_eq!(dry_run["type"], "boolean");
        assert_eq!(dry_run["default"], false);
        assert!(dry_run.get("enum").is_none());
        assert_eq!(dry_run["valueCount"]["maximum"], 0);

        for id in ["help", "version"] {
            let switch = rendered.data["globalOptions"]
                .as_array()
                .and_then(|arguments| arguments.iter().find(|argument| argument["id"] == id))
                .ok_or_else(|| format!("missing {id} switch"))?;
            assert_eq!(switch["type"], "boolean");
            assert!(switch.get("enum").is_none());
        }
        assert!(Cli::try_parse_from(["drag", "log", "ABC-1", "30m", "--dry-run=true"]).is_err());
        Ok(())
    }

    #[test]
    fn safety_sensitive_command_variants_are_explicit() -> Result<(), String> {
        let rendered = schema();
        let commands = &rendered.data["commands"];
        assert_eq!(commands["delete"]["sideEffects"]["atomic"], false);
        assert_eq!(
            commands["delete"]["sideEffects"]["processingOrder"],
            "worklogIdsInInputOrder"
        );
        assert_eq!(
            commands["setup"]["networkAccess"]["noOpen"]["browser"],
            "none"
        );
        assert_eq!(
            commands["setup"]["sideEffects"]["noOpen"],
            serde_json::json!(["verifyCredentials", "writeConfiguration"])
        );
        assert_eq!(commands["setup"]["dryRun"]["supported"], true);
        let setup_arguments = commands["setup"]["arguments"]
            .as_array()
            .ok_or_else(|| "setup arguments must be an array".to_owned())?;
        let verify = setup_arguments
            .iter()
            .find(|argument| argument["id"] == "verify")
            .ok_or_else(|| "setup verify argument must be documented".to_owned())?;
        assert_eq!(verify["requires"], serde_json::json!(["fromEnv", "dryRun"]));
        assert_eq!(
            commands["setup"]["networkAccess"]["fromEnvDryRun"],
            serde_json::json!({"browser": "none", "jira": "none", "tempo": "none"})
        );
        assert_eq!(
            commands["setup"]["networkAccess"]["fromEnvDryRunVerify"],
            serde_json::json!({"browser": "none", "jira": "read", "tempo": "read"})
        );
        assert!(Cli::try_parse_from(["drag", "setup", "--dry-run"]).is_err());
        assert!(Cli::try_parse_from(["drag", "setup", "--verify"]).is_err());
        assert!(
            Cli::try_parse_from(["drag", "setup", "--from-env", "--dry-run", "--verify"]).is_ok()
        );
        assert_eq!(
            commands["doctor"]["failureDetails"]["remote_check_failed"],
            commands["doctor"]["success"]
        );
        Ok(())
    }

    #[test]
    fn doctor_and_help_contracts_cover_conditional_shapes_and_aliases() {
        let rendered = schema();
        let doctor = &rendered.data["commands"]["doctor"]["success"];
        assert_eq!(
            doctor["properties"]["configured"]["required"],
            serde_json::json!([
                "tempoToken",
                "accountId",
                "atlassianEmail",
                "atlassianToken",
                "atlassianHost"
            ])
        );
        assert_eq!(
            doctor["properties"]["remoteChecks"]["required"],
            serde_json::json!(["jira", "tempo"])
        );
        assert_eq!(
            doctor["properties"]["remoteChecks"]["properties"]["jira"]["oneOf"][2]["required"],
            serde_json::json!(["status", "errorCode"])
        );

        let targets = rendered.data["commands"]["help"]["helpTargets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for alias in ["l", "ls", "d"] {
            assert!(
                targets.contains(&Value::String(alias.to_owned())),
                "missing {alias}"
            );
        }
    }
}
