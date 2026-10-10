//! Inverter sizing check: does the AC rating cover the peak demand, and how
//! much PV energy does the DC/AC ratio throw away by clipping?
//!
//! The rating is the array's DC nameplate divided by its DC/AC ratio. The
//! clipping estimate runs the same hourly PV model as the seasonal
//! simulation, month by month with that month's cloud and ambient factors,
//! and scales each representative day by its month length.
//!
//! The check is advisory. It does not limit the dispatch: a real inverter
//! would cap the PV power the simulation credits, and the clipped energy
//! reported here is the cost of that cap.

use crate::{pv_day, Site, SolarArray, DayLoad, MonthlyFactors, MONTHS_PER_YEAR, MONTH_DAYS};

/// Inverter rating, peak-demand coverage and annual clipping for a design.
#[derive(Debug, Clone, PartialEq)]
pub struct InverterReport {
    /// Installed DC capacity, kWp.
    pub dc_kwp: f64,
    /// DC-to-AC ratio the array is configured with.
    pub dc_ac_ratio: f64,
    /// Inverter AC rating, kW (DC capacity divided by the ratio).
    pub ac_rating_kw: f64,
    /// Highest hourly demand across the year, kW: the base profile's peak
    /// scaled by the largest monthly load factor.
    pub peak_demand_kw: f64,
    /// Whether the AC rating covers the peak demand. Only the PV inverter is
    /// counted; a separate battery inverter is not.
    pub covers_peak: bool,
    /// PV energy the inverter clips in a year, kWh.
    pub clipped_kwh_per_year: f64,
    /// Clipped share of the PV energy the array could have delivered
    /// unclipped, 0 to 1.
    pub clipped_fraction: f64,
}

/// Builds the [`InverterReport`] for `array` at `site` under the monthly
/// factors and the base load.
pub fn inverter_check(
    site: &Site,
    array: &SolarArray,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
) -> InverterReport {
    let mut clipped_kwh = 0.0;
    let mut potential_kwh = 0.0;
    for m in 0..MONTHS_PER_YEAR {
        let array_m = SolarArray {
            cloud_factor: factors.cloud[m],
            ambient_celsius: factors.ambient_c[m],
            ..array.clone()
        };
        // Hourly kW over one hour is kWh.
        let (ac, clip) = pv_day(site, &array_m, m);
        let days = f64::from(MONTH_DAYS[m]);
        let day_clipped: f64 = clip.iter().sum();
        let day_ac: f64 = ac.iter().sum();
        clipped_kwh += day_clipped * days;
        potential_kwh += (day_ac + day_clipped) * days;
    }

    let ac_rating_kw = array.capacity_kw / array.dc_ac_ratio;
    let peak_demand_kw =
        base_load.peak_kw() * factors.load.iter().copied().fold(0.0, f64::max);
    InverterReport {
        dc_kwp: array.capacity_kw,
        dc_ac_ratio: array.dc_ac_ratio,
        ac_rating_kw,
        peak_demand_kw,
        covers_peak: ac_rating_kw + 1e-9 >= peak_demand_kw,
        clipped_kwh_per_year: clipped_kwh,
        clipped_fraction: if potential_kwh > 1e-9 {
            clipped_kwh / potential_kwh
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HOURS_PER_DAY;

    fn clear_year() -> MonthlyFactors {
        MonthlyFactors::flat()
    }

    fn array_with_ratio(ratio: f64) -> SolarArray {
        SolarArray {
            capacity_kw: 5.0,
            dc_ac_ratio: ratio,
            ..SolarArray::default()
        }
    }

    #[test]
    fn rating_is_dc_over_ratio() {
        let report = inverter_check(
            &Site::default(),
            &array_with_ratio(1.25),
            &DayLoad::default(),
            &clear_year(),
        );
        assert!((report.ac_rating_kw - 4.0).abs() < 1e-12);
        assert_eq!(report.dc_kwp, 5.0);
        assert_eq!(report.dc_ac_ratio, 1.25);
    }

    #[test]
    fn no_oversizing_clips_only_a_sliver() {
        // At a 1.0 ratio the inverter matches the array. Clear-sky irradiance
        // on a tilted plane can still exceed the 1000 W/m² rating, so a little
        // clipping remains, but it stays small next to heavy oversizing.
        let report = inverter_check(
            &Site::default(),
            &array_with_ratio(1.0),
            &DayLoad::default(),
            &clear_year(),
        );
        assert!(report.clipped_fraction < 0.1, "{}", report.clipped_fraction);
        let heavy = inverter_check(
            &Site::default(),
            &array_with_ratio(1.6),
            &DayLoad::default(),
            &clear_year(),
        );
        assert!(report.clipped_fraction < heavy.clipped_fraction);
    }

    #[test]
    fn oversizing_clips_more_energy() {
        let site = Site::default();
        let load = DayLoad::default();
        let factors = clear_year();
        let gentle = inverter_check(&site, &array_with_ratio(1.2), &load, &factors);
        let heavy = inverter_check(&site, &array_with_ratio(1.6), &load, &factors);
        assert!(heavy.clipped_kwh_per_year > gentle.clipped_kwh_per_year);
        assert!(heavy.clipped_fraction > gentle.clipped_fraction);
        assert!((0.0..=1.0).contains(&heavy.clipped_fraction));
    }

    #[test]
    fn peak_coverage_uses_the_rating_against_peak_demand() {
        let site = Site::default();
        let factors = clear_year();
        // A 1.5 kW peak against a 4.17 kW rating is covered.
        let small = inverter_check(&site, &array_with_ratio(1.2), &DayLoad::default(), &factors);
        assert!(small.covers_peak);
        assert!((small.peak_demand_kw - DayLoad::default().peak_kw()).abs() < 1e-9);
        // A 10 kW peak is not.
        let big_load = DayLoad {
            hourly_kw: vec![10.0; HOURS_PER_DAY],
        };
        let big = inverter_check(&site, &array_with_ratio(1.2), &big_load, &factors);
        assert!(!big.covers_peak);
        assert!((big.peak_demand_kw - 10.0).abs() < 1e-9);
    }

    #[test]
    fn dc_ac_ratio_is_validated() {
        assert!(array_with_ratio(1.2).validate().is_empty());
        assert_eq!(array_with_ratio(0.2).validate().len(), 1);
        assert_eq!(array_with_ratio(3.0).validate().len(), 1);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn report_includes_the_inverter_section_when_given() {
        use crate::{design_report_markdown, simulate_seasonal, BatterySpec, ReportExtras};
        let site = Site::default();
        let array = array_with_ratio(1.6);
        let load = DayLoad::default();
        let factors = clear_year();
        let battery = BatterySpec::default();
        let seasonal = simulate_seasonal(&site, &array, &battery, &load, &factors);
        let check = inverter_check(&site, &array, &load, &factors);

        let with = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras {
                inverter: Some(&check),
                ..ReportExtras::default()
            },
        );
        assert!(with.contains("## Inverter"));
        assert!(with.contains("DC/AC ratio 1.60"));
        let without = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras::default(),
        );
        assert!(!without.contains("## Inverter"));
    }

    #[test]
    fn peak_demand_follows_the_largest_monthly_factor() {
        let mut factors = MonthlyFactors::flat();
        factors.load[6] = 2.0;
        let report = inverter_check(
            &Site::default(),
            &array_with_ratio(1.2),
            &DayLoad::default(),
            &factors,
        );
        assert!((report.peak_demand_kw - 2.0 * DayLoad::default().peak_kw()).abs() < 1e-9);
    }
}
