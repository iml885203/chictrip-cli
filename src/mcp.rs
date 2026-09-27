use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::ChicTripClient;

pub async fn serve(client: ChicTripClient, enable_write: bool) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).context("invalid MCP JSON-RPC request")?;
        if let Some(response) = handle(&client, enable_write, request).await {
            stdout
                .write_all(serde_json::to_string(&response)?.as_bytes())
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

async fn handle(client: &ChicTripClient, enable_write: bool, request: Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "chictrip", "version": "0.1.0" }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(enable_write) })),
        "tools/call" => {
            call_tool(
                client,
                enable_write,
                request.get("params").cloned().unwrap_or_default(),
            )
            .await
        }
        _ => Err(anyhow::anyhow!("method not found: {method}")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": error.to_string() } })
        }
    })
}

fn tools(enable_write: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({ "name": "list_trips", "description": "List the authenticated user's ChicTrip trips", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "get_trip", "description": "Get a private ChicTrip itinerary", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] } }),
        json!({ "name": "search_destinations", "description": "Search ChicTrip destinations and return location keys used to create trips", "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "minLength": 1 } }, "required": ["query"] } }),
    ];
    if enable_write {
        tools.push(json!({
            "name": "create_trip",
            "description": "Create a private ChicTrip itinerary. Use search_destinations first to obtain location keys.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "start": { "type": "string", "format": "date" },
                    "end": { "type": "string", "format": "date" },
                    "destination_keys": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "confirm": { "const": true }
                },
                "required": ["name", "start", "end", "destination_keys", "confirm"]
            }
        }));
        tools.push(json!({
            "name": "update_trip",
            "description": "Update a private ChicTrip itinerary after reading its latest updateTime.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string", "minLength": 1 },
                    "start": { "type": "string", "format": "date" },
                    "end": { "type": "string", "format": "date" },
                    "confirm": { "const": true }
                },
                "required": ["id", "confirm"],
                "anyOf": [{ "required": ["name"] }, { "required": ["start"] }, { "required": ["end"] }]
            }
        }));
        tools.push(json!({ "name": "delete_trip", "description": "Permanently delete a ChicTrip itinerary", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "confirm": { "const": true } }, "required": ["id", "confirm"] } }));
    }
    tools
}

async fn call_tool(client: &ChicTripClient, enable_write: bool, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("tool name is required")?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let data = match name {
        "list_trips" => client.list_trips().await?,
        "get_trip" => client.get_trip(required_string(&args, "id")?).await?,
        "search_destinations" => {
            client
                .search_destinations(required_string(&args, "query")?)
                .await?
        }
        "create_trip" if enable_write => {
            require_confirmation(&args)?;
            let destinations = required_string_array(&args, "destination_keys")?;
            client
                .create_trip(
                    required_string(&args, "name")?,
                    required_date(&args, "start")?,
                    required_date(&args, "end")?,
                    &destinations,
                )
                .await?
        }
        "update_trip" if enable_write => {
            require_confirmation(&args)?;
            let name = optional_string(&args, "name")?;
            let start = optional_date(&args, "start")?;
            let end = optional_date(&args, "end")?;
            if name.is_none() && start.is_none() && end.is_none() {
                bail!("provide at least one of name, start, or end");
            }
            client
                .update_trip(required_string(&args, "id")?, name, start, end)
                .await?
        }
        "create_trip" | "update_trip" => {
            bail!("write tools are disabled; restart with --enable-write")
        }
        "delete_trip" if enable_write => {
            require_confirmation(&args)?;
            client.delete_trip(required_string(&args, "id")?).await?
        }
        "delete_trip" => bail!("write tools are disabled; restart with --enable-write"),
        _ => bail!("unknown tool: {name}"),
    };
    Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&data)? }] }))
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("{field} is required"))
}

fn optional_string<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value)),
        Some(_) => bail!("{field} must be a non-empty string"),
    }
}

fn required_date(value: &Value, field: &str) -> Result<NaiveDate> {
    parse_date(required_string(value, field)?, field)
}

fn optional_date(value: &Value, field: &str) -> Result<Option<NaiveDate>> {
    optional_string(value, field)?
        .map(|value| parse_date(value, field))
        .transpose()
}

fn parse_date(value: &str, field: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("{field} must use YYYY-MM-DD"))
}

fn required_string_array(value: &Value, field: &str) -> Result<Vec<String>> {
    let values = value
        .get(field)
        .and_then(Value::as_array)
        .with_context(|| format!("{field} must be an array"))?;
    if values.is_empty() {
        bail!("{field} must not be empty");
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .with_context(|| format!("{field} values must be non-empty strings"))
        })
        .collect()
}

fn require_confirmation(value: &Value) -> Result<()> {
    if value.get("confirm").and_then(Value::as_bool) != Some(true) {
        bail!("write operation requires confirm=true");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::auth::Credentials;

    fn client() -> ChicTripClient {
        ChicTripClient::with_base_url(
            Credentials {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                member_id: "member".into(),
            },
            "http://127.0.0.1:1",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn read_only_server_does_not_advertise_write_tools() {
        let response = handle(
            &client(),
            false,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await
        .unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert_eq!(names, vec!["list_trips", "get_trip", "search_destinations"]);
    }

    #[tokio::test]
    async fn write_server_advertises_confirmed_trip_mutations() {
        let response = handle(
            &client(),
            true,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await
        .unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();

        for name in ["create_trip", "update_trip", "delete_trip"] {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            assert!(
                tool["inputSchema"]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("confirm"))
            );
        }
    }

    #[tokio::test]
    async fn write_calls_fail_closed_without_confirmation() {
        let response = handle(
            &client(),
            true,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_trip",
                    "arguments": { "id": "trip-1" }
                }
            }),
        )
        .await
        .unwrap();

        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("confirm=true")
        );
    }
}
