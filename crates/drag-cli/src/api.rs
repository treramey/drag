use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use drag::models::{AddWorklogRequest, ScheduleEntity, WorklogEntity};
use drag::pagination::{
    PaginationPlan, TraversalDecision, TraversalError, TraversalState, DEFAULT_RECORD_LIMIT,
};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use url::Url;

use crate::transport;
use crate::{config::Credentials, CliError, RemoteError, RemoteErrorKind, RemoteService};

const TEMPO_BASE: &str = "https://api.tempo.io/4/";

pub(crate) fn validate_tempo_continuation_input(
    value: &str,
    from: &str,
    to: &str,
    plan: PaginationPlan,
) -> Result<(), CliError> {
    let origin = Url::parse(TEMPO_BASE)
        .map_err(CliError::Url)?
        .origin()
        .ascii_serialization();
    parse_tempo_continuation(value, &origin, from, to, plan)
        .map(|_| ())
        .map_err(|_| {
            CliError::InvalidInput(
                "continuation must be a Tempo pagination URL for the selected month".to_owned(),
            )
        })
}

pub struct ApiClient {
    client: Client,
    credentials: Credentials,
    debug: bool,
    tempo_base: Url,
    tempo_origin: String,
    #[cfg(test)]
    atlassian_base: Option<Url>,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    results: Vec<T>,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Debug, Default, Deserialize)]
struct Metadata {
    next: Option<String>,
}

#[derive(Debug)]
pub(crate) struct WorklogPage {
    pub(crate) results: Vec<WorklogEntity>,
    pub(crate) next: Option<String>,
    pub(crate) pages_retrieved: u16,
}

impl ApiClient {
    pub fn new(credentials: Credentials, debug: bool) -> Result<Self, CliError> {
        Self::with_tempo_base(
            credentials,
            debug,
            Url::parse(TEMPO_BASE).map_err(CliError::Url)?,
        )
    }

    fn with_tempo_base(
        credentials: Credentials,
        debug: bool,
        tempo_base: Url,
    ) -> Result<Self, CliError> {
        let client = transport::shared_client()?;
        let tempo_origin = tempo_base.origin().ascii_serialization();
        Ok(Self {
            client,
            credentials,
            debug,
            tempo_base,
            tempo_origin,
            #[cfg(test)]
            atlassian_base: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_bases(
        credentials: Credentials,
        debug: bool,
        tempo_base: Url,
        atlassian_base: Url,
    ) -> Result<Self, CliError> {
        let mut client = Self::with_tempo_base(credentials, debug, tempo_base)?;
        client.atlassian_base = Some(atlassian_base);
        Ok(client)
    }

    pub async fn add_worklog(
        &self,
        mut request: AddWorklogRequest,
    ) -> Result<WorklogEntity, CliError> {
        request.author_account_id = Some(self.credentials.account_id.clone());
        let url = self.tempo_base.join("worklogs").map_err(CliError::Url)?;
        self.json(self.tempo(Method::POST, url).json(&request))
            .await
    }

    pub(crate) async fn execute_openapi_value(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
    ) -> Result<Value, CliError> {
        if url.origin().ascii_serialization() != self.tempo_origin {
            return Err(CliError::InvalidInput(
                "OpenAPI request must stay on the configured Tempo origin".to_owned(),
            ));
        }
        let mut request = self.tempo(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }
        let request = request.build().map_err(CliError::Http)?;
        let service = RemoteService::from_url(request.url());
        if self.debug {
            eprintln!("debug: {} {}", request.method(), request.url());
        }
        let response = transport::execute(&self.client, request).await?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(CliError::Http)?;
        if self.debug {
            eprintln!("debug: response {status}");
        }
        if !status.is_success() {
            return Err(api_error_for_service(
                service,
                status,
                &bytes,
                &self.redaction_secrets(),
            ));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            CliError::Remote(RemoteError {
                service,
                status: Some(status),
                kind: RemoteErrorKind::InvalidResponse,
                message: format!("returned malformed JSON: {error}"),
            })
        })
    }

    pub async fn get_worklog(&self, id: u64) -> Result<WorklogEntity, CliError> {
        let url = self
            .tempo_base
            .join(&format!("worklogs/{id}"))
            .map_err(CliError::Url)?;
        self.json(self.tempo(Method::GET, url)).await
    }

    pub async fn delete_worklog(&self, id: u64) -> Result<(), CliError> {
        let url = self
            .tempo_base
            .join(&format!("worklogs/{id}"))
            .map_err(CliError::Url)?;
        self.empty(self.tempo(Method::DELETE, url)).await
    }

    #[cfg(test)]
    async fn get_worklogs(&self, from: &str, to: &str) -> Result<Vec<WorklogEntity>, CliError> {
        Ok(self
            .get_worklogs_bounded(from, to, PaginationPlan::all_pages(), None)
            .await?
            .results)
    }

    pub(crate) async fn get_worklogs_bounded(
        &self,
        from: &str,
        to: &str,
        plan: PaginationPlan,
        continue_from: Option<&str>,
    ) -> Result<WorklogPage, CliError> {
        let mut results = Vec::new();
        let mut traversal = TraversalState::new(plan);
        let mut continuation = continue_from.map(str::to_owned);

        loop {
            let mut page = self
                .get_worklog_page(from, to, plan, continuation.as_deref(), results.len())
                .await?;
            let decision = traversal
                .consume_page(
                    page.pages_retrieved,
                    page.results.len(),
                    page.next.is_some(),
                )
                .map_err(pagination_traversal_error)?;
            results.append(&mut page.results);

            match (decision, page.next) {
                (TraversalDecision::Continue, Some(next)) => continuation = Some(next),
                (TraversalDecision::Complete, _) => {
                    return Ok(WorklogPage {
                        results,
                        next: None,
                        pages_retrieved: traversal.pages_retrieved(),
                    });
                }
                (TraversalDecision::Bounded, next) => {
                    return Ok(WorklogPage {
                        results,
                        next,
                        pages_retrieved: traversal.pages_retrieved(),
                    });
                }
                (TraversalDecision::Continue, None) => {
                    return Err(CliError::invariant(
                        "Tempo pagination state was internally inconsistent".to_owned(),
                    ));
                }
            }
        }
    }

    pub(crate) async fn get_worklog_page(
        &self,
        from: &str,
        to: &str,
        plan: PaginationPlan,
        continue_from: Option<&str>,
        records_retrieved: usize,
    ) -> Result<WorklogPage, CliError> {
        let account = safe_segment(&self.credentials.account_id)?;
        let url = match continue_from {
            Some(continuation) => self.valid_tempo_continuation(continuation, from, to, plan)?,
            None => self
                .tempo_base
                .join(&format!("worklogs/user/{account}"))
                .map_err(CliError::Url)?,
        };
        let request_limit = plan.request_limit(records_retrieved);
        let request = self.tempo(Method::GET, url);
        let request = if continue_from.is_none() {
            let request_limit = request_limit.to_string();
            request.query(&[("from", from), ("to", to), ("limit", &request_limit)])
        } else {
            request
        };
        let page: Page<WorklogEntity> = self.json(request).await?;
        if page.results.len() > request_limit {
            return Err(CliError::invalid_response(
                RemoteService::Tempo,
                None,
                "returned more worklogs than the requested record limit",
            ));
        }
        if let Some(next) = &page.metadata.next {
            self.valid_tempo_continuation(next, from, to, plan)?;
        }
        Ok(WorklogPage {
            results: page.results,
            next: page.metadata.next,
            pages_retrieved: 1,
        })
    }

    fn valid_tempo_continuation(
        &self,
        value: &str,
        from: &str,
        to: &str,
        plan: PaginationPlan,
    ) -> Result<Url, CliError> {
        parse_tempo_continuation(value, &self.tempo_origin, from, to, plan).map_err(|kind| {
            match kind {
                ContinuationError::Malformed => CliError::invalid_response(
                    RemoteService::Tempo,
                    None,
                    "returned a malformed pagination URL",
                ),
                ContinuationError::Unsafe => CliError::invalid_response(
                    RemoteService::Tempo,
                    None,
                    "returned an unsafe pagination URL",
                ),
                ContinuationError::Range => CliError::invalid_response(
                    RemoteService::Tempo,
                    None,
                    "returned pagination for an unexpected date range",
                ),
                ContinuationError::Limit => CliError::invalid_response(
                    RemoteService::Tempo,
                    None,
                    "returned pagination with an unsafe record limit",
                ),
            }
        })
    }

    pub async fn get_schedule(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<ScheduleEntity>, CliError> {
        let account = safe_segment(&self.credentials.account_id)?;
        let url = self
            .tempo_base
            .join(&format!("user-schedule/{account}"))
            .map_err(CliError::Url)?;
        let page: Page<ScheduleEntity> = self
            .json(
                self.tempo(Method::GET, url)
                    .query(&[("from", from), ("to", to)]),
            )
            .await?;
        Ok(page.results)
    }

    pub async fn get_issue_id(&self, issue_key: &str) -> Result<String, CliError> {
        #[derive(Deserialize)]
        struct IssueId {
            id: String,
        }
        let response: IssueId = self.json(self.atlassian(issue_key)?).await?;
        Ok(response.id)
    }

    pub async fn get_issue_key(&self, issue_id: &str) -> Result<String, CliError> {
        #[derive(Deserialize)]
        struct IssueKey {
            key: String,
        }
        let response: IssueKey = self.json(self.atlassian(issue_id)?).await?;
        Ok(response.key)
    }

    pub async fn get_current_user_account_id(&self) -> Result<String, CliError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CurrentUser {
            account_id: String,
        }

        let response: CurrentUser = self.json(self.atlassian_current_user()?).await?;
        let account_id = response.account_id.trim();
        if account_id.is_empty() {
            return Err(CliError::invalid_response(
                RemoteService::Jira,
                None,
                "returned an empty account ID",
            ));
        }
        Ok(account_id.to_owned())
    }

    pub async fn verify_tempo_connection(&self) -> Result<(), CliError> {
        let request = self.tempo_verification_request()?;
        let _: Page<Value> = self.json(request).await?;
        Ok(())
    }

    fn tempo(&self, method: Method, url: Url) -> RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.credentials.tempo_token)
    }

    fn atlassian(&self, issue: &str) -> Result<RequestBuilder, CliError> {
        let issue = safe_segment(issue)?;
        let url = self.atlassian_url(&format!("issue/{issue}"))?;
        Ok(self.atlassian_request(url))
    }

    fn atlassian_current_user(&self) -> Result<RequestBuilder, CliError> {
        Ok(self.atlassian_request(self.atlassian_url("myself")?))
    }

    fn atlassian_url(&self, endpoint: &str) -> Result<Url, CliError> {
        #[cfg(test)]
        if let Some(base) = &self.atlassian_base {
            return base.join(endpoint).map_err(CliError::Url);
        }
        let hostname = self.credentials.hostname.trim();
        if hostname.is_empty()
            || hostname
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
            || hostname.contains(['/', '?', '#', '@', ':', '%'])
        {
            return Err(CliError::InvalidInput(
                "invalid Atlassian hostname".to_owned(),
            ));
        }
        Url::parse(&format!("https://{hostname}/rest/api/3/{endpoint}")).map_err(CliError::Url)
    }

    fn atlassian_request(&self, url: Url) -> RequestBuilder {
        let basic = self.atlassian_basic_auth();
        self.client
            .get(url)
            .header(reqwest::header::AUTHORIZATION, format!("Basic {basic}"))
    }

    fn tempo_verification_request(&self) -> Result<RequestBuilder, CliError> {
        let account = safe_segment(&self.credentials.account_id)?;
        let url = self
            .tempo_base
            .join(&format!("worklogs/user/{account}"))
            .map_err(CliError::Url)?;
        let today = Utc::now().date_naive().to_string();
        Ok(self.tempo(Method::GET, url).query(&[
            ("from", today.as_str()),
            ("to", today.as_str()),
            ("limit", "1"),
        ]))
    }

    async fn json<T: DeserializeOwned>(&self, builder: RequestBuilder) -> Result<T, CliError> {
        let request = builder.build().map_err(CliError::Http)?;
        let service = RemoteService::from_url(request.url());
        if self.debug {
            eprintln!("debug: {} {}", request.method(), request.url());
        }
        let response = transport::execute(&self.client, request).await?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(CliError::Http)?;
        if self.debug {
            eprintln!("debug: response {status}");
        }
        if !status.is_success() {
            return Err(api_error_for_service(
                service,
                status,
                &bytes,
                &self.redaction_secrets(),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            CliError::Remote(RemoteError {
                service,
                status: Some(status),
                kind: RemoteErrorKind::InvalidResponse,
                message: format!("returned malformed JSON: {error}"),
            })
        })
    }

    async fn empty(&self, builder: RequestBuilder) -> Result<(), CliError> {
        let request = builder.build().map_err(CliError::Http)?;
        let service = RemoteService::from_url(request.url());
        if self.debug {
            eprintln!("debug: {} {}", request.method(), request.url());
        }
        let response = transport::execute(&self.client, request).await?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(CliError::Http)?;
        if status.is_success() {
            Ok(())
        } else {
            Err(api_error_for_service(
                service,
                status,
                &bytes,
                &self.redaction_secrets(),
            ))
        }
    }

    fn atlassian_basic_auth(&self) -> String {
        STANDARD.encode(format!(
            "{}:{}",
            self.credentials.atlassian_user_email, self.credentials.atlassian_token
        ))
    }

    fn redaction_secrets(&self) -> Vec<String> {
        vec![
            self.credentials.tempo_token.clone(),
            self.credentials.atlassian_token.clone(),
            self.atlassian_basic_auth(),
        ]
    }
}

fn pagination_traversal_error(error: TraversalError) -> CliError {
    match error {
        TraversalError::HardPageLimitExceeded => CliError::invalid_response(
            RemoteService::Tempo,
            None,
            "pagination exceeded the 100-page safety limit",
        ),
        TraversalError::AccountingOverflow => {
            CliError::invariant("Tempo pagination accounting overflowed")
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ContinuationError {
    Malformed,
    Unsafe,
    Range,
    Limit,
}

fn parse_tempo_continuation(
    value: &str,
    expected_origin: &str,
    expected_from: &str,
    expected_to: &str,
    plan: PaginationPlan,
) -> Result<Url, ContinuationError> {
    let url = Url::parse(value).map_err(|_| ContinuationError::Malformed)?;
    if url.origin().ascii_serialization() != expected_origin
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ContinuationError::Unsafe);
    }
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut limits = Vec::new();
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "from" => from.push(value.into_owned()),
            "to" => to.push(value.into_owned()),
            "limit" => limits.push(value.into_owned()),
            _ => {}
        }
    }
    if from.as_slice() != [expected_from] || to.as_slice() != [expected_to] {
        return Err(ContinuationError::Range);
    }
    let maximum = plan.record_limit().unwrap_or(DEFAULT_RECORD_LIMIT);
    if limits
        .first()
        .filter(|_| limits.len() == 1)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|limit| *limit > 0 && *limit <= maximum)
        .is_none()
    {
        return Err(ContinuationError::Limit);
    }
    Ok(url)
}

fn safe_segment(value: &str) -> Result<String, CliError> {
    if value.is_empty()
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '?' | '#' | '%')
        })
    {
        return Err(CliError::InvalidInput(
            "unsafe issue or account identifier".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

fn api_error_for_service(
    service: RemoteService,
    status: StatusCode,
    body: &[u8],
    secrets: &[String],
) -> CliError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return CliError::Remote(RemoteError {
            service,
            status: Some(status),
            kind: RemoteErrorKind::Authentication,
            message: format!("returned {status}; credentials are invalid, expired, or lack access"),
        });
    }
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let details = parsed
        .as_ref()
        .and_then(|value| value.get("errorMessages"))
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|value| value.get("errors"))
                .and_then(Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(|error| error.get("message").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|value| !value.is_empty())
        });
    let redact = |mut value: String| {
        let mut secrets = secrets
            .iter()
            .filter(|secret| !secret.is_empty())
            .collect::<Vec<_>>();
        secrets.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
        for secret in secrets {
            value = value.replace(secret, "[REDACTED]");
        }
        value
    };
    let message = match details {
        Some(details) => format!("returned {status}: {}", redact(details)),
        None => format!("returned {status}"),
    };
    CliError::Remote(RemoteError {
        service,
        status: Some(status),
        kind: RemoteErrorKind::Rejected,
        message,
    })
}

#[cfg(test)]
fn api_error(status: StatusCode, body: &[u8], secrets: &[String]) -> CliError {
    api_error_for_service(RemoteService::Unknown, status, body, secrets)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use reqwest::{Client, Method, StatusCode};
    use url::Url;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use drag::models::AddWorklogRequest;

    use super::{api_error, safe_segment, ApiClient, PaginationPlan};
    use crate::{config::Credentials, CliError, RemoteErrorKind};

    fn credentials() -> Credentials {
        Credentials {
            tempo_token: "tempo-secret".to_owned(),
            account_id: "account-1".to_owned(),
            atlassian_user_email: "person@example.com".to_owned(),
            atlassian_token: "jira-secret".to_owned(),
            hostname: "example.atlassian.net".to_owned(),
        }
    }

    fn worklog(id: &str) -> String {
        format!(
            r#"{{"tempoWorklogId":"{id}","startDate":"2026-07-14","startTime":"09:00:00","author":{{"accountId":"account-1"}},"issue":{{"self":"https://example.atlassian.net/issue/1","id":"1"}},"timeSpentSeconds":3600}}"#
        )
    }

    fn mock_tempo_base(server: &MockServer) -> Result<Url, url::ParseError> {
        Url::parse(&format!("{}/4/", server.uri()))
    }

    fn add_worklog_request() -> AddWorklogRequest {
        AddWorklogRequest {
            issue_id: "10001".to_owned(),
            time_spent_seconds: 4_500,
            start_date: "2026-07-14".to_owned(),
            start_time: "09:15:00".to_owned(),
            description: Some("review".to_owned()),
            remaining_estimate_seconds: Some(7_200),
            author_account_id: None,
        }
    }

    struct TransientThenSuccess {
        calls: AtomicUsize,
        success_body: String,
    }

    impl Respond for TransientThenSuccess {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(StatusCode::SERVICE_UNAVAILABLE)
                    .append_header("retry-after", "0")
            } else {
                ResponseTemplate::new(StatusCode::OK)
                    .set_body_raw(self.success_body.clone(), "application/json")
            }
        }
    }

    #[tokio::test]
    async fn add_worklog_posts_one_authenticated_request_with_the_configured_author(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let mut expected = add_worklog_request();
        expected.author_account_id = Some("account-1".to_owned());
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .and(header("authorization", "Bearer tempo-secret"))
            .and(body_json(expected))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(worklog("751393"), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let created = api.add_worklog(add_worklog_request()).await?;

        assert_eq!(created.tempo_worklog_id, "751393");
        Ok(())
    }

    #[tokio::test]
    async fn idempotent_reads_retry_transient_server_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(TransientThenSuccess {
                calls: AtomicUsize::new(0),
                success_body: r#"{"results":[],"metadata":{}}"#.to_owned(),
            })
            .expect(2)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let worklogs = api.get_worklogs("2026-07-01", "2026-07-31").await?;

        assert!(worklogs.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn mutations_are_not_retried_on_transient_server_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(StatusCode::SERVICE_UNAVAILABLE))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .add_worklog(add_worklog_request())
            .await
            .err()
            .ok_or("mutation unexpectedly succeeded")?;

        assert!(matches!(error, CliError::Remote(_)));
        Ok(())
    }

    #[tokio::test]
    async fn add_worklog_server_failure_is_redacted_and_classified_as_runtime_failure(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(500).set_body_raw(
                r#"{"errors":[{"message":"rejected tempo-secret and jira-secret"}]}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .add_worklog(add_worklog_request())
            .await
            .err()
            .ok_or("server failure unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Remote(_)));
        assert_eq!(error.exit_code(), 1);
        assert!(!error.to_string().contains("tempo-secret"));
        assert!(!error.to_string().contains("jira-secret"));
        assert!(error.to_string().contains("[REDACTED]"));
        Ok(())
    }

    #[tokio::test]
    async fn add_worklog_malformed_success_response_is_a_runtime_failure(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("{not json", "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .add_worklog(add_worklog_request())
            .await
            .err()
            .ok_or("malformed response unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Remote(_)));
        assert_eq!(error.exit_code(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn add_worklog_network_timeout_is_a_runtime_failure(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/4/worklogs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(1))
                    .set_body_raw(worklog("751393"), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;
        api.client = Client::builder()
            .timeout(Duration::from_millis(50))
            .build()?;

        let error = api
            .add_worklog(add_worklog_request())
            .await
            .err()
            .ok_or("network timeout unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Http(source) if source.is_timeout()));
        assert_eq!(error.exit_code(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn worklog_page_fetches_exactly_one_validated_page(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?page=2&from=2026-07-01&to=2026-07-31&limit=100",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"results":[{}],"metadata":{{"next":"{next}"}}}}"#,
                    worklog("1")
                ),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let page = api
            .get_worklog_page(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::all_pages(),
                None,
                0,
            )
            .await?;

        assert_eq!(page.results[0].tempo_worklog_id, "1");
        assert_eq!(page.pages_retrieved, 1);
        assert_eq!(page.next.as_deref(), Some(next.as_str()));
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_aggregates_pages_and_stops_at_terminal_page(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?page=2&from=2026-07-01&to=2026-07-31&limit=100",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"results":[{}],"metadata":{{"next":"{next}"}}}}"#,
                    worklog("1")
                ),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"{{"results":[{}],"metadata":{{}}}}"#, worklog("2")),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let page = api
            .get_worklogs_bounded(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::all_pages(),
                None,
            )
            .await?;

        assert_eq!(
            page.results
                .iter()
                .map(|worklog| worklog.tempo_worklog_id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(page.pages_retrieved, 2);
        assert!(page.next.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn bounded_worklog_pagination_stops_at_the_page_limit_and_returns_the_continuation(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?page=2&from=2026-07-01&to=2026-07-31&limit=10",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .and(query_param("limit", "10"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"results":[{}],"metadata":{{"next":"{next}"}}}}"#,
                    worklog("1")
                ),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let page = api
            .get_worklogs_bounded(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::bounded(10, 1)?,
                None,
            )
            .await?;

        assert_eq!(page.results.len(), 1);
        assert_eq!(page.pages_retrieved, 1);
        assert_eq!(page.next.as_deref(), Some(next.as_str()));
        Ok(())
    }

    #[tokio::test]
    async fn bounded_worklog_pagination_stops_at_the_record_limit(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?page=2&from=2026-07-01&to=2026-07-31&limit=1",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .and(query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"results":[{}],"metadata":{{"next":"{next}"}}}}"#,
                    worklog("1")
                ),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let page = api
            .get_worklogs_bounded(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::bounded(1, 3)?,
                None,
            )
            .await?;

        assert_eq!(page.results.len(), 1);
        assert_eq!(page.pages_retrieved, 1);
        assert_eq!(page.next.as_deref(), Some(next.as_str()));
        Ok(())
    }

    #[tokio::test]
    async fn bounded_worklog_pagination_resumes_from_an_exact_continuation(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs"))
            .and(query_param("page", "2"))
            .and(query_param("limit", "7"))
            .and(query_param("from", "2026-07-01"))
            .and(query_param("to", "2026-07-31"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"{{"results":[{}],"metadata":{{}}}}"#, worklog("2")),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;
        let continuation = format!(
            "{}/4/worklogs?page=2&limit=7&from=2026-07-01&to=2026-07-31",
            server.uri()
        );

        let page = api
            .get_worklogs_bounded(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::bounded(7, 1)?,
                Some(&continuation),
            )
            .await?;

        assert_eq!(page.results[0].tempo_worklog_id, "2");
        assert_eq!(page.pages_retrieved, 1);
        assert!(page.next.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn caller_continuations_are_validated_before_an_authenticated_request(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        for continuation in [
            "https://[tempo-secret",
            "https://attacker.example/worklogs?token=tempo-secret",
            &format!("http://user:password@{}/4/worklogs", server.address()),
            &format!(
                "{}/4/worklogs?from=2026-07-01&to=2026-07-31&limit=250&offset=250",
                server.uri()
            ),
        ] {
            let error = api
                .get_worklogs_bounded(
                    "2026-07-01",
                    "2026-07-31",
                    PaginationPlan::bounded(10, 1)?,
                    Some(continuation),
                )
                .await
                .err()
                .ok_or("unsafe caller continuation unexpectedly succeeded")?;

            assert_eq!(error.exit_code(), 1);
            assert!(!error.to_string().contains("tempo-secret"));
            assert!(!error.to_string().contains("password"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_rejects_cross_origin_continuations(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            r#"{{"results":[{}],"metadata":{{"next":"https://attacker.example/worklogs?token=tempo-secret"}}}}"#,
            worklog("1")
        );
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("unsafe continuation unexpectedly succeeded")?;

        assert!(error.to_string().contains("unsafe pagination URL"));
        assert!(!error.to_string().contains("tempo-secret"));
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_rejects_continuations_for_another_month(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?from=2026-06-01&to=2026-06-30&offset=100&limit=10",
            server.uri()
        );
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"results":[{}],"metadata":{{"next":"{next}"}}}}"#,
                    worklog("1")
                ),
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .get_worklogs_bounded(
                "2026-07-01",
                "2026-07-31",
                PaginationPlan::bounded(10, 1)?,
                None,
            )
            .await
            .err()
            .ok_or("mismatched continuation unexpectedly succeeded")?;

        assert!(error.to_string().contains("unexpected date range"));
        assert!(!error.to_string().contains("2026-06"));
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_treats_malformed_continuations_as_redacted_runtime_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            r#"{{"results":[{}],"metadata":{{"next":"https://[tempo-secret"}}}}"#,
            worklog("1")
        );
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("malformed continuation unexpectedly succeeded")?;

        assert!(matches!(
            &error,
            CliError::Remote(remote) if remote.kind == RemoteErrorKind::InvalidResponse
        ));
        assert_eq!(error.exit_code(), 1);
        assert!(!error.to_string().contains("tempo-secret"));
        Ok(())
    }

    #[tokio::test]
    async fn successful_responses_with_malformed_json_are_runtime_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("{not json", "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("malformed response unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Remote(_)));
        assert_eq!(error.exit_code(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_retains_the_hundred_page_safety_limit(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let next = format!(
            "{}/4/worklogs?page=next&from=2026-07-01&to=2026-07-31&limit=100",
            server.uri()
        );
        let body = format!(r#"{{"results":[],"metadata":{{"next":"{next}"}}}}"#);
        Mock::given(method("GET"))
            .and(path("/4/worklogs/user/account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body.clone(), "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/4/worklogs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(99)
            .mount(&server)
            .await;
        let api = ApiClient::with_tempo_base(credentials(), false, mock_tempo_base(&server)?)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("unbounded pagination unexpectedly succeeded")?;

        assert!(error.to_string().contains("100-page safety limit"));
        Ok(())
    }

    #[test]
    fn rejects_identifiers_that_can_change_a_url() {
        for value in [
            "",
            "ABC/1",
            "ABC?x=1",
            "ABC#fragment",
            "ABC%2F1",
            "ABC 1",
            "ABC\n1",
        ] {
            let result = safe_segment(value);
            assert!(result.is_err(), "{value:?}");
            if let Err(error) = result {
                if !value.is_empty() {
                    assert!(!error.to_string().contains(value), "{value:?}");
                }
            }
        }
        assert!(safe_segment("ABC-123").is_ok());
    }

    #[test]
    fn extracts_tempo_error_messages() {
        let error = api_error(
            StatusCode::BAD_REQUEST,
            br#"{"errors":[{"message":"bad worklog"}]}"#,
            &[],
        );
        assert!(error.to_string().contains("bad worklog"));
    }

    #[test]
    fn classifies_authentication_statuses_as_correctable_credentials() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            assert!(api_error(status, b"", &[]).is_authentication());
        }
    }

    #[test]
    fn redacts_tokens_echoed_by_a_server() {
        let error = api_error(
            StatusCode::BAD_REQUEST,
            br#"{"errors":[{"message":"rejected tempo-secret and jira-secret"}]}"#,
            &["tempo-secret".to_owned(), "jira-secret".to_owned()],
        );
        let message = error.to_string();
        assert!(!message.contains("tempo-secret"));
        assert!(!message.contains("jira-secret"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_overlapping_tokens_longest_first() {
        let error = api_error(
            StatusCode::BAD_REQUEST,
            br#"{"errors":[{"message":"rejected token-with-suffix"}]}"#,
            &["token".to_owned(), "token-with-suffix".to_owned()],
        );

        assert_eq!(
            error.to_string(),
            "API request failed: remote service returned 400 Bad Request: rejected [REDACTED]"
        );
    }

    #[test]
    fn redacts_encoded_basic_credentials() -> Result<(), Box<dyn std::error::Error>> {
        let api = ApiClient::new(
            Credentials {
                tempo_token: "tempo-secret".to_owned(),
                account_id: "account-1".to_owned(),
                atlassian_user_email: "person@example.com".to_owned(),
                atlassian_token: "jira-secret".to_owned(),
                hostname: "example.atlassian.net".to_owned(),
            },
            false,
        )?;
        let basic = api.atlassian_basic_auth();
        let body = format!(r#"{{"errors":[{{"message":"rejected Basic {basic}"}}]}}"#);

        let error = api_error(
            StatusCode::BAD_REQUEST,
            body.as_bytes(),
            &api.redaction_secrets(),
        );

        assert!(!error.to_string().contains(&basic));
        Ok(())
    }

    #[test]
    fn verification_requests_are_read_only() -> Result<(), Box<dyn std::error::Error>> {
        let api = ApiClient::new(
            Credentials {
                tempo_token: "tempo-secret".to_owned(),
                account_id: "account-1".to_owned(),
                atlassian_user_email: "person@example.com".to_owned(),
                atlassian_token: "jira-secret".to_owned(),
                hostname: "example.atlassian.net".to_owned(),
            },
            false,
        )?;

        let jira = api.atlassian_current_user()?.build()?;
        assert_eq!(jira.method(), Method::GET);
        assert_eq!(
            jira.url().as_str(),
            "https://example.atlassian.net/rest/api/3/myself"
        );

        let tempo = api.tempo_verification_request()?.build()?;
        assert_eq!(tempo.method(), Method::GET);
        assert_eq!(tempo.url().path(), "/4/worklogs/user/account-1");
        assert_eq!(
            tempo
                .url()
                .query_pairs()
                .find(|(key, _)| key == "limit")
                .map(|(_, value)| value.into_owned()),
            Some("1".to_owned())
        );
        Ok(())
    }
}
