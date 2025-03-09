use crate::config::{self, Config};
use crate::core::event::{EventDurationCalculator, RealClock};
use crate::core::template::generate;
use crate::google::calendar::model::{CreatedEventResponse, EventItem};
use crate::google::calendar::service::{CalendarEventParams, GoogleCalendarService};
use crate::google::oauth::{OAuth2Client, OAuth2Error, Token};
use crate::mcp::handler::McpHandler;
use crate::mcp::stdio::StdioTransport;
use crate::shared::utils::date::intersection_days;
use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Cal2PromptError {
    #[error("OAuth2 port in use: {0}")]
    OAuth2PortInUse(#[from] OAuth2Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum JsonRpcErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    // Custom error codes should be in the range -32000 to -32099
    PortInUse = -32000,
    AccountNotFound = -32001,
    CalendarIdNotFound = -32002,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct AccountConfig {
    pub account_name: String,
    pub calendar_ids: Vec<String>,
    pub authorize_account: String,
    pub token: Option<Token>,
    pub path: String,
}

pub type AccountName = String;
pub struct Cal2Prompt {
    config: Config,
    pub accounts: BTreeMap<AccountName, AccountConfig>,
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
            Ok(config) => {
                let mut accounts = BTreeMap::new();
                for account in &config.source.google.accounts {
                    let token_path = format!("{}/{}", config.settings.oauth2_path, account.name);

                    accounts.insert(
                        account.name.to_string(),
                        AccountConfig {
                            token: None,
                            path: token_path,
                            account_name: account.name.to_string(),
                            calendar_ids: account.calendar_ids.clone(),
                            authorize_account: account.authorize_account.clone(),
                        },
                    );
                }

                Ok(Self { config, accounts })
            }
            Err(e) => Err(e),
        }
    }

    pub async fn oauth(&mut self, account_name: Option<String>) -> anyhow::Result<()> {
        let account_name = account_name.unwrap_or_else(|| "work".to_string());

        let _account = self
            .config
            .source
            .google
            .accounts
            .iter()
            .find(|account| account.name == account_name)
            .ok_or_else(|| anyhow::anyhow!("Account not found: {}", account_name))?;

        let oauth2_client = OAuth2Client::new(
            &self.config.source.google.oauth2.client_id,
            &self.config.source.google.oauth2.client_secret,
            &self.config.source.google.oauth2.redirect_url,
        );

        let account_path = self.accounts.get(&account_name).unwrap().path.clone();

        let token = match fs::read_to_string(&account_path) {
            Ok(content) => {
                let stored = serde_json::from_str::<Token>(&content)?;

                if stored.is_expired() {
                    if let Some(ref refresh) = stored.refresh_token {
                        let refreshed = oauth2_client.refresh_token(refresh.clone()).await?;
                        Self::save_token(&refreshed, &account_path).await?;
                        refreshed
                    } else {
                        match oauth2_client.oauth_flow().await {
                            Ok(token) => {
                                Self::save_token(&token, &account_path).await?;
                                token
                            }
                            Err(e) => {
                                if let Some(OAuth2Error::PortInUse) =
                                    e.downcast_ref::<OAuth2Error>()
                                {
                                    return Err(Cal2PromptError::OAuth2PortInUse(
                                        OAuth2Error::PortInUse,
                                    )
                                    .into());
                                }
                                return Err(e);
                            }
                        }
                    }
                } else {
                    stored
                }
            }
            Err(_) => match oauth2_client.oauth_flow().await {
                Ok(new_token) => {
                    Self::save_token(&new_token, &account_path).await?;
                    new_token
                }
                Err(e) => {
                    if let Some(OAuth2Error::PortInUse) = e.downcast_ref::<OAuth2Error>() {
                        return Err(Cal2PromptError::OAuth2PortInUse(OAuth2Error::PortInUse).into());
                    }
                    return Err(e);
                }
            },
        };

        if let Some(account_config) = self.accounts.get_mut(&account_name) {
            account_config.token = Some(token);
        }

        Ok(())
    }

    pub async fn ensure_valid_token(&mut self, account: Option<String>) -> anyhow::Result<()> {
        let account_name = match &account {
            Some(p) => p.clone(),
            None => self.accounts.keys().next().unwrap().clone(),
        };

        let account_path = self.accounts.get(&account_name).unwrap().path.clone();

        if let Some(token) = &self.accounts.get(&account_name).unwrap().token {
            if token.is_expired() {
                let oauth2_client = OAuth2Client::new(
                    &self.config.source.google.oauth2.client_id,
                    &self.config.source.google.oauth2.client_secret,
                    &self.config.source.google.oauth2.redirect_url,
                );

                if let Some(ref refresh_token) = token.refresh_token {
                    let refreshed = oauth2_client.refresh_token(refresh_token.clone()).await?;
                    Self::save_token(&refreshed, &account_path).await?;

                    self.accounts.get_mut(&account_name).unwrap().token = Some(refreshed);
                } else {
                    match oauth2_client.oauth_flow().await {
                        Ok(new_token) => {
                            Self::save_token(&new_token, &account_path).await?;
                            self.accounts.get_mut(&account_name).unwrap().token = Some(new_token);
                        }
                        Err(e) => {
                            if let Some(OAuth2Error::PortInUse) = e.downcast_ref::<OAuth2Error>() {
                                return Err(Cal2PromptError::OAuth2PortInUse(
                                    OAuth2Error::PortInUse,
                                )
                                .into());
                            }
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn launch_mcp(&mut self) -> anyhow::Result<()> {
        let (transport, _sender) = StdioTransport::new();
        let mut handler = McpHandler::new(self);
        handler.launch_mcp(&transport).await
    }

    pub async fn insert_event(
        &self,
        summary: &str,
        description: Option<String>,
        start: &str,
        end: &str,
        account: Option<AccountName>,
    ) -> anyhow::Result<CreatedEventResponse> {
        let account_name = match &account {
            Some(p) => p.clone(),
            None => self.accounts.keys().next().unwrap().clone(),
        };
        let account_config = self.accounts.get(&account_name).unwrap();
        let calendar_service = GoogleCalendarService::new();
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let calendar_id = account_config.calendar_ids.first().unwrap(); // FIXME mapping calndarName and calnderId

        let params = CalendarEventParams {
            summary,
            description,
            start,
            end,
            tz: &tz,
            calendar_id,
            token: &account_config.token.as_ref().unwrap().access_token,
        };

        calendar_service.create_calendar_event(params).await
    }

    pub async fn fetch_days(
        &self,
        since: &str,
        until: &str,
        account: Option<AccountName>,
    ) -> anyhow::Result<Vec<Day>> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let account_name = match &account {
            Some(p) => p.clone(),
            None => self.accounts.keys().next().unwrap().clone(),
        };
        let account_config = self.accounts.get(&account_name).unwrap();

        let since_naive_date = NaiveDate::parse_from_str(since, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let until_naive_date = NaiveDate::parse_from_str(until, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let since_with_tz = tz.from_local_datetime(&since_naive_date).unwrap();
        let until_with_tz = tz.from_local_datetime(&until_naive_date).unwrap();

        let calendar_service = GoogleCalendarService::new();
        let all_events = calendar_service
            .get_calendar_events(
                since,
                until,
                &tz,
                &account_config.calendar_ids,
                &account_config.token.as_ref().unwrap().access_token,
            )
            .await?;

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

    #[allow(dead_code)]
    pub async fn get_events_short_cut(
        self,
        get_event_duration: GetEventDuration,
        account: Option<AccountName>,
    ) -> anyhow::Result<String> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let calculator = EventDurationCalculator::new(RealClock);
        let (since_with_tz, until_with_tz) = calculator.get_duration(&tz, get_event_duration);

        let since = since_with_tz.format("%Y-%m-%d").to_string();
        let until = until_with_tz.format("%Y-%m-%d").to_string();

        let days = self.fetch_days(&since, &until, account).await?;
        self.render_days(days)
    }

    pub async fn fetch_duration(
        &self,
        get_event_duration: GetEventDuration,
        account: Option<AccountName>,
    ) -> anyhow::Result<String> {
        let tz: Tz =
            self.config.settings.tz.parse().unwrap_or_else(|_| {
                panic!("Invalid time zone string '{}'", self.config.settings.tz)
            });

        let calculator = EventDurationCalculator::new(RealClock);
        let (since_with_tz, until_with_tz) = calculator.get_duration(&tz, get_event_duration);

        let since = since_with_tz.format("%Y-%m-%d").to_string();
        let until = until_with_tz.format("%Y-%m-%d").to_string();

        let days = self.fetch_days(&since, &until, account).await?;
        self.render_days(days)
    }

    pub fn render_days(&self, days: Vec<Day>) -> anyhow::Result<String> {
        generate(&self.config.prompt.template, days)
    }

    async fn save_token(token: &Token, token_file_path: &str) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(&token)?;
        fs::create_dir_all(
            Path::new(token_file_path)
                .parent()
                .expect("Failed to get token path"),
        )?;

        fs::write(token_file_path, text)?;
        Ok(())
    }

    pub fn get_accounts(&self) -> anyhow::Result<Vec<AccountConfig>> {
        let accounts = self.accounts.values().cloned().collect();
        Ok(accounts)
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
