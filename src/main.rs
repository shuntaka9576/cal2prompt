mod config;
mod core;
mod google;
mod mcp;
mod shared;

use clap::{Parser, Subcommand};
use core::cal2prompt::{Cal2Prompt, GetEventDuration};

const APP_VERSION: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " version ",
    env!("CARGO_PKG_VERSION"),
    " (rev:",
    env!("GIT_HASH"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "cal2prompt",
    version = "0.1.0",
    author = "shuntaka9576(@shuntaka_dev)",
    about = "✨ Fetches your schedule (e.g., from Google Calendar) and converts it into a single LLM prompt. It can also run as an MCP (Model Context Protocol) server.",
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(
        long,
        value_name = "DATE",
        requires = "until",
        help = "Start date (YYYY-MM-DD). Requires --until."
    )]
    pub since: Option<String>,
    #[arg(
        long,
        value_name = "DATE",
        requires = "since",
        help = "End date (YYYY-MM-DD). Requires --since."
    )]
    pub until: Option<String>,
    #[arg(long, help = "Fetch events for today only.")]
    pub today: bool,
    #[arg(long, help = "Fetch events for the current week (Mon-Sun).")]
    pub this_week: bool,
    #[arg(long, help = "Fetch events for the current month (1st - end).")]
    pub this_month: bool,
    #[arg(long, help = "Fetch events for the upcoming week (Mon-Sun).")]
    pub next_week: bool,
    #[arg(long, short = 'V', help = "Print version")]
    pub version: bool,
}

enum FetchMode {
    Shortcut(GetEventDuration),
    Range(String, String),
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(
        name = "mcp",
        about = "Launch cal2prompt as an MCP server (experimental)"
    )]
    Mcp,
}

fn main() {
    let cli = Cli::parse();

    if cli.version {
        print!("{}", APP_VERSION);
        std::process::exit(0);
    }

    match &cli.command {
        Some(cmd) => match cmd {
            Commands::Mcp => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    match init_cal2prompt_mcp().await {
                        Ok(mut cal2prompt) => {
                            if let Err(err) = cal2prompt.launch_mcp().await {
                                eprintln!("Error: {:?}", err);
                                std::process::exit(1);
                            }
                        }
                        Err(e) => {
                            eprintln!("{}", e);
                            std::process::exit(1);
                        }
                    }
                });
            }
        },
        None => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let cal2prompt = match init_cal2prompt_cli().await {
                    Ok(cal2prompt) => cal2prompt,
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                };

                let fetch_mode = determine_duration_or_range(&cli);
                // TODO: accounts loop request
                let calendar_ids = cal2prompt.config.prompt.calendar_ids.clone();

                match fetch_mode {
                    FetchMode::Shortcut(duration) => {
                        match cal2prompt.fetch_duration(duration).await {
                            Ok(output) => {
                                println!("{}", output);
                            }
                            Err(e) => {
                                eprintln!("{}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                    FetchMode::Range(since, until) => {
                        match cal2prompt.fetch_days(&since, &until, None).await {
                            Ok(days) => match cal2prompt.render_days(days) {
                                Ok(output) => {
                                    println!("{}", output);
                                }
                                Err(e) => {
                                    eprintln!("{}", e);
                                    std::process::exit(1);
                                }
                            },
                            Err(e) => {
                                eprintln!("{}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                }
            });
        }
    }
}

async fn init_cal2prompt_cli() -> anyhow::Result<Cal2Prompt> {
    let mut cal2prompt = Cal2Prompt::new()?;

    let accounts = cal2prompt
        .get_accounts()?
        .into_iter()
        .map(|account| account.account_name)
        .collect::<Vec<String>>();

    for name in accounts {
        if let Err(e) = cal2prompt.oauth(Some(name)).await {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    Ok(cal2prompt)
}

async fn init_cal2prompt_mcp() -> anyhow::Result<Cal2Prompt> {
    Cal2Prompt::new()
}

fn determine_duration_or_range(cli: &Cli) -> FetchMode {
    if let (Some(since), Some(until)) = (&cli.since, &cli.until) {
        FetchMode::Range(since.clone(), until.clone())
    } else if cli.today {
        FetchMode::Shortcut(GetEventDuration::Today)
    } else if cli.this_week {
        FetchMode::Shortcut(GetEventDuration::ThisWeek)
    } else if cli.this_month {
        FetchMode::Shortcut(GetEventDuration::ThisMonth)
    } else if cli.next_week {
        FetchMode::Shortcut(GetEventDuration::NextWeek)
    } else {
        FetchMode::Shortcut(GetEventDuration::Today)
    }
}
