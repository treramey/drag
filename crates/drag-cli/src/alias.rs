use std::path::Path;

use serde_json::json;

use crate::cli::{AliasDeleteArgs, AliasSetArgs};
use crate::config::Config;
use crate::{CliError, Rendered};

pub(crate) fn set(path: &Path, args: AliasSetArgs) -> Result<Rendered, CliError> {
    let mut config = Config::load(path)?;
    config
        .aliases
        .insert(args.alias.clone(), args.issue_key.clone());
    config.save(path)?;
    Ok(Rendered::new(
        json!({"alias": args.alias, "issueKey": args.issue_key}),
        format!("{} => {}", args.alias, args.issue_key),
    ))
}

pub(crate) fn delete(path: &Path, args: AliasDeleteArgs) -> Result<Rendered, CliError> {
    let mut config = Config::load(path)?;
    let issue_key = config.aliases.remove(&args.alias_name);
    config.save(path)?;
    Ok(Rendered::new(
        json!({"alias": args.alias_name, "deleted": issue_key.is_some(), "issueKey": issue_key}),
        if issue_key.is_some() {
            format!("Deleted alias {}.", args.alias_name)
        } else {
            format!("Alias {} did not exist.", args.alias_name)
        },
    ))
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
