use std::path::Path;

use chrono_tz::Tz;
use serde_json::json;

use crate::app::ConnectionEnvironment;
use crate::config::Config;
use crate::setup::ConnectionVerifier;
use crate::{CliError, Rendered, EXIT_USAGE};

struct ServiceCheck {
    status: ServiceStatus,
    error_code: Option<&'static str>,
    exit_code: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServiceStatus {
    Connected,
    NotConfigured,
    Failed,
}

impl ServiceStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::NotConfigured => "notConfigured",
            Self::Failed => "failed",
        }
    }
}

impl ServiceCheck {
    fn connected() -> Self {
        Self {
            status: ServiceStatus::Connected,
            error_code: None,
            exit_code: 0,
        }
    }

    fn not_configured() -> Self {
        Self {
            status: ServiceStatus::NotConfigured,
            error_code: None,
            exit_code: EXIT_USAGE,
        }
    }

    fn failed(error: &CliError) -> Self {
        Self {
            status: ServiceStatus::Failed,
            error_code: Some(error.code()),
            exit_code: error.exit_code(),
        }
    }

    fn preparation_failed(error: &CliError) -> Self {
        if matches!(error, CliError::NotConfigured(_)) {
            Self::not_configured()
        } else {
            Self::failed(error)
        }
    }

    fn is_connected(&self) -> bool {
        self.status == ServiceStatus::Connected
    }

    fn json(&self) -> serde_json::Value {
        let mut value = json!({"status": self.status.as_str()});
        if let Some(error_code) = self.error_code {
            value["errorCode"] = json!(error_code);
        }
        value
    }

    fn human(&self, service: &str) -> String {
        match self.status {
            ServiceStatus::Connected => format!("{service}: connected"),
            ServiceStatus::NotConfigured => format!("{service}: not configured"),
            ServiceStatus::Failed => format!(
                "{service}: failed ({})",
                self.error_code.unwrap_or("runtime_failure")
            ),
        }
    }
}

pub(crate) async fn run(
    path: &Path,
    timezone: Tz,
    remote: bool,
    debug: bool,
    environment: &dyn ConnectionEnvironment,
    verifier: &dyn ConnectionVerifier,
) -> Result<Rendered, CliError> {
    let config = Config::load(path)?;
    let configured = configured_fields(&config, environment);
    let jira_configured = configured["atlassianHost"].as_bool() == Some(true)
        && configured["atlassianEmail"].as_bool() == Some(true)
        && configured["atlassianToken"].as_bool() == Some(true);
    let tempo_configured = configured["tempoToken"].as_bool() == Some(true)
        && configured["accountId"].as_bool() == Some(true);
    let mut report = json!({
        "name": "drag",
        "version": env!("CARGO_PKG_VERSION"),
        "configPath": path,
        "configured": configured,
        "aliases": config.aliases.len(),
        "timezone": timezone.name(),
        "target": {
            "architecture": std::env::consts::ARCH,
            "operatingSystem": std::env::consts::OS
        }
    });
    let mut human = format!(
        "drag {}\nconfig: {}\ntimezone: {}\naliases: {}\nJira: {}\nTempo: {}",
        env!("CARGO_PKG_VERSION"),
        path.display(),
        timezone.name(),
        config.aliases.len(),
        configured_label(jira_configured),
        configured_label(tempo_configured),
    );

    if !remote {
        return Ok(Rendered::new(report, human));
    }

    let jira = match config.jira_credentials_from_source(|name| environment.value(name)) {
        Ok(connection) => match verifier.verify_jira(&connection, debug).await {
            Ok(_) => ServiceCheck::connected(),
            Err(error) => ServiceCheck::failed(&error),
        },
        Err(error) => ServiceCheck::preparation_failed(&error),
    };
    let tempo = match config.tempo_credentials_from_source(|name| environment.value(name)) {
        Ok(connection) => match verifier.verify_tempo(&connection, debug).await {
            Ok(()) => ServiceCheck::connected(),
            Err(error) => ServiceCheck::failed(&error),
        },
        Err(error) => ServiceCheck::preparation_failed(&error),
    };
    let successful = jira.is_connected() && tempo.is_connected();
    let failure_exit_code = jira.exit_code.max(tempo.exit_code);
    report["remoteChecks"] = json!({
        "jira": jira.json(),
        "tempo": tempo.json(),
    });
    human.push_str(&format!(
        "\n\nRemote checks (read-only)\n{}\n{}",
        jira.human("Jira"),
        tempo.human("Tempo")
    ));

    if successful {
        Ok(Rendered::new(report, human))
    } else {
        Ok(Rendered::failed(
            report,
            human,
            "remote_check_failed",
            "one or more remote connection checks failed",
            failure_exit_code,
        ))
    }
}

fn configured_label(configured: bool) -> &'static str {
    if configured {
        "configured"
    } else {
        "not configured"
    }
}

fn configured_fields(
    config: &Config,
    environment: &dyn ConnectionEnvironment,
) -> serde_json::Value {
    json!({
        "tempoToken": config.tempo_token.is_some() || environment.is_set("TEMPO_TOKEN"),
        "accountId": config.account_id.is_some() || environment.is_set("TEMPO_ACCOUNT_ID"),
        "atlassianEmail": config.atlassian_user_email.is_some() || environment.is_set("ATLASSIAN_EMAIL"),
        "atlassianToken": config.atlassian_token.is_some() || environment.is_set("ATLASSIAN_TOKEN"),
        "atlassianHost": config.hostname.is_some() || environment.is_set("ATLASSIAN_HOST"),
    })
}
