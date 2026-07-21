//! Setup workflow, verification boundaries, and terminal session contract.

use std::future::Future;
#[cfg(test)]
use std::io::{self, IsTerminal, Write};
use std::pin::Pin;
use url::Url;

use crate::api::ApiClient;
#[cfg(test)]
pub(crate) use crate::browser::NoopBrowserLauncher;
pub(crate) use crate::browser::{BrowserLauncher, SystemBrowserLauncher};
use crate::config::{
    normalize_jira_site, Config, Credentials, JiraCredentials, TempoCredentials,
    ATLASSIAN_EMAIL_ENV, ATLASSIAN_HOST_ENV, ATLASSIAN_TOKEN_ENV, TEMPO_TOKEN_ENV,
};
use crate::CliError;

pub(crate) const ATLASSIAN_TOKEN_URL: &str =
    "https://id.atlassian.com/manage-profile/security/api-tokens";
const TEMPO_TOKEN_PATH: &str =
    "/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration";

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

pub(crate) struct SetupCredentials {
    pub(crate) tempo_token: String,
    pub(crate) atlassian_user_email: String,
    pub(crate) atlassian_token: String,
    pub(crate) hostname: String,
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

impl SetupCredentials {
    pub(crate) fn from_source(
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

    pub(crate) fn to_credentials(&self, account_id: String) -> Credentials {
        Credentials {
            tempo_token: self.tempo_token.clone(),
            account_id,
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
        }
    }

    pub(crate) fn jira_connection(&self) -> JiraCredentials {
        JiraCredentials {
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum OnboardingStage {
    Jira,
    Tempo,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnboardingScreen {
    JiraDetails,
    JiraToken,
    Tempo,
    Save,
}

pub(crate) enum SecretInput {
    Replace(String),
    Retain,
}

pub(crate) enum ConnectionOutcome {
    Connected,
    Rejected(CliError),
}

pub(crate) struct TokenPage {
    pub(crate) instruction: &'static str,
    pub(crate) url: Url,
    pub(crate) open_browser: bool,
}

pub(crate) struct OnboardingWorkflow<'a> {
    verifier: &'a dyn ConnectionVerifier,
    debug: bool,
    open_browser: bool,
    stage: OnboardingStage,
    screen: OnboardingScreen,
    hostname_default: Option<String>,
    email_default: Option<String>,
    jira_token: Option<String>,
    tempo_token: Option<String>,
    jira_page_presented: bool,
    tempo_page_presented: bool,
    setup_credentials: Option<SetupCredentials>,
    account_id: Option<String>,
}

impl<'a> OnboardingWorkflow<'a> {
    pub(crate) fn new(
        existing: &Config,
        verifier: &'a dyn ConnectionVerifier,
        debug: bool,
        open_browser: bool,
    ) -> Self {
        Self {
            verifier,
            debug,
            open_browser,
            stage: OnboardingStage::Jira,
            screen: OnboardingScreen::JiraDetails,
            hostname_default: existing.hostname.clone(),
            email_default: existing.atlassian_user_email.clone(),
            jira_token: existing
                .atlassian_token
                .clone()
                .filter(|value| !value.is_empty()),
            tempo_token: existing
                .tempo_token
                .clone()
                .filter(|value| !value.is_empty()),
            jira_page_presented: false,
            tempo_page_presented: false,
            setup_credentials: None,
            account_id: None,
        }
    }

    pub(crate) fn hostname_default(&self) -> Option<&str> {
        self.hostname_default.as_deref()
    }

    pub(crate) fn email_default(&self) -> Option<&str> {
        self.email_default.as_deref()
    }

    pub(crate) fn can_retain_jira_token(&self) -> bool {
        self.jira_token.is_some()
    }

    pub(crate) fn can_retain_tempo_token(&self) -> bool {
        self.tempo_token.is_some()
    }

    pub(crate) const fn screen(&self) -> OnboardingScreen {
        self.screen
    }

    pub(crate) fn continue_from_jira_details(&mut self) -> Result<OnboardingScreen, CliError> {
        self.require_screen(OnboardingScreen::JiraDetails)?;
        self.screen = OnboardingScreen::JiraToken;
        Ok(self.screen)
    }

    pub(crate) fn continue_with_verified_jira(&mut self) -> Result<OnboardingScreen, CliError> {
        if !matches!(
            self.stage,
            OnboardingStage::Tempo | OnboardingStage::Complete
        ) {
            return Err(invalid_onboarding_state());
        }
        self.require_screen(OnboardingScreen::JiraToken)?;
        self.screen = OnboardingScreen::Tempo;
        Ok(self.screen)
    }

    pub(crate) fn continue_with_verified_tempo(&mut self) -> Result<OnboardingScreen, CliError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        self.screen = OnboardingScreen::Save;
        Ok(self.screen)
    }

    pub(crate) fn back(&mut self) -> Result<Option<OnboardingScreen>, CliError> {
        self.screen = match self.screen {
            OnboardingScreen::JiraDetails => return Ok(None),
            OnboardingScreen::JiraToken => OnboardingScreen::JiraDetails,
            OnboardingScreen::Tempo => OnboardingScreen::JiraToken,
            OnboardingScreen::Save => OnboardingScreen::Tempo,
        };
        Ok(Some(self.screen))
    }

    pub(crate) fn cancel(&self) -> CliError {
        setup_cancelled()
    }

    pub(crate) fn edit_jira(&mut self) -> OnboardingScreen {
        self.screen = OnboardingScreen::JiraDetails;
        self.screen
    }

    pub(crate) fn edit_tempo(&mut self) -> Result<OnboardingScreen, CliError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.screen = OnboardingScreen::Tempo;
        Ok(self.screen)
    }

    pub(crate) fn jira_token_page(&mut self) -> Result<TokenPage, CliError> {
        self.require_stage(OnboardingStage::Jira)?;
        self.require_screen(OnboardingScreen::JiraToken)?;
        let page = TokenPage {
            instruction: "Create or manage your Atlassian API token:",
            url: Url::parse(ATLASSIAN_TOKEN_URL)?,
            open_browser: self.open_browser && !self.jira_page_presented,
        };
        self.jira_page_presented = true;
        Ok(page)
    }

    pub(crate) fn tempo_token_page(&mut self) -> Result<TokenPage, CliError> {
        self.require_stage(OnboardingStage::Tempo)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        let hostname = self
            .setup_credentials
            .as_ref()
            .map(|credentials| credentials.hostname.as_str())
            .ok_or_else(invalid_onboarding_state)?;
        let page = TokenPage {
            instruction: "Create or manage your Tempo API token:",
            url: Url::parse(&format!("https://{hostname}{TEMPO_TOKEN_PATH}"))?,
            open_browser: self.open_browser && !self.tempo_page_presented,
        };
        self.tempo_page_presented = true;
        Ok(page)
    }

    pub(crate) async fn connect_jira(
        &mut self,
        hostname: String,
        email: String,
        token: SecretInput,
    ) -> Result<ConnectionOutcome, CliError> {
        self.require_stage(OnboardingStage::Jira)?;
        self.require_screen(OnboardingScreen::JiraToken)?;
        let hostname = normalize_jira_site(&hostname)?;
        let email = email.trim();
        if email.is_empty() {
            return Err(CliError::InvalidInput(
                "Atlassian email must not be empty".to_owned(),
            ));
        }
        let email = email.to_owned();
        self.hostname_default = Some(hostname.clone());
        self.email_default = Some(email.clone());
        let atlassian_token = resolve_secret(token, self.jira_token.as_deref())?;
        let setup_credentials = SetupCredentials {
            tempo_token: String::new(),
            atlassian_user_email: email,
            atlassian_token,
            hostname,
        };

        match self
            .verifier
            .verify_jira(&setup_credentials.jira_connection(), self.debug)
            .await
        {
            Ok(account_id) => {
                self.jira_token = Some(setup_credentials.atlassian_token.clone());
                self.setup_credentials = Some(setup_credentials);
                self.account_id = Some(account_id);
                self.stage = OnboardingStage::Tempo;
                self.screen = OnboardingScreen::Tempo;
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
        self.require_stage(OnboardingStage::Tempo)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        let tempo_token = resolve_secret(token, self.tempo_token.as_deref())?;
        let setup_credentials = self
            .setup_credentials
            .as_mut()
            .ok_or_else(invalid_onboarding_state)?;
        setup_credentials.tempo_token = tempo_token;
        let credentials = setup_credentials.to_credentials(
            self.account_id
                .as_ref()
                .ok_or_else(invalid_onboarding_state)?
                .clone(),
        );

        match self
            .verifier
            .verify_tempo(&TempoCredentials::from(&credentials), self.debug)
            .await
        {
            Ok(()) => {
                self.tempo_token = Some(setup_credentials.tempo_token.clone());
                self.stage = OnboardingStage::Complete;
                self.screen = OnboardingScreen::Save;
                Ok(ConnectionOutcome::Connected)
            }
            Err(error) if error.is_authentication() => Ok(ConnectionOutcome::Rejected(error)),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn invalidate_jira(&mut self) {
        self.stage = OnboardingStage::Jira;
        self.screen = OnboardingScreen::JiraToken;
        self.setup_credentials = None;
        self.account_id = None;
    }

    pub(crate) fn invalidate_tempo(&mut self) -> Result<(), CliError> {
        if self.stage == OnboardingStage::Jira {
            return Err(invalid_onboarding_state());
        }
        self.stage = OnboardingStage::Tempo;
        self.screen = OnboardingScreen::Tempo;
        if let Some(setup_credentials) = &mut self.setup_credentials {
            setup_credentials.tempo_token.clear();
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<Credentials, CliError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.require_screen(OnboardingScreen::Save)?;
        Ok(self
            .setup_credentials
            .ok_or_else(invalid_onboarding_state)?
            .to_credentials(self.account_id.ok_or_else(invalid_onboarding_state)?))
    }

    fn require_stage(&self, expected: OnboardingStage) -> Result<(), CliError> {
        if self.stage == expected {
            Ok(())
        } else {
            Err(invalid_onboarding_state())
        }
    }

    fn require_screen(&self, expected: OnboardingScreen) -> Result<(), CliError> {
        if self.screen == expected {
            Ok(())
        } else {
            Err(invalid_onboarding_state())
        }
    }
}

fn resolve_secret(input: SecretInput, existing: Option<&str>) -> Result<String, CliError> {
    match input {
        SecretInput::Replace(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        SecretInput::Retain => existing
            .map(str::to_owned)
            .ok_or_else(|| CliError::InvalidInput("token is required".to_owned())),
        SecretInput::Replace(_) => Err(CliError::InvalidInput("token is required".to_owned())),
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
