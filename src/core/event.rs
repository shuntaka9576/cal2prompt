use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

use crate::core::cal2prompt::GetEventDuration;

#[cfg_attr(test, mockall::automock)]
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
}

pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct EventDurationCalculator<C: Clock> {
    clock: C,
}

impl<C: Clock> EventDurationCalculator<C> {
    pub fn new(clock: C) -> Self {
        Self { clock }
    }

    pub fn get_duration<TZ: TimeZone>(
        &self,
        tz: &TZ,
        duration: GetEventDuration,
    ) -> (DateTime<TZ>, DateTime<TZ>) {
        let now_utc = self.clock.now();
        let now_local = now_utc.with_timezone(tz);

        match duration {
            GetEventDuration::Today => {
                let since = now_local.clone();
                let until = now_local.clone();
                (since, until)
            }
            GetEventDuration::ThisWeek => {
                let weekday = now_local.weekday();
                let days_from_monday = weekday.num_days_from_monday();
                let monday = now_local - Duration::days(days_from_monday.into());
                let sunday = monday.clone() + Duration::days(6);

                let since = monday;
                let until = sunday;
                (since, until)
            }
            GetEventDuration::ThisMonth => {
                let first_day = now_local.with_day(1).unwrap();
                let next_month = if now_local.month() == 12 {
                    first_day
                        .with_month(1)
                        .unwrap()
                        .with_year(now_local.year() + 1)
                        .unwrap()
                } else {
                    first_day.with_month(now_local.month() + 1).unwrap()
                };
                let last_day = next_month - Duration::days(1);

                let since = first_day;
                let until = last_day;
                (since, until)
            }
            GetEventDuration::NextWeek => {
                let weekday = now_local.weekday();
                let days_until_next_monday = 7 - weekday.num_days_from_monday();
                let next_monday = now_local + Duration::days(days_until_next_monday.into());
                let next_sunday = next_monday.clone() + Duration::days(6);

                let since = next_monday;
                let until = next_sunday;

                (since, until)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};

    #[test]
    fn test_today() {
        let mut mock_clock = MockClock::new();
        mock_clock
            .expect_now()
            .returning(|| Utc.with_ymd_and_hms(2025, 1, 26, 15, 0, 0).unwrap());

        let calculator = EventDurationCalculator::new(mock_clock);
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();

        let (since, until) = calculator.get_duration(&jst, GetEventDuration::Today);

        assert_eq!(since.format("%Y-%m-%d").to_string(), "2025-01-27");
        assert_eq!(until.format("%Y-%m-%d").to_string(), "2025-01-27");
    }

    #[test]
    fn test_this_week() {
        let mut mock_clock = MockClock::new();
        mock_clock
            .expect_now()
            .returning(|| Utc.with_ymd_and_hms(2025, 1, 27, 15, 0, 0).unwrap());

        let calculator = EventDurationCalculator::new(mock_clock);
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let (since, until) = calculator.get_duration(&jst, GetEventDuration::ThisWeek);

        assert_eq!(since.format("%Y-%m-%d").to_string(), "2025-01-27");
        assert_eq!(until.format("%Y-%m-%d").to_string(), "2025-02-02");
    }

    #[test]
    fn test_this_month() {
        let mut mock_clock = MockClock::new();
        mock_clock
            .expect_now()
            .returning(|| Utc.with_ymd_and_hms(2025, 1, 26, 15, 0, 0).unwrap());

        let calculator = EventDurationCalculator::new(mock_clock);
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let (since, until) = calculator.get_duration(&jst, GetEventDuration::ThisMonth);

        assert_eq!(since.format("%Y-%m-%d").to_string(), "2025-01-01");
        assert_eq!(until.format("%Y-%m-%d").to_string(), "2025-01-31");
    }

    #[test]
    fn test_next_week() {
        let mut mock_clock = MockClock::new();
        mock_clock
            .expect_now()
            .returning(|| Utc.with_ymd_and_hms(2025, 1, 26, 15, 0, 0).unwrap());

        let calculator = EventDurationCalculator::new(mock_clock);
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let (since, until) = calculator.get_duration(&jst, GetEventDuration::NextWeek);

        assert_eq!(since.format("%Y-%m-%d").to_string(), "2025-02-03");
        assert_eq!(until.format("%Y-%m-%d").to_string(), "2025-02-09");
    }
}
