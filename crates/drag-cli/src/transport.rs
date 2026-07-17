use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, Method, Request, Response, StatusCode};

use crate::CliError;

const MAX_ATTEMPTS: u32 = 3;
const MAX_RETRY_DELAY_SECS: u64 = 30;

pub(crate) fn shared_client() -> Result<Client, CliError> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(build_client) {
        Ok(client) => Ok(client.clone()),
        Err(message) => Err(CliError::Api(message.clone())),
    }
}

pub(crate) async fn execute(client: &Client, request: Request) -> Result<Response, CliError> {
    if !is_idempotent_read(request.method()) || request.try_clone().is_none() {
        return client.execute(request).await.map_err(CliError::Http);
    }

    for attempt in 0..MAX_ATTEMPTS {
        let Some(retry_request) = request.try_clone() else {
            unreachable!("cloneability was checked before the retry loop");
        };
        match client.execute(retry_request).await {
            Ok(response)
                if is_retryable_status(response.status()) && attempt + 1 < MAX_ATTEMPTS =>
            {
                tokio::time::sleep(retry_delay(&response, attempt)).await;
            }
            Ok(response) => return Ok(response),
            Err(error)
                if (error.is_connect() || error.is_timeout()) && attempt + 1 < MAX_ATTEMPTS =>
            {
                tokio::time::sleep(exponential_delay(attempt)).await;
            }
            Err(error) => return Err(CliError::Http(error)),
        }
    }
    unreachable!("the bounded retry loop always returns on its final attempt")
}

fn build_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("drag/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}

fn is_idempotent_read(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn retry_delay(response: &Response, attempt: u32) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(MAX_RETRY_DELAY_SECS)))
        .unwrap_or_else(|| exponential_delay(attempt))
}

fn exponential_delay(attempt: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_pow(attempt).min(MAX_RETRY_DELAY_SECS))
}

#[cfg(test)]
mod tests {
    use super::{exponential_delay, is_idempotent_read, is_retryable_status};
    use reqwest::{Method, StatusCode};

    #[test]
    fn retries_are_limited_to_idempotent_reads() {
        assert!(is_idempotent_read(&Method::GET));
        assert!(!is_idempotent_read(&Method::POST));
        assert!(!is_idempotent_read(&Method::DELETE));
    }

    #[test]
    fn transient_server_statuses_are_retryable() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[test]
    fn exponential_retry_delay_is_bounded() {
        assert_eq!(exponential_delay(20).as_secs(), 30);
    }
}
