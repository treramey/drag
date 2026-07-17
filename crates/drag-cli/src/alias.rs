use std::io::{self, Read};
use std::path::Path;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::{AliasDeleteArgs, AliasDeleteInput, AliasSetArgs, AliasSetInput};
use crate::config::Config;
use crate::{CliError, Rendered};

pub(crate) fn set(path: &Path, args: AliasSetArgs) -> Result<Rendered, CliError> {
    let (input, dry_run) = set_input(args)?;
    let mut config = Config::load(path)?;
    let plan = SetAliasPlan::new(&config, input);
    if !dry_run && plan.apply(&mut config) {
        config.save(path)?;
    }
    plan.render(dry_run)
}

pub(crate) fn delete(path: &Path, args: AliasDeleteArgs) -> Result<Rendered, CliError> {
    let (input, dry_run) = delete_input(args)?;
    let mut config = Config::load(path)?;
    let plan = DeleteAliasPlan::new(&config, input);
    if !dry_run && plan.apply(&mut config) {
        config.save(path)?;
    }
    plan.render(dry_run)
}

pub(crate) fn list(path: &Path) -> Result<Rendered, CliError> {
    let config = Config::load(path)?;
    let human = if config.aliases.is_empty() {
        "No aliases configured.".to_owned()
    } else {
        config
            .aliases
            .iter()
            .map(|(alias, issue)| format!("{alias} => {issue}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Rendered::new(json!({"aliases": config.aliases}), human))
}

fn set_input(args: AliasSetArgs) -> Result<(AliasSetInput, bool), CliError> {
    let dry_run = args.dry_run;
    let input = if let Some(raw) = args.json {
        structured_input(raw)?
    } else {
        AliasSetInput {
            alias: args
                .alias
                .ok_or_else(|| CliError::InvalidInput("missing alias".to_owned()))?,
            issue_key: args
                .issue_key
                .ok_or_else(|| CliError::InvalidInput("missing issue key".to_owned()))?,
        }
    };
    Ok((input, dry_run))
}

fn delete_input(args: AliasDeleteArgs) -> Result<(AliasDeleteInput, bool), CliError> {
    let dry_run = args.dry_run;
    let input = if let Some(raw) = args.json {
        structured_input(raw)?
    } else {
        AliasDeleteInput {
            alias: args
                .alias_name
                .ok_or_else(|| CliError::InvalidInput("missing alias".to_owned()))?,
        }
    };
    Ok((input, dry_run))
}

fn raw_input(raw: String) -> Result<String, CliError> {
    if raw != "-" {
        return Ok(raw);
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    Ok(input)
}

fn structured_input<T: DeserializeOwned>(raw: String) -> Result<T, CliError> {
    let value: Value = serde_json::from_str(&raw_input(raw)?)?;
    if !value.is_object() {
        return Err(CliError::Json(serde_json::Error::io(io::Error::new(
            io::ErrorKind::InvalidData,
            "alias JSON input must be an object",
        ))));
    }
    Ok(serde_json::from_value(value)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum SetAliasAction {
    Create,
    Replace,
    Unchanged,
}

struct SetAliasPlan {
    alias: String,
    issue_key: String,
    previous_issue_key: Option<String>,
    action: SetAliasAction,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AliasSetResult {
    alias: String,
    issue_key: String,
    action: SetAliasAction,
    previous_issue_key: Option<String>,
    dry_run: bool,
}

impl SetAliasPlan {
    fn new(config: &Config, input: AliasSetInput) -> Self {
        let previous_issue_key = config.aliases.get(&input.alias).cloned();
        let action = match previous_issue_key.as_deref() {
            None => SetAliasAction::Create,
            Some(current) if current == input.issue_key => SetAliasAction::Unchanged,
            Some(_) => SetAliasAction::Replace,
        };
        Self {
            alias: input.alias,
            issue_key: input.issue_key,
            previous_issue_key,
            action,
        }
    }

    fn apply(&self, config: &mut Config) -> bool {
        if self.action == SetAliasAction::Unchanged {
            return false;
        }
        config
            .aliases
            .insert(self.alias.clone(), self.issue_key.clone());
        true
    }

    fn render(self, dry_run: bool) -> Result<Rendered, CliError> {
        let human = if dry_run {
            match self.action {
                SetAliasAction::Create => {
                    format!("Would create alias {} => {}.", self.alias, self.issue_key)
                }
                SetAliasAction::Replace => format!(
                    "Would replace alias {}: {} => {}.",
                    self.alias,
                    self.previous_issue_key.as_deref().unwrap_or_default(),
                    self.issue_key
                ),
                SetAliasAction::Unchanged => format!(
                    "Alias {} would remain unchanged at {}.",
                    self.alias, self.issue_key
                ),
            }
        } else {
            format!("{} => {}", self.alias, self.issue_key)
        };
        let result = AliasSetResult {
            alias: self.alias,
            issue_key: self.issue_key,
            action: self.action,
            previous_issue_key: self.previous_issue_key,
            dry_run,
        };
        Ok(Rendered::new(serde_json::to_value(result)?, human))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum DeleteAliasAction {
    Delete,
    Unchanged,
}

struct DeleteAliasPlan {
    alias: String,
    issue_key: Option<String>,
    action: DeleteAliasAction,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AliasDeleteResult {
    alias: String,
    deleted: bool,
    issue_key: Option<String>,
    action: DeleteAliasAction,
    dry_run: bool,
}

impl DeleteAliasPlan {
    fn new(config: &Config, input: AliasDeleteInput) -> Self {
        let issue_key = config.aliases.get(&input.alias).cloned();
        let action = if issue_key.is_some() {
            DeleteAliasAction::Delete
        } else {
            DeleteAliasAction::Unchanged
        };
        Self {
            alias: input.alias,
            issue_key,
            action,
        }
    }

    fn apply(&self, config: &mut Config) -> bool {
        if self.action == DeleteAliasAction::Unchanged {
            return false;
        }
        config.aliases.remove(&self.alias);
        true
    }

    fn render(self, dry_run: bool) -> Result<Rendered, CliError> {
        let human = match (dry_run, self.action) {
            (true, DeleteAliasAction::Delete) => format!(
                "Would delete alias {} => {}.",
                self.alias,
                self.issue_key.as_deref().unwrap_or_default()
            ),
            (true, DeleteAliasAction::Unchanged) => format!(
                "Alias {} would remain unchanged because it does not exist.",
                self.alias
            ),
            (false, DeleteAliasAction::Delete) => format!("Deleted alias {}.", self.alias),
            (false, DeleteAliasAction::Unchanged) => {
                format!("Alias {} did not exist.", self.alias)
            }
        };
        let result = AliasDeleteResult {
            alias: self.alias,
            deleted: !dry_run && self.action == DeleteAliasAction::Delete,
            issue_key: self.issue_key,
            action: self.action,
            dry_run,
        };
        Ok(Rendered::new(serde_json::to_value(result)?, human))
    }
}
