//! Fixed-origin Tempo OpenAPI discovery, caching, and operation lookup.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use reqwest::header::{ACCEPT, CONTENT_LENGTH, ETAG, IF_NONE_MATCH};
use reqwest::StatusCode;
use serde_json::{json, Value};
use url::Url;

use crate::api::ApiClient;
use crate::config::Config;
use crate::{transport, CliError, Rendered};

pub(crate) const TEMPO_OPENAPI_URL: &str = "https://apidocs.tempo.io/tempo-openapi.yaml";
const CACHE_FILE: &str = "tempo-openapi.yaml";
const ETAG_FILE: &str = "tempo-openapi.etag";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REF_DEPTH: usize = 64;

struct LoadedDocument {
    value: Value,
    cached: bool,
}

#[derive(Clone)]
struct OperationDescriptor {
    resource: String,
    method: String,
    friendly_alias: Option<String>,
    operation_id: String,
    http_method: String,
    api_path: String,
    path_parameters: Vec<Value>,
    definition: Value,
}

struct PreparedRequest {
    operation_id: String,
    method: String,
    url: Url,
    body: Option<Value>,
}

pub(crate) enum CommandOutput {
    Rendered(Rendered),
    Plain(String),
}

pub(crate) async fn run_command(
    arguments: Vec<String>,
    config_path: &Path,
    debug: bool,
) -> Result<CommandOutput, CliError> {
    let loaded = load_document().await?;
    let operations = executable_operations(&loaded.value)?;
    let command = build_command(&operations);
    let matches =
        match command.try_get_matches_from(std::iter::once("tempo".to_owned()).chain(arguments)) {
            Ok(matches) => matches,
            Err(error)
                if matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
            {
                return Ok(CommandOutput::Plain(error.to_string()));
            }
            Err(error) => return Err(CliError::Usage(error.to_string())),
        };
    let (resource, resource_matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::InvalidInput("missing Tempo OpenAPI resource".to_owned()))?;
    let (method, method_matches) = resource_matches
        .subcommand()
        .ok_or_else(|| CliError::InvalidInput("missing Tempo OpenAPI method".to_owned()))?;
    let operation = operations
        .iter()
        .find(|operation| operation.resource == resource && operation.method == method)
        .ok_or_else(|| CliError::InvalidInput("unknown Tempo OpenAPI method".to_owned()))?;
    let prepared = prepare_request(
        operation,
        method_matches
            .get_one::<String>("params")
            .map(String::as_str),
        operation
            .definition
            .get("requestBody")
            .and_then(|_| method_matches.get_one::<String>("json"))
            .map(String::as_str),
        &loaded.value,
    )?;
    if method_matches.get_flag("dry-run") {
        return render_prepared_request(&prepared, true, Value::Null).map(CommandOutput::Rendered);
    }

    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let api = ApiClient::new(credentials, debug)?;
    let method = reqwest::Method::from_bytes(prepared.method.as_bytes())
        .map_err(|error| CliError::Api(format!("Tempo OpenAPI method is invalid: {error}")))?;
    let response = api
        .execute_openapi_value(method, prepared.url.clone(), prepared.body.as_ref())
        .await?;
    render_prepared_request(&prepared, false, response).map(CommandOutput::Rendered)
}

fn render_prepared_request(
    prepared: &PreparedRequest,
    dry_run: bool,
    response: Value,
) -> Result<Rendered, CliError> {
    let data = if dry_run {
        json!({
            "dryRun": true,
            "operationId": prepared.operation_id,
            "method": prepared.method,
            "url": prepared.url.as_str(),
            "body": prepared.body
        })
    } else {
        response
    };
    let human = serde_json::to_string_pretty(&data)?;
    Ok(Rendered::new(data, human))
}

pub(crate) async fn operation_schema(
    dotted_path: &str,
    resolve_refs: bool,
) -> Result<Rendered, CliError> {
    let loaded = load_document().await?;
    let (http_method, api_path, mut operation) = find_operation(&loaded.value, dotted_path)?;
    if resolve_refs {
        let mut stack = HashSet::new();
        resolve_local_refs(&mut operation, &loaded.value, &mut stack, 0)?;
    }
    let operation_object = operation
        .as_object_mut()
        .ok_or_else(|| CliError::Api("Tempo OpenAPI operation is not an object".to_owned()))?;
    operation_object.insert("httpMethod".to_owned(), Value::String(http_method));
    operation_object.insert("path".to_owned(), Value::String(api_path));

    let openapi = loaded
        .value
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::Api("Tempo OpenAPI document has no version".to_owned()))?;
    let data = json!({
        "path": dotted_path,
        "source": {
            "kind": "tempoOpenApi",
            "url": TEMPO_OPENAPI_URL,
            "openapi": openapi,
            "cached": loaded.cached
        },
        "operation": operation
    });
    let human = serde_json::to_string_pretty(&data)?;
    Ok(Rendered::new(data, human))
}

async fn load_document() -> Result<LoadedDocument, CliError> {
    let cache_dir = cache_dir()?;
    let cache_path = cache_dir.join(CACHE_FILE);
    if cache_is_fresh(&cache_path) {
        if let Ok(value) = read_document(&cache_path) {
            return Ok(LoadedDocument {
                value,
                cached: true,
            });
        }
    }

    let client = transport::shared_client()?;
    let mut request = client
        .get(TEMPO_OPENAPI_URL)
        .header(ACCEPT, "application/yaml");
    let etag_path = cache_dir.join(ETAG_FILE);
    if cache_path.exists() {
        if let Ok(etag) = fs::read_to_string(&etag_path) {
            request = request.header(IF_NONE_MATCH, etag.trim());
        }
    }
    let response = request.send().await?;
    if response.status() == StatusCode::NOT_MODIFIED {
        let bytes = fs::read(&cache_path)?;
        write_cache(&cache_path, &bytes)?;
        return Ok(LoadedDocument {
            value: parse_document(&bytes)?,
            cached: true,
        });
    }
    if !response.status().is_success() {
        return Err(CliError::Api(format!(
            "Tempo OpenAPI discovery returned {}",
            response.status()
        )));
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_DOCUMENT_BYTES)
    {
        return Err(CliError::Api(
            "Tempo OpenAPI document exceeds the 2 MiB safety limit".to_owned(),
        ));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_DOCUMENT_BYTES {
        return Err(CliError::Api(
            "Tempo OpenAPI document exceeds the 2 MiB safety limit".to_owned(),
        ));
    }
    let value = parse_document(&bytes)?;
    fs::create_dir_all(&cache_dir)?;
    write_cache(&cache_path, &bytes)?;
    if let Some(etag) = etag {
        fs::write(etag_path, etag)?;
    }
    Ok(LoadedDocument {
        value,
        cached: false,
    })
}

fn cache_dir() -> Result<PathBuf, CliError> {
    if let Some(path) = env::var_os("DRAG_CACHE_DIR") {
        return Ok(PathBuf::from(path));
    }
    dirs::cache_dir()
        .map(|path| path.join("drag"))
        .ok_or_else(|| CliError::Config {
            message: "could not determine the cache directory".to_owned(),
            source: None,
        })
}

fn cache_is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < CACHE_TTL)
}

fn read_document(path: &Path) -> Result<Value, CliError> {
    parse_document(&fs::read(path)?)
}

fn parse_document(bytes: &[u8]) -> Result<Value, CliError> {
    serde_yaml_ng::from_slice(bytes)
        .map_err(|error| CliError::Api(format!("Tempo returned invalid OpenAPI YAML: {error}")))
}

fn write_cache(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn executable_operations(document: &Value) -> Result<Vec<OperationDescriptor>, CliError> {
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Api("Tempo OpenAPI document has no paths".to_owned()))?;
    let mut operations = Vec::new();
    for (api_path, path_item) in paths {
        if !api_path.starts_with("/4/") {
            continue;
        }
        for http_method in ["get", "post", "put", "patch", "delete"] {
            let Some(operation) = path_item.get(http_method) else {
                continue;
            };
            let Some(resource) = operation
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|tags| tags.first())
                .and_then(Value::as_str)
                .map(kebab_case)
            else {
                continue;
            };
            let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            operations.push(OperationDescriptor {
                resource: resource.clone(),
                method: kebab_case(operation_id),
                friendly_alias: friendly_method_name(
                    http_method,
                    api_path,
                    operation_id,
                    &resource,
                ),
                operation_id: operation_id.to_owned(),
                http_method: http_method.to_ascii_uppercase(),
                api_path: api_path.clone(),
                path_parameters: path_item
                    .get("parameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                definition: operation.clone(),
            });
        }
    }
    let mut alias_counts = HashMap::new();
    for operation in &operations {
        if let Some(alias) = &operation.friendly_alias {
            *alias_counts
                .entry((operation.resource.clone(), alias.clone()))
                .or_insert(0_usize) += 1;
        }
    }
    for operation in &mut operations {
        if operation.friendly_alias.as_ref().is_some_and(|alias| {
            alias_counts
                .get(&(operation.resource.clone(), alias.clone()))
                .copied()
                .unwrap_or_default()
                != 1
        }) {
            operation.friendly_alias = None;
        }
    }
    operations.sort_by(|left, right| {
        (&left.resource, &left.method).cmp(&(&right.resource, &right.method))
    });
    Ok(operations)
}

fn build_command(operations: &[OperationDescriptor]) -> clap::Command {
    use clap::{Arg, ArgAction, Command};

    let mut resources: BTreeMap<&str, Vec<&OperationDescriptor>> = BTreeMap::new();
    for operation in operations {
        resources
            .entry(&operation.resource)
            .or_default()
            .push(operation);
    }
    let mut root = Command::new("tempo")
        .about("Call Tempo operations generated from the official OpenAPI document")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for (resource, operations) in resources {
        let mut resource_command = Command::new(resource.to_owned())
            .subcommand_required(true)
            .arg_required_else_help(true);
        for operation in operations {
            let about = operation
                .definition
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or(&operation.operation_id)
                .to_owned();
            let mut method = Command::new(operation.method.clone())
                .about(about)
                .arg(
                    Arg::new("params")
                        .long("params")
                        .value_name("JSON")
                        .help("JSON object containing OpenAPI path and query parameters"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .help("Validate and print the request without calling Tempo")
                        .action(ArgAction::SetTrue),
                );
            if operation.definition.get("requestBody").is_some() {
                method = method.arg(
                    Arg::new("json")
                        .long("json")
                        .value_name("JSON")
                        .help("JSON request body validated against the OpenAPI schema"),
                );
            }
            if let Some(alias) = &operation.friendly_alias {
                method = method.visible_alias(alias.clone());
            }
            resource_command = resource_command.subcommand(method);
        }
        root = root.subcommand(resource_command);
    }
    root
}

fn prepare_request(
    operation: &OperationDescriptor,
    raw_params: Option<&str>,
    raw_body: Option<&str>,
    document: &Value,
) -> Result<PreparedRequest, CliError> {
    prepare_request_with_base(
        operation,
        raw_params,
        raw_body,
        document,
        Url::parse("https://api.tempo.io")?,
    )
}

fn prepare_request_with_base(
    operation: &OperationDescriptor,
    raw_params: Option<&str>,
    raw_body: Option<&str>,
    document: &Value,
    mut url: Url,
) -> Result<PreparedRequest, CliError> {
    if !operation.api_path.starts_with("/4/") {
        return Err(CliError::InvalidInput(
            "only Tempo API v4 operations are available".to_owned(),
        ));
    }
    let supplied = match raw_params {
        Some(raw) => serde_json::from_str::<Value>(raw)?,
        None => json!({}),
    };
    let supplied = supplied
        .as_object()
        .ok_or_else(|| CliError::InvalidInput("--params must be a JSON object".to_owned()))?;
    let definitions = parameter_definitions(operation, document)?;
    for key in supplied.keys() {
        if !definitions.contains_key(key) {
            return Err(CliError::InvalidInput(format!(
                "unknown parameter '{key}' for {}",
                operation.operation_id
            )));
        }
    }
    for (name, definition) in &definitions {
        if definition
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !supplied.contains_key(name)
        {
            return Err(CliError::InvalidInput(format!(
                "missing required parameter '{name}'"
            )));
        }
    }

    {
        let mut segments = url.path_segments_mut().map_err(|()| {
            CliError::InvalidInput("Tempo API base cannot contain path segments".to_owned())
        })?;
        segments.pop_if_empty();
        for segment in operation.api_path.trim_start_matches('/').split('/') {
            if let Some(name) = segment
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
            {
                let value = supplied.get(name).ok_or_else(|| {
                    CliError::InvalidInput(format!("missing required path parameter '{name}'"))
                })?;
                let values = validated_parameter_values(name, value, &definitions[name])?;
                let value = values.first().ok_or_else(|| {
                    CliError::InvalidInput(format!("path parameter '{name}' cannot be empty"))
                })?;
                segments.push(value);
            } else if segment.contains(['{', '}']) {
                return Err(CliError::Api(
                    "Tempo OpenAPI path contains an unsupported parameter template".to_owned(),
                ));
            } else {
                segments.push(segment);
            }
        }
    }
    let mut query_pairs = Vec::new();
    for (name, value) in supplied {
        let definition = &definitions[name];
        if definition.get("in").and_then(Value::as_str) != Some("query") {
            continue;
        }
        for value in validated_parameter_values(name, value, definition)? {
            query_pairs.push((name, value));
        }
    }
    if !query_pairs.is_empty() {
        let mut query = url.query_pairs_mut();
        for (name, value) in query_pairs {
            query.append_pair(name, &value);
        }
    }
    let body = request_body(operation, raw_body, document)?;
    Ok(PreparedRequest {
        operation_id: operation.operation_id.clone(),
        method: operation.http_method.clone(),
        url,
        body,
    })
}

fn request_body(
    operation: &OperationDescriptor,
    raw_body: Option<&str>,
    document: &Value,
) -> Result<Option<Value>, CliError> {
    let Some(mut request_body) = operation.definition.get("requestBody").cloned() else {
        return Ok(None);
    };
    let mut stack = HashSet::new();
    resolve_local_refs(&mut request_body, document, &mut stack, 0)?;
    let required = request_body
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(raw_body) = raw_body else {
        if required {
            return Err(CliError::InvalidInput(
                "missing required JSON request body; pass --json".to_owned(),
            ));
        }
        return Ok(None);
    };
    let body = serde_json::from_str::<Value>(raw_body)?;
    let mut schema = request_body
        .pointer("/content/application~1json/schema")
        .cloned()
        .ok_or_else(|| {
            CliError::Api("Tempo OpenAPI request body has no application/json schema".to_owned())
        })?;
    let mut stack = HashSet::new();
    resolve_local_refs(&mut schema, document, &mut stack, 0)?;
    validate_json_schema(&body, &schema, "body")?;
    Ok(Some(body))
}

fn validate_json_schema(value: &Value, schema: &Value, path: &str) -> Result<(), CliError> {
    if value.is_null()
        && schema
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(());
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(CliError::InvalidInput(format!(
                "{path} is not an allowed value"
            )));
        }
    }
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        if variants
            .iter()
            .any(|variant| validate_json_schema(value, variant, path).is_ok())
        {
            return Ok(());
        }
        return Err(CliError::InvalidInput(format!(
            "{path} does not match an allowed OpenAPI shape"
        )));
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        for part in parts {
            validate_json_schema(value, part, path)?;
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| CliError::InvalidInput(format!("{path} must be a JSON object")))?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(name) {
                        return Err(CliError::InvalidInput(format!("{path}.{name} is required")));
                    }
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                for name in object.keys() {
                    if !properties.is_some_and(|properties| properties.contains_key(name)) {
                        return Err(CliError::InvalidInput(format!(
                            "{path}.{name} is not declared by the OpenAPI schema"
                        )));
                    }
                }
            }
            if let Some(properties) = properties {
                for (name, property_schema) in properties {
                    if let Some(property) = object.get(name) {
                        validate_json_schema(property, property_schema, &format!("{path}.{name}"))?;
                    }
                }
            }
        }
        Some("array") => {
            let array = value
                .as_array()
                .ok_or_else(|| CliError::InvalidInput(format!("{path} must be a JSON array")))?;
            if let Some(items) = schema.get("items") {
                for (index, item) in array.iter().enumerate() {
                    validate_json_schema(item, items, &format!("{path}[{index}]"))?;
                }
            }
        }
        Some("string") if !value.is_string() => {
            return Err(CliError::InvalidInput(format!(
                "{path} must be a JSON string"
            )));
        }
        Some("integer") if value.as_i64().is_none() && value.as_u64().is_none() => {
            return Err(CliError::InvalidInput(format!(
                "{path} must be a JSON integer"
            )));
        }
        Some("number") if !value.is_number() => {
            return Err(CliError::InvalidInput(format!(
                "{path} must be a JSON number"
            )));
        }
        Some("boolean") if !value.is_boolean() => {
            return Err(CliError::InvalidInput(format!(
                "{path} must be a JSON boolean"
            )));
        }
        _ => {}
    }
    Ok(())
}

fn parameter_definitions(
    operation: &OperationDescriptor,
    document: &Value,
) -> Result<HashMap<String, Value>, CliError> {
    let mut definitions = HashMap::new();
    let mut parameters = operation.path_parameters.clone();
    parameters.extend(
        operation
            .definition
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    for mut parameter in parameters {
        let mut stack = HashSet::new();
        resolve_local_refs(&mut parameter, document, &mut stack, 0)?;
        let name = parameter
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| CliError::Api("Tempo OpenAPI parameter has no name".to_owned()))?;
        definitions.insert(name.to_owned(), parameter);
    }
    Ok(definitions)
}

fn validated_parameter_values(
    name: &str,
    value: &Value,
    definition: &Value,
) -> Result<Vec<String>, CliError> {
    let schema = definition.get("schema").unwrap_or(&Value::Null);
    let expected = schema.get("type").and_then(Value::as_str);
    let valid_type = match expected {
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some(_) | None => value.is_string() || value.is_number() || value.is_boolean(),
    };
    if !valid_type {
        return Err(CliError::InvalidInput(format!(
            "parameter '{name}' has the wrong JSON type"
        )));
    }
    let values = match value {
        Value::Array(values) => values
            .iter()
            .map(|value| scalar_parameter_value(name, value))
            .collect::<Result<Vec<_>, _>>()?,
        _ => vec![scalar_parameter_value(name, value)?],
    };
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        for value in &values {
            if !allowed
                .iter()
                .any(|allowed| allowed.as_str() == Some(value))
            {
                return Err(CliError::InvalidInput(format!(
                    "parameter '{name}' is not an allowed value"
                )));
            }
        }
    }
    Ok(values)
}

fn scalar_parameter_value(name: &str, value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(CliError::InvalidInput(format!(
            "parameter '{name}' must contain scalar values"
        ))),
    }
}

fn find_operation(
    document: &Value,
    dotted_path: &str,
) -> Result<(String, String, Value), CliError> {
    let segments = dotted_path.split('.').collect::<Vec<_>>();
    if segments.len() != 3 || segments[0] != "tempo" {
        return Err(CliError::InvalidInput(
            "Tempo schema path must use tempo.<resource>.<method>".to_owned(),
        ));
    }
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Api("Tempo OpenAPI document has no paths".to_owned()))?;
    let mut matches = Vec::new();
    for (api_path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for method in ["get", "post", "put", "patch", "delete"] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let Some(resource) = operation
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|tags| tags.first())
                .and_then(Value::as_str)
                .map(kebab_case)
            else {
                continue;
            };
            if resource != segments[1] {
                continue;
            }
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let canonical_method = kebab_case(operation_id);
            let friendly_method = friendly_method_name(method, api_path, operation_id, &resource);
            if segments[2] == canonical_method || friendly_method.as_deref() == Some(segments[2]) {
                matches.push((method.to_uppercase(), api_path.clone(), operation.clone()));
            }
        }
    }
    match matches.len() {
        1 => matches
            .pop()
            .ok_or_else(|| CliError::Api("Tempo operation lookup failed".to_owned())),
        0 => Err(CliError::InvalidInput(format!(
            "unknown Tempo OpenAPI operation '{dotted_path}'"
        ))),
        _ => Err(CliError::InvalidInput(format!(
            "ambiguous Tempo OpenAPI operation '{dotted_path}'; use its operation ID"
        ))),
    }
}

fn friendly_method_name(
    http_method: &str,
    api_path: &str,
    operation_id: &str,
    resource: &str,
) -> Option<String> {
    let resource_compact = resource.replace('-', "");
    let operation_compact = operation_id.to_ascii_lowercase();
    let singular_resource = resource_compact
        .strip_suffix('s')
        .unwrap_or(&resource_compact);
    for (prefix, action) in [
        ("create", "create"),
        ("update", "update"),
        ("delete", "delete"),
        ("search", "search"),
    ] {
        if let Some(subject) = operation_compact.strip_prefix(prefix) {
            return (subject == resource_compact || subject == singular_resource)
                .then(|| action.to_owned());
        }
    }
    if http_method == "get" {
        let subject = operation_compact.strip_prefix("get")?;
        let is_item = api_path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.starts_with('{'));
        if is_item && subject.starts_with(singular_resource) {
            return Some("get".to_owned());
        }
        if !is_item && (subject == resource_compact || subject == singular_resource) {
            return Some("list".to_owned());
        }
    }
    None
}

fn kebab_case(input: &str) -> String {
    let mut output = String::new();
    let mut previous_was_separator = true;
    let mut previous_was_lower_or_digit = false;
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase()
                && previous_was_lower_or_digit
                && !output.ends_with('-')
            {
                output.push('-');
            }
            output.push(character.to_ascii_lowercase());
            previous_was_separator = false;
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        } else if !previous_was_separator && !output.ends_with('-') {
            output.push('-');
            previous_was_separator = true;
            previous_was_lower_or_digit = false;
        }
    }
    output.trim_matches('-').to_owned()
}

fn resolve_local_refs(
    value: &mut Value,
    document: &Value,
    stack: &mut HashSet<String>,
    depth: usize,
) -> Result<(), CliError> {
    if depth > MAX_REF_DEPTH {
        return Err(CliError::Api(
            "Tempo OpenAPI reference depth exceeds the safety limit".to_owned(),
        ));
    }
    match value {
        Value::Object(object) => {
            if let Some(reference) = object
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                if let Some(pointer) = reference.strip_prefix('#') {
                    if !stack.insert(reference.clone()) {
                        return Ok(());
                    }
                    let mut resolved = document.pointer(pointer).cloned().ok_or_else(|| {
                        CliError::Api(
                            "Tempo OpenAPI document contains an invalid reference".to_owned(),
                        )
                    })?;
                    resolve_local_refs(&mut resolved, document, stack, depth + 1)?;
                    stack.remove(&reference);
                    if let Value::Object(resolved_object) = &mut resolved {
                        for (key, sibling) in object.iter() {
                            if key != "$ref" {
                                resolved_object.insert(key.clone(), sibling.clone());
                            }
                        }
                    }
                    *value = resolved;
                    return Ok(());
                }
            }
            for child in object.values_mut() {
                resolve_local_refs(child, document, stack, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                resolve_local_refs(child, document, stack, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use url::Url;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        executable_operations, friendly_method_name, kebab_case, parse_document,
        prepare_request_with_base,
    };
    use crate::api::ApiClient;
    use crate::config::Credentials;

    #[test]
    fn openapi_names_have_stable_cli_forms() {
        assert_eq!(kebab_case("Work Attributes"), "work-attributes");
        assert_eq!(kebab_case("createWorklog"), "create-worklog");
        assert_eq!(
            friendly_method_name("post", "/worklogs", "createWorklog", "worklogs").as_deref(),
            Some("create")
        );
        assert_eq!(
            friendly_method_name(
                "post",
                "/worklogs/work-attribute-values",
                "createWorkAttributeValuesForWorklogs",
                "worklogs",
            ),
            None
        );
        assert_eq!(
            friendly_method_name(
                "get",
                "/work-attributes",
                "getWorkAttributes",
                "work-attributes",
            )
            .as_deref(),
            Some("list")
        );
    }

    #[tokio::test]
    async fn generated_read_request_reaches_tempo_with_bearer_auth(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/work-attributes"))
            .and(query_param("limit", "25"))
            .and(header("authorization", "Bearer tempo-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "metadata": {"count": 1},
                "results": [{"key": "_Test_", "name": "Account", "required": true}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let document = parse_document(
            br#"openapi: 3.0.3
paths:
  /4/work-attributes:
    get:
      operationId: getWorkAttributes
      tags: [Work Attributes]
      parameters:
        - in: query
          name: limit
          schema: {type: integer}
components: {schemas: {}}
"#,
        )?;
        let operation = executable_operations(&document)?
            .into_iter()
            .find(|operation| operation.operation_id == "getWorkAttributes")
            .ok_or("missing generated operation")?;
        let base = Url::parse(&server.uri())?;
        let prepared = prepare_request_with_base(
            &operation,
            Some(r#"{"limit":25}"#),
            None,
            &document,
            base.clone(),
        )?;
        let api = ApiClient::with_bases(
            Credentials {
                tempo_token: "tempo-secret".to_owned(),
                account_id: "account-1".to_owned(),
                atlassian_user_email: "person@example.com".to_owned(),
                atlassian_token: "jira-secret".to_owned(),
                hostname: "example.atlassian.net".to_owned(),
            },
            false,
            Url::parse(&format!("{}/4/", server.uri()))?,
            base,
        )?;

        let response = api
            .execute_openapi_value(reqwest::Method::GET, prepared.url, prepared.body.as_ref())
            .await?;

        assert_eq!(response["metadata"]["count"], 1);
        assert_eq!(response["results"][0]["key"], "_Test_");
        Ok(())
    }

    #[tokio::test]
    async fn generated_mutation_validates_and_posts_json_once(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "issueId": 10001,
            "timeSpentSeconds": 3600
        });
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .and(header("authorization", "Bearer tempo-secret"))
            .and(body_json(&body))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tempoWorklogId": 42
            })))
            .expect(1)
            .mount(&server)
            .await;
        let document = parse_document(
            br#"openapi: 3.0.3
paths:
  /4/worklogs:
    post:
      operationId: createWorklog
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
components: {schemas: {}}
"#,
        )?;
        let operation = executable_operations(&document)?
            .into_iter()
            .find(|operation| operation.operation_id == "createWorklog")
            .ok_or("missing generated operation")?;
        let base = Url::parse(&server.uri())?;
        let prepared = prepare_request_with_base(
            &operation,
            None,
            Some(&body.to_string()),
            &document,
            base.clone(),
        )?;
        let api = ApiClient::with_bases(
            Credentials {
                tempo_token: "tempo-secret".to_owned(),
                account_id: "account-1".to_owned(),
                atlassian_user_email: "person@example.com".to_owned(),
                atlassian_token: "jira-secret".to_owned(),
                hostname: "example.atlassian.net".to_owned(),
            },
            false,
            Url::parse(&format!("{}/4/", server.uri()))?,
            base,
        )?;

        let response = api
            .execute_openapi_value(reqwest::Method::POST, prepared.url, prepared.body.as_ref())
            .await?;

        assert_eq!(response["tempoWorklogId"], 42);
        Ok(())
    }

    #[test]
    fn generated_paths_encode_user_values_as_single_segments(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let document = parse_document(
            br#"openapi: 3.0.3
paths:
  /4/accounts/{key}:
    parameters:
      - in: path
        name: key
        required: true
        schema: {type: string}
    get:
      operationId: getAccountByKey
      tags: [Accounts]
components: {schemas: {}}
"#,
        )?;
        let operation = executable_operations(&document)?
            .into_iter()
            .next()
            .ok_or("missing generated operation")?;

        let prepared = prepare_request_with_base(
            &operation,
            Some(r#"{"key":"a/b?admin=true"}"#),
            None,
            &document,
            Url::parse("https://api.tempo.io")?,
        )?;

        assert_eq!(
            prepared.url.as_str(),
            "https://api.tempo.io/4/accounts/a%2Fb%3Fadmin=true"
        );
        Ok(())
    }
}
