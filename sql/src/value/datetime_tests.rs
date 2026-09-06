// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::sync::Arc;

use super::{FormatOptions, Value};
use arrow_array::{Array, Decimal128Array, TimestampMicrosecondArray};
use arrow_schema::{DataType as ArrowDataType, Field, TimeUnit};
use chrono::{DateTime, FixedOffset, NaiveDateTime};
use chrono_tz::Tz;
use databend_client::schema::{DataType, ARROW_EXT_TYPE_TIMESTAMP_TIMEZONE, EXTENSION_KEY};
use databend_client::ResultFormatSettings;

fn decode(ty: &DataType, text: &str, tz: Tz) -> Value {
    Value::try_from((ty, text.to_owned(), &tz)).unwrap()
}

fn assert_format(value: &Value, text: &str) {
    assert_eq!(value.to_string(), text);
    assert_eq!(String::try_from(value.clone()).unwrap(), text);
    assert_eq!(value.to_sql_string(), format!("'{text}'"));
    let options = FormatOptions {
        true_string: b"true",
        false_string: b"false",
        float_options: Default::default(),
    };
    assert_eq!(value.to_string_with_options(&options), text);
}

#[test]
fn text_datetime_boundaries() {
    for (ty, text, micros) in [
        (
            DataType::Timestamp,
            "9999-12-31 23:59:59.999999",
            253_402_300_799_999_999,
        ),
        (
            DataType::Timestamp,
            "+10000-01-01 00:00:00.000000",
            253_402_300_800_000_000,
        ),
        (
            DataType::Timestamp,
            "+11000-12-31 23:59:59.999999",
            284_990_831_999_999_999,
        ),
        (
            DataType::TimestampTz,
            "9999-12-31 23:59:59.999999 +0530",
            253_402_280_999_999_999,
        ),
        (
            DataType::TimestampTz,
            "+11000-12-31 23:59:59.999999 -0800",
            284_990_860_799_999_999,
        ),
    ] {
        let value = decode(&ty, text, Tz::UTC);
        assert_format(&value, text);
        if matches!(ty, DataType::Timestamp) {
            assert_eq!(
                DateTime::<Tz>::try_from(value.clone())
                    .unwrap()
                    .timestamp_micros(),
                micros
            );
            let naive = NaiveDateTime::try_from(value.clone()).unwrap();
            assert_eq!(naive.and_utc().timestamp_micros(), micros);
            assert_eq!(Value::from(naive), value);
            assert_eq!(Value::from(&naive), value);
        } else {
            assert_eq!(
                DateTime::<FixedOffset>::try_from(value.clone())
                    .unwrap()
                    .timestamp_micros(),
                micros
            );
        }
        // Nested timestamps have a separate text decoder.
        assert_eq!(
            decode(
                &DataType::Array(Box::new(ty)),
                &format!("['{text}']"),
                Tz::UTC
            ),
            Value::Array(vec![value])
        );
    }
}

#[test]
fn session_timezone_and_dst() {
    for (tz, text, utc, formatted) in [
        (
            Tz::Asia__Shanghai,
            "+11001-01-01 07:59:59.999999",
            "+11000-12-31 23:59:59.999999",
            "+11001-01-01 07:59:59.999999",
        ),
        // Folds choose the earlier instant; gaps shift by the offset change.
        (
            Tz::America__New_York,
            "2024-11-03 01:30:00.123456",
            "2024-11-03 05:30:00.123456",
            "2024-11-03 01:30:00.123456",
        ),
        (
            Tz::America__New_York,
            "2024-03-10 02:30:00.123456",
            "2024-03-10 07:30:00.123456",
            "2024-03-10 03:30:00.123456",
        ),
        (
            Tz::Australia__Lord_Howe,
            "2024-10-06 02:15:00.000000",
            "2024-10-05 15:45:00.000000",
            "2024-10-06 02:45:00.000000",
        ),
        (
            Tz::Pacific__Apia,
            "2011-12-30 12:00:00.000000",
            "2011-12-30 22:00:00.000000",
            "2011-12-31 12:00:00.000000",
        ),
    ] {
        let value = decode(&DataType::Timestamp, text, tz);
        assert_eq!(value.to_string(), formatted);
        let dt = DateTime::<Tz>::try_from(value.clone()).unwrap();
        assert_eq!(dt.timezone(), tz);
        assert_eq!(
            dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
            utc
        );
        assert_eq!(NaiveDateTime::try_from(value).unwrap(), dt.naive_utc());
    }
    // Explicit offsets must ignore the session timezone, even in a gap.
    let text = "2024-03-10 02:30:00.123456 +0000";
    let value = decode(&DataType::TimestampTz, text, Tz::America__New_York);
    assert_eq!(
        DateTime::<FixedOffset>::try_from(value)
            .unwrap()
            .timestamp_micros(),
        1_710_037_800_123_456
    );
}

fn arrow_timestamp(micros: i64, tz: Tz) -> crate::error::Result<Value> {
    let field = Field::new(
        "ts",
        ArrowDataType::Timestamp(TimeUnit::Microsecond, None),
        true,
    );
    let array: Arc<dyn Array> = Arc::new(TimestampMicrosecondArray::from(vec![micros]));
    let settings = ResultFormatSettings {
        timezone: tz,
        ..Default::default()
    };
    Value::try_from((&field, &array, 0, &settings))
}

fn arrow_timestamp_tz(micros: i64, offset: i32) -> crate::error::Result<Value> {
    // The lower 64 bits are signed microseconds; the upper bits store the offset.
    let packed = ((offset as i128) << 64) | (micros as u64 as i128);
    let array: Arc<dyn Array> = Arc::new(Decimal128Array::from(vec![packed]));
    let field = Field::new("z", array.data_type().clone(), true).with_metadata(HashMap::from([(
        EXTENSION_KEY.to_owned(),
        ARROW_EXT_TYPE_TIMESTAMP_TIMEZONE.to_owned(),
    )]));
    Value::try_from((&field, &array, 0, &ResultFormatSettings::default()))
}

#[test]
fn arrow_preserves_datetime_instants() {
    for (micros, tz, offset) in [
        (-1, Tz::UTC, -28800),
        (253_402_300_799_999_999, Tz::UTC, 0),
        (284_990_831_999_999_999, Tz::Asia__Shanghai, 19800),
    ] {
        let dt = DateTime::<Tz>::try_from(arrow_timestamp(micros, tz).unwrap()).unwrap();
        assert_eq!(dt.timestamp_micros(), micros);
        assert_eq!(dt.timezone(), tz);
        let dt =
            DateTime::<FixedOffset>::try_from(arrow_timestamp_tz(micros, offset).unwrap()).unwrap();
        assert_eq!(dt.timestamp_micros(), micros);
        assert_eq!(dt.offset().local_minus_utc(), offset);
    }
    // Arrow must preserve both instants of a repeated local time.
    for micros in [1_730_611_800_000_000, 1_730_615_400_000_000] {
        let value = arrow_timestamp(micros, Tz::America__New_York).unwrap();
        assert_eq!(value.to_string(), "2024-11-03 01:30:00.000000");
        assert_eq!(
            DateTime::<Tz>::try_from(value).unwrap().timestamp_micros(),
            micros
        );
    }
    // Removing the clamp must return errors for unrepresentable wire values.
    for micros in [i64::MIN, i64::MAX] {
        assert!(arrow_timestamp(micros, Tz::UTC).is_err());
        assert!(arrow_timestamp_tz(micros, 0).is_err());
    }
    for offset in [-86400, 86400] {
        assert!(arrow_timestamp_tz(0, offset).is_err());
    }
}
