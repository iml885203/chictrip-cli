mod auth;
mod browser_login;
mod client;
mod mcp;

use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
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
    /// Search ChicTrip points of interest.
    Pois {
        #[command(subcommand)]
        command: PoisCommand,
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
    ShowNote {
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
    UpdateNote {
        id: String,
        #[arg(long)]
        note: String,
        #[arg(long)]
        yes: bool,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    AddPoi {
        id: String,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        poi: String,
        #[arg(long)]
        yes: bool,
    },
    UpdateItem {
        id: String,
        item_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = parse_time)]
        arrival: Option<NaiveTime>,
        #[arg(long, value_parser = parse_time)]
        departure: Option<NaiveTime>,
        #[arg(long)]
        stay_minutes: Option<u32>,
        #[arg(long)]
        yes: bool,
    },
    NoteItem {
        id: String,
        item_id: String,
        #[arg(long)]
        note: String,
        #[arg(long)]
        yes: bool,
    },
    DeleteItem {
        id: String,
        item_id: String,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        yes: bool,
    },
    ReorderDay {
        id: String,
        #[arg(long)]
        day: u32,
        #[arg(long = "item", required = true)]
        item_ids: Vec<String>,
        #[arg(long)]
        yes: bool,
    },
    MoveItem {
        id: String,
        item_id: String,
        #[arg(long)]
        from_day: u32,
        #[arg(long)]
        to_day: u32,
        #[arg(long)]
        yes: bool,
    },
    Routes {
        id: String,
        item_id: String,
        #[arg(long)]
        day: u32,
        #[arg(long = "type", default_value = "Transit")]
        traffic_type: String,
    },
    SetRoute {
        id: String,
        item_id: String,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        route: String,
        #[arg(long)]
        yes: bool,
    },
    SetCustomRoute {
        id: String,
        item_id: String,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        duration_minutes: u32,
        #[arg(long, default_value = "")]
        note: String,
        #[arg(long)]
        yes: bool,
    },
    SetFlightRoute {
        id: String,
        item_id: String,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        duration_minutes: u32,
        #[arg(long)]
        note: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DestinationsCommand {
    Search { query: String },
}

#[derive(Subcommand)]
enum PoisCommand {
    Search {
        query: String,
        #[arg(long, default_value_t = 33.2)]
        latitude: f64,
        #[arg(long, default_value_t = 130.7)]
        longitude: f64,
    },
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
        Command::Pois { command } => pois(command).await,
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
        TripsCommand::ShowNote { id } => client.get_trip_note(&id).await?,
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
        TripsCommand::UpdateNote { id, note, yes } => {
            require_yes(yes)?;
            client.update_trip_note(&id, &note).await?
        }
        TripsCommand::Delete { id, yes } => {
            if !yes {
                bail!("deletion is irreversible; repeat with --yes");
            }
            client.delete_trip(&id).await?
        }
        TripsCommand::AddPoi { id, day, poi, yes } => {
            require_yes(yes)?;
            client.add_trip_poi(&id, day, &poi).await?
        }
        TripsCommand::UpdateItem {
            id,
            item_id,
            name,
            arrival,
            departure,
            stay_minutes,
            yes,
        } => {
            require_yes(yes)?;
            if name.is_none() && arrival.is_none() && departure.is_none() && stay_minutes.is_none()
            {
                bail!("provide at least one of --name, --arrival, --departure, or --stay-minutes");
            }
            client
                .update_trip_item(
                    &id,
                    &item_id,
                    name.as_deref(),
                    arrival,
                    departure,
                    stay_minutes,
                )
                .await?
        }
        TripsCommand::NoteItem {
            id,
            item_id,
            note,
            yes,
        } => {
            require_yes(yes)?;
            client.update_trip_item_note(&id, &item_id, &note).await?
        }
        TripsCommand::DeleteItem {
            id,
            item_id,
            day,
            yes,
        } => {
            require_yes(yes)?;
            client.delete_trip_item(&id, day, &item_id).await?
        }
        TripsCommand::ReorderDay {
            id,
            day,
            item_ids,
            yes,
        } => {
            require_yes(yes)?;
            client.reorder_trip_day(&id, day, &item_ids).await?
        }
        TripsCommand::MoveItem {
            id,
            item_id,
            from_day,
            to_day,
            yes,
        } => {
            require_yes(yes)?;
            client
                .move_trip_item(&id, from_day, to_day, &item_id)
                .await?
        }
        TripsCommand::Routes {
            id,
            item_id,
            day,
            traffic_type,
        } => {
            client
                .list_trip_item_routes(&id, day, &item_id, &traffic_type)
                .await?
        }
        TripsCommand::SetRoute {
            id,
            item_id,
            day,
            route,
            yes,
        } => {
            require_yes(yes)?;
            client
                .set_trip_item_route(&id, day, &item_id, &route)
                .await?
        }
        TripsCommand::SetCustomRoute {
            id,
            item_id,
            day,
            duration_minutes,
            note,
            yes,
        } => {
            require_yes(yes)?;
            client
                .set_trip_item_custom_route(&id, day, &item_id, duration_minutes, &note)
                .await?
        }
        TripsCommand::SetFlightRoute {
            id,
            item_id,
            day,
            duration_minutes,
            note,
            yes,
        } => {
            require_yes(yes)?;
            client
                .set_trip_item_flight_route(&id, day, &item_id, duration_minutes, &note)
                .await?
        }
    };
    println!("{}", to_string_pretty(&value)?);
    Ok(())
}

async fn pois(command: PoisCommand) -> Result<()> {
    let value = match command {
        PoisCommand::Search {
            query,
            latitude,
            longitude,
        } => client()?.search_pois(&query, latitude, longitude).await?,
    };
    println!("{}", to_string_pretty(&value)?);
    Ok(())
}

fn require_yes(yes: bool) -> Result<()> {
    if !yes {
        bail!("this changes an itinerary; repeat with --yes");
    }
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

fn parse_time(value: &str) -> std::result::Result<NaiveTime, String> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| "expected a time in HH:MM format".to_owned())
}
