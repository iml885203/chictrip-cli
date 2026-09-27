mod auth;
mod browser_login;
mod client;
mod mcp;

use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use serde_json::to_string_pretty;

use auth::Credentials;
use client::ChicTripClient;

#[derive(Parser)]
#[command(
    name = "chictrip",
    version,
    about = "Unofficial ChicTrip CLI and MCP server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in through Chrome and save the session in the user config directory.
    Login {
        /// Complete the official phone/OTP login in an isolated Chrome window.
        #[arg(long, conflicts_with = "manual")]
        browser: bool,
        /// Manually import tokens from browser local storage.
        #[arg(long, conflicts_with = "browser")]
        manual: bool,
    },
    /// Verify the saved session.
    Status,
    /// Remove the saved session.
    Logout,
    /// Work with personal itineraries.
    Trips {
        #[command(subcommand)]
        command: TripsCommand,
    },
    /// Find destination keys used when creating an itinerary.
    Destinations {
        #[command(subcommand)]
        command: DestinationsCommand,
    },
    /// Run a stdio MCP server (read-only unless explicitly enabled).
    Mcp {
        #[arg(long)]
        enable_write: bool,
    },
}

#[derive(Subcommand)]
enum TripsCommand {
    List,
    Show {
        id: String,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = parse_date)]
        start: NaiveDate,
        #[arg(long, value_parser = parse_date)]
        end: NaiveDate,
        #[arg(long = "destination", required = true)]
        destinations: Vec<String>,
    },
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = parse_date)]
        start: Option<NaiveDate>,
        #[arg(long, value_parser = parse_date)]
        end: Option<NaiveDate>,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DestinationsCommand {
    Search { query: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Login { browser, manual } => login(browser, manual).await,
        Command::Status => {
            client()?.verify().await?;
            println!("Logged in to ChicTrip.");
            Ok(())
        }
        Command::Logout => {
            auth::delete()?;
            browser_login::delete_profile()?;
            println!(
                "Logged out; stored credentials and the dedicated browser profile were removed."
            );
            Ok(())
        }
        Command::Trips { command } => trips(command).await,
        Command::Destinations { command } => destinations(command).await,
        Command::Mcp { enable_write } => mcp::serve(client()?, enable_write).await,
    }
}

async fn login(browser: bool, manual: bool) -> Result<()> {
    if browser || !manual {
        let credentials = browser_login::login().await?;
        let candidate = ChicTripClient::new(credentials.clone())?;
        candidate
            .verify()
            .await
            .context("the browser login session could not be verified")?;
        auth::save(&credentials)?;
        println!("Login verified and saved in the user configuration directory.");
        return Ok(());
    }
    eprintln!("Sign in at https://www.chictrip.com.tw/ first.");
    eprintln!(
        "Then open browser developer tools → Application → Local Storage and copy the three requested values."
    );
    let credentials = Credentials {
        access_token: rpassword::prompt_password("accessToken: ")?,
        refresh_token: rpassword::prompt_password("refreshToken: ")?,
        member_id: prompt("memberId: ")?,
    };
    auth::validate(&credentials)?;
    let candidate = ChicTripClient::new(credentials.clone())?;
    candidate
        .verify()
        .await
        .context("the imported session could not be verified")?;
    auth::save(&credentials)?;
    println!("Login verified and saved in the user configuration directory.");
    Ok(())
}

async fn trips(command: TripsCommand) -> Result<()> {
    let client = client()?;
    let value = match command {
        TripsCommand::List => client.list_trips().await?,
        TripsCommand::Show { id } => client.get_trip(&id).await?,
        TripsCommand::Create {
            name,
            start,
            end,
            destinations,
        } => client.create_trip(&name, start, end, &destinations).await?,
        TripsCommand::Update {
            id,
            name,
            start,
            end,
        } => {
            if name.is_none() && start.is_none() && end.is_none() {
                bail!("provide at least one of --name, --start, or --end");
            }
            client.update_trip(&id, name.as_deref(), start, end).await?
        }
        TripsCommand::Delete { id, yes } => {
            if !yes {
                bail!("deletion is irreversible; repeat with --yes");
            }
            client.delete_trip(&id).await?
        }
    };
    println!("{}", to_string_pretty(&value)?);
    Ok(())
}

async fn destinations(command: DestinationsCommand) -> Result<()> {
    let value = match command {
        DestinationsCommand::Search { query } => client()?.search_destinations(&query).await?,
    };
    println!("{}", to_string_pretty(&value)?);
    Ok(())
}

fn client() -> Result<ChicTripClient> {
    ChicTripClient::new(auth::load()?)
}

fn prompt(label: &str) -> Result<String> {
    eprint!("{label}");
    io::stderr().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_owned())
}

fn parse_date(value: &str) -> std::result::Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| "expected a date in YYYY-MM-DD format".to_owned())
}
