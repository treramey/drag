use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use drag::models::{AddWorklogRequest, ScheduleEntity, WorklogEntity};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use url::Url;

use crate::{config::Credentials, CliError};

const TEMPO_BASE: &str = "https://api.tempo.io/4/";

pub struct ApiClient {
    client: Client,
    credentials: Credentials,
    debug: bool,
    tempo_base: Url,
    tempo_origin: String,
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
        let client = Client::builder()
            .user_agent(concat!("drag/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(CliError::Http)?;
        let tempo_origin = tempo_base.origin().ascii_serialization();
        Ok(Self {
            client,
            credentials,
            debug,
            tempo_base,
            tempo_origin,
        })
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

    pub async fn get_worklogs(&self, from: &str, to: &str) -> Result<Vec<WorklogEntity>, CliError> {
        let account = safe_segment(&self.credentials.account_id)?;
        let url = self
            .tempo_base
            .join(&format!("worklogs/user/{account}"))
            .map_err(CliError::Url)?;
        let first =
            self.tempo(Method::GET, url)
                .query(&[("from", from), ("to", to), ("limit", "1000")]);
        let mut page: Page<WorklogEntity> = self.json(first).await?;
        let mut results = std::mem::take(&mut page.results);
        let mut next = page.metadata.next;
        let mut pages = 1_u16;
        while let Some(next_url) = next {
            pages += 1;
            if pages > 100 {
                return Err(CliError::Api(
                    "Tempo pagination exceeded the 100-page safety limit".to_owned(),
                ));
            }
            let url = Url::parse(&next_url).map_err(|_| {
                CliError::Api("Tempo returned a malformed pagination URL".to_owned())
            })?;
            if url.origin().ascii_serialization() != self.tempo_origin {
                return Err(CliError::Api(
                    "Tempo returned an unsafe pagination URL".to_owned(),
                ));
            }
            let mut page: Page<WorklogEntity> = self.json(self.tempo(Method::GET, url)).await?;
            results.append(&mut page.results);
            next = page.metadata.next;
        }
        Ok(results)
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
            return Err(CliError::Api(
                "Jira returned an empty account ID".to_owned(),
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
        if self.debug {
            eprintln!("debug: {} {}", request.method(), request.url());
        }
        let response = self.client.execute(request).await.map_err(CliError::Http)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(CliError::Http)?;
        if self.debug {
            eprintln!("debug: response {status}");
        }
        if !status.is_success() {
            return Err(api_error(status, &bytes, &self.redaction_secrets()));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| CliError::Api(format!("server returned malformed JSON: {error}")))
    }

    async fn empty(&self, builder: RequestBuilder) -> Result<(), CliError> {
        let request = builder.build().map_err(CliError::Http)?;
        if self.debug {
            eprintln!("debug: {} {}", request.method(), request.url());
        }
        let response = self.client.execute(request).await.map_err(CliError::Http)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(CliError::Http)?;
        if status.is_success() {
            Ok(())
        } else {
            Err(api_error(status, &bytes, &self.redaction_secrets()))
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

fn safe_segment(value: &str) -> Result<String, CliError> {
    if value.is_empty()
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '?' | '#' | '%')
        })
    {
        return Err(CliError::InvalidInput(format!(
            "unsafe issue or account identifier: {value:?}"
        )));
    }
    Ok(value.to_owned())
}

fn api_error(status: StatusCode, body: &[u8], secrets: &[String]) -> CliError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return CliError::Authentication(format!(
            "{status}; credentials are invalid, expired, or lack access"
        ));
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
    match details {
        Some(details) => CliError::Api(format!("server returned {status}: {}", redact(details))),
        None => CliError::Api(format!("server returned {status}")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::thread;
    use std::time::{Duration, Instant};

    use reqwest::{Method, StatusCode};
    use url::Url;

    use super::{api_error, safe_segment, ApiClient};
    use crate::{config::Credentials, CliError};

    type MockServer = thread::JoinHandle<Result<(), String>>;

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

    fn mock_tempo(bodies: Vec<String>) -> Result<(Url, MockServer), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let base = Url::parse(&format!("http://{address}/4/"))?;
        let base_text = base.as_str().to_owned();
        let handle = thread::spawn(move || {
            for body in bodies {
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return Err("timed out waiting for a Tempo request".to_owned());
                            }
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            return Err(format!("failed to accept Tempo request: {error}"))
                        }
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|error| format!("failed to set request timeout: {error}"))?;
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                loop {
                    let bytes_read = stream
                        .read(&mut chunk)
                        .map_err(|error| format!("failed to read Tempo request: {error}"))?;
                    if bytes_read == 0 {
                        return Err("Tempo request ended before its headers".to_owned());
                    }
                    request.extend_from_slice(&chunk[..bytes_read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                    if request.len() > 64 * 1024 {
                        return Err("Tempo request headers exceeded 64 KiB".to_owned());
                    }
                }
                let body = body.replace("{MOCK_TEMPO_BASE}", &base_text);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .map_err(|error| format!("failed to write Tempo response: {error}"))?;
                stream
                    .flush()
                    .map_err(|error| format!("failed to flush Tempo response: {error}"))?;
                if let Err(error) = stream.shutdown(Shutdown::Write) {
                    if error.kind() != std::io::ErrorKind::NotConnected {
                        return Err(format!("failed to finish Tempo response: {error}"));
                    }
                }
            }
            Ok(())
        });
        Ok((base, handle))
    }

    fn finish_mock(server: MockServer) -> Result<(), Box<dyn std::error::Error>> {
        let result = server
            .join()
            .map_err(|_| std::io::Error::other("mock Tempo server panicked"))?;
        result.map_err(std::io::Error::other)?;
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_aggregates_pages_and_stops_at_terminal_page(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let bodies = vec![
            format!(
                r#"{{"results":[{}],"metadata":{{"next":"{{MOCK_TEMPO_BASE}}worklogs?page=2"}}}}"#,
                worklog("1")
            ),
            format!(r#"{{"results":[{}],"metadata":{{}}}}"#, worklog("2")),
        ];
        let (base, server) = mock_tempo(bodies)?;
        let api = ApiClient::with_tempo_base(credentials(), false, base)?;

        let results = api.get_worklogs("2026-07-01", "2026-07-31").await?;

        assert_eq!(
            results
                .iter()
                .map(|worklog| worklog.tempo_worklog_id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        finish_mock(server)?;
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_rejects_cross_origin_continuations(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            r#"{{"results":[{}],"metadata":{{"next":"https://attacker.example/worklogs?token=tempo-secret"}}}}"#,
            worklog("1")
        );
        let (base, server) = mock_tempo(vec![body])?;
        let api = ApiClient::with_tempo_base(credentials(), false, base)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("unsafe continuation unexpectedly succeeded")?;

        assert!(error.to_string().contains("unsafe pagination URL"));
        assert!(!error.to_string().contains("tempo-secret"));
        finish_mock(server)?;
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_treats_malformed_continuations_as_redacted_runtime_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            r#"{{"results":[{}],"metadata":{{"next":"https://[tempo-secret"}}}}"#,
            worklog("1")
        );
        let (base, server) = mock_tempo(vec![body])?;
        let api = ApiClient::with_tempo_base(credentials(), false, base)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("malformed continuation unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Api(_)));
        assert_eq!(error.exit_code(), 1);
        assert!(!error.to_string().contains("tempo-secret"));
        finish_mock(server)?;
        Ok(())
    }

    #[tokio::test]
    async fn successful_responses_with_malformed_json_are_runtime_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (base, server) = mock_tempo(vec!["{not json".to_owned()])?;
        let api = ApiClient::with_tempo_base(credentials(), false, base)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("malformed response unexpectedly succeeded")?;

        assert!(matches!(&error, CliError::Api(_)));
        assert_eq!(error.exit_code(), 1);
        finish_mock(server)?;
        Ok(())
    }

    #[tokio::test]
    async fn worklog_pagination_retains_the_hundred_page_safety_limit(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = r#"{"results":[],"metadata":{"next":"{MOCK_TEMPO_BASE}worklogs?page=next"}}"#
            .to_owned();
        let (base, server) = mock_tempo(vec![body; 100])?;
        let api = ApiClient::with_tempo_base(credentials(), false, base)?;

        let error = api
            .get_worklogs("2026-07-01", "2026-07-31")
            .await
            .err()
            .ok_or("unbounded pagination unexpectedly succeeded")?;

        assert!(error.to_string().contains("100-page safety limit"));
        finish_mock(server)?;
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
            assert!(safe_segment(value).is_err(), "{value:?}");
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
            assert!(matches!(
                api_error(status, b"", &[]),
                CliError::Authentication(_)
            ));
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
            "API request failed: server returned 400 Bad Request: rejected [REDACTED]"
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
