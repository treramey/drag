use std::fs;

use assert_cmd::Command;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;
use tempfile::TempDir;

fn command(config: &std::path::Path) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("drag")?;
    command.args([
        "--config",
        config
            .to_str()
            .ok_or("temporary config path is not UTF-8")?,
        "--timezone",
        "Europe/Warsaw",
        "--output",
        "json",
    ]);
    for variable in [
        "TEMPO_TOKEN",
        "TEMPO_ACCOUNT_ID",
        "ATLASSIAN_EMAIL",
        "ATLASSIAN_TOKEN",
        "ATLASSIAN_HOST",
        "DRAG_REDUCED_MOTION",
        "DRAG_CACHE_DIR",
    ] {
        command.env_remove(variable);
    }
    Ok(command)
}

fn configured_file(directory: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{
          "tempoToken":"tempo-secret",
          "accountId":"account-1",
          "atlassianUserEmail":"person@example.com",
          "atlassianToken":"atlassian-secret",
          "hostname":"example.atlassian.net"
        }"#,
    )?;
    Ok(path)
}

fn list_continuation(
    selected_date: &str,
    month_start: &str,
    month_end: &str,
    url: &str,
    limit: Option<u16>,
    page_limit: u16,
    all_pages: bool,
) -> Result<String, serde_json::Error> {
    Ok(
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "selectedDate": selected_date,
            "monthStart": month_start,
        "monthEnd": month_end,
        "url": url,
        "limit": limit,
        "pageLimit": page_limit,
        "allPages": all_pages,
        }))?),
    )
}

#[test]
fn reports_version() -> Result<(), Box<dyn std::error::Error>> {
    Command::cargo_bin("drag")?
        .arg("--version")
        .assert()
        .success();
    Ok(())
}

#[test]
fn ndjson_output_is_explicit_and_reserved_for_list() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");

    let list = Command::cargo_bin("drag")?
        .args([
            "--config",
            missing
                .to_str()
                .ok_or("temporary config path is not UTF-8")?,
            "--output",
            "ndjson",
            "list",
        ])
        .output()?;
    assert_eq!(list.status.code(), Some(2));
    assert!(list.stdout.is_empty());
    let list_error: Value = serde_json::from_slice(&list.stderr)?;
    assert_eq!(list_error["error"]["code"], "not_configured");

    let schema = Command::cargo_bin("drag")?
        .args(["--output", "ndjson", "schema"])
        .output()?;
    assert_eq!(schema.status.code(), Some(2));
    assert!(schema.stdout.is_empty());
    let schema_error: Value = serde_json::from_slice(&schema.stderr)?;
    assert_eq!(schema_error["error"]["code"], "invalid_input");
    Ok(())
}

#[test]
fn delete_json_rejects_invalid_batches_before_configuration_or_network_access(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing_config = directory.path().join("missing.json");
    let cases = [
        (
            vec!["delete", "--json", r#"{"worklogIds":[]}"#],
            "invalid_input",
        ),
        (
            vec!["delete", "--json", r#"{"worklogIds":["123"]}"#],
            "invalid_json",
        ),
        (vec!["delete", "--json", r#"[[123,456]]"#], "invalid_json"),
        (
            vec!["delete", "--json", r#"{"worklogIds":[123],"extra":true}"#],
            "invalid_json",
        ),
        (
            vec!["delete", "123", "--json", r#"{"worklogIds":[123]}"#],
            "usage",
        ),
    ];

    for (args, expected_code) in cases {
        let output = command(&missing_config)?.args(args).output()?;
        assert_eq!(output.status.code(), Some(2));
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], expected_code);
    }
    assert!(!missing_config.exists());
    Ok(())
}

#[test]
fn delete_accepts_ordered_json_batches_inline_and_from_stdin(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing_config = directory.path().join("missing.json");
    let raw = r#"{"worklogIds":[123,456,123]}"#;

    let inline = command(&missing_config)?
        .args(["delete", "--json", raw, "--dry-run"])
        .output()?;
    let stdin = command(&missing_config)?
        .args(["delete", "--json", "-", "--dry-run"])
        .write_stdin(raw)
        .output()?;

    for output in [inline, stdin] {
        assert_eq!(output.status.code(), Some(2));
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], "not_configured");
    }
    Ok(())
}

#[test]
fn log_dry_run_parses_without_network_access() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args([
            "log",
            "ABC-1",
            "1h15m",
            "2020-02-28",
            "--start",
            "9:30",
            "--dry-run",
        ])
        .output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["dryRun"], true);
    assert_eq!(body["data"]["request"]["timeSpentSeconds"], 4_500);
    assert_eq!(body["data"]["request"]["startTime"], "09:30:00");
    assert_eq!(body["data"]["issueKey"], "ABC-1");
    Ok(())
}

#[test]
fn log_and_l_produce_equivalent_positional_dot_interval_previews(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let mut previews = Vec::new();
    for command_name in ["log", "l"] {
        let output = command(&path)?
            .args([
                command_name,
                "ABC-1",
                "11.35-14.20",
                "2020-02-28",
                "--start",
                "6:15",
                "--dry-run",
            ])
            .output()?;

        assert!(output.status.success(), "{command_name} failed");
        assert!(output.stderr.is_empty(), "{command_name} wrote to stderr");
        let body: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(body["data"]["request"]["startTime"], "11:35:00");
        assert_eq!(body["data"]["request"]["timeSpentSeconds"], 9_900);
        previews.push(body["data"].clone());
    }
    assert_eq!(previews[0], previews[1]);
    Ok(())
}

#[test]
fn positional_inline_and_stdin_log_inputs_are_equivalent() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let raw = r#"{"issueKey":"ABC-1","durationOrInterval":"30m","when":"2020-02-28","description":"review with team","start":"9:30","remainingEstimate":"2h"}"#;
    let positional = command(&path)?
        .args([
            "log",
            "ABC-1",
            "30m",
            "2020-02-28",
            "--description",
            "review with team",
            "--start",
            "9:30",
            "--remaining-estimate",
            "2h",
            "--dry-run",
        ])
        .output()?;
    let inline = command(&path)?
        .args(["log", "--json", raw, "--dry-run"])
        .output()?;
    let stdin = command(&path)?
        .args(["log", "--json", "-", "--dry-run"])
        .write_stdin(raw)
        .output()?;
    assert!(positional.status.success());
    assert!(inline.status.success());
    assert!(stdin.status.success());
    assert!(positional.stderr.is_empty());
    assert!(inline.stderr.is_empty());
    assert!(stdin.stderr.is_empty());
    let positional: Value = serde_json::from_slice(&positional.stdout)?;
    let inline: Value = serde_json::from_slice(&inline.stdout)?;
    let stdin: Value = serde_json::from_slice(&stdin.stdout)?;
    assert_eq!(stdin["ok"], true);
    assert_eq!(inline["data"], positional["data"]);
    assert_eq!(stdin["data"], positional["data"]);
    Ok(())
}

#[test]
fn log_rejects_raw_json_combined_with_positional_input() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args([
            "log",
            "ABC-1",
            "30m",
            "--json",
            r#"{"issueKey":"ABC-1","durationOrInterval":"30m"}"#,
            "--dry-run",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "usage");
    Ok(())
}

#[test]
fn log_rejects_unknown_raw_json_fields() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("missing.json");
    let output = command(&path)?
        .args([
            "log",
            "--json",
            r#"{"issueKey":"ABC-1","durationOrInterval":"30m","descripton":"typo"}"#,
            "--dry-run",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_json");
    Ok(())
}

#[test]
fn invalid_duration_is_a_structured_usage_error() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args(["log", "ABC-1", "nonsense", "--dry-run"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_duration");
    Ok(())
}

#[test]
fn zero_duration_is_a_structured_usage_error() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args(["log", "ABC-1", "0m", "--dry-run"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "non_positive_duration");
    Ok(())
}

#[test]
fn log_reports_missing_and_malformed_configuration_without_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");
    let missing_output = command(&missing)?
        .args(["log", "ABC-1", "30m", "--dry-run"])
        .output()?;
    assert_eq!(missing_output.status.code(), Some(2));
    let missing_body: Value = serde_json::from_slice(&missing_output.stderr)?;
    assert_eq!(missing_body["error"]["code"], "not_configured");

    let malformed = directory.path().join("malformed.json");
    fs::write(&malformed, "{not valid json")?;
    let malformed_output = command(&malformed)?
        .args(["log", "ABC-1", "30m", "--dry-run"])
        .output()?;
    assert_eq!(malformed_output.status.code(), Some(1));
    let malformed_body: Value = serde_json::from_slice(&malformed_output.stderr)?;
    assert_eq!(malformed_body["error"]["code"], "config_error");
    Ok(())
}

#[test]
fn log_json_debug_runtime_failure_stays_on_stderr() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("malformed.json");
    fs::write(&path, "{not valid json")?;

    let output = command(&path)?
        .args(["log", "ABC-1", "30m", "--debug"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "config_error");
    Ok(())
}

#[test]
fn invalid_list_date_is_a_structured_usage_error_before_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?.args(["list", "not-a-date"]).output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_date");
    Ok(())
}

#[test]
fn list_reports_missing_and_malformed_configuration_without_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");
    let missing_output = command(&missing)?.arg("list").output()?;
    assert_eq!(missing_output.status.code(), Some(2));
    let missing_body: Value = serde_json::from_slice(&missing_output.stderr)?;
    assert_eq!(missing_body["error"]["code"], "not_configured");

    let malformed = directory.path().join("malformed.json");
    fs::write(&malformed, "{not valid json")?;
    let malformed_output = command(&malformed)?.arg("ls").output()?;
    assert_eq!(malformed_output.status.code(), Some(1));
    let malformed_body: Value = serde_json::from_slice(&malformed_output.stderr)?;
    assert_eq!(malformed_body["error"]["code"], "config_error");
    Ok(())
}

#[test]
fn schema_documents_safety_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let output = command(&path)?.arg("schema").output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    let contract = &body["data"];
    assert_eq!(contract["schemaVersion"], 3);
    assert_eq!(contract["cliVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(contract["output"]["successStream"], "stdout");
    assert_eq!(contract["output"]["errorStream"], "stderr");
    assert_eq!(contract["errors"]["codes"]["usage"], 2);
    assert_eq!(contract["errors"]["codes"]["api_error"], 1);
    assert_eq!(
        contract["output"]["modes"]["ndjson"],
        "newline-delimited list events"
    );
    assert_eq!(
        contract["output"]["modeConstraints"]["ndjson"]["commands"],
        serde_json::json!(["list"])
    );
    assert_eq!(
        contract["output"]["modeConstraints"]["ndjson"]["otherwise"]["errorCode"],
        "invalid_input"
    );

    for name in ["log", "list", "delete", "setup", "doctor", "schema", "help"] {
        let command = &contract["commands"][name];
        assert!(command.is_object(), "missing command {name}");
        assert!(
            command["errorCodes"].is_array(),
            "missing errors for {name}"
        );
        assert!(
            command["sideEffects"].is_object(),
            "missing effects for {name}"
        );
        assert!(
            command["networkAccess"].is_object(),
            "missing network contract for {name}"
        );
        assert!(
            command["dryRun"].is_object(),
            "missing dry-run contract for {name}"
        );
    }
    assert_eq!(
        contract["commands"]["log"]["aliases"],
        serde_json::json!(["l"])
    );
    assert_eq!(
        contract["commands"]["list"]["aliases"],
        serde_json::json!(["ls"])
    );
    assert_eq!(
        contract["commands"]["delete"]["aliases"],
        serde_json::json!(["d"])
    );
    let log_arguments = contract["commands"]["log"]["arguments"]
        .as_array()
        .ok_or("log arguments are not an array")?;
    let json_input = log_arguments
        .iter()
        .find(|argument| argument["id"] == "json")
        .ok_or("missing --json")?;
    assert_eq!(json_input["stdinValue"], "-");
    assert_eq!(json_input["jsonSchema"]["additionalProperties"], false);
    assert_eq!(
        json_input["jsonSchema"]["required"],
        serde_json::json!(["issueKey", "durationOrInterval"])
    );
    assert_eq!(
        json_input["conflictsWith"],
        serde_json::json!([
            "description",
            "durationOrInterval",
            "issueKey",
            "remainingEstimate",
            "start",
            "when"
        ])
    );
    assert_eq!(
        contract["commands"]["log"]["dryRun"]["networkAccess"],
        false
    );
    assert_eq!(
        contract["commands"]["delete"]["dryRun"]["networkAccess"],
        "read-only"
    );
    let delete_arguments = contract["commands"]["delete"]["arguments"]
        .as_array()
        .ok_or("delete arguments are not an array")?;
    let delete_json = delete_arguments
        .iter()
        .find(|argument| argument["id"] == "json")
        .ok_or("missing delete --json")?;
    assert_eq!(delete_json["stdinValue"], "-");
    assert_eq!(delete_json["jsonSchema"]["additionalProperties"], false);
    assert_eq!(
        delete_json["jsonSchema"]["required"],
        serde_json::json!(["worklogIds"])
    );
    assert_eq!(
        delete_json["jsonSchema"]["properties"]["worklogIds"]["minItems"],
        1
    );
    let delete_ids = delete_arguments
        .iter()
        .find(|argument| argument["id"] == "worklog_ids")
        .ok_or("missing positional delete IDs")?;
    assert_eq!(
        delete_ids["requiredUnlessPresent"],
        serde_json::json!(["json"])
    );
    assert_eq!(
        delete_json["conflictsWith"],
        serde_json::json!(["worklogIds"])
    );
    assert!(contract["commands"]["list"]["success"]["properties"]["worklogs"]["items"].is_object());
    let list_arguments = contract["commands"]["list"]["arguments"]
        .as_array()
        .ok_or("list arguments are not an array")?;
    let list_argument = |id: &str| list_arguments.iter().find(|argument| argument["id"] == id);
    let limit = list_argument("limit").ok_or("missing list --limit")?;
    assert_eq!(limit["type"], "unsignedInteger");
    assert_eq!(limit["default"], 100);
    assert_eq!(limit["minimum"], 1);
    assert_eq!(limit["maximum"], 1000);
    assert_eq!(limit["conflictsWith"], serde_json::json!(["allPages"]));
    let page_limit = list_argument("page_limit").ok_or("missing list --page-limit")?;
    assert_eq!(page_limit["type"], "unsignedInteger");
    assert_eq!(page_limit["default"], 1);
    assert_eq!(page_limit["minimum"], 1);
    assert_eq!(page_limit["maximum"], 100);
    let fields = list_argument("fields").ok_or("missing list --fields")?;
    assert_eq!(fields["type"], "fieldMask");
    assert_eq!(fields["separator"], ",");
    assert!(fields["allowedFields"].as_array().is_some_and(|paths| paths
        .contains(&Value::String("worklogs.interval.startTime".to_owned()))
        && paths.contains(&Value::String("schedule.dayLoggedDuration".to_owned()))
        && paths.contains(&Value::String("pagination.next".to_owned()))));
    assert!(list_argument("continue_from").is_some());
    let all_pages = list_argument("all_pages").ok_or("missing list --all-pages")?;
    assert_eq!(
        all_pages["conflictsWith"],
        serde_json::json!(["limit", "pageLimit"])
    );
    let pagination = &contract["$defs"]["ListPagination"];
    assert_eq!(
        contract["commands"]["list"]["success"]["properties"]["pagination"]["$ref"],
        "#/$defs/ListPagination"
    );
    assert_eq!(
        contract["commands"]["list"]["successWithFieldSelection"]["$ref"],
        "#/$defs/ProjectedListResult"
    );
    let stream = &contract["commands"]["list"]["ndjson"];
    assert_eq!(stream["outputMode"], "ndjson");
    assert_eq!(stream["discriminator"], "kind");
    assert_eq!(
        stream["eventOrder"],
        serde_json::json!(["zeroOrMoreWorklog", "summary", "pagination"])
    );
    assert_eq!(
        stream["events"]["worklog"]["$ref"],
        "#/$defs/ListWorklogEvent"
    );
    assert_eq!(
        stream["events"]["summary"]["$ref"],
        "#/$defs/ListSummaryEvent"
    );
    assert_eq!(
        stream["events"]["pagination"]["$ref"],
        "#/$defs/ListPaginationEvent"
    );
    assert_eq!(stream["terminalEvent"], "pagination");
    assert_eq!(stream["failureStream"], "stderrErrorEnvelope");
    assert_eq!(
        stream["pageEmission"],
        "worklog events are flushed before requesting the next Tempo page"
    );
    assert_eq!(stream["brokenPipe"], "clean successful termination");
    let projected = &contract["$defs"]["ProjectedListResult"];
    assert_eq!(projected["minProperties"], 1);
    assert_eq!(projected["additionalProperties"], false);
    assert!(projected["required"].is_null());
    assert_eq!(
        projected["properties"]["worklogs"]["items"]["$ref"],
        "#/$defs/ProjectedWorklog"
    );
    assert_eq!(pagination["additionalProperties"], false);
    assert_eq!(pagination["properties"]["pageLimit"]["maximum"], 100);
    assert!(pagination["required"]
        .as_array()
        .is_some_and(|required| required.contains(&Value::String("next".to_owned()))));
    for field in ["selectedDate", "monthStart", "monthEnd", "totalsComplete"] {
        assert!(pagination["required"]
            .as_array()
            .is_some_and(|required| required.contains(&Value::String(field.to_owned()))));
    }
    assert_eq!(
        contract["commands"]["list"]["behavior"]["pagination"]["defaultPageLimit"],
        1
    );
    let selection = &contract["commands"]["list"]["behavior"]["fieldSelection"];
    assert_eq!(selection["option"], "fields");
    assert_eq!(selection["default"], "allFields");
    assert_eq!(
        selection["recommendation"],
        "requestOnlyFieldsNeededForTask"
    );
    assert_eq!(selection["appliesTo"], "structuredOutputOnly");
    assert_eq!(selection["projection"], "beforeSerialization");
    assert_eq!(selection["ordering"], "canonicalResultOrder");
    let interactive = &contract["commands"]["list"]["behavior"]["interactive"];
    assert_eq!(interactive["outputMode"], "human");
    assert_eq!(
        interactive["terminalRequirements"],
        serde_json::json!(["stdin", "stdout", "stderr"])
    );
    assert_eq!(interactive["allTerminalsRequired"], true);
    assert_eq!(interactive["renderStream"], "stderr");
    assert_eq!(interactive["fallback"], "completedPlainTextReport");
    assert_eq!(interactive["controls"]["previousDate"], "h");
    assert_eq!(interactive["controls"]["nextDate"], "l");
    assert_eq!(interactive["controls"]["openFocusedJiraIssue"], "o");
    assert_eq!(
        interactive["controls"]["quit"],
        serde_json::json!(["q", "escape", "ctrl-c"])
    );
    assert_eq!(
        interactive["browser"]["sideEffect"],
        "openLocalDefaultBrowser"
    );
    assert_eq!(interactive["browser"]["additionalApiRequestByDrag"], false);
    assert_eq!(interactive["browser"]["remoteMutation"], false);
    assert_eq!(
        contract["commands"]["list"]["networkAccess"]["interactive"],
        serde_json::json!({"browser": "may-open", "jira": "read", "tempo": "read"})
    );
    assert_eq!(
        contract["commands"]["list"]["behavior"]["automation"],
        serde_json::json!({
            "recommendation": "useExplicitJsonOutput",
            "arguments": ["--output", "json"],
            "interactive": false,
            "successContract": "unchangedListJsonContract"
        })
    );
    assert_eq!(
        contract["commands"]["list"]["sideEffects"]["interactive"],
        serde_json::json!(["openFocusedJiraUrlInLocalDefaultBrowser"])
    );
    assert_eq!(
        contract["commands"]["setup"]["behavior"]["interactive"]["renderStream"],
        "stderr"
    );
    assert_eq!(
        contract["commands"]["doctor"]["networkAccess"]["default"],
        serde_json::json!({})
    );
    Ok(())
}

#[test]
fn dotted_tempo_schema_lookup_uses_the_cached_official_openapi_contract(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let config = directory.path().join("config.json");
    let cache = directory.path().join("cache");
    fs::create_dir_all(&cache)?;
    fs::write(
        cache.join("tempo-openapi.yaml"),
        r#"openapi: 3.0.3
info:
  title: Tempo API
  version: "4"
paths:
  /worklogs:
    post:
      operationId: createWorklog
      summary: Create Worklog
      tags: [Worklogs]
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/WorklogInput'
      responses:
        "200":
          description: SUCCESS
components:
  schemas:
    WorklogInput:
      type: object
      required: [issueId, timeSpentSeconds]
      properties:
        issueId:
          type: string
        timeSpentSeconds:
          type: integer
"#,
    )?;

    let output = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .args(["schema", "tempo.worklogs.create", "--resolve-refs"])
        .output()?;

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    let data = &body["data"];
    assert_eq!(data["path"], "tempo.worklogs.create");
    assert_eq!(
        data["source"]["url"],
        "https://apidocs.tempo.io/tempo-openapi.yaml"
    );
    assert_eq!(data["source"]["openapi"], "3.0.3");
    assert_eq!(data["operation"]["operationId"], "createWorklog");
    assert_eq!(data["operation"]["httpMethod"], "POST");
    assert_eq!(data["operation"]["path"], "/worklogs");
    assert_eq!(
        data["operation"]["requestBody"]["content"]["application/json"]["schema"]["type"],
        "object"
    );
    assert_eq!(
        data["operation"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
            ["timeSpentSeconds"]["type"],
        "integer"
    );
    Ok(())
}

#[test]
fn dynamic_tempo_read_command_previews_the_openapi_request(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let config = directory.path().join("config.json");
    let cache = directory.path().join("cache");
    fs::create_dir_all(&cache)?;
    fs::write(
        cache.join("tempo-openapi.yaml"),
        r#"openapi: 3.0.3
info:
  title: Tempo API
  version: "4"
paths:
  /4/work-attributes:
    get:
      operationId: getWorkAttributes
      summary: Retrieve Work Attributes
      tags: [Work Attributes]
      parameters:
        - in: query
          name: offset
          schema: {type: integer, default: 0}
        - in: query
          name: limit
          schema: {type: integer, default: 50}
      responses:
        "200": {description: SUCCESS}
  /4/worklogs:
    post:
      operationId: createWorklog
      summary: Create Worklog
      tags: [Worklogs]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [issueId, timeSpentSeconds]
              additionalProperties: false
              properties:
                issueId: {type: integer}
                timeSpentSeconds: {type: integer}
                attributes:
                  type: array
                  items:
                    type: object
                    required: [key, value]
                    additionalProperties: false
                    properties:
                      key: {type: string}
                      value: {type: string}
      responses:
        "200": {description: SUCCESS}
components: {schemas: {}}
"#,
    )?;

    let output = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .args([
            "tempo",
            "work-attributes",
            "list",
            "--params",
            r#"{"limit":25}"#,
            "--dry-run",
        ])
        .output()?;

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["dryRun"], true);
    assert_eq!(body["data"]["operationId"], "getWorkAttributes");
    assert_eq!(body["data"]["method"], "GET");
    assert_eq!(
        body["data"]["url"],
        "https://api.tempo.io/4/work-attributes?limit=25"
    );
    assert!(body["data"]["body"].is_null());

    let mutation = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .args([
            "tempo",
            "worklogs",
            "create",
            "--json",
            r#"{"issueId":10001,"timeSpentSeconds":3600,"attributes":[{"key":"_Test_","value":"PS"},{"key":"_PSTYPE_","value":"Consulting"}]}"#,
            "--dry-run",
        ])
        .output()?;
    assert!(mutation.status.success());
    assert!(mutation.stderr.is_empty());
    let mutation: Value = serde_json::from_slice(&mutation.stdout)?;
    assert_eq!(mutation["data"]["dryRun"], true);
    assert_eq!(mutation["data"]["operationId"], "createWorklog");
    assert_eq!(mutation["data"]["method"], "POST");
    assert_eq!(
        mutation["data"]["body"],
        serde_json::json!({
            "issueId": 10001,
            "timeSpentSeconds": 3600,
            "attributes": [
                {"key": "_Test_", "value": "PS"},
                {"key": "_PSTYPE_", "value": "Consulting"}
            ]
        })
    );

    let help = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .args(["tempo", "--help"])
        .output()?;
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let help = String::from_utf8(help.stdout)?;
    assert!(help.contains("work-attributes"));
    assert!(help.contains("Usage: tempo <COMMAND>"));
    assert!(!help.contains("\"ok\""));

    let bare = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .arg("tempo")
        .output()?;
    assert!(bare.status.success());
    assert!(bare.stderr.is_empty());
    assert_eq!(String::from_utf8(bare.stdout)?, help);

    let rejected = command(&config)?
        .env("DRAG_CACHE_DIR", &cache)
        .args([
            "tempo",
            "work-attributes",
            "list",
            "--params",
            r#"{"undeclared":"value"}"#,
            "--dry-run",
        ])
        .output()?;
    assert_eq!(rejected.status.code(), Some(2));
    assert!(rejected.stdout.is_empty());
    let error: Value = serde_json::from_slice(&rejected.stderr)?;
    assert_eq!(error["error"]["code"], "invalid_input");
    assert!(error["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("unknown parameter 'undeclared'")));
    Ok(())
}

#[test]
fn log_help_documents_inputs_safety_and_examples() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::cargo_bin("drag")?
        .args(["log", "--help"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    for expected in [
        "[ISSUE_KEY] [DURATION_OR_INTERVAL] [WHEN]",
        "defaults to today in the configured local time zone",
        "Aliases:",
        "drag l",
        "11:35-14:20",
        "11.35-14.20",
        "2026-07-14",
        "--description",
        "--start",
        "--remaining-estimate",
        "--json",
        "--dry-run",
        "--debug",
    ] {
        assert!(stdout.contains(expected), "help omitted {expected}");
    }
    Ok(())
}

#[test]
fn setup_help_documents_guided_and_unattended_modes() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::cargo_bin("drag")?
        .args(["setup", "--help"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("opens Ratatui"));
    assert!(stdout.contains("terminal-capable stdin and stderr"));
    assert!(stdout
        .contains("Jira account details, Atlassian API token, Tempo account, and Review & save"));
    assert!(stdout.contains("No browser opens while entering Jira details"));
    assert!(stdout.contains("explicitly enter its token stage"));
    assert!(stdout.contains("Tab and Shift-Tab"));
    assert!(stdout.contains("Escape goes back"));
    assert!(stdout.contains("cancels from Jira account"));
    assert!(stdout.contains("Ctrl-C"));
    assert!(stdout.contains("--from-env"));
    assert!(stdout.contains("--no-open"));
    assert!(stdout.contains("DRAG_REDUCED_MOTION=1"));
    assert!(stdout.contains("Print token URLs without launching a browser"));
    Ok(())
}

#[test]
fn list_help_documents_read_only_date_and_verbose_behavior(
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::cargo_bin("drag")?
        .args(["list", "--help"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("without changing Jira or Tempo"));
    assert!(stdout.contains("defaults to today"));
    assert!(stdout.contains("[DATE]"));
    assert!(stdout.contains("--verbose"));
    assert!(stdout.contains("descriptions and Jira URLs"));
    assert!(stdout.contains("stdin, stdout, and stderr are all"));
    assert!(stdout.contains("interactive stderr report"));
    assert!(stdout.contains("local default browser"));
    assert!(stdout.contains("without changing Jira or Tempo"));
    assert!(stdout.contains("--output json explicitly"));
    assert!(stdout.contains("--fields"));
    assert!(stdout.contains("Comma-delimited result fields"));
    assert!(stdout.contains("--limit"));
    assert!(stdout.contains("default: 100"));
    assert!(stdout.contains("--page-limit"));
    assert!(stdout.contains("default: 1"));
    assert!(stdout.contains("--continue-from"));
    assert!(stdout.contains("opaque continuation token"));
    assert!(stdout.contains("--all-pages"));
    assert!(stdout.contains("100-page safety ceiling"));
    Ok(())
}

#[test]
fn invalid_list_bounds_and_incompatible_all_pages_fail_before_configuration(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");

    for arguments in [
        vec!["list", "--limit", "0"],
        vec!["list", "--limit", "1001"],
        vec!["list", "--page-limit", "0"],
        vec!["list", "--page-limit", "101"],
        vec!["list", "--all-pages", "--limit", "10"],
        vec!["list", "--all-pages", "--page-limit", "2"],
    ] {
        let output = command(&missing)?.args(&arguments).output()?;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        let error: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(error["ok"], false, "{arguments:?}");
        assert_eq!(error["error"]["code"], "usage", "{arguments:?}");
        assert_ne!(error["error"]["code"], "config_error", "{arguments:?}");
    }
    Ok(())
}

#[test]
fn invalid_list_field_masks_fail_before_configuration_or_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");

    for mask in [
        "",
        "worklogs.id,worklogs.id",
        "worklogs,worklogs.id",
        "worklogs.interval.unknown",
        "request.issueId",
    ] {
        let output = command(&missing)?
            .args(["list", "--fields", mask])
            .output()?;

        assert_eq!(output.status.code(), Some(2), "{mask:?}");
        assert!(output.stdout.is_empty(), "{mask:?}");
        let error: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(error["error"]["code"], "invalid_input", "{mask:?}");
        assert!(!missing.exists(), "{mask:?}");
    }
    Ok(())
}

#[test]
fn unsafe_list_continuations_fail_before_configuration_or_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");

    let continuations = [
        "not-a-token".to_owned(),
        list_continuation(
            "2026-07-14",
            "2026-07-01",
            "2026-07-31",
            "https://attacker.example/4/worklogs?from=2026-07-01&to=2026-07-31",
            Some(100),
            1,
            false,
        )?,
        list_continuation(
            "2026-07-14",
            "2026-07-01",
            "2026-07-31",
            "https://user:password@api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31",
            Some(100),
            1,
            false,
        )?,
    ];
    for continuation in continuations {
        let output = command(&missing)?
            .args(["list", "2026-07-14", "--continue-from", &continuation])
            .output()?;

        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(error["error"]["code"], "invalid_input");
        assert!(!error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(&continuation)));
    }
    Ok(())
}

#[test]
fn list_continuations_for_another_selected_date_fail_before_configuration_or_networking(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");
    let continuation = list_continuation(
        "2026-07-13",
        "2026-07-01",
        "2026-07-31",
        "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=100",
        Some(100),
        1,
        false,
    )?;

    let output = command(&missing)?
        .args(["list", "2026-07-14", "--continue-from", &continuation])
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(error["error"]["code"], "invalid_input");
    assert!(!error["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains(&continuation)));
    Ok(())
}

#[test]
fn list_continuation_rejects_incompatible_explicit_bounds_before_configuration(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let missing = directory.path().join("missing.json");
    let continuation = list_continuation(
        "2026-07-14",
        "2026-07-01",
        "2026-07-31",
        "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&limit=250&offset=250",
        Some(250),
        3,
        false,
    )?;

    for arguments in [
        vec![
            "list",
            "2026-07-14",
            "--continue-from",
            &continuation,
            "--limit",
            "100",
        ],
        vec![
            "list",
            "2026-07-14",
            "--continue-from",
            &continuation,
            "--page-limit",
            "1",
        ],
        vec![
            "list",
            "2026-07-14",
            "--continue-from",
            &continuation,
            "--all-pages",
        ],
    ] {
        let output = command(&missing)?.args(arguments).output()?;
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(error["error"]["code"], "invalid_input");
    }
    Ok(())
}

#[test]
fn doctor_help_documents_opt_in_read_only_remote_checks() -> Result<(), Box<dyn std::error::Error>>
{
    let output = Command::cargo_bin("drag")?
        .args(["doctor", "--help"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("without network access"));
    assert!(stdout.contains("opt-in, read-only"));
    assert!(stdout.contains("--remote"));
    Ok(())
}

#[test]
fn doctor_defaults_to_network_free_local_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");

    let output = command(&path)?.arg("doctor").output()?;

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["ok"], true);
    assert!(body["data"].get("remoteChecks").is_none());
    Ok(())
}

#[test]
fn doctor_preserves_configured_presence_semantics_for_empty_environment_values(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");

    let output = command(&path)?
        .arg("doctor")
        .env("TEMPO_TOKEN", "")
        .output()?;

    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["configured"]["tempoToken"], true);
    Ok(())
}

#[test]
fn doctor_remote_reports_missing_services_and_exits_unsuccessfully(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");

    let output = command(&path)?
        .args(["doctor", "--remote", "--debug"])
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["ok"], false);
    assert_eq!(
        body["error"]["details"]["remoteChecks"]["jira"]["status"],
        "notConfigured"
    );
    assert_eq!(
        body["error"]["details"]["remoteChecks"]["tempo"]["status"],
        "notConfigured"
    );
    Ok(())
}

#[test]
fn doctor_remote_reports_malformed_config_as_a_structured_error(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    fs::write(&path, "{not valid json")?;

    let output = command(&path)?.args(["doctor", "--remote"]).output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "config_error");
    Ok(())
}

#[test]
fn headless_setup_requires_only_the_four_connection_variables(
) -> Result<(), Box<dyn std::error::Error>> {
    const VARIABLES: [&str; 4] = [
        "ATLASSIAN_HOST",
        "ATLASSIAN_EMAIL",
        "ATLASSIAN_TOKEN",
        "TEMPO_TOKEN",
    ];
    for missing in VARIABLES {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let mut process = command(&path)?;
        process
            .args(["setup", "--from-env"])
            .env("ATLASSIAN_HOST", "example.atlassian.net")
            .env("ATLASSIAN_EMAIL", "person@example.com")
            .env("ATLASSIAN_TOKEN", "jira-token-must-not-leak")
            .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
            .env("TEMPO_ACCOUNT_ID", "legacy-account-must-not-win")
            .env_remove(missing);
        let output = process.output()?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], "invalid_input");
        assert!(body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(missing)));
        let stderr = String::from_utf8(output.stderr)?;
        assert!(!stderr.contains("jira-token-must-not-leak"));
        assert!(!stderr.contains("tempo-token-must-not-leak"));
    }
    Ok(())
}

#[test]
fn headless_setup_dry_run_emits_a_secret_free_local_plan_without_writing(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let original = br#"{}"#;
    fs::write(&path, original)?;

    let output = command(&path)?
        .args(["setup", "--from-env", "--dry-run"])
        .env("ATLASSIAN_HOST", "example.atlassian.net")
        .env("ATLASSIAN_EMAIL", "person@example.com")
        .env("ATLASSIAN_TOKEN", "jira-token-must-not-leak")
        .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
        .output()?;

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["dryRun"], true);
    assert_eq!(body["data"]["localValidation"]["status"], "passed");
    assert_eq!(body["data"]["remoteVerification"]["status"], "planned");
    assert_eq!(body["data"]["configuration"]["status"], "planned");
    assert_eq!(fs::read(&path)?, original);
    let all_output = format!(
        "{}{}",
        String::from_utf8(output.stdout)?,
        String::from_utf8(output.stderr)?
    );
    assert!(!all_output.contains("jira-token-must-not-leak"));
    assert!(!all_output.contains("tempo-token-must-not-leak"));
    Ok(())
}

#[test]
fn headless_setup_dry_run_rejects_unsafe_environment_values_without_writing(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let before = fs::read(&path)?;

    let output = command(&path)?
        .args(["setup", "--from-env", "--dry-run"])
        .env("ATLASSIAN_HOST", "example.atlassian.net")
        .env("ATLASSIAN_EMAIL", "person@example.com")
        .env("ATLASSIAN_TOKEN", "jira-token\nwith-control")
        .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read(&path)?, before);
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_input");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!stderr.contains("jira-token\nwith-control"));
    assert!(!stderr.contains("tempo-token-must-not-leak"));
    Ok(())
}

#[test]
fn unattended_setup_rejects_argument_and_json_secret_transport_without_echoing_secrets(
) -> Result<(), Box<dyn std::error::Error>> {
    for arguments in [
        vec![
            "setup",
            "--from-env",
            "--atlassian-token",
            "argument-secret",
        ],
        vec!["setup", "--from-env", "positional-secret"],
        vec![
            "setup",
            "--from-env",
            "--json",
            r#"{"atlassianToken":"json-secret","tempoToken":"json-tempo-secret"}"#,
        ],
        vec![
            "setup",
            "--from-env",
            r#"--json={"atlassianToken":"equals-json-secret"}"#,
        ],
    ] {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let output = command(&path)?.args(arguments).output()?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)?;
        assert!(!stderr.contains("argument-secret"));
        assert!(!stderr.contains("json-secret"));
        assert!(!stderr.contains("json-tempo-secret"));
        assert!(!stderr.contains("positional-secret"));
        assert!(!stderr.contains("equals-json-secret"));
        assert!(!path.exists());
    }
    Ok(())
}

#[test]
fn headless_setup_rejects_unsafe_jira_sites_without_network_access(
) -> Result<(), Box<dyn std::error::Error>> {
    for site in [
        "http://example.atlassian.net",
        "https://user:password@example.atlassian.net",
        "example.atlassian.net/path",
    ] {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let output = command(&path)?
            .args(["setup", "--from-env"])
            .env("ATLASSIAN_HOST", site)
            .env("ATLASSIAN_EMAIL", "person@example.com")
            .env("ATLASSIAN_TOKEN", "jira-token-must-not-leak")
            .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
            .output()?;
        assert_eq!(output.status.code(), Some(2));
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], "invalid_input");
    }
    Ok(())
}

#[test]
fn headless_setup_parses_existing_config_before_reading_credentials(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    fs::write(&path, "{not valid json")?;
    let before = fs::read(&path)?;
    let output = command(&path)?
        .args(["setup", "--from-env"])
        .env_remove("ATLASSIAN_HOST")
        .env_remove("ATLASSIAN_EMAIL")
        .env_remove("ATLASSIAN_TOKEN")
        .env_remove("TEMPO_TOKEN")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "config_error");
    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[test]
fn interactive_setup_without_a_terminal_points_automation_to_from_env(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let output = command(&path)?.arg("setup").output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("setup --from-env")));
    assert!(!path.exists());
    Ok(())
}
