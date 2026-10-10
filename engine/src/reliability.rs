//! Reliability metrics for a design: how often load goes unserved over a
//! year (loss-of-load probability and expectation), and how long the battery
//! alone can carry the worst month's load.
//!
//! Loss of load is taken from the same steady-state representative days as
//! the seasonal simulation, with the optional backup generator applied, so
//! the figures agree with the rest of the report. Each representative day
//! stands for every day of its month, so the metrics describe the typical
//! year, not a particular weather sequence. Autonomy is the storage-only
//! counterpart of the multi-day cloudy-spell check: no sun at all, the
//! battery starts full, and the worst month's load is repeated.

use crate::{
    dispatch_hours, simulate_steady_day, BatterySpec, DayLoad, GeneratorSpec, MonthlyFactors,
    Site, SolarArray, HOURS_PER_DAY, MONTHS_PER_YEAR, MONTH_DAYS,
};

/// Longest storage-only run reported, in days. Batteries that carry the load
/// longer than this report the cap.
pub const MAX_AUTONOMY_DAYS: f64 = 60.0;

/// Loss-of-load and autonomy figures for a design.
#[derive(Debug, Clone, PartialEq)]
pub struct ReliabilityReport {
    /// Loss-of-load probability: the share of the year's hours with load not
    /// served, after any generator, 0 to 1.
    pub lolp: f64,
    /// Hours per year with load not served.
    pub loss_of_load_hours_per_year: f64,
    /// Expected days per year with at least one unserved hour.
    pub lole_days_per_year: f64,
    /// Days the battery alone carries the worst month's load from full, with
    /// no sun, to the first unserved hour. Fractional; capped at
    /// [`MAX_AUTONOMY_DAYS`].
    pub autonomy_days: f64,
}

/// Loss-of-load and autonomy for `array`, `battery` and `base_load` at
/// `site`, under the monthly factors, with an optional backup generator.
pub fn reliability_report(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    generator: Option<&GeneratorSpec>,
) -> ReliabilityReport {
    let mut loss_hours = 0.0;
    let mut lole_days = 0.0;
    for m in 0..MONTHS_PER_YEAR {
        let array_m = SolarArray {
            cloud_factor: factors.cloud[m],
            ambient_celsius: factors.ambient_c[m],
            ..array.clone()
        };
        let load_m = base_load.scaled(factors.load[m]);
        let day = simulate_steady_day(site, &array_m, battery, &load_m, m);
        let unserved = day
            .hours
            .iter()
            .filter(|h| residual_unmet_kw(h.unmet_kw, generator) > 1e-9)
            .count();
        let days = f64::from(MONTH_DAYS[m]);
        loss_hours += unserved as f64 * days;
        if unserved > 0 {
            lole_days += days;
        }
    }

    // The worst month is the one with the largest load multiplier: the
    // battery has to carry that day's profile on its own.
    let worst = (0..MONTHS_PER_YEAR)
        .max_by(|a, b| factors.load[*a].total_cmp(&factors.load[*b]))
        .unwrap_or(0);
    let worst_profile = base_load.scaled(factors.load[worst]);

    let hours_per_year = (MONTH_DAYS.iter().sum::<u32>() as usize * HOURS_PER_DAY) as f64;
    ReliabilityReport {
        lolp: loss_hours / hours_per_year,
        loss_of_load_hours_per_year: loss_hours,
        lole_days_per_year: lole_days,
        autonomy_days: storage_autonomy_days(battery, &worst_profile.hourly_kw),
    }
}

/// Unmet load left after the generator has served what it can in an hour.
fn residual_unmet_kw(unmet_kw: f64, generator: Option<&GeneratorSpec>) -> f64 {
    let from_generator = generator.map_or(0.0, |g| unmet_kw.min(g.power_kw.max(0.0)));
    (unmet_kw - from_generator).max(0.0)
}

/// Full days of storage-only operation before the first unserved hour, with
/// the battery starting full and the sun absent. The fraction is the hour
/// within the failing day.
fn storage_autonomy_days(battery: &BatterySpec, profile_kw: &[f64]) -> f64 {
    let no_sun = vec![0.0; profile_kw.len()];
    let mut spec = battery.clone();
    spec.initial_soc = 1.0;
    for day in 0..MAX_AUTONOMY_DAYS as usize {
        let (hours, totals) = dispatch_hours(&no_sun, profile_kw, &spec);
        if let Some(first) = hours.iter().position(|h| h.unmet_kw > 1e-9) {
            return day as f64 + first as f64 / HOURS_PER_DAY as f64;
        }
        spec.initial_soc = totals.end_soc;
    }
    MAX_AUTONOMY_DAYS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> MonthlyFactors {
        MonthlyFactors::flat()
    }

    fn constant(kw: f64) -> DayLoad {
        DayLoad {
            hourly_kw: vec![kw; HOURS_PER_DAY],
        }
    }

    #[test]
    fn no_supply_means_load_is_almost_always_lost() {
        let report = reliability_report(
            &Site::default(),
            &SolarArray {
                capacity_kw: 0.05,
                ..SolarArray::default()
            },
            &BatterySpec {
                capacity_kwh: 0.0,
                power_kw: 0.0,
                ..BatterySpec::default()
            },
            &constant(1.0),
            &flat(),
            None,
        );
        assert!(report.lolp > 0.95, "{}", report.lolp);
        assert!((report.lole_days_per_year - 365.0).abs() < 1e-9);
        assert_eq!(report.autonomy_days, 0.0);
    }

    #[test]
    fn a_generous_design_has_no_loss_of_load() {
        let report = reliability_report(
            &Site::default(),
            &SolarArray {
                capacity_kw: 20.0,
                ..SolarArray::default()
            },
            &BatterySpec {
                capacity_kwh: 50.0,
                power_kw: 25.0,
                ..BatterySpec::default()
            },
            &constant(1.0),
            &flat(),
            None,
        );
        assert_eq!(report.loss_of_load_hours_per_year, 0.0);
        assert_eq!(report.lole_days_per_year, 0.0);
        assert_eq!(report.lolp, 0.0);
    }

    #[test]
    fn generator_removes_loss_of_load() {
        let generator = GeneratorSpec {
            power_kw: 5.0,
            ..GeneratorSpec::default()
        };
        let without = reliability_report(
            &Site::default(),
            &SolarArray {
                capacity_kw: 0.05,
                ..SolarArray::default()
            },
            &BatterySpec {
                capacity_kwh: 0.0,
                power_kw: 0.0,
                ..BatterySpec::default()
            },
            &constant(1.0),
            &flat(),
            None,
        );
        let with = reliability_report(
            &Site::default(),
            &SolarArray {
                capacity_kw: 0.05,
                ..SolarArray::default()
            },
            &BatterySpec {
                capacity_kwh: 0.0,
                power_kw: 0.0,
                ..BatterySpec::default()
            },
            &constant(1.0),
            &flat(),
            Some(&generator),
        );
        assert!(without.lolp > 0.95);
        assert_eq!(with.lolp, 0.0);
    }

    #[test]
    fn autonomy_matches_usable_energy_over_load() {
        // 10 kWh, 10% floor, lossless, 1 kW flat: nine usable kWh last nine
        // hours, so the first unserved hour is hour 9 of day 0.
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.1,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let days = storage_autonomy_days(&battery, &constant(1.0).hourly_kw);
        assert!((days - 9.0 / 24.0).abs() < 1e-9, "{days}");
    }

    #[test]
    fn autonomy_is_capped() {
        let battery = BatterySpec {
            capacity_kwh: 500.0,
            power_kw: 500.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.0,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let days = storage_autonomy_days(&battery, &constant(0.1).hourly_kw);
        assert_eq!(days, MAX_AUTONOMY_DAYS);
    }

    #[test]
    fn report_includes_the_reliability_section_when_given() {
        use crate::{design_report_markdown, simulate_seasonal, ReportExtras};
        let site = Site::default();
        let array = SolarArray::default();
        let battery = BatterySpec::default();
        let load = DayLoad::default();
        let factors = MonthlyFactors::default();
        let seasonal = simulate_seasonal(&site, &array, &battery, &load, &factors);
        let rel = reliability_report(&site, &array, &battery, &load, &factors, None);
        let with = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras {
                reliability: Some(&rel),
                ..ReportExtras::default()
            },
        );
        assert!(with.contains("## Reliability"));
        assert!(with.contains("Loss-of-load probability (LOLP)"));
        let without = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras::default(),
        );
        assert!(!without.contains("## Reliability"));
    }

    #[test]
    fn residual_unmet_respects_generator_power() {
        let generator = GeneratorSpec {
            power_kw: 2.0,
            ..GeneratorSpec::default()
        };
        assert_eq!(residual_unmet_kw(1.5, Some(&generator)), 0.0);
        assert!((residual_unmet_kw(3.0, Some(&generator)) - 1.0).abs() < 1e-12);
        assert_eq!(residual_unmet_kw(3.0, None), 3.0);
    }
}
