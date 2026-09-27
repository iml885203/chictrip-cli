use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
use reqwest::{Client, Method, StatusCode, multipart};
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
    Multipart(Vec<(String, String)>),
}

impl ChicTripClient {
    pub fn new(credentials: Credentials) -> Result<Self> {
        Self::with_base_url(credentials, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(credentials: Credentials, base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .user_agent(concat!("chictrip-cli/", env!("CARGO_PKG_VERSION")))
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

    pub async fn get_trip_note(&self, id: &str) -> Result<Value> {
        let detail = self.get_trip(id).await?;
        let path = format!(
            "/TravelSchedule/GetNote?id={}&updateTime={}",
            urlencoding::encode(id),
            urlencoding::encode(&trip_update_time(&detail)?)
        );
        self.request(Method::GET, &path, None).await
    }

    pub async fn update_trip_note(&self, id: &str, note: &str) -> Result<Value> {
        let detail = self.get_trip(id).await?;
        self.request_multipart(
            Method::PUT,
            "/TravelSchedule/UpdateNote",
            vec![
                ("id".into(), id.into()),
                ("note".into(), note.into()),
                ("updateTime".into(), trip_update_time(&detail)?),
            ],
        )
        .await
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

    pub async fn search_pois(&self, query: &str, latitude: f64, longitude: f64) -> Result<Value> {
        let path = format!(
            "/PoiSearch/SearchByKeyword?keyword={}&centerLongitude={longitude}&centerLatitude={latitude}",
            urlencoding::encode(query)
        );
        self.request(Method::GET, &path, None).await
    }

    pub async fn add_trip_poi(&self, trip_id: &str, day: u32, poi_id: &str) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        validate_day(&detail, day)?;
        let info = detail
            .get("travelScheduleInfo")
            .context("trip response has no travelScheduleInfo")?;
        let update_time = api_scalar_string(info, "updateTime")?;
        let poi = self.get_poi(poi_id).await?;
        let path = format!(
            "/TravelScheduleDetail/GetAddWhere?poiId={}&travelScheduleId={}&travelScheduleUpdateTime=0",
            urlencoding::encode(poi_id),
            urlencoding::encode(trip_id)
        );
        let positions = self.request(Method::GET, &path, None).await?;
        let day_entry = positions
            .get("dayList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| entry.get("day").and_then(Value::as_u64) == Some(day.into()))
            .context("ChicTrip returned no insertion positions for that day")?;
        let add_where_id = day_entry
            .get("addWhereList")
            .and_then(Value::as_array)
            .and_then(|positions| positions.last())
            .and_then(|position| position.get("addWhereId"))
            .and_then(Value::as_str)
            .context("ChicTrip returned no insertion position")?;
        let cover_id = poi
            .pointer("/cover/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.request_multipart(
            Method::POST,
            "/TravelScheduleDetail/Add",
            vec![
                ("TravelScheduleId".into(), trip_id.into()),
                ("Day".into(), day.to_string()),
                ("PoiId".into(), poi_id.into()),
                ("AddWhereId".into(), add_where_id.into()),
                ("TravelScheduleUpdateTime".into(), update_time),
                ("TsdCoverMediaId".into(), cover_id.into()),
                ("TsdName".into(), required_api_string(&poi, "name")?.into()),
            ],
        )
        .await
    }

    pub async fn update_trip_item(
        &self,
        trip_id: &str,
        item_id: &str,
        name: Option<&str>,
        arrival: Option<NaiveTime>,
        departure: Option<NaiveTime>,
        stay_minutes: Option<u32>,
    ) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let info = detail
            .get("travelScheduleInfo")
            .context("trip response has no travelScheduleInfo")?;
        let update_time = api_scalar_string(info, "updateTime")?;
        let path = format!(
            "/TravelScheduleDetail/GetEditInfo?TsdId={}&TravelScheduleId={}&TravelScheduleUpdateTime={}",
            urlencoding::encode(item_id),
            urlencoding::encode(trip_id),
            urlencoding::encode(&update_time)
        );
        let edit = self.request(Method::GET, &path, None).await?;
        let stay = stay_minutes
            .unwrap_or_else(|| edit.get("stayTime").and_then(Value::as_u64).unwrap_or(60) as u32);
        let mut fields = vec![
            ("TsdId".into(), item_id.into()),
            (
                "Name".into(),
                name.unwrap_or(required_api_string(&edit, "name")?).into(),
            ),
            (
                "PoiClassificationId".into(),
                required_api_string(&edit, "poiClassificationId")?.into(),
            ),
            ("StayTime".into(), stay.to_string()),
            ("TravelScheduleId".into(), trip_id.into()),
            ("travelScheduleUpdateTime".into(), update_time),
        ];
        append_custom_time(&mut fields, "Arrival", arrival, &edit);
        append_custom_time(&mut fields, "Departure", departure, &edit);
        self.request_multipart(Method::PUT, "/TravelScheduleDetail/Update", fields)
            .await
    }

    pub async fn update_trip_item_note(
        &self,
        trip_id: &str,
        item_id: &str,
        note: &str,
    ) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let update_time = api_scalar_string(
            detail
                .get("travelScheduleInfo")
                .context("trip response has no travelScheduleInfo")?,
            "updateTime",
        )?;
        self.request_multipart(
            Method::PUT,
            "/TravelScheduleDetail/UpdateNote",
            vec![
                ("TravelScheduleId".into(), trip_id.into()),
                ("Note".into(), note.into()),
                ("TsdId".into(), item_id.into()),
                ("TravelScheduleUpdateTime".into(), update_time),
            ],
        )
        .await
    }

    pub async fn delete_trip_item(&self, trip_id: &str, day: u32, item_id: &str) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let item = trip_item(&detail, day, item_id)?;
        let update_time = trip_update_time(&detail)?;
        self.request_form(
            Method::DELETE,
            "/TravelScheduleDetail/Delete",
            vec![
                ("TravelScheduleId".into(), trip_id.into()),
                ("Day".into(), day.to_string()),
                ("TsdId".into(), required_api_string(item, "id")?.into()),
                ("TravelScheduleUpdateTime".into(), update_time),
            ],
        )
        .await
    }

    pub async fn reorder_trip_day(
        &self,
        trip_id: &str,
        day: u32,
        item_ids: &[String],
    ) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let current = trip_day(&detail, day)?
            .get("tsdList")
            .and_then(Value::as_array)
            .context("trip day has no tsdList")?;
        let current_ids: Vec<&str> = current
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .collect();
        validate_reorder(&current_ids, item_ids)?;
        let mut fields = vec![
            ("TravelScheduleId".into(), trip_id.into()),
            ("MoveOutDay".into(), day.to_string()),
            ("MoveInDay".into(), day.to_string()),
            (
                "MoveTsdId".into(),
                item_ids
                    .first()
                    .context("item_ids must not be empty")?
                    .clone(),
            ),
            (
                "travelScheduleUpdateTime".into(),
                trip_update_time(&detail)?,
            ),
        ];
        fields.extend(item_ids.iter().map(|id| ("TsdIdList[]".into(), id.clone())));
        self.request_multipart(Method::PUT, "/TravelScheduleDetail/Sort", fields)
            .await
    }

    pub async fn move_trip_item(
        &self,
        trip_id: &str,
        from_day: u32,
        to_day: u32,
        item_id: &str,
    ) -> Result<Value> {
        if from_day == to_day {
            bail!("from_day and to_day must be different; use reorder for the same day");
        }
        let detail = self.get_trip(trip_id).await?;
        validate_day(&detail, to_day)?;
        let item = trip_item(&detail, from_day, item_id)?;
        let copied = self
            .request_multipart(
                Method::POST,
                "/TravelScheduleDetail/Copy",
                vec![
                    ("TravelScheduleId".into(), trip_id.into()),
                    ("CopyDay".into(), to_day.to_string()),
                    ("CopyTsdId".into(), item_id.into()),
                    ("StayTime".into(), api_scalar_string(item, "stayTime")?),
                    (
                        "ArrivalTrafficType".into(),
                        required_api_string(item, "arrivalTrafficType")?.into(),
                    ),
                    (
                        "TravelScheduleUpdateTime".into(),
                        trip_update_time(&detail)?,
                    ),
                ],
            )
            .await?;
        let update_time = api_scalar_string(&copied, "travelScheduleUpdateTime").context(
            "item was copied, but ChicTrip returned no update time for removing the original",
        )?;
        self.request_form(
            Method::DELETE,
            "/TravelScheduleDetail/Delete",
            vec![
                ("TravelScheduleId".into(), trip_id.into()),
                ("Day".into(), from_day.to_string()),
                ("TsdId".into(), item_id.into()),
                ("TravelScheduleUpdateTime".into(), update_time),
            ],
        )
        .await
        .context("item was copied to the new day, but removing the original failed")
    }

    pub async fn list_trip_item_routes(
        &self,
        trip_id: &str,
        day: u32,
        item_id: &str,
        traffic_type: &str,
    ) -> Result<Value> {
        validate_traffic_type(traffic_type)?;
        let detail = self.get_trip(trip_id).await?;
        let item = trip_item(&detail, day, item_id)?;
        let route_id = required_api_string(item, "tsdRouteDetailId")?;
        let path = format!(
            "/TravelScheduleDetailRoute/GetRouteList?tsdRouteDetailId={}&trafficType={}&travelScheduleId={}&TravelScheduleUpdateTime={}",
            urlencoding::encode(route_id),
            urlencoding::encode(traffic_type),
            urlencoding::encode(trip_id),
            urlencoding::encode(&trip_update_time(&detail)?)
        );
        self.request(Method::GET, &path, None).await
    }

    pub async fn set_trip_item_route(
        &self,
        trip_id: &str,
        day: u32,
        item_id: &str,
        poi_route_detail_id: &str,
    ) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let item = trip_item(&detail, day, item_id)?;
        self.request_form(
            Method::PUT,
            "/TravelScheduleDetail/SetRoute",
            vec![
                (
                    "TsdRouteDetailId".into(),
                    required_api_string(item, "tsdRouteDetailId")?.into(),
                ),
                ("PoiRouteDetailId".into(), poi_route_detail_id.into()),
                ("TravelScheduleId".into(), trip_id.into()),
                (
                    "travelScheduleUpdateTime".into(),
                    trip_update_time(&detail)?,
                ),
            ],
        )
        .await
    }

    pub async fn set_trip_item_custom_route(
        &self,
        trip_id: &str,
        day: u32,
        item_id: &str,
        duration_minutes: u32,
        note: &str,
    ) -> Result<Value> {
        let detail = self.get_trip(trip_id).await?;
        let item = trip_item(&detail, day, item_id)?;
        self.request_form(
            Method::PUT,
            "/TravelScheduleDetail/SetCustomRoute",
            vec![
                (
                    "TsdRouteDetailId".into(),
                    required_api_string(item, "tsdRouteDetailId")?.into(),
                ),
                ("Duration".into(), duration_minutes.to_string()),
                ("Note".into(), note.into()),
                ("TravelScheduleId".into(), trip_id.into()),
                (
                    "travelScheduleUpdateTime".into(),
                    trip_update_time(&detail)?,
                ),
            ],
        )
        .await
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

    async fn request_multipart(
        &self,
        method: Method,
        path: &str,
        fields: Vec<(String, String)>,
    ) -> Result<Value> {
        self.request(method, path, Some(RequestBody::Multipart(fields)))
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
                RequestBody::Multipart(fields) => {
                    let form = fields
                        .into_iter()
                        .fold(multipart::Form::new(), |form, (name, value)| {
                            form.text(name, value)
                        });
                    request.multipart(form)
                }
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

    async fn get_poi(&self, id: &str) -> Result<Value> {
        let path = format!("/Poi/GetPoiById?id={}", urlencoding::encode(id));
        self.request(Method::GET, &path, None).await
    }
}

fn validate_day(detail: &Value, day: u32) -> Result<()> {
    let total = detail
        .get("dayList")
        .and_then(Value::as_array)
        .map(Vec::len)
        .context("trip response has no dayList")?;
    if day == 0 || day as usize > total {
        bail!("day must be between 1 and {total}");
    }
    Ok(())
}

fn trip_update_time(detail: &Value) -> Result<String> {
    api_scalar_string(
        detail
            .get("travelScheduleInfo")
            .context("trip response has no travelScheduleInfo")?,
        "updateTime",
    )
}

fn trip_day(detail: &Value, day: u32) -> Result<&Value> {
    validate_day(detail, day)?;
    detail
        .get("dayList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("day").and_then(Value::as_u64) == Some(day.into()))
        .with_context(|| format!("trip has no day {day}"))
}

fn trip_item<'a>(detail: &'a Value, day: u32, item_id: &str) -> Result<&'a Value> {
    trip_day(detail, day)?
        .get("tsdList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
        .with_context(|| format!("item {item_id} was not found on day {day}"))
}

fn validate_reorder(current_ids: &[&str], requested_ids: &[String]) -> Result<()> {
    if current_ids.len() != requested_ids.len() {
        bail!("item_ids must contain every item on the day exactly once");
    }
    let mut current = current_ids.to_vec();
    let mut requested: Vec<&str> = requested_ids.iter().map(String::as_str).collect();
    current.sort_unstable();
    requested.sort_unstable();
    if current != requested {
        bail!("item_ids must contain every item on the day exactly once");
    }
    Ok(())
}

fn validate_traffic_type(value: &str) -> Result<()> {
    if matches!(value, "Driving" | "TwoWheeler" | "Transit" | "Walking") {
        Ok(())
    } else {
        bail!("traffic type must be Driving, TwoWheeler, Transit, or Walking")
    }
}

fn append_custom_time(
    fields: &mut Vec<(String, String)>,
    kind: &str,
    value: Option<NaiveTime>,
    current: &Value,
) {
    let use_field = format!("IsUseCustom{kind}Time");
    let time_field = format!("Custom{kind}Time");
    if let Some(value) = value {
        fields.push((use_field, "1".into()));
        fields.push((
            time_field,
            format!("0001/01/01 {}:00", value.format("%H:%M")),
        ));
    } else {
        let is_custom = current
            .get(format!("isUseCustom{kind}Time"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        fields.push((use_field, if is_custom { "1" } else { "0" }.into()));
        fields.push((
            time_field,
            current
                .get(format!("custom{kind}Time"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ));
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

    #[tokio::test]
    async fn appends_a_poi_using_the_web_apps_multipart_contract() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetWithDetail")
                .query_param("travelScheduleId", "trip-1")
                .query_param("updateTime", "0");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{
                    "travelScheduleInfo": { "id": "trip-1", "updateTime": 123456 },
                    "dayList": [{ "day": 1 }]
                }]
            }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/Poi/GetPoiById")
                .query_param("id", "poi-1");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "name": "Kushida Shrine", "cover": { "id": "cover-1" } }
            }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelScheduleDetail/GetAddWhere")
                .query_param("poiId", "poi-1")
                .query_param("travelScheduleId", "trip-1")
                .query_param("travelScheduleUpdateTime", "0");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": {
                    "dayList": [{
                        "day": 1,
                        "addWhereList": [{ "addWhereId": "append-here" }]
                    }]
                }
            }));
        });
        let add = server.mock(|when, then| {
            when.method(POST)
                .path("/TravelScheduleDetail/Add")
                .header_matches("content-type", "^multipart/form-data; boundary=")
                .body_includes("name=\"TravelScheduleId\"")
                .body_includes("trip-1")
                .body_includes("name=\"PoiId\"")
                .body_includes("poi-1")
                .body_includes("name=\"AddWhereId\"")
                .body_includes("append-here")
                .body_includes("name=\"TsdName\"")
                .body_includes("Kushida Shrine");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "id": "item-1" }
            }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let item = client.add_trip_poi("trip-1", 1, "poi-1").await.unwrap();

        add.assert();
        assert_eq!(item["id"], "item-1");
    }

    #[tokio::test]
    async fn reorders_a_complete_day_using_the_web_apps_contract() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetWithDetail")
                .query_param("travelScheduleId", "trip-1");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{
                    "travelScheduleInfo": { "id": "trip-1", "updateTime": 123456 },
                    "dayList": [{
                        "day": 1,
                        "tsdList": [{ "id": "item-1" }, { "id": "item-2" }]
                    }]
                }]
            }));
        });
        let reorder = server.mock(|when, then| {
            when.method(PUT)
                .path("/TravelScheduleDetail/Sort")
                .header_matches("content-type", "^multipart/form-data; boundary=")
                .body_includes("name=\"MoveOutDay\"")
                .body_includes("name=\"TsdIdList[]\"")
                .body_includes("item-2")
                .body_includes("item-1");
            then.status(200)
                .json_body(json!({ "apiStatus": "001", "data": 123457 }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let update = client
            .reorder_trip_day("trip-1", 1, &["item-2".into(), "item-1".into()])
            .await
            .unwrap();

        reorder.assert();
        assert_eq!(update, 123457);
    }

    #[tokio::test]
    async fn lists_routes_for_the_segment_arriving_at_an_item() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetWithDetail")
                .query_param("travelScheduleId", "trip-1");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{
                    "travelScheduleInfo": { "id": "trip-1", "updateTime": 123456 },
                    "dayList": [{
                        "day": 1,
                        "tsdList": [{ "id": "item-1", "tsdRouteDetailId": "segment-1" }]
                    }]
                }]
            }));
        });
        let routes = server.mock(|when, then| {
            when.method(GET)
                .path("/TravelScheduleDetailRoute/GetRouteList")
                .query_param("tsdRouteDetailId", "segment-1")
                .query_param("trafficType", "Transit")
                .query_param("travelScheduleId", "trip-1")
                .query_param("TravelScheduleUpdateTime", "123456");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "tsdRouteTransitList": [{ "poiRouteDetailId": "route-1" }] }
            }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let result = client
            .list_trip_item_routes("trip-1", 1, "item-1", "Transit")
            .await
            .unwrap();

        routes.assert();
        assert_eq!(
            result["tsdRouteTransitList"][0]["poiRouteDetailId"],
            "route-1"
        );
    }

    #[tokio::test]
    async fn moves_an_item_by_copying_then_deleting_the_original() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/TravelSchedule/GetWithDetail")
                .query_param("travelScheduleId", "trip-1");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": [{
                    "travelScheduleInfo": { "id": "trip-1", "updateTime": 123456 },
                    "dayList": [
                        { "day": 1, "tsdList": [{
                            "id": "item-1", "stayTime": 60,
                            "arrivalTrafficType": "Walking"
                        }] },
                        { "day": 2, "tsdList": [] }
                    ]
                }]
            }));
        });
        let copy = server.mock(|when, then| {
            when.method(POST)
                .path("/TravelScheduleDetail/Copy")
                .body_includes("name=\"CopyDay\"")
                .body_includes("name=\"CopyTsdId\"")
                .body_includes("item-1");
            then.status(200).json_body(json!({
                "apiStatus": "001",
                "data": { "travelScheduleUpdateTime": 123457 }
            }));
        });
        let delete = server.mock(|when, then| {
            when.method(DELETE)
                .path("/TravelScheduleDetail/Delete")
                .header("content-type", "application/x-www-form-urlencoded")
                .body_matches("(^|&)Day=1(&|$)")
                .body_matches("(^|&)TsdId=item-1(&|$)")
                .body_matches("(^|&)TravelScheduleUpdateTime=123457(&|$)");
            then.status(200)
                .json_body(json!({ "apiStatus": "001", "data": 123458 }));
        });
        let client = ChicTripClient::with_base_url(credentials(), server.base_url()).unwrap();

        let update = client
            .move_trip_item("trip-1", 1, 2, "item-1")
            .await
            .unwrap();

        copy.assert();
        delete.assert();
        assert_eq!(update, 123458);
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
