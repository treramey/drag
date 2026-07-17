use clap::{Arg, ArgAction, Command, CommandFactory};
use drag::models::{AddWorklogRequest, Worklog};
use drag::schedule::ScheduleDetails;
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Map, Value};

use crate::cli::{Cli, LogInput};
use crate::output::Rendered;

const SCHEMA_VERSION: u64 = 2;

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
        "$schema": "https://json-schema.org/draft/2020-12/schema",
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
            "DRAG_CONFIG": {"type": "path", "purpose": "Override the configuration file."},
            "DRAG_REDUCED_MOTION": {"type": "boolean-like", "purpose": "Reduce interactive setup motion."},
            "TEMPO_TOKEN": {"type": "secret", "purpose": "Override the stored Tempo token."},
            "TEMPO_ACCOUNT_ID": {"type": "string", "purpose": "Runtime compatibility override for the Tempo account ID."},
            "ATLASSIAN_EMAIL": {"type": "string", "purpose": "Override the stored Atlassian email."},
            "ATLASSIAN_TOKEN": {"type": "secret", "purpose": "Override the stored Atlassian API token."},
            "ATLASSIAN_HOST": {"type": "https-host", "purpose": "Override the stored Atlassian host."}
        },
        "syntax": {
            "date": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"],
            "duration": ["15m", "1h", "1h15m"],
            "interval": ["11-14", "11-14:30", "11:35-14:20", "11.35-14.20", "23:30-00:30"]
        }
    });

    Rendered::new(
        data,
        "Use `drag --output json schema` for the full CLI contract.".to_owned(),
    )
}

fn command_contract(command: &Command, path: &str) -> Value {
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
    let semantics = command_semantics(path);

    let mut contract = json!({
        "path": path,
        "description": command.get_about().map(ToString::to_string),
        "aliases": aliases,
        "hidden": command.is_hide_set(),
        "arguments": argument_contracts(command, path, false),
        "subcommands": subcommands,
        "success": semantics.success,
        "errorCodes": semantics.error_codes,
        "sideEffects": semantics.side_effects,
        "networkAccess": semantics.network_access,
        "dryRun": semantics.dry_run,
        "behavior": command_behavior(path)
    });
    if command.get_name() == "help" {
        contract["helpTargets"] = json!(help_targets(command));
    }
    contract
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
            collect(subcommand, &path, targets);
        }
    }

    let mut targets = Vec::new();
    collect(command, "", &mut targets);
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
    let possible_values = argument
        .get_value_parser()
        .possible_values()
        .map(|values| {
            values
                .map(|value| value.get_name().to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
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
        contract["default"] = json!(defaults[0]);
    } else if !defaults.is_empty() {
        contract["default"] = json!(defaults);
    }
    if let Some(required_unless) = required_unless(path, id) {
        contract["requiredUnlessPresent"] = json!(required_unless);
    }
    if let Some(default) = semantic_default(path, id) {
        contract["semanticDefault"] = json!(default);
    }
    if path == "log" && id == "json" {
        contract["stdinValue"] = json!("-");
        contract["jsonSchema"] = json_schema::<LogInput>();
    }
    contract
}

fn argument_type(path: &str, argument: &Arg, possible_values: &[String]) -> &'static str {
    if matches!(
        argument.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse
    ) {
        "boolean"
    } else if path == "delete" && argument.get_id() == "worklog_ids" {
        "unsignedInteger"
    } else if argument.get_id() == "config" {
        "path"
    } else if !possible_values.is_empty() {
        "enum"
    } else {
        "string"
    }
}

fn required_unless(path: &str, id: &str) -> Option<Vec<&'static str>> {
    match (path, id) {
        ("log", "issue_key_or_alias" | "duration_or_interval") => Some(vec!["json"]),
        _ => None,
    }
}

fn semantic_default(path: &str, id: &str) -> Option<&'static str> {
    match (path, id) {
        ("log" | "list", "when") => Some("todayInConfiguredLocalTimeZone"),
        ("completions", "shell") => Some("detectedFromShellEnvironmentOrBash"),
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

fn command_semantics(path: &str) -> CommandSemantics {
    let remote_errors = vec![
        "usage",
        "invalid_input",
        "not_configured",
        "config_error",
        "api_error",
        "http_error",
        "invalid_url",
        "invalid_json",
        "io_error",
    ];
    let local_errors = vec!["usage", "invalid_input", "config_error", "io_error"];
    if path == "help" || path.ends_with(" help") {
        return CommandSemantics {
            success: json!({"type": "string", "format": "clapHelpText", "envelope": false}),
            error_codes: vec!["usage"],
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        };
    }
    match path {
        "log" => CommandSemantics {
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
        "list" => CommandSemantics {
            success: object_schema(
                &["date", "worklogs", "schedule"],
                json!({"date": {"type": "string", "format": "date"}, "worklogs": {"type": "array", "items": schema_ref("Worklog")}, "schedule": schema_ref("ScheduleDetails")}),
            ),
            error_codes: [remote_errors, vec!["invalid_date"]].concat(),
            side_effects: json!({"default": []}),
            network_access: json!({"default": {"jira": "read", "tempo": "read"}}),
            dry_run: unsupported_dry_run(),
        },
        "delete" => CommandSemantics {
            success: object_schema(
                &["dryRun", "worklogs"],
                json!({"dryRun": {"type": "boolean"}, "worklogs": {"type": "array", "items": schema_ref("Worklog")}}),
            ),
            error_codes: remote_errors,
            side_effects: json!({"default": ["deleteTempoWorklogs"], "dryRun": []}),
            network_access: json!({"default": {"jira": "read", "tempo": "read-write"}, "dryRun": {"jira": "read", "tempo": "read"}}),
            dry_run: json!({"supported": true, "option": "dryRun", "sideEffects": false, "networkAccess": "read-only"}),
        },
        "setup" => CommandSemantics {
            success: setup_success_schema(),
            error_codes: remote_errors,
            side_effects: json!({"default": ["verifyCredentials", "writeConfiguration"], "preservesConfiguration": ["aliases"]}),
            network_access: json!({"default": {"browser": "may-open", "jira": "read", "tempo": "read"}, "fromEnv": {"browser": "none", "jira": "read", "tempo": "read"}}),
            dry_run: unsupported_dry_run(),
        },
        "alias set" | "alias:set" => CommandSemantics {
            success: object_schema(
                &["alias", "issueKey"],
                json!({"alias": {"type": "string"}, "issueKey": {"type": "string"}}),
            ),
            error_codes: local_errors,
            side_effects: json!({"default": ["writeConfiguration", "createOrReplaceAlias"]}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        "alias delete" | "alias:delete" => CommandSemantics {
            success: object_schema(
                &["alias", "deleted", "issueKey"],
                json!({"alias": {"type": "string"}, "deleted": {"type": "boolean"}, "issueKey": {"type": ["string", "null"]}}),
            ),
            error_codes: local_errors,
            side_effects: json!({"default": ["writeConfiguration", "deleteAliasIfPresent"]}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        "alias list" | "alias:list" => CommandSemantics {
            success: object_schema(
                &["aliases"],
                json!({"aliases": {"type": "object", "additionalProperties": {"type": "string"}}}),
            ),
            error_codes: local_errors,
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        "completions" => CommandSemantics {
            success: object_schema(
                &["shell", "script"],
                json!({"shell": {"type": "string"}, "script": {"type": "string"}}),
            ),
            error_codes: [local_errors, vec!["encoding_error"]].concat(),
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        "doctor" => CommandSemantics {
            success: doctor_success_schema(),
            error_codes: [remote_errors, vec!["remote_check_failed"]].concat(),
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}, "remote": {"jira": "read", "tempo": "read"}}),
            dry_run: unsupported_dry_run(),
        },
        "schema" => CommandSemantics {
            success: json!({"type": "object", "description": "The contract document described by this schema command."}),
            error_codes: local_errors,
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
        "alias" => CommandSemantics {
            success: Value::Null,
            error_codes: vec!["usage"],
            side_effects: json!({"dependsOnSubcommand": true}),
            network_access: json!({"dependsOnSubcommand": true}),
            dry_run: unsupported_dry_run(),
        },
        _ => CommandSemantics {
            success: Value::Null,
            error_codes: local_errors,
            side_effects: json!({"default": []}),
            network_access: json!({"default": {}}),
            dry_run: unsupported_dry_run(),
        },
    }
}

fn unsupported_dry_run() -> Value {
    json!({"supported": false})
}

fn command_behavior(path: &str) -> Value {
    if path == "help" || path.ends_with(" help") {
        return json!({
            "target": "zero or more command names from the surrounding command tree",
            "output": "Clap help text on stdout without a JSON envelope"
        });
    }
    match path {
        "log" => json!({
            "dateDefault": "todayInConfiguredLocalTimeZone",
            "durationOrInterval": {
                "durationSyntax": ["15m", "1h", "1h15m"],
                "intervalSyntax": ["11-14", "11-14:30", "11:35-14:20", "11.35-14.20"],
                "overnight": "endAtOrBeforeStartUsesNextLocalDay"
            }
        }),
        "list" => json!({
            "dateDefault": "todayInConfiguredLocalTimeZone",
            "verbose": "adds descriptions and Jira URLs to human output only"
        }),
        "setup" => json!({
            "interactive": {
                "interface": "ratatui",
                "terminalRequired": true,
                "renderStream": "stderr",
                "events": "asynchronousCrossterm",
                "stages": ["jiraAccountDetails", "atlassianApiToken", "tempoAccount", "reviewAndSave"],
                "reducedMotionEnvironment": "DRAG_REDUCED_MOTION"
            },
            "fromEnv": {
                "interactive": false,
                "requiredEnvironment": ["ATLASSIAN_HOST", "ATLASSIAN_EMAIL", "ATLASSIAN_TOKEN", "TEMPO_TOKEN"]
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
                "runtimeCompatibilityEnvironment": "TEMPO_ACCOUNT_ID"
            },
            "writesConfiguration": "onceAfterVerification"
        }),
        "doctor" => json!({
            "remote": "opt-in read-only Jira and Tempo checks",
            "remoteStatuses": ["connected", "notConfigured", "failed"]
        }),
        _ => json!({}),
    }
}

fn object_schema(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object",
        "required": required,
        "properties": properties,
        "additionalProperties": false
    })
}

fn json_schema<T: JsonSchema>() -> Value {
    json!(schema_for!(T))
}

fn schema_ref(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

fn shared_definitions() -> Value {
    let mut definitions = Map::new();
    add_definition::<Worklog>(&mut definitions, "Worklog");
    add_definition::<AddWorklogRequest>(&mut definitions, "AddWorklogRequest");
    add_definition::<ScheduleDetails>(&mut definitions, "ScheduleDetails");
    Value::Object(definitions)
}

fn add_definition<T: JsonSchema>(definitions: &mut Map<String, Value>, name: &str) {
    let mut schema = json_schema::<T>();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        if let Some(Value::Object(nested)) = object.remove("$defs") {
            for (nested_name, nested_schema) in nested {
                definitions.entry(nested_name).or_insert(nested_schema);
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
            "aliases",
            "timezone",
            "target",
        ],
        json!({
            "name": {"const": "drag"},
            "version": {"type": "string"},
            "configPath": {"type": "string"},
            "configured": {"type": "object", "additionalProperties": {"type": "boolean"}},
            "aliases": {"type": "integer", "minimum": 0},
            "timezone": {"type": "string"},
            "target": {"type": "object", "required": ["architecture", "operatingSystem"], "properties": {"architecture": {"type": "string"}, "operatingSystem": {"type": "string"}}, "additionalProperties": false},
            "remoteChecks": {"type": "object", "properties": {"jira": service_check_schema(), "tempo": service_check_schema()}, "additionalProperties": false}
        }),
    )
}

fn service_check_schema() -> Value {
    json!({
        "type": "object",
        "required": ["status"],
        "properties": {
            "status": {"enum": ["connected", "notConfigured", "failed"]},
            "errorCode": {"type": "string"}
        },
        "additionalProperties": false
    })
}

fn output_contract() -> Value {
    json!({
        "modes": {
            "auto": "human on a stdout TTY; otherwise json",
            "human": "human-readable text",
            "json": "one JSON document"
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
            "http_error": 1,
            "io_error": 1,
            "encoding_error": 1,
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
    use serde_json::Value;

    use super::{schema, SCHEMA_VERSION};
    use crate::cli::{Cli, LogInput};

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
            serde_json::json!(["issueKeyOrAlias", "durationOrInterval"])
        );
        for field in ["when", "description", "start", "remainingEstimate"] {
            assert!(
                input_schema["properties"][field].is_object(),
                "missing {field}"
            );
        }
        serde_json::from_value::<LogInput>(serde_json::json!({
            "issueKeyOrAlias": "ABC-1",
            "durationOrInterval": "30m"
        }))
        .map_err(|error| error.to_string())?;
        assert!(serde_json::from_value::<LogInput>(serde_json::json!({
            "issueKeyOrAlias": "ABC-1",
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
        for id in ["issue_key_or_alias", "duration_or_interval"] {
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
            r#"{"issueKeyOrAlias":"ABC-1","durationOrInterval":"30m"}"#,
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
        assert_eq!(ids["required"], true);
        assert_eq!(ids["valueCount"]["minimum"], 1);
        assert!(ids["valueCount"]["maximum"].is_null());
        Cli::try_parse_from(["drag", "delete", "1", "2"]).map_err(|error| error.to_string())?;
        assert!(Cli::try_parse_from(["drag", "delete", "not-a-number"]).is_err());

        let output = rendered.data["globalOptions"]
            .as_array()
            .and_then(|arguments| arguments.iter().find(|argument| argument["id"] == "output"))
            .ok_or_else(|| "missing output option".to_owned())?;
        assert_eq!(output["enum"], serde_json::json!(["auto", "human", "json"]));
        assert_eq!(output["default"], "auto");
        assert!(Cli::try_parse_from(["drag", "--output", "xml", "schema"]).is_err());
        Ok(())
    }
}
