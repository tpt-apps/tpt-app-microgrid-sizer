//! The view layer: message type, outcome enums, the mounted-once shell, the
//! reactive result sections, and the chart data builders.
//!
//! Everything here is pure `UITree` construction over engine types — no DOM,
//! no wasm — so the whole UI structure compiles and unit-tests on the host
//! (`cargo test`) as well as in the wasm build. DOM glue lives in `app`.

use std::rc::Rc;

use tpt_appfront_core::UITree;
use tpt_microgrid_engine::{presets, DayResult, HOURS_PER_DAY, MONTH_NAMES};
#[cfg(feature = "pro")]
use tpt_microgrid_engine::MONTHS_PER_YEAR;
#[cfg(feature = "pro")]
use tpt_microgrid_engine::{MonthlyFactors, SeasonalResult, SizingRecommendation, SpellResult};

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
    pub const BAT_KWH: &str = "10";
    pub const BAT_KW: &str = "5";
    pub const BAT_RTE: &str = "95";
    pub const BAT_MIN: &str = "10";
    pub const BAT_INIT: &str = "50";
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
                    card.text("Azimuth from north: 0 = north-facing (southern-hemisphere sites), 180 = south-facing. Weather: the free edition simulates a clear day; the Pro edition varies cloud month by month.")
                        .class("mg-hint");
                })
                .class("mg-card");

                sidebar.container(|card| {
                    card.heading(3, "Battery");
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
                            .button("Export design report (.md)")
                            .class("mg-btn")
                            .on_click(Msg::ExportReport);
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
                color: "#e0972a",
                fill_color: Some("rgba(224,151,42,0.20)"),
                values: result.hours.iter().map(|h| h.solar_kw).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                color: "#4a5568",
                fill_color: None,
                values: result.hours.iter().map(|h| h.load_kw).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
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

#[cfg(feature = "pro")]
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
                color: "#e0972a",
                fill_color: Some("rgba(224,151,42,0.20)"),
                values: result.months.iter().map(|m| m.solar_kwh).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
                color: "#4a5568",
                fill_color: None,
                values: result.months.iter().map(|m| m.load_kwh).collect(),
                dashed: false,
                right_axis: false,
            },
            chart::ChartSeries {
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
            "mg-pv-az", "mg-bat-kwh", "mg-bat-kw", "mg-bat-rte", "mg-bat-min", "mg-bat-init",
            "mg-preset",
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
        assert!(invalid.contains("class=\""mg-issues\"\""));

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
    }}