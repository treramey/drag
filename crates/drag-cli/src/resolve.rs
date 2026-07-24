use std::collections::BTreeMap;
use std::path::Path;

use drag::models::WorkAttribute;
use serde_json::json;

use crate::api::ApiClient;
use crate::cli::ResolveArgs;
use crate::config::{Config, Credentials};
use crate::{CliError, Rendered};

pub(crate) trait ResolveGateway: Send + Sync {
    async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError>;
    async fn required_work_attributes(&self) -> Result<Vec<WorkAttribute>, CliError>;
}

pub(crate) struct ApiResolveGateway {
    api: ApiClient,
}

impl ApiResolveGateway {
    pub(crate) fn new(credentials: Credentials, debug: bool) -> Result<Self, CliError> {
        Ok(Self {
            api: ApiClient::new(credentials, debug)?,
        })
    }
}

impl ResolveGateway for ApiResolveGateway {
    async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError> {
        self.api.get_issue_id(issue_key).await
    }

    async fn required_work_attributes(&self) -> Result<Vec<WorkAttribute>, CliError> {
        self.api.get_required_work_attributes().await
    }
}

pub(crate) async fn run<G>(
    config_path: &Path,
    args: ResolveArgs,
    make_gateway: impl FnOnce(Credentials) -> Result<G, CliError>,
) -> Result<Rendered, CliError>
where
    G: ResolveGateway,
{
    let issue_key = normalize_issue_key(&args.issue_key)?;
    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let tempo_account_id = credentials.account_id.clone();
    let gateway = make_gateway(credentials)?;
    let issue_id = gateway.resolve_issue_id(&issue_key).await?;
    let required_work_attributes = gateway.required_work_attributes().await?;
    let required_keys = required_work_attributes
        .iter()
        .map(|attribute| attribute.key.clone())
        .collect::<Vec<_>>();
    let attributes_by_key = required_work_attributes
        .iter()
        .map(|attribute| {
            (
                attribute.key.clone(),
                json!({
                    "key": attribute.key,
                    "name": attribute.name,
                    "required": attribute.required,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(Rendered::new(
        json!({
            "schemaVersion": 1,
            "readOnly": true,
            "liveMutationAllowed": false,
            "issue": {
                "key": issue_key,
                "id": issue_id,
            },
            "tempo": {
                "authenticatedAccountId": tempo_account_id,
                "requiredWorkAttributes": required_work_attributes.iter().map(|attribute| json!({
                    "key": attribute.key,
                    "name": attribute.name,
                    "required": attribute.required,
                })).collect::<Vec<_>>(),
                "requiredWorkAttributeKeys": required_keys,
                "requiredWorkAttributesByKey": attributes_by_key,
            },
        }),
        "Resolved Jira issue and required Tempo work attributes without mutation.".to_owned(),
    ))
}

fn normalize_issue_key(issue_key: &str) -> Result<String, CliError> {
    let trimmed = issue_key.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) || !trimmed.contains('-') {
        return Err(CliError::InvalidInput(
            "issue key must be a non-empty Jira key such as ABC-123".to_owned(),
        ));
    }
    Ok(trimmed.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Debug, PartialEq, Eq)]
    enum Operation {
        ResolveIssue(String),
        RequiredAttributes,
    }

    struct FakeResolveGateway {
        operations: Arc<Mutex<Vec<Operation>>>,
        issue_failure: bool,
        attributes: Vec<WorkAttribute>,
    }

    impl ResolveGateway for FakeResolveGateway {
        async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("operation lock poisoned".to_owned()))?
                .push(Operation::ResolveIssue(issue_key.to_owned()));
            if self.issue_failure {
                Err(CliError::Api("Jira issue was not found".to_owned()))
            } else {
                Ok("10001".to_owned())
            }
        }

        async fn required_work_attributes(&self) -> Result<Vec<WorkAttribute>, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("operation lock poisoned".to_owned()))?
                .push(Operation::RequiredAttributes);
            Ok(self.attributes.clone())
        }
    }

    fn configured_file(directory: &TempDir) -> Result<std::path::PathBuf, CliError> {
        let path = directory.path().join("config.json");
        Config {
            tempo_token: Some("tempo-secret".to_owned()),
            account_id: Some("account-1".to_owned()),
            atlassian_user_email: Some("person@example.com".to_owned()),
            atlassian_token: Some("atlassian-secret".to_owned()),
            hostname: Some("example.atlassian.net".to_owned()),
        }
        .save(&path)?;
        Ok(path)
    }

    #[tokio::test]
    async fn resolve_is_read_only_and_returns_stable_schema() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let operations = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeResolveGateway {
            operations: Arc::clone(&operations),
            issue_failure: false,
            attributes: vec![WorkAttribute {
                key: "_Account_".to_owned(),
                name: "Account".to_owned(),
                required: true,
            }],
        };

        let rendered = run(
            &path,
            ResolveArgs {
                issue_key: "drag-143".to_owned(),
            },
            |_| Ok(fake),
        )
        .await?;

        assert_eq!(rendered.data["schemaVersion"], 1);
        assert_eq!(rendered.data["readOnly"], true);
        assert_eq!(rendered.data["liveMutationAllowed"], false);
        assert_eq!(rendered.data["issue"]["key"], "DRAG-143");
        assert_eq!(rendered.data["issue"]["id"], "10001");
        assert_eq!(
            rendered.data["tempo"]["authenticatedAccountId"],
            "account-1"
        );
        assert_eq!(
            rendered.data["tempo"]["requiredWorkAttributeKeys"],
            serde_json::json!(["_Account_"])
        );
        assert_eq!(
            *operations
                .lock()
                .map_err(|_| CliError::Api("operation lock poisoned".to_owned()))?,
            vec![
                Operation::ResolveIssue("DRAG-143".to_owned()),
                Operation::RequiredAttributes,
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_fails_safely_before_attribute_lookup_when_issue_is_unknown(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let operations = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeResolveGateway {
            operations: Arc::clone(&operations),
            issue_failure: true,
            attributes: Vec::new(),
        };

        let error = run(
            &path,
            ResolveArgs {
                issue_key: "DRAG-404".to_owned(),
            },
            |_| Ok(fake),
        )
        .await
        .err()
        .ok_or_else(|| CliError::Api("expected unknown issue failure".to_owned()))?;

        assert!(error.to_string().contains("Jira issue was not found"));
        assert_eq!(
            *operations
                .lock()
                .map_err(|_| CliError::Api("operation lock poisoned".to_owned()))?,
            vec![Operation::ResolveIssue("DRAG-404".to_owned())]
        );
        Ok(())
    }
}
