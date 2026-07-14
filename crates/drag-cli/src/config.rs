use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use drag::tracker::Tracker;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::CliError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atlassian_user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atlassian_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(
        default,
        with = "aliases_compat",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub aliases: BTreeMap<String, String>,
    #[serde(
        default,
        with = "trackers_compat",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub trackers: BTreeMap<String, Tracker>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    #[serde(skip_serializing)]
    pub tempo_token: String,
    pub account_id: String,
    pub atlassian_user_email: String,
    #[serde(skip_serializing)]
    pub atlassian_token: String,
    pub hostname: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, CliError> {
        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|source| CliError::Config {
                message: format!("could not parse {}", path.display()),
                source: Some(Box::new(source)),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(CliError::Config {
                message: format!("could not read {}", path.display()),
                source: Some(Box::new(source)),
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), CliError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(CliError::Io)?;
        }
        let temporary = path.with_extension("tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(CliError::Io)?;
        let contents = serde_json::to_vec_pretty(self).map_err(CliError::Json)?;
        file.write_all(&contents).map_err(CliError::Io)?;
        file.write_all(b"\n").map_err(CliError::Io)?;
        file.sync_all().map_err(CliError::Io)?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(CliError::Io)?;
        }
        fs::rename(&temporary, path).map_err(CliError::Io)?;
        Ok(())
    }

    pub fn credentials(&self) -> Result<Credentials, CliError> {
        fn value(configured: &Option<String>, environment: &str) -> Option<String> {
            std::env::var(environment)
                .ok()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    configured
                        .as_ref()
                        .filter(|value| !value.is_empty())
                        .cloned()
                })
        }

        let missing = |field: &str, variable: &str| {
            CliError::NotConfigured(format!(
                "missing {field}; run `drag setup` or set {variable}"
            ))
        };
        Ok(Credentials {
            tempo_token: value(&self.tempo_token, "TEMPO_TOKEN")
                .ok_or_else(|| missing("Tempo token", "TEMPO_TOKEN"))?,
            account_id: value(&self.account_id, "TEMPO_ACCOUNT_ID")
                .ok_or_else(|| missing("account ID", "TEMPO_ACCOUNT_ID"))?,
            atlassian_user_email: value(&self.atlassian_user_email, "ATLASSIAN_EMAIL")
                .ok_or_else(|| missing("Atlassian email", "ATLASSIAN_EMAIL"))?,
            atlassian_token: value(&self.atlassian_token, "ATLASSIAN_TOKEN")
                .ok_or_else(|| missing("Atlassian token", "ATLASSIAN_TOKEN"))?,
            hostname: value(&self.hostname, "ATLASSIAN_HOST")
                .ok_or_else(|| missing("Atlassian hostname", "ATLASSIAN_HOST"))?,
        })
    }

    pub fn resolve_issue(&self, issue_or_alias: &str) -> String {
        self.aliases
            .get(issue_or_alias)
            .cloned()
            .unwrap_or_else(|| issue_or_alias.to_owned())
    }
}

pub fn config_path() -> Result<PathBuf, CliError> {
    if let Some(path) = std::env::var_os("DRAG_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    dirs::home_dir()
        .map(|home| home.join(".drag"))
        .ok_or_else(|| CliError::Config {
            message: "could not determine the home directory".to_owned(),
            source: None,
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyMapRef<'a, T> {
    data_type: &'static str,
    value: Vec<(&'a String, &'a T)>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MapRepresentation<T> {
    Legacy {
        #[serde(rename = "dataType")]
        _data_type: String,
        value: Vec<(String, T)>,
    },
    Object(BTreeMap<String, T>),
}

fn serialize_map<S, T>(map: &BTreeMap<String, T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    LegacyMapRef {
        data_type: "Map",
        value: map.iter().collect(),
    }
    .serialize(serializer)
}

fn deserialize_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match MapRepresentation::deserialize(deserializer)? {
        MapRepresentation::Legacy { value, .. } => value.into_iter().collect(),
        MapRepresentation::Object(map) => map,
    })
}

mod aliases_compat {
    use super::*;

    pub fn serialize<S>(map: &BTreeMap<String, String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_map(map, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_map(deserializer)
    }
}

mod trackers_compat {
    use super::*;

    pub fn serialize<S>(map: &BTreeMap<String, Tracker>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_map(map, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<String, Tracker>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_map(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::{CliError, Config};

    #[test]
    fn reads_and_writes_typescript_map_format() -> Result<(), Box<dyn std::error::Error>> {
        let input = r#"{
          "tempoToken":"secret",
          "aliases":{"dataType":"Map","value":[["lunch","ABC-1"]]},
          "trackers":{"dataType":"Map","value":[]}
        }"#;
        let config: Config = serde_json::from_str(input)?;
        assert_eq!(
            config.aliases.get("lunch").map(String::as_str),
            Some("ABC-1")
        );
        let output = serde_json::to_string(&config)?;
        assert!(output.contains("\"dataType\":\"Map\""));
        Ok(())
    }

    #[test]
    fn malformed_config_is_not_silently_discarded() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.json");
        std::fs::write(&path, "not json")?;
        assert!(matches!(Config::load(&path), Err(CliError::Config { .. })));
        Ok(())
    }
}
