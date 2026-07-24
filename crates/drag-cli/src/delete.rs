use std::io::{self, Read};
use std::path::Path;

use chrono_tz::Tz;
use drag::models::WorklogEntity;
use serde_json::{json, Value};

use crate::api::ApiClient;
use crate::cli::{DeleteArgs, DeleteInput};
use crate::config::{Config, Credentials};
use crate::output::escape_terminal_data;
use crate::{CliError, Rendered};

pub(crate) trait DeleteGateway: Send + Sync {
    async fn get_worklog(&self, id: u64) -> Result<WorklogEntity, CliError>;
    async fn get_issue_key(&self, issue_id: &str) -> Result<String, CliError>;
    async fn delete_worklog(&self, id: u64) -> Result<(), CliError>;
}

pub(crate) struct ApiDeleteGateway {
    api: ApiClient,
}

impl ApiDeleteGateway {
    pub(crate) fn new(credentials: Credentials, debug: bool) -> Result<Self, CliError> {
        Ok(Self {
            api: ApiClient::new(credentials, debug)?,
        })
    }
}

impl DeleteGateway for ApiDeleteGateway {
    async fn get_worklog(&self, id: u64) -> Result<WorklogEntity, CliError> {
        self.api.get_worklog(id).await
    }

    async fn get_issue_key(&self, issue_id: &str) -> Result<String, CliError> {
        self.api.get_issue_key(issue_id).await
    }

    async fn delete_worklog(&self, id: u64) -> Result<(), CliError> {
        self.api.delete_worklog(id).await
    }
}

pub(crate) async fn run<G>(
    config_path: &Path,
    timezone: Tz,
    args: DeleteArgs,
    make_gateway: impl FnOnce(Credentials) -> Result<G, CliError>,
) -> Result<Rendered, CliError>
where
    G: DeleteGateway,
{
    let (input, dry_run) = delete_input(args)?;
    let credentials = Config::load(config_path)?.credentials()?;
    let gateway = make_gateway(credentials)?;
    let mut deleted = Vec::with_capacity(input.worklog_ids.len());
    for id in input.worklog_ids {
        let entity = gateway.get_worklog(id).await?;
        let issue_key = gateway.get_issue_key(&entity.issue.id).await?;
        let worklog = crate::log::to_worklog(entity, issue_key, timezone)?;
        if !dry_run {
            gateway.delete_worklog(id).await?;
        }
        deleted.push(worklog);
    }
    let human = deleted
        .iter()
        .map(|worklog| {
            if dry_run {
                format!(
                    "Would delete worklog {} ({} {}).",
                    escape_terminal_data(&worklog.id),
                    escape_terminal_data(&worklog.issue_key),
                    escape_terminal_data(&worklog.duration)
                )
            } else {
                format!(
                    "Deleted worklog {} ({} {}).",
                    escape_terminal_data(&worklog.id),
                    escape_terminal_data(&worklog.issue_key),
                    escape_terminal_data(&worklog.duration)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Rendered::new(
        json!({"dryRun": dry_run, "worklogs": deleted}),
        human,
    ))
}

fn delete_input(args: DeleteArgs) -> Result<(DeleteInput, bool), CliError> {
    let input = if let Some(raw) = args.json {
        let raw = if raw == "-" {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input
        } else {
            raw
        };
        let value: Value = serde_json::from_str(&raw)?;
        if !value.is_object() {
            return Err(CliError::Json(serde_json::Error::io(io::Error::new(
                io::ErrorKind::InvalidData,
                "delete JSON input must be an object",
            ))));
        }
        serde_json::from_value(value)?
    } else {
        DeleteInput {
            worklog_ids: args.worklog_ids,
        }
    };
    if input.worklog_ids.is_empty() {
        return Err(CliError::InvalidInput(
            "at least one worklog ID is required".to_owned(),
        ));
    }
    Ok((input, args.dry_run))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use drag::models::{Author, Issue};
    use tempfile::TempDir;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Operation {
        Get(u64),
        Resolve(String),
        Delete(u64),
    }

    struct FakeGateway {
        operations: Arc<Mutex<Vec<Operation>>>,
    }

    impl DeleteGateway for FakeGateway {
        async fn get_worklog(&self, id: u64) -> Result<WorklogEntity, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("test operations lock was poisoned".to_owned()))?
                .push(Operation::Get(id));
            Ok(WorklogEntity {
                tempo_worklog_id: id.to_string(),
                start_date: "2026-07-14".to_owned(),
                start_time: "09:00:00".to_owned(),
                author: Author {
                    account_id: "me".to_owned(),
                },
                issue: Issue {
                    self_url: format!("https://example.atlassian.net/rest/api/3/issue/{id}"),
                    id: id.to_string(),
                },
                description: String::new(),
                time_spent_seconds: 3_600,
                attributes: Default::default(),
            })
        }

        async fn get_issue_key(&self, issue_id: &str) -> Result<String, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("test operations lock was poisoned".to_owned()))?
                .push(Operation::Resolve(issue_id.to_owned()));
            Ok(format!("KEY-{issue_id}"))
        }

        async fn delete_worklog(&self, id: u64) -> Result<(), CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("test operations lock was poisoned".to_owned()))?
                .push(Operation::Delete(id));
            Ok(())
        }
    }

    fn configured_file(directory: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
            "tempoToken":"tempo-secret","accountId":"account-1",
            "atlassianUserEmail":"person@example.com","atlassianToken":"jira-secret",
            "hostname":"example.atlassian.net"
        }"#,
        )?;
        Ok(path)
    }

    #[tokio::test]
    async fn ordered_batch_plan_drives_dry_run_and_execution() -> Result<(), CliError> {
        for structured in [false, true] {
            for dry_run in [true, false] {
                let directory = TempDir::new()?;
                let path = configured_file(&directory)?;
                let operations = Arc::new(Mutex::new(Vec::new()));
                let fake_operations = Arc::clone(&operations);
                let args = if structured {
                    DeleteArgs {
                        worklog_ids: Vec::new(),
                        json: Some(r#"{"worklogIds":[2,1,2]}"#.to_owned()),
                        dry_run,
                    }
                } else {
                    DeleteArgs {
                        worklog_ids: vec![2, 1, 2],
                        json: None,
                        dry_run,
                    }
                };
                let rendered = run(&path, chrono_tz::UTC, args, |_| {
                    Ok(FakeGateway {
                        operations: fake_operations,
                    })
                })
                .await?;

                assert_eq!(rendered.data["worklogs"][0]["id"], "2");
                assert_eq!(rendered.data["worklogs"][1]["id"], "1");
                assert_eq!(rendered.data["worklogs"][2]["id"], "2");
                let expected = if dry_run {
                    vec![
                        Operation::Get(2),
                        Operation::Resolve("2".to_owned()),
                        Operation::Get(1),
                        Operation::Resolve("1".to_owned()),
                        Operation::Get(2),
                        Operation::Resolve("2".to_owned()),
                    ]
                } else {
                    vec![
                        Operation::Get(2),
                        Operation::Resolve("2".to_owned()),
                        Operation::Delete(2),
                        Operation::Get(1),
                        Operation::Resolve("1".to_owned()),
                        Operation::Delete(1),
                        Operation::Get(2),
                        Operation::Resolve("2".to_owned()),
                        Operation::Delete(2),
                    ]
                };
                let actual = operations
                    .lock()
                    .map_err(|_| CliError::Api("test operations lock was poisoned".to_owned()))?;
                assert_eq!(*actual, expected);
            }
        }
        Ok(())
    }
}
