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

/// Hours per simulated day.
pub const HOURS_PER_DAY: usize = 24;
/// Months per simulated year.
pub const MONTHS_PER_YEAR: usize = 12;
/// Representative calendar day simulated for every month.
const REPRESENTATIVE_DAY: u32 = 21;
/// Day counts per month for `REPRESENTATIVE_YEAR` (non-leap) — the seasonal
/// simulation scales each representative day by its month's length.
#[cfg(feature = "pro")]
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
}

impl Default for SolarArray {
    fn default() -> Self {
        Self {
            capacity_kw: 5.0,
            tilt_deg: 30.0,
            azimuth_deg: 0.0,
            cloud_factor: 1.0,
            ambient_celsius: 15.0,
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
        issues
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
}

impl Default for BatterySpec {
    fn default() -> Self {
        Self {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 0.95,
            min_soc: 0.1,
            initial_soc: 0.5,
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
        issues
    }

    fn clamped(&self) -> Self {
        Self {
            capacity_kwh: self.capacity_kwh.max(0.0),
            power_kw: self.power_kw.max(0.0),
            round_trip_efficiency: self.round_trip_efficiency.clamp(0.5, 1.0),
            min_soc: self.min_soc.clamp(0.0, 0.95),
            initial_soc: self.initial_soc.clamp(0.0, 1.0),
        }
    }
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

/// Runs the greedy hourly dispatch over arbitrary solar/load profiles and
/// returns only the totals — the hand-checkable core primitive.
///
/// Dispatch priority per hour: solar serves load directly; surplus charges
/// the battery (then curtails); deficits discharge the battery (then go
/// unmet). No grid connection.
pub fn dispatch_profile(
    solar_kw: &[f64],
    load_kw: &[f64],
    battery: &BatterySpec,
) -> DispatchTotals {
    dispatch_hours(solar_kw, load_kw, battery).1
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

/// Hourly PV AC output (kW) for the representative day of `month`
/// (0 = January) under `array`'s cloud/ambient conditions.
pub fn solar_profile_kw(site: &Site, array: &SolarArray, month: usize) -> Vec<f64> {
    let solar_model = SolarModel::new(
        site.latitude_deg,
        site.longitude_deg,
        site.altitude_m,
        site.timezone_offset_hours,
    );
    let mut config = PvPlantConfig::new(
        array.capacity_kw / 1000.0,
        array.tilt_deg,
        array.azimuth_deg,
        array.ambient_celsius,
    );
    config.soiling_loss = SOILING_LOSS;
    let plant = PvPlant::new(solar_model.clone(), config);
    let cloud = array.cloud_factor.clamp(0.0, 1.0);
    let base_utc = month_base_utc(site, month);

    (0..HOURS_PER_DAY)
        .map(|h| {
            // Hour `h` covers [h, h+1): sample the sun at the midpoint.
            let pos = solar_model.solar_position(base_utc + Duration::minutes(h as i64 * 60 + 30));
            let clear = plant.output_clearsky(&pos);
            // Clear-sky POA (undo soiling), scale by cloud, re-run the
            // public output path (re-applies soiling + thermal + clipping).
            let poa_clear = clear.poa_w_per_m2 / (1.0 - SOILING_LOSS);
            plant.output_from_poa(poa_clear * cloud).ac_power_mw * 1000.0
        })
        .collect()
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
        }
    }
}

impl OptimizationInputs {
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
    const PV_MULTIPLIERS: [f64; 9] = [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];
    const BATTERY_MULTIPLIERS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0];

    let mut best_feasible: Option<SizingRecommendation> = None;
    let mut best_effort: Option<SizingRecommendation> = None;

    for &pv_mult in &PV_MULTIPLIERS {
        let solar_kw = (peak_kw * pv_mult).max(0.05);
        for &batt_mult in &BATTERY_MULTIPLIERS {
            let battery_kwh = (daily_kwh * batt_mult).max(0.1);
            let battery = BatterySpec {
                capacity_kwh: battery_kwh,
                power_kw: battery_kwh * inputs.battery_power_ratio,
                ..battery_template.clone()
            };
            // The optimizer sweeps capacity; the array's orientation comes
            // from the caller's design.
            let array_c = SolarArray {
                capacity_kw: solar_kw,
                ..array.clone()
            };
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
                Some(best) => {
                    candidate.capex_usd < best.capex_usd - 1e-9
                        || ((candidate.capex_usd - best.capex_usd).abs() <= 1e-9
                            && candidate.unmet_kwh < best.unmet_kwh)
                }
                None => candidate.feasible,
            };
            if candidate.feasible && better_feasible {
                best_feasible = Some(candidate.clone());
            }
            let better_effort = match &best_effort {
                Some(best) => {
                    candidate.unmet_kwh < best.unmet_kwh - 1e-9
                        || ((candidate.unmet_kwh - best.unmet_kwh).abs() <= 1e-9
                            && candidate.capex_usd < best.capex_usd)
                }
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

/// Builds the exportable system-design report (Markdown) for a simulated
/// design. `recommendation` is included when the optimizer has run.
#[cfg(feature = "pro")]
pub fn design_report_markdown(
    site: &Site,
    array: &SolarArray,
    battery: &BatterySpec,
    base_load: &DayLoad,
    factors: &MonthlyFactors,
    seasonal: &SeasonalResult,
    recommendation: Option<&SizingRecommendation>,
) -> String {
    use std::fmt::Write as _;

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
            "- Achieved served fraction: {:.1}% ({} unmet/year)",
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
        }
    }

    fn constant_load(kw: f64) -> DayLoad {
        DayLoad {
            hourly_kw: vec![kw; HOURS_PER_DAY],
        }
    }

    /// Hand-checkable balance, generation side zeroed: a constant 2 kW load
    /// against a 10 kWh battery (RTE 1.0, full) must empty it in exactly
    /// five hours and then go unmet — 10 kWh served, 38 kWh unmet.
    #[test]
    fn battery_only_dispatch_hand_check() {
        let totals = dispatch_profile(&[0.0; 24], &vec![2.0; 24], &full_battery());
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
        };
        let mut solar = [0.0; 24];
        solar[0] = 2.0;
        solar[1] = 2.0;
        solar[2] = 2.0;
        let totals = dispatch_profile(&solar, &[0.0; 24], &battery);
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
        };
        let mut solar = [0.0; 24];
        solar[0] = 2.0;
        solar[1] = 2.0;
        solar[2] = 2.0;
        let mut load = vec![0.0; 24];
        for kw in load.iter_mut().skip(3) {
            *kw = 3.0;
        }
        let totals = dispatch_profile(&solar, &load, &battery);
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
        };
        let totals = dispatch_profile(&[4.0; 24], &[0.0; 24], &battery);
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
        };
        let totals = dispatch_profile(&[0.0; 24], &vec![50.0; 24], &battery);
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
        let totals = dispatch_profile(&solar, &load, &battery);
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
        // battery, reproduces its claimed service.
        let array = SolarArray {
            capacity_kw: rec.solar_kw,
            ..SolarArray::default()
        };
        let battery = BatterySpec {
            capacity_kwh: rec.battery_kwh,
            power_kw: rec.battery_kw,
            ..BatterySpec::default()
        };
        let check = simulate_seasonal(&Site::default(), &array, &battery, &DayLoad::default(), &factors);
        assert!((check.served_fraction() - rec.served_fraction).abs() < 1e-6);
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
            None,
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
        let totals = dispatch_profile(&solar, &load, &battery);
        assert!((totals.unmet_kwh - 2.0).abs() < 1e-9);
        assert!((totals.curtailed_kwh - 8.0).abs() < 1e-9);
    }

    #[test]
    fn negative_load_is_floored_and_energy_balances() {
        let solar = vec![0.0, 3.0, 0.0];
        let load = vec![-2.0, 1.0, 2.0];
        let totals = dispatch_profile(&solar, &load, &BatterySpec::default());
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
}
