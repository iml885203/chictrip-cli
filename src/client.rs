use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::auth::{self, Credentials};

const DEFAULT_BASE_URL: &str = "https://api.chictrip.com.tw";

#[derive(Clone)]
pub struct ChicTripClient {
    http: Client,
    base_url: String,
    credentials: Arc<Mutex<Credentials>>,
}

#[derive(Clone)]
enum RequestBody {
    Form(Vec<(String, String)>),
}

impl ChicTripClient {
    pub fn new(credentials: Credentials) -> Result<Self> {
        Self::with_base_url(credentials, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(credentials: Credentials, base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .user_agent("chictrip-cli/0.1.0")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()?,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            credentials: Arc::new(Mutex::new(credentials)),
        })
    }

    pub async fn list_trips(&self) -> Result<Value> {
        self.request(
            Method::GET,
            "/TravelSchedule/GetMyAndCollaboration?updateTime=0&orderByColumn=updatetime&sort=desc",
            None,
        )
        .await
    }

    pub async fn get_trip(&self, id: &str) -> Result<Value> {
        let path = format!(
            "/TravelSchedule/GetWithDetail?travelScheduleId={}&updateTime=0",
            urlencoding::encode(id)
        );
        let trips = self.request(Method::GET, &path, None).await?;
        trips
            .as_array()
            .into_iter()
            .flatten()
            .find(|trip| {
                trip.pointer("/travelScheduleInfo/id")
                    .and_then(Value::as_str)
                    == Some(id)
            })
            .cloned()
            .context("trip was not found in the current user's itineraries")
    }

    pub async fn delete_trip(&self, id: &str) -> Result<Value> {
        self.request_form(
            Method::DELETE,
            "/TravelSchedule/Delete",
            vec![("id".into(), id.into())],
        )
        .await
    }

    pub async fn search_destinations(&self, query: &str) -> Result<Value> {
        let path = format!("/Location/SearchV2?key={}", urlencoding::encode(query));
        self.request(Method::GET, &path, None).await
    }

    pub async fn create_trip(
        &self,
        name: &str,
        start: NaiveDate,
        end: NaiveDate,
        location_keys: &[String],
    ) -> Result<Value> {
        validate_dates(start, end)?;
        if location_keys.is_empty() {
            bail!("at least one destination location key is required");
        }
        let (cover, label) = tokio::try_join!(self.default_cover_id(), self.default_label_id())?;
        let mut form = vec![
            ("CoverMediaId".into(), cover),
            ("Name".into(), name.into()),
            ("StartDate".into(), start.format("%Y/%m/%d").to_string()),
            ("EndDate".into(), end.format("%Y/%m/%d").to_string()),
            (
                "TotalDay".into(),
                ((end - start).num_days() + 1).to_string(),
            ),
            ("ViewMode".into(), "DetailMode".into()),
            ("TravelScheduleUserLabelId".into(), label),
            ("id".into(), String::new()),
            ("TrafficType".into(), "Custom".into()),
            ("IsForceUpdateTsdRoute".into(), "0".into()),
            ("updateTime".into(), "0".into()),
        ];
        append_array(
            &mut form,
            "LocationKey",
            location_keys.iter().map(String::as_str),
        );
        self.request_form(Method::POST, "/TravelSchedule/AddV2", form)
            .await
    }

    pub async fn update_trip(
        &self,
        id: &str,
        name: Option<&str>,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> Result<Value> {
        let detail = self.get_trip(id).await?;
        let info = detail
            .get("travelScheduleInfo")
            .context("trip response has no travelScheduleInfo")?;
        let current_start = parse_api_date(info, "startDate")?;
        let current_end = parse_api_date(info, "endDate")?;
        let start = start.unwrap_or(current_start);
        let end = end.unwrap_or(current_end);
        validate_dates(start, end)?;
        let current_update_time = api_scalar_string(info, "updateTime")?;
        let verified = self.verify_update_time(id, &current_update_time).await?;
        let update_time = api_scalar_string(&verified, "updateTime")?;
        let location_keys: Vec<String> = info
            .get("destinationList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|destination| {
                destination
                    .get("locationKey")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect();
        let mut form = vec![
            (
                "CoverMediaId".into(),
                required_api_string(info, "coverMediaId")?.into(),
            ),
            (
                "Name".into(),
                name.unwrap_or(required_api_string(info, "name")?).into(),
            ),
            ("StartDate".into(), start.format("%Y/%m/%d").to_string()),
            ("EndDate".into(), end.format("%Y/%m/%d").to_string()),
            (
                "TotalDay".into(),
                ((end - start).num_days() + 1).to_string(),
            ),
            ("ViewMode".into(), "DetailMode".into()),
            (
                "TravelScheduleUserLabelId".into(),
                required_api_string(info, "userLabelId")?.into(),
            ),
            ("id".into(), id.into()),
            (
                "TrafficType".into(),
                required_api_string(info, "trafficType")?.into(),
            ),
            ("IsForceUpdateTsdRoute".into(), "0".into()),
            ("updateTime".into(), update_time),
        ];
        form.extend(
            location_keys
                .iter()
                .map(|value| ("LocationKey[]".into(), value.clone())),
        );
        self.request_form(Method::PUT, "/TravelSchedule/UpdateV3", form)
            .await
    }

    pub async fn verify(&self) -> Result<()> {
        self.list_trips().await.map(|_| ())
    }

    async fn verify_update_time(&self, id: &str, update_time: &str) -> Result<Value> {
        let path = format!(
            "/TravelScheduleDetail/VerifyUpdateTime?TravelScheduleId={}&travelScheduleUpdateTime={}",
            urlencoding::encode(id),
            urlencoding::encode(update_time)
        );
        self.request(Method::GET, &path, None).await
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<RequestBody>,
    ) -> Result<Value> {
        let first = self.send(method.clone(), path, body.clone()).await?;
        if api_status(&first) == Some("003") {
            self.refresh().await?;
            return self.send(method, path, body).await.and_then(check_api);
        }
        check_api(first)
    }

    async fn request_form(
        &self,
        method: Method,
        path: &str,
        form: Vec<(String, String)>,
    ) -> Result<Value> {
        self.request(method, path, Some(RequestBody::Form(form)))
            .await
    }

    async fn send(&self, method: Method, path: &str, body: Option<RequestBody>) -> Result<Value> {
        let token = self.credentials.lock().await.access_token.clone();
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(token)
            .header("osType", "web")
            .header("language", "zhtw")
            .header("version", "2.0.38");
        if let Some(body) = body {
            request = match body {
                RequestBody::Form(fields) => request.form(&fields),
            };
        }
        let response = request.send().await.context("ChicTrip request failed")?;
        if response.status() != StatusCode::OK {
            bail!("ChicTrip returned HTTP {}", response.status());
        }
        response
            .json()
            .await
            .context("ChicTrip returned invalid JSON")
    }

    async fn refresh(&self) -> Result<()> {
        let current = self.credentials.lock().await.clone();
        let response: Value = self
            .http
            .post(format!("{}/Token/Refresh", self.base_url))
            .json(&json!({
                "refreshToken": current.refresh_token,
                "memberId": current.member_id,
            }))
            .send()
            .await
            .context("could not refresh ChicTrip login")?
            .json()
            .await
            .context("ChicTrip returned an invalid refresh response")?;
        let status = api_status(&response);
        if status != Some("001") {
            bail!("ChicTrip login expired; run `chictrip login` again");
        }
        let data = response
            .get("data")
            .context("refresh response has no data")?;
        let updated = Credentials {
            access_token: string_field(data, "accessToken")?,
            refresh_token: string_field(data, "refreshToken")?,
            member_id: string_field(data, "memberId")?,
        };
        auth::save(&updated)?;
        *self.credentials.lock().await = updated;
        Ok(())
    }

    async fn default_cover_id(&self) -> Result<String> {
        let covers = self
            .request(Method::GET, "/TravelSchedule/GetSystemCoverList", None)
            .await?;
        first_string_field(&covers, "id").context("ChicTrip returned no system cover")
    }

    async fn default_label_id(&self) -> Result<String> {
        let labels = self
            .request(Method::GET, "/TravelScheduleUserLabel/Get", None)
            .await?;
        let list = labels
            .as_array()
            .context("ChicTrip returned invalid labels")?;
        list.iter()
            .find(|label| label.get("isSystem").and_then(Value::as_bool) == Some(true))
            .or_else(|| list.first())
            .and_then(|label| label.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .context("ChicTrip returned no itinerary label")
    }
}

fn api_status(value: &Value) -> Option<&str> {
    value
        .get("apiStatus")
        .or_else(|| value.get("ApiStatus"))
        .and_then(Value::as_str)
}

fn check_api(value: Value) -> Result<Value> {
    match api_status(&value) {
        Some("001") => Ok(value.get("data").cloned().unwrap_or(Value::Null)),
        Some(status) => bail!(
            "ChicTrip API error {status}: {}",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ),
        None => bail!("ChicTrip response did not contain apiStatus"),
    }
}

fn string_field(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .with_context(|| format!("refresh response has no {field}"))
}

fn required_api_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("trip response has no {field}"))
}

fn api_scalar_string(value: &Value, field: &str) -> Result<String> {
    match value.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        _ => bail!("trip response has no {field}"),
    }
}

fn first_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .as_array()?
        .first()?
        .get(field)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn parse_api_date(value: &Value, field: &str) -> Result<NaiveDate> {
    let raw = required_api_string(value, field)?;
    ["%Y/%m/%d", "%Y-%m-%d"]
        .into_iter()
        .find_map(|format| NaiveDate::parse_from_str(raw, format).ok())
        .with_context(|| format!("trip response contains an invalid {field}"))
}

fn validate_dates(start: NaiveDate, end: NaiveDate) -> Result<()> {
    let days = (end - start).num_days() + 1;
    if !(1..=60).contains(&days) {
        bail!("an itinerary must be between 1 and 60 days");
    }
    Ok(())
}

fn append_array<'a>(
    form: &mut Vec<(String, String)>,
    field: &str,
    values: impl IntoIterator<Item = &'a str>,
) {
    for (index, value) in values.into_iter().enumerate() {
        form.push((format!("{field}[{index}]"), value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;
    use serde_json::json;

    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            member_id: "member".into(),
        }
    }

    #[tokio::test]
    async fn lists_private_trips_with_bearer_auth() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetMyAndCollaboration")
                .query_param("updateTime", "0")
                .header("authorization", "Bearer access");
            then.status(200)
                .json_body(json!({ "apiStatus": "001", "data": [{ "name": "Taipei" }] }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let trips = client.list_trips().await.unwrap();

        mock.assert();
        assert_eq!(trips[0]["name"], "Taipei");
    }

    #[tokio::test]
    async fn reports_api_errors_without_returning_untrusted_data() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET);
            then.status(200).json_body(
                json!({ "apiStatus": "002", "message": "denied", "data": { "secret": true } }),
            );
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let error = client.list_trips().await.unwrap_err().to_string();

        assert!(error.contains("API error 002"));
        assert!(!error.contains("secret"));
    }

    #[tokio::test]
    async fn creates_trip_as_the_form_contract_used_by_the_web_app() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/TravelSchedule/GetSystemCoverList");
            then.status(200)
                .json_body(json!({ "apiStatus": "001", "data": [{ "id": "cover-1" }] }));
        });
        server.mock(|when, then| {
            when.method(GET).path("/TravelScheduleUserLabel/Get");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{ "id": "label-1", "isSystem": true }]
            }));
        });
        let create = server.mock(|when, then| {
            when.method(POST)
                .path("/TravelSchedule/AddV2")
                .header("content-type", "application/x-www-form-urlencoded")
                .body_matches("(^|&)Name=Taipei(&|$)")
                .body_matches("(^|&)StartDate=2026%2F10%2F01(&|$)")
                .body_matches("(^|&)LocationKey%5B0%5D=tw-tpe(&|$)");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "id": "trip-1", "name": "Taipei" }
            }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let created = client
            .create_trip(
                "Taipei",
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
                &["tw-tpe".into()],
            )
            .await
            .unwrap();

        create.assert();
        assert_eq!(created["id"], "trip-1");
    }

    #[tokio::test]
    async fn updates_from_latest_trip_state_and_preserves_existing_fields() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetMyAndCollaboration");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{ "id": "trip-1", "updateTime": 123456 }]
            }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetWithDetail")
                .query_param("travelScheduleId", "trip-1")
                .query_param("updateTime", "0");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{
                    "travelScheduleInfo": {
                        "id": "trip-1",
                        "name": "Old name",
                        "startDate": "2026/10/01",
                        "endDate": "2026/10/02",
                        "coverMediaId": "cover-1",
                        "userLabelId": "label-1",
                        "trafficType": "Custom",
                        "updateTime": 123456,
                        "destinationList": [{ "locationKey": "tw-tpe" }]
                    }
                }]
            }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelScheduleDetail/VerifyUpdateTime")
                .query_param("TravelScheduleId", "trip-1")
                .query_param("travelScheduleUpdateTime", "123456");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "updateTime": 123457 }
            }));
        });
        let update = server.mock(|when, then| {
            when.method(PUT)
                .path("/TravelSchedule/UpdateV3")
                .header("content-type", "application/x-www-form-urlencoded")
                .body_matches(r"(^|&)Name=New\+name(&|$)")
                .body_matches("(^|&)updateTime=123457(&|$)")
                .body_matches("(^|&)LocationKey%5B%5D=tw-tpe(&|$)");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "id": "trip-1", "updateTime": 123457 }
            }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let updated = client
            .update_trip("trip-1", Some("New name"), None, None)
            .await
            .unwrap();

        update.assert();
        assert_eq!(updated["updateTime"], 123457);
    }

    #[test]
    fn rejects_invalid_trip_lengths() {
        let start = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert!(validate_dates(start, end).is_err());

        let end = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        assert!(validate_dates(start, end).is_err());
    }
}
