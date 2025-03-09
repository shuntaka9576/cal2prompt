pub mod error;
pub mod templates;

use crate::config::error::ConfigError;
use crate::shared::utils;
use mlua::{Lua, Table, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Config {
    pub source: Source,
    pub prompt: Prompt,
    pub settings: Settings,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Settings {
    pub tz: String,
    pub oauth2_path: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Source {
    pub google: GoogleSource,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Prompt {
    pub template: String,
    pub calendar_ids: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GoogleSource {
    pub oauth2: GoogleOAuth2,
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GoogleOAuth2 {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct AccountConfig {
    pub name: String,
    pub calendar_ids: Vec<String>,
    pub authorize_account: String,
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
    let default_path = format!("{}/.local/share/cal2prompt/oauth2", home_dir);
    let p = PathBuf::from(&default_path);

    Ok(p)
}

fn load_config(config_file_path: &Path) -> anyhow::Result<Config> {
    let lua = Lua::new();
    setup_lua_environment(&lua, config_file_path)?;

    let config_code = fs::read_to_string(config_file_path.to_string_lossy().to_string())?;
    let config_eval = lua.load(&config_code).eval()?;

    if let Value::Table(config_tbl) = config_eval {
        let source = parse_source(&config_tbl, config_file_path, &lua)?;
        let prompt = parse_prompt(&config_tbl, config_file_path, &lua)?;
        let settings = parse_settings(&config_tbl)?;

        Ok(Config {
            source,
            prompt,
            settings,
        })
    } else {
        Err(ConfigError::RequiredFieldNotFound(
            "config.lua did not return a table!".to_owned(),
            utils::path::contract_tilde(config_file_path),
        )
        .into())
    }
}

fn setup_lua_environment(lua: &Lua, config_file_path: &Path) -> anyhow::Result<()> {
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

    Ok(())
}

fn parse_source(config_tbl: &Table, config_file_path: &Path, lua: &Lua) -> anyhow::Result<Source> {
    let source_tbl: Table = config_tbl.get::<Table>("source")?;
    let google_tbl: Table = source_tbl.get::<Table>("google")?;

    let oauth2 = parse_oauth2(&google_tbl, config_file_path, lua)?;
    let accounts = parse_accounts(&google_tbl, config_file_path)?;

    Ok(Source {
        google: GoogleSource { oauth2, accounts },
    })
}

fn parse_oauth2(
    google_tbl: &Table,
    config_file_path: &Path,
    lua: &Lua,
) -> anyhow::Result<GoogleOAuth2> {
    let google_oauth2_tbl: Table = google_tbl.get::<Table>("oauth2")?;

    let client_id: String = google_oauth2_tbl
        .get::<Option<String>>("clientID")?
        .ok_or_else(|| {
            ConfigError::RequiredFieldNotFound(
                "source.google.oauth2.clientID".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
        })?;

    let client_secret: String = google_oauth2_tbl
        .get::<Option<String>>("clientSecret")?
        .ok_or_else(|| {
            ConfigError::RequiredFieldNotFound(
                "source.google.oauth2.clientSecret".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
        })?;

    let default_scopes_table = lua.create_table()?;
    default_scopes_table.push("https://www.googleapis.com/auth/calendar.events")?;

    let scopes: Vec<String> = google_oauth2_tbl
        .get::<Option<Table>>("scopes")?
        .unwrap_or(default_scopes_table)
        .sequence_values()
        .collect::<Result<_, _>>()?;

    let redirect_url: String = google_oauth2_tbl
        .get::<Option<String>>("redirectURL")?
        .unwrap_or("http://127.0.0.1:9004".to_string());

    Ok(GoogleOAuth2 {
        client_id,
        client_secret,
        redirect_url,
        scopes,
    })
}

fn parse_accounts(
    google_tbl: &Table,
    config_file_path: &Path,
) -> anyhow::Result<Vec<AccountConfig>> {
    let mut accounts = Vec::new();

    if let Ok(accounts_tbl) = google_tbl.get::<Table>("accounts") {
        for account_value in accounts_tbl.sequence_values::<Table>().flatten() {
            let name = account_value
                .get::<Option<String>>("name")?
                .ok_or_else(|| {
                    ConfigError::RequiredFieldNotFound(
                        "source.google.accounts[].name".to_owned(),
                        utils::path::contract_tilde(config_file_path),
                    )
                })?;

            let authorize_account = account_value
                .get::<Option<String>>("authorizeAccount")?
                .ok_or_else(|| {
                    ConfigError::RequiredFieldNotFound(
                        "source.google.accounts[].authorizeAccount".to_owned(),
                        utils::path::contract_tilde(config_file_path),
                    )
                })?;

            if let Ok(calendar_ids_tbl) = account_value.get::<Table>("calendarIDs") {
                let calendar_ids: Vec<String> = calendar_ids_tbl
                    .sequence_values()
                    .filter_map(|v| {
                        v.ok()
                            .and_then(|val: mlua::Value| val.as_str().map(|s| s.to_string()))
                    })
                    .collect();

                accounts.push(AccountConfig {
                    name,
                    calendar_ids,
                    authorize_account,
                });
            }
        }
    }

    Ok(accounts)
}

fn parse_prompt(config_tbl: &Table, config_file_path: &Path, lua: &Lua) -> anyhow::Result<Prompt> {
    let prompt_tbl = match config_tbl.get::<Table>("prompt") {
        Ok(tbl) => tbl,
        Err(_) => {
            let default_tbl = lua.create_table()?;
            default_tbl.set("template", crate::config::templates::google::STANDARD)?;
            default_tbl
        }
    };

    let template: String = prompt_tbl
        .get::<Option<String>>("template")?
        .unwrap_or(crate::config::templates::google::STANDARD.to_string());

    let calendar_ids: Vec<String> = prompt_tbl
        .get::<Option<Vec<String>>>("calendarIDs")?
        .ok_or_else(|| {
            ConfigError::RequiredFieldNotFound(
                "prompt.calendarIDs".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
        })?;

    Ok(Prompt {
        template,
        calendar_ids,
    })
}

fn parse_settings(config_tbl: &Table) -> anyhow::Result<Settings> {
    let oauth_default_path = get_oauth_path()?;

    match config_tbl.get::<Option<Table>>("settings") {
        Ok(Some(table)) => {
            let oauth2_path = table
                .get::<Option<String>>("oauth2Path")?
                .unwrap_or(oauth_default_path.to_string_lossy().to_string());
            let tz = table
                .get::<Option<String>>("TZ")?
                .unwrap_or("UTC".to_string());

            Ok(Settings { oauth2_path, tz })
        }
        _ => Ok(Settings {
            oauth2_path: oauth_default_path.to_string_lossy().to_string(),
            tz: "UTC".to_string(),
        }),
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
        clientID = secrets.google.oauth2.clientID,
        clientSecret = secrets.google.oauth2.clientSecret,
        redirectURL = "http://127.0.0.1:9004",
      },
      accounts = {
        {
          name = "work",
          calendarIDs = { "test@example.com" },
          authorizeAccount = "test@example.com"
        },
        {
          name = "private",
          calendarIDs = { "private@example.com" },
          authorizeAccount = "private@example.com"
        }
      }
    },
  },
  prompt = {
    template = cal2prompt.template.google.standard,
    calendarIDs = {
      "test@example.com",
      "private@example.com",
    },
  }
}
"#;
        fs::write(&config_file_path, config_code)?;

        let secrets_code = r#"
local M = {}

M.google = {
  oauth2 = {
    clientID = "test_client_id",
    clientSecret = "test_client_secret",
  },
  work = {
    authorizeAccount = "test@example.com",
    calendarIDs = {
      "test@example.com",
    },
  },
  private = {
    authorizeAccount = "private@example.com",
    calendarIDs = {
      "private@example.com",
    },
  },
  prompt = {
    calendarIDs = {
      "test@example.com",
      "private@example.com",
    },
  }
}

return M
"#;
        fs::write(&secrets_file_path, secrets_code)?;

        let config = load_config(&config_file_path)?;

        let home_dir = env::var("HOME")?;
        let oauth2_path = format!("{}/.local/share/cal2prompt/oauth2", home_dir);
        let tz = "UTC".to_string();

        let accounts = vec![
            AccountConfig {
                name: "work".to_string(),
                calendar_ids: vec!["test@example.com".to_string()],
                authorize_account: "test@example.com".to_string(),
            },
            AccountConfig {
                name: "private".to_string(),
                calendar_ids: vec!["private@example.com".to_string()],
                authorize_account: "private@example.com".to_string(),
            },
        ];

        let expected = Config {
            source: Source {
                google: GoogleSource {
                    oauth2: GoogleOAuth2 {
                        client_id: "test_client_id".to_string(),
                        client_secret: "test_client_secret".to_string(),
                        redirect_url: "http://127.0.0.1:9004".to_string(),
                        scopes: vec!["https://www.googleapis.com/auth/calendar.events".to_string()],
                    },
                    accounts,
                },
            },
            prompt: Prompt {
                template: crate::config::templates::google::STANDARD.to_string(),
                calendar_ids: vec![
                    "test@example.com".to_string(),
                    "private@example.com".to_string(),
                ],
            },
            settings: Settings { oauth2_path, tz },
        };

        assert_eq!(config, expected, "Config should match the expected struct");
        assert_eq!(
            config.source.google.accounts.len(),
            2,
            "Should have 2 accounts"
        );
        assert!(
            config.source.google.accounts.contains(&AccountConfig {
                name: "work".to_string(),
                calendar_ids: vec!["test@example.com".to_string()],
                authorize_account: "test@example.com".to_string(),
            }),
            "Should contain 'work' account"
        );
        assert!(
            config.source.google.accounts.contains(&AccountConfig {
                name: "private".to_string(),
                calendar_ids: vec!["private@example.com".to_string()],
                authorize_account: "private@example.com".to_string(),
            }),
            "Should contain 'private' account"
        );

        Ok(())
    }

    #[test]
    fn test_load_config_default_value() -> anyhow::Result<()> {
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
        clientID = secrets.google.oauth2.clientID,
        clientSecret = secrets.google.oauth2.clientSecret,
      },
      accounts = {
        {
          name = "work",
          calendarIDs = { "test@example.com" },
          authorizeAccount = "test@example.com"
        },
        {
          name = "private",
          calendarIDs = { "private@example.com" },
          authorizeAccount = "private@example.com"
        }
      }
    },
  },
  prompt = {
    calendarIDs = {
      "test@example.com",
    },
  }
}
"#;
        fs::write(&config_file_path, config_code)?;

        let secrets_code = r#"
local M = {}

M.google = {
  oauth2 = {
    clientID = "test_client_id",
    clientSecret = "test_client_secret",
  },
  work = {
    authorizeAccount = "test@example.com",
    calendarIDs = {
      "test@example.com",
    },
  },
  private = {
    authorizeAccount = "private@example.com",
    calendarIDs = {
      "private@example.com",
    },
  },
}

return M
"#;
        fs::write(&secrets_file_path, secrets_code)?;

        let config = load_config(&config_file_path)?;

        let home_dir = env::var("HOME")?;
        let oauth2_path = format!("{}/.local/share/cal2prompt/oauth2", home_dir);
        let tz = "UTC".to_string();

        let accounts = vec![
            AccountConfig {
                name: "work".to_string(),
                calendar_ids: vec!["test@example.com".to_string()],
                authorize_account: "test@example.com".to_string(),
            },
            AccountConfig {
                name: "private".to_string(),
                calendar_ids: vec!["private@example.com".to_string()],
                authorize_account: "private@example.com".to_string(),
            },
        ];

        let expected = Config {
            source: Source {
                google: GoogleSource {
                    oauth2: GoogleOAuth2 {
                        client_id: "test_client_id".to_string(),
                        client_secret: "test_client_secret".to_string(),
                        redirect_url: "http://127.0.0.1:9004".to_string(),
                        scopes: vec!["https://www.googleapis.com/auth/calendar.events".to_string()],
                    },
                    accounts,
                },
            },
            prompt: Prompt {
                template: crate::config::templates::google::STANDARD.to_string(),
                calendar_ids: vec!["test@example.com".to_string()],
            },
            settings: Settings { oauth2_path, tz },
        };

        assert_eq!(config, expected, "Config should match the expected struct");

        Ok(())
    }
}
