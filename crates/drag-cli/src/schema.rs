use serde_json::json;

use crate::output::Rendered;

pub(crate) fn schema() -> Rendered {
    let data = json!({
        "schemaVersion": 1,
        "name": "drag",
        "output": {"modes": ["auto", "human", "json"], "errorsOn": "stderr"},
        "commands": {
            "setup": {
                "sideEffects": true,
                "interactive": true,
                "interactiveInterface": "ratatui",
                "interactiveTerminalRequired": true,
                "interactiveEvents": "asynchronousCrossterm",
                "interactiveRendering": "stderr",
                "interactiveStages": ["jiraAccountDetails", "atlassianApiToken", "tempoAccount", "reviewAndSave"],
                "reducedMotionEnvironment": "DRAG_REDUCED_MOTION",
                "fromEnv": true,
                "noOpen": true,
                "fromEnvRequired": ["ATLASSIAN_HOST", "ATLASSIAN_EMAIL", "ATLASSIAN_TOKEN", "TEMPO_TOKEN"],
                "fromEnvInteractive": false,
                "browser": {
                    "default": "openEachTokenPageOnExplicitTokenStageEntry",
                    "beforeTokenStage": false,
                    "failure": "warning",
                    "noOpen": "printLinksWithoutOpening",
                    "fromEnv": false
                },
                "verification": {"jira": "read-only", "tempo": "read-only"},
                "accountId": {
                    "setup": "derivedFromVerifiedJiraUser",
                    "runtimeCompatibilityEnvironment": "TEMPO_ACCOUNT_ID"
                },
                "derivesAccountId": true,
                "writesConfiguration": "onceAfterVerification",
                "preservesConfiguration": ["aliases"]
            },
            "log": {
                "aliases": ["l"],
                "sideEffects": true,
                "networkAccess": {"jira": "read", "tempo": "write"},
                "mutation": "createTempoWorklog",
                "arguments": [
                    {"name": "issueKeyOrAlias", "requiredUnless": "json"},
                    {"name": "durationOrInterval", "requiredUnless": "json"},
                    {
                        "name": "when",
                        "required": false,
                        "default": "todayInConfiguredLocalTimeZone"
                    }
                ],
                "durationOrInterval": {
                    "durationSyntax": ["15m", "1h", "1h15m"],
                    "intervalSyntax": ["11-14", "11-14:30", "11:35-14:20", "11.35-14.20"],
                    "overnight": "endAtOrBeforeStartUsesNextLocalDay"
                },
                "date": {
                    "required": false,
                    "default": "todayInConfiguredLocalTimeZone",
                    "syntax": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"]
                },
                "flags": {
                    "description": {"short": "d", "value": "string"},
                    "start": {"short": "s", "value": "HH:mm", "appliesTo": "duration"},
                    "remainingEstimate": {
                        "short": "r",
                        "value": "duration",
                        "syntax": ["15m", "1h", "1h15m"]
                    },
                    "debug": {
                        "global": true,
                        "output": "humanStderr",
                        "credentials": "redacted"
                    }
                },
                "rawJson": true,
                "rawJsonInput": {
                    "stdinValue": "-",
                    "denyUnknownFields": true,
                    "fields": [
                        "issueKeyOrAlias",
                        "durationOrInterval",
                        "when",
                        "description",
                        "start",
                        "remainingEstimate"
                    ]
                },
                "dryRun": true,
                "dryRunBehavior": {"sideEffects": false, "networkAccess": false}
            },
            "list": {
                "aliases": ["ls"],
                "sideEffects": false,
                "networkAccess": "read-only",
                "date": {
                    "required": false,
                    "default": "todayInConfiguredLocalTimeZone",
                    "syntax": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"]
                },
                "verbose": true
            },
            "delete": {"aliases": ["d"], "dryRun": true},
            "alias": {"subcommands": ["set", "list", "delete"]},
            "completions": {},
            "doctor": {
                "remote": true,
                "defaultNetworkAccess": false,
                "remoteNetworkAccess": "read-only",
                "remoteChecks": {"jira": "read-only", "tempo": "read-only"},
                "remoteStatuses": ["connected", "notConfigured", "failed"],
                "failureExitCodes": {
                    "remoteFailure": 1,
                    "notConfiguredOrInvalid": 2
                }
            },
            "schema": {}
        },
        "dateSyntax": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"],
        "durationSyntax": ["15m", "1h", "1h15m", "11-12:30", "11.35-14.20", "23:30-00:30"],
        "environment": ["DRAG_CONFIG", "DRAG_REDUCED_MOTION", "TEMPO_TOKEN", "TEMPO_ACCOUNT_ID", "ATLASSIAN_EMAIL", "ATLASSIAN_TOKEN", "ATLASSIAN_HOST"],
        "exitCodes": {"0": "success", "1": "runtime failure", "2": "usage or invalid input"}
    });
    Rendered::new(
        data,
        "Use `drag --output json schema` for the full CLI contract.".to_owned(),
    )
}
