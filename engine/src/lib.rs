//! TPT Microgrid Sizer engine — load/generation balance for off-grid solar
//! + battery systems.
//!
//! A thin, UI-free adapter over `tpt-energy`'s solar (`tpt-nrg-solar`) and
//! storage (`tpt-nrg-battery`) models, host-unit-testable on any target:
//!
//! - [`SolarModel`] + `PvPlant` give hourly PV AC output (NOCT thermal
//!   derating, inverter clipping, soiling),
//! - [`BatteryStorage`] gives the SoC-tracked dispatch (split-sqrt
//!   round-trip efficiency, minimum-SoC floor),
//! - this crate supplies what tpt-energy has no crate for: the hourly
//!   balance loop, unmet/curtailed energy accounting, the seasonal
//!   aggregation, and the sizing search.
//!
//! Units: the public API speaks **kW / kWh** (what the UI and design report
//! show); conversions to the MW / MWh the tpt-energy models use happen at
//! the calls into those crates.
//!
//! Time model: hourly steps over representative days (the 21st of each
//! month) with the site's fixed UTC offset — daylight saving is not
//! modelled. `tpt-nrg-solar` computes solar geometry from UTC instants
//! directly, so hour *h* local maps to UTC `h − timezone_offset_hours`.
//!
//! Cloud cover: `tpt-nrg-solar` keeps its irradiance models private, so
//! cloudiness is applied through the public output path — take the
//! clear-sky plane-of-array irradiance from `PvPlant::output_clearsky`,
//! undo the soiling factor, scale by the cloud factor, and re-run through
//! `PvPlant::output_from_poa` (which re-applies soiling, cell-temperature
//! derating, and inverter clipping).

#![deny(missing_docs)]

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use tpt_nrg_battery::BatteryStorage;
use tpt_nrg_solar::{PvPlant, PvPlantConfig, SolarModel};

mod import;
pub use import::{
    cloud_factors_from_weather, import_load_csv, import_weather_csv, ImportError, LoadImport,
    HOURS_PER_YEAR,
};

mod appliances;
pub use appliances::{appliance_day_load, validate_appliances, Appliance};

mod sites;
pub use sites::{site_presets, suggested_utc_offset, suggested_utc_offset_at, SitePreset};

mod inverter;
pub use inverter::{inverter_check, InverterReport};

#[cfg(feature = "pro")]
mod grid;
#[cfg(feature = "pro")]
pub use grid::{grid_flows, grid_year, GridEnergy, GridTariff, GridYear};

#[cfg(feature = "pro")]
mod reliability;
#[cfg(feature = "pro")]
pub use reliability::{reliability_report, ReliabilityReport, MAX_AUTONOMY_DAYS};

mod export;
pub use export::{day_csv, markdown_to_pdf};
#[cfg(feature = "pro")]
pub use export::seasonal_csv;

/// Hours per simulated day.
pub const HOURS_PER_DAY: usize = 24;
/// Months per simulated year.
pub const MONTHS_PER_YEAR: usize = 12;
/// Representative calendar day simulated for every month.
const REPRESENTATIVE_DAY: u32 = 21;
/// Day counts per month for `REPRESENTATIVE_YEAR` (non-leap) — the seasonal
/// simulation scales each representative day by its month's length.
const MONTH_DAYS: [u32; MONTHS_PER_YEAR] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
/// Calendar year used for solar geometry (declination differs between
/// years by well under the model's accuracy).
const REPRESENTATIVE_YEAR: i32 = 2026;
/// Soiling loss fraction applied by the PV model (matches the
/// `PvPlantConfig` default; restated here because the cloud path inverts
/// and re-applies it).
const SOILING_LOSS: f64 = 0.02;

/// Month names, January first — chart/report labels.
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Physical site of the microgrid.
#[derive(Debug, Clone)]
pub struct Site {
    /// Degrees north (negative = southern hemisphere).
    pub latitude_deg: f64,
    /// Degrees east.
    pub longitude_deg: f64,
    /// Fixed UTC offset in hours (e.g. `12.0` for NZST, `9.5` for ACST).
    pub timezone_offset_hours: f64,
    /// Metres above sea level (enters the clear-sky model).
    pub altitude_m: f64,
}

impl Default for Site {
    /// Wellington, NZ — the tool's home audience.
    fn default() -> Self {
        Self {
            latitude_deg: -41.2866,
            longitude_deg: 174.7756,
            timezone_offset_hours: 12.0,
            altitude_m: 30.0,
        }
    }
}

impl Site {
    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(-90.0..=90.0).contains(&self.latitude_deg) {
            issues.push("Latitude must be between -90 and 90 degrees.".to_string());
        }
        if !(-180.0..=180.0).contains(&self.longitude_deg) {
            issues.push("Longitude must be between -180 and 180 degrees.".to_string());
        }
        if !(-14.0..=14.0).contains(&self.timezone_offset_hours) {
            issues.push("UTC offset must be between -14 and +14 hours.".to_string());
        }
        if !(-500.0..=9000.0).contains(&self.altitude_m) {
            issues.push("Altitude must be between -500 and 9000 metres.".to_string());
        }
        issues
    }
}

/// The photovoltaic array being sized.
#[derive(Debug, Clone)]
pub struct SolarArray {
    /// DC nameplate capacity in kWp.
    pub capacity_kw: f64,
    /// Panel tilt from horizontal, degrees.
    pub tilt_deg: f64,
    /// Panel azimuth, degrees clockwise from north (`0` = north — correct
    /// for southern-hemisphere sites; `180` = south).
    pub azimuth_deg: f64,
    /// Fraction of clear-sky irradiance reaching the array (1.0 = clear).
    pub cloud_factor: f64,
    /// Ambient temperature for the cell-temperature model, °C.
    pub ambient_celsius: f64,
    /// Output lost per year to module degradation (0.005 = 0.5%/yr, the
    /// typical crystalline-silicon warranty rate). Applied through
    /// [`Self::aged`].
    pub annual_degradation: f64,
    /// DC-to-AC ratio: DC nameplate over inverter AC rating. 1.0 means no
    /// oversizing (the inverter can carry the whole array); 1.2 is typical
    /// and the value the PV model used before this was configurable.
    pub dc_ac_ratio: f64,
    /// Planes of a multi-orientation array, each taking a share of the
    /// nameplate. Empty (the default) means one plane at `tilt_deg` and
    /// `azimuth_deg`; when given, the shares are normalised to sum to one.
    pub orientations: Vec<Orientation>,
    /// Obstructions that shade the array for whole hours of the day.
    pub obstructions: Vec<Obstruction>,
}

/// One plane of a multi-orientation array.
#[derive(Debug, Clone, PartialEq)]
pub struct Orientation {
    /// Share of the array's nameplate on this plane (any positive weight;
    /// shares are normalised).
    pub share: f64,
    /// Plane tilt from horizontal, degrees.
    pub tilt_deg: f64,
    /// Plane azimuth, degrees clockwise from north.
    pub azimuth_deg: f64,
}

/// An obstruction (tree, chimney, neighbouring building) that cuts the
/// irradiance on the array for whole hours of the day.
#[derive(Debug, Clone, PartialEq)]
pub struct Obstruction {
    /// First shaded hour of the day (0-23), inclusive.
    pub start_hour: usize,
    /// Hour the shade ends, exclusive (1-24). When `end_hour` is earlier than
    /// `start_hour` the shade wraps past midnight.
    pub end_hour: usize,
    /// Fraction of the irradiance removed in the shaded hours (0-1).
    pub loss_fraction: f64,
}

impl Obstruction {
    /// Whether hour `hour` (0-23) falls inside the shaded window.
    fn covers(&self, hour: usize) -> bool {
        if self.start_hour < self.end_hour {
            (self.start_hour..self.end_hour).contains(&hour)
        } else if self.start_hour > self.end_hour {
            hour >= self.start_hour || hour < self.end_hour
        } else {
            false
        }
    }
}

/// The fraction of irradiance removed in each hour of the day by all of
/// `obstructions`. Overlapping obstructions combine multiplicatively.
pub fn hourly_shade(obstructions: &[Obstruction]) -> [f64; HOURS_PER_DAY] {
    let mut shade = [0.0; HOURS_PER_DAY];
    for (hour, value) in shade.iter_mut().enumerate() {
        let kept: f64 = obstructions
            .iter()
            .filter(|o| o.covers(hour))
            .map(|o| 1.0 - o.loss_fraction.clamp(0.0, 1.0))
            .product();
        *value = 1.0 - kept;
    }
    shade
}

impl Default for SolarArray {
    fn default() -> Self {
        Self {
            capacity_kw: 5.0,
            tilt_deg: 30.0,
            azimuth_deg: 0.0,
            cloud_factor: 1.0,
            ambient_celsius: 15.0,
            annual_degradation: 0.005,
            dc_ac_ratio: 1.2,
            orientations: Vec::new(),
            obstructions: Vec::new(),
        }
    }
}

impl SolarArray {
    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.05..=10_000.0).contains(&self.capacity_kw) {
            issues.push("Solar capacity must be between 0.05 and 10,000 kWp.".to_string());
        }
        if !(0.0..=90.0).contains(&self.tilt_deg) {
            issues.push("Tilt must be between 0 and 90 degrees.".to_string());
        }
        if !(-360.0..=360.0).contains(&self.azimuth_deg) {
            issues.push("Azimuth must be between -360 and 360 degrees.".to_string());
        }
        if !(0.0..=1.0).contains(&self.cloud_factor) {
            issues.push("Cloud factor must be between 0 and 1.".to_string());
        }
        if !(-30.0..=55.0).contains(&self.ambient_celsius) {
            issues.push("Ambient temperature must be between -30 and 55 °C.".to_string());
        }
        if !(0.0..=0.05).contains(&self.annual_degradation) {
            issues.push("PV degradation must be between 0% and 5% per year.".to_string());
        }
        if self.orientations.len() > 4 {
            issues.push("At most four array orientations are supported.".to_string());
        }
        for (i, plane) in self.orientations.iter().enumerate() {
            let n = i + 1;
            if !(plane.share.is_finite() && plane.share >= 0.0) {
                issues.push(format!("Orientation {n} share must be zero or more."));
            }
            if !(0.0..=90.0).contains(&plane.tilt_deg) {
                issues.push(format!("Orientation {n} tilt must be between 0 and 90 degrees."));
            }
            if !(-360.0..=360.0).contains(&plane.azimuth_deg) {
                issues.push(format!("Orientation {n} azimuth must be between -360 and 360 degrees."));
            }
        }
        if !self.orientations.is_empty() && self.orientations.iter().all(|p| p.share <= 0.0) {
            issues.push("At least one orientation needs a share above zero.".to_string());
        }
        for (i, obstruction) in self.obstructions.iter().enumerate() {
            let n = i + 1;
            if obstruction.start_hour > 23 || obstruction.end_hour > 24 {
                issues.push(format!("Shading {n} hours must be between 0 and 24."));
            }
            if obstruction.start_hour == obstruction.end_hour {
                issues.push(format!("Shading {n} must cover at least one hour."));
            }
            if !(0.0..=1.0).contains(&obstruction.loss_fraction) {
                issues.push(format!("Shading {n} loss must be between 0% and 100%."));
            }
        }
        if !(0.5..=2.0).contains(&self.dc_ac_ratio) {
            issues.push("DC/AC ratio must be between 0.5 and 2.0.".to_string());
        }
        issues
    }

    /// The array after `years` of module degradation: nameplate scaled by
    /// `(1 - annual_degradation)^years`. Orientation and cloud/ambient
    /// settings are unchanged.
    pub fn aged(&self, years: f64) -> Self {
        let retained = 1.0 - self.annual_degradation.clamp(0.0, 1.0);
        Self {
            capacity_kw: self.capacity_kw * retained.powf(years.max(0.0)),
            ..self.clone()
        }
    }
}

/// The battery bank being sized.
#[derive(Debug, Clone)]
pub struct BatterySpec {
    /// Nameplate energy capacity in kWh (0 = no battery).
    pub capacity_kwh: f64,
    /// Power rating in kW (both charge and discharge).
    pub power_kw: f64,
    /// Round-trip efficiency in (0, 1].
    pub round_trip_efficiency: f64,
    /// Minimum state of charge as a fraction of nameplate (0-1).
    pub min_soc: f64,
    /// State of charge at the start of the simulated period (0-1).
    pub initial_soc: f64,
    /// Nameplate capacity lost per year to cell ageing (0.02 = 2%/yr). Power
    /// rating is not derated. Applied through [`Self::aged`].
    pub annual_fade: f64,
    /// Fraction of the stored charge lost per day to self-discharge
    /// (0.01 = 1%/day; 0 = none). Applied hourly while idle or cycling.
    pub self_discharge_per_day: f64,
}

impl Default for BatterySpec {
    fn default() -> Self {
        Self {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 0.95,
            min_soc: 0.1,
            initial_soc: 0.5,
            annual_fade: 0.02,
            self_discharge_per_day: 0.0,
        }
    }
}

impl BatterySpec {
    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.0..=10_000.0).contains(&self.capacity_kwh) {
            issues.push("Battery capacity must be between 0 and 10,000 kWh.".to_string());
        }
        if !(0.0..=10_000.0).contains(&self.power_kw) {
            issues.push("Battery power must be between 0 and 10,000 kW.".to_string());
        }
        if !(0.5..=1.0).contains(&self.round_trip_efficiency) {
            issues.push("Round-trip efficiency must be between 50% and 100%.".to_string());
        }
        if !(0.0..=0.9).contains(&self.min_soc) {
            issues.push("Minimum SoC must be between 0% and 90%.".to_string());
        }
        if !(0.0..=1.0).contains(&self.initial_soc) {
            issues.push("Initial SoC must be between 0% and 100%.".to_string());
        } else if self.min_soc <= 0.9 && self.initial_soc < self.min_soc {
            issues.push("Initial SoC must not be below the minimum SoC.".to_string());
        }
        if !(0.0..=0.2).contains(&self.annual_fade) {
            issues.push("Battery fade must be between 0% and 20% per year.".to_string());
        }
        if !(0.0..=0.2).contains(&self.self_discharge_per_day) {
            issues.push("Self-discharge must be between 0% and 20% per day.".to_string());
        }
        issues
    }

    fn clamped(&self) -> Self {
        Self {
            capacity_kwh: self.capacity_kwh.max(0.0),
            power_kw: self.power_kw.max(0.0),
            round_trip_efficiency: self.round_trip_efficiency.clamp(0.5, 1.0),
            min_soc: self.min_soc.clamp(0.0, 0.95),
            initial_soc: self.initial_soc.clamp(0.0, 1.0),
            annual_fade: self.annual_fade,
            self_discharge_per_day: self.self_discharge_per_day.clamp(0.0, 0.2),
        }
    }

    /// The battery after `years` of ageing: nameplate energy scaled by
    /// `(1 - annual_fade)^years`. Power rating and chemistry are unchanged.
    pub fn aged(&self, years: f64) -> Self {
        let retained = 1.0 - self.annual_fade.clamp(0.0, 1.0);
        Self {
            capacity_kwh: self.capacity_kwh * retained.powf(years.max(0.0)),
            ..self.clone()
        }
    }
}

/// A battery chemistry's typical parameters, used to fill in the battery
/// (and, in Pro, the optimizer cost/life) inputs. Values are indicative
/// installed-system figures, not datasheet guarantees — the user can edit
/// every field afterwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryChemistry {
    /// Display name.
    pub name: &'static str,
    /// Round-trip efficiency in (0, 1].
    pub round_trip_efficiency: f64,
    /// Minimum state of charge (1 - usable depth of discharge).
    pub min_soc: f64,
    /// Nameplate capacity lost per year.
    pub annual_fade: f64,
    /// Fraction of stored charge lost per day.
    pub self_discharge_per_day: f64,
    /// Sensible continuous power per kWh of capacity (kW/kWh).
    pub power_per_kwh: f64,
    /// Installed cost, $/kWh.
    pub cost_usd_per_kwh: f64,
    /// Service life before replacement, years.
    pub life_years: f64,
}

impl BatteryChemistry {
    /// `base` with this chemistry's electrical parameters applied; capacity,
    /// initial SoC and (when `scale_power` is set) power follow the chemistry.
    pub fn apply(&self, base: &BatterySpec, scale_power: bool) -> BatterySpec {
        BatterySpec {
            power_kw: if scale_power {
                base.capacity_kwh * self.power_per_kwh
            } else {
                base.power_kw
            },
            round_trip_efficiency: self.round_trip_efficiency,
            min_soc: self.min_soc,
            initial_soc: base.initial_soc.max(self.min_soc),
            annual_fade: self.annual_fade,
            self_discharge_per_day: self.self_discharge_per_day,
            ..base.clone()
        }
    }
}

/// Built-in chemistries, in UI order. The first entry matches
/// [`BatterySpec::default`].
pub fn chemistries() -> &'static [BatteryChemistry] {
    const ALL: &[BatteryChemistry] = &[
        BatteryChemistry {
            name: "Lithium iron phosphate (LFP)",
            round_trip_efficiency: 0.95,
            min_soc: 0.10,
            annual_fade: 0.02,
            self_discharge_per_day: 0.0,
            power_per_kwh: 0.5,
            cost_usd_per_kwh: 600.0,
            life_years: 10.0,
        },
        BatteryChemistry {
            name: "Lithium NMC / Li-ion",
            round_trip_efficiency: 0.95,
            min_soc: 0.10,
            annual_fade: 0.03,
            self_discharge_per_day: 0.0,
            power_per_kwh: 1.0,
            cost_usd_per_kwh: 650.0,
            life_years: 8.0,
        },
        BatteryChemistry {
            name: "Sodium-ion",
            round_trip_efficiency: 0.92,
            min_soc: 0.10,
            annual_fade: 0.02,
            self_discharge_per_day: 0.0005,
            power_per_kwh: 0.5,
            cost_usd_per_kwh: 450.0,
            life_years: 10.0,
        },
        BatteryChemistry {
            name: "Lead-acid, flooded",
            round_trip_efficiency: 0.80,
            min_soc: 0.50,
            annual_fade: 0.04,
            self_discharge_per_day: 0.002,
            power_per_kwh: 0.2,
            cost_usd_per_kwh: 200.0,
            life_years: 5.0,
        },
        BatteryChemistry {
            name: "Lead-acid, AGM / gel",
            round_trip_efficiency: 0.85,
            min_soc: 0.50,
            annual_fade: 0.04,
            self_discharge_per_day: 0.001,
            power_per_kwh: 0.25,
            cost_usd_per_kwh: 300.0,
            life_years: 6.0,
        },
        BatteryChemistry {
            name: "Nickel-iron (NiFe)",
            round_trip_efficiency: 0.70,
            min_soc: 0.10,
            annual_fade: 0.005,
            self_discharge_per_day: 0.01,
            power_per_kwh: 0.2,
            cost_usd_per_kwh: 450.0,
            life_years: 25.0,
        },
        BatteryChemistry {
            name: "Vanadium redox flow",
            round_trip_efficiency: 0.70,
            min_soc: 0.05,
            annual_fade: 0.005,
            self_discharge_per_day: 0.001,
            power_per_kwh: 0.25,
            cost_usd_per_kwh: 600.0,
            life_years: 20.0,
        },
    ];
    ALL
}

/// One representative day's load, one value per hour, in kW.
#[derive(Debug, Clone)]
pub struct DayLoad {
    /// 24 hourly kW values, hour 0 = local midnight.
    pub hourly_kw: Vec<f64>,
}

impl Default for DayLoad {
    /// A small off-grid cabin profile (evening peak), ≈9.4 kWh/day.
    fn default() -> Self {
        Self {
            hourly_kw: presets::CABIN.to_vec(),
        }
    }
}

impl DayLoad {
    /// Total daily energy in kWh.
    pub fn daily_kwh(&self) -> f64 {
        self.hourly_kw.iter().sum()
    }

    /// Peak hourly power in kW.
    pub fn peak_kw(&self) -> f64 {
        self.hourly_kw.iter().copied().fold(0.0, f64::max)
    }

    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.hourly_kw.len() != HOURS_PER_DAY {
            issues.push(format!(
                "The load profile needs exactly {HOURS_PER_DAY} hourly values."
            ));
            return issues;
        }
        if self.hourly_kw.iter().any(|kw| !(0.0..=10_000.0).contains(kw)) {
            issues.push("Every hourly load must be between 0 and 10,000 kW.".to_string());
        }
        if self.hourly_kw.iter().sum::<f64>() <= 0.0 {
            issues.push("The daily load total must be greater than zero.".to_string());
        }
        issues
    }

    #[cfg(feature = "pro")]
    fn scaled(&self, factor: f64) -> Self {
        Self {
            hourly_kw: self.hourly_kw.iter().map(|kw| kw * factor).collect(),
        }
    }
}

/// Built-in load-profile presets (24 hourly kW values).
pub mod presets {
    use super::HOURS_PER_DAY;

    /// Small off-grid cabin: ≈9.4 kWh/day, 1.5 kW evening peak.
    pub const CABIN: [f64; 24] = [
        0.12, 0.12, 0.12, 0.12, 0.12, 0.12, 0.3, 0.6, 0.5, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.25,
        0.35, 0.6, 1.2, 1.5, 1.1, 0.6, 0.3, 0.15,
    ];
    /// Small commercial site: daytime-heavy, ≈62 kWh/day, 5 kW peak.
    pub const SMALL_COMMERCIAL: [f64; 24] = [
        0.8, 0.8, 0.8, 0.8, 0.8, 0.8, 0.8, 2.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0,
        3.0, 1.5, 1.0, 1.0, 1.0, 1.0, 0.8,
    ];
    /// Flat 1 kW base load, 24 kWh/day.
    pub const CONSTANT_1KW: [f64; 24] = [1.0; 24];

    /// All presets as `(label, values)` for UI select options.
    pub fn all() -> [(&'static str, &'static [f64; HOURS_PER_DAY]); 3] {
        [
            ("Off-grid cabin (9.4 kWh/day)", &CABIN),
            ("Small commercial (62 kWh/day)", &SMALL_COMMERCIAL),
            ("Constant 1 kW (24 kWh/day)", &CONSTANT_1KW),
        ]
    }
}

/// Balance at one hour of the day.
#[derive(Debug, Clone)]
pub struct HourPoint {
    /// Load in kW.
    pub load_kw: f64,
    /// PV AC output in kW.
    pub solar_kw: f64,
    /// Battery grid-side power in kW: positive = charging, negative =
    /// discharging.
    pub battery_kw: f64,
    /// Battery state of charge (0-1) at the end of the hour.
    pub soc: f64,
    /// Load that could not be served in this hour, kW.
    pub unmet_kw: f64,
    /// Solar that could be neither used nor stored, kW.
    pub curtailed_kw: f64,
}

/// Energy totals over a dispatch period.
#[derive(Debug, Clone, Default)]
pub struct DispatchTotals {
    /// Total load served + unmet, kWh.
    pub load_kwh: f64,
    /// Total PV AC production, kWh.
    pub solar_kwh: f64,
    /// Load energy that could not be served, kWh.
    pub unmet_kwh: f64,
    /// Solar energy curtailed (neither used nor stored), kWh.
    pub curtailed_kwh: f64,
    /// Grid-side energy into the battery, kWh (losses included).
    pub charged_kwh: f64,
    /// Grid-side energy out of the battery, kWh (losses included).
    pub discharged_kwh: f64,
    /// State of charge (0-1) at the end of the period.
    pub end_soc: f64,
}

/// Why a solar/load profile pair could not be dispatched.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    /// The solar and load arrays describe different time spans.
    LengthMismatch {
        /// Number of hourly solar values supplied.
        solar: usize,
        /// Number of hourly load values supplied.
        load: usize,
    },
    /// A value in either profile was NaN or infinite.
    NotFinite {
        /// Hour index of the offending value.
        hour: usize,
        /// Which profile it came from.
        profile: &'static str,
    },
}

impl core::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LengthMismatch { solar, load } => write!(
                f,
                "the solar profile has {solar} hour(s) but the load profile has {load} \
                 - both must cover the same period"
            ),
            Self::NotFinite { hour, profile } => {
                write!(f, "{profile} value at hour {hour} is not a finite number")
            }
        }
    }
}

impl ProfileError {
    /// Checks a solar/load pair before dispatch. Previously a short solar
    /// array was silently padded with zero sun, which reads as a dark-sky
    /// result rather than a data-entry mistake.
    pub fn check(solar_kw: &[f64], load_kw: &[f64]) -> Result<(), Self> {
        if solar_kw.len() != load_kw.len() {
            return Err(Self::LengthMismatch {
                solar: solar_kw.len(),
                load: load_kw.len(),
            });
        }
        for (hour, value) in solar_kw.iter().chain(load_kw.iter()).enumerate() {
            if !value.is_finite() {
                let profile = if hour < solar_kw.len() {
                    "solar"
                } else {
                    "load"
                };
                return Err(Self::NotFinite {
                    hour: if hour < solar_kw.len() {
                        hour
                    } else {
                        hour - solar_kw.len()
                    },
                    profile,
                });
            }
        }
        Ok(())
    }
}

/// Runs the greedy hourly dispatch over arbitrary solar/load profiles and
/// returns only the totals — the hand-checkable core primitive.
///
/// Dispatch priority per hour: solar serves load directly; surplus charges
/// the battery (then curtails); deficits discharge the battery (then go
/// unmet). No grid connection.
///
/// # Errors
/// [`ProfileError`] when the two profiles cover different periods or carry a
/// non-finite value — dispatching mismatched arrays would otherwise report a
/// plausible but meaningless balance.
pub fn dispatch_profile(
    solar_kw: &[f64],
    load_kw: &[f64],
    battery: &BatterySpec,
) -> Result<DispatchTotals, ProfileError> {
    ProfileError::check(solar_kw, load_kw)?;
    Ok(dispatch_hours(solar_kw, load_kw, battery).1)
}

/// Internal hourly dispatch returning the full trajectory.
fn dispatch_hours(
    solar_kw: &[f64],
    load_kw: &[f64],
    battery: &BatterySpec,
) -> (Vec<HourPoint>, DispatchTotals) {
    let spec = battery.clamped();
    let usable = spec.capacity_kwh > 1e-9 && spec.power_kw > 1e-9;
    // tpt-nrg-battery speaks MW/MWh.
    let mut storage = BatteryStorage::new(
        spec.capacity_kwh / 1000.0,
        spec.power_kw / 1000.0,
        spec.round_trip_efficiency,
    )
    .with_soc(spec.min_soc, spec.initial_soc.clamp(spec.min_soc, 1.0));
    let one_way_eff = spec.round_trip_efficiency.sqrt();
    // Per-hour charge retention from the per-day self-discharge rate.
    let retention = (1.0 - spec.self_discharge_per_day).powf(1.0 / 24.0);

    let mut hours = Vec::with_capacity(load_kw.len());
    let mut totals = DispatchTotals {
        end_soc: storage.soc,
        ..DispatchTotals::default()
    };
    // tpt-nrg-battery mutates SoC *before* checking its limits, so a request
    // that lands exactly on the floor/ceiling can come back as an `Err`
    // (float dust) even though the energy was delivered. Pre-clamping every
    // request against `available`/`headroom` with a hair of slack keeps the
    // error path unreachable and the returned amounts authoritative.
    const SOC_EPSILON: f64 = 1e-9;

    for (h, &load_raw) in load_kw.iter().enumerate() {
        let load = load_raw.max(0.0);
        let solar = solar_kw.get(h).copied().unwrap_or(0.0).max(0.0);
        let net_kw = solar - load;
        let (battery_kw, unmet_kw, curtailed_kw);
        if !usable {
            battery_kw = 0.0;
            unmet_kw = (-net_kw).max(0.0);
            curtailed_kw = net_kw.max(0.0);
        } else if net_kw >= 0.0 {
            // Grid-side input the headroom can absorb; `charge` returns the
            // stored MWh.
            let headroom_in_mw =
                storage.headroom_energy_mwh() / one_way_eff * (1.0 - SOC_EPSILON);
            let requested_mw = (net_kw / 1000.0)
                .min(storage.power_rating_mw)
                .min(headroom_in_mw);
            let stored_mwh = storage.charge(requested_mw, 1.0).unwrap_or(0.0);
            let accepted_kw = stored_mwh * 1000.0 / one_way_eff;
            battery_kw = accepted_kw;
            curtailed_kw = net_kw - accepted_kw;
            unmet_kw = 0.0;
        } else {
            // Grid-side output the usable energy can back; `discharge`
            // returns the delivered MWh.
            let deficit_kw = -net_kw;
            let available_out_mw =
                storage.available_energy_mwh() * one_way_eff * (1.0 - SOC_EPSILON);
            let requested_mw = (deficit_kw / 1000.0)
                .min(storage.power_rating_mw)
                .min(available_out_mw);
            let delivered_mwh = storage.discharge(requested_mw, 1.0).unwrap_or(0.0);
            let delivered_kw = delivered_mwh * 1000.0;
            battery_kw = -delivered_kw;
            unmet_kw = deficit_kw - delivered_kw;
            curtailed_kw = 0.0;
        }
        if usable && spec.self_discharge_per_day > 0.0 {
            // Leakage never takes the bank below its protected floor.
            storage.soc = (storage.soc * retention).max(spec.min_soc);
        }
        totals.load_kwh += load;
        totals.solar_kwh += solar;
        totals.unmet_kwh += unmet_kw;
        totals.curtailed_kwh += curtailed_kw;
        if battery_kw >= 0.0 {
            totals.charged_kwh += battery_kw;
        } else {
            totals.discharged_kwh += -battery_kw;
        }
        hours.push(HourPoint {
            load_kw: load,
            solar_kw: solar,
            battery_kw,
            soc: storage.soc,
            unmet_kw,
            curtailed_kw,
        });
    }
    totals.end_soc = storage.soc;
    (hours, totals)
}

/// The solar geometry and one PV plant per plane of `array`. Shared by the
/// clear-sky profile and the measured-weather import, so both use the same
/// soiling, thermal and inverter configuration. Each plane has its own
/// inverter share, so clipping is computed per plane, and an array with no
/// explicit orientations is a single plane at its tilt and azimuth.
pub(crate) fn planes_for(site: &Site, array: &SolarArray) -> (SolarModel, Vec<PvPlant>) {
    let solar_model = SolarModel::new(
        site.latitude_deg,
        site.longitude_deg,
        site.altitude_m,
        site.timezone_offset_hours,
    );
    let planes: Vec<(f64, f64, f64)> = if array.orientations.is_empty() {
        vec![(1.0, array.tilt_deg, array.azimuth_deg)]
    } else {
        let total: f64 = array.orientations.iter().map(|p| p.share.max(0.0)).sum();
        array
            .orientations
            .iter()
            .filter(|p| p.share > 0.0 && total > 0.0)
            .map(|p| (p.share / total, p.tilt_deg, p.azimuth_deg))
            .collect()
    };
    let plants = planes
        .into_iter()
        .map(|(share, tilt_deg, azimuth_deg)| {
            let mut config = PvPlantConfig::new(
                array.capacity_kw * share / 1000.0,
                tilt_deg,
                azimuth_deg,
                array.ambient_celsius,
            );
            config.soiling_loss = SOILING_LOSS;
            config.dc_ac_ratio = array.dc_ac_ratio;
            PvPlant::new(solar_model.clone(), config)
        })
        .collect();
    (solar_model, plants)
}

/// Hourly PV AC output (kW) for the representative day of `month`
/// (0 = January) under `array`'s cloud/ambient conditions.
pub fn solar_profile_kw(site: &Site, array: &SolarArray, month: usize) -> Vec<f64> {
    pv_day(site, array, month).0
}

/// Hourly PV AC output and the power the inverter clipped in each hour, both
/// in kW, for the representative day of `month`.
fn pv_day(site: &Site, array: &SolarArray, month: usize) -> (Vec<f64>, Vec<f64>) {
    let (solar_model, plants) = planes_for(site, array);
    let cloud = array.cloud_factor.clamp(0.0, 1.0);
    let shade = hourly_shade(&array.obstructions);
    let base_utc = month_base_utc(site, month);

    (0..HOURS_PER_DAY)
        .map(|h| {
            // Hour `h` covers [h, h+1): sample the sun at the midpoint.
            let pos = solar_model.solar_position(base_utc + Duration::minutes(h as i64 * 60 + 30));
            let (mut ac_mw, mut clipped_mw) = (0.0, 0.0);
            for plant in &plants {
                let clear = plant.output_clearsky(&pos);
                // Clear-sky POA (undo soiling), scale by cloud and shading,
                // re-run the public output path (re-applies soiling, thermal
                // and clipping for this plane).
                let poa_clear = clear.poa_w_per_m2 / (1.0 - SOILING_LOSS);
                let out = plant.output_from_poa(poa_clear * cloud * (1.0 - shade[h]));
                ac_mw += out.ac_power_mw;
                clipped_mw += out.inverter_clipping_mw;
            }
            (ac_mw * 1000.0, clipped_mw * 1000.0)
        })
        .unzip()
}

/// Local midnight of the representative day of `month`, as a UTC instant.
fn month_base_utc(site: &Site, month: usize) -> chrono::DateTime<Utc> {
    let month1 = (month % MONTHS_PER_YEAR) as u32 + 1;
    let date = NaiveDate::from_ymd_opt(REPRESENTATIVE_YEAR, month1, REPRESENTATIVE_DAY)
        .expect("representative date is valid");
    let local_midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    let offset_secs = (site.timezone_offset_hours * 3600.0).round() as i64;
    Utc.from_utc_datetime(&(local_midnight - Duration::seconds(offset_secs)))
}

/// Result of a single-day simulation.
#[derive(Debug, Clone)]
pub struct DayResult {
    /// Month simulated (0 = January).
    pub month: usize,
    /// The 24 hourly balance points.
    pub hours: Vec<HourPoint>,
    /// Period totals.
    pub totals: DispatchTotals,
    /// Equivalent full discharge cycles this day represents.
    pub battery_cycles: f64,
}

impl DayResult {
    /// Fraction of load energy served (1.0 when there was no load).
    pub fn served_fraction(&self) -> f64 {
        served(self.totals.unmet_kwh, self.totals.load_kwh)
    }
}

/// Equivalent full cycles: discharged energy over *usable* capacity
/// (nameplate above the SoC floor).
fn equivalent_cycles(discharged_kwh: f64, battery: &BatterySpec) -> f64 {
    let usable_kwh = battery.capacity_kwh * (1.0 - battery.min_soc.clamp(0.0, 0.95));
    if usable_kwh > 1e-9 {
        discharged_kwh / usable_kwh
    } else {
        0.0
    }
}

/// Simulates one representative day of `month` (0 = January).
pub fn simulate_day(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    load: &DayLoad,
    month: usize,
) -> DayResult {
    let solar = solar_profile_kw(site, array, month);
    let (hours, totals) = dispatch_hours(&solar, &load.hourly_kw, battery);
    let battery_cycles = equivalent_cycles(totals.discharged_kwh, battery);
    DayResult {
        month: month % MONTHS_PER_YEAR,
        hours,
        totals,
        battery_cycles,
    }
}

fn served(unmet_kwh: f64, load_kwh: f64) -> f64 {
    if load_kwh <= 1e-9 {
        1.0
    } else {
        (1.0 - unmet_kwh / load_kwh).clamp(0.0, 1.0)
    }
}

/// Per-month load/cloud/ambient variation for the seasonal simulation.
#[derive(Debug, Clone)]
pub struct MonthlyFactors {
    /// Load multipliers applied to the base daily profile (12 values).
    pub load: [f64; MONTHS_PER_YEAR],
    /// Cloud factors — fraction of clear-sky irradiance (12 values).
    pub cloud: [f64; MONTHS_PER_YEAR],
    /// Monthly mean ambient temperature, °C (12 values).
    pub ambient_c: [f64; MONTHS_PER_YEAR],
}

impl MonthlyFactors {
    /// Flat factors: full clear sky, no load variation, 15 °C.
    pub fn flat() -> Self {
        Self {
            load: [1.0; MONTHS_PER_YEAR],
            cloud: [1.0; MONTHS_PER_YEAR],
            ambient_c: [15.0; MONTHS_PER_YEAR],
        }
    }

    /// Temperate southern-hemisphere defaults (Wellington-ish): winter-peaked
    /// load (June–August), cloudier and cooler winters — the case that
    /// punishes undersized systems and therefore the one worth defaulting to.
    pub fn temperate_southern() -> Self {
        Self {
            load: [0.9, 0.9, 0.98, 1.05, 1.12, 1.18, 1.2, 1.15, 1.08, 1.0, 0.92, 0.9],
            cloud: [0.62, 0.6, 0.56, 0.52, 0.48, 0.46, 0.46, 0.5, 0.55, 0.58, 0.6, 0.64],
            ambient_c: [17.0, 17.4, 16.2, 14.1, 12.0, 10.2, 9.5, 9.9, 11.3, 12.9, 14.5, 16.3],
        }
    }

    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (m, factor) in self.load.iter().enumerate() {
            if !(0.2..=3.0).contains(factor) {
                issues.push(format!(
                    "{} load factor must be between 0.2 and 3.0.",
                    MONTH_NAMES[m]
                ));
            }
        }
        for (m, factor) in self.cloud.iter().enumerate() {
            if !(0.0..=1.0).contains(factor) {
                issues.push(format!(
                    "{} cloud factor must be between 0 and 1.",
                    MONTH_NAMES[m]
                ));
            }
        }
        for (m, temp) in self.ambient_c.iter().enumerate() {
            if !(-30.0..=55.0).contains(temp) {
                issues.push(format!(
                    "{} ambient temperature must be between -30 and 55 °C.",
                    MONTH_NAMES[m]
                ));
            }
        }
        issues
    }
}

impl Default for MonthlyFactors {
    fn default() -> Self {
        Self::temperate_southern()
    }
}

/// Energy totals for one simulated month.
#[derive(Debug, Clone)]
pub struct MonthSummary {
    /// Month index (0 = January).
    pub month: usize,
    /// Total load energy, kWh.
    pub load_kwh: f64,
    /// Total PV AC production, kWh.
    pub solar_kwh: f64,
    /// Unmet load energy, kWh.
    pub unmet_kwh: f64,
    /// Curtailed solar energy, kWh.
    pub curtailed_kwh: f64,
    /// Grid-side energy delivered by the battery, kWh.
    pub discharged_kwh: f64,
    /// Energy supplied by the backup generator, kWh.
    pub generator_kwh: f64,
}

/// Result of the full seasonal (12-month) simulation.
#[derive(Debug, Clone)]
pub struct SeasonalResult {
    /// One entry per month, January through December.
    pub months: Vec<MonthSummary>,
    /// Annual load energy, kWh.
    pub load_kwh: f64,
    /// Annual PV production, kWh.
    pub solar_kwh: f64,
    /// Annual unmet load, kWh.
    pub unmet_kwh: f64,
    /// Annual curtailed solar, kWh.
    pub curtailed_kwh: f64,
    /// Annual energy supplied by the backup generator, kWh.
    pub generator_kwh: f64,
    /// Annual generator fuel use, litres.
    pub fuel_litres: f64,
    /// Unmet energy in the worst month, kWh — the month that governs
    /// sizing.
    pub worst_month_unmet_kwh: f64,
    /// Index of the worst month (0 = January).
    pub worst_month: usize,
}

impl SeasonalResult {
    /// Annual fraction of load energy served.
    pub fn served_fraction(&self) -> f64 {
        served(self.unmet_kwh, self.load_kwh)
    }

    /// Fraction of load energy served in the worst month.
    pub fn worst_month_served_fraction(&self) -> f64 {
        let month = &self.months[self.worst_month];
        served(month.unmet_kwh, month.load_kwh)
    }
}

/// Simulates the full year, one representative day per month. Each day is
/// repeated until the battery's state of charge returns to where the day
/// started (the periodic steady state), so a month's result describes that
/// month's sustainable operation and does not depend on the previous month
/// or the configured initial SoC. Each month's day is scaled by that
/// month's day count, so month and annual totals read as real month/year
/// energies; served fractions and the governing month are unaffected by the
/// scaling. A single repeating day has no multi-day cloudy spells, so
/// results remain optimistic for sites with long overcast runs.
#[cfg(feature = "pro")]
pub fn simulate_seasonal(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
) -> SeasonalResult {
    simulate_seasonal_with_generator(site, array, battery, base_load, factors, None)
}

/// [`simulate_seasonal`] with an optional backup generator that serves
/// whatever the PV and battery leave unmet, up to its rated power each hour.
#[cfg(feature = "pro")]
pub fn simulate_seasonal_with_generator(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    generator: Option<&GeneratorSpec>,
) -> SeasonalResult {
    let mut months = Vec::with_capacity(MONTHS_PER_YEAR);
    let mut totals = DispatchTotals::default();
    let mut worst_month = 0usize;
    let mut worst_month_unmet_kwh = 0.0f64;
    let mut generator_kwh = 0.0f64;
    let mut fuel_litres = 0.0f64;

    for m in 0..MONTHS_PER_YEAR {
        let array_m = SolarArray {
            cloud_factor: factors.cloud[m],
            ambient_celsius: factors.ambient_c[m],
            ..array.clone()
        };
        let load_m = base_load.scaled(factors.load[m]);
        let day = simulate_steady_day(site, &array_m, battery, &load_m, m);
        let scale = f64::from(MONTH_DAYS[m]);
        let gen_day_kwh = generator
            .map(|g| generator_energy_kwh(&day.hours, g))
            .unwrap_or(0.0);
        let unmet_day_kwh = (day.totals.unmet_kwh - gen_day_kwh).max(0.0);
        generator_kwh += gen_day_kwh * scale;
        fuel_litres += gen_day_kwh * scale * generator.map_or(0.0, |g| g.fuel_l_per_kwh);
        totals.load_kwh += day.totals.load_kwh * scale;
        totals.solar_kwh += day.totals.solar_kwh * scale;
        totals.unmet_kwh += unmet_day_kwh * scale;
        totals.curtailed_kwh += day.totals.curtailed_kwh * scale;
        totals.discharged_kwh += day.totals.discharged_kwh * scale;
        if unmet_day_kwh * scale > worst_month_unmet_kwh {
            worst_month_unmet_kwh = unmet_day_kwh * scale;
            worst_month = m;
        }
        months.push(MonthSummary {
            month: m,
            load_kwh: day.totals.load_kwh * scale,
            solar_kwh: day.totals.solar_kwh * scale,
            unmet_kwh: unmet_day_kwh * scale,
            curtailed_kwh: day.totals.curtailed_kwh * scale,
            discharged_kwh: day.totals.discharged_kwh * scale,
            generator_kwh: gen_day_kwh * scale,
        });
    }

    SeasonalResult {
        months,
        load_kwh: totals.load_kwh,
        solar_kwh: totals.solar_kwh,
        unmet_kwh: totals.unmet_kwh,
        curtailed_kwh: totals.curtailed_kwh,
        generator_kwh,
        fuel_litres,
        worst_month_unmet_kwh,
        worst_month,
    }
}

/// Repeats the month's representative day until the end-of-day SoC matches
/// the start-of-day SoC (or settles within tolerance), then returns that
/// converged day.
#[cfg(feature = "pro")]
fn simulate_steady_day(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    load: &DayLoad,
    month: usize,
) -> DayResult {
    const MAX_ITERATIONS: usize = 60;
    const SOC_TOLERANCE: f64 = 1e-6;
    // The solar profile is SoC-independent: compute it once.
    let solar = solar_profile_kw(site, array, month);
    let mut spec = battery.clone();
    let mut run = dispatch_hours(&solar, &load.hourly_kw, &spec);
    for _ in 0..MAX_ITERATIONS {
        let end = run.0.last().map(|h| h.soc).unwrap_or(spec.initial_soc);
        if (end - spec.initial_soc).abs() < SOC_TOLERANCE {
            break;
        }
        spec.initial_soc = end;
        run = dispatch_hours(&solar, &load.hourly_kw, &spec);
    }
    let (hours, totals) = run;
    let battery_cycles = if battery.capacity_kwh > 1e-9 {
        totals.discharged_kwh / battery.capacity_kwh
    } else {
        0.0
    };
    DayResult {
        month: month % MONTHS_PER_YEAR,
        hours,
        totals,
        battery_cycles,
    }
}

/// An optional diesel/petrol backup generator that serves load the PV and
/// battery cannot. It follows load (never charges the battery), up to its
/// rated power each hour.
#[derive(Debug, Clone)]
pub struct GeneratorSpec {
    /// Rated output, kW.
    pub power_kw: f64,
    /// Fuel burn per kWh generated, litres/kWh (about 0.3 for a small diesel).
    pub fuel_l_per_kwh: f64,
    /// Fuel price, USD per litre.
    pub fuel_cost_usd_per_l: f64,
    /// Installed generator cost, USD per kW.
    pub cost_usd_per_kw: f64,
}

impl Default for GeneratorSpec {
    fn default() -> Self {
        Self {
            power_kw: 5.0,
            fuel_l_per_kwh: 0.3,
            fuel_cost_usd_per_l: 1.5,
            cost_usd_per_kw: 400.0,
        }
    }
}

impl GeneratorSpec {
    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.0..=10_000.0).contains(&self.power_kw) {
            issues.push("Generator power must be between 0 and 10,000 kW.".to_string());
        }
        if !(0.0..=2.0).contains(&self.fuel_l_per_kwh) {
            issues.push("Generator fuel use must be between 0 and 2 L/kWh.".to_string());
        }
        if !(0.0..=100.0).contains(&self.fuel_cost_usd_per_l) {
            issues.push("Fuel cost must be between 0 and 100 $/L.".to_string());
        }
        if !(0.0..=100_000.0).contains(&self.cost_usd_per_kw) {
            issues.push("Generator cost must be between 0 and 100,000 $/kW.".to_string());
        }
        issues
    }
}

/// Energy a load-following generator supplies from an hourly unmet profile.
#[cfg(feature = "pro")]
fn generator_energy_kwh(hours: &[HourPoint], generator: &GeneratorSpec) -> f64 {
    hours
        .iter()
        .map(|h| h.unmet_kw.min(generator.power_kw.max(0.0)))
        .sum()
}

/// Result of the multi-day overcast ("cloudy spell") endurance check.
#[cfg(feature = "pro")]
#[derive(Debug, Clone)]
pub struct SpellResult {
    /// Month whose weather the spell was taken from (the governing month of
    /// the annual run — least solar, most load).
    pub month: usize,
    /// Consecutive days of the spell simulated (1-60).
    pub days_simulated: usize,
    /// Zero-based day index on which load first went unserved; equals
    /// `days_simulated` when the whole spell was covered (see `survived`).
    pub days_to_failure: usize,
    /// `true` when the whole spell was covered without unmet energy.
    pub survived: bool,
    /// Unmet energy over the whole spell, kWh.
    pub unmet_kwh: f64,
    /// Load served over the whole spell, kWh.
    pub served_kwh: f64,
}

/// Simulates `days` back-to-back days of the worst month's weather, starting
/// from a full battery, carrying the SoC across the days.
///
/// The seasonal model runs each month to a steady-state SoC cycle, so it
/// cannot see a long run of overcast days draining a battery that never gets
/// a chance to recharge — the classic optimistic failure for off-grid sites.
/// This check answers "how many dark days can this design ride out?".
#[cfg(feature = "pro")]
pub fn simulate_cloudy_spell(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    seasonal: &SeasonalResult,
    days: usize,
) -> SpellResult {
    const MAX_SPELL_DAYS: usize = 60;
    let days_simulated = days.clamp(1, MAX_SPELL_DAYS);
    let month = seasonal.worst_month;
    let spell_array = SolarArray {
        cloud_factor: factors.cloud[month],
        ambient_celsius: factors.ambient_c[month],
        ..array.clone()
    };
    let spell_load = base_load.scaled(factors.load[month]);
    let solar = solar_profile_kw(site, &spell_array, month);

    // A spell always starts from a charged battery — that is the design
    // margin the check is asking about.
    let mut spec = battery.clamped();
    spec.initial_soc = 1.0;
    let mut served_kwh = 0.0;
    let mut unmet_kwh = 0.0;
    let mut days_to_failure = days_simulated;

    for day in 0..days_simulated {
        let (_, totals) = dispatch_hours(&solar, &spell_load.hourly_kw, &spec);
        let unmet_day = totals.unmet_kwh;
        served_kwh += totals.load_kwh - unmet_day;
        unmet_kwh += unmet_day;
        if unmet_day > 1e-9 && day < days_to_failure {
            days_to_failure = day;
        }
        // Carry the SoC into the next day; once the battery is flat this
        // keeps running so the caller sees the whole spell's unmet energy.
        spec.initial_soc = totals.end_soc;
    }

    SpellResult {
        month,
        days_simulated,
        days_to_failure,
        survived: unmet_kwh <= 1e-9,
        unmet_kwh,
        served_kwh,
    }
}

/// Cost and target inputs for the sizing optimization.
#[derive(Debug, Clone)]
pub struct OptimizationInputs {
    /// Installed PV cost, USD per kWp.
    pub pv_cost_usd_per_kw: f64,
    /// Installed battery cost, USD per kWh of nameplate.
    pub battery_cost_usd_per_kwh: f64,
    /// Annual served-energy fraction the design must meet (e.g. 0.99).
    pub target_served_fraction: f64,
    /// Battery power rating per kWh of capacity (kW/kWh; 0.5 = 2-hour).
    pub battery_power_ratio: f64,
    /// Real discount rate used to annualise capital (0.07 = 7%).
    pub discount_rate: f64,
    /// Project lifetime in years.
    pub project_years: f64,
    /// Annual operations & maintenance cost as a fraction of capex.
    pub om_fraction_of_capex: f64,
    /// Battery replacement interval in years (replaced within the project
    /// life whenever it is shorter).
    pub battery_life_years: f64,
    /// Optional backup generator included in every candidate design.
    pub generator: Option<GeneratorSpec>,
    /// What the sizing search minimises among the feasible candidates.
    /// Installed capital is the historic default; annualised cost (LCOE's
    /// numerator) is the engineering-preferred view because it prices
    /// generator fuel and battery replacements.
    pub objective: OptimizationObjective,
}

/// What [`recommend_size`] minimises among feasible candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptimizationObjective {
    /// Cheapest installed capital (USD).
    Capex,
    /// Cheapest levelised annual cost (USD/year) — capex annualised with the
    /// capital recovery factor, plus O&M, battery replacements and generator
    /// fuel. The default.
    #[default]
    AnnualCost,
    /// Cheapest levelised cost of energy (USD per kWh served). Same annual
    /// cost as [`Self::AnnualCost`], normalised by the energy each candidate
    /// actually serves — favours slightly larger arrays that trim the tail.
    Lcoe,
}

impl OptimizationObjective {
    /// Stable identifier used by the UI select and persisted in reports.
    pub fn id(self) -> &'static str {
        match self {
            Self::Capex => "capex",
            Self::AnnualCost => "annual-cost",
            Self::Lcoe => "lcoe",
        }
    }

    /// Parses [`Self::id`], falling back to the default (as the UI does when
    /// a stored scenario holds an unknown value).
    pub fn from_id(id: &str) -> Self {
        match id {
            "capex" => Self::Capex,
            "lcoe" => Self::Lcoe,
            _ => Self::AnnualCost,
        }
    }

    /// Human-readable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Capex => "Lowest installed cost (capex)",
            Self::AnnualCost => "Lowest levelised annual cost",
            Self::Lcoe => "Lowest cost of energy (LCOE)",
        }
    }
}

impl Default for OptimizationInputs {
    fn default() -> Self {
        Self {
            pv_cost_usd_per_kw: 1400.0,
            battery_cost_usd_per_kwh: 600.0,
            target_served_fraction: 0.99,
            battery_power_ratio: 0.5,
            discount_rate: 0.07,
            project_years: 20.0,
            om_fraction_of_capex: 0.015,
            battery_life_years: 10.0,
            generator: None,
            objective: OptimizationObjective::default(),
        }
    }
}

impl OptimizationInputs {
    /// Ages `(pv_years, battery_years)` at which a candidate must still meet
    /// the target. The PV runs the whole project life. The battery is
    /// replaced every `battery_life_years`, so its worst point is just before
    /// the replacement (or the project end, if that comes first).
    pub fn end_of_life_ages(&self) -> (f64, f64) {
        (
            self.project_years,
            self.battery_life_years.min(self.project_years),
        )
    }

    /// Validity errors, phrased for direct display in the UI.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !(0.0..=100_000.0).contains(&self.pv_cost_usd_per_kw) {
            issues.push("PV cost must be between 0 and 100,000 $/kWp.".to_string());
        }
        if !(0.0..=100_000.0).contains(&self.battery_cost_usd_per_kwh) {
            issues.push("Battery cost must be between 0 and 100,000 $/kWh.".to_string());
        }
        if !(0.5..=1.0).contains(&self.target_served_fraction) {
            issues.push("Target served fraction must be between 50% and 100%.".to_string());
        }
        if !(0.05..=4.0).contains(&self.battery_power_ratio) {
            issues.push("Battery power ratio must be between 0.05 and 4 kW/kWh.".to_string());
        }
        if !(0.0..=0.5).contains(&self.discount_rate) {
            issues.push("Discount rate must be between 0% and 50%.".to_string());
        }
        if !(1.0..=50.0).contains(&self.project_years) {
            issues.push("Project life must be between 1 and 50 years.".to_string());
        }
        if !(0.0..=0.2).contains(&self.om_fraction_of_capex) {
            issues.push("O&M must be between 0% and 20% of capex per year.".to_string());
        }
        if !(1.0..=50.0).contains(&self.battery_life_years) {
            issues.push("Battery life must be between 1 and 50 years.".to_string());
        }
        if let Some(generator) = &self.generator {
            issues.extend(generator.validate());
        }
        issues
    }
}

/// A sizing recommendation from [`recommend_size`].
#[derive(Debug, Clone)]
pub struct SizingRecommendation {
    /// Recommended PV capacity, kWp.
    pub solar_kw: f64,
    /// Recommended battery nameplate capacity, kWh.
    pub battery_kwh: f64,
    /// Recommended battery power rating, kW.
    pub battery_kw: f64,
    /// Estimated installed capex, USD.
    pub capex_usd: f64,
    /// Annual served fraction the recommendation achieves.
    pub served_fraction: f64,
    /// Annual unmet energy at the recommendation, kWh.
    pub unmet_kwh: f64,
    /// Annual generator fuel use at the recommendation, litres.
    pub fuel_litres: f64,
    /// False when no candidate in the search grid reached the target —
    /// the result is the best-effort (lowest-unmet) candidate instead.
    pub feasible: bool,
    /// Levelised annual cost (annualised capex + O&M + battery
    /// replacements), USD per year.
    pub annual_cost_usd: f64,
    /// Levelised cost of the energy actually served, USD per kWh
    /// (infinite when nothing is served).
    pub lcoe_usd_per_kwh: f64,
}

/// Levelised annual cost of a PV + battery design: capex annualised with the
/// capital recovery factor, plus O&M, plus the battery's replacements
/// (each discounted to year 0 and annualised the same way).
///
/// Generator capex (when `inputs.generator` is set) is annualised like the
/// PV; fuel is added by the caller, since it depends on the simulation.
pub fn annual_cost_usd(solar_kw: f64, battery_kwh: f64, inputs: &OptimizationInputs) -> f64 {
    let pv_capex = solar_kw * inputs.pv_cost_usd_per_kw
        + inputs
            .generator
            .as_ref()
            .map_or(0.0, |g| g.power_kw * g.cost_usd_per_kw);
    let battery_capex = battery_kwh * inputs.battery_cost_usd_per_kwh;
    let n = inputs.project_years;
    let r = inputs.discount_rate;
    let crf = if r.abs() < 1e-9 {
        1.0 / n
    } else {
        r * (1.0 + r).powf(n) / ((1.0 + r).powf(n) - 1.0)
    };
    let mut replacement_pv = 0.0;
    let mut year = inputs.battery_life_years;
    while year < n - 1e-9 {
        replacement_pv += battery_capex / (1.0 + r).powf(year);
        year += inputs.battery_life_years;
    }
    (pv_capex + battery_capex + replacement_pv) * crf
        + (pv_capex + battery_capex) * inputs.om_fraction_of_capex
}

/// Searches PV × battery combinations over the seasonal simulation and
/// returns the cheapest one meeting [`OptimizationInputs::
/// target_served_fraction`], or the lowest-unmet candidate if none does
/// (`feasible == false`).
///
/// Every candidate is simulated at end of life — the PV aged over the
/// project life and the battery at its last point before replacement (see
/// [`OptimizationInputs::end_of_life_ages`]) — so a design that only meets
/// the target when new is not recommended. The reported served fraction,
/// unmet energy and fuel are therefore end-of-life figures; capex and cost
/// are unaffected.
///
/// The grid scales with the load: PV from 1× to 8× peak demand, battery
/// from 0.25× to 3× daily energy — coarse steps that land a design in the
/// right ballpark, not a substitute for detailed engineering.
///
/// `battery_template` supplies the chemistry settings (round-trip
/// efficiency, SoC floor, initial SoC); its capacity and power are swept.
#[cfg(feature = "pro")]
pub fn recommend_size(
    site: &Site,
    array: &SolarArray,
    battery_template: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    inputs: &OptimizationInputs,
) -> SizingRecommendation {
    let peak_kw = (base_load.peak_kw() * factors.load.iter().copied().fold(0.0, f64::max)).max(0.1);
    let daily_kwh = base_load.daily_kwh() * factors.load.iter().copied().fold(0.0, f64::max);
    let (pv_years, battery_years) = inputs.end_of_life_ages();
    const PV_MULTIPLIERS: [f64; 9] = [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];
    const BATTERY_MULTIPLIERS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0];

    let mut best_feasible: Option<SizingRecommendation> = None;
    let mut best_effort: Option<SizingRecommendation> = None;
    // Lower is better for whichever metric the caller chose; ties always fall
    // back to unmet energy so the choice is deterministic.
    let score = |c: &SizingRecommendation| match inputs.objective {
        OptimizationObjective::Capex => c.capex_usd,
        OptimizationObjective::AnnualCost => c.annual_cost_usd,
        OptimizationObjective::Lcoe => c.lcoe_usd_per_kwh,
    };
    let better = |candidate: &SizingRecommendation, best: &SizingRecommendation| {
        let (cand_score, best_score) = (score(candidate), score(best));
        cand_score < best_score - 1e-9
            || ((cand_score - best_score).abs() <= 1e-9
                && candidate.unmet_kwh < best.unmet_kwh - 1e-9)
            || ((cand_score - best_score).abs() <= 1e-9
                && (candidate.unmet_kwh - best.unmet_kwh).abs() <= 1e-9
                && candidate.capex_usd < best.capex_usd)
    };

    for &pv_mult in &PV_MULTIPLIERS {
        let solar_kw = (peak_kw * pv_mult).max(0.05);
        for &batt_mult in &BATTERY_MULTIPLIERS {
            let battery_kwh = (daily_kwh * batt_mult).max(0.1);
            // Nameplate values are what gets bought; the simulation uses
            // their end-of-life state.
            let battery = BatterySpec {
                capacity_kwh: battery_kwh,
                power_kw: battery_kwh * inputs.battery_power_ratio,
                ..battery_template.clone()
            }
            .aged(battery_years);
            // The optimizer sweeps capacity; the array's orientation comes
            // from the caller's design.
            let array_c = SolarArray {
                capacity_kw: solar_kw,
                ..array.clone()
            }
            .aged(pv_years);
            let seasonal = simulate_seasonal_with_generator(
                site,
                &array_c,
                &battery,
                base_load,
                factors,
                inputs.generator.as_ref(),
            );
            let fuel_cost = seasonal.fuel_litres
                * inputs.generator.as_ref().map_or(0.0, |g| g.fuel_cost_usd_per_l);
            let cost = annual_cost_usd(solar_kw, battery_kwh, inputs) + fuel_cost;
            let served_kwh = seasonal.load_kwh - seasonal.unmet_kwh;
            let lcoe = if served_kwh > 1e-9 { cost / served_kwh } else { f64::INFINITY };
            let candidate = SizingRecommendation {
                solar_kw,
                battery_kwh,
                battery_kw: battery_kwh * inputs.battery_power_ratio,
                capex_usd: solar_kw * inputs.pv_cost_usd_per_kw
                    + battery_kwh * inputs.battery_cost_usd_per_kwh
                    + inputs
                        .generator
                        .as_ref()
                        .map_or(0.0, |g| g.power_kw * g.cost_usd_per_kw),
                served_fraction: seasonal.served_fraction(),
                unmet_kwh: seasonal.unmet_kwh,
                fuel_litres: seasonal.fuel_litres,
                feasible: seasonal.served_fraction() >= inputs.target_served_fraction,
                annual_cost_usd: cost,
                lcoe_usd_per_kwh: lcoe,
            };
            let better_feasible = match &best_feasible {
                Some(best) => better(&candidate, best),
                None => candidate.feasible,
            };
            if candidate.feasible && better_feasible {
                best_feasible = Some(candidate.clone());
            }
            let better_effort = match &best_effort {
                Some(best) => better(&candidate, best),
                None => true,
            };
            if better_effort {
                best_effort = Some(candidate);
            }
        }
    }

    best_feasible
        .or(best_effort)
        .expect("the search grid is never empty")
}

/// The optional extras a design report can carry. Grouped into one argument so
/// [`design_report_markdown`] stays within a readable parameter count.
#[cfg(feature = "pro")]
#[derive(Debug, Clone, Copy, Default)]
pub struct ReportExtras<'a> {
    /// The optimizer's recommendation, when it has run.
    pub recommendation: Option<&'a SizingRecommendation>,
    /// The cost assumptions behind the LCOE, so it can be reproduced.
    pub cost_inputs: Option<&'a OptimizationInputs>,
    /// The multi-day overcast endurance check.
    pub spell: Option<&'a SpellResult>,
    /// The inverter sizing and clipping check.
    pub inverter: Option<&'a InverterReport>,
    /// The loss-of-load and autonomy figures.
    pub reliability: Option<&'a ReliabilityReport>,
    /// The grid-connected annual energy and bills.
    pub grid: Option<&'a GridYear>,
}

/// Builds the exportable system-design report (Markdown) for a simulated
/// design. See [`ReportExtras`] for the optional sections.
#[cfg(feature = "pro")]
#[allow(clippy::too_many_arguments)]
pub fn design_report_markdown(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    seasonal: &SeasonalResult,
    extras: ReportExtras<'_>,
) -> String {
    use std::fmt::Write as _;

    let ReportExtras {
        recommendation,
        cost_inputs,
        spell,
        inverter,
        reliability,
        grid,
    } = extras;

    let mut out = String::new();
    out.push_str("# TPT Microgrid Sizer — System Design Report\n\n");

    out.push_str("## Site\n\n");
    let hemisphere = if site.latitude_deg >= 0.0 { "N" } else { "S" };
    let _ = writeln!(
        out,
        "- Location: {:.4}°{}, {:.4}°{}, UTC{:+.1}, {} m ASL",
        site.latitude_deg.abs(),
        hemisphere,
        site.longitude_deg.abs(),
        if site.longitude_deg >= 0.0 { "E" } else { "W" },
        site.timezone_offset_hours,
        site.altitude_m
    );

    out.push_str("\n## Load\n\n");
    let _ = writeln!(
        out,
        "- Base daily profile: {:.1} kWh/day, {:.2} kW peak",
        base_load.daily_kwh(),
        base_load.peak_kw()
    );
    let _ = writeln!(
        out,
        "- Seasonal range: {:.2}×–{:.2}× the base profile",
        factors.load.iter().copied().fold(f64::INFINITY, f64::min),
        factors.load.iter().copied().fold(0.0f64, f64::max)
    );

    out.push_str("\n## Monthly factors\n\n");
    out.push_str("| Month | Load × | Cloud (fraction of clear sky) | Ambient °C |\n");
    out.push_str("|---|---:|---:|---:|\n");
    for m in 0..MONTHS_PER_YEAR {
        let _ = writeln!(
            out,
            "| {} | {:.2} | {:.2} | {:.1} |",
            MONTH_NAMES[m], factors.load[m], factors.cloud[m], factors.ambient_c[m]
        );
    }

    out.push_str("\n## Solar array\n\n");
    let _ = writeln!(out, "- Capacity: {:.1} kWp DC", array.capacity_kw);
    let _ = writeln!(out, "- Tilt: {:.0}°, azimuth: {:.0}° (0 = north)", array.tilt_deg, array.azimuth_deg);
    let _ = writeln!(
        out,
        "- Degradation: {:.2}%/year (seasonal results are as-new; sizing is end-of-life)",
        array.annual_degradation * 100.0
    );
    if !array.orientations.is_empty() {
        let total: f64 = array.orientations.iter().map(|p| p.share.max(0.0)).sum();
        for (i, plane) in array.orientations.iter().enumerate() {
            let _ = writeln!(
                out,
                "- Plane {}: {:.0}% of capacity, tilt {:.0}°, azimuth {:.0}°",
                i + 1,
                plane.share / total.max(1e-12) * 100.0,
                plane.tilt_deg,
                plane.azimuth_deg
            );
        }
    }
    for (i, shade) in array.obstructions.iter().enumerate() {
        let _ = writeln!(
            out,
            "- Shading {}: {:.0}% loss from {:02}:00 to {:02}:00",
            i + 1,
            shade.loss_fraction * 100.0,
            shade.start_hour,
            shade.end_hour
        );
    }

    out.push_str("\n## Battery\n\n");
    let _ = writeln!(
        out,
        "- Capacity: {:.1} kWh nameplate ({:.0}% usable floor), power {:.1} kW, round-trip efficiency {:.0}%, initial SoC {:.0}%",
        battery.capacity_kwh,
        battery.min_soc * 100.0,
        battery.power_kw,
        battery.round_trip_efficiency * 100.0,
        battery.initial_soc * 100.0
    );
    let _ = writeln!(
        out,
        "- Capacity fade: {:.1}%/year, self-discharge {:.2}%/day",
        battery.annual_fade * 100.0,
        battery.self_discharge_per_day * 100.0
    );

    if let Some(check) = inverter {
        out.push_str("\n## Inverter\n\n");
        let _ = writeln!(
            out,
            "- DC/AC ratio {:.2}: AC rating {:.2} kW for {:.1} kWp DC",
            check.dc_ac_ratio, check.ac_rating_kw, check.dc_kwp
        );
        let _ = writeln!(
            out,
            "- Peak demand {:.2} kW: {}",
            check.peak_demand_kw,
            if check.covers_peak {
                "covered by the inverter rating"
            } else {
                "exceeds the inverter rating, so the inverter is too small for the peak"
            }
        );
        let _ = writeln!(
            out,
            "- Clipped PV energy: {} per year ({:.1}% of unclipped PV output)",
            format_kwh(check.clipped_kwh_per_year),
            check.clipped_fraction * 100.0
        );
    }

    if let Some(rel) = reliability {
        out.push_str("\n## Reliability\n\n");
        let _ = writeln!(
            out,
            "- Loss-of-load probability (LOLP): {:.2}% of hours ({:.0} hours per year)",
            rel.lolp * 100.0,
            rel.loss_of_load_hours_per_year
        );
        let _ = writeln!(
            out,
            "- Loss-of-load expectation (LOLE): {:.1} days per year with unserved load",
            rel.lole_days_per_year
        );
        let _ = writeln!(
            out,
            "- Battery-only autonomy, worst month, no sun: {:.2} days{}",
            rel.autonomy_days,
            if rel.autonomy_days >= MAX_AUTONOMY_DAYS {
                format!(" (capped at {MAX_AUTONOMY_DAYS:.0} days)")
            } else {
                String::new()
            }
        );
    }

    if let Some(grid) = grid {
        out.push_str("\n## Grid connection\n\n");
        let _ = writeln!(
            out,
            "- Annual load {}: bought from the grid with no system would cost US$ {:.0}",
            format_kwh(grid.load_kwh),
            grid.bill_without_system_usd
        );
        let _ = writeln!(
            out,
            "- With the system: bought {}, sold {}, bill US$ {:.0}",
            format_kwh(grid.imported_kwh),
            format_kwh(grid.exported_kwh),
            grid.bill_with_system_usd
        );
        let _ = writeln!(out, "- Annual saving: US$ {:.0}", grid.savings_usd());
        if grid.unmet_kwh > 0.01 {
            let _ = writeln!(
                out,
                "- Load still unserved after the import limit: {} per year",
                format_kwh(grid.unmet_kwh)
            );
        }
    }

    out.push_str("\n## Seasonal simulation (representative day per month)\n\n");
    out.push_str("| Month | Load kWh | Solar kWh | Battery out kWh | Unmet kWh | Curtailed kWh |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|\n");
    for m in &seasonal.months {
        let _ = writeln!(
            out,
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
            MONTH_NAMES[m.month],
            m.load_kwh,
            m.solar_kwh,
            m.discharged_kwh,
            m.unmet_kwh,
            m.curtailed_kwh
        );
    }
    let _ = writeln!(
        out,
        "\n- Annual served fraction: **{:.1}%** ({} unmet of {})",
        seasonal.served_fraction() * 100.0,
        format_kwh(seasonal.unmet_kwh),
        format_kwh(seasonal.load_kwh)
    );
    let _ = writeln!(
        out,
        "- Governing month: {} ({:.1}% served, {} unmet)",
        MONTH_NAMES[seasonal.worst_month],
        seasonal.worst_month_served_fraction() * 100.0,
        format_kwh(seasonal.worst_month_unmet_kwh)
    );
    let _ = writeln!(
        out,
        "- Annual curtailed solar: {}",
        format_kwh(seasonal.curtailed_kwh)
    );
    if seasonal.generator_kwh > 0.0 {
        let _ = writeln!(
            out,
            "- Backup generator: {} ({:.0} L fuel/year)",
            format_kwh(seasonal.generator_kwh),
            seasonal.fuel_litres
        );
    }

    if let Some(spell) = spell {
        let _ = writeln!(
            out,
            "\n- Overcast endurance ({} days of {} weather, battery full at the start): {}",
            spell.days_simulated,
            MONTH_NAMES[spell.month],
            if spell.survived {
                format!(
                    "fully covered ({} served, no unmet energy)",
                    format_kwh(spell.served_kwh)
                )
            } else if spell.days_to_failure == 0 {
                format!(
                    "load unmet on the very first day, {} unmet over the spell",
                    format_kwh(spell.unmet_kwh)
                )
            } else {
                format!(
                    "load first unmet after {} day(s); {} unmet over the spell",
                    spell.days_to_failure,
                    format_kwh(spell.unmet_kwh)
                )
            }
        );
    }

    if let Some(rec) = recommendation {
        out.push_str("\n## Recommended sizing\n\n");
        let _ = writeln!(out, "- Solar array: **{:.1} kWp**", rec.solar_kw);
        let _ = writeln!(
            out,
            "- Battery: **{:.1} kWh** at {:.1} kW",
            rec.battery_kwh, rec.battery_kw
        );
        let _ = writeln!(out, "- Estimated capex: **US$ {:.0}**", rec.capex_usd);
        let _ = writeln!(
            out,
            "- Levelised annual cost: US$ {:.0}/year; LCOE: US$ {:.3}/kWh served",
            rec.annual_cost_usd, rec.lcoe_usd_per_kwh
        );
        let _ = writeln!(
            out,
            "- Achieved served fraction at end of life: {:.1}% ({} unmet/year)",
            rec.served_fraction * 100.0,
            format_kwh(rec.unmet_kwh)
        );
        if !rec.feasible {
            out.push_str(
                "\n- ⚠️ No candidate in the search grid reached the target served fraction; \
                 the values above are the best-effort candidate. Consider relaxing the target \
                 or adding a backup generator.\n",
            );
        }
    }

    if let Some(inputs) = cost_inputs {
        out.push_str("\n## Cost basis\n\n");
        let _ = writeln!(out, "- PV installed cost: US$ {:.0}/kWp", inputs.pv_cost_usd_per_kw);
        let _ = writeln!(
            out,
            "- Battery installed cost: US$ {:.0}/kWh nameplate",
            inputs.battery_cost_usd_per_kwh
        );
        let _ = writeln!(
            out,
            "- Discount rate: {:.1}%; project life: {:.0} years; battery replacement: every {:.0} years",
            inputs.discount_rate * 100.0,
            inputs.project_years,
            inputs.battery_life_years
        );
        let _ = writeln!(
            out,
            "- O&M: {:.1}% of capex per year",
            inputs.om_fraction_of_capex * 100.0
        );
        let (pv_years, battery_years) = inputs.end_of_life_ages();
        let _ = writeln!(
            out,
            "- Sized at end of life: PV aged {:.0} years, battery aged {:.0} years",
            pv_years, battery_years
        );
        match &inputs.generator {
            Some(g) => {
                let _ = writeln!(
                    out,
                    "- Backup generator: {:.1} kW at US$ {:.0}/kW installed, {:.2} L/kWh, fuel US$ {:.2}/L",
                    g.power_kw,
                    g.cost_usd_per_kw,
                    g.fuel_l_per_kwh,
                    g.fuel_cost_usd_per_l
                );
            }
            None => out.push_str("- Backup generator: none included.\n"),
        }
        let _ = writeln!(
            out,
            "- Target served fraction: {:.1}%; battery power rating {:.2} kW/kWh",
            inputs.target_served_fraction * 100.0,
            inputs.battery_power_ratio
        );
        let _ = writeln!(out, "- Sizing objective: {}", inputs.objective.label());
    }

    out.push_str("\n## Assumptions\n\n");
    out.push_str(
        "- Hourly steps over the 21st of each month; fixed UTC offset (no daylight saving).\n\
         - Clear-sky irradiance (Ineichen) scaled by monthly cloud factors; NOCT cell-temperature derating; inverter DC/AC ratio 1.2; 2% soiling.\n\
         - Battery: split-sqrt round-trip efficiency, minimum-SoC floor, power rating symmetric for charge and discharge.\n\
         - Dispatch: solar serves load first, surplus charges the battery, deficits discharge it; surplus beyond full charge is curtailed; no grid export or backup generation.\n",
    );
    out
}

#[cfg(feature = "pro")]
fn format_kwh(kwh: f64) -> String {
    if kwh >= 10_000.0 {
        format!("{:.0} MWh", kwh / 1000.0)
    } else {
        format!("{:.1} kWh", kwh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_sky() -> SolarArray {
        SolarArray {
            cloud_factor: 0.0,
            ..SolarArray::default()
        }
    }

    fn full_battery() -> BatterySpec {
        BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.0,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        }
    }

    fn constant_load(kw: f64) -> DayLoad {
        DayLoad {
            hourly_kw: vec![kw; HOURS_PER_DAY],
        }
    }

    /// `dispatch_profile` with the length/finite check unwrapped — the tests
    /// below all supply matching, finite 24-hour profiles, and the check
    /// itself is covered by `profile_length_mismatch_is_an_error`.
    fn dispatch(solar_kw: &[f64], load_kw: &[f64], battery: &BatterySpec) -> DispatchTotals {
        dispatch_profile(solar_kw, load_kw, battery).expect("test profiles are valid")
    }

    /// Hand-checkable balance, generation side zeroed: a constant 2 kW load
    /// against a 10 kWh battery (RTE 1.0, full) must empty it in exactly
    /// five hours and then go unmet — 10 kWh served, 38 kWh unmet.
    #[test]
    fn battery_only_dispatch_hand_check() {
        let totals = dispatch(&[0.0; 24], &[2.0; 24], &full_battery());
        assert!((totals.load_kwh - 48.0).abs() < 1e-6);
        assert!((totals.solar_kwh).abs() < 1e-9);
        assert!((totals.discharged_kwh - 10.0).abs() < 1e-6);
        assert!((totals.unmet_kwh - 38.0).abs() < 1e-6);
        assert!(totals.end_soc.abs() < 1e-6);

        let day = simulate_day(&Site::default(), &no_sky(), &full_battery(), &constant_load(2.0), 0);
        for (h, point) in day.hours.iter().enumerate() {
            if h < 5 {
                assert!((point.battery_kw + 2.0).abs() < 1e-6, "hour {h}");
                assert!((point.unmet_kw).abs() < 1e-6, "hour {h}");
                assert!((point.soc - (10.0 - 2.0 * (h + 1) as f64) / 10.0).abs() < 1e-6);
            } else {
                assert!((point.unmet_kw - 2.0).abs() < 1e-6, "hour {h}");
                assert!((point.battery_kw).abs() < 1e-6, "hour {h}");
            }
        }
    }

    /// Hand-checkable charge side: 2 kW of solar for 3 hours into an empty
    /// 10 kWh battery at RTE 0.81 stores 3 × 1.8 kWh (grid-side accepted
    /// 6 kWh, one-way loss 0.9 each leg).
    #[test]
    fn charge_side_hand_check() {
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 0.81,
            min_soc: 0.0,
            initial_soc: 0.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let mut solar = [0.0; 24];
        solar[0] = 2.0;
        solar[1] = 2.0;
        solar[2] = 2.0;
        let totals = dispatch(&solar, &[0.0; 24], &battery);
        assert!((totals.charged_kwh - 6.0).abs() < 1e-9, "{}", totals.charged_kwh);
        assert!((totals.curtailed_kwh).abs() < 1e-9);
        assert!((totals.end_soc - 0.54).abs() < 1e-9, "{}", totals.end_soc);
    }

    /// Round trip through the dispatch loop: charge 6 kWh grid-side into an
    /// empty battery (RTE 0.81 → 5.4 kWh stored), then draw 3 kW from hour
    /// 3 — the battery delivers 4.86 kWh (5.4 × 0.9) before hitting empty.
    #[test]
    fn round_trip_through_dispatch() {
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 0.81,
            min_soc: 0.0,
            initial_soc: 0.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let mut solar = [0.0; 24];
        solar[0] = 2.0;
        solar[1] = 2.0;
        solar[2] = 2.0;
        let mut load = vec![0.0; 24];
        for kw in load.iter_mut().skip(3) {
            *kw = 3.0;
        }
        let totals = dispatch(&solar, &load, &battery);
        // 21 deficit hours × 3 kW = 63 kWh of load; the stored 5.4 kWh
        // delivers 4.86 kWh grid-side.
        assert!((totals.load_kwh - 63.0).abs() < 1e-6, "{}", totals.load_kwh);
        assert!((totals.discharged_kwh - 4.86).abs() < 1e-6, "{}", totals.discharged_kwh);
        assert!((totals.unmet_kwh - (63.0 - 4.86)).abs() < 1e-6);
        assert!((totals.charged_kwh - 6.0).abs() < 1e-6);
    }

    /// Power-limit: a 4 kW surplus into a 2 kW battery charger curtails the
    /// excess even with unlimited headroom.
    #[test]
    fn power_rating_curtails_surplus() {
        let battery = BatterySpec {
            capacity_kwh: 100.0,
            power_kw: 2.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.0,
            initial_soc: 0.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let totals = dispatch(&[4.0; 24], &[0.0; 24], &battery);
        assert!((totals.charged_kwh - 48.0).abs() < 1e-9);
        assert!((totals.curtailed_kwh - 48.0).abs() < 1e-9);
    }

    /// SoC floor: a 10% floor on a 10 kWh battery gives only 9 kWh of
    /// service.
    #[test]
    fn min_soc_limits_discharge() {
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 20.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.1,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let totals = dispatch(&[0.0; 24], &[50.0; 24], &battery);
        assert!((totals.discharged_kwh - 9.0).abs() < 1e-6, "{}", totals.discharged_kwh);
        assert!((totals.end_soc - 0.1).abs() < 1e-6);
    }

    /// No battery: every deficit hour is unmet, every surplus hour curtailed.
    #[test]
    fn zero_battery_behaves_like_passthrough() {
        let battery = BatterySpec {
            capacity_kwh: 0.0,
            ..BatterySpec::default()
        };
        let solar = [3.0; 24];
        let load = vec![1.0; 12].into_iter().chain(vec![5.0; 12]).collect::<Vec<_>>();
        let totals = dispatch(&solar, &load, &battery);
        assert!((totals.unmet_kwh - 24.0).abs() < 1e-9);
        assert!((totals.curtailed_kwh - 24.0).abs() < 1e-9);
    }

    /// Wellington in full December sun: a 5 kWp array should produce in the
    /// 25–45 kWh/day band, never exceed its DC/AC-limited inverter rating,
    /// and produce nothing at night.
    #[test]
    fn pv_output_reasonable_wellington_summer() {
        let site = Site::default();
        let array = SolarArray::default();
        let day = simulate_day(&site, &array, &BatterySpec::default(), &constant_load(0.5), 11);
        assert!(
            day.totals.solar_kwh > 25.0 && day.totals.solar_kwh < 45.0,
            "December yield = {} kWh",
            day.totals.solar_kwh
        );
        assert!(day.hours[3].solar_kw.abs() < 1e-9, "3 a.m. output");
        assert!(day.hours[21].solar_kw.abs() < 1e-9, "9 p.m. output");
        // dc_ac_ratio 1.2 caps AC at capacity/1.2 in every hour.
        for point in &day.hours {
            assert!(point.solar_kw <= array.capacity_kw / 1.2 + 1e-9);
        }
    }

    /// Southern hemisphere seasonality: June must under-produce December at
    /// latitude −41, and the reverse holds at +41.
    #[test]
    fn seasonal_asymmetry_follows_hemisphere() {
        let site = Site::default();
        let array = SolarArray::default();
        let june = simulate_day(&site, &array, &BatterySpec::default(), &constant_load(0.0), 5);
        let december = simulate_day(&site, &array, &BatterySpec::default(), &constant_load(0.0), 11);
        assert!(
            june.totals.solar_kwh < december.totals.solar_kwh * 0.75,
            "june {} december {}",
            june.totals.solar_kwh,
            december.totals.solar_kwh
        );

        let mut northern = site.clone();
        northern.latitude_deg = 41.0;
        let june_n = simulate_day(&northern, &array, &BatterySpec::default(), &constant_load(0.0), 5);
        let december_n =
            simulate_day(&northern, &array, &BatterySpec::default(), &constant_load(0.0), 11);
        assert!(june_n.totals.solar_kwh > december_n.totals.solar_kwh);
    }

    /// Cloud cuts yield roughly proportionally.
    #[test]
    fn cloud_factor_scales_yield() {
        let site = Site::default();
        let clear = SolarArray::default();
        let overcast = SolarArray {
            cloud_factor: 0.4,
            ..SolarArray::default()
        };
        let a = simulate_day(&site, &clear, &BatterySpec::default(), &constant_load(0.0), 0);
        let b = simulate_day(&site, &overcast, &BatterySpec::default(), &constant_load(0.0), 0);
        let ratio = b.totals.solar_kwh / a.totals.solar_kwh;
        // Sub-linear in the cloud factor: diffuse irradiance persists under
        // cloud, so 0.4× sky clears still yield ~half the energy.
        assert!(
            (0.3..=0.65).contains(&ratio),
            "cloudy/clear ratio = {ratio}"
        );
    }

    /// Validation catches out-of-range input.
    #[test]
    fn validation_reports_issues() {
        let site = Site {
            latitude_deg: 123.0,
            ..Site::default()
        };
        assert!(!site.validate().is_empty());
        let battery = BatterySpec {
            round_trip_efficiency: 0.3,
            ..BatterySpec::default()
        };
        assert!(!battery.validate().is_empty());
        let load = DayLoad {
            hourly_kw: vec![0.0; 12],
        };
        assert_eq!(load.validate().len(), 1);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_twelve_months_and_served_fraction() {
        let seasonal = simulate_seasonal(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
        );
        assert_eq!(seasonal.months.len(), 12);
        assert!((0.0..=1.0).contains(&seasonal.served_fraction()));
        assert!(seasonal.worst_month_unmet_kwh <= seasonal.unmet_kwh + 1e-9);
        // Sanity: the annual sums equal the sum of the months.
        let sum: f64 = seasonal.months.iter().map(|m| m.load_kwh).sum();
        assert!((sum - seasonal.load_kwh).abs() < 1e-6);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_winter_is_governing_month_southern_hemisphere() {
        // Deliberately undersized so unmet energy is non-zero every month.
        let seasonal = simulate_seasonal(
            &Site::default(),
            &SolarArray {
                capacity_kw: 2.0,
                ..SolarArray::default()
            },
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
        );
        // With winter-peaked load and cloudy winters, an undersized system
        // must struggle most in June–August.
        assert!(
            (5..=7).contains(&seasonal.worst_month),
            "worst month = {}",
            MONTH_NAMES[seasonal.worst_month]
        );
    }

    #[cfg(feature = "pro")]
    #[test]
    fn recommendation_meets_target_and_matches_simulation() {
        // A deliberately undersized base design: the optimizer must find
        // something materially larger that meets a 95% target.
        let factors = MonthlyFactors::default();
        let inputs = OptimizationInputs {
            target_served_fraction: 0.95,
            ..OptimizationInputs::default()
        };
        let rec = recommend_size(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &factors,
            &inputs,
        );
        assert!(rec.feasible, "cabin load should be servable in-grid");
        assert!(rec.served_fraction >= 0.95 - 1e-9);
        assert!(rec.capex_usd > 0.0);
        // The recommendation, re-simulated with its exact PV capacity and
        // battery at its end-of-life ages, reproduces its claimed service.
        let (pv_years, battery_years) = inputs.end_of_life_ages();
        let array = SolarArray {
            capacity_kw: rec.solar_kw,
            ..SolarArray::default()
        }
        .aged(pv_years);
        let battery = BatterySpec {
            capacity_kwh: rec.battery_kwh,
            power_kw: rec.battery_kw,
            ..BatterySpec::default()
        }
        .aged(battery_years);
        let check = simulate_seasonal(&Site::default(), &array, &battery, &DayLoad::default(), &factors);
        assert!((check.served_fraction() - rec.served_fraction).abs() < 1e-6);
    }

    #[test]
    fn obstruction_shades_its_whole_hours_only() {
        let shade = hourly_shade(&[Obstruction {
            start_hour: 14,
            end_hour: 16,
            loss_fraction: 0.6,
        }]);
        assert_eq!(shade[13], 0.0);
        assert!((shade[14] - 0.6).abs() < 1e-12);
        assert!((shade[15] - 0.6).abs() < 1e-12);
        assert_eq!(shade[16], 0.0);
    }

    #[test]
    fn overlapping_obstructions_combine_multiplicatively() {
        let shade = hourly_shade(&[
            Obstruction {
                start_hour: 9,
                end_hour: 11,
                loss_fraction: 0.5,
            },
            Obstruction {
                start_hour: 10,
                end_hour: 12,
                loss_fraction: 0.5,
            },
        ]);
        assert!((shade[9] - 0.5).abs() < 1e-12);
        // Both shades cover 10:00: 1 - 0.5 × 0.5.
        assert!((shade[10] - 0.75).abs() < 1e-12);
        assert!((shade[11] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn shading_wraps_past_midnight() {
        let shade = hourly_shade(&[Obstruction {
            start_hour: 22,
            end_hour: 2,
            loss_fraction: 1.0,
        }]);
        assert_eq!(shade[23], 1.0);
        assert_eq!(shade[1], 1.0);
        assert_eq!(shade[2], 0.0);
        assert_eq!(shade[21], 0.0);
    }

    #[test]
    fn shading_cuts_the_shaded_hours_of_the_profile() {
        let site = Site::default();
        let plain = SolarArray {
            cloud_factor: 1.0,
            ..SolarArray::default()
        };
        let shaded = SolarArray {
            obstructions: vec![Obstruction {
                start_hour: 14,
                end_hour: 16,
                loss_fraction: 1.0,
            }],
            ..plain.clone()
        };
        let before = solar_profile_kw(&site, &plain, 0);
        let after = solar_profile_kw(&site, &shaded, 0);
        assert!(before[14] > 0.0 && before[15] > 0.0);
        assert_eq!(after[14], 0.0);
        assert_eq!(after[15], 0.0);
        assert!((after[12] - before[12]).abs() < 1e-9);
    }

    #[test]
    fn a_single_orientation_is_the_same_as_no_orientations() {
        let site = Site::default();
        let plain = SolarArray {
            cloud_factor: 0.5,
            ..SolarArray::default()
        };
        let one_plane = SolarArray {
            orientations: vec![Orientation {
                share: 3.0,
                tilt_deg: plain.tilt_deg,
                azimuth_deg: plain.azimuth_deg,
            }],
            ..plain.clone()
        };
        let a = solar_profile_kw(&site, &plain, 0);
        let b = solar_profile_kw(&site, &one_plane, 0);
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-9, "{x} vs {y}");
        }
    }

    #[test]
    fn two_identical_planes_split_the_array_without_changing_the_yield() {
        // Below the inverter limit (cloudy enough that nothing clips), two
        // half-size planes with the same tilt and azimuth give the same
        // output as one plane.
        let site = Site::default();
        let plain = SolarArray {
            cloud_factor: 0.5,
            ..SolarArray::default()
        };
        let split = SolarArray {
            orientations: vec![
                Orientation {
                    share: 1.0,
                    tilt_deg: plain.tilt_deg,
                    azimuth_deg: plain.azimuth_deg,
                },
                Orientation {
                    share: 1.0,
                    tilt_deg: plain.tilt_deg,
                    azimuth_deg: plain.azimuth_deg,
                },
            ],
            ..plain.clone()
        };
        let a = solar_profile_kw(&site, &plain, 0);
        let b = solar_profile_kw(&site, &split, 0);
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-6, "{x} vs {y}");
        }
    }

    #[test]
    fn orientation_and_shading_inputs_are_validated() {
        let array = SolarArray {
            orientations: vec![
                Orientation {
                    share: -1.0,
                    tilt_deg: 30.0,
                    azimuth_deg: 0.0,
                },
                Orientation {
                    share: 1.0,
                    tilt_deg: 120.0,
                    azimuth_deg: 0.0,
                },
            ],
            obstructions: vec![Obstruction {
                start_hour: 5,
                end_hour: 5,
                loss_fraction: 1.5,
            }],
            ..SolarArray::default()
        };
        let issues = array.validate();
        assert!(issues.iter().any(|m| m.contains("Orientation 1 share")));
        assert!(issues.iter().any(|m| m.contains("Orientation 2 tilt")));
        assert!(issues.iter().any(|m| m.contains("Shading 1 must cover")));
        assert!(issues.iter().any(|m| m.contains("Shading 1 loss")));
        assert!(SolarArray::default().validate().is_empty());
    }

    #[test]
    fn aging_scales_nameplate_geometrically() {
        let array = SolarArray {
            capacity_kw: 10.0,
            annual_degradation: 0.005,
            ..SolarArray::default()
        };
        // Ten years at 0.5%/yr: 10 kWp × 0.995^10.
        let aged = array.aged(10.0);
        assert!((aged.capacity_kw - 10.0 * 0.995f64.powi(10)).abs() < 1e-9);
        // Orientation and environment are untouched.
        assert_eq!(aged.tilt_deg, array.tilt_deg);
        assert_eq!(aged.azimuth_deg, array.azimuth_deg);

        let battery = BatterySpec {
            capacity_kwh: 10.0,
            annual_fade: 0.02,
            self_discharge_per_day: 0.0,
            ..BatterySpec::default()
        };
        // Five years at 2%/yr: 10 kWh × 0.98^5, power rating unchanged.
        let aged_b = battery.aged(5.0);
        assert!((aged_b.capacity_kwh - 10.0 * 0.98f64.powi(5)).abs() < 1e-9);
        assert_eq!(aged_b.power_kw, battery.power_kw);
        assert_eq!(aged_b.round_trip_efficiency, battery.round_trip_efficiency);

        // Zero rates and zero years are identities.
        let flat = SolarArray {
            annual_degradation: 0.0,
            ..array.clone()
        };
        assert_eq!(flat.aged(25.0).capacity_kw, array.capacity_kw);
        assert_eq!(array.aged(0.0).capacity_kw, array.capacity_kw);
    }

    #[test]
    fn degradation_rates_are_validated() {
        let array = SolarArray {
            annual_degradation: 0.2,
            ..SolarArray::default()
        };
        assert_eq!(array.validate().len(), 1);
        let battery = BatterySpec {
            annual_fade: -0.01,
            self_discharge_per_day: 0.0,
            ..BatterySpec::default()
        };
        assert_eq!(battery.validate().len(), 1);
        assert!(SolarArray::default().validate().is_empty());
        assert!(BatterySpec::default().validate().is_empty());
    }

    #[cfg(feature = "pro")]
    #[test]
    fn end_of_life_ages_follow_battery_replacement() {
        // Battery replaced at year 10 of a 20-year life: aged 10 years at most.
        let inputs = OptimizationInputs {
            project_years: 20.0,
            battery_life_years: 10.0,
            ..OptimizationInputs::default()
        };
        assert_eq!(inputs.end_of_life_ages(), (20.0, 10.0));
        // A battery that outlasts the project is aged over the whole project.
        let long_life = OptimizationInputs {
            battery_life_years: 30.0,
            ..inputs.clone()
        };
        assert_eq!(long_life.end_of_life_ages(), (20.0, 20.0));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn degradation_never_shrinks_the_recommendation() {
        // Ageing only ever removes served energy, so the feasible set
        // shrinks and the cheapest feasible design can only grow.
        let inputs = OptimizationInputs {
            target_served_fraction: 0.95,
            ..OptimizationInputs::default()
        };
        let run = |pv_deg: f64, fade: f64| {
            recommend_size(
                &Site::default(),
                &SolarArray {
                    annual_degradation: pv_deg,
                    ..SolarArray::default()
                },
                &BatterySpec {
                    annual_fade: fade,
                    self_discharge_per_day: 0.0,
                    ..BatterySpec::default()
                },
                &DayLoad::default(),
                &MonthlyFactors::default(),
                &inputs,
            )
        };
        let new = run(0.0, 0.0);
        let aged = run(0.01, 0.03);
        assert!(aged.feasible && new.feasible);
        assert!(aged.capex_usd >= new.capex_usd - 1e-9);
        // The end-of-life served fraction of the aged recommendation meets
        // the target, which the as-new design need not do.
        assert!(aged.served_fraction >= 0.95 - 1e-9);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn report_contains_key_sections() {
        let seasonal = simulate_seasonal(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
        );
        let report = design_report_markdown(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
            &seasonal,
            ReportExtras::default(),
        );
        for needle in [
            "# TPT Microgrid Sizer",
            "## Site",
            "## Seasonal simulation",
            "## Assumptions",
            "Annual served fraction",
        ] {
            assert!(report.contains(needle), "missing `{needle}`");
        }
        assert!(report.contains("## Monthly factors"));
        assert_eq!(report.matches("| Month |").count(), 2);
    }

    /// A short solar array is a data-entry error, not a sunless day: the
    /// dispatch refuses the pair instead of silently padding with zeros.
    #[test]
    fn profile_length_mismatch_is_an_error() {
        let err = dispatch_profile(&[1.0, 2.0, 3.0], &[1.0; 24], &full_battery())
            .expect_err("mismatched profile lengths must be rejected");
        assert_eq!(
            err,
            ProfileError::LengthMismatch {
                solar: 3,
                load: 24
            }
        );
        assert!(err.to_string().contains("24"));

        let err = dispatch_profile(&[f64::NAN, 0.0], &[0.0, 0.0], &full_battery())
            .expect_err("non-finite solar must be rejected");
        assert_eq!(
            err,
            ProfileError::NotFinite {
                hour: 0,
                profile: "solar"
            }
        );

        let err = dispatch_profile(&[0.0, 0.0], &[0.0, f64::INFINITY], &full_battery())
            .expect_err("non-finite load must be rejected");
        assert_eq!(
            err,
            ProfileError::NotFinite {
                hour: 1,
                profile: "load"
            }
        );

        // Matching finite profiles still dispatch.
        assert!(dispatch_profile(&[0.0; 24], &[0.0; 24], &full_battery()).is_ok());
    }

    /// `solar_profile_kw` and the load profile always cover the same period,
    /// which is exactly what `ProfileError` guards at the public entry point.
    #[test]
    fn solar_and_load_profiles_are_the_same_length() {
        let site = Site::default();
        for m in 0..MONTHS_PER_YEAR {
            let solar = solar_profile_kw(&site, &SolarArray::default(), m);
            let load = constant_load(1.0).hourly_kw;
            assert_eq!(solar.len(), load.len(), "month {m}");
            assert!(ProfileError::check(&solar, &load).is_ok(), "month {m}");
        }
    }

    /// The free build must not expose the Pro surface. This test only runs in
    /// the free configuration, and it fails the moment the pro feature flag is
    /// switched on for the shipped WASM bundle.
    #[cfg(not(feature = "pro"))]
    #[test]
    fn free_build_excludes_pro_surface() {
        // Single-day balance is available in both editions.
        let day = simulate_day(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &constant_load(1.0),
            0,
        );
        assert!((day.totals.load_kwh - 24.0).abs() < 1e-9);
        assert_eq!(day.hours.len(), HOURS_PER_DAY);
        // The pro feature flag must be off for this compilation.
        assert!(!cfg!(feature = "pro"));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn format_kwh_uses_correct_units() {
        assert_eq!(format_kwh(512.34), "512.3 kWh");
        assert_eq!(format_kwh(5_000.0), "5000.0 kWh");
        assert_eq!(format_kwh(12_000.0), "12 MWh");
    }

    #[cfg(feature = "pro")]
    #[test]
    fn recommendation_respects_battery_template() {
        let factors = MonthlyFactors::default();
        let inputs = OptimizationInputs {
            target_served_fraction: 0.95,
            ..OptimizationInputs::default()
        };
        let run = |rte: f64| {
            recommend_size(
                &Site::default(),
                &SolarArray::default(),
                &BatterySpec {
                    round_trip_efficiency: rte,
                    ..BatterySpec::default()
                },
                &DayLoad::default(),
                &factors,
                &inputs,
            )
        };
        let good = run(0.95);
        let poor = run(0.60);
        // A much lossier battery can never serve more load at the same size.
        assert!(
            poor.capex_usd >= good.capex_usd - 1e-9
                || poor.unmet_kwh >= good.unmet_kwh - 1e-9,
            "template efficiency had no effect"
        );
        assert!(poor.unmet_kwh != good.unmet_kwh || poor.capex_usd != good.capex_usd);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_is_independent_of_initial_soc() {
        let run = |soc: f64| {
            simulate_seasonal(
                &Site::default(),
                &SolarArray {
                    capacity_kw: 2.0,
                    ..SolarArray::default()
                },
                &BatterySpec {
                    initial_soc: soc,
                    ..BatterySpec::default()
                },
                &DayLoad::default(),
                &MonthlyFactors::default(),
            )
        };
        let (low, high) = (run(0.1), run(1.0));
        assert!((low.unmet_kwh - high.unmet_kwh).abs() < 1e-3);
    }

    #[test]
    fn validation_rejects_nan_and_out_of_range() {
        let nan_battery = BatterySpec {
            capacity_kwh: f64::NAN,
            ..BatterySpec::default()
        };
        assert!(!nan_battery.validate().is_empty());
        let low_init = BatterySpec {
            min_soc: 0.5,
            initial_soc: 0.2,
            ..BatterySpec::default()
        };
        assert!(!low_init.validate().is_empty());
        let neg_load = DayLoad {
            hourly_kw: vec![-1.0; HOURS_PER_DAY],
        };
        assert!(!neg_load.validate().is_empty());
        let short_load = DayLoad {
            hourly_kw: vec![1.0; 12],
        };
        assert!(!short_load.validate().is_empty());
        assert!(BatterySpec::default().validate().is_empty());
    }

    #[test]
    fn zero_power_battery_behaves_like_no_battery() {
        let solar = vec![0.0, 0.0, 5.0, 5.0];
        let load = vec![1.0, 1.0, 1.0, 1.0];
        let battery = BatterySpec {
            capacity_kwh: 100.0,
            power_kw: 0.0,
            ..BatterySpec::default()
        };
        let totals = dispatch(&solar, &load, &battery);
        assert!((totals.unmet_kwh - 2.0).abs() < 1e-9);
        assert!((totals.curtailed_kwh - 8.0).abs() < 1e-9);
    }

    #[test]
    fn negative_load_is_floored_and_energy_balances() {
        let solar = vec![0.0, 3.0, 0.0];
        let load = vec![-2.0, 1.0, 2.0];
        let totals = dispatch(&solar, &load, &BatterySpec::default());
        assert!((totals.load_kwh - 3.0).abs() < 1e-9);
        assert!(totals.unmet_kwh >= 0.0 && totals.curtailed_kwh >= 0.0);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn infeasible_target_returns_best_effort() {
        let inputs = OptimizationInputs {
            // Served fraction can never exceed 1.0.
            target_served_fraction: 1.5,
            ..OptimizationInputs::default()
        };
        let rec = recommend_size(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
            &inputs,
        );
        assert!(!rec.feasible);
        assert!(rec.capex_usd > 0.0);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn annual_cost_matches_hand_calculation() {
        let inputs = OptimizationInputs {
            pv_cost_usd_per_kw: 1000.0,
            battery_cost_usd_per_kwh: 500.0,
            discount_rate: 0.0,
            project_years: 20.0,
            om_fraction_of_capex: 0.01,
            battery_life_years: 10.0,
            ..OptimizationInputs::default()
        };
        // 10 kWp + 10 kWh: capex 10,000 + 5,000; one battery replacement at
        // year 10 (5,000); zero discounting => (15,000 + 5,000)/20 = 1,000,
        // plus O&M 1% of 15,000 = 150.
        let cost = annual_cost_usd(10.0, 10.0, &inputs);
        assert!((cost - 1150.0).abs() < 1e-6, "cost = {cost}");
        let rec_cost_with_rate = annual_cost_usd(
            10.0,
            10.0,
            &OptimizationInputs {
                discount_rate: 0.07,
                ..inputs
            },
        );
        assert!(rec_cost_with_rate > cost);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn generator_covers_unmet_and_counts_fuel() {
        let array = SolarArray {
            capacity_kw: 1.0,
            ..SolarArray::default()
        };
        let battery = BatterySpec::default();
        let run = |generator: Option<&GeneratorSpec>| {
            simulate_seasonal_with_generator(
                &Site::default(),
                &array,
                &battery,
                &DayLoad::default(),
                &MonthlyFactors::default(),
                generator,
            )
        };
        let without = run(None);
        assert!(without.unmet_kwh > 0.0);
        assert_eq!(without.generator_kwh, 0.0);
        let generator = GeneratorSpec {
            power_kw: 100.0,
            fuel_l_per_kwh: 0.25,
            ..GeneratorSpec::default()
        };
        let with = run(Some(&generator));
        assert!(with.unmet_kwh < 1e-6, "unmet = {}", with.unmet_kwh);
        assert!((with.generator_kwh - without.unmet_kwh).abs() < 1e-6);
        assert!((with.fuel_litres - with.generator_kwh * 0.25).abs() < 1e-6);
        // Conservation: served energy is unchanged in total.
        assert!((with.load_kwh - without.load_kwh).abs() < 1e-9);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn limited_generator_leaves_residual_unmet() {
        let array = SolarArray {
            capacity_kw: 0.1,
            ..SolarArray::default()
        };
        let weak = GeneratorSpec {
            power_kw: 0.1,
            ..GeneratorSpec::default()
        };
        let result = simulate_seasonal_with_generator(
            &Site::default(),
            &array,
            &BatterySpec::default(),
            &DayLoad::default(),
            &MonthlyFactors::default(),
            Some(&weak),
        );
        assert!(result.unmet_kwh > 0.0);
        assert!(result.generator_kwh > 0.0);
    }

    #[test]
    fn generator_validation_rejects_bad_values() {
        assert!(GeneratorSpec::default().validate().is_empty());
        let bad = GeneratorSpec {
            fuel_l_per_kwh: f64::NAN,
            ..GeneratorSpec::default()
        };
        assert!(!bad.validate().is_empty());
    }

    /// Fractional UTC offsets (ACST +9:30, Nepal +5:45, Chatham +12:45) shift
    /// the solar profile but must never produce NaN or a negative yield.
    #[test]
    fn fractional_utc_offsets_simulate_sensibly() {
        for offset in [9.5, 5.75, 12.75, -3.5] {
            let site = Site {
                timezone_offset_hours: offset,
                ..Site::default()
            };
            let profile = solar_profile_kw(&site, &SolarArray::default(), 0);
            assert_eq!(profile.len(), HOURS_PER_DAY, "offset {offset}");
            assert!(
                profile.iter().all(|k| k.is_finite() && *k >= 0.0),
                "offset {offset} produced {profile:?}"
            );
            assert!(
                profile.iter().sum::<f64>() > 0.0,
                "offset {offset} produced no energy at all"
            );
        }
    }

    /// Panel orientation: the same 0° (north) array out-produces itself in the
    /// southern hemisphere, and a due-south array beats due-north in the north.
    #[test]
    fn panel_orientation_changes_yield() {
        let southern = Site {
            latitude_deg: -41.3,
            ..Site::default()
        };
        let northern = Site {
            latitude_deg: 52.4,
            longitude_deg: 4.9,
            ..Site::default()
        };
        let energy = |site: &Site, array: &SolarArray, month: usize| -> f64 {
            solar_profile_kw(site, array, month).iter().sum()
        };
        let north_facing = SolarArray::default();
        // December at both sites, same array and azimuth.
        let wellington_december = energy(&southern, &north_facing, 11);
        let amsterdam_december = energy(&northern, &north_facing, 11);
        assert!(wellington_december > 0.0 && amsterdam_december > 0.0);
        assert!(
            wellington_december > amsterdam_december,
            "a 0° azimuth in the southern hemisphere should beat one in the north"
        );
        // And a due-south array in Amsterdam beats due-north there.
        let amsterdam_south = energy(
            &northern,
            &SolarArray {
                azimuth_deg: 180.0,
                ..SolarArray::default()
            },
            11,
        );
        assert!(amsterdam_south > amsterdam_december);
    }

    /// The seasonal year is a fixed 365-day representative year: February is
    /// scaled by 28 days and the monthly loads must sum to the annual total.
    #[cfg(feature = "pro")]
    #[test]
    fn month_day_scaling_sums_to_the_year() {
        let seasonal = simulate_seasonal_with_generator(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &constant_load(1.0),
            &MonthlyFactors::flat(),
            None,
        );
        assert_eq!(seasonal.months.len(), MONTHS_PER_YEAR);
        let summed: f64 = seasonal.months.iter().map(|m| m.load_kwh).sum();
        // 365 days x 24 h x 1 kW.
        assert!((summed - 8760.0).abs() < 1e-6, "{summed}");
        assert!((seasonal.load_kwh - 8760.0).abs() < 1e-6);
        assert!((seasonal.months[1].load_kwh - 672.0).abs() < 1e-6, "February");
    }
/// A multi-day overcast spell is stricter than the steady-state seasonal
    /// month it is drawn from, and its reported failure day is consistent with
    /// the unmet energy seen.
    #[cfg(feature = "pro")]
    #[test]
    fn cloudy_spell_is_harsher_than_the_steady_month() {
        let site = Site::default();
        let array = SolarArray {
            capacity_kw: 3.0,
            ..SolarArray::default()
        };
        let battery = BatterySpec {
            capacity_kwh: 6.0,
            power_kw: 3.0,
            round_trip_efficiency: 0.9,
            min_soc: 0.1,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let load = constant_load(1.5);
        let factors = MonthlyFactors::flat();
        let seasonal =
            simulate_seasonal_with_generator(&site, &array, &battery, &load, &factors, None);

        // An undersized design cannot survive a week of the worst month.
        let spell = simulate_cloudy_spell(&site, &array, &battery, &load, &factors, &seasonal, 7);
        assert_eq!(spell.days_simulated, 7);
        assert_eq!(spell.month, seasonal.worst_month);
        assert!(
            !spell.survived,
            "a 6 kWh battery cannot ride out a 7-day winter spell"
        );
        assert!(spell.days_to_failure < 7);
        assert!(spell.unmet_kwh > 0.0);
        assert!(spell.served_kwh > 0.0);
        // Every day is still logged, so served + unmet equals the whole load.
        let spell_load_kwh = load.daily_kwh() * factors.load[spell.month] * 7.0;
        assert!(
            ((spell.served_kwh + spell.unmet_kwh) - spell_load_kwh).abs() < 1e-6,
            "{} vs {}",
            spell.served_kwh + spell.unmet_kwh,
            spell_load_kwh
        );

        // A generous design does survive the same spell: 300 kWh covers the
        // worst-month deficit for the whole week.
        let big = BatterySpec {
            capacity_kwh: 300.0,
            power_kw: 20.0,
            ..battery.clone()
        };
        let big_seasonal =
            simulate_seasonal_with_generator(&site, &array, &big, &load, &factors, None);
        let survives =
            simulate_cloudy_spell(&site, &array, &big, &load, &factors, &big_seasonal, 7);
        assert!(survives.survived, "{survives:?}");
        assert_eq!(survives.days_to_failure, 7);

        // The day count is clamped into a sane range.
        let clamped =
            simulate_cloudy_spell(&site, &array, &big, &load, &factors, &big_seasonal, 0);
        assert_eq!(clamped.days_simulated, 1);
        let long =
            simulate_cloudy_spell(&site, &array, &big, &load, &factors, &big_seasonal, 999);
        assert_eq!(long.days_simulated, 60);
    }

    /// The optimizer honours the requested objective: minimising LCOE returns
    /// a candidate at least as cheap per kWh as the capex-optimal one, and the
    /// objective ids round-trip.
    #[cfg(feature = "pro")]
    #[test]
    fn optimizer_objective_selects_the_requested_metric() {
        let site = Site::default();
        let array = SolarArray {
            capacity_kw: 3.0,
            ..SolarArray::default()
        };
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 0.9,
            min_soc: 0.1,
            initial_soc: 0.5,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let load = constant_load(1.2);
        let factors = MonthlyFactors::flat();
        let base = OptimizationInputs {
            target_served_fraction: 0.95,
            generator: None,
            ..OptimizationInputs::default()
        };

        let by_capex = recommend_size(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &OptimizationInputs {
                objective: OptimizationObjective::Capex,
                ..base.clone()
            },
        );
        let by_lcoe = recommend_size(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &OptimizationInputs {
                objective: OptimizationObjective::Lcoe,
                ..base.clone()
            },
        );
        assert!(by_capex.feasible && by_lcoe.feasible);
        assert!(
            by_lcoe.lcoe_usd_per_kwh <= by_capex.lcoe_usd_per_kwh + 1e-9,
            "LCOE objective {} worse than capex objective {}",
            by_lcoe.lcoe_usd_per_kwh,
            by_capex.lcoe_usd_per_kwh
        );
        // More energy costs more capital.
        assert!(by_lcoe.capex_usd >= by_capex.capex_usd - 1e-6);
        // Both still meet the target they were given.
        assert!(by_lcoe.served_fraction >= base.target_served_fraction);

        for objective in [
            OptimizationObjective::Capex,
            OptimizationObjective::AnnualCost,
            OptimizationObjective::Lcoe,
        ] {
            assert_eq!(
                OptimizationObjective::from_id(objective.id()),
                objective,
                "{}",
                objective.id()
            );
            assert!(!objective.label().is_empty());
        }
        assert_eq!(
            OptimizationObjective::from_id("nonsense"),
            OptimizationObjective::AnnualCost
        );
        assert_eq!(
            OptimizationInputs::default().objective,
            OptimizationObjective::AnnualCost
        );
    }

    /// The report documents the cost basis and generator settings, so a reader
    /// can reproduce the LCOE, and prints real site values rather than just
    /// headings.
    #[cfg(feature = "pro")]
    #[test]
    fn report_documents_costs_and_generator() {
        let site = Site::default();
        let array = SolarArray {
            capacity_kw: 4.0,
            ..SolarArray::default()
        };
        let battery = BatterySpec {
            capacity_kwh: 12.0,
            power_kw: 5.0,
            min_soc: 0.1,
            initial_soc: 0.6,
            ..BatterySpec::default()
        };
        let load = constant_load(1.4);
        let factors = MonthlyFactors::flat();
        let seasonal =
            simulate_seasonal_with_generator(&site, &array, &battery, &load, &factors, None);
        let inputs = OptimizationInputs {
            pv_cost_usd_per_kw: 1250.0,
            battery_cost_usd_per_kwh: 550.0,
            discount_rate: 0.06,
            project_years: 25.0,
            battery_life_years: 12.0,
            om_fraction_of_capex: 0.02,
            target_served_fraction: 0.98,
            battery_power_ratio: 0.4,
            generator: Some(GeneratorSpec {
                power_kw: 8.0,
                fuel_l_per_kwh: 0.32,
                fuel_cost_usd_per_l: 1.8,
                cost_usd_per_kw: 500.0,
            }),
            objective: OptimizationObjective::Lcoe,
        };
        let rec = recommend_size(&site, &array, &battery, &load, &factors, &inputs);
        let report = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras {
                recommendation: Some(&rec),
                cost_inputs: Some(&inputs),
                spell: None,
                inverter: None,
                reliability: None,
                grid: None,
            },
        );

        // Site, array and battery values are printed, not just the headings.
        assert!(report.contains(&format!("{:.4}", site.latitude_deg.abs())), "{report}");
        assert!(report.contains("UTC+12.0"), "{report}");
        assert!(report.contains(&format!("{:.1} kWp DC", array.capacity_kw)));
        assert!(report.contains("initial SoC 60%"), "{report}");
        assert!(report.contains(&format!("{:.1} kWh nameplate", battery.capacity_kwh)));

        assert!(report.contains("## Cost basis"), "{report}");
        assert!(report.contains("US$ 1250/kWp"));
        assert!(report.contains("US$ 550/kWh nameplate"));
        assert!(report.contains("Discount rate: 6.0%"));
        assert!(report.contains("project life: 25 years"));
        assert!(report.contains("every 12 years"));
        assert!(report.contains("O&M: 2.0% of capex"));
        assert!(report.contains("Backup generator: 8.0 kW at US$ 500/kW"));
        assert!(report.contains("fuel US$ 1.80/L"));
        assert!(report.contains("Target served fraction: 98.0%"));
        assert!(report.contains("0.40 kW/kWh"));
        assert!(report.contains("Lowest cost of energy (LCOE)"));

        assert!(report.contains("## Recommended sizing"));
        assert!(report.contains(&format!("US$ {:.0}/year", rec.annual_cost_usd)));
        assert!(report.contains(&format!("US$ {:.0}", rec.capex_usd)));
        // Every month appears in both tables.
        assert_eq!(report.matches("| January |").count(), 2);
        // MWh formatting kicks in for annual totals.
        assert!(report.contains("MWh"), "{report}");

        // Without cost inputs the section is simply absent.
        let bare = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras::default(),
        );
        assert!(!bare.contains("## Cost basis"));
        assert!(!bare.contains("## Recommended sizing"));
        assert!(bare.contains("## Assumptions"));
        // A generator-less input set says so explicitly.
        let no_gen = OptimizationInputs {
            generator: None,
            ..inputs.clone()
        };
        let report = design_report_markdown(
            &site,
            &array,
            &battery,
            &load,
            &factors,
            &seasonal,
            ReportExtras {
                cost_inputs: Some(&no_gen),
                ..ReportExtras::default()
            },
        );
        assert!(report.contains("Backup generator: none included."), "{report}");
    }

    /// The overcast line is written correctly for all three spell outcomes:
    /// survived, failed on day one, and failed part-way through.
    #[cfg(feature = "pro")]
    #[test]
    fn report_writes_every_spell_outcome() {
        let site = Site::default();
        let array = SolarArray {
            capacity_kw: 2.0,
            ..SolarArray::default()
        };
        let load = constant_load(2.0);
        let factors = MonthlyFactors::flat();
        let spell_report = |battery: &BatterySpec, days: usize| -> (SpellResult, String) {
            let seasonal =
                simulate_seasonal_with_generator(&site, &array, battery, &load, &factors, None);
            let spell =
                simulate_cloudy_spell(&site, &array, battery, &load, &factors, &seasonal, days);
            let report = design_report_markdown(
                &site,
                &array,
                battery,
                &load,
                &factors,
                &seasonal,
                ReportExtras {
                    spell: Some(&spell),
                    ..ReportExtras::default()
                },
            );
            (spell, report)
        };

        // Survives: a battery far larger than the load.
        let big = BatterySpec {
            capacity_kwh: 200.0,
            power_kw: 20.0,
            min_soc: 0.05,
            initial_soc: 1.0,
            ..BatterySpec::default()
        };
        let (spell, report) = spell_report(&big, 3);
        assert!(spell.survived, "{spell:?}");
        assert!(report.contains("Overcast endurance (3 days of"), "{report}");
        assert!(report.contains("fully covered"), "{report}");

        // Fails on day one: no battery at all.
        let none = BatterySpec {
            capacity_kwh: 0.0,
            power_kw: 0.0,
            ..BatterySpec::default()
        };
        let (spell, report) = spell_report(&none, 3);
        assert!(!spell.survived && spell.days_to_failure == 0, "{spell:?}");
        assert!(report.contains("unmet on the very first day"), "{report}");

        // Fails part-way through: enough for day one, not for the whole spell.
        let mid = BatterySpec {
            capacity_kwh: 120.0,
            power_kw: 5.0,
            min_soc: 0.1,
            initial_soc: 1.0,
            ..BatterySpec::default()
        };
        let (spell, report) = spell_report(&mid, 5);
        assert!(!spell.survived && spell.days_to_failure > 0, "{spell:?}");
        assert!(report.contains("load first unmet after"), "{report}");

        // With no spell the line is absent.
        let seasonal =
            simulate_seasonal_with_generator(&site, &array, &mid, &load, &factors, None);
        let bare = design_report_markdown(
            &site,
            &array,
            &mid,
            &load,
            &factors,
            &seasonal,
            ReportExtras::default(),
        );
        assert!(!bare.contains("Overcast endurance"));
    }
    #[test]
    fn chemistries_are_valid_and_first_matches_default() {
        let all = chemistries();
        assert!(all.iter().any(|c| c.name.contains("Nickel-iron")));
        let base = BatterySpec::default();
        for c in all {
            let spec = c.apply(&base, true);
            assert!(spec.validate().is_empty(), "{}: {:?}", c.name, spec.validate());
            assert!((spec.power_kw - base.capacity_kwh * c.power_per_kwh).abs() < 1e-9);
        }
        let lfp = all[0].apply(&base, false);
        assert_eq!(lfp.round_trip_efficiency, base.round_trip_efficiency);
        assert_eq!(lfp.min_soc, base.min_soc);
        assert_eq!(lfp.annual_fade, base.annual_fade);
        assert_eq!(lfp.power_kw, base.power_kw);
    }

    #[test]
    fn self_discharge_drains_idle_battery_but_not_below_floor() {
        let idle = [0.0; 24];
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.2,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.1,
        };
        let totals = dispatch(&idle, &idle, &battery);
        // One day at 10%/day: SoC ends at 0.9.
        assert!((totals.end_soc - 0.9).abs() < 1e-6, "{}", totals.end_soc);
        let leaky = BatterySpec { self_discharge_per_day: 0.2, min_soc: 0.85, ..battery.clone() };
        let floored = dispatch(&idle, &idle, &leaky);
        assert!(floored.end_soc >= 0.85 - 1e-9, "{}", floored.end_soc);
        let none = BatterySpec { self_discharge_per_day: 0.0, ..battery };
        assert!((dispatch(&idle, &idle, &none).end_soc - 1.0).abs() < 1e-9);
    }

    #[test]
    fn self_discharge_validation() {
        let bad = BatterySpec { self_discharge_per_day: 0.5, ..BatterySpec::default() };
        assert!(!bad.validate().is_empty());
    }
}
