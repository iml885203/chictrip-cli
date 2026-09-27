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
- 🗑️ Explicitly confirmed trip deletion
- 📍 Search ChicTrip destination keys
- 🤖 Stdio MCP server for agent integration
- 🛡️ Read-only MCP by default; writes require two separate opt-ins
- 🔄 Automatic access-token refresh
- 🖥️ macOS, Linux, and Windows configuration paths

## Current scope

| Capability | CLI | MCP |
| --- | --- | --- |
| List trips | ✅ | ✅ |
| Read a trip | ✅ | ✅ |
| Search destinations | ✅ | ✅ |
| Create a trip | ✅ | ✅, gated |
| Update name or dates | ✅ | ✅, gated |
| Delete a trip | ✅, `--yes` | ✅, gated |
| Edit daily places/routes | Not yet | Not yet |

## Installation

Rust 1.91.1 or newer and Google Chrome/Chromium are required.

```bash
git clone https://github.com/iml885203/chictrip-cli.git
cd chictrip-cli
cargo install --path . --locked
```

The executable is installed as `chictrip`, normally under `~/.cargo/bin`.

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
```

Trip IDs are returned by `trips list`.

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
- `search_destinations`

To advertise mutation tools, the host must explicitly add `--enable-write`:

```bash
chictrip mcp --enable-write
```

This adds `create_trip`, `update_trip`, and `delete_trip`. Every mutation call
must also contain `confirm: true`; enabling the server flag alone is insufficient.

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
