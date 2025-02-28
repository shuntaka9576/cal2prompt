use crate::config::{self, Config};
use crate::core::event::{EventDurationCalculator, RealClock};
use crate::core::template::generate;
use crate::google::calendar::client::GoogleCalendarClient;
use crate::google::calendar::model::{
    CreatedEventResponse, EventDateTime, EventItem, InsertEventRequest,
};
use crate::google::oauth::{OAuth2Client, Token};
use crate::mcp::stdio::{Message, StdioTransport, Transport};
use crate::shared::utils::date::{intersection_days, to_utc_start_of_start_rfc3339};
use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use futures::future;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

static TOOLS_JSON: &str = include_str!("./mcp/tools.json");

#[derive(Debug)]
#[allow(dead_code)]
pub enum JsonRpcErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("No calendar_id configured. Please specify experimental.mcp.insertCalendarEvent.calendarID in your config.")]
    NoCalendarId,
}

pub struct Cal2Prompt {
    config: Config,
    token: Option<Token>,
}

#[derive(Debug, Serialize)]
struct Event {
    summary: String,
    start: String,
    end: String,
    location: Option<String>,
    description: Option<String>,
    attendees: Vec<String>,
    html_link: Option<String>,
    all_day: bool,
}

#[derive(Serialize, Debug)]
pub struct Day {
    date: String,
    all_day_events: Vec<Event>,
    timed_events: Vec<Event>,
}

#[derive(Debug, PartialEq)]
pub enum GetEventDuration {
    Today,
    ThisWeek,
    ThisMonth,
    NextWeek,
}

impl Cal2Prompt {
    pub fn new() -> anyhow::Result<Self> {
        match config::init() {
            Ok(config) => Ok(Self {
                config,
                token: None,
            }),
            Err(e) => Err(e),
        }
    }

    pub async fn oauth(&mut self) -> anyhow::Result<()> {
        let oauth2_client = OAuth2Client::new(
            &self.config.source.google.oauth2.client_id,
            &self.config.source.google.oauth2.client_secret,
            &self.config.source.google.oauth2.redirect_url,
        );

        let token = match fs::read_to_string(&self.config.settings.oauth_file_path) {
            Ok(content) => {
                let stored = serde_json::from_str::<Token>(&content)?;

                if stored.is_expired() {
                    if let Some(ref refresh) = stored.refresh_token {
                        let refreshed = oauth2_client.refresh_token(refresh.clone()).await?;
                        self.save_token(&refreshed).await?;
                        refreshed
                    } else {
                        let token = oauth2_client.oauth_flow().await?;
                        self.save_token(&token).await?;
                        token
                    }
                } else {
                    stored
                }
            }
            Err(_) => {
                let new_token = oauth2_client.oauth_flow().await?;
                self.save_token(&new_token).await?;
                new_token
            }
        };

        self.token = Some(token);

        Ok(())
    }

    async fn ensure_valid_token(&mut self) -> anyhow::Result<()> {
        if let Some(token) = &self.token {
            if token.is_expired() {
                let oauth2_client = OAuth2Client::new(
                    &self.config.source.google.oauth2.client_id,
                    &self.config.source.google.oauth2.client_secret,
                    &self.config.source.google.oauth2.redirect_url,
                );

                if let Some(ref refresh_token) = token.refresh_token {
                    let refreshed = oauth2_client.refresh_token(refresh_token.clone()).await?;
                    self.save_token(&refreshed).await?;
                    self.token = Some(refreshed);
                } else {
                    let new_token = oauth2_client.oauth_flow().await?;
                    self.save_token(&new_token).await?;
                    self.token = Some(new_token);
                }
            }
        }
        Ok(())
    }

    pub async fn launch_mcp(&mut self) -> anyhow::Result<()> {
        let (transport, _sender) = StdioTransport::new();
        let mut stream = transport.receive();

        eprintln!("MCP stdio transport server started. Waiting for JSON messages on stdin...");

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(Message::Request {
                    id, method, params, ..
                }) => {
                    eprintln!(
                        "[SERVER] Got Request: id={}, method={}, params={:?}",
                        id, method, params
                    );

                    if let Err(err) = self.ensure_valid_token().await {
                        self.send_error_response(
                            &transport,
                            id,
                            JsonRpcErrorCode::InternalError,
                            format!("Failed to refresh token: {}", err),
                        )
                        .await?;
                        continue;
                    }

                    if let Err(err) = self.handle_request(&transport, id, method, params).await {
                        eprintln!("[SERVER] Error handling request: {:?}", err);
                        self.send_error_response(
                            &transport,
                            id,
                            JsonRpcErrorCode::InternalError,
                            format!("Failed to handle request: {}", err),
                        )
                        .await?;
                    }
                }
                Ok(Message::Notification { method, params, .. }) => {
                    eprintln!(
                        "[SERVER] Got Notification: method={}, params={:?}",
                        method, params
                    );
                }
                Ok(Message::Response {
                    id, result, error, ..
                }) => {
                    eprintln!(
                        "[SERVER] Got Response: id={}, result={:?}, error={:?}",
                        id, result, error
                    );
                }
                Err(e) => {
                    eprintln!("[SERVER] Error receiving message: {:?}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_request(
        &self,
        transport: &StdioTransport,
        id: u64,
        method: String,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        match &*method {
            "initialize" => self.handle_initialize(transport, id).await?,
            "tools/list" => self.handle_tools_list(transport, id).await?,
            "tools/call" => {
                if let Some(params_val) = params {
                    self.handle_tools_call(transport, id, params_val).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_initialize(&self, transport: &StdioTransport, id: u64) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "capabilities": {
                    "experimental": {},
                    "prompts": { "listChanged": false },
                    "resources": { "listChanged": false, "subscribe": false },
                    "tools": { "listChanged": false }
                },
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "cal2prompt",
                    "version": "0.1.0"
                }
            })),
            error: None,
        };
        transport.send(response).await?;
        Ok(())
    }

    async fn handle_tools_list(&self, transport: &StdioTransport, id: u64) -> anyhow::Result<()> {
        let tools_value: serde_json::Value =
            serde_json::from_str(TOOLS_JSON).expect("tools.json must be valid JSON");

        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(tools_value),
            error: None,
        };

        transport.send(response).await?;
        Ok(())
    }

    async fn handle_tools_call(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: serde_json::Value,
    ) -> anyhow::Result<()> {
        let tool_name = match params_val.get("name").and_then(Value::as_str) {
            Some(name) => name,
            None => return Ok(()),
        };

        match tool_name {
            "list_calendar_events" => {
                self.handle_list_calendar_events(transport, id, &params_val)
                    .await?
            }
            "insert_calendar_event" => {
                self.handle_insert_calendar_event(transport, id, &params_val)
                    .await?
            }
            _ => {}
        }

        Ok(())
    }

    async fn handle_list_calendar_events(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let since_str = params_val
            .pointer("/arguments/since")
            .and_then(Value::as_str)
            .unwrap_or("");
        let until_str = params_val
            .pointer("/arguments/until")
            .and_then(Value::as_str)
            .unwrap_or("");

        match self.fetch_days(since_str, until_str).await {
            Ok(days) => {
                let result_json = json!({ "days": days });
                let obj_as_str = serde_json::to_string(&result_json)?;
                self.send_text_response(transport, id, &obj_as_str).await?;
            }
            Err(err) => {
                self.send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InternalError,
                    format!("Failed to fetch calendar events: {}", err),
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn handle_insert_calendar_event(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let summary_str = params_val
            .pointer("/arguments/summary")
            .and_then(Value::as_str)
            .unwrap_or("");
        let description_str: Option<String> = params_val
            .pointer("/arguments/description")
            .and_then(Value::as_str)
            .map(String::from);
        let start_str = params_val
            .pointer("/arguments/start")
            .and_then(Value::as_str)
            .unwrap_or("");
        let end_str = params_val
            .pointer("/arguments/end")
            .and_then(Value::as_str)
            .unwrap_or("");

        match self
            .insert_event(summary_str, description_str, start_str, end_str)
            .await
        {
            Ok(res) => {
                let obj_as_str = serde_json::to_string(&res)?;
                self.send_text_response(transport, id, &obj_as_str).await?;
            }
            Err(err) => {
                let (code, message) = match err.downcast_ref::<CalendarError>() {
                    Some(CalendarError::NoCalendarId) => {
                        (JsonRpcErrorCode::InvalidParams, err.to_string())
                    }
                    None => (
                        JsonRpcErrorCode::InternalError,
                        format!("Unexpected error: {}", err),
                    ),
                };

                self.send_error_response(transport, id, code, message)
                    .await?;
            }
        }

        Ok(())
    }

    async fn send_text_response(
        &self,
        transport: &StdioTransport,
        id: u64,
        text: &str,
    ) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "content": [{
                    "type": "text",
                    "text": text,
                }],
            })),
            error: None,
        };
        transport.send(response).await?;
        Ok(())
    }

    async fn send_error_response(
        &self,
        transport: &StdioTransport,
        id: u64,
        code: JsonRpcErrorCode,
        message: String,
    ) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(json!({
                "code": code as i32,
                "message": message,
            })),
        };
        transport.send(response).await?;
        Ok(())
    }

    pub async fn insert_event(
        &self,
        summary: &str,
        description: Option<String>,
        start: &str,
        end: &str,
    ) -> anyhow::Result<CreatedEventResponse> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let start_naive_date = NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M")?;
        let end_naive_date = NaiveDateTime::parse_from_str(end, "%Y-%m-%d %H:%M")?;

        let start_with_tz = tz.from_local_datetime(&start_naive_date).unwrap();
        let end_with_tz = tz.from_local_datetime(&end_naive_date).unwrap();

        let start_rfc3339 = start_with_tz.to_rfc3339();
        let end_rfc3339 = end_with_tz.to_rfc3339();

        let calendar_client = GoogleCalendarClient::new(
            self.token
                .as_ref()
                .expect("token not set")
                .access_token
                .clone(),
        );

        let Some(calendar_id) = &self
            .config
            .experimental
            .mcp
            .insert_calendar_event
            .calendar_id
        else {
            return Err(CalendarError::NoCalendarId.into());
        };

        let res = calendar_client
            .create_calendar_event(
                calendar_id,
                &InsertEventRequest {
                    summary: summary.to_string(),
                    start: EventDateTime {
                        date_time: Some(start_rfc3339),
                        time_zone: Some("Asia/Tokyo".to_string()),
                        date: None,
                    },
                    end: EventDateTime {
                        date_time: Some(end_rfc3339),
                        time_zone: Some("Asia/Tokyo".to_string()),
                        date: None,
                    },
                    location: None,
                    description,
                    attendees: None, // TODO: add attendees
                },
            )
            .await?;

        Ok(res)
    }

    pub async fn fetch_days(&self, since: &str, until: &str) -> anyhow::Result<Vec<Day>> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let since_naive_date = NaiveDate::parse_from_str(since, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let until_naive_date = NaiveDate::parse_from_str(until, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let since_with_tz = tz.from_local_datetime(&since_naive_date).unwrap();
        let until_with_tz = tz.from_local_datetime(&until_naive_date).unwrap();

        let until_plus_one = until_with_tz.checked_add_days(Days::new(1)).unwrap();
        let since_rfc3339 = to_utc_start_of_start_rfc3339(since_with_tz);
        let until_rfc3339 = to_utc_start_of_start_rfc3339(until_plus_one);

        let calendar_client = GoogleCalendarClient::new(
            self.token
                .as_ref()
                .expect("token not set")
                .access_token
                .clone(),
        );

        let mut fetch_futures = Vec::new();
        for calendar_id in &self.config.source.google.calendar.get_events.calendar_ids {
            let fut =
                calendar_client.fetch_calendar_events(calendar_id, &since_rfc3339, &until_rfc3339);
            fetch_futures.push(fut);
        }

        let results = future::join_all(fetch_futures).await;

        let mut all_events: Vec<EventItem> = Vec::new();
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(mut res) => {
                    all_events.append(&mut res.items);
                }
                Err(e) => {
                    eprintln!(
                        "Error fetching events from calendar_id={}: {}",
                        &self.config.source.google.calendar.get_events.calendar_ids[i], e
                    );
                }
            }
        }

        Ok(Self::group_events_into_days(
            all_events,
            since_with_tz,
            until_with_tz,
            tz,
        ))
    }

    fn group_events_into_days(
        mut all_events: Vec<EventItem>,
        since_with_tz: DateTime<Tz>,
        until_with_tz: DateTime<Tz>,
        tz: Tz,
    ) -> Vec<Day> {
        all_events.sort_by_key(|e| e.start_time_utc());

        let mut grouped: BTreeMap<String, (Vec<Event>, Vec<Event>)> = BTreeMap::new();

        for ev_item in &all_events {
            let is_all_day = ev_item.is_all_day();
            let mut attendees_emails = Vec::new();
            if let Some(ats) = &ev_item.attendees {
                for at in ats {
                    if let Some(email) = &at.email {
                        attendees_emails.push(email.to_string());
                    }
                }
            }

            if is_all_day {
                let all_day_start_day = ev_item.start.as_ref().unwrap().date.clone().unwrap();
                let all_day_end_day = ev_item.end.as_ref().unwrap().date.clone().unwrap();
                let all_day_start_day =
                    NaiveDate::parse_from_str(&all_day_start_day, "%Y-%m-%d").unwrap();
                let all_day_end_day =
                    NaiveDate::parse_from_str(&all_day_end_day, "%Y-%m-%d").unwrap();

                let duration = intersection_days(
                    all_day_start_day,
                    all_day_end_day,
                    since_with_tz.date_naive(),
                    until_with_tz.date_naive(),
                );

                for day in duration {
                    let entry = grouped
                        .entry(day.to_string())
                        .or_insert_with(|| (vec![], vec![]));

                    let event = Event {
                        summary: ev_item
                            .summary
                            .clone()
                            .unwrap_or_else(|| "(no summary)".to_string()),
                        start: all_day_start_day.to_string(),
                        end: all_day_end_day.to_string(),
                        location: ev_item.location.clone(),
                        description: ev_item.description.clone(),
                        attendees: attendees_emails.clone(),
                        html_link: ev_item.html_link.clone(),
                        all_day: true,
                    };

                    entry.0.push(event);
                }
            } else {
                let start_utc_opt = ev_item.start_time_utc().unwrap();
                let end_utc_opt = ev_item.end_time_utc().unwrap();

                let date_key = start_utc_opt
                    .with_timezone(&tz)
                    .date_naive()
                    .format("%Y-%m-%d")
                    .to_string();
                let start_local_str = start_utc_opt
                    .with_timezone(&tz)
                    .naive_local()
                    .format("%H:%M")
                    .to_string();
                let end_local_str = end_utc_opt
                    .with_timezone(&tz)
                    .naive_local()
                    .format("%H:%M")
                    .to_string();

                let event = Event {
                    summary: ev_item
                        .summary
                        .clone()
                        .unwrap_or_else(|| "(no summary)".to_string()),
                    start: start_local_str,
                    end: end_local_str,
                    location: ev_item.location.clone(),
                    description: ev_item.description.clone(),
                    attendees: attendees_emails,
                    html_link: ev_item.html_link.clone(),
                    all_day: false,
                };

                let entry = grouped.entry(date_key).or_insert_with(|| (vec![], vec![]));
                entry.1.push(event);
            }
        }

        let mut days = Vec::new();
        for (date, (all_day_events, timed_events)) in grouped {
            days.push(Day {
                date,
                all_day_events,
                timed_events,
            });
        }
        days
    }

    pub async fn get_events_duration(self, since: String, until: String) -> anyhow::Result<String> {
        let days = self.fetch_days(&since, &until).await?;
        generate(&self.config.output.template, days)
    }

    pub async fn get_events_short_cut(
        self,
        get_event_duration: GetEventDuration,
    ) -> anyhow::Result<String> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let calculator = EventDurationCalculator::new(RealClock);
        let (since_with_tz, until_with_tz) = calculator.get_duration(&tz, get_event_duration);

        let since = since_with_tz.format("%Y-%m-%d").to_string();
        let until = until_with_tz.format("%Y-%m-%d").to_string();

        let days = self.fetch_days(&since, &until).await?;
        generate(&self.config.output.template, days)
    }

    async fn save_token(&self, token: &Token) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(&token)?;
        fs::create_dir_all(
            Path::new(&self.config.settings.oauth_file_path)
                .parent()
                .expect("Failed to get token dir"),
        )?;
        fs::write(&self.config.settings.oauth_file_path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::templates::google::STANDARD;
    use crate::google::calendar::model::CalendarEventsResponse;

    #[test]
    fn test_event_to_llm_prompt() {
        let json_str = r#"
{
 "kind": "calendar#events",
 "etag": "\"p33sbfm5on6bom0o\"",
 "summary": "example@email.com",
 "description": "",
 "updated": "2025-01-28T03:14:25.579Z",
 "timeZone": "Asia/Tokyo",
 "accessRole": "owner",
 "defaultReminders": [
  {
   "method": "popup",
   "minutes": 10
  }
 ],
 "items": [
  {
   "kind": "calendar#event",
   "etag": "\"3476068131158000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T03:14:25.000Z",
   "updated": "2025-01-28T03:14:25.579Z",
   "summary": "All Day Event!",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "date": "2025-01-04"
   },
   "end": {
    "date": "2025-01-07"
   },
   "transparency": "transparent",
   "iCalUID": "0thgu6kfnv5j3q408oi5a58ihi@google.com",
   "sequence": 0,
   "guestsCanModify": true,
   "reminders": {
    "useDefault": false
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Morning Routine",
   "description": "Wake up and get ready for the day.",
   "location": "Home",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-05T23:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T00:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "morning_routine_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Commute to Office",
   "description": "Drive or take public transit to work.",
   "location": "Silicon Valley",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T00:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T00:30:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "commute_to_office_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Check Email & Prep",
   "description": "Respond to emails, plan tasks for the day.",
   "location": "Office Desk",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T00:30:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T01:30:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "check_email_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Team Stand-up Meeting",
   "description": "Daily stand-up with the dev team.",
   "location": "Meeting Room A",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T01:30:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T02:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "team_standup_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Development & Coding",
   "description": "Focus time for coding new features and bug fixes.",
   "location": "Office Desk",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T02:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T05:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "morning_dev_time_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Lunch Break",
   "description": "Grab lunch with coworkers or nearby café.",
   "location": "Cafeteria / Nearby Restaurant",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T05:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T06:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "lunch_break_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Code Review & Collaboration",
   "description": "Review pull requests, pair programming session.",
   "location": "Office Desk / Meeting Room B",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T06:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T08:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "afternoon_code_review_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Development & Debugging",
   "description": "Continue feature development, address tech debt.",
   "location": "Office Desk",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T08:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T10:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "afternoon_dev_time_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Commute Home",
   "description": "Traffic or train ride back home.",
   "location": "Silicon Valley",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T10:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T11:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "commute_home_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  },
  {
   "kind": "calendar#event",
   "etag": "\"3476046468512000\"",
   "id": "***",
   "status": "confirmed",
   "htmlLink": "https://www.google.com/calendar/event?eid=***",
   "created": "2025-01-28T00:13:54.000Z",
   "updated": "2025-01-28T00:13:54.256Z",
   "summary": "Evening / Personal Time",
   "description": "Relax, dinner, side projects, or family time.",
   "location": "Home",
   "creator": {
    "email": "example@email.com",
    "self": true
   },
   "organizer": {
    "email": "example@email.com",
    "self": true
   },
   "start": {
    "dateTime": "2025-01-06T11:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "end": {
    "dateTime": "2025-01-06T16:00:00+09:00",
    "timeZone": "America/Los_Angeles"
   },
   "iCalUID": "evening_personal_time_20250105@siliconvalley",
   "sequence": 0,
   "reminders": {
    "useDefault": true
   },
   "eventType": "default"
  }
 ]
}
    "#;

        let parsed: CalendarEventsResponse = serde_json::from_str(json_str).unwrap();
        let tz: Tz = "America/Los_Angeles".parse().unwrap();
        let since_naive_date = NaiveDate::parse_from_str("2025-01-05", "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let until_naive_date = NaiveDate::parse_from_str("2025-01-06", "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let since_with_tz = tz.from_local_datetime(&since_naive_date).unwrap();
        let until_with_tz = tz.from_local_datetime(&until_naive_date).unwrap();

        let days =
            Cal2Prompt::group_events_into_days(parsed.items, since_with_tz, until_with_tz, tz);
        let prompt = generate(STANDARD, days).unwrap();

        assert_eq!(
            prompt,
            r#"Here is your schedule summary. Please find the details below:
## Date: 2025-01-05

### All-Day Events:
- All Day Event!
  - (All Day)
  - Location: N/A
  - Description: No description.
  - Attendees:
    - (No attendees)

### Events:
- Morning Routine
  - Start: 06:00
  - End:   07:00
  - Location: Home
  - Description: Wake up and get ready for the day.
  - Attendees:
    - (No attendees)
- Commute to Office
  - Start: 07:00
  - End:   07:30
  - Location: Silicon Valley
  - Description: Drive or take public transit to work.
  - Attendees:
    - (No attendees)
- Check Email & Prep
  - Start: 07:30
  - End:   08:30
  - Location: Office Desk
  - Description: Respond to emails, plan tasks for the day.
  - Attendees:
    - (No attendees)
- Team Stand-up Meeting
  - Start: 08:30
  - End:   09:00
  - Location: Meeting Room A
  - Description: Daily stand-up with the dev team.
  - Attendees:
    - (No attendees)
- Development & Coding
  - Start: 09:00
  - End:   12:00
  - Location: Office Desk
  - Description: Focus time for coding new features and bug fixes.
  - Attendees:
    - (No attendees)
- Lunch Break
  - Start: 12:00
  - End:   13:00
  - Location: Cafeteria / Nearby Restaurant
  - Description: Grab lunch with coworkers or nearby café.
  - Attendees:
    - (No attendees)
- Code Review & Collaboration
  - Start: 13:00
  - End:   15:00
  - Location: Office Desk / Meeting Room B
  - Description: Review pull requests, pair programming session.
  - Attendees:
    - (No attendees)
- Development & Debugging
  - Start: 15:00
  - End:   17:00
  - Location: Office Desk
  - Description: Continue feature development, address tech debt.
  - Attendees:
    - (No attendees)
- Commute Home
  - Start: 17:00
  - End:   18:00
  - Location: Silicon Valley
  - Description: Traffic or train ride back home.
  - Attendees:
    - (No attendees)
- Evening / Personal Time
  - Start: 18:00
  - End:   23:00
  - Location: Home
  - Description: Relax, dinner, side projects, or family time.
  - Attendees:
    - (No attendees)
## Date: 2025-01-06

### All-Day Events:
- All Day Event!
  - (All Day)
  - Location: N/A
  - Description: No description.
  - Attendees:
    - (No attendees)

### Events:
(No timed events)
"#
        )
    }
}
