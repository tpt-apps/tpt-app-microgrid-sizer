//! Per-appliance load builder: turns a short list of appliances (power, start
//! hour, hours of use per day) into the 24-hour load profile the simulation
//! runs on.
//!
//! Each appliance draws its power for a run of consecutive hours starting at
//! `start_hour`, wrapping past midnight. The power is taken as constant
//! during a run (a fridge's compressor cycle averages out to its mean draw),
//! and the profile is the same every day.

use crate::{DayLoad, HOURS_PER_DAY};

/// One appliance's daily use.
#[derive(Debug, Clone, PartialEq)]
pub struct Appliance {
    /// Power drawn while running, kW.
    pub power_kw: f64,
    /// First hour of the run (0-23).
    pub start_hour: usize,
    /// Hours of use per day (0-24).
    pub hours_per_day: usize,
}

impl Appliance {
    /// Validity errors for this appliance, numbered for display (`n` is its
    /// 1-based position in the list).
    pub fn validate(&self, n: usize) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.0..=100.0).contains(&self.power_kw) {
            issues.push(format!("Appliance {n} power must be between 0 and 100 kW."));
        }
        if self.start_hour >= HOURS_PER_DAY {
            issues.push(format!("Appliance {n} start hour must be between 0 and 23."));
        }
        if self.hours_per_day > HOURS_PER_DAY {
            issues.push(format!("Appliance {n} hours per day must be between 0 and 24."));
        }
        issues
    }
}

/// Validity errors for a list of appliances, phrased for direct display.
pub fn validate_appliances(appliances: &[Appliance]) -> Vec<String> {
    appliances
        .iter()
        .enumerate()
        .flat_map(|(i, a)| a.validate(i + 1))
        .collect()
}

/// The summed hourly load of `appliances`. Invalid entries are clamped into
/// range (use [`validate_appliances`] first to refuse them instead).
pub fn appliance_day_load(appliances: &[Appliance]) -> DayLoad {
    let mut hourly_kw = vec![0.0; HOURS_PER_DAY];
    for appliance in appliances {
        let power = appliance.power_kw.clamp(0.0, 100.0);
        let run = appliance.hours_per_day.min(HOURS_PER_DAY);
        let start = appliance.start_hour % HOURS_PER_DAY;
        for step in 0..run {
            hourly_kw[(start + step) % HOURS_PER_DAY] += power;
        }
    }
    DayLoad { hourly_kw }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appliance(power_kw: f64, start_hour: usize, hours_per_day: usize) -> Appliance {
        Appliance {
            power_kw,
            start_hour,
            hours_per_day,
        }
    }

    #[test]
    fn a_full_day_appliance_is_flat() {
        let load = appliance_day_load(&[appliance(0.1, 0, 24)]);
        assert!(load.hourly_kw.iter().all(|kw| (kw - 0.1).abs() < 1e-12));
    }

    #[test]
    fn a_run_wraps_past_midnight() {
        // A 2 kW kettle run from 23:00 for two hours covers 23:00 and 00:00.
        let load = appliance_day_load(&[appliance(2.0, 23, 2)]);
        assert_eq!(load.hourly_kw[23], 2.0);
        assert_eq!(load.hourly_kw[0], 2.0);
        assert_eq!(load.hourly_kw[1], 0.0);
        assert_eq!(load.hourly_kw.iter().sum::<f64>(), 4.0);
    }

    #[test]
    fn appliances_add_up_hour_by_hour() {
        let load = appliance_day_load(&[appliance(0.5, 18, 5), appliance(1.0, 20, 2)]);
        assert_eq!(load.hourly_kw[18], 0.5);
        assert_eq!(load.hourly_kw[20], 1.5);
        assert_eq!(load.hourly_kw[21], 1.5);
        assert_eq!(load.hourly_kw[22], 0.5);
        assert_eq!(load.hourly_kw[23], 0.0);
        // Energy is power times hours: 0.5 × 5 + 1.0 × 2.
        assert!((load.daily_kwh() - 4.5).abs() < 1e-12);
    }

    #[test]
    fn no_appliances_is_no_load() {
        let load = appliance_day_load(&[]);
        assert_eq!(load.daily_kwh(), 0.0);
    }

    #[test]
    fn validation_names_the_bad_appliance() {
        let issues = validate_appliances(&[appliance(0.1, 0, 4), appliance(-1.0, 25, 30)]);
        assert!(issues.iter().any(|m| m.contains("Appliance 2 power")));
        assert!(issues.iter().any(|m| m.contains("Appliance 2 start hour")));
        assert!(issues.iter().any(|m| m.contains("Appliance 2 hours per day")));
        assert!(!issues.iter().any(|m| m.contains("Appliance 1")));
    }
}
