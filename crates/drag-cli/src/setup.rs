//! Setup workflow, verification boundaries, and terminal session contract.

use std::future::Future;
#[cfg(test)]
use std::io::{self, IsTerminal, Write};
use std::pin::Pin;

use crate::api::ApiClient;
#[cfg(test)]
pub(crate) use crate::browser::NoopBrowserLauncher;
pub(crate) use crate::browser::{BrowserLauncher, SystemBrowserLauncher};
use crate::config::{
    normalize_jira_site, Config, Credentials, JiraCredentials, TempoCredentials,
    ATLASSIAN_EMAIL_ENV, ATLASSIAN_HOST_ENV, ATLASSIAN_TOKEN_ENV, TEMPO_TOKEN_ENV,
};
use crate::CliError;
#[cfg(test)]
pub(crate) use drag::setup::ATLASSIAN_TOKEN_URL;
use drag::setup::{CompletedSetup, OnboardingError, OnboardingState, SetupDefaults};
pub(crate) use drag::setup::{OnboardingScreen, SecretInput, SetupCredentials, TokenPage};

pub(crate) type VerificationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;
pub(crate) type OnboardingFuture<'a> =
    Pin<Box<dyn Future<Output = Result<OnboardingWorkflow<'a>, CliError>> + Send + 'a>>;

pub(crate) trait ConnectionVerifier: Send + Sync {
    fn verify_jira<'a>(
        &'a self,
        connection: &'a JiraCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, String>;

    fn verify_tempo<'a>(
        &'a self,
        connection: &'a TempoCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, ()>;
}

pub(crate) struct RemoteConnectionVerifier;

impl ConnectionVerifier for RemoteConnectionVerifier {
    fn verify_jira<'a>(
        &'a self,
        connection: &'a JiraCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async move {
            let api = ApiClient::new(connection.to_credentials(), debug)?;
            api.get_current_user_account_id().await
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        connection: &'a TempoCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            let api = ApiClient::new(connection.to_credentials(), debug)?;
            api.verify_tempo_connection().await
        })
    }
}

impl JiraCredentials {
    fn to_credentials(&self) -> Credentials {
        Credentials {
            tempo_token: String::new(),
            account_id: String::new(),
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
        }
    }
}

impl TempoCredentials {
    fn to_credentials(&self) -> Credentials {
        Credentials {
            tempo_token: self.tempo_token.clone(),
            account_id: self.account_id.clone(),
            atlassian_user_email: String::new(),
            atlassian_token: String::new(),
            hostname: String::new(),
        }
    }
}

#[cfg(test)]
pub(crate) trait SetupPrompter: Send + Sync {
    fn is_terminal(&self) -> bool;
    fn message(&self, message: &str) -> Result<(), CliError>;
    fn prompt_text(&self, label: &str, default: Option<&str>) -> Result<String, CliError>;
    fn prompt_secret(&self, label: &str, can_retain: bool) -> Result<Option<String>, CliError>;
}

#[cfg(test)]
pub(crate) struct TerminalSetupPrompter;

#[cfg(test)]
impl SetupPrompter for TerminalSetupPrompter {
    fn is_terminal(&self) -> bool {
        io::stdin().is_terminal()
    }

    fn message(&self, message: &str) -> Result<(), CliError> {
        writeln!(io::stderr().lock(), "{message}")?;
        Ok(())
    }

    fn prompt_text(&self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut stderr = io::stderr().lock();
        match default {
            Some(default) => write!(stderr, "{label} [{default}]: ")?,
            None => write!(stderr, "{label}: ")?,
        }
        stderr.flush()?;

        let mut input = String::new();
        let bytes_read = io::stdin()
            .read_line(&mut input)
            .map_err(map_setup_input_error)?;
        if bytes_read == 0 {
            return Err(setup_cancelled());
        }
        let value = input.trim();
        if value.is_empty() {
            Ok(default.unwrap_or_default().to_owned())
        } else {
            Ok(value.to_owned())
        }
    }

    fn prompt_secret(&self, label: &str, can_retain: bool) -> Result<Option<String>, CliError> {
        let prompt = if can_retain {
            format!(
                "{label} (pasted input will not be displayed; press Enter to keep the existing token): "
            )
        } else {
            format!("{label} (pasted input will not be displayed): ")
        };
        let config = rpassword::ConfigBuilder::new()
            .output_writer(io::stderr())
            .build();
        let value = rpassword::prompt_password_with_config(prompt, config)
            .map_err(map_setup_input_error)?;
        let value = value.trim();
        if value.is_empty() && can_retain {
            Ok(None)
        } else {
            Ok(Some(value.to_owned()))
        }
    }
}

/// A normalized unattended setup request shared by preview and execution.
pub(crate) struct EnvironmentSetupPlan {
    credentials: SetupCredentials,
}

impl EnvironmentSetupPlan {
    pub(crate) fn new(credentials: SetupCredentials) -> Self {
        Self { credentials }
    }

    pub(crate) fn credentials(&self) -> &SetupCredentials {
        &self.credentials
    }
}

pub(crate) trait SetupCredentialsExt {
    fn from_source(source: impl FnMut(&str) -> Result<String, CliError>) -> Result<Self, CliError>
    where
        Self: Sized;
    fn to_credentials(&self, account_id: String) -> Credentials;
    fn jira_connection(&self) -> JiraCredentials;
}

impl SetupCredentialsExt for SetupCredentials {
    fn from_source(
        mut source: impl FnMut(&str) -> Result<String, CliError>,
    ) -> Result<Self, CliError> {
        let hostname = normalize_jira_site(&source(ATLASSIAN_HOST_ENV)?)?;
        let atlassian_user_email = source(ATLASSIAN_EMAIL_ENV)?.trim().to_owned();
        let atlassian_token = source(ATLASSIAN_TOKEN_ENV)?.trim().to_owned();
        let tempo_token = source(TEMPO_TOKEN_ENV)?.trim().to_owned();
        Ok(Self {
            tempo_token,
            atlassian_user_email,
            atlassian_token,
            hostname,
        })
    }

    fn to_credentials(&self, account_id: String) -> Credentials {
        Credentials {
            tempo_token: self.tempo_token.clone(),
            account_id,
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
        }
    }

    fn jira_connection(&self) -> JiraCredentials {
        JiraCredentials {
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
        }
    }
}

impl From<CompletedSetup> for Credentials {
    fn from(completed: CompletedSetup) -> Self {
        Self {
            tempo_token: completed.tempo_token,
            account_id: completed.account_id,
            atlassian_user_email: completed.atlassian_user_email,
            atlassian_token: completed.atlassian_token,
            hostname: completed.hostname,
        }
    }
}

impl From<&Credentials> for TempoCredentials {
    fn from(credentials: &Credentials) -> Self {
        Self {
            tempo_token: credentials.tempo_token.clone(),
            account_id: credentials.account_id.clone(),
        }
    }
}

pub(crate) trait OnboardingSession: Send + Sync {
    fn is_terminal(&self) -> bool;
    fn run<'a>(&'a self, workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a>;
}

#[cfg(test)]
pub(crate) struct LineOnboardingSession {
    prompter: Box<dyn SetupPrompter>,
    browser_launcher: Box<dyn BrowserLauncher>,
}

#[cfg(test)]
impl LineOnboardingSession {
    pub(crate) fn with_dependencies(
        prompter: impl SetupPrompter + 'static,
        browser_launcher: impl BrowserLauncher + 'static,
    ) -> Self {
        Self {
            prompter: Box::new(prompter),
            browser_launcher: Box::new(browser_launcher),
        }
    }

    async fn run_workflow<'a>(
        &'a self,
        mut workflow: OnboardingWorkflow<'a>,
    ) -> Result<OnboardingWorkflow<'a>, CliError> {
        self.prompter.message(
            "Connect Jira, connect Tempo, then save. Nothing is saved until both connections succeed.",
        )?;
        self.prompter.message("\nConnect Jira")?;

        loop {
            if workflow.screen() != OnboardingScreen::JiraDetails {
                workflow.edit_jira();
            }
            let hostname = self.prompt_jira_site(workflow.hostname_default())?;
            let email = self.prompt_non_empty_text("Atlassian email", workflow.email_default())?;
            workflow.continue_from_jira_details()?;
            let token_page = workflow.jira_token_page()?;
            self.present_token_page(&token_page)?;
            let token =
                self.prompt_token("Atlassian API token", workflow.can_retain_jira_token())?;

            match workflow.connect_jira(hostname, email, token).await? {
                ConnectionOutcome::Connected => break,
                ConnectionOutcome::Rejected(error) => self.prompter.message(&format!(
                    "Could not connect to Jira: {error}\nCheck the Jira site, email, and token, then try again."
                ))?,
            }
        }

        self.prompter.message("\nConnect Tempo")?;
        loop {
            let token_page = workflow.tempo_token_page()?;
            self.present_token_page(&token_page)?;
            let token = self.prompt_token("Tempo API token", workflow.can_retain_tempo_token())?;

            match workflow.connect_tempo(token).await? {
                ConnectionOutcome::Connected => break,
                ConnectionOutcome::Rejected(error) => self.prompter.message(&format!(
                    "Could not connect to Tempo: {error}\nCheck the Tempo token, then try again."
                ))?,
            }
        }

        self.prompter.message("\nSave")?;
        Ok(workflow)
    }

    fn present_token_page(&self, page: &TokenPage) -> Result<(), CliError> {
        self.prompter
            .message(&format!("{}\n{}", page.instruction, page.url))?;
        if page.open_browser {
            if let Err(error) = self.browser_launcher.open(page.url.as_str()) {
                self.prompter.message(&format!(
                    "Warning: could not open this page in your browser: {error}. Continue with the URL above."
                ))?;
            }
        }
        Ok(())
    }

    fn prompt_jira_site(&self, default: Option<&str>) -> Result<String, CliError> {
        loop {
            let input = self
                .prompter
                .prompt_text("Jira site (hostname or HTTPS URL)", default)?;
            match normalize_jira_site(&input) {
                Ok(hostname) => return Ok(hostname),
                Err(error) => self.prompter.message(&format!(
                    "Invalid Jira site: {error}\nPaste a bare hostname or any HTTPS URL from your Jira site."
                ))?,
            }
        }
    }

    fn prompt_non_empty_text(
        &self,
        label: &str,
        default: Option<&str>,
    ) -> Result<String, CliError> {
        loop {
            let value = self.prompter.prompt_text(label, default)?;
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_owned());
            }
            self.prompter
                .message(&format!("{label} must not be empty; try again."))?;
        }
    }

    fn prompt_token(&self, label: &str, can_retain: bool) -> Result<SecretInput, CliError> {
        loop {
            match self.prompter.prompt_secret(label, can_retain)? {
                Some(value) if !value.trim().is_empty() => {
                    return Ok(SecretInput::Replace(value.trim().to_owned()));
                }
                None => return Ok(SecretInput::Retain),
                Some(_) => self
                    .prompter
                    .message(&format!("{label} must not be empty; try again."))?,
            }
        }
    }
}

#[cfg(test)]
impl OnboardingSession for LineOnboardingSession {
    fn is_terminal(&self) -> bool {
        self.prompter.is_terminal()
    }

    fn run<'a>(&'a self, workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
        Box::pin(async move { self.run_workflow(workflow).await })
    }
}

pub(crate) enum ConnectionOutcome {
    Connected,
    Rejected(CliError),
}

pub(crate) struct OnboardingWorkflow<'a> {
    verifier: &'a dyn ConnectionVerifier,
    debug: bool,
    state: OnboardingState,
}

impl<'a> OnboardingWorkflow<'a> {
    pub(crate) fn new(
        existing: &Config,
        verifier: &'a dyn ConnectionVerifier,
        debug: bool,
        open_browser: bool,
    ) -> Self {
        let defaults = SetupDefaults {
            hostname: existing.hostname.clone(),
            atlassian_user_email: existing.atlassian_user_email.clone(),
            atlassian_token: existing.atlassian_token.clone(),
            tempo_token: existing.tempo_token.clone(),
        };
        Self {
            verifier,
            debug,
            state: OnboardingState::new(defaults, open_browser),
        }
    }

    pub(crate) fn hostname_default(&self) -> Option<&str> {
        self.state.hostname_default()
    }

    pub(crate) fn email_default(&self) -> Option<&str> {
        self.state.email_default()
    }

    pub(crate) fn can_retain_jira_token(&self) -> bool {
        self.state.can_retain_jira_token()
    }

    pub(crate) fn can_retain_tempo_token(&self) -> bool {
        self.state.can_retain_tempo_token()
    }

    pub(crate) fn screen(&self) -> OnboardingScreen {
        self.state.screen()
    }

    pub(crate) fn continue_from_jira_details(&mut self) -> Result<OnboardingScreen, CliError> {
        self.state.continue_from_jira_details().map_err(Into::into)
    }

    pub(crate) fn continue_with_verified_jira(&mut self) -> Result<OnboardingScreen, CliError> {
        self.state.continue_with_verified_jira().map_err(Into::into)
    }

    pub(crate) fn continue_with_verified_tempo(&mut self) -> Result<OnboardingScreen, CliError> {
        self.state
            .continue_with_verified_tempo()
            .map_err(Into::into)
    }

    pub(crate) fn back(&mut self) -> Result<Option<OnboardingScreen>, CliError> {
        self.state.back().map_err(Into::into)
    }

    pub(crate) fn cancel(&self) -> CliError {
        setup_cancelled()
    }

    pub(crate) fn edit_jira(&mut self) -> OnboardingScreen {
        self.state.edit_jira()
    }

    pub(crate) fn edit_tempo(&mut self) -> Result<OnboardingScreen, CliError> {
        self.state.edit_tempo().map_err(Into::into)
    }

    pub(crate) fn jira_token_page(&mut self) -> Result<TokenPage, CliError> {
        self.state.jira_token_page().map_err(Into::into)
    }

    pub(crate) fn tempo_token_page(&mut self) -> Result<TokenPage, CliError> {
        self.state.tempo_token_page().map_err(Into::into)
    }

    pub(crate) async fn connect_jira(
        &mut self,
        hostname: String,
        email: String,
        token: SecretInput,
    ) -> Result<ConnectionOutcome, CliError> {
        let hostname = normalize_jira_site(&hostname)?;
        let setup_credentials = self
            .state
            .prepare_jira_connection(hostname, email, token)
            .map_err(CliError::from)?;

        match self
            .verifier
            .verify_jira(&setup_credentials.jira_connection(), self.debug)
            .await
        {
            Ok(account_id) => {
                self.state
                    .accept_verified_jira(setup_credentials, account_id)
                    .map_err(CliError::from)?;
                Ok(ConnectionOutcome::Connected)
            }
            Err(error) if error.is_authentication() => Ok(ConnectionOutcome::Rejected(error)),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn connect_tempo(
        &mut self,
        token: SecretInput,
    ) -> Result<ConnectionOutcome, CliError> {
        let attempt = self
            .state
            .prepare_tempo_connection(token)
            .map_err(CliError::from)?;
        let tempo_credentials = TempoCredentials {
            tempo_token: attempt.tempo_token,
            account_id: attempt.account_id,
        };

        match self
            .verifier
            .verify_tempo(&tempo_credentials, self.debug)
            .await
        {
            Ok(()) => {
                self.state.accept_verified_tempo().map_err(CliError::from)?;
                Ok(ConnectionOutcome::Connected)
            }
            Err(error) if error.is_authentication() => Ok(ConnectionOutcome::Rejected(error)),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn invalidate_jira(&mut self) {
        self.state.invalidate_jira();
    }

    pub(crate) fn invalidate_tempo(&mut self) -> Result<(), CliError> {
        self.state.invalidate_tempo().map_err(Into::into)
    }

    pub(crate) fn finish(self) -> Result<Credentials, CliError> {
        self.state
            .finish()
            .map(Credentials::from)
            .map_err(Into::into)
    }
}

impl From<OnboardingError> for CliError {
    fn from(error: OnboardingError) -> Self {
        match error {
            OnboardingError::InvalidState => invalid_onboarding_state(),
            OnboardingError::TokenRequired => {
                CliError::InvalidInput("token is required".to_owned())
            }
            OnboardingError::AtlassianEmailRequired => {
                CliError::InvalidInput("Atlassian email must not be empty".to_owned())
            }
        }
    }
}

fn invalid_onboarding_state() -> CliError {
    CliError::InvalidInput("invalid onboarding workflow state".to_owned())
}

pub(crate) fn setup_cancelled() -> CliError {
    CliError::InvalidInput(
        "interactive setup was cancelled; configuration was not changed".to_owned(),
    )
}

#[cfg(test)]
fn map_setup_input_error(error: io::Error) -> CliError {
    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::UnexpectedEof
    ) {
        setup_cancelled()
    } else {
        CliError::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionOutcome, ConnectionVerifier, OnboardingScreen, OnboardingWorkflow, SecretInput,
        VerificationFuture,
    };
    use crate::config::{Config, JiraCredentials, TempoCredentials};
    use crate::CliError;

    struct AcceptingVerifier;

    impl ConnectionVerifier for AcceptingVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            Box::pin(async { Ok("account-1".to_owned()) })
        }

        fn verify_tempo<'a>(
            &'a self,
            _connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn workflow_owns_forward_back_and_cancel_navigation() -> Result<(), CliError> {
        let verifier = AcceptingVerifier;
        let mut workflow = OnboardingWorkflow::new(&Config::default(), &verifier, false, false);

        assert_eq!(workflow.screen(), OnboardingScreen::JiraDetails);
        assert_eq!(workflow.back()?, None);
        assert_eq!(
            workflow.continue_from_jira_details()?,
            OnboardingScreen::JiraToken
        );
        assert_eq!(workflow.back()?, Some(OnboardingScreen::JiraDetails));

        Ok(())
    }

    #[tokio::test]
    async fn workflow_owns_verification_backtracking_and_invalidation() -> Result<(), CliError> {
        let verifier = AcceptingVerifier;
        let mut workflow = OnboardingWorkflow::new(&Config::default(), &verifier, false, false);
        workflow.continue_from_jira_details()?;

        assert!(matches!(
            workflow
                .connect_jira(
                    "example.atlassian.net".to_owned(),
                    "person@example.com".to_owned(),
                    SecretInput::Replace("jira-token".to_owned()),
                )
                .await?,
            ConnectionOutcome::Connected
        ));
        assert_eq!(workflow.screen(), OnboardingScreen::Tempo);
        assert!(matches!(
            workflow
                .connect_tempo(SecretInput::Replace("tempo-token".to_owned()))
                .await?,
            ConnectionOutcome::Connected
        ));
        assert_eq!(workflow.screen(), OnboardingScreen::Save);

        assert_eq!(workflow.back()?, Some(OnboardingScreen::Tempo));
        assert_eq!(
            workflow.continue_with_verified_tempo()?,
            OnboardingScreen::Save
        );
        workflow.edit_tempo()?;
        assert_eq!(workflow.screen(), OnboardingScreen::Tempo);
        assert!(workflow.finish().is_err());

        Ok(())
    }

    #[tokio::test]
    async fn reconnecting_jira_invalidates_both_verified_connections() -> Result<(), CliError> {
        let verifier = AcceptingVerifier;
        let mut workflow = OnboardingWorkflow::new(&Config::default(), &verifier, false, false);
        workflow.continue_from_jira_details()?;
        let _ = workflow
            .connect_jira(
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
                SecretInput::Replace("jira-token".to_owned()),
            )
            .await?;
        let _ = workflow
            .connect_tempo(SecretInput::Replace("tempo-token".to_owned()))
            .await?;

        assert_eq!(workflow.edit_jira(), OnboardingScreen::JiraDetails);
        workflow.continue_from_jira_details()?;
        workflow.invalidate_jira();
        assert!(workflow.finish().is_err());

        Ok(())
    }

    #[tokio::test]
    async fn navigation_and_review_editing_preserve_verification_until_submission(
    ) -> Result<(), CliError> {
        let verifier = AcceptingVerifier;
        let mut workflow = OnboardingWorkflow::new(&Config::default(), &verifier, false, false);
        workflow.continue_from_jira_details()?;
        let _ = workflow
            .connect_jira(
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
                SecretInput::Replace("jira-token".to_owned()),
            )
            .await?;
        let _ = workflow
            .connect_tempo(SecretInput::Replace("tempo-token".to_owned()))
            .await?;

        assert_eq!(workflow.back()?, Some(OnboardingScreen::Tempo));
        assert_eq!(workflow.back()?, Some(OnboardingScreen::JiraToken));
        assert_eq!(workflow.back()?, Some(OnboardingScreen::JiraDetails));
        workflow.continue_from_jira_details()?;
        workflow.continue_with_verified_jira()?;
        workflow.continue_with_verified_tempo()?;

        workflow.edit_jira();
        workflow.continue_from_jira_details()?;
        workflow.continue_with_verified_jira()?;
        workflow.continue_with_verified_tempo()?;
        workflow.edit_tempo()?;
        workflow.continue_with_verified_tempo()?;

        assert!(workflow.finish().is_ok());
        Ok(())
    }
}
