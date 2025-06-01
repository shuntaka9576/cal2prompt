pub mod error;
pub mod templates;

use crate::config::error::ConfigError;
use crate::shared::utils;
use anyhow::Context;
use chrono::prelude::*;
use chrono_tz::Tz;
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
    pub mcp: Mcp,
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

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Mcp {
    pub insert_event: InsertEvent,
    pub get_events: GetEvents,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct InsertEvent {
    pub target: Vec<Target>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Target {
    pub nickname: String,
    pub calendar_id: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GetEvents {
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
        let mcp = parse_mcp(&config_tbl, config_file_path)?;

        Ok(Config {
            source,
            prompt,
            settings,
            mcp,
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
    let source_tbl = config_tbl.get("source")?;
    let source_tbl: Table = match source_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "source".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let google_tbl = source_tbl.get("google")?;
    let google_tbl: Table = match google_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "source.google".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

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
    let google_oauth2_tbl = google_tbl.get("oauth2")?;
    let google_oauth2_tbl: Table = match google_oauth2_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "source.google.oauth2".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let client_id = google_oauth2_tbl.get("clientID")?;
    let client_id: String = match client_id {
        Value::String(s) => s.to_str()?.to_string(),
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "source.google.oauth2.clientID".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let client_secret = google_oauth2_tbl.get("clientSecret")?;
    let client_secret: String = match client_secret {
        Value::String(s) => s.to_str()?.to_string(),
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "source.google.oauth2.clientSecret".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let default_scopes_table = lua.create_table()?;
    default_scopes_table.push("https://www.googleapis.com/auth/calendar.events")?;

    let scopes = google_oauth2_tbl.get("scopes")?;
    let scopes: Table = match scopes {
        Value::Table(tbl) => tbl,
        _ => default_scopes_table,
    };

    let mut scopes_vec = Vec::new();
    for i in 1..=scopes.len()? {
        let scope = scopes.get(i)?;
        if let Value::String(s) = scope {
            scopes_vec.push(s.to_str()?.to_string());
        }
    }

    let redirect_url = google_oauth2_tbl.get("redirectURL")?;
    let redirect_url: String = match redirect_url {
        Value::String(s) => s.to_str()?.to_string(),
        _ => "http://127.0.0.1:9004".to_string(),
    };

    Ok(GoogleOAuth2 {
        client_id,
        client_secret,
        redirect_url,
        scopes: scopes_vec,
    })
}

fn parse_accounts(
    google_tbl: &Table,
    config_file_path: &Path,
) -> anyhow::Result<Vec<AccountConfig>> {
    let mut accounts = Vec::new();

    let accounts_tbl = google_tbl.get("accounts")?;
    if let Value::Table(accounts_tbl) = accounts_tbl {
        for i in 1..=accounts_tbl.len()? {
            let account_value = accounts_tbl.get(i)?;
            if let Value::Table(account_value) = account_value {
                let name = account_value.get("name")?;
                let name: String = match name {
                    Value::String(s) => s.to_str()?.to_string(),
                    _ => {
                        return Err(ConfigError::RequiredFieldNotFound(
                            "source.google.accounts[].name".to_owned(),
                            utils::path::contract_tilde(config_file_path),
                        )
                        .into());
                    }
                };

                let authorize_account = account_value.get("authorizeAccount")?;
                let authorize_account: String = match authorize_account {
                    Value::String(s) => s.to_str()?.to_string(),
                    _ => {
                        return Err(ConfigError::RequiredFieldNotFound(
                            "source.google.accounts[].authorizeAccount".to_owned(),
                            utils::path::contract_tilde(config_file_path),
                        )
                        .into());
                    }
                };

                let calendar_ids_tbl = account_value.get("calendarIDs")?;
                let calendar_ids_tbl: Table = match calendar_ids_tbl {
                    Value::Table(tbl) => tbl,
                    _ => continue,
                };

                let mut calendar_ids = Vec::new();
                for i in 1..=calendar_ids_tbl.len()? {
                    let id = calendar_ids_tbl.get(i)?;
                    if let Value::String(id) = id {
                        calendar_ids.push(id.to_str()?.to_string());
                    }
                }

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
    let prompt_tbl = config_tbl.get("prompt")?;
    let prompt_tbl: Table = match prompt_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            let default_tbl = lua.create_table()?;
            default_tbl.set("template", crate::config::templates::google::STANDARD)?;
            default_tbl
        }
    };

    let template = prompt_tbl.get("template")?;
    let template: String = match template {
        Value::String(s) => s.to_str()?.to_string(),
        _ => crate::config::templates::google::STANDARD.to_string(),
    };

    let calendar_ids_tbl = prompt_tbl.get("calendarIDs")?;
    let calendar_ids_tbl: Table = match calendar_ids_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "prompt.calendarIDs".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let mut calendar_ids = Vec::new();
    for i in 1..=calendar_ids_tbl.len()? {
        let id = calendar_ids_tbl.get(i)?;
        if let Value::String(id) = id {
            calendar_ids.push(id.to_str()?.to_string());
        }
    }

    Ok(Prompt {
        template,
        calendar_ids,
    })
}

fn parse_settings(config_tbl: &Table) -> anyhow::Result<Settings> {
    let oauth_default_path = get_oauth_path()?;

    let settings_tbl = config_tbl.get("settings")?;
    let settings_tbl: Table = match settings_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Ok(Settings {
                oauth2_path: oauth_default_path.to_string_lossy().to_string(),
                tz: "UTC".to_string(),
            });
        }
    };

    let oauth2_path = settings_tbl.get("oauth2Path")?;
    let oauth2_path: String = match oauth2_path {
        Value::String(s) => s.to_str()?.to_string(),
        _ => oauth_default_path.to_string_lossy().to_string(),
    };

    let tz = settings_tbl.get("TZ")?;
    let tz: String = match tz {
        Value::String(s) => s.to_str()?.to_string(),
        _ => "UTC".to_string(),
    };

    Ok(Settings { oauth2_path, tz })
}

fn parse_mcp(config_tbl: &Table, config_file_path: &Path) -> anyhow::Result<Mcp> {
    let mcp_tbl = config_tbl.get("mcp")?;
    let mcp_tbl: Table = match mcp_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "mcp".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let insert_event_tbl = mcp_tbl.get("insertEvent")?;
    let insert_event_tbl: Table = match insert_event_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "mcp.insertEvent".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let target_tbl = insert_event_tbl.get("target")?;
    let target_tbl: Table = match target_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "mcp.insertEvent.target".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let mut targets = Vec::new();
    for i in 1..=target_tbl.len()? {
        let item = target_tbl.get(i)?;
        if let Value::Table(item) = item {
            let nickname = item.get("nickname")?;
            let nickname: String = match nickname {
                Value::String(s) => s.to_str()?.to_string(),
                _ => continue,
            };

            let calendar_id = item.get("calendarID")?;
            let calendar_id: String = match calendar_id {
                Value::String(s) => s.to_str()?.to_string(),
                _ => continue,
            };

            targets.push(Target {
                nickname,
                calendar_id,
            });
        }
    }

    let get_events_tbl = mcp_tbl.get("getEvents")?;
    let get_events_tbl: Table = match get_events_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "mcp.getEvents".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let calendar_ids_tbl = get_events_tbl.get("calendarIDs")?;
    let calendar_ids_tbl: Table = match calendar_ids_tbl {
        Value::Table(tbl) => tbl,
        _ => {
            return Err(ConfigError::RequiredFieldNotFound(
                "mcp.getEvents.calendarIDs".to_owned(),
                utils::path::contract_tilde(config_file_path),
            )
            .into());
        }
    };

    let mut calendar_ids = Vec::new();
    for i in 1..=calendar_ids_tbl.len()? {
        let id = calendar_ids_tbl.get(i)?;
        if let Value::String(id) = id {
            calendar_ids.push(id.to_str()?.to_string());
        }
    }

    Ok(Mcp {
        insert_event: InsertEvent { target: targets },
        get_events: GetEvents { calendar_ids },
    })
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
          authorizeAccount = "test@example.com",
          calendarIDs = {
            "test@example.com",
          },
        },
        {
          name = "private",
          authorizeAccount = "private@example.com",
          calendarIDs = {
            "private@example.com",
          },
        },
      },
    },
  },
  prompt = {
    calendarIDs = {
      "test@example.com",
      "private@example.com",
    },
  },
  mcp = {
    insertEvent = {
      target = {
        {
          nickname = "work",
          calendarID = "test@example.com",
        },
        {
          nickname = "private",
          calendarID = "private@example.com",
        },
      },
    },
    getEvents = {
      calendarIDs = {
        "test@example.com",
        "private@example.com",
      },
    },
  },
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
                calendar_ids: vec![
                    "test@example.com".to_string(),
                    "private@example.com".to_string(),
                ],
            },
            settings: Settings { oauth2_path, tz },
            mcp: Mcp {
                insert_event: InsertEvent {
                    target: vec![
                        Target {
                            nickname: "work".to_string(),
                            calendar_id: "test@example.com".to_string(),
                        },
                        Target {
                            nickname: "private".to_string(),
                            calendar_id: "private@example.com".to_string(),
                        },
                    ],
                },
                get_events: GetEvents {
                    calendar_ids: vec![
                        "test@example.com".to_string(),
                        "private@example.com".to_string(),
                    ],
                },
            },
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
          authorizeAccount = "test@example.com",
          calendarIDs = {
            "test@example.com",
          },
        },
        {
          name = "private",
          authorizeAccount = "private@example.com",
          calendarIDs = {
            "private@example.com",
          },
        },
      },
    },
  },
  prompt = {
    calendarIDs = {
      "test@example.com",
      "private@example.com",
    },
  },
  mcp = {
    insertEvent = {
      target = {
        {
          nickname = "work",
          calendarID = "test@example.com",
        },
        {
          nickname = "private",
          calendarID = "private@example.com",
        },
      },
    },
    getEvents = {
      calendarIDs = {
        "test@example.com",
        "private@example.com",
      },
    },
  },
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
                calendar_ids: vec![
                    "test@example.com".to_string(),
                    "private@example.com".to_string(),
                ],
            },
            settings: Settings { oauth2_path, tz },
            mcp: Mcp {
                insert_event: InsertEvent {
                    target: vec![
                        Target {
                            nickname: "work".to_string(),
                            calendar_id: "test@example.com".to_string(),
                        },
                        Target {
                            nickname: "private".to_string(),
                            calendar_id: "private@example.com".to_string(),
                        },
                    ],
                },
                get_events: GetEvents {
                    calendar_ids: vec![
                        "test@example.com".to_string(),
                        "private@example.com".to_string(),
                    ],
                },
            },
        };

        assert_eq!(config, expected, "Config should match the expected struct");

        Ok(())
    }
}
