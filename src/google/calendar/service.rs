use chrono::{Days, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use futures::future;

use crate::google::calendar::client::GoogleCalendarClient;
use crate::google::calendar::model::{
    CreatedEventResponse, EventDateTime, EventItem, InsertEventRequest,
};
use crate::shared::utils::date::to_utc_start_of_start_rfc3339;

#[derive(Debug, thiserror::Error)]
pub enum CalendarServiceError {
    #[error("No calendar_id configured. Please specify experimental.mcp.insertCalendarEvent.calendarID in your config.")]
    NoCalendarId,
    #[error("Profile '{0}' not found in configuration")]
    ProfileNotFound(String),
}

pub struct GoogleCalendarService {
    calendar_client: GoogleCalendarClient,
}

impl GoogleCalendarService {
    pub fn new() -> Self {
        let calendar_client = GoogleCalendarClient::new();

        Self { calendar_client }
    }

    pub async fn create_calendar_event(
        &self,
        summary: &str,
        description: Option<String>,
        start: &str,
        end: &str,
        tz: &Tz,
        calendar_id: &str,
        token: &str,
    ) -> anyhow::Result<CreatedEventResponse> {
        let start_naive_date = NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M")?;
        let end_naive_date = NaiveDateTime::parse_from_str(end, "%Y-%m-%d %H:%M")?;

        let start_with_tz = tz.from_local_datetime(&start_naive_date).unwrap();
        let end_with_tz = tz.from_local_datetime(&end_naive_date).unwrap();

        let start_rfc3339 = start_with_tz.to_rfc3339();
        let end_rfc3339 = end_with_tz.to_rfc3339();

        let res = self
            .calendar_client
            .create_calendar_event(
                token,
                calendar_id,
                &InsertEventRequest {
                    summary: summary.to_string(),
                    start: EventDateTime {
                        date_time: Some(start_rfc3339),
                        time_zone: Some(tz.to_string()),
                        date: None,
                    },
                    end: EventDateTime {
                        date_time: Some(end_rfc3339),
                        time_zone: Some(tz.to_string()),
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

    // #[allow(dead_code)]
    // pub async fn get_calendar_events(
    //     &self,
    //     since: &str,
    //     until: &str,
    // ) -> anyhow::Result<Vec<EventItem>> {
    //     self.get_calendar_events(since, until, None).await
    // }

    pub async fn get_calendar_events(
        &self,
        since: &str,
        until: &str,
        tz: &Tz,
        calendar_ids: &[String],
        token: &String,
    ) -> anyhow::Result<Vec<EventItem>> {
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

        let mut fetch_futures = Vec::new();
        for calendar_id in calendar_ids {
            let fut = self.calendar_client.fetch_calendar_events(
                calendar_id,
                &since_rfc3339,
                &until_rfc3339,
                token,
            );
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
                        &calendar_ids[i], e
                    );
                }
            }
        }

        Ok(all_events)
        // if let Some(profile_config) = self.config.source.google.profile.get(profile) {
        //     for calendar_id in &profile_config.calendar_ids {
        //         let fut = self.calendar_client.fetch_calendar_events(
        //             calendar_id,
        //             &since_rfc3339,
        //             &until_rfc3339,
        //         );
        //         fetch_futures.push(fut);
        //         used_calendar_ids.push(calendar_id.clone());
        //     }
        // } else {
        //     for calendar_id in &self
        //         .config
        //         .source
        //         .google
        //         .profile
        //         .get("work")
        //         .unwrap()
        //         .calendar_ids
        //     {
        //         let fut = self.calendar_client.fetch_calendar_events(
        //             calendar_id,
        //             &since_rfc3339,
        //             &until_rfc3339,
        //         );
        //         fetch_futures.push(fut);
        //         used_calendar_ids.push(calendar_id.clone());
        //     }
        // }

        // let results = future::join_all(fetch_futures).await;

        // let mut all_events: Vec<EventItem> = Vec::new();
        // for (i, result) in results.into_iter().enumerate() {
        //     match result {
        //         Ok(mut res) => {
        //             all_events.append(&mut res.items);
        //         }
        //         Err(e) => {
        //             eprintln!(
        //                 "Error fetching events from calendar_id={}: {}",
        //                 &used_calendar_ids[i], e
        //             );
        //         }
        //     }
        // }
    }
}
