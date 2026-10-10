//! Grid connection, tariffs and export: what the design costs to run when a
//! grid connection is available, and what the grid does for it.
//!
//! The grid works as a backstop and a sink. It imports energy the PV and
//! battery leave unmet (up to an import limit) and exports surplus the
//! battery cannot store (up to an export limit). It never charges the
//! battery from the grid, so the design's own storage stays what is being
//! sized. Tariffs are flat: there is no time-of-use, demand charge or
//! monthly netting. Export at a credit equal to the import price models net
//! metering; a lower credit models a feed-in tariff.
//!
//! The bill compares two cases over the typical year: all load bought from
//! the grid (no system), and the load served by the system with the grid
//! covering the rest (with system). Both pay the same daily charge.

use crate::{
    simulate_steady_day, BatterySpec, DayLoad, HourPoint, MonthlyFactors, Site, SolarArray,
    MONTHS_PER_YEAR, MONTH_DAYS,
};

/// Tariff and connection limits for a grid-connected design.
#[derive(Debug, Clone, PartialEq)]
pub struct GridTariff {
    /// Price paid for each kWh imported, USD.
    pub import_usd_per_kwh: f64,
    /// Credit received for each kWh exported, USD. Equal to the import price
    /// for net metering.
    pub export_usd_per_kwh: f64,
    /// Fixed connection charge, USD per day.
    pub daily_charge_usd: f64,
    /// Largest power the grid can supply, kW.
    pub import_limit_kw: f64,
    /// Largest power the design can export, kW.
    pub export_limit_kw: f64,
}

impl Default for GridTariff {
    /// A generic flat tariff with net-metering-style export at a lower credit.
    fn default() -> Self {
        Self {
            import_usd_per_kwh: 0.30,
            export_usd_per_kwh: 0.08,
            daily_charge_usd: 0.80,
            import_limit_kw: 15.0,
            export_limit_kw: 5.0,
        }
    }
}

impl GridTariff {
    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.0..=5.0).contains(&self.import_usd_per_kwh) {
            issues.push("Import price must be between 0 and 5 $/kWh.".to_string());
        }
        if !(0.0..=5.0).contains(&self.export_usd_per_kwh) {
            issues.push("Export credit must be between 0 and 5 $/kWh.".to_string());
        }
        if !(0.0..=10.0).contains(&self.daily_charge_usd) {
            issues.push("Daily charge must be between 0 and 10 $/day.".to_string());
        }
        if !(0.0..=10_000.0).contains(&self.import_limit_kw) {
            issues.push("Import limit must be between 0 and 10,000 kW.".to_string());
        }
        if !(0.0..=10_000.0).contains(&self.export_limit_kw) {
            issues.push("Export limit must be between 0 and 10,000 kW.".to_string());
        }
        issues
    }
}

/// Energy flows over a period once the grid has been applied, kWh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridEnergy {
    /// Energy bought from the grid.
    pub imported_kwh: f64,
    /// Energy sold to the grid.
    pub exported_kwh: f64,
    /// Load still unserved after the import limit.
    pub unmet_kwh: f64,
    /// Surplus still lost after the export limit.
    pub curtailed_kwh: f64,
}

/// Applies the grid to one day's hourly balance: unmet load is imported up
/// to the import limit, and curtailed surplus is exported up to the export
/// limit. Each hour's kW is the kWh for that hour.
pub fn grid_flows(hours: &[HourPoint], tariff: &GridTariff) -> GridEnergy {
    let import_limit = tariff.import_limit_kw.max(0.0);
    let export_limit = tariff.export_limit_kw.max(0.0);
    let mut energy = GridEnergy::default();
    for h in hours {
        let imported = h.unmet_kw.min(import_limit).max(0.0);
        let exported = h.curtailed_kw.min(export_limit).max(0.0);
        energy.imported_kwh += imported;
        energy.unmet_kwh += h.unmet_kw - imported;
        energy.exported_kwh += exported;
        energy.curtailed_kwh += h.curtailed_kw - exported;
    }
    energy
}

/// The year's grid energy and bills for a design.
#[derive(Debug, Clone, PartialEq)]
pub struct GridYear {
    /// Annual load, kWh.
    pub load_kwh: f64,
    /// Annual energy bought from the grid, kWh.
    pub imported_kwh: f64,
    /// Annual energy sold to the grid, kWh.
    pub exported_kwh: f64,
    /// Annual load still unserved after the import limit, kWh.
    pub unmet_kwh: f64,
    /// Annual surplus lost after the export limit, kWh.
    pub curtailed_kwh: f64,
    /// Annual bill buying all the load from the grid (no system), USD.
    pub bill_without_system_usd: f64,
    /// Annual bill with the system and the grid covering the rest, USD.
    pub bill_with_system_usd: f64,
}

impl GridYear {
    /// Annual saving from the system, USD. Negative if the system costs more
    /// than it saves (for example, when exports earn less than imports cost).
    pub fn savings_usd(&self) -> f64 {
        self.bill_without_system_usd - self.bill_with_system_usd
    }

    /// Simple payback on `capex_usd`, in years. `None` when the system saves
    /// nothing or the capex is zero.
    pub fn simple_payback_years(&self, capex_usd: f64) -> Option<f64> {
        let savings = self.savings_usd();
        if savings > 1e-9 && capex_usd > 0.0 {
            Some(capex_usd / savings)
        } else {
            None
        }
    }
}

/// Simulates the typical year with the grid connected and prices the energy.
/// Uses the same steady representative days as the seasonal run.
pub fn grid_year(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    tariff: &GridTariff,
) -> GridYear {
    let mut total = GridEnergy::default();
    let mut load_kwh = 0.0;
    for m in 0..MONTHS_PER_YEAR {
        let array_m = SolarArray {
            cloud_factor: factors.cloud[m],
            ambient_celsius: factors.ambient_c[m],
            ..array.clone()
        };
        let load_m = base_load.scaled(factors.load[m]);
        let day = simulate_steady_day(site, &array_m, battery, &load_m, m);
        let days = f64::from(MONTH_DAYS[m]);
        let flows = grid_flows(&day.hours, tariff);
        load_kwh += day.totals.load_kwh * days;
        total.imported_kwh += flows.imported_kwh * days;
        total.exported_kwh += flows.exported_kwh * days;
        total.unmet_kwh += flows.unmet_kwh * days;
        total.curtailed_kwh += flows.curtailed_kwh * days;
    }

    let year_days = MONTH_DAYS.iter().sum::<u32>() as f64;
    let fixed = tariff.daily_charge_usd * year_days;
    GridYear {
        load_kwh,
        imported_kwh: total.imported_kwh,
        exported_kwh: total.exported_kwh,
        unmet_kwh: total.unmet_kwh,
        curtailed_kwh: total.curtailed_kwh,
        bill_without_system_usd: load_kwh * tariff.import_usd_per_kwh + fixed,
        bill_with_system_usd: total.imported_kwh * tariff.import_usd_per_kwh + fixed
            - total.exported_kwh * tariff.export_usd_per_kwh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HOURS_PER_DAY;

    fn hour(unmet_kw: f64, curtailed_kw: f64) -> HourPoint {
        HourPoint {
            load_kw: 0.0,
            solar_kw: 0.0,
            battery_kw: 0.0,
            soc: 0.0,
            unmet_kw,
            curtailed_kw,
        }
    }

    fn tariff(import_limit_kw: f64, export_limit_kw: f64) -> GridTariff {
        GridTariff {
            import_limit_kw,
            export_limit_kw,
            ..GridTariff::default()
        }
    }

    #[test]
    fn unmet_is_imported_up_to_the_limit() {
        let flows = grid_flows(&[hour(2.0, 0.0)], &tariff(1.5, 5.0));
        assert!((flows.imported_kwh - 1.5).abs() < 1e-12);
        assert!((flows.unmet_kwh - 0.5).abs() < 1e-12);
        assert_eq!(flows.exported_kwh, 0.0);
    }

    #[test]
    fn surplus_is_exported_up_to_the_limit() {
        let flows = grid_flows(&[hour(0.0, 3.0)], &tariff(15.0, 5.0));
        assert!((flows.exported_kwh - 3.0).abs() < 1e-12);
        assert_eq!(flows.curtailed_kwh, 0.0);
        let capped = grid_flows(&[hour(0.0, 3.0)], &tariff(15.0, 2.0));
        assert!((capped.exported_kwh - 2.0).abs() < 1e-12);
        assert!((capped.curtailed_kwh - 1.0).abs() < 1e-12);
    }

    #[test]
    fn grid_only_design_pays_the_same_as_no_system() {
        // No PV and no battery: the grid supplies everything, so the bill
        // with the system equals the bill without it.
        let year = grid_year(
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
            &DayLoad {
                hourly_kw: vec![1.0; HOURS_PER_DAY],
            },
            &MonthlyFactors::flat(),
            &GridTariff::default(),
        );
        assert!((year.load_kwh - 24.0 * 365.0).abs() < 1e-6, "{}", year.load_kwh);
        // A 0.05 kWp array still serves a little daytime load, so the grid
        // supplies nearly, not exactly, all of it.
        assert!(year.imported_kwh > 0.95 * year.load_kwh, "{}", year.imported_kwh);
        assert!(year.unmet_kwh < 1e-6);
        assert!(
            year.savings_usd().abs() < 0.05 * year.bill_without_system_usd,
            "{}",
            year.savings_usd()
        );
    }

    #[test]
    fn a_solar_array_saves_money_and_exports() {
        let tariff = GridTariff {
            export_limit_kw: 50.0,
            ..GridTariff::default()
        };
        let year = grid_year(
            &Site::default(),
            &SolarArray {
                capacity_kw: 10.0,
                ..SolarArray::default()
            },
            &BatterySpec {
                capacity_kwh: 0.0,
                power_kw: 0.0,
                ..BatterySpec::default()
            },
            &DayLoad {
                hourly_kw: vec![1.0; HOURS_PER_DAY],
            },
            &MonthlyFactors::flat(),
            &tariff,
        );
        assert!(year.exported_kwh > 0.0, "{}", year.exported_kwh);
        assert!(year.imported_kwh < year.load_kwh);
        assert!(year.savings_usd() > 0.0);
        let payback = year.simple_payback_years(10.0 * 1400.0).expect("saves money");
        assert!((payback - 14000.0 / year.savings_usd()).abs() < 1e-9);
    }

    #[test]
    fn payback_is_none_without_savings() {
        let year = GridYear {
            load_kwh: 100.0,
            imported_kwh: 100.0,
            exported_kwh: 0.0,
            unmet_kwh: 0.0,
            curtailed_kwh: 0.0,
            bill_without_system_usd: 30.0,
            bill_with_system_usd: 30.0,
        };
        assert_eq!(year.simple_payback_years(5000.0), None);
    }

    #[test]
    fn tariff_validation_rejects_bad_prices() {
        assert!(GridTariff::default().validate().is_empty());
        let bad = GridTariff {
            import_usd_per_kwh: -0.1,
            ..GridTariff::default()
        };
        assert_eq!(bad.validate().len(), 1);
    }
}
