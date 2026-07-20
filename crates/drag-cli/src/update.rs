//! Best-effort release update discovery for interactive interfaces.

use std::time::Duration;

use semver::Version;
use serde::Deserialize;

use crate::transport;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/treramey/drag/releases/latest";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// Returns the latest version only when it is newer than this binary.
///
/// Update discovery is supplemental: offline use, rate limits, malformed
/// responses, and timeouts all resolve to no notification.
pub(crate) async fn available_version() -> Option<String> {
    tokio::time::timeout(UPDATE_CHECK_TIMEOUT, fetch_available_version())
        .await
        .ok()
        .flatten()
}

async fn fetch_available_version() -> Option<String> {
    let client = transport::shared_client().ok()?;
    let request = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .build()
        .ok()?;
    let response = transport::execute(&client, request).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release = response.json::<LatestRelease>().await.ok()?;
    newer_version(env!("CARGO_PKG_VERSION"), &release.tag_name)
}

fn newer_version(current: &str, latest_tag: &str) -> Option<String> {
    let current = Version::parse(current).ok()?;
    let latest = Version::parse(latest_tag.strip_prefix('v').unwrap_or(latest_tag)).ok()?;
    (latest > current).then(|| latest.to_string())
}

#[cfg(test)]
mod tests {
    use super::newer_version;

    #[test]
    fn recognizes_a_newer_release_with_or_without_a_v_prefix() {
        assert_eq!(newer_version("0.5.0", "v0.6.0"), Some("0.6.0".to_owned()));
        assert_eq!(newer_version("0.5.0", "1.0.0"), Some("1.0.0".to_owned()));
    }

    #[test]
    fn ignores_current_older_and_malformed_releases() {
        assert_eq!(newer_version("0.5.0", "v0.5.0"), None);
        assert_eq!(newer_version("0.5.0", "v0.4.0"), None);
        assert_eq!(newer_version("0.5.0", "latest"), None);
    }
}
