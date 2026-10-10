//! CSV import for measured or typical-year data.
//!
//! Two kinds of file are accepted:
//!
//! - **Load** — 24 hourly kW values (one representative day), or 8760
//!   hourly values (one year, January 1 00:00 onward). A year becomes the
//!   hour-of-day mean profile plus a multiplier per month, which is exactly
//!   the shape the seasonal simulation already consumes.
//! - **Weather** — 8760 hourly rows of global horizontal, direct normal and
//!   diffuse horizontal irradiance (W/m², in that column order), e.g. a
//!   PVGIS or NIWA typical meteorological year. These are not fed through
//!   the simulation directly. Instead, each month's measured AC yield is
//!   divided by the clear-sky yield to give a monthly cloud factor, so the
//!   existing seasonal model reproduces the measured weather.
//!
//! Both readers take the **last** N numeric columns of each row, so a
//! leading timestamp or index column is ignored. A first row that is not
//! numeric is treated as a header. Blank lines and `#` comments are skipped.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use tpt_nrg_solar::Irradiance;

use crate::{
    planes_for, DayLoad, Site, SolarArray, HOURS_PER_DAY, MONTHS_PER_YEAR, MONTH_DAYS,
    REPRESENTATIVE_YEAR,
};

/// Hours in a non-leap year — the length a full-year import must have.
pub const HOURS_PER_YEAR: usize = 8760;

/// Why an import was refused. Messages are phrased for direct display.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportError {
    /// The file held no data rows.
    Empty,
    /// A row, counted from 1, had a field that is not a number.
    NotANumber {
        /// 1-based line number in the file.
        line: usize,
        /// The offending line, trimmed.
        text: String,
    },
    /// A row had fewer numeric columns than the file format needs.
    MissingColumns {
        /// 1-based line number in the file.
        line: usize,
        /// Columns the format needs per row.
        expected: usize,
    },
    /// A value was negative or not finite.
    InvalidValue {
        /// 1-based line number in the file.
        line: usize,
    },
    /// The number of data rows is not one the format accepts.
    RowCount {
        /// Rows found.
        found: usize,
        /// Accepted counts, in words.
        expected: &'static str,
    },
}

impl core::fmt::Display for ImportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "The file has no data rows."),
            Self::NotANumber { line, text } => {
                write!(f, "Line {line} is not numeric: \"{text}\".")
            }
            Self::MissingColumns { line, expected } => {
                write!(f, "Line {line} needs {expected} numeric columns.")
            }
            Self::InvalidValue { line } => {
                write!(f, "Line {line} has a negative or non-finite value.")
            }
            Self::RowCount { found, expected } => {
                write!(f, "Found {found} data rows; expected {expected}.")
            }
        }
    }
}

/// Parses `text` into rows of the last `columns` numeric fields each.
fn parse_rows(text: &str, columns: usize) -> Result<Vec<Vec<f64>>, ImportError> {
    let mut rows = Vec::new();
    let mut header_allowed = true;
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() < columns {
            if header_allowed && rows.is_empty() {
                header_allowed = false;
                continue;
            }
            return Err(ImportError::MissingColumns {
                line: line_no,
                expected: columns,
            });
        }
        let tail = &fields[fields.len() - columns..];
        let parsed: Result<Vec<f64>, _> = tail.iter().map(|f| f.parse::<f64>()).collect();
        match parsed {
            Ok(values) => {
                if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err(ImportError::InvalidValue { line: line_no });
                }
                rows.push(values);
                header_allowed = false;
            }
            Err(_) if header_allowed && rows.is_empty() => {
                // The first non-numeric row is a column header.
                header_allowed = false;
            }
            Err(_) => {
                return Err(ImportError::NotANumber {
                    line: line_no,
                    text: line.to_string(),
                });
            }
        }
    }
    if rows.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(rows)
}

/// A parsed load file.
#[derive(Debug, Clone)]
pub struct LoadImport {
    /// Hourly kW profile: the file as given for one day, or the hour-of-day
    /// mean over the year.
    pub day: DayLoad,
    /// Monthly load multipliers relative to the annual mean day (January
    /// first). `None` for a single-day file, which has no seasonal shape.
    pub monthly_factors: Option<[f64; MONTHS_PER_YEAR]>,
}

/// Parses a load CSV: 24 hourly values (one day) or 8760 (one year).
///
/// # Errors
/// [`ImportError`] when the file is empty, a value is not a non-negative
/// number, or the row count is neither 24 nor 8760.
pub fn import_load_csv(text: &str) -> Result<LoadImport, ImportError> {
    let values: Vec<f64> = parse_rows(text, 1)?.into_iter().map(|r| r[0]).collect();
    match values.len() {
        HOURS_PER_DAY => Ok(LoadImport {
            day: DayLoad {
                hourly_kw: values,
            },
            monthly_factors: None,
        }),
        HOURS_PER_YEAR => Ok(year_load(&values)),
        found => Err(ImportError::RowCount {
            found,
            expected: "24 (one day) or 8760 (one year, hourly)",
        }),
    }
}

/// Collapses an 8760-hour load series into the mean day and monthly factors.
fn year_load(values: &[f64]) -> LoadImport {
    let days = HOURS_PER_YEAR / HOURS_PER_DAY;
    let mut hour_of_day = vec![0.0; HOURS_PER_DAY];
    for (i, kw) in values.iter().enumerate() {
        hour_of_day[i % HOURS_PER_DAY] += kw;
    }
    for total in &mut hour_of_day {
        *total /= days as f64;
    }

    let annual_mean_kw = values.iter().sum::<f64>() / HOURS_PER_YEAR as f64;
    let mut factors = [1.0; MONTHS_PER_YEAR];
    if annual_mean_kw > 1e-12 {
        let mut offset = 0usize;
        for (m, factor) in factors.iter_mut().enumerate() {
            let hours = MONTH_DAYS[m] as usize * HOURS_PER_DAY;
            let month: f64 = values[offset..offset + hours].iter().sum();
            *factor = (month / hours as f64) / annual_mean_kw;
            offset += hours;
        }
    }
    LoadImport {
        day: DayLoad {
            hourly_kw: hour_of_day,
        },
        monthly_factors: Some(factors),
    }
}

/// Parses an 8760-row weather CSV of GHI, DNI and DHI (W/m²), in that
/// column order.
///
/// # Errors
/// [`ImportError`] when a value is not a non-negative number, or the row
/// count is not 8760.
pub fn import_weather_csv(text: &str) -> Result<Vec<Irradiance>, ImportError> {
    let rows = parse_rows(text, 3)?;
    if rows.len() != HOURS_PER_YEAR {
        return Err(ImportError::RowCount {
            found: rows.len(),
            expected: "8760 (one year, hourly)",
        });
    }
    Ok(rows
        .into_iter()
        .map(|r| Irradiance {
            ghi_w_per_m2: r[0],
            dni_w_per_m2: r[1],
            dhi_w_per_m2: r[2],
        })
        .collect())
}

/// Start of local hour `hour` on day `day_of_year` (0-based, non-leap
/// calendar), as a UTC instant, for the site's fixed UTC offset.
fn local_hour_utc(site: &Site, day_of_year: usize, hour: usize) -> DateTime<Utc> {
    let date = NaiveDate::from_yo_opt(REPRESENTATIVE_YEAR, day_of_year as u32 + 1)
        .expect("day of year is within the representative year");
    let local = date
        .and_hms_opt(hour as u32, 0, 0)
        .expect("hour is within the day");
    let offset_secs = (site.timezone_offset_hours * 3600.0).round() as i64;
    Utc.from_utc_datetime(&(local - Duration::seconds(offset_secs)))
}

/// Monthly cloud factors that make the clear-sky model reproduce a measured
/// year's AC yield for `array` at `site`.
///
/// For each month, the measured hours are run through the PV model from
/// their GHI/DNI/DHI and divided by the same hours' clear-sky yield. The
/// result is a multiplier in `[0, 1]`: measured irradiance above the clear
/// sky (cloud enhancement, albedo) saturates at 1. The ratio is taken on AC
/// output, so inverter clipping is included; it is an energy-level
/// calibration, not an hour-by-hour match.
///
/// # Errors
/// [`ImportError::RowCount`] when `weather` does not hold 8760 hours.
pub fn cloud_factors_from_weather(
    site: &Site,
    array: &SolarArray,
    weather: &[Irradiance],
) -> Result<[f64; MONTHS_PER_YEAR], ImportError> {
    if weather.len() != HOURS_PER_YEAR {
        return Err(ImportError::RowCount {
            found: weather.len(),
            expected: "8760 (one year, hourly)",
        });
    }
    let (solar_model, plants) = planes_for(site, array);
    let mut factors = [1.0; MONTHS_PER_YEAR];
    let mut hour_index = 0usize;
    let mut day_of_year = 0usize;
    for (m, factor) in factors.iter_mut().enumerate() {
        let mut measured_mw = 0.0;
        let mut clear_mw = 0.0;
        for _ in 0..MONTH_DAYS[m] {
            for hour in 0..HOURS_PER_DAY {
                // Sample the sun at the hour's midpoint, as the clear-sky
                // profile does, so the two sides see the same geometry.
                let instant = local_hour_utc(site, day_of_year, hour) + Duration::minutes(30);
                let pos = solar_model.solar_position(instant);
                for plant in &plants {
                    measured_mw +=
                        plant.output_from_irrad(&weather[hour_index], &pos).ac_power_mw;
                    clear_mw += plant.output_clearsky(&pos).ac_power_mw;
                }
                hour_index += 1;
            }
            day_of_year += 1;
        }
        *factor = if clear_mw > 1e-12 {
            (measured_mw / clear_mw).clamp(0.0, 1.0)
        } else {
            // No clear-sky yield (polar night): the factor has no effect.
            1.0
        };
    }
    Ok(factors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(values: &[f64]) -> String {
        values.iter().map(|v| format!("{v}\n")).collect()
    }

    #[test]
    fn single_day_load_is_taken_as_given() {
        let values: Vec<f64> = (0..HOURS_PER_DAY).map(|h| h as f64 * 0.1).collect();
        let import = import_load_csv(&csv(&values)).expect("24 rows parse");
        assert_eq!(import.day.hourly_kw, values);
        assert!(import.monthly_factors.is_none());
    }

    #[test]
    fn header_timestamp_and_comments_are_ignored() {
        let mut text = String::from("# hourly load, kW\ntimestamp,load_kw\n");
        for h in 0..HOURS_PER_DAY {
            text.push_str(&format!("2026-01-01 {h:02}:00,{}\n", 1.5));
        }
        let import = import_load_csv(&text).expect("header and timestamps are skipped");
        assert!(import.day.hourly_kw.iter().all(|kw| (kw - 1.5).abs() < 1e-12));
    }

    #[test]
    fn wrong_row_counts_are_refused() {
        let err = import_load_csv(&csv(&[1.0; 23])).unwrap_err();
        assert_eq!(
            err,
            ImportError::RowCount {
                found: 23,
                expected: "24 (one day) or 8760 (one year, hourly)",
            }
        );
        assert_eq!(import_load_csv("header only\n").unwrap_err(), ImportError::Empty);
    }

    #[test]
    fn bad_values_report_their_line() {
        let text = "1\n2\nabc\n4\n";
        assert_eq!(
            parse_rows(text, 1).unwrap_err(),
            ImportError::NotANumber {
                line: 3,
                text: "abc".to_string(),
            }
        );
        let negative = "1\n-2\n";
        assert_eq!(
            parse_rows(negative, 1).unwrap_err(),
            ImportError::InvalidValue { line: 2 }
        );
        let short = "1,2\n3\n";
        assert_eq!(
            parse_rows(short, 2).unwrap_err(),
            ImportError::MissingColumns {
                line: 2,
                expected: 2,
            }
        );
    }

    #[test]
    fn year_load_gives_mean_day_and_monthly_factors() {
        // January runs at 2 kW, the rest of the year at 1 kW.
        let jan_hours = MONTH_DAYS[0] as usize * HOURS_PER_DAY;
        let values: Vec<f64> = (0..HOURS_PER_YEAR)
            .map(|i| if i < jan_hours { 2.0 } else { 1.0 })
            .collect();
        let import = import_load_csv(&csv(&values)).expect("8760 rows parse");
        assert_eq!(import.day.hourly_kw.len(), HOURS_PER_DAY);
        let factors = import.monthly_factors.expect("a year has monthly factors");

        let annual_mean = values.iter().sum::<f64>() / HOURS_PER_YEAR as f64;
        assert!((factors[0] - 2.0 / annual_mean).abs() < 1e-12);
        assert!((factors[1] - 1.0 / annual_mean).abs() < 1e-12);
        // Day-weighted, the factors average to exactly one.
        let weighted: f64 = (0..MONTHS_PER_YEAR)
            .map(|m| factors[m] * f64::from(MONTH_DAYS[m]))
            .sum::<f64>()
            / 365.0;
        assert!((weighted - 1.0).abs() < 1e-12);
        // The mean day carries the annual mean energy per day.
        let daily: f64 = import.day.hourly_kw.iter().sum();
        assert!((daily - annual_mean * HOURS_PER_DAY as f64).abs() < 1e-9);
    }

    #[test]
    fn weather_needs_three_columns_and_a_full_year() {
        let row = "ghi,dni,dhi\n600,500,100\n";
        let short = import_weather_csv(row).unwrap_err();
        assert_eq!(
            short,
            ImportError::RowCount {
                found: 1,
                expected: "8760 (one year, hourly)",
            }
        );
        let mut text = String::from("ghi,dni,dhi\n");
        for _ in 0..HOURS_PER_YEAR {
            text.push_str("0,0,0\n");
        }
        let weather = import_weather_csv(&text).expect("a full year parses");
        assert_eq!(weather.len(), HOURS_PER_YEAR);
        assert_eq!(weather[0].ghi_w_per_m2, 0.0);
    }

    #[test]
    fn no_sunshine_means_no_cloud_yield() {
        let dark = vec![
            Irradiance {
                ghi_w_per_m2: 0.0,
                dni_w_per_m2: 0.0,
                dhi_w_per_m2: 0.0,
            };
            HOURS_PER_YEAR
        ];
        let factors =
            cloud_factors_from_weather(&Site::default(), &SolarArray::default(), &dark).unwrap();
        assert!(factors.iter().all(|f| *f == 0.0), "{factors:?}");
    }

    #[test]
    fn dimmer_weather_gives_lower_cloud_factors() {
        let bright: Vec<Irradiance> = (0..HOURS_PER_YEAR)
            .map(|i| {
                let hour = i % HOURS_PER_DAY;
                let sun = if (8..=16).contains(&hour) { 1.0 } else { 0.0 };
                Irradiance {
                    ghi_w_per_m2: 900.0 * sun,
                    dni_w_per_m2: 700.0 * sun,
                    dhi_w_per_m2: 200.0 * sun,
                }
            })
            .collect();
        let dim: Vec<Irradiance> = bright
            .iter()
            .map(|w| Irradiance {
                ghi_w_per_m2: w.ghi_w_per_m2 * 0.5,
                dni_w_per_m2: w.dni_w_per_m2 * 0.5,
                dhi_w_per_m2: w.dhi_w_per_m2 * 0.5,
            })
            .collect();
        let site = Site::default();
        let array = SolarArray {
            cloud_factor: 1.0,
            ..SolarArray::default()
        };
        let hi = cloud_factors_from_weather(&site, &array, &bright).unwrap();
        let lo = cloud_factors_from_weather(&site, &array, &dim).unwrap();
        for m in 0..MONTHS_PER_YEAR {
            assert!((0.0..=1.0).contains(&hi[m]), "month {m}: {}", hi[m]);
            assert!(lo[m] < hi[m], "month {m}: dim {} vs bright {}", lo[m], hi[m]);
        }
    }

    #[test]
    fn weather_length_is_checked_for_calibration() {
        let err = cloud_factors_from_weather(&Site::default(), &SolarArray::default(), &[])
            .unwrap_err();
        assert!(matches!(err, ImportError::RowCount { found: 0, .. }));
    }
}
