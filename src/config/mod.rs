pub mod error;
pub mod templates;

use crate::config::error::ConfigError;
use crate::shared::utils;
use mlua::{Lua, Table, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    pub source: Source,
    pub output: Output,
    pub settings: Settings,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    pub tz: String,
    pub oauth_file_path: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub google: GoogleSource,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub template: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GoogleSource {
    pub oauth2: GoogleOAuth2,
    pub calendar: GoogleCalendar,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GoogleOAuth2 {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GoogleCalendar {
    pub get_events: GoogleCalendarGetEvents,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GoogleCalendarGetEvents {
    pub calendar_ids: Vec<String>,
}

pub fn init() -> anyhow::Result<Config> {
    let path_buf = get_config_file_path()?;
    load_config(&path_buf)
}

fn get_config_file_path() -> anyhow::Result<PathBuf> {
    let config_file_path = match std::env::var("CAL2_PROMPT_CONFIG_FILE_PATH") {
        Ok(path) => path.trim().to_string(),
        Err(_) => {
            let home_dir =
                env::var("HOME").map_err(|_e| ConfigError::HomeEnvironmentNotFoundError)?;
            let path = format!("{}/.config/cal2prompt/config.lua", home_dir);
            path
        }
    };

    let config_file_path_buf = utils::path::expand_tilde(&config_file_path);

    if config_file_path_buf.is_file() {
        Ok(config_file_path_buf)
    } else {
        Err(
            ConfigError::ConfigFileNotFoundError(utils::path::contract_tilde(
                &config_file_path_buf,
            ))
            .into(),
        )
    }
}

fn get_oauth_path() -> anyhow::Result<PathBuf> {
    let home_dir = env::var("HOME").map_err(|_e| ConfigError::HomeEnvironmentNotFoundError)?;
    let default_path = format!("{}/.local/share/cal2prompt/oauth", home_dir);
    let p = PathBuf::from(&default_path);

    Ok(p)
}

fn load_config(config_file_path: &Path) -> anyhow::Result<Config> {
    let lua = Lua::new();

    let config_path = config_file_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_string_lossy();

    lua.load(format!(
        r#"package.path = package.path .. ";{}/?.lua""#,
        config_path
    ))
    .exec()?;

    let cal2prompt_mod = lua.create_table()?;
    let template_sub_mod = lua.create_table()?;
    let template_google_sub_mod = lua.create_table()?;

    template_google_sub_mod
        .set("standard", crate::config::templates::google::STANDARD)
        .map_err(|e| ConfigError::LuaRuntimeError(e.to_string()))?;
    template_sub_mod.set("google", template_google_sub_mod)?;
    cal2prompt_mod.set("template", template_sub_mod)?;

    let globals = lua.globals();
    let package: Table = globals.get("package")?;
    let loaded: Table = package.get("loaded")?;

    loaded.set("cal2prompt", cal2prompt_mod)?;

    let config_code = fs::read_to_string(config_file_path.to_string_lossy().to_string())?;
    let config_eval = lua.load(&config_code).eval()?;

    if let Value::Table(config_tbl) = config_eval {
        let source_tbl: Table = config_tbl
            .get::<_, Option<Table>>("source")?
            .ok_or_else(|| {
                ConfigError::RequiredFieldNotFound(
                    "source".to_owned(),
                    utils::path::contract_tilde(config_file_path),
                )
            })?;

        let google_tbl: Table = source_tbl
            .get::<_, Option<Table>>("google")?
            .ok_or_else(|| {
                ConfigError::RequiredFieldNotFound(
                    "source.google".to_owned(),
                    utils::path::contract_tilde(config_file_path),
                )
            })?;
        let google_oauth2_tbl: Table =
            google_tbl
                .get::<_, Option<Table>>("oauth2")?
                .ok_or_else(|| {
                    ConfigError::RequiredFieldNotFound(
                        "source.google.oauth2".to_owned(),
                        utils::path::contract_tilde(config_file_path),
                    )
                })?;
        let google_oauth2_client_id: String = google_oauth2_tbl
            .get::<_, Option<String>>("clientID")?
            .ok_or_else(|| {
                ConfigError::RequiredFieldNotFound(
                    "source.google.oauth2.clientID".to_owned(),
                    utils::path::contract_tilde(config_file_path),
                )
            })?;
        let google_oauth2_client_secret: String = google_oauth2_tbl
            .get::<_, Option<String>>("clientSecret")?
            .ok_or_else(|| {
                ConfigError::RequiredFieldNotFound(
                    "source.google.oauth2.clientSecret".to_owned(),
                    utils::path::contract_tilde(config_file_path),
                )
            })?;

        let default_scopes_table = lua.create_table()?;
        default_scopes_table.push("https://www.googleapis.com/auth/calendar.events")?;

        let google_oauth2_scopes: Vec<String> = google_oauth2_tbl
            .get::<_, Option<Table>>("scopes")?
            .unwrap_or(default_scopes_table)
            .sequence_values()
            .collect::<Result<_, _>>()?;

        let google_calendar_tbl: Table = google_tbl.get("calendar")?;
        let google_get_events_tbl: Table = google_calendar_tbl.get("getEvents")?;
        let calendar_ids_table: Table = google_get_events_tbl.get("calendarIDs")?;
        let calendar_ids: Vec<String> = calendar_ids_table
            .sequence_values()
            .collect::<Result<_, _>>()?;

        let redirect_url: String = google_oauth2_tbl
            .get::<_, Option<String>>("redirectURL")?
            .unwrap_or("http://127.0.0.1:9004".to_string());

        let output_tbl: Table = config_tbl.get("output")?;
        let template: String = output_tbl
            .get::<_, Option<String>>("template")?
            .ok_or_else(|| {
                ConfigError::RequiredFieldNotFound(
                    "template not found".to_owned(),
                    utils::path::contract_tilde(config_file_path),
                )
            })?;

        let oauth_default_path = get_oauth_path()?;
        let settings: Settings = match config_tbl.get::<_, Option<Table>>("settings") {
            Ok(settings_tbl) => match settings_tbl {
                Some(table) => {
                    let oauth_file_path = table
                        .get::<_, Option<String>>("oauthFilePath")?
                        .unwrap_or(oauth_default_path.to_string_lossy().to_string());
                    let tz = table
                        .get::<_, Option<String>>("TZ")?
                        .unwrap_or("UTC".to_string());

                    Settings {
                        oauth_file_path,
                        tz,
                    }
                }
                None => Settings {
                    oauth_file_path: oauth_default_path.to_string_lossy().to_string(),
                    tz: "UTC".to_string(),
                },
            },
            Err(_) => Settings {
                oauth_file_path: oauth_default_path.to_string_lossy().to_string(),
                tz: "UTC".to_string(),
            },
        };
        let config = Config {
            source: Source {
                google: GoogleSource {
                    oauth2: GoogleOAuth2 {
                        client_id: google_oauth2_client_id,
                        client_secret: google_oauth2_client_secret,
                        redirect_url,
                        scopes: google_oauth2_scopes,
                    },
                    calendar: GoogleCalendar {
                        get_events: GoogleCalendarGetEvents { calendar_ids },
                    },
                },
            },
            output: Output { template },
            settings,
        };
        Ok(config)
    } else {
        Err(ConfigError::RequiredFieldNotFound(
            "config.lua did not return a table!".to_owned(),
            utils::path::contract_tilde(config_file_path),
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_config() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let config_path = temp_dir.path();

        let config_file_path = config_path.join("config.lua");
        let secrets_file_path = config_path.join("secrets.lua");

        let config_code = r#"
local cal2prompt = require("cal2prompt")
local secrets = require("secrets")

return {
  source = {
    google = {
      oauth2 = {
        clientID = secrets.googleOAuth2Client.clientID,
        clientSecret = secrets.googleOAuth2Client.clientSecret,
        redirectURL = "http://127.0.0.1:9004",
      },
      calendar = {
        getEvents = {
          calendarIDs = { "test@example.com" }
        }
      }
    },
  },
  output = {
    template = cal2prompt.template.google.standard
  }
}
"#;
        fs::write(&config_file_path, config_code)?;

        let secrets_code = r#"
local M = {}

M.googleOAuth2Client = {
  clientID = "test_client_id",
  clientSecret = "test_client_secret",
}

return M
"#;
        fs::write(&secrets_file_path, secrets_code)?;

        let config = load_config(&config_file_path)?;

        let home_dir = env::var("HOME")?;
        let oauth_file_path = format!("{}/.local/share/cal2prompt/oauth", home_dir);
        let tz = "UTC".to_string();
        let calendar_ids = vec!["test@example.com".to_string()];

        let expected = Config {
            source: Source {
                google: GoogleSource {
                    oauth2: GoogleOAuth2 {
                        client_id: "test_client_id".to_string(),
                        client_secret: "test_client_secret".to_string(),
                        redirect_url: "http://127.0.0.1:9004".to_string(),
                        scopes: vec!["https://www.googleapis.com/auth/calendar.events".to_string()],
                    },
                    calendar: GoogleCalendar {
                        get_events: GoogleCalendarGetEvents { calendar_ids },
                    },
                },
            },
            output: Output {
                template: crate::config::templates::google::STANDARD.to_string(),
            },
            settings: Settings {
                oauth_file_path,
                tz,
            },
        };

        assert_eq!(config, expected, "Config should match the expected struct");

        Ok(())
    }
}
