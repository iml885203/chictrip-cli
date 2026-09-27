use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
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
            "serverInfo": { "name": "chictrip", "version": env!("CARGO_PKG_VERSION") }
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
        json!({ "name": "get_trip_note", "description": "Get the itinerary-level note for a private ChicTrip trip.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] } }),
        json!({ "name": "search_destinations", "description": "Search ChicTrip destinations and return location keys used to create trips", "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "minLength": 1 } }, "required": ["query"] } }),
        json!({ "name": "search_pois", "description": "Search ChicTrip points of interest. Results include POI IDs used by add_trip_poi.", "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "minLength": 1 }, "latitude": { "type": "number", "default": 33.2 }, "longitude": { "type": "number", "default": 130.7 } }, "required": ["query"] } }),
        json!({ "name": "list_trip_item_routes", "description": "List ChicTrip route choices for the segment arriving at an itinerary item.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "item_id": { "type": "string" }, "traffic_type": { "type": "string", "enum": ["Driving", "TwoWheeler", "Transit", "Walking"], "default": "Transit" } }, "required": ["trip_id", "day", "item_id"] } }),
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
        tools.push(json!({ "name": "update_trip_note", "description": "Replace the itinerary-level note for a private ChicTrip trip.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "note": { "type": "string" }, "confirm": { "const": true } }, "required": ["id", "note", "confirm"] } }));
        tools.push(json!({ "name": "add_trip_poi", "description": "Append a ChicTrip POI to a numbered itinerary day. Search with search_pois first.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "poi_id": { "type": "string" }, "confirm": { "const": true } }, "required": ["trip_id", "day", "poi_id", "confirm"] } }));
        tools.push(json!({ "name": "update_trip_item", "description": "Set a trip item's display name, arrival/departure time, or stay duration.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "item_id": { "type": "string" }, "name": { "type": "string", "minLength": 1 }, "arrival": { "type": "string", "pattern": "^[0-2][0-9]:[0-5][0-9]$" }, "departure": { "type": "string", "pattern": "^[0-2][0-9]:[0-5][0-9]$" }, "stay_minutes": { "type": "integer", "minimum": 0 }, "confirm": { "const": true } }, "required": ["trip_id", "item_id", "confirm"], "anyOf": [{"required":["name"]},{"required":["arrival"]},{"required":["departure"]},{"required":["stay_minutes"]}] } }));
        tools.push(json!({ "name": "update_trip_item_note", "description": "Replace the note attached to a trip item.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "item_id": { "type": "string" }, "note": { "type": "string" }, "confirm": { "const": true } }, "required": ["trip_id", "item_id", "note", "confirm"] } }));
        tools.push(json!({ "name": "delete_trip_item", "description": "Permanently remove an item from a numbered itinerary day.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "item_id": { "type": "string" }, "confirm": { "const": true } }, "required": ["trip_id", "day", "item_id", "confirm"] } }));
        tools.push(json!({ "name": "reorder_trip_day", "description": "Replace the order of a day's items. item_ids must contain every current item ID exactly once, in the desired order.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "item_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "uniqueItems": true }, "confirm": { "const": true } }, "required": ["trip_id", "day", "item_ids", "confirm"] } }));
        tools.push(json!({ "name": "move_trip_item", "description": "Move an itinerary item to another day. ChicTrip performs this as a copy followed by deletion of the original.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "from_day": { "type": "integer", "minimum": 1 }, "to_day": { "type": "integer", "minimum": 1 }, "item_id": { "type": "string" }, "confirm": { "const": true } }, "required": ["trip_id", "from_day", "to_day", "item_id", "confirm"] } }));
        tools.push(json!({ "name": "set_trip_item_route", "description": "Select a ChicTrip route option for the segment arriving at an item. Obtain poi_route_detail_id from list_trip_item_routes.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "item_id": { "type": "string" }, "poi_route_detail_id": { "type": "string" }, "confirm": { "const": true } }, "required": ["trip_id", "day", "item_id", "poi_route_detail_id", "confirm"] } }));
        tools.push(json!({ "name": "set_trip_item_custom_route", "description": "Set custom travel duration and note for the segment arriving at an item.", "inputSchema": { "type": "object", "properties": { "trip_id": { "type": "string" }, "day": { "type": "integer", "minimum": 1 }, "item_id": { "type": "string" }, "duration_minutes": { "type": "integer", "minimum": 0 }, "note": { "type": "string", "default": "" }, "confirm": { "const": true } }, "required": ["trip_id", "day", "item_id", "duration_minutes", "confirm"] } }));
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
        "get_trip_note" => client.get_trip_note(required_string(&args, "id")?).await?,
        "search_destinations" => {
            client
                .search_destinations(required_string(&args, "query")?)
                .await?
        }
        "search_pois" => {
            client
                .search_pois(
                    required_string(&args, "query")?,
                    optional_f64(&args, "latitude").unwrap_or(33.2),
                    optional_f64(&args, "longitude").unwrap_or(130.7),
                )
                .await?
        }
        "list_trip_item_routes" => {
            client
                .list_trip_item_routes(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    required_string(&args, "item_id")?,
                    optional_string(&args, "traffic_type")?.unwrap_or("Transit"),
                )
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
        "update_trip_note" if enable_write => {
            require_confirmation(&args)?;
            client
                .update_trip_note(
                    required_string(&args, "id")?,
                    required_string_allow_empty(&args, "note")?,
                )
                .await?
        }
        "update_trip_note" => bail!("write tools are disabled; restart with --enable-write"),
        "add_trip_poi" if enable_write => {
            require_confirmation(&args)?;
            client
                .add_trip_poi(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    required_string(&args, "poi_id")?,
                )
                .await?
        }
        "update_trip_item" if enable_write => {
            require_confirmation(&args)?;
            let name = optional_string(&args, "name")?;
            let arrival = optional_time(&args, "arrival")?;
            let departure = optional_time(&args, "departure")?;
            let stay = optional_u32(&args, "stay_minutes")?;
            if name.is_none() && arrival.is_none() && departure.is_none() && stay.is_none() {
                bail!("provide at least one editable item field");
            }
            client
                .update_trip_item(
                    required_string(&args, "trip_id")?,
                    required_string(&args, "item_id")?,
                    name,
                    arrival,
                    departure,
                    stay,
                )
                .await?
        }
        "update_trip_item_note" if enable_write => {
            require_confirmation(&args)?;
            client
                .update_trip_item_note(
                    required_string(&args, "trip_id")?,
                    required_string(&args, "item_id")?,
                    required_string_allow_empty(&args, "note")?,
                )
                .await?
        }
        "delete_trip_item" if enable_write => {
            require_confirmation(&args)?;
            client
                .delete_trip_item(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    required_string(&args, "item_id")?,
                )
                .await?
        }
        "reorder_trip_day" if enable_write => {
            require_confirmation(&args)?;
            client
                .reorder_trip_day(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    &required_string_array(&args, "item_ids")?,
                )
                .await?
        }
        "move_trip_item" if enable_write => {
            require_confirmation(&args)?;
            client
                .move_trip_item(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "from_day")?,
                    required_u32(&args, "to_day")?,
                    required_string(&args, "item_id")?,
                )
                .await?
        }
        "set_trip_item_route" if enable_write => {
            require_confirmation(&args)?;
            client
                .set_trip_item_route(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    required_string(&args, "item_id")?,
                    required_string(&args, "poi_route_detail_id")?,
                )
                .await?
        }
        "set_trip_item_custom_route" if enable_write => {
            require_confirmation(&args)?;
            client
                .set_trip_item_custom_route(
                    required_string(&args, "trip_id")?,
                    required_u32(&args, "day")?,
                    required_string(&args, "item_id")?,
                    required_u32(&args, "duration_minutes")?,
                    args.get("note").and_then(Value::as_str).unwrap_or_default(),
                )
                .await?
        }
        "add_trip_poi"
        | "update_trip_item"
        | "update_trip_item_note"
        | "delete_trip_item"
        | "reorder_trip_day"
        | "move_trip_item"
        | "set_trip_item_route"
        | "set_trip_item_custom_route" => {
            bail!("write tools are disabled; restart with --enable-write")
        }
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

fn required_string_allow_empty<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("{field} must be a string"))
}

fn optional_f64(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

fn required_u32(value: &Value, field: &str) -> Result<u32> {
    optional_u32(value, field)?.with_context(|| format!("{field} is required"))
}

fn optional_u32(value: &Value, field: &str) -> Result<Option<u32>> {
    value
        .get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .with_context(|| format!("{field} must be a non-negative integer"))
        })
        .transpose()
}

fn optional_time(value: &Value, field: &str) -> Result<Option<NaiveTime>> {
    optional_string(value, field)?
        .map(|value| {
            NaiveTime::parse_from_str(value, "%H:%M")
                .with_context(|| format!("{field} must use HH:MM"))
        })
        .transpose()
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

        assert_eq!(
            names,
            vec![
                "list_trips",
                "get_trip",
                "get_trip_note",
                "search_destinations",
                "search_pois",
                "list_trip_item_routes"
            ]
        );
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

        for name in [
            "create_trip",
            "update_trip",
            "delete_trip",
            "update_trip_note",
            "add_trip_poi",
            "update_trip_item",
            "update_trip_item_note",
            "delete_trip_item",
            "reorder_trip_day",
            "move_trip_item",
            "set_trip_item_route",
            "set_trip_item_custom_route",
        ] {
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
