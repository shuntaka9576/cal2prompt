use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;

pub fn to_utc_start_of_start_rfc3339(datetime_tz: DateTime<Tz>) -> String {
    let tz = datetime_tz.timezone();

    let local_start_naive_date_time = datetime_tz.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let local_start_date_time = tz
        .from_local_datetime(&local_start_naive_date_time)
        .unwrap();
    local_start_date_time.to_utc().to_rfc3339()
}

pub fn intersection_days(
    start1: NaiveDate,
    end1: NaiveDate,
    start2: NaiveDate,
    end2: NaiveDate,
) -> Vec<NaiveDate> {
    let intersection_start = start1.max(start2);
    let intersection_end = end1.min(end2);

    if intersection_start > intersection_end {
        return vec![];
    }

    let mut days = Vec::new();
    let mut current = intersection_start;
    while current <= intersection_end {
        days.push(current);
        current = current.succ_opt().unwrap();
    }

    days
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};
    use chrono_tz::Tz;

    #[test]
    fn test_parse_local_date_to_utc_tokyo() -> anyhow::Result<()> {
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let naive_datetime = NaiveDate::from_ymd_opt(2023, 1, 2)
            .unwrap()
            .and_hms_opt(12, 34, 56)
            .unwrap();
        let local_datetime = tz.from_local_datetime(&naive_datetime).single().unwrap();

        let result = to_utc_start_of_start_rfc3339(local_datetime);

        let expected_str = "2023-01-01T15:00:00+00:00";

        assert_eq!(result, expected_str);

        Ok(())
    }

    #[test]
    fn test_parse_local_date_to_utc_los_angeles_summer_time() -> anyhow::Result<()> {
        let tz: Tz = "America/Los_Angeles".parse().unwrap();
        let naive_datetime = NaiveDate::from_ymd_opt(2023, 7, 10)
            .unwrap()
            .and_hms_opt(12, 34, 56)
            .unwrap();
        let local_datetime = tz.from_local_datetime(&naive_datetime).single().unwrap();

        let result = to_utc_start_of_start_rfc3339(local_datetime);

        let expected_str = "2023-07-10T07:00:00+00:00";

        assert_eq!(result, expected_str);

        Ok(())
    }
}
