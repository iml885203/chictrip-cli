# ChicTrip CLI

> Unofficial command-line client and MCP server for [去趣 ChicTrip](https://www.chictrip.com.tw/). Not affiliated with Hotai Connected or the ChicTrip team.

Manage your personal ChicTrip itineraries from a terminal or an AI agent. The
CLI supports browser-assisted phone/OTP login, JSON output, trip CRUD, destination
search, and a safety-gated Model Context Protocol server.

> [!WARNING]
> This project uses ChicTrip's undocumented web API. Endpoints and payloads may
> change without notice. Keep a backup of important itinerary data and review
> write operations before confirming them.

## Features

- 🔐 Official browser login flow — phone numbers, OTPs, and passwords stay in ChicTrip's pages
- 📋 List and inspect private itineraries
- ✍️ Create and update trip metadata
- 📝 Read and edit itinerary-level notes
- 🗑️ Explicitly confirmed trip deletion
- 📍 Search ChicTrip destination keys
- 🗺️ Search and append points of interest to numbered trip days
- 🕐 Set custom arrival/departure times, stay duration, and item notes
- ↕️ Delete, reorder, and move daily itinerary items
- 🚆 Inspect and select ChicTrip routes, or record custom travel time and notes
- ✈️ Record native flight segments between airport itinerary items
- 🤖 Stdio MCP server for agent integration
- 🛡️ Read-only MCP by default; writes require two separate opt-ins
- 🔄 Automatic access-token refresh
- 🖥️ macOS, Linux, and Windows configuration paths

## Current scope

| Capability | CLI | MCP |
| --- | --- | --- |
| List trips | ✅ | ✅ |
| Read a trip | ✅ | ✅ |
| Read and edit trip notes | ✅ | ✅, gated |
| Search destinations | ✅ | ✅ |
| Create a trip | ✅ | ✅, gated |
| Update name or dates | ✅ | ✅, gated |
| Delete a trip | ✅, `--yes` | ✅, gated |
| Search and add daily places | ✅ | ✅, gated |
| Edit place time/name/note | ✅ | ✅, gated |
| Delete, reorder, and move daily places | ✅ | ✅, gated |
| Configure routes | ✅ | ✅, gated |
| Configure flight segments | ✅ | ✅, gated |

## Installation

Google Chrome or Chromium is required for browser login. Rust is not required
when installing a prebuilt release.

### macOS and Linux

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/iml885203/chictrip-cli/main/install.sh | sh
```

This installs `chictrip` to `~/.local/bin`. Set `CHICTRIP_INSTALL_DIR` to use a
different location.

### Windows

Download `chictrip-x86_64-pc-windows-msvc.zip` from the
[latest release](https://github.com/iml885203/chictrip-cli/releases/latest),
extract `chictrip.exe`, and place it in a directory on `PATH`.

### Build from source

Developers with Rust 1.91.1 or newer can install from source:

```bash
git clone https://github.com/iml885203/chictrip-cli.git
cd chictrip-cli
cargo install --path . --locked
```

The source-built executable is installed as `chictrip`, normally under
`~/.cargo/bin`.

## Login

```bash
chictrip login --browser
```

The CLI opens a dedicated Chrome profile. Complete ChicTrip's official phone/OTP
or social login in that window. The CLI reads only the resulting ChicTrip session
after the browser returns to `chictrip.com.tw`; it does not receive your phone
number, OTP, or password.

The dedicated profile is isolated from everyday browsing but persists so future
logins can reuse the official-site session. Check or clear the local login with:

```bash
chictrip status
chictrip logout
```

If Chrome cannot be launched, tokens can be imported manually:

```bash
chictrip login --manual
```

## Usage

### Read trips

All command results are emitted as JSON for scripting and agent use.

```bash
chictrip trips list
chictrip trips show <trip-id>
chictrip trips show-note <trip-id>
```

Trip IDs are returned by `trips list`.

### Build a daily itinerary

Search near the trip region, then use the returned POI ID:

```bash
chictrip pois search "櫛田神社" --latitude 33.59 --longitude 130.40

chictrip trips add-poi <trip-id> \
  --day 1 \
  --poi <poi-id> \
  --yes
```

Set the item's custom time, duration, display name, or note using the item ID
returned by `trips show`:

```bash
chictrip trips update-item <trip-id> <item-id> \
  --arrival 13:00 \
  --departure 14:00 \
  --stay-minutes 60 \
  --yes

chictrip trips note-item <trip-id> <item-id> \
  --note "Reserve two weeks ahead" \
  --yes

# The --item sequence must contain every item on that day exactly once.
chictrip trips reorder-day <trip-id> --day 1 \
  --item <first-item-id> --item <second-item-id> \
  --yes

chictrip trips delete-item <trip-id> <item-id> --day 1 --yes
chictrip trips move-item <trip-id> <item-id> \
  --from-day 1 --to-day 2 --yes
```

Inspect ChicTrip's transit, driving, walking, or scooter route choices for the
segment arriving at an item, then select one of the returned route IDs:

```bash
chictrip trips routes <trip-id> <item-id> --day 1 --type Transit
chictrip trips set-route <trip-id> <item-id> --day 1 \
  --route <poi-route-detail-id> --yes

chictrip trips set-custom-route <trip-id> <item-id> --day 1 \
  --duration-minutes 40 --note "九州橫斷巴士" --yes

chictrip trips set-flight-route <trip-id> <arrival-airport-item-id> --day 1 \
  --duration-minutes 140 --note "IT240 TPE 06:15 → FUK 09:35" --yes
```

### Find a destination

Creating a trip requires at least one ChicTrip location key:

```bash
chictrip destinations search 台北
```

### Create and update a trip

```bash
chictrip trips create \
  --name "Taipei weekend" \
  --start 2026-10-01 \
  --end 2026-10-02 \
  --destination <location-key>

chictrip trips update <trip-id> --name "Taipei long weekend"
chictrip trips update <trip-id> --start 2026-10-01 --end 2026-10-03
chictrip trips update-note <trip-id> --note "Backup plans and reminders" --yes
```

Dates use `YYYY-MM-DD`. ChicTrip currently limits an itinerary to 60 days.

### Delete a trip

Deletion is irreversible and fails unless `--yes` is present:

```bash
chictrip trips delete <trip-id> --yes
```

## MCP server

Start the stdio MCP server in its default read-only mode:

```bash
chictrip mcp
```

Read-only tools:

- `list_trips`
- `get_trip`
- `get_trip_note`
- `search_destinations`
- `search_pois`
- `list_trip_item_routes`

To advertise mutation tools, the host must explicitly add `--enable-write`:

```bash
chictrip mcp --enable-write
```

This adds trip and itinerary mutations: create/update/delete trips; add,
update, annotate, delete, and reorder daily items; and select either a ChicTrip
route or a custom travel segment. Every mutation call must also contain
`confirm: true`; enabling the server flag alone is insufficient.

Example MCP host configuration:

```json
{
  "mcpServers": {
    "chictrip": {
      "command": "chictrip",
      "args": ["mcp"]
    }
  }
}
```

Use `"args": ["mcp", "--enable-write"]` only when the agent should be allowed
to propose confirmed changes.

## Credential storage

Credentials are stored outside the repository in the platform user configuration
directory:

- macOS: `~/Library/Application Support/chictrip/credentials.json`
- Linux: `$XDG_CONFIG_HOME/chictrip/credentials.json` (usually `~/.config/chictrip/credentials.json`)
- Windows: the user's roaming configuration directory under `chictrip/credentials.json`

On Unix, the credential file is forced to mode `0600`. It is not encrypted, so
protect your OS account and backups. `chictrip logout` removes both the credential
file and the dedicated browser profile.

For ephemeral environments or CI, set all three variables instead of writing a
credential file:

```bash
export CHICTRIP_ACCESS_TOKEN="..."
export CHICTRIP_REFRESH_TOKEN="..."
export CHICTRIP_MEMBER_ID="..."
```

`CHICTRIP_CONFIG_DIR` overrides the configuration directory, and
`CHICTRIP_CHROME` can point to a Chrome/Chromium executable.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The tests use mock HTTP servers and do not require a ChicTrip account. Live API
verification is intentionally not part of CI.

## Security and privacy

- Never commit `credentials.json`, browser profiles, exported trip JSON, or `.env` files.
- Prefer the default read-only MCP server unless writes are necessary.
- Treat CLI JSON output as private because it may contain itinerary details.
- The API is unofficial; verify changes in the ChicTrip app after important writes.

Please report security issues privately to the repository owner instead of
opening an issue containing credentials or itinerary data.

## License

[MIT](LICENSE)
