use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct CalendarEventsResponse {
    #[serde(rename = "kind")]
    pub kind: Option<String>,
    #[serde(rename = "etag")]
    pub etag: Option<String>,
    #[serde(rename = "summary")]
    pub summary: Option<String>,
    #[serde(rename = "description")]
    pub description: Option<String>,
    #[serde(rename = "updated")]
    pub updated: Option<String>,
    #[serde(rename = "timeZone")]
    pub time_zone: Option<String>,
    #[serde(rename = "accessRole")]
    pub access_role: Option<String>,
    #[serde(rename = "defaultReminders")]
    pub default_reminders: Option<Vec<DefaultReminder>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "items")]
    pub items: Vec<EventItem>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct DefaultReminder {
    #[serde(rename = "method")]
    pub method: Option<String>,
    #[serde(rename = "minutes")]
    pub minutes: Option<i64>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct EventItem {
    #[serde(rename = "kind")]
    pub kind: Option<String>,
    #[serde(rename = "etag")]
    pub etag: Option<String>,
    #[serde(rename = "id")]
    pub id: Option<String>,
    #[serde(rename = "status")]
    pub status: Option<String>,
    #[serde(rename = "htmlLink")]
    pub html_link: Option<String>,
    #[serde(rename = "created")]
    pub created: Option<String>,
    #[serde(rename = "updated")]
    pub updated: Option<String>,
    #[serde(rename = "summary")]
    pub summary: Option<String>,
    #[serde(rename = "description")]
    pub description: Option<String>,
    #[serde(rename = "location")]
    pub location: Option<String>,
    #[serde(rename = "recurringEventId")]
    pub recurring_event_id: Option<String>,
    #[serde(rename = "originalStartTime")]
    pub original_start_time: Option<EventDateTime>,
    #[serde(rename = "attendees")]
    pub attendees: Option<Vec<Attendee>>,
    #[serde(rename = "hangoutLink")]
    pub hangout_link: Option<String>,
    #[serde(rename = "conferenceData")]
    pub conference_data: Option<ConferenceData>,
    #[serde(rename = "guestsCanModify")]
    pub guests_can_modify: Option<bool>,
    #[serde(rename = "attachments")]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(rename = "creator")]
    pub creator: Option<CalendarUser>,
    #[serde(rename = "organizer")]
    pub organizer: Option<CalendarUser>,
    #[serde(rename = "start")]
    pub start: Option<EventDateTime>,
    #[serde(rename = "end")]
    pub end: Option<EventDateTime>,
    #[serde(rename = "iCalUID")]
    pub i_cal_uid: Option<String>,
    #[serde(rename = "sequence")]
    pub sequence: Option<i64>,
    #[serde(rename = "reminders")]
    pub reminders: Option<Reminders>,
    #[serde(rename = "eventType")]
    pub event_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Attendee {
    #[serde(rename = "email")]
    pub email: Option<String>,

    #[serde(rename = "organizer")]
    pub organizer: Option<bool>,

    #[serde(rename = "self")]
    pub self_field: Option<bool>,

    #[serde(rename = "resource")]
    pub resource: Option<bool>,

    #[serde(rename = "optional")]
    pub optional: Option<bool>,

    #[serde(rename = "displayName")]
    pub display_name: Option<String>,

    #[serde(rename = "comment")]
    pub comment: Option<String>,

    #[serde(rename = "responseStatus")]
    pub response_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConferenceData {
    #[serde(rename = "entryPoints")]
    pub entry_points: Option<Vec<EntryPoint>>,
    #[serde(rename = "conferenceSolution")]
    pub conference_solution: Option<ConferenceSolution>,
    #[serde(rename = "conferenceId")]
    pub conference_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntryPoint {
    #[serde(rename = "entryPointType")]
    pub entry_point_type: Option<String>,
    #[serde(rename = "uri")]
    pub uri: Option<String>,
    #[serde(rename = "label")]
    pub label: Option<String>,
    #[serde(rename = "pin")]
    pub pin: Option<String>,
    #[serde(rename = "regionCode")]
    pub region_code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConferenceSolution {
    #[serde(rename = "key")]
    pub key: Option<ConferenceSolutionKey>,

    #[serde(rename = "name")]
    pub name: Option<String>,

    #[serde(rename = "iconUri")]
    pub icon_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConferenceSolutionKey {
    #[serde(rename = "type")]
    pub key_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Attachment {
    #[serde(rename = "fileUrl")]
    pub file_url: Option<String>,
    #[serde(rename = "title")]
    pub title: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    #[serde(rename = "iconLink")]
    pub icon_link: Option<String>,
    #[serde(rename = "fileId")]
    pub file_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct CalendarUser {
    #[serde(rename = "email")]
    pub email: Option<String>,

    #[serde(rename = "self")]
    pub is_self: Option<bool>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct Reminders {
    #[serde(rename = "useDefault")]
    pub use_default: Option<bool>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct EventDateTime {
    #[serde(rename = "dateTime")]
    pub date_time: Option<String>,
    #[serde(rename = "timeZone")]
    pub time_zone: Option<String>,
    #[serde(rename = "date")]
    pub date: Option<String>,
}

impl EventItem {
    pub fn is_all_day(&self) -> bool {
        if let Some(start) = &self.start {
            if start.date.is_some() {
                return true;
            }
        }
        false
    }

    pub fn start_time_utc(&self) -> Option<DateTime<Utc>> {
        if let Some(start) = &self.start {
            if let Some(dt_str) = &start.date_time {
                if let Ok(dt) = DateTime::parse_from_rfc3339(dt_str) {
                    return Some(dt.with_timezone(&Utc));
                }
            }
        }
        None
    }

    pub fn end_time_utc(&self) -> Option<DateTime<Utc>> {
        if let Some(end) = &self.end {
            if let Some(dt_str) = &end.date_time {
                if let Ok(dt) = DateTime::parse_from_rfc3339(dt_str) {
                    return Some(dt.with_timezone(&Utc));
                }
            }
        }
        None
    }
}
