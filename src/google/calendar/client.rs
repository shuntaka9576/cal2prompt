use reqwest::Client;
use thiserror::Error;

use super::model::{CalendarEventsResponse, CreatedEventResponse, InsertEventRequest};

#[derive(Error, Debug)]
pub enum GoogleCalendarError {
    #[error("http error: {0}")]
    HttpError(#[from] reqwest::Error),
}

pub struct GoogleCalendarClient {
    client: Client,
    access_token: String,
}

impl GoogleCalendarClient {
    pub fn new<T: Into<String>>(token: T) -> Self {
        GoogleCalendarClient {
            client: Client::new(),
            access_token: token.into(),
        }
    }

    pub async fn fetch_calendar_events(
        &self,
        calendar_id: &str,
        since: &str,
        until: &str,
    ) -> anyhow::Result<CalendarEventsResponse> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            calendar_id
        );

        let response = self
            .client
            .get(&url)
            .bearer_auth(self.access_token.clone())
            .query(&[
                ("timeMin", since),
                ("timeMax", until),
                ("singleEvents", "true"),
                ("orderBy", "startTime"),
            ])
            .send()
            .await?
            .error_for_status()?;

        let calendar_events_response = response.json::<CalendarEventsResponse>().await?;

        Ok(calendar_events_response)
    }

    pub async fn create_calendar_event(
        &self,
        calendar_id: &str,
        new_event: &InsertEventRequest,
    ) -> anyhow::Result<CreatedEventResponse> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            calendar_id
        );

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(new_event)
            .send()
            .await?
            .error_for_status()?;

        let created_event = response.json::<CreatedEventResponse>().await?;
        Ok(created_event)
    }
}
