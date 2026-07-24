//! Deterministic setup and onboarding state transitions.
//!
//! This module is intentionally I/O-independent. It owns the finite-state
//! workflow and pure value transformations used by setup frontends, while the
//! CLI remains responsible for prompting, rendering, filesystem access, HTTP,
//! and remote verification futures.

use thiserror::Error;

/// URL where users create or manage Atlassian API tokens.
pub const ATLASSIAN_TOKEN_URL: &str = "https://id.atlassian.com/manage-profile/security/api-tokens";
const TEMPO_TOKEN_PATH: &str =
    "/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration";

/// Values loaded from an existing setup before onboarding starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupDefaults {
    /// Existing Jira hostname, if configured.
    pub hostname: Option<String>,
    /// Existing Atlassian account email, if configured.
    pub atlassian_user_email: Option<String>,
    /// Existing Atlassian API token, if configured.
    pub atlassian_token: Option<String>,
    /// Existing Tempo API token, if configured.
    pub tempo_token: Option<String>,
}

/// Credentials collected during setup before the Jira account id is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupCredentials {
    /// Tempo API token.
    pub tempo_token: String,
    /// Atlassian account email.
    pub atlassian_user_email: String,
    /// Atlassian API token.
    pub atlassian_token: String,
    /// Jira hostname.
    pub hostname: String,
}

/// Credentials that have passed both Jira and Tempo verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedSetup {
    /// Tempo API token.
    pub tempo_token: String,
    /// Jira account id returned by remote verification.
    pub account_id: String,
    /// Atlassian account email.
    pub atlassian_user_email: String,
    /// Atlassian API token.
    pub atlassian_token: String,
    /// Jira hostname.
    pub hostname: String,
}

/// Tempo verification inputs prepared by the workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoConnectionAttempt {
    /// Tempo API token to verify.
    pub tempo_token: String,
    /// Jira account id established by Jira verification.
    pub account_id: String,
}

/// The visible onboarding screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardingScreen {
    /// Jira hostname and email entry.
    JiraDetails,
    /// Atlassian API token entry.
    JiraToken,
    /// Tempo token entry.
    Tempo,
    /// Review and save.
    Save,
}

/// Secret field submission behavior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretInput {
    /// Replace any retained secret with this value.
    Replace(String),
    /// Reuse the retained secret.
    Retain,
}

/// Token-management page presentation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenPage {
    /// User-facing instruction text.
    pub instruction: &'static str,
    /// Page URL.
    pub url: String,
    /// Whether this transition should attempt to open a browser.
    pub open_browser: bool,
}

/// Pure onboarding state-machine errors.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum OnboardingError {
    /// A transition was requested from an invalid stage or screen.
    #[error("invalid onboarding workflow state")]
    InvalidState,
    /// A required token was not supplied and no retained token exists.
    #[error("token is required")]
    TokenRequired,
    /// The Atlassian email field was empty.
    #[error("Atlassian email must not be empty")]
    AtlassianEmailRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnboardingStage {
    Jira,
    Tempo,
    Complete,
}

/// I/O-independent setup finite-state workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnboardingState {
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

impl OnboardingState {
    /// Create a workflow from existing setup defaults.
    #[must_use]
    pub fn new(defaults: SetupDefaults, open_browser: bool) -> Self {
        Self {
            open_browser,
            stage: OnboardingStage::Jira,
            screen: OnboardingScreen::JiraDetails,
            hostname_default: defaults.hostname,
            email_default: defaults.atlassian_user_email,
            jira_token: defaults.atlassian_token.filter(|value| !value.is_empty()),
            tempo_token: defaults.tempo_token.filter(|value| !value.is_empty()),
            jira_page_presented: false,
            tempo_page_presented: false,
            setup_credentials: None,
            account_id: None,
        }
    }

    /// Current Jira hostname default.
    #[must_use]
    pub fn hostname_default(&self) -> Option<&str> {
        self.hostname_default.as_deref()
    }

    /// Current Atlassian email default.
    #[must_use]
    pub fn email_default(&self) -> Option<&str> {
        self.email_default.as_deref()
    }

    /// Whether a retained Jira token can be reused.
    #[must_use]
    pub fn can_retain_jira_token(&self) -> bool {
        self.jira_token.is_some()
    }

    /// Whether a retained Tempo token can be reused.
    #[must_use]
    pub fn can_retain_tempo_token(&self) -> bool {
        self.tempo_token.is_some()
    }

    /// Current visible screen.
    #[must_use]
    pub const fn screen(&self) -> OnboardingScreen {
        self.screen
    }

    /// Continue from Jira details to token entry.
    pub fn continue_from_jira_details(&mut self) -> Result<OnboardingScreen, OnboardingError> {
        self.require_screen(OnboardingScreen::JiraDetails)?;
        self.screen = OnboardingScreen::JiraToken;
        Ok(self.screen)
    }

    /// Continue to Tempo when Jira has already been verified.
    pub fn continue_with_verified_jira(&mut self) -> Result<OnboardingScreen, OnboardingError> {
        if !matches!(
            self.stage,
            OnboardingStage::Tempo | OnboardingStage::Complete
        ) {
            return Err(OnboardingError::InvalidState);
        }
        self.require_screen(OnboardingScreen::JiraToken)?;
        self.screen = OnboardingScreen::Tempo;
        Ok(self.screen)
    }

    /// Continue to Save when Tempo has already been verified.
    pub fn continue_with_verified_tempo(&mut self) -> Result<OnboardingScreen, OnboardingError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        self.screen = OnboardingScreen::Save;
        Ok(self.screen)
    }

    /// Navigate backward by one screen.
    pub fn back(&mut self) -> Result<Option<OnboardingScreen>, OnboardingError> {
        self.screen = match self.screen {
            OnboardingScreen::JiraDetails => return Ok(None),
            OnboardingScreen::JiraToken => OnboardingScreen::JiraDetails,
            OnboardingScreen::Tempo => OnboardingScreen::JiraToken,
            OnboardingScreen::Save => OnboardingScreen::Tempo,
        };
        Ok(Some(self.screen))
    }

    /// Edit Jira fields from review.
    pub fn edit_jira(&mut self) -> OnboardingScreen {
        self.screen = OnboardingScreen::JiraDetails;
        self.screen
    }

    /// Edit Tempo token from review.
    pub fn edit_tempo(&mut self) -> Result<OnboardingScreen, OnboardingError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.screen = OnboardingScreen::Tempo;
        Ok(self.screen)
    }

    /// Return the Atlassian token-management page for the current transition.
    pub fn jira_token_page(&mut self) -> Result<TokenPage, OnboardingError> {
        self.require_stage(OnboardingStage::Jira)?;
        self.require_screen(OnboardingScreen::JiraToken)?;
        let page = TokenPage {
            instruction: "Create or manage your Atlassian API token:",
            url: ATLASSIAN_TOKEN_URL.to_owned(),
            open_browser: self.open_browser && !self.jira_page_presented,
        };
        self.jira_page_presented = true;
        Ok(page)
    }

    /// Return the Tempo token-management page for the current transition.
    pub fn tempo_token_page(&mut self) -> Result<TokenPage, OnboardingError> {
        self.require_stage(OnboardingStage::Tempo)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        let hostname = self
            .setup_credentials
            .as_ref()
            .map(|credentials| credentials.hostname.as_str())
            .ok_or(OnboardingError::InvalidState)?;
        let page = TokenPage {
            instruction: "Create or manage your Tempo API token:",
            url: format!("https://{hostname}{TEMPO_TOKEN_PATH}"),
            open_browser: self.open_browser && !self.tempo_page_presented,
        };
        self.tempo_page_presented = true;
        Ok(page)
    }

    /// Prepare Jira verification inputs and update editable defaults.
    pub fn prepare_jira_connection(
        &mut self,
        hostname: String,
        email: String,
        token: SecretInput,
    ) -> Result<SetupCredentials, OnboardingError> {
        self.require_stage(OnboardingStage::Jira)?;
        self.require_screen(OnboardingScreen::JiraToken)?;
        let email = email.trim();
        if email.is_empty() {
            return Err(OnboardingError::AtlassianEmailRequired);
        }
        let email = email.to_owned();
        self.hostname_default = Some(hostname.clone());
        self.email_default = Some(email.clone());
        let atlassian_token = resolve_secret(token, self.jira_token.as_deref())?;
        Ok(SetupCredentials {
            tempo_token: String::new(),
            atlassian_user_email: email,
            atlassian_token,
            hostname,
        })
    }

    /// Accept a successful Jira verification and advance to Tempo.
    pub fn accept_verified_jira(
        &mut self,
        setup_credentials: SetupCredentials,
        account_id: String,
    ) -> Result<OnboardingScreen, OnboardingError> {
        self.require_stage(OnboardingStage::Jira)?;
        self.require_screen(OnboardingScreen::JiraToken)?;
        self.jira_token = Some(setup_credentials.atlassian_token.clone());
        self.setup_credentials = Some(setup_credentials);
        self.account_id = Some(account_id);
        self.stage = OnboardingStage::Tempo;
        self.screen = OnboardingScreen::Tempo;
        Ok(self.screen)
    }

    /// Prepare Tempo verification inputs.
    pub fn prepare_tempo_connection(
        &mut self,
        token: SecretInput,
    ) -> Result<TempoConnectionAttempt, OnboardingError> {
        self.require_stage(OnboardingStage::Tempo)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        let tempo_token = resolve_secret(token, self.tempo_token.as_deref())?;
        let setup_credentials = self
            .setup_credentials
            .as_mut()
            .ok_or(OnboardingError::InvalidState)?;
        setup_credentials.tempo_token = tempo_token.clone();
        let account_id = self
            .account_id
            .as_ref()
            .ok_or(OnboardingError::InvalidState)?
            .clone();
        Ok(TempoConnectionAttempt {
            tempo_token,
            account_id,
        })
    }

    /// Accept a successful Tempo verification and advance to Save.
    pub fn accept_verified_tempo(&mut self) -> Result<OnboardingScreen, OnboardingError> {
        self.require_stage(OnboardingStage::Tempo)?;
        self.require_screen(OnboardingScreen::Tempo)?;
        let setup_credentials = self
            .setup_credentials
            .as_ref()
            .ok_or(OnboardingError::InvalidState)?;
        self.tempo_token = Some(setup_credentials.tempo_token.clone());
        self.stage = OnboardingStage::Complete;
        self.screen = OnboardingScreen::Save;
        Ok(self.screen)
    }

    /// Invalidate Jira verification after editing Jira fields or token.
    pub fn invalidate_jira(&mut self) {
        self.stage = OnboardingStage::Jira;
        self.screen = OnboardingScreen::JiraToken;
        self.setup_credentials = None;
        self.account_id = None;
    }

    /// Invalidate Tempo verification after editing Tempo token.
    pub fn invalidate_tempo(&mut self) -> Result<(), OnboardingError> {
        if self.stage == OnboardingStage::Jira {
            return Err(OnboardingError::InvalidState);
        }
        self.stage = OnboardingStage::Tempo;
        self.screen = OnboardingScreen::Tempo;
        if let Some(setup_credentials) = &mut self.setup_credentials {
            setup_credentials.tempo_token.clear();
        }
        Ok(())
    }

    /// Finish a completely verified setup.
    pub fn finish(self) -> Result<CompletedSetup, OnboardingError> {
        self.require_stage(OnboardingStage::Complete)?;
        self.require_screen(OnboardingScreen::Save)?;
        let setup_credentials = self
            .setup_credentials
            .ok_or(OnboardingError::InvalidState)?;
        Ok(setup_credentials.complete(self.account_id.ok_or(OnboardingError::InvalidState)?))
    }

    fn require_stage(&self, expected: OnboardingStage) -> Result<(), OnboardingError> {
        if self.stage == expected {
            Ok(())
        } else {
            Err(OnboardingError::InvalidState)
        }
    }

    fn require_screen(&self, expected: OnboardingScreen) -> Result<(), OnboardingError> {
        if self.screen == expected {
            Ok(())
        } else {
            Err(OnboardingError::InvalidState)
        }
    }
}

impl SetupCredentials {
    /// Add the verified Jira account id to complete setup credentials.
    #[must_use]
    pub fn complete(self, account_id: String) -> CompletedSetup {
        CompletedSetup {
            tempo_token: self.tempo_token,
            account_id,
            atlassian_user_email: self.atlassian_user_email,
            atlassian_token: self.atlassian_token,
            hostname: self.hostname,
        }
    }
}

fn resolve_secret(input: SecretInput, existing: Option<&str>) -> Result<String, OnboardingError> {
    match input {
        SecretInput::Replace(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        SecretInput::Retain => existing
            .map(str::to_owned)
            .ok_or(OnboardingError::TokenRequired),
        SecretInput::Replace(_) => Err(OnboardingError::TokenRequired),
    }
}

#[cfg(test)]
mod tests {
    use super::{OnboardingError, OnboardingScreen, OnboardingState, SecretInput, SetupDefaults};

    fn verified_jira_state() -> Result<OnboardingState, OnboardingError> {
        let mut state = OnboardingState::new(SetupDefaults::default(), true);
        state.continue_from_jira_details()?;
        let credentials = state.prepare_jira_connection(
            "example.atlassian.net".to_owned(),
            " person@example.com ".to_owned(),
            SecretInput::Replace(" jira-token ".to_owned()),
        )?;
        state.accept_verified_jira(credentials, "account-1".to_owned())?;
        Ok(state)
    }

    #[test]
    fn workflow_starts_with_defaults_and_retained_secret_flags() {
        let state = OnboardingState::new(
            SetupDefaults {
                hostname: Some("example.atlassian.net".to_owned()),
                atlassian_user_email: Some("person@example.com".to_owned()),
                atlassian_token: Some("jira-token".to_owned()),
                tempo_token: Some(String::new()),
            },
            false,
        );

        assert_eq!(state.screen(), OnboardingScreen::JiraDetails);
        assert_eq!(state.hostname_default(), Some("example.atlassian.net"));
        assert_eq!(state.email_default(), Some("person@example.com"));
        assert!(state.can_retain_jira_token());
        assert!(!state.can_retain_tempo_token());
    }

    #[test]
    fn navigation_moves_forward_and_back_in_screen_order() -> Result<(), OnboardingError> {
        let mut state = OnboardingState::new(SetupDefaults::default(), false);

        assert_eq!(state.back()?, None);
        assert_eq!(
            state.continue_from_jira_details()?,
            OnboardingScreen::JiraToken
        );
        assert_eq!(state.back()?, Some(OnboardingScreen::JiraDetails));

        Ok(())
    }

    #[test]
    fn token_pages_open_browser_only_once_per_page() -> Result<(), OnboardingError> {
        let mut state = verified_jira_state()?;
        let first_tempo_page = state.tempo_token_page()?;
        let second_tempo_page = state.tempo_token_page()?;

        assert!(first_tempo_page.open_browser);
        assert!(!second_tempo_page.open_browser);
        assert_eq!(
            first_tempo_page.url,
            "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
        );

        Ok(())
    }

    #[test]
    fn jira_verification_updates_defaults_but_only_accept_advances_stage(
    ) -> Result<(), OnboardingError> {
        let mut state = OnboardingState::new(SetupDefaults::default(), false);
        state.continue_from_jira_details()?;

        let credentials = state.prepare_jira_connection(
            "example.atlassian.net".to_owned(),
            " person@example.com ".to_owned(),
            SecretInput::Replace(" jira-token ".to_owned()),
        )?;

        assert_eq!(state.hostname_default(), Some("example.atlassian.net"));
        assert_eq!(state.email_default(), Some("person@example.com"));
        assert_eq!(state.screen(), OnboardingScreen::JiraToken);
        assert_eq!(credentials.atlassian_token, "jira-token");

        state.accept_verified_jira(credentials, "account-1".to_owned())?;
        assert_eq!(state.screen(), OnboardingScreen::Tempo);
        assert!(state.can_retain_jira_token());

        Ok(())
    }

    #[test]
    fn tempo_acceptance_completes_setup_and_finish_returns_selected_credentials(
    ) -> Result<(), OnboardingError> {
        let mut state = verified_jira_state()?;
        let attempt =
            state.prepare_tempo_connection(SecretInput::Replace(" tempo-token ".to_owned()))?;
        state.accept_verified_tempo()?;
        let completed = state.finish()?;

        assert_eq!(attempt.account_id, "account-1");
        assert_eq!(completed.tempo_token, "tempo-token");
        assert_eq!(completed.account_id, "account-1");
        assert_eq!(completed.atlassian_user_email, "person@example.com");

        Ok(())
    }

    #[test]
    fn retaining_secrets_requires_existing_tokens() -> Result<(), OnboardingError> {
        let mut state = OnboardingState::new(SetupDefaults::default(), false);
        state.continue_from_jira_details()?;

        let error = state
            .prepare_jira_connection(
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
                SecretInput::Retain,
            )
            .err();

        assert_eq!(error, Some(OnboardingError::TokenRequired));
        Ok(())
    }

    #[test]
    fn invalidation_rewinds_verified_connections() -> Result<(), OnboardingError> {
        let mut state = verified_jira_state()?;
        state.prepare_tempo_connection(SecretInput::Replace("tempo-token".to_owned()))?;
        state.accept_verified_tempo()?;

        state.edit_jira();
        state.continue_from_jira_details()?;
        state.invalidate_jira();

        assert_eq!(state.screen(), OnboardingScreen::JiraToken);
        assert_eq!(state.finish().err(), Some(OnboardingError::InvalidState));
        Ok(())
    }
}
