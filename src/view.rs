//! The view layer: message type, outcome enums, the mounted-once shell, the
//! reactive result sections, and the chart data builders.
//!
//! Everything here is pure `UITree` construction over engine types — no DOM,
//! no wasm — so the whole UI structure compiles and unit-tests on the host
//! (`cargo test`) as well as in the wasm build. DOM glue lives in `app`.

use std::rc::Rc;

use tpt_appfront_core::UITree;
use tpt_microgrid_engine::{
    chemistries, presets, site_presets, DayResult, HOURS_PER_DAY, MONTH_NAMES,
};
#[cfg(feature = "pro")]
use tpt_microgrid_engine::MONTHS_PER_YEAR;
#[cfg(feature = "pro")]
use tpt_microgrid_engine::{
    GridYear, InverterReport, MonthlyFactors, ReliabilityReport, SeasonalResult,
    SizingRecommendation, SpellResult, MAX_AUTONOMY_DAYS,
};

use crate::chart;

/// The single-day results section's current content.
#[derive(Clone)]
pub(crate) enum DayOutcome {
    Idle,
    Invalid(Vec<String>),
    Solved(Rc<DayResult>),
}

/// The seasonal results section's current content (Pro).
#[cfg(feature = "pro")]
#[derive(Clone)]
pub(crate) enum SeasonalOutcome {
    Idle,
    Invalid(Vec<String>),
    /// The annual run, plus the multi-day overcast endurance check when one
    /// was run (the spell input is optional tooling).
    Solved(Rc<SeasonalResult>, Option<Rc<SpellResult>>),
}

/// The recommendation section's current content (Pro).
#[cfg(feature = "pro")]
#[derive(Clone)]
pub(crate) enum RecOutcome {
    Idle,
    Invalid(Vec<String>),
    Solved(Rc<SizingRecommendation>),
}

#[derive(Debug, Clone)]
pub(crate) enum Msg {
    SimulateDay,
    /// Fill the 24 hourly load inputs from a built-in preset.
    Preset(usize),
    /// Open a TPT Solutions URL — new tab in the browser, or via the
    /// desktop host's system-browser IPC inside the Pro shell. See
    /// [`visit_site`].
    VisitSite(String),
    /// (pro) run the 12-month simulation.
    #[cfg(feature = "pro")]
    RunSeasonal,
    /// Fill the battery fields from a chemistry (0 = custom, no change).
    Chemistry(usize),
    /// Parse the pasted load CSV into the hourly load inputs.
    ImportLoad,
    /// (pro) Calibrate the monthly cloud factors to the pasted weather CSV.
    #[cfg(feature = "pro")]
    ImportWeather,
    /// (pro) Check the inverter rating against the peak and estimate clipping.
    #[cfg(feature = "pro")]
    CheckInverter,
    /// (pro) Compute loss-of-load and battery-only autonomy.
    #[cfg(feature = "pro")]
    CheckReliability,
    /// (pro) Price the year with the grid connected.
    #[cfg(feature = "pro")]
    CheckGrid,
    /// Build the hourly load from the appliance list.
    BuildLoad,
    /// Fill the site inputs from a built-in place (0 = custom, no change).
    SitePreset(usize),
    /// Fill the site inputs from the device's location (browser permission).
    UseLocation,
    /// Choose the colour theme: 0 = follow the system, 1 = light, 2 = dark.
    Theme(usize),
    /// Download the hourly balance of the single-day run as CSV.
    ExportDayCsv,
    /// (pro) Download the monthly totals of the seasonal run as CSV.
    #[cfg(feature = "pro")]
    ExportSeasonalCsv,
    /// (pro) Download the system-design report as PDF.
    #[cfg(feature = "pro")]
    ExportReportPdf,
    /// Save the current inputs (and the latest run metrics) as a scenario.
    SaveScenario,
    /// Restore a saved scenario's inputs.
    LoadScenario(usize),
    /// Remove a saved scenario.
    DeleteScenario(usize),
    /// (pro) run the battery/solar sizing optimization.
    #[cfg(feature = "pro")]
    RunOptimize,
    /// (pro) copy the recommendation's PV/battery sizes into the inputs.
    #[cfg(feature = "pro")]
    ApplyRecommendation,
    /// (pro) download the system-design report (Markdown).
    #[cfg(feature = "pro")]
    ExportReport,
}

/// Default field values — a Wellington cabin with a 5 kWp north-facing
/// array and a 10 kWh battery, so the first Simulate works with no edits.
mod defaults {
    pub const LAT: &str = "-41.2866";
    pub const LON: &str = "174.7756";
    pub const TZ: &str = "12";
    pub const ALT: &str = "30";
    pub const MONTH: &str = "11";
    pub const PV_CAP: &str = "5";
    pub const PV_TILT: &str = "30";
    pub const PV_AZ: &str = "0";
    pub const PV_DEG: &str = "0.5";
    pub const PV_DCAC: &str = "1.2";
    /// Appliance rows: (power kW, start hour, hours per day). Only the first
    /// two are used by default; the rest are empty slots.
    pub const APPLIANCES: [(&str, &str, &str); 6] = [
        ("0.15", "0", "24"),
        ("0.12", "18", "5"),
        ("0", "0", "0"),
        ("0", "0", "0"),
        ("0", "0", "0"),
        ("0", "0", "0"),
    ];
    #[cfg(feature = "pro")]
    pub const GRID_IMPORT: &str = "0.30";
    #[cfg(feature = "pro")]
    pub const GRID_EXPORT: &str = "0.08";
    #[cfg(feature = "pro")]
    pub const GRID_DAILY: &str = "0.80";
    #[cfg(feature = "pro")]
    pub const GRID_IMPORT_LIMIT: &str = "15";
    #[cfg(feature = "pro")]
    pub const GRID_EXPORT_LIMIT: &str = "5";
    pub const BAT_KWH: &str = "10";
    pub const BAT_KW: &str = "5";
    pub const BAT_RTE: &str = "95";
    pub const BAT_MIN: &str = "10";
    pub const BAT_INIT: &str = "50";
    pub const BAT_FADE: &str = "2";
    pub const BAT_SD: &str = "0";
    #[cfg(feature = "pro")]
    pub const OPT_PV_COST: &str = "1400";
    #[cfg(feature = "pro")]
    pub const OPT_BAT_COST: &str = "600";
    #[cfg(feature = "pro")]
    pub const OPT_TARGET: &str = "0.99";
    #[cfg(feature = "pro")]
    pub const OPT_RATIO: &str = "0.5";
    #[cfg(feature = "pro")]
    pub const OPT_DISCOUNT: &str = "7";
    #[cfg(feature = "pro")]
    pub const OPT_YEARS: &str = "20";
    #[cfg(feature = "pro")]
    pub const OPT_OM: &str = "1.5";
    #[cfg(feature = "pro")]
    pub const OPT_BAT_LIFE: &str = "10";
    #[cfg(feature = "pro")]
    pub const GEN_KW: &str = "0";
    #[cfg(feature = "pro")]
    pub const GEN_FUEL_COST: &str = "1.5";
    /// Sizing objective select: capex / levelised annual cost / LCOE.
    #[cfg(feature = "pro")]
    pub const OPT_OBJECTIVE: &str = "annual-cost";
    /// Length of the multi-day overcast ("cloudy spell") endurance check.
    #[cfg(feature = "pro")]
    pub const SPELL_DAYS: &str = "5";
}

/// The mounted-once shell: header, setup cards, chart card, results
/// placeholders, upsell.
pub(crate) fn shell_tree() -> UITree<Msg> {
    UITree::container(|c| {
        c.container(|header| {
            header.text("MS").class("mg-brand");
            header.container(|group| {
                group.container(|row| {
                    row.heading(2, "Microgrid Sizer");
                    #[cfg(feature = "pro")]
                    row.text("Pro").class("mg-chip");
                })
                .class("mg-title-row");
                group.text("Off-grid solar + battery sizing for cabins and small commercial sites — hourly load/generation balance, solved locally in your browser. Units: kW, kWh.")
                    .class("mg-sub");
                group.container(|theme| {
                    label(theme, "Theme");
                    theme
                        .select(
                            vec![
                                ("0".to_string(), "Match system".to_string()),
                                ("1".to_string(), "Light".to_string()),
                                ("2".to_string(), "Dark".to_string()),
                            ],
                            "0",
                        )
                        .class("mg-input mg-theme")
                        .on_input(|value| Msg::Theme(value.parse::<usize>().unwrap_or(0)));
                })
                .class("mg-theme-wrap");
            })
            .class("mg-title-group");
        })
        .class("mg-header");

        c.container(|shell| {
            shell.container(|sidebar| {
                sidebar.container(|card| {
                    card.heading(3, "Site & day");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Start from a place");
                            field
                                .select(
                                    std::iter::once("Custom (type below)")
                                        .chain(site_presets().iter().map(|p| p.name))
                                        .enumerate()
                                        .map(|(i, name)| (i.to_string(), name.to_string()))
                                        .collect::<Vec<_>>(),
                                    "0",
                                )
                                .class("mg-input mg-site-preset")
                                .on_input(|value| {
                                    Msg::SitePreset(value.parse::<usize>().unwrap_or(0))
                                });
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|actions| {
                        actions
                            .button("Use my location")
                            .class("mg-btn")
                            .on_click(Msg::UseLocation);
                    })
                    .class("mg-actions");
                    card.container(|_| {}).class("mg-site-status");
                    card.text("Finding your coordinates: open any map (Google Maps, LINZ or the Australian national map), right-click your spot and copy the two numbers shown. The first is latitude and the second longitude. Use a minus sign for southern latitudes; longitudes east of Greenwich are positive. The UTC offset is your standard time zone (NZ is +12, eastern Australia +10), not the summer-time offset.")
                        .class("mg-hint");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Latitude (\u{b0})");
                            field.input(defaults::LAT).class("mg-input mg-lat");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Longitude (\u{b0})");
                            field.input(defaults::LON).class("mg-input mg-lon");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "UTC offset (h)");
                            field.input(defaults::TZ).class("mg-input mg-tz");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Altitude (m)");
                            field.input(defaults::ALT).class("mg-input mg-alt");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Day simulated");
                            field
                                .select(
                                    MONTH_NAMES
                                        .iter()
                                        .enumerate()
                                        .map(|(i, name)| (i.to_string(), name.to_string()))
                                        .collect::<Vec<_>>(),
                                    defaults::MONTH,
                                )
                                .class("mg-input mg-month");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.text("The 21st of the selected month is simulated, at the site's fixed UTC offset (daylight saving is not modelled). The free edition assumes clear sky and 15 degrees C, so results are best-case; Pro applies monthly cloud and temperature factors.")
                        .class("mg-hint");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Solar array");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Capacity (kWp)");
                            field.input(defaults::PV_CAP).class("mg-input mg-pv-cap");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Tilt (\u{b0})");
                            field.input(defaults::PV_TILT).class("mg-input mg-pv-tilt");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Azimuth (\u{b0})");
                            field.input(defaults::PV_AZ).class("mg-input mg-pv-az");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "PV degradation (%/yr)");
                            field.input(defaults::PV_DEG).class("mg-input mg-pv-deg");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "DC/AC ratio");
                            field.input(defaults::PV_DCAC).class("mg-input mg-pv-dcac");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.text("Azimuth from north: 0 = north-facing (southern-hemisphere sites), 180 = south-facing. Weather: the free edition simulates a clear day; the Pro edition varies cloud month by month. Degradation applies to the Pro sizing optimizer, which judges designs at end of life.")
                        .class("mg-hint");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Orientations and shading");
                    card.text("The main plane uses the solar tilt and azimuth above. Extra planes take a percentage of the capacity, and the main plane keeps the rest; leave shares at 0 for a single plane. Shading removes irradiance for whole hours of the day, and overlapping shading combines.")
                        .class("mg-hint");
                    for n in 1..=2 {
                        card.container(|row| {
                            row.container(|field| {
                                label(field, format!("Plane {n} share (%)"));
                                field.input("0").class(format!("mg-input mg-or{n}-share"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, format!("Plane {n} tilt (\u{b0})"));
                                field.input("30").class(format!("mg-input mg-or{n}-tilt"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, format!("Plane {n} azimuth (\u{b0})"));
                                field.input("90").class(format!("mg-input mg-or{n}-az"));
                            })
                            .class("mg-field");
                        })
                        .class("mg-row");
                    }
                    for n in 1..=2 {
                        card.container(|row| {
                            row.container(|field| {
                                label(field, format!("Shade {n}: from hour"));
                                field.input("0").class(format!("mg-input mg-ob{n}-start"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, format!("Shade {n}: to hour"));
                                field.input("0").class(format!("mg-input mg-ob{n}-end"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, format!("Shade {n}: loss (%)"));
                                field.input("0").class(format!("mg-input mg-ob{n}-loss"));
                            })
                            .class("mg-field");
                        })
                        .class("mg-row");
                    }
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Battery");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Chemistry");
                            // Index 0 is "Custom" (leaves the fields alone);
                            // index n selects `chemistries()[n - 1]`.
                            field
                                .select(
                                    std::iter::once("Custom (manual)")
                                        .chain(chemistries().iter().map(|c| c.name))
                                        .enumerate()
                                        .map(|(i, name)| (i.to_string(), name.to_string()))
                                        .collect::<Vec<_>>(),
                                    "1",
                                )
                                .class("mg-input mg-bat-chem")
                                .on_input(|value| {
                                    Msg::Chemistry(value.parse::<usize>().unwrap_or(0))
                                });
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Capacity (kWh)");
                            field.input(defaults::BAT_KWH).class("mg-input mg-bat-kwh");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Power (kW)");
                            field.input(defaults::BAT_KW).class("mg-input mg-bat-kw");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Round-trip eff. (%)");
                            field.input(defaults::BAT_RTE).class("mg-input mg-bat-rte");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Min SoC (%)");
                            field.input(defaults::BAT_MIN).class("mg-input mg-bat-min");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Initial SoC (%)");
                            field.input(defaults::BAT_INIT).class("mg-input mg-bat-init");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Capacity fade (%/yr)");
                            field.input(defaults::BAT_FADE).class("mg-input mg-bat-fade");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Self-discharge (%/day)");
                            field.input(defaults::BAT_SD).class("mg-input mg-bat-sd");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.text("Choosing a chemistry fills the battery fields (and, in Pro, the cost, life and kW/kWh inputs) with typical values; edit any of them afterwards.")
                        .class("mg-hint");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Load profile");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Preset");
                            field
                                .select(
                                    presets::all()
                                        .iter()
                                        .enumerate()
                                        .map(|(i, (name, _))| (i.to_string(), name.to_string()))
                                        .collect::<Vec<_>>(),
                                    "0",
                                )
                                .class("mg-input mg-preset")
                                .on_input(|value| {
                                    Msg::Preset(value.parse::<usize>().unwrap_or(0))
                                });
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|hours| {
                        for h in 0..HOURS_PER_DAY {
                            hours.container(|field| {
                                styled_text(field, format!("{h:02}"), "mg-hour-label");
                                field.input(&format!("{:.2}", presets::CABIN[h]))
                                    .class(format!("mg-input mg-hour mg-hour-{h:02}"));
                            })
                            .class("mg-hour");
                        }
                    })
                    .class("mg-hours");
                    card.container(|actions| {
                        actions
                            .button("Simulate day")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::SimulateDay);
                    })
                    .class("mg-actions");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Appliance load builder");
                    card.text("List what runs and when. Each appliance draws its power for the given hours, wrapping past midnight; the total fills the hourly load above. Leave rows at 0 hours to skip them.")
                        .class("mg-hint");
                    for (i, (kw, start, hours)) in defaults::APPLIANCES.iter().enumerate() {
                        let n = i + 1;
                        card.container(|row| {
                            row.container(|field| {
                                label(field, format!("{n}. Power (kW)"));
                                field.input(*kw).class(format!("mg-input mg-ap{n}-kw"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, "Start hour (0-23)");
                                field.input(*start).class(format!("mg-input mg-ap{n}-start"));
                            })
                            .class("mg-field");
                            row.container(|field| {
                                label(field, "Hours per day");
                                field.input(*hours).class(format!("mg-input mg-ap{n}-hours"));
                            })
                            .class("mg-field");
                        })
                        .class("mg-row");
                    }
                    card.container(|actions| {
                        actions
                            .button("Build hourly load")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::BuildLoad);
                    })
                    .class("mg-actions");
                    // The builder's result or errors mount into this placeholder.
                    card.container(|_| {}).class("mg-appliance-status");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Import data");
                    card.text("Paste a CSV. Load: 24 hourly kW values (one day) or 8760 (a full year), which fills the hourly load above and, in Pro, the monthly load factors. Any leading timestamp column is ignored.")
                        .class("mg-hint");
                    card.container(|field| {
                        label(field, "Load CSV");
                        field.textarea("").class("mg-input mg-import-load");
                    })
                    .class("mg-field");
                    card.container(|actions| {
                        actions
                            .button("Import load")
                            .class("mg-btn")
                            .on_click(Msg::ImportLoad);
                    })
                    .class("mg-actions");
                    #[cfg(feature = "pro")]
                    {
                        card.text("Weather: 8760 hourly rows of GHI, DNI and DHI (W/m\u{b2}), e.g. a PVGIS or NIWA typical year. It sets the monthly cloud factors to match the measured yield for the current site and array.")
                            .class("mg-hint");
                        card.container(|field| {
                            label(field, "Weather CSV");
                            field.textarea("").class("mg-input mg-import-weather");
                        })
                        .class("mg-field");
                        card.container(|actions| {
                            actions
                                .button("Import weather")
                                .class("mg-btn")
                                .on_click(Msg::ImportWeather);
                        })
                        .class("mg-actions");
                    }
                    card.container(|_| {}).class("mg-import-status");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                sidebar.container(|card| {
                    card.heading(3, "Seasonal variation (Pro)");
                    card.text("Monthly factors applied to the base profile, the clear sky, and the cell-temperature model.")
                        .class("mg-hint");
                    let factors = MonthlyFactors::temperate_southern();
                    card.container(|grid| {
                        grid.container(|_| {}).class("mg-factors-head");
                        for name in MONTH_NAMES {
                            styled_text(grid, &name[..3], "mg-factors-head");
                        }
                        styled_text(grid, "Load \u{d7}", "mg-factors-head");
                        for (m, value) in factors.load.iter().enumerate() {
                            grid_input(grid, "mg-loadf", m, *value, 2);
                        }
                        styled_text(grid, "Cloud \u{d7}", "mg-factors-head");
                        for (m, value) in factors.cloud.iter().enumerate() {
                            grid_input(grid, "mg-cloud", m, *value, 2);
                        }
                        styled_text(grid, "\u{b0}C", "mg-factors-head");
                        for (m, value) in factors.ambient_c.iter().enumerate() {
                            grid_input(grid, "mg-temp", m, *value, 1);
                        }
                    })
                    .class("mg-factors");
                    card.container(|actions| {
                        actions
                            .button("Run seasonal simulation")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::RunSeasonal);
                    })
                    .class("mg-actions");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                sidebar.container(|card| {
                    card.heading(3, "Sizing optimization (Pro)");
                    card.text("Searches solar \u{d7} battery combinations over the seasonal simulation for the cheapest design that meets the target.")
                        .class("mg-hint");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "PV cost ($/kWp)");
                            field
                                .input(defaults::OPT_PV_COST)
                                .class("mg-input mg-opt-pv-cost");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Battery cost ($/kWh)");
                            field
                                .input(defaults::OPT_BAT_COST)
                                .class("mg-input mg-opt-bat-cost");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Target served");
                            field
                                .select(
                                    [
                                        ("0.95", "95%"),
                                        ("0.98", "98%"),
                                        ("0.99", "99%"),
                                        ("0.999", "99.9%"),
                                    ],
                                    defaults::OPT_TARGET,
                                )
                                .class("mg-input mg-opt-target");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Battery kW/kWh");
                            field
                                .input(defaults::OPT_RATIO)
                                .class("mg-input mg-opt-ratio");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Discount rate (%)");
                            field
                                .input(defaults::OPT_DISCOUNT)
                                .class("mg-input mg-opt-discount");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Project life (years)");
                            field
                                .input(defaults::OPT_YEARS)
                                .class("mg-input mg-opt-years");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "O&M (% of capex/yr)");
                            field
                                .input(defaults::OPT_OM)
                                .class("mg-input mg-opt-om");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Battery life (years)");
                            field
                                .input(defaults::OPT_BAT_LIFE)
                                .class("mg-input mg-opt-bat-life");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Backup generator (kW, 0 = none)");
                            field
                                .input(defaults::GEN_KW)
                                .class("mg-input mg-gen-kw");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Fuel cost ($/L)");
                            field
                                .input(defaults::GEN_FUEL_COST)
                                .class("mg-input mg-gen-fuel");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Objective");
                            field
                                .select(
                                    [
                                        ("capex", "Lowest installed cost"),
                                        ("annual-cost", "Lowest annual cost"),
                                        ("lcoe", "Lowest cost of energy"),
                                    ],
                                    defaults::OPT_OBJECTIVE,
                                )
                                .class("mg-input mg-opt-objective");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Overcast spell (days)");
                            field
                                .input(defaults::SPELL_DAYS)
                                .class("mg-input mg-spell-days");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|actions| {
                        actions
                            .button("Run optimization")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::RunOptimize);
                    })
                    .class("mg-actions");
                })
                .class("mg-card");
            })
            .class("mg-sidebar");

            shell.container(|main| {
                main.container(|card| {
                    card.heading(3, "Load vs generation");
                    card.text("Solar serves the load directly; the battery shifts the rest. Press \u{201c}Simulate day\u{201d} to fill the chart.")
                        .class("mg-hint");
                    // The canvas is appended into .mg-chart by mount_app.
                    card.container(|holder| {
                        holder.container(|_| {}).class("mg-chart");
                        holder.container(|legend| {
                            legend.container(|item| {
                                styled_text(item, "Load (kW)", "mg-legend-label");
                            })
                            .class("mg-swatch-fg");
                            legend.container(|item| {
                                styled_text(item, "Solar (kW)", "mg-legend-label");
                            })
                            .class("mg-swatch-solar");
                            legend.container(|item| {
                                styled_text(item, "Battery SoC (%)", "mg-legend-label");
                            })
                            .class("mg-swatch-soc");
                            legend.container(|item| {
                                styled_text(item, "Shaded = unmet hours", "mg-legend-label");
                            })
                            .class("mg-swatch-unmet");
                        })
                        .class("mg-legend");
                    })
                    .class("mg-chartwrap");
                })
                .class("mg-card");
                main.container(|card| {
                    card.heading(3, "Results");
                    // The reactive results section mounts into this placeholder.
                    card.container(|_| {}).class("mg-results");
                    card.container(|actions| {
                        actions
                            .button("Download hourly CSV")
                            .class("mg-btn")
                            .on_click(Msg::ExportDayCsv);
                    })
                    .class("mg-actions");
                })
                .class("mg-card");

                main.container(|card| {
                    card.heading(3, "Saved scenarios");
                    card.text("Save the inputs to keep a design. Each scenario stores the metrics from the latest runs, so run the seasonal simulation and optimization before saving to compare them. Scenarios are kept in this browser.")
                        .class("mg-hint");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Scenario name");
                            field.input("").class("mg-input mg-scen-name");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|actions| {
                        actions
                            .button("Save scenario")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::SaveScenario);
                    })
                    .class("mg-actions");
                    // The comparison table mounts into this placeholder.
                    card.container(|_| {}).class("mg-scenarios");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                main.container(|card| {
                    card.heading(3, "Seasonal simulation (Pro)");
                    card.text("One representative day per month (the 21st), with the monthly load/cloud/temperature factors from the sidebar with each day repeated until the battery charge settles into a steady daily cycle (the initial SoC does not affect the seasonal result).")
                        .class("mg-hint");
                    // The seasonal canvas is appended here by mount_app.
                    card.container(|_| {}).class("mg-seasonal-chart");
                    // The reactive seasonal results mount into this placeholder.
                    card.container(|_| {}).class("mg-seasonal-results");
                    card.container(|actions| {
                        actions
                            .button("Download monthly CSV")
                            .class("mg-btn")
                            .on_click(Msg::ExportSeasonalCsv);
                        actions
                            .button("Export design report (.md)")
                            .class("mg-btn")
                            .on_click(Msg::ExportReport);
                        actions
                            .button("Export design report (.pdf)")
                            .class("mg-btn")
                            .on_click(Msg::ExportReportPdf);
                    })
                    .class("mg-actions");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                main.container(|card| {
                    card.heading(3, "Recommended sizing (Pro)");
                    // The reactive recommendation mounts into this placeholder.
                    card.container(|_| {}).class("mg-rec-results");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                main.container(|card| {
                    card.heading(3, "Inverter check (Pro)");
                    card.text("Sizes the inverter's AC rating (DC nameplate over the DC/AC ratio) against the peak demand, and estimates the energy the inverter clips across the year, using the monthly factors.")
                        .class("mg-hint");
                    card.container(|actions| {
                        actions
                            .button("Check inverter")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::CheckInverter);
                    })
                    .class("mg-actions");
                    // The reactive inverter result mounts into this placeholder.
                    card.container(|_| {}).class("mg-inverter");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                main.container(|card| {
                    card.heading(3, "Reliability (Pro)");
                    card.text("Loss of load over the year, from the same representative days as the seasonal run and including any backup generator, plus how long the battery alone can carry the worst month with no sun.")
                        .class("mg-hint");
                    card.container(|actions| {
                        actions
                            .button("Check reliability")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::CheckReliability);
                    })
                    .class("mg-actions");
                    // The reactive reliability result mounts into this placeholder.
                    card.container(|_| {}).class("mg-reliability");
                })
                .class("mg-card");

                #[cfg(feature = "pro")]
                main.container(|card| {
                    card.heading(3, "Grid connection and tariffs (Pro)");
                    card.text("The grid covers load the system leaves unmet (up to the import limit) and takes surplus the battery cannot store (up to the export limit). It never charges the battery. Tariffs are flat; export at the import price models net metering.")
                        .class("mg-hint");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Import price (US$/kWh)");
                            field.input(defaults::GRID_IMPORT).class("mg-input mg-grid-import");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Export credit (US$/kWh)");
                            field.input(defaults::GRID_EXPORT).class("mg-input mg-grid-export");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Daily charge (US$/day)");
                            field.input(defaults::GRID_DAILY).class("mg-input mg-grid-daily");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|row| {
                        row.container(|field| {
                            label(field, "Import limit (kW)");
                            field.input(defaults::GRID_IMPORT_LIMIT).class("mg-input mg-grid-imlim");
                        })
                        .class("mg-field");
                        row.container(|field| {
                            label(field, "Export limit (kW)");
                            field.input(defaults::GRID_EXPORT_LIMIT).class("mg-input mg-grid-exlim");
                        })
                        .class("mg-field");
                    })
                    .class("mg-row");
                    card.container(|actions| {
                        actions
                            .button("Price the year")
                            .class("mg-btn mg-primary")
                            .on_click(Msg::CheckGrid);
                    })
                    .class("mg-actions");
                    // The reactive grid result mounts into this placeholder.
                    card.container(|_| {}).class("mg-grid");
                })
                .class("mg-card");

                #[cfg(not(feature = "pro"))]
                main.container(|upsell| {
                    upsell.heading(3, "Will this system survive winter?");
                    upsell.text(
                        "The Pro edition ($149, desktop) adds full seasonal simulation \
                         (12 months of load/cloud/temperature variation), battery/generation \
                         sizing optimization that searches for the cheapest design meeting a \
                         served-energy target, and exportable system-design reports.",
                    );
                    upsell.link(
                        "https://tptsolutions.co.nz/tools/microgrid-sizer",
                        "Get the Pro edition",
                    );
                })
                .class("mg-upsell");
            })
            .class("mg-main");
        })
        .class("mg-shell");

        // Present in both editions — the Pro desktop shell has no browser
        // chrome and no other route back to tptsolutions.co.nz once
        // installed, so without this the app is a dead end for discovering
        // the rest of the product line. See `visit_site`/`footer_link` for
        // why this can't just be a plain `<a href>`.
        c.container(|footer| {
            styled_text(footer, "TPT Solutions", "mg-footer-brand");
            footer.container(|links| {
                footer_link(links, "More tools", "https://tptsolutions.co.nz/tools");
                footer_link(links, "tptsolutions.co.nz", "https://tptsolutions.co.nz");
            })
            .class("mg-footer-links");
        })
        .class("mg-footer");
    })
}

/// Narrow numeric input for the monthly-factors grid; `month` is baked into
/// the class so values can be read back per month.
#[cfg(feature = "pro")]
fn grid_input(
    c: &mut tpt_appfront_core::ContainerBuilder<Msg>,
    class: &str,
    month: usize,
    value: f64,
    decimals: usize,
) {
    c.input(&format!("{value:.decimals$}"))
        .class(format!("mg-input {class}-{month:02}"));
}

/// `.text()` produces a raw DOM text node, which can't carry a `class`
/// attribute (the reconciler no-ops classes for text nodes) — wrap the text
/// in a container so the styling actually reaches the page.
fn styled_text(
    c: &mut tpt_appfront_core::ContainerBuilder<Msg>,
    text: impl Into<String>,
    class: &str,
) {
    let text = text.into();
    c.container(move |c| {
        c.text(text);
    })
    .class(class);
}

/// Field label (see [`styled_text`]).
fn label(field: &mut tpt_appfront_core::ContainerBuilder<Msg>, text: impl Into<String>) {
    styled_text(field, text, "mg-label");
}

/// A footer "link" rendered as a plain `<button>` (not `<a>`) dispatching
/// [`Msg::VisitSite`]. Deliberately not `ContainerBuilder::link` — that
/// builder emits a real `href` alongside `on_click`, and the DOM
/// reconciler's click handler never calls `preventDefault`, so an anchor
/// with both would navigate the local `app://` webview *and* fire the
/// message. A button has no navigation behavior to suppress.
fn footer_link(c: &mut tpt_appfront_core::ContainerBuilder<Msg>, label: &str, url: &str) {
    c.button(label)
        .class("mg-footer-link")
        .on_click(Msg::VisitSite(url.to_string()));
}

/// Metrics stored with a saved scenario, taken from the last runs at save
/// time. A field is `None` when that run had not been done (for example,
/// the optimizer metrics in a scenario saved before optimizing).
#[derive(Clone, Default, PartialEq)]
pub(crate) struct ScenarioMetrics {
    /// Annual served fraction from the seasonal run, percent.
    pub served_pct: Option<f64>,
    /// Annual unmet energy from the seasonal run, kWh.
    pub unmet_kwh: Option<f64>,
    /// Recommended PV capacity, kWp.
    pub pv_kw: Option<f64>,
    /// Recommended battery capacity, kWh.
    pub battery_kwh: Option<f64>,
    /// Estimated installed capex, USD.
    pub capex_usd: Option<f64>,
    /// Levelised annual cost, USD per year.
    pub annual_cost_usd: Option<f64>,
    /// Levelised cost of energy, USD per kWh served.
    pub lcoe_usd_per_kwh: Option<f64>,
}

impl ScenarioMetrics {
    /// Storage keys and values, in a fixed order.
    pub(crate) fn fields(&self) -> [(&'static str, Option<f64>); 7] {
        [
            ("served_pct", self.served_pct),
            ("unmet_kwh", self.unmet_kwh),
            ("pv_kw", self.pv_kw),
            ("battery_kwh", self.battery_kwh),
            ("capex_usd", self.capex_usd),
            ("annual_cost_usd", self.annual_cost_usd),
            ("lcoe_usd_per_kwh", self.lcoe_usd_per_kwh),
        ]
    }

    /// Rebuilds from stored values; `get` returns `None` for a missing key.
    pub(crate) fn from_fields(get: impl Fn(&str) -> Option<f64>) -> Self {
        Self {
            served_pct: get("served_pct"),
            unmet_kwh: get("unmet_kwh"),
            pv_kw: get("pv_kw"),
            battery_kwh: get("battery_kwh"),
            capex_usd: get("capex_usd"),
            annual_cost_usd: get("annual_cost_usd"),
            lcoe_usd_per_kwh: get("lcoe_usd_per_kwh"),
        }
    }

    /// Comparison-table rows: label and display value ("—" when unknown).
    pub(crate) fn rows(&self) -> Vec<(&'static str, String)> {
        fn shown(value: Option<f64>, format: impl Fn(f64) -> String) -> String {
            match value {
                Some(v) if v.is_finite() => format(v),
                _ => "\u{2014}".to_string(),
            }
        }
        vec![
            ("Annual served", shown(self.served_pct, |v| format!("{v:.1}%"))),
            ("Unmet energy per year", shown(self.unmet_kwh, fmt_kwh)),
            ("PV (kWp)", shown(self.pv_kw, |v| format!("{v:.1}"))),
            ("Battery (kWh)", shown(self.battery_kwh, |v| format!("{v:.1}"))),
            ("Capex (US$)", shown(self.capex_usd, |v| format!("{v:.0}"))),
            ("Annual cost (US$/yr)", shown(self.annual_cost_usd, |v| format!("{v:.0}"))),
            ("LCOE (US$/kWh)", shown(self.lcoe_usd_per_kwh, |v| format!("{v:.3}"))),
        ]
    }
}

/// A saved scenario: its name, every input value in page order, and the
/// metrics captured when it was saved.
#[derive(Clone, PartialEq)]
pub(crate) struct Scenario {
    /// User-given name (shown as the column header).
    pub name: String,
    /// Input values in DOM order, restored by position.
    pub values: Vec<String>,
    /// Metrics from the last runs at save time.
    pub metrics: ScenarioMetrics,
}

/// The side-by-side comparison of saved scenarios: one column per scenario,
/// one row per metric, with Load and Delete under each name.
pub(crate) fn scenarios_view(scenarios: Vec<Scenario>) -> UITree<Msg> {
    UITree::container(|c| {
        if scenarios.is_empty() {
            styled_text(
                c,
                "No saved scenarios yet. Save the current inputs to compare designs side by side.",
                "mg-detail",
            );
            return;
        }
        c.container(|table| {
            table
                .container(|row| {
                    row.container(|_| {}).class("mg-cmp-head");
                    for (i, scenario) in scenarios.iter().enumerate() {
                        row.container(|cell| {
                            styled_text(cell, scenario.name.clone(), "mg-cmp-name");
                            cell.container(|actions| {
                                actions
                                    .button("Load")
                                    .class("mg-btn")
                                    .on_click(Msg::LoadScenario(i));
                                actions
                                    .button("Delete")
                                    .class("mg-btn")
                                    .on_click(Msg::DeleteScenario(i));
                            })
                            .class("mg-actions");
                        })
                        .class("mg-cmp-cell");
                    }
                })
                .class("mg-cmp-row");
            let labels = scenarios[0].metrics.rows();
            for (r, (label, _)) in labels.iter().enumerate() {
                table
                    .container(|row| {
                        styled_text(row, *label, "mg-cmp-label");
                        for scenario in &scenarios {
                            let value = scenario.metrics.rows()[r].1.clone();
                            styled_text(row, value, "mg-cmp-cell");
                        }
                    })
                    .class("mg-cmp-row");
            }
        })
        .class("mg-cmp");
    })
}

/// The inverter check's current content (Pro).
#[cfg(feature = "pro")]
#[derive(Clone)]
pub(crate) enum InverterOutcome {
    Idle,
    Invalid(Vec<String>),
    Solved(Rc<InverterReport>),
}

/// The reliability check's current content (Pro).
#[cfg(feature = "pro")]
#[derive(Clone)]
pub(crate) enum ReliabilityOutcome {
    Idle,
    Invalid(Vec<String>),
    Solved(Rc<ReliabilityReport>),
}

/// The grid check's current content (Pro): the year's grid energy and bills,
/// and the installed cost used for the payback.
#[cfg(feature = "pro")]
#[derive(Clone)]
pub(crate) enum GridOutcome {
    Idle,
    Invalid(Vec<String>),
    Solved(Rc<GridYear>, f64),
}

/// The reactive grid section (Pro).
#[cfg(feature = "pro")]
pub(crate) fn grid_view(outcome: GridOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        GridOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Price the year\u{201d} to compare the bill with and without the system, given the tariff and grid limits.",
                "mg-detail",
            );
        }
        GridOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        GridOutcome::Solved(year, capex_usd) => {
            // A negative bill is a credit: exports earned more than the
            // imports and the daily charge cost.
            let with = if year.bill_with_system_usd < 0.0 {
                format!("a credit of US$ {:.0}", -year.bill_with_system_usd)
            } else {
                format!("US$ {:.0}", year.bill_with_system_usd)
            };
            styled_text(
                c,
                format!(
                    "Annual bill: US$ {:.0} without the system, {with} with it.",
                    year.bill_without_system_usd
                ),
                "mg-headline",
            );
            let savings = year.savings_usd();
            let payback = match year.simple_payback_years(capex_usd) {
                Some(years) => format!("simple payback on US$ {capex_usd:.0} in {years:.1} years"),
                None if capex_usd > 0.0 => "no saving, so no payback".to_string(),
                None => "no system cost entered, so no payback".to_string(),
            };
            styled_text(
                c,
                format!("Saving US$ {savings:.0} a year; {payback}."),
                if savings > 0.0 { "mg-detail" } else { "mg-warn" },
            );
            styled_text(
                c,
                format!(
                    "Bought {}, sold {} over the year.",
                    fmt_kwh(year.imported_kwh),
                    fmt_kwh(year.exported_kwh)
                ),
                "mg-detail",
            );
            if year.unmet_kwh > 0.01 {
                styled_text(
                    c,
                    format!(
                        "Load still unserved after the import limit: {} a year.",
                        fmt_kwh(year.unmet_kwh)
                    ),
                    "mg-warn",
                );
            }
        }
    })
}

/// The reactive reliability section (Pro).
#[cfg(feature = "pro")]
pub(crate) fn reliability_view(outcome: ReliabilityOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        ReliabilityOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Check reliability\u{201d} for how often load goes unserved and how long the battery alone can carry the worst month.",
                "mg-detail",
            );
        }
        ReliabilityOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        ReliabilityOutcome::Solved(rel) => {
            styled_text(
                c,
                format!(
                    "{:.2}% of hours unserved ({:.0} hours a year), on {:.1} days a year.",
                    rel.lolp * 100.0,
                    rel.loss_of_load_hours_per_year,
                    rel.lole_days_per_year
                ),
                "mg-headline",
            );
            let capped = rel.autonomy_days >= MAX_AUTONOMY_DAYS;
            styled_text(
                c,
                format!(
                    "Battery alone carries the worst month for {:.2} days{}.",
                    rel.autonomy_days,
                    if capped {
                        format!(" (capped at {MAX_AUTONOMY_DAYS:.0})")
                    } else {
                        String::new()
                    }
                ),
                "mg-detail",
            );
        }
    })
}

/// The reactive inverter-check section (Pro).
#[cfg(feature = "pro")]
pub(crate) fn inverter_view(outcome: InverterOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        InverterOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Check inverter\u{201d} to compare the AC rating with the peak demand and estimate clipping.",
                "mg-detail",
            );
        }
        InverterOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        InverterOutcome::Solved(check) => {
            styled_text(
                c,
                format!(
                    "AC rating {:.2} kW for {:.1} kWp DC (DC/AC {:.2}).",
                    check.ac_rating_kw, check.dc_kwp, check.dc_ac_ratio
                ),
                "mg-headline",
            );
            if check.covers_peak {
                styled_text(
                    c,
                    format!(
                        "Peak demand {:.2} kW is covered by the rating.",
                        check.peak_demand_kw
                    ),
                    "mg-detail",
                );
            } else {
                styled_text(
                    c,
                    format!(
                        "Peak demand {:.2} kW exceeds the rating: the inverter is too small for the peak. Lower the DC/AC ratio or add inverter capacity.",
                        check.peak_demand_kw
                    ),
                    "mg-warn",
                );
            }
            styled_text(
                c,
                format!(
                    "Clipping loses {} per year, {:.1}% of the unclipped PV output.",
                    fmt_kwh(check.clipped_kwh_per_year),
                    check.clipped_fraction * 100.0
                ),
                "mg-detail",
            );
        }
    })
}

/// The import card's last result: nothing yet, a success note, or the
/// reasons a file was refused.
#[derive(Clone)]
pub(crate) enum ImportOutcome {
    Idle,
    Done(String),
    Failed(Vec<String>),
}

/// The reactive import status line, mounted under the import inputs.
pub(crate) fn import_status_view(outcome: ImportOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        ImportOutcome::Idle => {}
        ImportOutcome::Done(text) => {
            styled_text(c, text, "mg-detail");
        }
        ImportOutcome::Failed(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
    })
}

/// The reactive single-day results section.
pub(crate) fn day_results_view(outcome: DayOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        DayOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Simulate day\u{201d} to run the hourly balance.",
                "mg-detail",
            );
        }
        DayOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        DayOutcome::Solved(result) => {
            let totals = &result.totals;
            let served = result.served_fraction();
            c.container(|summary| {
                styled_text(
                    summary,
                    format!(
                        "{:.0}% of {}'s load is served by solar + battery.",
                        served * 100.0,
                        MONTH_NAMES[result.month]
                    ),
                    "mg-headline",
                );
                styled_text(
                    summary,
                    if totals.unmet_kwh > 0.01 {
                        format!(
                            "{:.1} kWh unmet across {} hour(s) without sun or charge — the shaded bands on the chart.",
                            totals.unmet_kwh,
                            result.hours.iter().filter(|h| h.unmet_kw > 0.01).count()
                        )
                    } else {
                        "Every hour of the day is covered by solar or stored charge.".to_string()
                    },
                    "mg-detail",
                );
            })
            .class("mg-summary");

            c.container(|stats| {
                stat(stats, &format!("{:.1}", totals.load_kwh), "Load kWh");
                stat(stats, &format!("{:.1}", totals.solar_kwh), "Solar kWh");
                stat(stats, &format!("{:.1}", totals.unmet_kwh), "Unmet kWh");
                stat(stats, &format!("{:.1}", totals.curtailed_kwh), "Curtailed kWh");
                stat(stats, &format!("{:.1}", totals.discharged_kwh), "Battery out kWh");
                stat(stats, &format!("{:.2}", result.battery_cycles), "Equiv. cycles");
                stat(stats, &format!("{:.0}%", totals.end_soc * 100.0), "End SoC");
            })
            .class("mg-stats");

            c.container(|table| {
                table.container(|row| {
                    styled_text(row, "Hour", "mg-cell");
                    styled_text(row, "Load kW", "mg-cellnum");
                    styled_text(row, "Solar kW", "mg-cellnum");
                    styled_text(row, "Battery kW", "mg-cellnum");
                    styled_text(row, "SoC %", "mg-cellnum");
                    styled_text(row, "Unmet kW", "mg-cellnum");
                })
                .class("mg-thead");
                for (h, point) in result.hours.iter().enumerate() {
                    table.container(|row| {
                        styled_text(row, format!("{h:02}:00"), "mg-cell");
                        styled_text(row, format!("{:.2}", point.load_kw), "mg-cellnum");
                        styled_text(row, format!("{:.2}", point.solar_kw), "mg-cellnum");
                        styled_text(row, format!("{:+.2}", point.battery_kw), "mg-cellnum");
                        styled_text(row, format!("{:.0}", point.soc * 100.0), "mg-cellnum");
                        if point.unmet_kw > 0.01 {
                            styled_text(row, format!("{:.2}", point.unmet_kw), "mg-cellnum mg-warn");
                        } else {
                            styled_text(row, "0.00", "mg-cellnum");
                        }
                    })
                    .class("mg-trow");
                }
            })
            .class("mg-table");
        }
    })
}

/// One summary stat chip.
fn stat(c: &mut tpt_appfront_core::ContainerBuilder<Msg>, value: &str, label: &str) {
    c.container(|chip| {
        styled_text(chip, value, "mg-stat-value");
        styled_text(chip, label, "mg-stat-label");
    })
    .class("mg-stat");
}

/// Chart data for the single-day balance.
pub(crate) fn day_chart(result: &DayResult) -> chart::Chart {
    let hours: Vec<String> = (0..HOURS_PER_DAY).map(|h| format!("{h:02}:00")).collect();
    chart::Chart {
        series: vec![
            chart::ChartSeries {
                name: "Solar",
                color: "#e0972a",
                fill_color: Some("rgba(224,151,42,0.20)"),
                values: result.hours.iter().map(|h| h.solar_kw).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                name: "Load",
                color: "#4a5568",
                fill_color: None,
                values: result.hours.iter().map(|h| h.load_kw).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                name: "Battery charge",
                color: "#2e9e6b",
                fill_color: None,
                values: result.hours.iter().map(|h| h.soc * 100.0).collect(),
                dashed: true,
                right_axis: true,
            },
        ],
        x_labels: hours,
        left_title: "kW".to_string(),
        right_title: Some("%".to_string()),
        right_max: 100.0,
        highlight: result.hours.iter().map(|h| h.unmet_kw > 0.01).collect(),
    }
}

/// The reactive seasonal results section.
#[cfg(feature = "pro")]
pub(crate) fn seasonal_results_view(outcome: SeasonalOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        SeasonalOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Run seasonal simulation\u{201d} to model the full year.",
                "mg-detail",
            );
        }
        SeasonalOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        SeasonalOutcome::Solved(result, spell) => {
            c.container(|summary| {
                styled_text(
                    summary,
                    format!("{:.1}% of annual load served.", result.served_fraction() * 100.0),
                    "mg-headline",
                );
                styled_text(
                    summary,
                    format!(
                        "{} unmet of {} across the year; the governing month is {} at {:.1}% served.",
                        fmt_kwh(result.unmet_kwh),
                        fmt_kwh(result.load_kwh),
                        MONTH_NAMES[result.worst_month],
                        result.worst_month_served_fraction() * 100.0
                    ),
                    "mg-detail",
                );
                if let Some(spell) = spell {
                    styled_text(
                        summary,
                        if spell.survived {
                            format!(
                                "Overcast spell: all {} consecutive {} days served from a full battery.",
                                spell.days_simulated,
                                MONTH_NAMES[spell.month]
                            )
                        } else {
                            format!(
                                "Overcast spell: load first unmet on day {} of {} in {} ({} unmet over the spell).",
                                spell.days_to_failure + 1,
                                spell.days_simulated,
                                MONTH_NAMES[spell.month],
                                fmt_kwh(spell.unmet_kwh)
                            )
                        },
                        "mg-detail",
                    );
                }
            })
            .class("mg-summary");

            c.container(|table| {
                table.container(|row| {
                    styled_text(row, "Month", "mg-cell");
                    styled_text(row, "Load kWh", "mg-cellnum");
                    styled_text(row, "Solar kWh", "mg-cellnum");
                    styled_text(row, "Battery kWh", "mg-cellnum");
                    styled_text(row, "Unmet kWh", "mg-cellnum");
                    styled_text(row, "Served %", "mg-cellnum");
                })
                .class("mg-thead");
                for m in &result.months {
                    table.container(|row| {
                        styled_text(row, MONTH_NAMES[m.month], "mg-cell");
                        styled_text(row, format!("{:.1}", m.load_kwh), "mg-cellnum");
                        styled_text(row, format!("{:.1}", m.solar_kwh), "mg-cellnum");
                        styled_text(row, format!("{:.1}", m.discharged_kwh), "mg-cellnum");
                        if m.unmet_kwh > 0.01 {
                            styled_text(row, format!("{:.1}", m.unmet_kwh), "mg-cellnum mg-warn");
                        } else {
                            styled_text(row, "0.0", "mg-cellnum");
                        }
                        styled_text(
                            row,
                            format!("{:.1}", percent_served(m.unmet_kwh, m.load_kwh)),
                            "mg-cellnum",
                        );
                    })
                    .class("mg-trow");
                }
            })
            .class("mg-table");
        }
    })
}

#[cfg(feature = "pro")]
fn percent_served(unmet: f64, load: f64) -> f64 {
    if load <= 1e-9 {
        100.0
    } else {
        ((1.0 - unmet / load) * 100.0).clamp(0.0, 100.0)
    }
}

fn fmt_kwh(kwh: f64) -> String {
    if kwh >= 10_000.0 {
        format!("{:.1} MWh", kwh / 1000.0)
    } else {
        format!("{:.1} kWh", kwh)
    }
}

/// Chart data for the seasonal overview.
#[cfg(feature = "pro")]
pub(crate) fn seasonal_chart(result: &SeasonalResult) -> chart::Chart {
    chart::Chart {
        series: vec![
            chart::ChartSeries {
                name: "Solar",
                color: "#e0972a",
                fill_color: Some("rgba(224,151,42,0.20)"),
                values: result.months.iter().map(|m| m.solar_kwh).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                name: "Load",
                color: "#4a5568",
                fill_color: None,
                values: result.months.iter().map(|m| m.load_kwh).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                name: "Unmet",
                color: "#b3261e",
                fill_color: None,
                values: result.months.iter().map(|m| m.unmet_kwh).collect(),
                dashed: false,
                right_axis: false,
            },
        ],
        x_labels: MONTH_NAMES.iter().map(|n| n[..3].to_string()).collect(),
        left_title: "kWh/month".to_string(),
        right_title: None,
        right_max: 100.0,
        highlight: vec![false; MONTHS_PER_YEAR],
    }
}

/// The reactive recommendation section.
#[cfg(feature = "pro")]
pub(crate) fn rec_results_view(outcome: RecOutcome) -> UITree<Msg> {
    UITree::container(|c| match outcome {
        RecOutcome::Idle => {
            styled_text(
                c,
                "Press \u{201c}Run optimization\u{201d} to search for the cheapest design meeting the target.",
                "mg-detail",
            );
        }
        RecOutcome::Invalid(issues) => {
            c.container(|list| {
                for issue in issues {
                    styled_text(list, issue, "mg-issue");
                }
            })
            .class("mg-issues");
        }
        RecOutcome::Solved(rec) => {
            c.container(|summary| {
                styled_text(
                    summary,
                    format!(
                        "Recommended: {:.1} kWp solar + {:.1} kWh battery at {:.1} kW.",
                        rec.solar_kw,
                        rec.battery_kwh,
                        rec.battery_kw
                    ),
                    "mg-headline",
                );
                styled_text(
                    summary,
                    if rec.feasible {
                        format!(
                            "Meets the target at {:.1}% served ({} unmet/year). Estimated capex US$ {:.0}; levelised cost US$ {:.3}/kWh served (US$ {:.0}/year).",
                            rec.served_fraction * 100.0,
                            fmt_kwh(rec.unmet_kwh),
                            rec.capex_usd,
                            rec.lcoe_usd_per_kwh,
                            rec.annual_cost_usd
                        )
                    } else {
                        format!(
                            "No candidate in the search grid met the target — best effort serves {:.1}% ({} unmet/year) for US$ {:.0}. Consider a backup generator.",
                            rec.served_fraction * 100.0,
                            fmt_kwh(rec.unmet_kwh),
                            rec.capex_usd
                        )
                    },
                    "mg-detail",
                );
            })
            .class("mg-summary");

            c.container(|actions| {
                actions
                    .button("Apply to inputs")
                    .class("mg-btn")
                    .on_click(Msg::ApplyRecommendation);
            })
            .class("mg-actions");
        }
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use tpt_appfront_core::inspect_tree;
    use tpt_microgrid_engine::{BatterySpec, DayLoad, Site, SolarArray};
    #[cfg(feature = "pro")]
    use tpt_microgrid_engine::{MonthSummary, SizingRecommendation, SpellResult};

    /// A January day that empties the battery after five hours — the same
    /// hand-checkable scenario the engine tests use, so the unmet-hour
    /// highlighting has real data to work with.
    fn solved_day() -> DayResult {
        let array = SolarArray {
            cloud_factor: 0.0,
            ..SolarArray::default()
        };
        let battery = BatterySpec {
            capacity_kwh: 10.0,
            power_kw: 5.0,
            round_trip_efficiency: 1.0,
            min_soc: 0.0,
            initial_soc: 1.0,
            annual_fade: 0.0,
            self_discharge_per_day: 0.0,
        };
        let load = DayLoad {
            hourly_kw: vec![2.0; HOURS_PER_DAY],
        };
        tpt_microgrid_engine::simulate_day(&Site::default(), &array, &battery, &load, 0)
    }

    #[test]
    fn shell_has_site_solar_battery_and_day_controls() {
        let tree = inspect_tree(&shell_tree());
        for class in [
            "mg-lat", "mg-lon", "mg-tz", "mg-alt", "mg-month", "mg-pv-cap", "mg-pv-tilt",
            "mg-pv-az", "mg-pv-deg", "mg-pv-dcac", "mg-bat-kwh", "mg-bat-kw", "mg-bat-rte", "mg-bat-min",
            "mg-bat-init", "mg-bat-fade", "mg-bat-sd", "mg-bat-chem", "mg-preset",
            "mg-import-load", "mg-import-status", "mg-scen-name", "mg-scenarios",
            "mg-ap1-kw", "mg-ap6-hours", "mg-appliance-status", "mg-or1-share", "mg-ob2-loss",
            "mg-theme", "mg-site-preset", "mg-site-status",
        ] {
            assert!(tree.contains(class), "shell is missing {class}");
        }
        assert!(tree.contains("Button \"Simulate day\""));
        // All 24 hourly load fields, each labelled by hour.
        for h in 0..HOURS_PER_DAY {
            assert!(tree.contains(&format!("mg-hour-{h:02}")), "missing hour {h:02}");
        }
        assert!(tree.contains("mg-footer-link"), "footer links missing");
    }

    #[cfg(not(feature = "pro"))]
    #[test]
    fn free_shell_excludes_pro_controls() {
        let tree = inspect_tree(&shell_tree());
        for absent in [
            "mg-opt-objective",
            "mg-spell-days",
            "Run seasonal simulation",
            "Run optimization",
            "Export design report",
            "mg-loadf-00",
        ] {
            assert!(!tree.contains(absent), "free build must not offer {absent}");
        }
        assert!(
            tree.contains("Get the Pro edition"),
            "free build keeps the upsell"
        );
    }

    #[cfg(feature = "pro")]
    #[test]
    fn pro_shell_includes_objective_and_overcast_spell_inputs() {
        let tree = inspect_tree(&shell_tree());
        // Objective select: three choices, annual-cost preselected.
        assert!(
            tree.lines().any(|l| l.contains("mg-opt-objective")
                && l.contains("selected=\"annual-cost\"")
                && l.contains("(\"capex\", \"Lowest installed cost\")")
                && l.contains("(\"lcoe\", \"Lowest cost of energy\")")),
            "objective select missing or wrong: {tree}"
        );
        // Overcast spell days input, defaulting to five.
        assert!(
            tree.lines()
                .any(|l| l.contains("mg-spell-days") && l.contains("value=\"5\"")),
            "overcast spell input missing or wrong default"
        );
        assert!(tree.contains("Button \"Run seasonal simulation\""));
        assert!(tree.contains("Button \"Run optimization\""));
        assert!(tree.contains("Button \"Export design report (.md)\""));
        assert!(tree.contains("mg-loadf-00"), "monthly factors grid missing");
        assert!(
            !tree.contains("Get the Pro edition"),
            "pro build drops the upsell"
        );
        assert!(tree.contains("Text \"Pro\""), "pro chip missing");
    }

    #[test]
    fn day_results_view_renders_idle_invalid_and_solved() {
        let idle = inspect_tree(&day_results_view(DayOutcome::Idle));
        assert!(idle.contains("to run the hourly balance"));

        let invalid = inspect_tree(&day_results_view(DayOutcome::Invalid(vec![
            "Latitude must be a number.".to_string(),
        ])));
        assert!(invalid.contains("Latitude must be a number."));
        assert!(invalid.contains("class=\"mg-issues\""));

        let solved = inspect_tree(&day_results_view(DayOutcome::Solved(Rc::new(
            solved_day(),
        ))));
        // 10 kWh battery against a 48 kWh day serves 10/48 -> 21%.
        assert!(
            solved.contains("21% of January's load is served by solar + battery."),
            "headline wrong: {solved}"
        );
        assert!(solved.contains("kWh unmet across"));
        assert!(solved.contains("Load kWh"));
        assert!(solved.contains("Text \"23:00\""), "hourly table missing");
    }

    #[test]
    fn day_chart_builds_three_series_and_highlights_unmet_hours() {
        let chart = day_chart(&solved_day());
        assert_eq!(chart.series.len(), 3);
        assert_eq!(chart.x_labels.len(), HOURS_PER_DAY);
        assert_eq!(chart.x_labels[0], "00:00");
        assert_eq!(chart.x_labels[HOURS_PER_DAY - 1], "23:00");
        assert_eq!(chart.left_title, "kW");
        assert_eq!(chart.right_title.as_deref(), Some("%"));
        assert_eq!(chart.right_max, 100.0);
        // Solar area fill, plain load line, dashed SoC on the right axis.
        assert!(chart.series[0].fill_color.is_some());
        assert!(chart.series[1].fill_color.is_none());
        assert!(chart.series[2].right_axis && chart.series[2].dashed);
        // The battery covers hour 0; after five hours the day runs unmet.
        assert_eq!(chart.highlight.len(), HOURS_PER_DAY);
        assert!(!chart.highlight[0], "battery serves the first hour");
        assert!(chart.highlight[HOURS_PER_DAY - 1], "flat battery is unmet");
        // SoC series is plotted as a percentage.
        assert!(
            chart.series[2]
                .values
                .iter()
                .all(|v| (0.0..=100.0).contains(v))
        );
    }

    #[cfg(feature = "pro")]
    fn seasonal() -> SeasonalResult {
        SeasonalResult {
            months: (0..MONTHS_PER_YEAR)
                .map(|m| MonthSummary {
                    month: m,
                    load_kwh: 400.0,
                    solar_kwh: 450.0,
                    unmet_kwh: if m == 5 { 360.0 } else { 0.0 },
                    curtailed_kwh: 10.0,
                    discharged_kwh: 300.0,
                    generator_kwh: 0.0,
                })
                .collect(),
            load_kwh: 4800.0,
            solar_kwh: 5400.0,
            unmet_kwh: 360.0,
            curtailed_kwh: 120.0,
            generator_kwh: 0.0,
            fuel_litres: 0.0,
            worst_month_unmet_kwh: 360.0,
            worst_month: 5,
        }
    }

    #[cfg(feature = "pro")]
    fn spell_survived() -> SpellResult {
        SpellResult {
            month: 5,
            days_simulated: 5,
            days_to_failure: 5,
            survived: true,
            unmet_kwh: 0.0,
            served_kwh: 60.0,
        }
    }

    #[cfg(feature = "pro")]
    fn recommendation() -> SizingRecommendation {
        SizingRecommendation {
            solar_kw: 12.0,
            battery_kwh: 24.0,
            battery_kw: 6.0,
            capex_usd: 30000.0,
            served_fraction: 0.99,
            unmet_kwh: 48.0,
            fuel_litres: 0.0,
            feasible: true,
            annual_cost_usd: 15000.0,
            lcoe_usd_per_kwh: 0.5,
        }
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_view_summarises_annual_and_survived_spell() {
        let tree = inspect_tree(&seasonal_results_view(SeasonalOutcome::Solved(
            Rc::new(seasonal()),
            Some(Rc::new(spell_survived())),
        )));
        assert!(
            tree.contains("92.5% of annual load served."),
            "annual headline wrong: {tree}"
        );
        assert!(tree.contains("the governing month is June at 10.0% served."));
        assert!(
            tree.contains(
                "Overcast spell: all 5 consecutive June days served from a full battery."
            ),
            "spell survived line missing"
        );
        assert!(tree.contains("Text \"December\""), "monthly table missing");
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_view_reports_spell_failure_day() {
        let failed = SpellResult {
            days_to_failure: 2,
            survived: false,
            unmet_kwh: 36.0,
            served_kwh: 24.0,
            ..spell_survived()
        };
        let tree = inspect_tree(&seasonal_results_view(SeasonalOutcome::Solved(
            Rc::new(seasonal()),
            Some(Rc::new(failed)),
        )));
        assert!(
            tree.contains(
                "Overcast spell: load first unmet on day 3 of 5 in June (36.0 kWh unmet over the spell)."
            ),
            "failure line wrong: {tree}"
        );
    }

    #[cfg(feature = "pro")]
    #[test]
    fn rec_view_shows_recommendation_and_apply_button() {
        let tree = inspect_tree(&rec_results_view(RecOutcome::Solved(Rc::new(
            recommendation(),
        ))));
        assert!(
            tree.contains("Recommended: 12.0 kWp solar + 24.0 kWh battery at 6.0 kW."),
            "headline wrong: {tree}"
        );
        assert!(tree.contains("Estimated capex US$ 30000;"));
        assert!(tree.contains("Button \"Apply to inputs\""));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn rec_view_reports_infeasible_search() {
        let best_effort = SizingRecommendation {
            feasible: false,
            served_fraction: 0.9,
            ..recommendation()
        };
        let tree = inspect_tree(&rec_results_view(RecOutcome::Solved(Rc::new(best_effort))));
        assert!(
            tree.contains("No candidate in the search grid met the target"),
            "infeasible line missing"
        );
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_chart_plots_twelve_month_labels() {
        let chart = seasonal_chart(&seasonal());
        assert_eq!(chart.x_labels.len(), MONTHS_PER_YEAR);
        assert_eq!(chart.x_labels[0], "Jan");
        assert_eq!(chart.x_labels[11], "Dec");
        assert_eq!(chart.series.len(), 3);
        assert_eq!(chart.left_title, "kWh/month");
        assert!(chart.right_title.is_none());
        assert!(chart.highlight.iter().all(|flag| !flag));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn fmt_kwh_switches_to_mwh_above_ten_thousand() {
        assert_eq!(fmt_kwh(512.34), "512.3 kWh");
        assert_eq!(fmt_kwh(9999.0), "9999.0 kWh");
        assert_eq!(fmt_kwh(12345.0), "12.3 MWh");
    }

    #[cfg(feature = "pro")]
    #[test]
    fn percent_served_handles_edges() {
        assert!((percent_served(0.0, 100.0) - 100.0).abs() < 1e-9);
        assert!((percent_served(10.0, 100.0) - 90.0).abs() < 1e-9);
        assert!((percent_served(0.0, 0.0) - 100.0).abs() < 1e-9, "no load");
        assert!((percent_served(200.0, 100.0) - 0.0).abs() < 1e-9, "clamped");
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_and_rec_idle_and_invalid_states_render() {
        let idle = inspect_tree(&seasonal_results_view(SeasonalOutcome::Idle));
        assert!(idle.contains("to model the full year"), "idle prompt missing");
        assert!(
            inspect_tree(&rec_results_view(RecOutcome::Idle))
                .contains("search for the cheapest design"),
            "rec idle prompt missing"
        );

        let invalid = inspect_tree(&seasonal_results_view(SeasonalOutcome::Invalid(vec![
            "Latitude must be a number.".to_string(),
        ])));
        assert!(invalid.contains("Latitude must be a number."));
        assert!(invalid.contains("class=\"mg-issues\""));

        let rec_invalid = inspect_tree(&rec_results_view(RecOutcome::Invalid(vec![
            "Target served fraction must be between 50% and 100%.".to_string(),
        ])));
        assert!(rec_invalid.contains("Target served fraction must be between 50% and 100%."));
    }

    #[test]
    fn msg_payloads_round_trip() {
        let preset = Msg::Preset(2);
        let Msg::Preset(index) = preset else {
            panic!("expected Preset");
        };
        assert_eq!(index, 2);

        let visit = Msg::VisitSite("https://tptsolutions.co.nz/tools".to_string());
        let Msg::VisitSite(url) = visit else {
            panic!("expected VisitSite");
        };
        assert_eq!(url, "https://tptsolutions.co.nz/tools");

        let chemistry = Msg::Chemistry(3);
        let Msg::Chemistry(index) = chemistry else {
            panic!("expected Chemistry");
        };
        assert_eq!(index, 3);

        let site = Msg::SitePreset(3);
        let Msg::SitePreset(index) = site else {
            panic!("expected SitePreset");
        };
        assert_eq!(index, 3);
        let theme = Msg::Theme(2);
        let Msg::Theme(index) = theme else {
            panic!("expected Theme");
        };
        assert_eq!(index, 2);

        let load = Msg::LoadScenario(1);
        let Msg::LoadScenario(index) = load else {
            panic!("expected LoadScenario");
        };
        assert_eq!(index, 1);
        let delete = Msg::DeleteScenario(2);
        let Msg::DeleteScenario(index) = delete else {
            panic!("expected DeleteScenario");
        };
        assert_eq!(index, 2);
    }

    #[test]
    fn import_status_renders_each_outcome() {
        let failed = inspect_tree(&import_status_view(ImportOutcome::Failed(vec![
            "Found 3 data rows; expected 24.".to_string(),
        ])));
        assert!(failed.contains("Found 3 data rows"));
        assert!(failed.contains("mg-issues"));

        let done = inspect_tree(&import_status_view(ImportOutcome::Done(
            "Loaded a 24-hour profile into the hourly load.".to_string(),
        )));
        assert!(done.contains("Loaded a 24-hour profile"));
        assert!(!done.contains("mg-issues"));

        let idle = inspect_tree(&import_status_view(ImportOutcome::Idle));
        assert!(!idle.contains("mg-issues"));
    }

    #[test]
    fn scenario_metrics_round_trip_and_format() {
        let metrics = ScenarioMetrics {
            served_pct: Some(98.46),
            pv_kw: Some(12.0),
            lcoe_usd_per_kwh: Some(f64::INFINITY),
            ..ScenarioMetrics::default()
        };
        let stored: std::collections::HashMap<&str, f64> = metrics
            .fields()
            .into_iter()
            .filter_map(|(key, value)| value.map(|v| (key, v)))
            .collect();
        let restored = ScenarioMetrics::from_fields(|key| stored.get(key).copied());
        assert_eq!(restored.served_pct, Some(98.46));
        assert_eq!(restored.pv_kw, Some(12.0));
        assert_eq!(restored.battery_kwh, None);

        let rows = metrics.rows();
        assert_eq!(rows[0], ("Annual served", "98.5%".to_string()));
        assert_eq!(rows[2], ("PV (kWp)", "12.0".to_string()));
        // Unknown and non-finite values show as a dash, never "inf".
        assert_eq!(rows[6].1, "\u{2014}");
        assert_eq!(rows[3].1, "\u{2014}");
    }

    #[test]
    fn scenarios_view_lists_each_scenario_with_actions() {
        let empty = inspect_tree(&scenarios_view(Vec::new()));
        assert!(empty.contains("No saved scenarios yet"));

        let scenarios = vec![
            Scenario {
                name: "Cabin".to_string(),
                values: vec!["5".to_string()],
                metrics: ScenarioMetrics {
                    pv_kw: Some(5.0),
                    ..ScenarioMetrics::default()
                },
            },
            Scenario {
                name: "Bigger".to_string(),
                values: vec!["8".to_string()],
                metrics: ScenarioMetrics::default(),
            },
        ];
        let tree = inspect_tree(&scenarios_view(scenarios));
        assert!(tree.contains("Cabin"));
        assert!(tree.contains("Bigger"));
        assert!(tree.contains("Button \"Load\""));
        assert!(tree.contains("Button \"Delete\""));
        assert!(tree.contains("PV (kWp)"));
        assert!(tree.contains("mg-cmp"));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn inverter_view_reports_coverage_and_clipping() {
        let report = |covers_peak: bool| InverterReport {
            dc_kwp: 5.0,
            dc_ac_ratio: 1.2,
            ac_rating_kw: 4.17,
            peak_demand_kw: if covers_peak { 2.0 } else { 6.0 },
            covers_peak,
            clipped_kwh_per_year: 120.0,
            clipped_fraction: 0.02,
        };
        let short = inspect_tree(&inverter_view(InverterOutcome::Solved(Rc::new(report(false)))));
        assert!(short.contains("exceeds the rating"));
        assert!(short.contains("mg-warn"));
        assert!(short.contains("2.0%"));

        let fine = inspect_tree(&inverter_view(InverterOutcome::Solved(Rc::new(report(true)))));
        assert!(fine.contains("covered by the rating"));
        assert!(!fine.contains("mg-warn"));

        let idle = inspect_tree(&inverter_view(InverterOutcome::Idle));
        assert!(idle.contains("Check inverter"));

        let invalid = inspect_tree(&inverter_view(InverterOutcome::Invalid(vec![
            "DC/AC ratio must be between 0.5 and 2.0.".to_string(),
        ])));
        assert!(invalid.contains("DC/AC ratio must be"));
        assert!(invalid.contains("mg-issues"));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn grid_view_reports_bills_saving_and_payback() {
        let year = GridYear {
            load_kwh: 8760.0,
            imported_kwh: 6000.0,
            exported_kwh: 800.0,
            unmet_kwh: 0.0,
            curtailed_kwh: 0.0,
            bill_without_system_usd: 2800.0,
            bill_with_system_usd: 1700.0,
        };
        let tree = inspect_tree(&grid_view(GridOutcome::Solved(Rc::new(year.clone()), 14000.0)));
        assert!(tree.contains("US$ 2800 without the system"));
        // 14,000 / (2,800 - 1,700) = 12.7 years.
        assert!(tree.contains("simple payback on US$ 14000 in 12.7 years"));
        assert!(!tree.contains("Load still unserved"));

        // An export credit larger than the bill reads as a credit, not "-".
        let credit = GridYear {
            bill_with_system_usd: -220.0,
            ..year.clone()
        };
        let tree = inspect_tree(&grid_view(GridOutcome::Solved(Rc::new(credit), 0.0)));
        assert!(tree.contains("a credit of US$ 220 with it"), "{tree}");
        assert!(!tree.contains("US$ -"));

        let idle = inspect_tree(&grid_view(GridOutcome::Idle));
        assert!(idle.contains("Price the year"));

        let invalid = inspect_tree(&grid_view(GridOutcome::Invalid(vec![
            "Import price must be between 0 and 5 $/kWh.".to_string(),
        ])));
        assert!(invalid.contains("Import price must be between"));
    }

    #[cfg(feature = "pro")]
    #[test]
    fn reliability_view_reports_lolp_and_autonomy() {
        let report = ReliabilityReport {
            lolp: 0.0123,
            loss_of_load_hours_per_year: 107.8,
            lole_days_per_year: 9.0,
            autonomy_days: MAX_AUTONOMY_DAYS,
        };
        let tree = inspect_tree(&reliability_view(ReliabilityOutcome::Solved(Rc::new(report))));
        assert!(tree.contains("1.23% of hours unserved"));
        assert!(tree.contains("capped at 60"));

        let idle = inspect_tree(&reliability_view(ReliabilityOutcome::Idle));
        assert!(idle.contains("Check reliability"));

        let invalid = inspect_tree(&reliability_view(ReliabilityOutcome::Invalid(vec![
            "Battery capacity must be between 0 and 10,000 kWh.".to_string(),
        ])));
        assert!(invalid.contains("Battery capacity"));
    }}
