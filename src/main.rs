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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.version {
        print!("{}", APP_VERSION);
        std::process::exit(0);
    }

    match &cli.command {
        Some(cmd) => match cmd {
            Commands::Mcp => {
                // For MCP mode, initialize without OAuth to allow proper error handling via JSON-RPC
                match init_cal2prompt_without_oauth().await {
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
            }
        },
        None => {
            // For CLI mode, initialize with OAuth as before
            let cal2prompt = match init_cal2prompt().await {
                Ok(cal2prompt) => cal2prompt,
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            };

            let fetch_mode = determine_duration_or_range(&cli);

            match fetch_mode {
                FetchMode::Shortcut(duration) => {
                    match cal2prompt.get_events_short_cut(duration).await {
                        Ok(generate) => {
                            println!("{}", generate);
                        }
                        Err(err) => {
                            eprint!("{:?}", err);
                        }
                    }
                }
                FetchMode::Range(since, until) => {
                    match cal2prompt.get_events_duration(since, until).await {
                        Ok(generate) => {
                            println!("{}", generate);
                        }
                        Err(err) => {
                            eprint!("{:?}", err);
                        }
                    }
                }
            }
        }
    };
}

async fn init_cal2prompt() -> anyhow::Result<Cal2Prompt> {
    let mut cal2prompt = Cal2Prompt::new()?;
    let _ = cal2prompt.oauth().await.map_err(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });

    Ok(cal2prompt)
}

async fn init_cal2prompt_without_oauth() -> anyhow::Result<Cal2Prompt> {
    // Initialize Cal2Prompt without performing OAuth
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
