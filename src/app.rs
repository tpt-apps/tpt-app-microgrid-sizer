//! Free WASM edition UI (tpt-appfront-dom).
//!
//! Rendering architecture (mirrors fea-lite/cutlist-optimizer's proven
//! split):
//! - the **input panel is mounted once** (`tpt_appfront_dom::mount`) —
//!   typing never loses focus; field values are read straight from the DOM
//!   when Simulate is pressed;
//! - the **results sections re-render fine-grained** via
//!   `tpt_appfront_dom::render` driven by outcome signals;
//! - the **charts are raw web-sys `<canvas>` elements** (the DOM backend
//!   has no canvas node kind) appended into placeholder containers and
//!   redrawn from effects that watch the outcome signals.
//!
//! Two DOM-backend quirks the markup works around (same lessons fea-lite
//! learned): a bare text node can't carry a `class`, so every styled piece
//! of text is wrapped in its own container; and the reconciler force-syncs
//! an input's DOM value to the UITree's static default on any re-render,
//! which is why all inputs live in the mounted-once shell and are only ever
//! written to directly.

use std::cell::Cell;
use std::rc::Rc;

use tpt_appfront_core::{create_effect, Signal};
use tpt_microgrid_engine::{
    chemistries, import_load_csv, presets, BatterySpec, DayLoad, Site, SolarArray, HOURS_PER_DAY,
    MONTHS_PER_YEAR,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::chart;
use crate::view::{
    day_chart, day_results_view, import_status_view, scenarios_view, shell_tree, DayOutcome,
    ImportOutcome, Msg, Scenario, ScenarioMetrics,
};
#[cfg(feature = "pro")]
use crate::view::{
    grid_view, inverter_view, rec_results_view, reliability_view, seasonal_chart,
    seasonal_results_view, GridOutcome, InverterOutcome, RecOutcome, ReliabilityOutcome,
    SeasonalOutcome,
};

use tpt_microgrid_engine::{
    appliance_day_load, day_csv, site_presets, suggested_utc_offset_at, validate_appliances,
    Appliance, Obstruction, Orientation,
};

/// Appliance rows in the builder.
const APPLIANCE_ROWS: usize = 6;
#[cfg(feature = "pro")]
use tpt_microgrid_engine::{
    cloud_factors_from_weather, design_report_markdown, import_weather_csv, inverter_check,
    grid_year, markdown_to_pdf, recommend_size, reliability_report, seasonal_csv,
    simulate_cloudy_spell, GridTariff, simulate_seasonal_with_generator, GeneratorSpec,
    MonthlyFactors, OptimizationInputs, OptimizationObjective, MONTH_NAMES,
};

const BASE_CSS: &str = r#"
.mg-app{--mg-bg:#f7f8fa;--mg-surface:#ffffff;--mg-panel:#f2f4f7;--mg-fg:#181d27;--mg-muted:#667085;--mg-border:#e3e7ee;--mg-accent:#0e7c6b;--mg-accent-2:#0a5f52;--mg-accent-soft:#e6f5f2;--mg-accent-fg:#ffffff;--mg-err:#b3261e;--mg-err-bg:#fbeeec;--mg-grid:rgba(20,30,45,.045);--mg-shadow:0 1px 2px rgba(16,24,40,.04),0 1px 3px rgba(16,24,40,.06);color-scheme:light;max-width:1360px;margin:0 auto;background:var(--mg-bg);color:var(--mg-fg);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;font-size:14.5px;line-height:1.55;padding:0 4px 32px}
@media(prefers-color-scheme:dark){.mg-app:not([data-theme="light"]){color-scheme:dark;--mg-bg:#0b0f16;--mg-surface:#141a24;--mg-panel:#161d2a;--mg-fg:#e8ebf0;--mg-muted:#98a2b3;--mg-border:#262f3d;--mg-accent:#34b39a;--mg-accent-2:#4fd6ba;--mg-accent-soft:#122824;--mg-accent-fg:#04140f;--mg-err:#f2b8b5;--mg-err-bg:#2c1a1c;--mg-grid:rgba(255,255,255,.045);--mg-shadow:0 1px 2px rgba(0,0,0,.3)}}
.mg-app[data-theme="dark"]{color-scheme:dark;--mg-bg:#0b0f16;--mg-surface:#141a24;--mg-panel:#161d2a;--mg-fg:#e8ebf0;--mg-muted:#98a2b3;--mg-border:#262f3d;--mg-accent:#34b39a;--mg-accent-2:#4fd6ba;--mg-accent-soft:#122824;--mg-accent-fg:#04140f;--mg-err:#f2b8b5;--mg-err-bg:#2c1a1c;--mg-grid:rgba(255,255,255,.045);--mg-shadow:0 1px 2px rgba(0,0,0,.3)}
.mg-theme-wrap{display:flex;align-items:center;gap:6px;margin-left:auto;width:max-content;font-size:.8rem}
.mg-theme-wrap .mg-label{margin:0}
select.mg-theme{width:auto;padding:5px 8px}
.mg-app *{box-sizing:border-box}
.mg-app h2{font-size:1.36rem;margin:0;font-weight:700;letter-spacing:-.01em}
.mg-app h3{font-size:1rem;margin:0 0 6px;font-weight:700}

.mg-header{display:flex;align-items:flex-start;gap:14px;padding:22px 2px 18px;border-bottom:1px solid var(--mg-border);margin-bottom:22px}
.mg-brand{flex:0 0 auto;width:42px;height:42px;border-radius:11px;background:linear-gradient(135deg,var(--mg-accent),var(--mg-accent-2));display:flex;align-items:center;justify-content:center;color:var(--mg-accent-fg);font-weight:800;font-size:.78rem;letter-spacing:.01em;box-shadow:var(--mg-shadow)}
.mg-title-group{flex:1;min-width:0}
.mg-title-row{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.mg-chip{display:inline-flex;align-items:center;padding:3px 10px;border-radius:999px;font-size:.66rem;font-weight:800;letter-spacing:.05em;text-transform:uppercase;background:var(--mg-accent-soft);color:var(--mg-accent)}
.mg-sub{color:var(--mg-muted);margin:5px 0 0;font-size:.88rem}

.mg-shell{display:grid;grid-template-columns:minmax(320px,380px) 1fr;gap:22px;align-items:start}
@media(max-width:900px){.mg-shell{grid-template-columns:minmax(0,1fr)}}
.mg-sidebar{display:flex;flex-direction:column;gap:16px;min-width:0;position:sticky;top:16px;max-height:calc(100vh - 32px);overflow-y:auto;padding:2px 4px 12px 2px;counter-reset:mg-step}
@media(max-width:900px){.mg-sidebar{position:static;max-height:none;overflow:visible}}
.mg-main{display:flex;flex-direction:column;gap:16px;min-width:0}

.mg-card{background:var(--mg-surface);border:1px solid var(--mg-border);border-radius:14px;padding:16px 18px;box-shadow:var(--mg-shadow)}
.mg-sidebar .mg-card h3{display:flex;align-items:center;gap:9px}
.mg-sidebar .mg-card h3::before{counter-increment:mg-step;content:counter(mg-step);flex:0 0 auto;width:20px;height:20px;border-radius:50%;background:var(--mg-accent);color:var(--mg-accent-fg);font-size:.68rem;display:flex;align-items:center;justify-content:center;font-weight:800}
.mg-hint{color:var(--mg-muted);font-size:.8rem;margin:0 0 10px}
.mg-row{display:flex;gap:12px;flex-wrap:wrap;margin-top:10px}
.mg-field{display:flex;flex-direction:column;gap:5px}
.mg-label{font-size:.72rem;color:var(--mg-muted);font-weight:600;text-transform:uppercase;letter-spacing:.03em}
.mg-input{width:110px;padding:7px 9px;border:1px solid var(--mg-border);border-radius:8px;background:var(--mg-bg);color:var(--mg-fg);font:inherit;transition:border-color .12s,box-shadow .12s}
select.mg-input{width:100%}
textarea.mg-input{width:100%;min-height:72px;font-family:ui-monospace,Consolas,monospace;font-size:.8rem;resize:vertical}
.mg-btn{padding:8px 14px;border-radius:9px;border:1px solid var(--mg-border);background:var(--mg-surface);color:var(--mg-fg);font:inherit;font-weight:600;cursor:pointer;transition:border-color .12s,background .12s}
.mg-btn:hover{border-color:var(--mg-accent)}
.mg-btn:focus-visible{outline:2px solid var(--mg-accent);outline-offset:2px}
.mg-btn.mg-primary{background:var(--mg-accent);border-color:var(--mg-accent);color:var(--mg-accent-fg)}
.mg-btn.mg-primary:hover{background:var(--mg-accent-2);border-color:var(--mg-accent-2)}
.mg-input:focus{outline:none;border-color:var(--mg-accent);box-shadow:0 0 0 3px var(--mg-accent-soft)}

.mg-hours{display:grid;grid-template-columns:repeat(6,1fr);gap:6px;margin-top:10px}
.mg-hour{display:flex;flex-direction:column;gap:2px;align-items:center}
.mg-hour-label{font-size:.62rem;color:var(--mg-muted)}
.mg-hours .mg-input{width:100%;padding:5px 4px;font-size:.8rem;text-align:center}
.mg-factors{display:grid;grid-template-columns:52px repeat(12,44px);gap:4px;margin-top:10px;align-items:center;overflow-x:auto;padding-bottom:4px;contain:inline-size;scrollbar-color:var(--mg-border) transparent}
.mg-factors .mg-input{width:100%;padding:5px 3px;font-size:.74rem;text-align:center}
.mg-factors-head{font-size:.66rem;color:var(--mg-muted);font-weight:700;text-align:center}

.mg-chartwrap{position:relative;border-radius:12px;overflow:hidden;border:1px solid var(--mg-border)}
.mg-chartwrap canvas{width:100%;height:auto;display:block;background-color:var(--mg-panel);background-image:linear-gradient(var(--mg-grid) 1px,transparent 1px),linear-gradient(90deg,var(--mg-grid) 1px,transparent 1px);background-size:24px 24px}
.mg-legend{display:flex;gap:14px;flex-wrap:wrap;margin-top:10px}
.mg-legend>div{display:inline-flex;align-items:center;gap:6px}
.mg-legend-label{font-size:.8rem;color:var(--mg-muted)}
.mg-swatch-fg::before{content:"";width:18px;height:0;border-top:3px solid #4a5568;border-radius:2px}
.mg-swatch-solar::before{content:"";width:18px;height:12px;background:rgba(224,151,42,.45);border-bottom:2px solid #e0972a}
.mg-swatch-soc::before{content:"";width:18px;height:0;border-top:3px dashed #2e9e6b}
.mg-swatch-unmet::before{content:"";width:12px;height:12px;background:rgba(179,38,30,.12);border:1px solid rgba(179,38,30,.4)}

.mg-results-view{margin:4px 0 0}
.mg-summary{margin:0 0 14px;padding:14px 16px;background:var(--mg-accent-soft);border-radius:10px}
.mg-headline{font-weight:700;font-size:1.05rem;margin:0 0 4px;color:var(--mg-fg)}
.mg-detail{color:var(--mg-muted);font-size:.83rem;margin:0}
.mg-stats{display:flex;gap:8px;flex-wrap:wrap;margin:0 0 14px}
.mg-stat{background:var(--mg-panel);border:1px solid var(--mg-border);border-radius:10px;padding:8px 12px;min-width:110px}
.mg-stat-value{display:block;font-size:1.02rem;font-variant-numeric:tabular-nums}
.mg-stat-label{font-size:.7rem;color:var(--mg-muted);text-transform:uppercase;letter-spacing:.03em;font-weight:600}
.mg-issues:empty{display:none}
.mg-issues{margin:0 0 12px;padding:10px 12px;background:var(--mg-err-bg);border:1px solid var(--mg-err);border-radius:10px}
.mg-issue{color:var(--mg-err);font-size:.88rem;margin:2px 0}
.mg-table{display:flex;flex-direction:column;font-size:.85rem;margin-top:4px}
.mg-thead,.mg-trow{display:grid;grid-template-columns:1.3fr repeat(5,1fr);gap:4px;padding:7px 8px;border-bottom:1px solid var(--mg-border)}
.mg-thead{color:var(--mg-muted);font-size:.7rem;font-weight:700;text-transform:uppercase;letter-spacing:.03em;border-bottom:2px solid var(--mg-border)}
.mg-trow:hover{background:var(--mg-panel)}
.mg-cell{text-align:left}
.mg-cellnum{text-align:right;font-variant-numeric:tabular-nums}
.mg-warn{color:var(--mg-err);font-weight:600}

.mg-upsell{border-radius:14px;padding:16px 18px;background:linear-gradient(135deg,var(--mg-accent-soft),transparent);border:1px dashed var(--mg-accent);color:var(--mg-fg);font-size:.88rem}
.mg-upsell h3{margin:0 0 6px}
.mg-upsell a{color:var(--mg-accent);font-weight:700;text-decoration:none}
.mg-upsell a:hover{text-decoration:underline}

.mg-footer{display:flex;flex-wrap:wrap;align-items:center;justify-content:space-between;gap:10px;margin-top:26px;padding-top:16px;border-top:1px solid var(--mg-border);color:var(--mg-muted);font-size:.82rem}
.mg-footer-brand{font-weight:700;color:var(--mg-fg)}
.mg-footer-links{display:flex;flex-wrap:wrap;gap:4px 16px}
.mg-footer-link{padding:0;border:none;background:none;color:var(--mg-accent);font:inherit;font-weight:600;cursor:pointer}
.mg-footer-link:hover{text-decoration:underline}

.mg-cmp{display:flex;flex-direction:column;overflow-x:auto;font-size:.85rem}
.mg-cmp-row{display:flex;gap:8px;align-items:flex-start;padding:7px 0;border-bottom:1px solid var(--mg-border);min-width:max-content}
.mg-cmp-row>*{flex:1 1 0;min-width:130px}
.mg-cmp-row>:first-child{flex:0 0 190px;min-width:0}
.mg-cmp-name{font-weight:700}
.mg-cmp-label{color:var(--mg-muted)}
.mg-cmp-cell{font-variant-numeric:tabular-nums}

.mg-chart,.mg-seasonal-chart{position:relative}
.mg-chart canvas,.mg-seasonal-chart canvas{display:block;width:100%;max-width:100%;border-radius:10px}
.mg-chart canvas:focus-visible,.mg-seasonal-chart canvas:focus-visible{outline:2px solid var(--mg-accent);outline-offset:2px}
.mg-tip{display:none;position:absolute;top:28px;transform:translateX(12px);pointer-events:none;background:var(--mg-surface);color:var(--mg-fg);border:1px solid var(--mg-border);border-radius:8px;padding:6px 9px;font-size:12px;line-height:1.4;box-shadow:var(--mg-shadow);white-space:pre-line;z-index:2}
.mg-tip.mg-tip-on{display:block}
.mg-tip.mg-tip-left{transform:translateX(calc(-100% - 12px))}
.mg-sr-only{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap;margin:-1px;padding:0;border:0}
"#;

/// Mounts the whole app into `container` (the hub's container div, or the
/// standalone `#tpt-appfront-root` marker).
pub fn mount_app(container: &web_sys::Element) -> Result<(), JsValue> {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    upsert_style(&document, "microgrid-sizer-styles", BASE_CSS);
    // `.mg-app` on the mount root itself: every color/font custom property
    // lives here, so the whole theme depends on this class being present on
    // an ancestor of the tree the CSS above targets.
    let existing_class = container.get_attribute("class").unwrap_or_default();
    if !existing_class.split_whitespace().any(|c| c == "mg-app") {
        let combined = if existing_class.is_empty() {
            "mg-app".to_string()
        } else {
            format!("{existing_class} mg-app")
        };
        let _ = container.set_attribute("class", &combined);
    }

    let day: Signal<DayOutcome> = Signal::new(DayOutcome::Idle);
    #[cfg(feature = "pro")]
    let seasonal: Signal<SeasonalOutcome> = Signal::new(SeasonalOutcome::Idle);
    #[cfg(feature = "pro")]
    let recommendation: Signal<RecOutcome> = Signal::new(RecOutcome::Idle);
    let import: Signal<ImportOutcome> = Signal::new(ImportOutcome::Idle);
    let appliance_status: Signal<ImportOutcome> = Signal::new(ImportOutcome::Idle);
    let site_status: Signal<ImportOutcome> = Signal::new(ImportOutcome::Idle);
    let scenarios: Signal<Vec<Scenario>> = Signal::new(load_scenarios());
    #[cfg(feature = "pro")]
    let inverter: Signal<InverterOutcome> = Signal::new(InverterOutcome::Idle);
    #[cfg(feature = "pro")]
    let reliability: Signal<ReliabilityOutcome> = Signal::new(ReliabilityOutcome::Idle);
    #[cfg(feature = "pro")]
    let grid: Signal<GridOutcome> = Signal::new(GridOutcome::Idle);

    let dispatch: Rc<dyn Fn(Msg)> = {
        let day = day.clone();
        #[cfg(feature = "pro")]
        let seasonal = seasonal.clone();
        #[cfg(feature = "pro")]
        let recommendation = recommendation.clone();
        let import = import.clone();
        let appliance_status = appliance_status.clone();
        let site_status = site_status.clone();
        let scenarios = scenarios.clone();
        #[cfg(feature = "pro")]
        let inverter = inverter.clone();
        #[cfg(feature = "pro")]
        let reliability = reliability.clone();
        #[cfg(feature = "pro")]
        let grid = grid.clone();
        let container = container.clone();
        Rc::new(move |msg| match msg {
            Msg::SimulateDay => run_simulate_day(&container, &day),
            Msg::Preset(index) => apply_preset(&container, index),
            Msg::Chemistry(index) => apply_chemistry(&container, index),
            Msg::ImportLoad => run_import_load(&container, &import),
            Msg::BuildLoad => run_build_load(&container, &appliance_status),
            Msg::SitePreset(index) => apply_site_preset(&container, index),
            Msg::UseLocation => run_use_location(&container, &site_status),
            Msg::Theme(index) => apply_theme(&container, index),
            #[cfg(feature = "pro")]
            Msg::ImportWeather => run_import_weather(&container, &import),
            #[cfg(feature = "pro")]
            Msg::CheckInverter => run_check_inverter(&container, &inverter),
            #[cfg(feature = "pro")]
            Msg::CheckReliability => run_check_reliability(&container, &reliability),
            #[cfg(feature = "pro")]
            Msg::CheckGrid => run_check_grid(&container, &grid),
            Msg::ExportDayCsv => export_day_csv(&day),
            #[cfg(feature = "pro")]
            Msg::ExportSeasonalCsv => export_seasonal_csv(&seasonal),
            #[cfg(feature = "pro")]
            Msg::ExportReportPdf => export_report_pdf(&container, &seasonal, &recommendation),
            #[cfg(feature = "pro")]
            Msg::SaveScenario => {
                let metrics = scenario_metrics(&seasonal, &recommendation);
                save_scenario(&container, &scenarios, metrics);
            }
            #[cfg(not(feature = "pro"))]
            Msg::SaveScenario => save_scenario(&container, &scenarios, ScenarioMetrics::default()),
            Msg::LoadScenario(index) => load_scenario(&container, &scenarios, index),
            Msg::DeleteScenario(index) => delete_scenario(&scenarios, index),
            Msg::VisitSite(url) => visit_site(&url),
            #[cfg(feature = "pro")]
            Msg::RunSeasonal => run_seasonal(&container, &seasonal),
            #[cfg(feature = "pro")]
            Msg::RunOptimize => run_optimize(&container, &recommendation),
            #[cfg(feature = "pro")]
            Msg::ApplyRecommendation => apply_recommendation(&container, &recommendation),
            #[cfg(feature = "pro")]
            Msg::ExportReport => export_report(&container, &seasonal, &recommendation),
        })
    };

    // 1. Static shell — mounted exactly once.
    let shell = shell_tree();
    let root = tpt_appfront_dom::mount(container, &shell, dispatch.clone())?;
    std::mem::forget(root);

    // 2. Reactive single-day results section into its placeholder.
    let results_el = container
        .query_selector(".mg-results")?
        .ok_or_else(|| JsValue::from_str("results placeholder missing"))?;
    let day_for_render = day.clone();
    let handle = tpt_appfront_dom::render(
        &results_el,
        Rc::new(move || day_results_view(day_for_render.get())),
        dispatch.clone(),
    )?;
    std::mem::forget(handle);

    // 2b. Import status line under the import inputs.
    let import_el = container
        .query_selector(".mg-import-status")?
        .ok_or_else(|| JsValue::from_str("import status placeholder missing"))?;
    let import_for_render = import.clone();
    let import_handle = tpt_appfront_dom::render(
        &import_el,
        Rc::new(move || import_status_view(import_for_render.get())),
        dispatch.clone(),
    )?;
    std::mem::forget(import_handle);

    // 2-site. Location status under the site buttons.
    let site_el = container
        .query_selector(".mg-site-status")?
        .ok_or_else(|| JsValue::from_str("site status placeholder missing"))?;
    let site_for_render = site_status.clone();
    let site_handle = tpt_appfront_dom::render(
        &site_el,
        Rc::new(move || import_status_view(site_for_render.get())),
        dispatch.clone(),
    )?;
    std::mem::forget(site_handle);

    // Restore the saved theme (a per-browser preference) before first paint
    // of the results.
    if let Some(saved) = local_storage().and_then(|s| s.get_item(THEME_KEY).ok().flatten()) {
        let index = match saved.as_str() {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        };
        apply_theme(container, index);
    }

    // 2a. Appliance builder status under its button.
    let appliance_el = container
        .query_selector(".mg-appliance-status")?
        .ok_or_else(|| JsValue::from_str("appliance status placeholder missing"))?;
    let appliance_for_render = appliance_status.clone();
    let appliance_handle = tpt_appfront_dom::render(
        &appliance_el,
        Rc::new(move || import_status_view(appliance_for_render.get())),
        dispatch.clone(),
    )?;
    std::mem::forget(appliance_handle);

    // 2c. Saved-scenario comparison table.
    let scenarios_el = container
        .query_selector(".mg-scenarios")?
        .ok_or_else(|| JsValue::from_str("scenario placeholder missing"))?;
    let scenarios_for_render = scenarios.clone();
    let scenarios_handle = tpt_appfront_dom::render(
        &scenarios_el,
        Rc::new(move || scenarios_view(scenarios_for_render.get())),
        dispatch.clone(),
    )?;
    std::mem::forget(scenarios_handle);

    // 2d. Inverter check result (Pro).
    #[cfg(feature = "pro")]
    {
        let inverter_el = container
            .query_selector(".mg-inverter")?
            .ok_or_else(|| JsValue::from_str("inverter placeholder missing"))?;
        let inverter_for_render = inverter.clone();
        let inverter_handle = tpt_appfront_dom::render(
            &inverter_el,
            Rc::new(move || inverter_view(inverter_for_render.get())),
            dispatch.clone(),
        )?;
        std::mem::forget(inverter_handle);

        let reliability_el = container
            .query_selector(".mg-reliability")?
            .ok_or_else(|| JsValue::from_str("reliability placeholder missing"))?;
        let reliability_for_render = reliability.clone();
        let reliability_handle = tpt_appfront_dom::render(
            &reliability_el,
            Rc::new(move || reliability_view(reliability_for_render.get())),
            dispatch.clone(),
        )?;
        std::mem::forget(reliability_handle);

        let grid_el = container
            .query_selector(".mg-grid")?
            .ok_or_else(|| JsValue::from_str("grid placeholder missing"))?;
        let grid_for_render = grid.clone();
        let grid_handle = tpt_appfront_dom::render(
            &grid_el,
            Rc::new(move || grid_view(grid_for_render.get())),
            dispatch.clone(),
        )?;
        std::mem::forget(grid_handle);
    }

    // 3. Single-day chart: canvas, hover tooltip, keyboard cursor and text
    //    alternative, repainted whenever the day outcome changes.
    let chart_holder = container
        .query_selector(".mg-chart")?
        .ok_or_else(|| JsValue::from_str("chart placeholder missing"))?;
    let day_for_chart = day.clone();
    mount_chart(
        &document,
        container,
        &chart_holder,
        "Hourly load, solar and battery charge for the chosen month.",
        Rc::new(move || match day_for_chart.get() {
            DayOutcome::Solved(result) => Some(day_chart(&result)),
            _ => None,
        }),
    )?;

    // 5. (pro) Seasonal chart + reactive results, and the reactive
    //    recommendation section.
    #[cfg(feature = "pro")]
    {
        let seasonal_chart_holder = container
            .query_selector(".mg-seasonal-chart")?
            .ok_or_else(|| JsValue::from_str("seasonal chart placeholder missing"))?;
        let seasonal_for_chart = seasonal.clone();
        mount_chart(
            &document,
            container,
            &seasonal_chart_holder,
            "Monthly solar, load and unmet energy for the year.",
            Rc::new(move || match seasonal_for_chart.get() {
                SeasonalOutcome::Solved(result, _) => Some(seasonal_chart(&result)),
                _ => None,
            }),
        )?;

        let seasonal_el = container
            .query_selector(".mg-seasonal-results")?
            .ok_or_else(|| JsValue::from_str("seasonal results placeholder missing"))?;
        let seasonal_for_render = seasonal.clone();
        let handle = tpt_appfront_dom::render(
            &seasonal_el,
            Rc::new(move || seasonal_results_view(seasonal_for_render.get())),
            dispatch.clone(),
        )?;
        std::mem::forget(handle);

        let rec_el = container
            .query_selector(".mg-rec-results")?
            .ok_or_else(|| JsValue::from_str("recommendation placeholder missing"))?;
        let rec_for_render = recommendation.clone();
        let handle = tpt_appfront_dom::render(
            &rec_el,
            Rc::new(move || rec_results_view(rec_for_render.get())),
            dispatch.clone(),
        )?;
        std::mem::forget(handle);
    }

    Ok(())
}

/// Sizes `canvas` to its container's width, and returns the CSS size the
/// chart is laid out at. The backing store is scaled by the device pixel
/// ratio, so lines stay sharp on high-density screens.
fn fit_canvas(canvas: &web_sys::HtmlCanvasElement) -> (f64, f64) {
    let measured = canvas.client_width();
    let css_w = if measured > 0 { f64::from(measured) } else { 960.0 };
    let css_h = (css_w * 420.0 / 960.0).clamp(220.0, 520.0);
    let dpr = web_sys::window()
        .map_or(1.0, |w| w.device_pixel_ratio())
        .max(1.0);
    let _ = canvas.style().set_property("height", &format!("{css_h}px"));
    canvas.set_width((css_w * dpr).round() as u32);
    canvas.set_height((css_h * dpr).round() as u32);
    (css_w, css_h)
}

/// Shows the tooltip for the cursor slot, placed over that slot and flipped
/// to the left of the pointer in the right-hand part of the plot. Hides it
/// when there is no cursor.
fn place_tip(
    tip: &web_sys::HtmlElement,
    chart: &chart::Chart,
    cursor: Option<usize>,
    css_w: f64,
    css_h: f64,
) {
    let lines = cursor
        .map(|index| chart::tooltip_lines(chart, index))
        .unwrap_or_default();
    let (Some(index), false) = (cursor, lines.is_empty()) else {
        tip.set_class_name("mg-tip");
        return;
    };
    let g = chart::geometry(css_w, css_h, chart.x_labels.len(), chart.right_title.is_some());
    let x = g.left + (index as f64 + 0.5) * g.slot_w;
    tip.set_text_content(Some(&lines.join("\n")));
    let _ = tip.style().set_property("left", &format!("{x}px"));
    let flip = x > css_w * 0.6;
    tip.set_class_name(if flip {
        "mg-tip mg-tip-on mg-tip-left"
    } else {
        "mg-tip mg-tip-on"
    });
}

/// Mounts one chart into `holder`: a canvas that follows its container's
/// width, a hover tooltip, a keyboard cursor (arrow keys, Home, End, Escape),
/// and a visually hidden list of the values for screen readers.
///
/// `source` gives the chart to draw, or `None` while there is nothing to show.
/// It is read inside an effect, so the chart repaints whenever its outcome
/// changes; window resizes repaint it too.
fn mount_chart(
    document: &web_sys::Document,
    container: &web_sys::Element,
    holder: &web_sys::Element,
    label: &str,
    source: Rc<dyn Fn() -> Option<chart::Chart>>,
) -> Result<(), JsValue> {
    let canvas: web_sys::HtmlCanvasElement = document
        .create_element("canvas")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("canvas cast failed"))?;
    canvas.set_attribute("role", "img")?;
    canvas.set_attribute("aria-label", label)?;
    canvas.set_attribute("tabindex", "0")?;
    holder.append_child(&canvas)?;

    let tip: web_sys::HtmlElement = document
        .create_element("div")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("tooltip cast failed"))?;
    tip.set_class_name("mg-tip");
    tip.set_attribute("aria-hidden", "true")?;
    holder.append_child(&tip)?;

    let values_list = document.create_element("ul")?;
    values_list.set_class_name("mg-sr-only");
    holder.append_child(&values_list)?;

    let cursor: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    // Paints the canvas and tooltip from the current chart and cursor.
    let repaint: Rc<dyn Fn()> = {
        let canvas = canvas.clone();
        let tip = tip.clone();
        let container = container.clone();
        let source = source.clone();
        let cursor = cursor.clone();
        Rc::new(move || {
            let Some(chart) = source() else {
                tip.set_class_name("mg-tip");
                return;
            };
            let (css_w, css_h) = fit_canvas(&canvas);
            let fg = read_css(&container, "--mg-fg", "#1f2430");
            let muted = read_css(&container, "--mg-muted", "#667085");
            let border = read_css(&container, "--mg-border", "#e3e7ee");
            let palette = chart::Palette {
                fg: &fg,
                muted: &muted,
                border: &border,
            };
            let _ = chart::draw(&canvas, &chart, &palette, (css_w, css_h), cursor.get());
            place_tip(&tip, &chart, cursor.get(), css_w, css_h);
        })
    };

    // Rebuilds the visually hidden value list (only when the data changes).
    let refresh_text: Rc<dyn Fn()> = {
        let document = document.clone();
        let values_list = values_list.clone();
        let source = source.clone();
        Rc::new(move || {
            values_list.set_text_content(None);
            let Some(chart) = source() else { return };
            for row in chart::accessible_rows(&chart) {
                if let Ok(item) = document.create_element("li") {
                    item.set_text_content(Some(&row));
                    let _ = values_list.append_child(&item);
                }
            }
        })
    };

    {
        let repaint = repaint.clone();
        let refresh_text = refresh_text.clone();
        let handle = create_effect(move || {
            refresh_text();
            repaint();
        });
        std::mem::forget(handle);
    }

    // Pointer: the slot under the mouse.
    {
        let canvas_ref = canvas.clone();
        let cursor = cursor.clone();
        let repaint = repaint.clone();
        let source = source.clone();
        let on_move = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
            let Some(chart) = source() else { return };
            let rect = canvas_ref.get_bounding_client_rect();
            let x = f64::from(ev.client_x()) - rect.left();
            let slot = chart::slot_at(
                rect.width(),
                rect.height(),
                chart.x_labels.len(),
                chart.right_title.is_some(),
                x,
            );
            if slot != cursor.get() {
                cursor.set(slot);
                repaint();
            }
        });
        canvas.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
        on_move.forget();
    }

    // Pointer leaves, or focus leaves: clear the cursor.
    for event in ["mouseleave", "blur"] {
        let cursor = cursor.clone();
        let repaint = repaint.clone();
        let on_leave = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
            if cursor.get().is_some() {
                cursor.set(None);
                repaint();
            }
        });
        canvas.add_event_listener_with_callback(event, on_leave.as_ref().unchecked_ref())?;
        on_leave.forget();
    }

    // Keyboard: arrows step the cursor, Home and End jump, Escape clears it.
    {
        let canvas_ref = canvas.clone();
        let cursor = cursor.clone();
        let repaint = repaint.clone();
        let source = source.clone();
        let on_key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let Some(chart) = source() else { return };
            let last = chart.x_labels.len().saturating_sub(1);
            let current = cursor.get();
            let next = match ev.key().as_str() {
                "ArrowRight" => Some(current.map_or(0, |i| (i + 1).min(last))),
                "ArrowLeft" => Some(current.map_or(last, |i| i.saturating_sub(1))),
                "Home" => Some(0),
                "End" => Some(last),
                "Escape" => None,
                _ => return,
            };
            ev.prevent_default();
            cursor.set(next);
            repaint();
        });
        canvas_ref.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())?;
        on_key.forget();
    }

    // Window resizes repaint at the new width.
    if let Some(window) = web_sys::window() {
        let repaint = repaint.clone();
        let on_resize = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| repaint());
        window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())?;
        on_resize.forget();
    }
    Ok(())
}

/// Opens a TPT Solutions URL from wherever the app happens to be running.
///
/// The free edition's wasm runs in a browser tab, where a normal new-tab
/// open is correct. The Pro edition's *identical* wasm bundle is instead
/// served over the desktop shell's `app://localhost` custom protocol —
/// there, `window.open`/navigation would replace the app inside the native
/// window with no way back, so the open is routed through the desktop
/// host's allowlisted IPC bridge. Runtime protocol sniffing (rather than
/// `#[cfg(feature = "pro")]`) is what distinguishes the two, since it's the
/// hosting context that matters, not the build.
fn visit_site(url: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let in_desktop_shell = window
        .location()
        .protocol()
        .map(|p| p == "app:")
        .unwrap_or(false);

    if in_desktop_shell {
        if let Ok(appfront) = js_sys::Reflect::get(&window, &JsValue::from_str("__appfront")) {
            if let Ok(post) = js_sys::Reflect::get(&appfront, &JsValue::from_str("post")) {
                if let Ok(post_fn) = post.dyn_into::<js_sys::Function>() {
                    let params = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &params,
                        &JsValue::from_str("url"),
                        &JsValue::from_str(url),
                    );
                    let _ = post_fn.call2(&appfront, &JsValue::from_str("open_external"), &params);
                }
            }
        }
    } else {
        let _ = window.open_with_url_and_target(url, "_blank");
    }
}

/// Downloads `text` as a file.
fn download_text(filename: &str, text: &str) -> Result<(), JsValue> {
    download_bytes(filename, text.as_bytes())
}

/// Downloads `bytes` as a file (text exports and the PDF report).
fn download_bytes(filename: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("anchor cast failed"))?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
    }
    anchor.click();
    let _ = anchor.remove();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// Reads the form, validates, simulates the single day, and publishes the
/// outcome.
fn run_simulate_day(container: &web_sys::Element, day: &Signal<DayOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let month = read_month(container);
    if !issues.is_empty() {
        day.set(DayOutcome::Invalid(issues));
        return;
    }
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(battery.validate());
    issues.extend(load.validate());
    if !issues.is_empty() {
        day.set(DayOutcome::Invalid(issues));
        return;
    }
    day.set(DayOutcome::Solved(Rc::new(tpt_microgrid_engine::simulate_day(
        &site, &array, &battery, &load, month,
    ))));
}

fn num(container: &web_sys::Element, selector: &str, name: &str, issues: &mut Vec<String>) -> f64 {
    let text = read_input(container, selector);
    match text.trim().parse::<f64>() {
        Ok(value) if value.is_finite() => value,
        _ => {
            issues.push(format!("{name} must be a number (got \u{201c}{text}\u{201d})."));
            f64::NAN
        }
    }
}

fn read_site(container: &web_sys::Element, issues: &mut Vec<String>) -> Site {
    Site {
        latitude_deg: num(container, ".mg-lat", "Latitude", issues),
        longitude_deg: num(container, ".mg-lon", "Longitude", issues),
        timezone_offset_hours: num(container, ".mg-tz", "UTC offset", issues),
        altitude_m: num(container, ".mg-alt", "Altitude", issues),
    }
}

fn read_array(container: &web_sys::Element, issues: &mut Vec<String>) -> SolarArray {
    let tilt_deg = num(container, ".mg-pv-tilt", "Tilt", issues);
    let azimuth_deg = num(container, ".mg-pv-az", "Azimuth", issues);
    SolarArray {
        capacity_kw: num(container, ".mg-pv-cap", "Solar capacity", issues),
        tilt_deg,
        azimuth_deg,
        cloud_factor: 1.0,
        ambient_celsius: 15.0,
        annual_degradation: num(container, ".mg-pv-deg", "PV degradation", issues) / 100.0,
        dc_ac_ratio: num(container, ".mg-pv-dcac", "DC/AC ratio", issues),
        orientations: read_orientations(container, tilt_deg, azimuth_deg, issues),
        obstructions: read_obstructions(container, issues),
    }
}

/// Reads the extra planes. Without any, the array is a single plane (empty
/// list). With extras, the main plane takes whatever percentage they leave.
fn read_orientations(
    container: &web_sys::Element,
    main_tilt: f64,
    main_azimuth: f64,
    issues: &mut Vec<String>,
) -> Vec<Orientation> {
    let mut extras = Vec::new();
    for n in 1..=2 {
        let share = num(container, &format!(".mg-or{n}-share"), &format!("Plane {n} share"), issues);
        if share > 0.0 {
            extras.push(Orientation {
                share,
                tilt_deg: num(container, &format!(".mg-or{n}-tilt"), &format!("Plane {n} tilt"), issues),
                azimuth_deg: num(container, &format!(".mg-or{n}-az"), &format!("Plane {n} azimuth"), issues),
            });
        }
    }
    if extras.is_empty() {
        return Vec::new();
    }
    let extra_total: f64 = extras.iter().map(|p| p.share).sum();
    if extra_total > 100.0 {
        issues.push("Extra planes cannot take more than 100% of the array.".to_string());
    }
    let mut planes = vec![Orientation {
        share: (100.0 - extra_total).max(0.0),
        tilt_deg: main_tilt,
        azimuth_deg: main_azimuth,
    }];
    planes.extend(extras);
    planes
}

/// Reads the shading windows. A window with zero loss is treated as absent,
/// so empty rows don't need to be cleared.
fn read_obstructions(container: &web_sys::Element, issues: &mut Vec<String>) -> Vec<Obstruction> {
    let mut windows = Vec::new();
    for n in 1..=2 {
        let loss = num(container, &format!(".mg-ob{n}-loss"), &format!("Shade {n} loss"), issues) / 100.0;
        if loss > 0.0 {
            let start = num(container, &format!(".mg-ob{n}-start"), &format!("Shade {n} start"), issues);
            let end = num(container, &format!(".mg-ob{n}-end"), &format!("Shade {n} end"), issues);
            windows.push(Obstruction {
                start_hour: whole_number(start, &format!("Shade {n} start hour"), issues),
                end_hour: whole_number(end, &format!("Shade {n} end hour"), issues),
                loss_fraction: loss,
            });
        }
    }
    windows
}

/// A whole-number input as a count. Anything else is reported and read as 0.
fn whole_number(value: f64, name: &str, issues: &mut Vec<String>) -> usize {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 {
        value as usize
    } else {
        issues.push(format!("{name} must be a whole number."));
        0
    }
}

/// The key the colour-theme preference is stored under in this browser.
const THEME_KEY: &str = "tpt-microgrid-theme";

/// Fills the site inputs from a built-in place (index 0 leaves them alone).
/// The altitude is left as the user set it.
fn apply_site_preset(container: &web_sys::Element, index: usize) {
    let Some(preset) = index.checked_sub(1).and_then(|i| site_presets().get(i)) else {
        return;
    };
    write_field(container, ".mg-lat", &format!("{:.4}", preset.latitude_deg));
    write_field(container, ".mg-lon", &format!("{:.4}", preset.longitude_deg));
    write_field(
        container,
        ".mg-tz",
        &format!("{}", preset.timezone_offset_hours),
    );
}

/// Fills the site inputs from the device's location. The browser asks the
/// user's permission first, and the position is used only to fill the inputs.
fn run_use_location(container: &web_sys::Element, status: &Signal<ImportOutcome>) {
    let geolocation = web_sys::window().and_then(|w| w.navigator().geolocation().ok());
    let Some(geolocation) = geolocation else {
        status.set(ImportOutcome::Failed(vec![
            "This browser does not offer location. Enter the coordinates by hand.".to_string(),
        ]));
        return;
    };
    status.set(ImportOutcome::Done("Asking the browser for your location…".to_string()));

    let on_found = {
        let container = container.clone();
        let status = status.clone();
        Closure::<dyn FnMut(web_sys::Position)>::new(move |position: web_sys::Position| {
            let coords = position.coords();
            let (lat, lon) = (coords.latitude(), coords.longitude());
            write_field(&container, ".mg-lat", &format!("{lat:.4}"));
            write_field(&container, ".mg-lon", &format!("{lon:.4}"));
            write_field(
                &container,
                ".mg-tz",
                &format!("{}", suggested_utc_offset_at(lat, lon)),
            );
            status.set(ImportOutcome::Done(format!(
                "Set to {lat:.4}, {lon:.4}. The UTC offset is a guess from the longitude: check it against your time zone."
            )));
        })
    };
    let on_error = {
        let status = status.clone();
        Closure::<dyn FnMut(web_sys::PositionError)>::new(move |error: web_sys::PositionError| {
            status.set(ImportOutcome::Failed(vec![format!(
                "Could not get your location ({}). Enter the coordinates by hand.",
                error.message()
            )]));
        })
    };
    let request = geolocation.get_current_position_with_error_callback(
        on_found.as_ref().unchecked_ref(),
        Some(on_error.as_ref().unchecked_ref()),
    );
    if request.is_err() {
        status.set(ImportOutcome::Failed(vec![
            "The browser refused the location request. Enter the coordinates by hand.".to_string(),
        ]));
    }
    // The browser calls these later, once; keep them alive until then.
    on_found.forget();
    on_error.forget();
}

/// Applies the colour theme: 0 follows the system, 1 is light, 2 is dark.
/// The choice is remembered in this browser.
fn apply_theme(container: &web_sys::Element, index: usize) {
    let stored = match index {
        1 => Some("light"),
        2 => Some("dark"),
        _ => None,
    };
    match stored {
        Some(theme) => {
            let _ = container.set_attribute("data-theme", theme);
        }
        None => {
            let _ = container.remove_attribute("data-theme");
        }
    }
    write_field(container, ".mg-theme", &index.to_string());
    if let Some(storage) = local_storage() {
        match stored {
            Some(theme) => {
                let _ = storage.set_item(THEME_KEY, theme);
            }
            None => {
                let _ = storage.remove_item(THEME_KEY);
            }
        }
    }
}

/// Builds the hourly load from the appliance rows and writes it into the 24
/// hourly inputs. The outcome (the daily total, or the reasons it was refused)
/// shows under the builder.
fn run_build_load(container: &web_sys::Element, status: &Signal<ImportOutcome>) {
    let mut issues = Vec::new();
    let mut appliances = Vec::new();
    for n in 1..=APPLIANCE_ROWS {
        let power_kw = num(container, &format!(".mg-ap{n}-kw"), &format!("Appliance {n} power"), &mut issues);
        let start = num(container, &format!(".mg-ap{n}-start"), &format!("Appliance {n} start hour"), &mut issues);
        let hours = num(container, &format!(".mg-ap{n}-hours"), &format!("Appliance {n} hours"), &mut issues);
        let start_hour = whole_number(start, &format!("Appliance {n} start hour"), &mut issues);
        let hours_per_day = whole_number(hours, &format!("Appliance {n} hours"), &mut issues);
        appliances.push(Appliance {
            power_kw,
            start_hour,
            hours_per_day,
        });
    }
    issues.extend(validate_appliances(&appliances));
    if !issues.is_empty() {
        status.set(ImportOutcome::Failed(issues));
        return;
    }
    let load = appliance_day_load(&appliances);
    for (h, kw) in load.hourly_kw.iter().enumerate() {
        write_field(container, &format!(".mg-hour-{h:02}"), &format!("{kw:.3}"));
    }
    let used = appliances
        .iter()
        .filter(|a| a.power_kw > 0.0 && a.hours_per_day > 0)
        .count();
    status.set(ImportOutcome::Done(format!(
        "Hourly load built from {used} appliance(s): {:.2} kWh a day.",
        load.daily_kwh()
    )));
}

fn read_battery(container: &web_sys::Element, issues: &mut Vec<String>) -> BatterySpec {
    BatterySpec {
        capacity_kwh: num(container, ".mg-bat-kwh", "Battery capacity", issues),
        power_kw: num(container, ".mg-bat-kw", "Battery power", issues),
        round_trip_efficiency: num(container, ".mg-bat-rte", "Round-trip efficiency", issues)
            / 100.0,
        min_soc: num(container, ".mg-bat-min", "Minimum SoC", issues) / 100.0,
        initial_soc: num(container, ".mg-bat-init", "Initial SoC", issues) / 100.0,
        annual_fade: num(container, ".mg-bat-fade", "Capacity fade", issues) / 100.0,
        self_discharge_per_day: num(container, ".mg-bat-sd", "Self-discharge", issues) / 100.0,
    }
}

fn read_load(container: &web_sys::Element, issues: &mut Vec<String>) -> DayLoad {
    let mut hourly = Vec::with_capacity(HOURS_PER_DAY);
    for h in 0..HOURS_PER_DAY {
        hourly.push(num(
            container,
            &format!(".mg-hour-{h:02}"),
            &format!("Hour {h:02} load"),
            issues,
        ));
    }
    DayLoad { hourly_kw: hourly }
}

fn read_month(container: &web_sys::Element) -> usize {
    read_input(container, ".mg-month")
        .trim()
        .parse::<usize>()
        .unwrap_or(0)
        % MONTHS_PER_YEAR
}

/// Fills the 24 hourly load inputs from a built-in preset.
fn apply_preset(container: &web_sys::Element, index: usize) {
    let all = presets::all();
    let values = all[index % all.len()].1;
    for (h, kw) in values.iter().enumerate() {
        write_field(container, &format!(".mg-hour-{h:02}"), &format!("{kw:.2}"));
    }
}

/// Fills the battery fields from a chemistry preset (index 0 = custom: no
/// change). Power follows the chemistry's kW/kWh ratio for the current
/// capacity, and the Pro optimizer's cost/life/ratio inputs follow too.
fn apply_chemistry(container: &web_sys::Element, index: usize) {
    let Some(chem) = index.checked_sub(1).and_then(|i| chemistries().get(i)) else {
        return;
    };
    write_field(container, ".mg-bat-rte", &format!("{:.0}", chem.round_trip_efficiency * 100.0));
    write_field(container, ".mg-bat-min", &format!("{:.0}", chem.min_soc * 100.0));
    write_field(container, ".mg-bat-fade", &format!("{}", chem.annual_fade * 100.0));
    write_field(container, ".mg-bat-sd", &format!("{}", chem.self_discharge_per_day * 100.0));
    if let Ok(kwh) = read_input(container, ".mg-bat-kwh").trim().parse::<f64>() {
        if kwh.is_finite() && kwh >= 0.0 {
            write_field(container, ".mg-bat-kw", &format!("{:.2}", kwh * chem.power_per_kwh));
        }
    }
    // No-ops in the free build, where these inputs don't exist.
    write_field(container, ".mg-opt-bat-cost", &format!("{:.0}", chem.cost_usd_per_kwh));
    write_field(container, ".mg-opt-ratio", &format!("{}", chem.power_per_kwh));
    write_field(container, ".mg-opt-bat-life", &format!("{:.0}", chem.life_years));
}

/// Parses the pasted load CSV into the hourly load inputs. A full year also
/// sets the monthly load factors (Pro shows them; the free build has no grid,
/// so those writes are no-ops there).
fn run_import_load(container: &web_sys::Element, outcome: &Signal<ImportOutcome>) {
    let text = read_textarea(container, ".mg-import-load");
    let import = match import_load_csv(&text) {
        Ok(import) => import,
        Err(err) => {
            outcome.set(ImportOutcome::Failed(vec![err.to_string()]));
            return;
        }
    };
    for (h, kw) in import.day.hourly_kw.iter().enumerate() {
        write_field(container, &format!(".mg-hour-{h:02}"), &format!("{kw:.3}"));
    }
    let message = match import.monthly_factors {
        Some(factors) => {
            for (m, factor) in factors.iter().enumerate() {
                write_field(container, &format!(".mg-loadf-{m:02}"), &format!("{factor:.3}"));
            }
            "Loaded a full year: the mean day fills the hourly load, and the monthly load factors follow the measured months."
        }
        None => "Loaded a 24-hour profile into the hourly load.",
    };
    outcome.set(ImportOutcome::Done(message.to_string()));
}

/// Calibrates the monthly cloud factors to a measured year of weather for
/// the site and array currently in the inputs.
#[cfg(feature = "pro")]
fn run_import_weather(container: &web_sys::Element, outcome: &Signal<ImportOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    if !issues.is_empty() {
        outcome.set(ImportOutcome::Failed(issues));
        return;
    }
    let text = read_textarea(container, ".mg-import-weather");
    let calibrated = import_weather_csv(&text)
        .and_then(|weather| cloud_factors_from_weather(&site, &array, &weather));
    match calibrated {
        Ok(cloud) => {
            for (m, factor) in cloud.iter().enumerate() {
                write_field(container, &format!(".mg-cloud-{m:02}"), &format!("{factor:.3}"));
            }
            outcome.set(ImportOutcome::Done(
                "Monthly cloud factors now match the measured yield for this site and array."
                    .to_string(),
            ));
        }
        Err(err) => outcome.set(ImportOutcome::Failed(vec![err.to_string()])),
    }
}

/// Reads a textarea's text (empty when the element is missing).
fn read_textarea(container: &web_sys::Element, selector: &str) -> String {
    container
        .query_selector(selector)
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        .map(|area| area.value())
        .unwrap_or_default()
}

/// Reads the 12 monthly factor columns from the sidebar grid.
#[cfg(feature = "pro")]
fn read_factors(container: &web_sys::Element, issues: &mut Vec<String>) -> MonthlyFactors {
    let mut factors = MonthlyFactors::flat();
    for m in 0..MONTHS_PER_YEAR {
        factors.load[m] = num(
            container,
            &format!(".mg-loadf-{m:02}"),
            &format!("{} load factor", MONTH_NAMES[m]),
            issues,
        );
        factors.cloud[m] = num(
            container,
            &format!(".mg-cloud-{m:02}"),
            &format!("{} cloud factor", MONTH_NAMES[m]),
            issues,
        );
        factors.ambient_c[m] = num(
            container,
            &format!(".mg-temp-{m:02}"),
            &format!("{} temperature", MONTH_NAMES[m]),
            issues,
        );
    }
    factors
}

#[cfg(feature = "pro")]
fn read_optimization(container: &web_sys::Element, issues: &mut Vec<String>) -> OptimizationInputs {
    OptimizationInputs {
        pv_cost_usd_per_kw: num(container, ".mg-opt-pv-cost", "PV cost", issues),
        battery_cost_usd_per_kwh: num(container, ".mg-opt-bat-cost", "Battery cost", issues),
        target_served_fraction: read_input(container, ".mg-opt-target")
            .trim()
            .parse()
            .unwrap_or(0.99),
        battery_power_ratio: num(container, ".mg-opt-ratio", "Battery power ratio", issues),
        discount_rate: num(container, ".mg-opt-discount", "Discount rate", issues) / 100.0,
        project_years: num(container, ".mg-opt-years", "Project life", issues),
        om_fraction_of_capex: num(container, ".mg-opt-om", "O&M", issues) / 100.0,
        battery_life_years: num(container, ".mg-opt-bat-life", "Battery life", issues),
        generator: read_generator(container, issues),
        objective: OptimizationObjective::from_id(&read_input(container, ".mg-opt-objective")),
    }
}

/// Reads the multi-day overcast spell length (days, clamped to 1-60).
#[cfg(feature = "pro")]
fn read_spell_days(container: &web_sys::Element, issues: &mut Vec<String>) -> usize {
    let days = num(container, ".mg-spell-days", "Overcast spell days", issues);
    if days.is_nan() {
        return 1;
    }
    (days.round() as i64).clamp(1, 60) as usize
}

/// Reads the optional backup generator (`None` when its power is 0).
#[cfg(feature = "pro")]
fn read_generator(container: &web_sys::Element, issues: &mut Vec<String>) -> Option<GeneratorSpec> {
    let power_kw = num(container, ".mg-gen-kw", "Generator power", issues);
    let fuel_cost = num(container, ".mg-gen-fuel", "Fuel cost", issues);
    if power_kw.is_nan() || power_kw <= 0.0 {
        return None;
    }
    Some(GeneratorSpec {
        power_kw,
        fuel_cost_usd_per_l: fuel_cost,
        ..GeneratorSpec::default()
    })
}

/// Runs the 12-month simulation and publishes the outcome.
#[cfg(feature = "pro")]
fn run_seasonal(container: &web_sys::Element, seasonal: &Signal<SeasonalOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    let generator = read_generator(container, &mut issues);
    let spell_days = read_spell_days(container, &mut issues);
    if !issues.is_empty() {
        seasonal.set(SeasonalOutcome::Invalid(issues));
        return;
    }
    if let Some(generator) = &generator {
        issues.extend(generator.validate());
    }
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(battery.validate());
    issues.extend(load.validate());
    issues.extend(factors.validate());
    if !issues.is_empty() {
        seasonal.set(SeasonalOutcome::Invalid(issues));
        return;
    }
    let result = simulate_seasonal_with_generator(
        &site,
        &array,
        &battery,
        &load,
        &factors,
        generator.as_ref(),
    );
    let spell = Rc::new(simulate_cloudy_spell(
        &site, &array, &battery, &load, &factors, &result, spell_days,
    ));
    seasonal.set(SeasonalOutcome::Solved(Rc::new(result), Some(spell)));
}

/// Runs the sizing search and publishes the outcome.
#[cfg(feature = "pro")]
fn run_optimize(container: &web_sys::Element, recommendation: &Signal<RecOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    let inputs = read_optimization(container, &mut issues);
    if !issues.is_empty() {
        recommendation.set(RecOutcome::Invalid(issues));
        return;
    }
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(battery.validate());
    issues.extend(load.validate());
    issues.extend(factors.validate());
    issues.extend(inputs.validate());
    if !issues.is_empty() {
        recommendation.set(RecOutcome::Invalid(issues));
        return;
    }
    recommendation.set(RecOutcome::Solved(Rc::new(recommend_size(
        &site, &array, &battery, &load, &factors, &inputs,
    ))));
}

/// Copies the recommendation's PV/battery sizes into the sidebar inputs.
#[cfg(feature = "pro")]
fn apply_recommendation(container: &web_sys::Element, recommendation: &Signal<RecOutcome>) {
    if let RecOutcome::Solved(rec) = recommendation.get() {
        write_field(container, ".mg-pv-cap", &format!("{:.2}", rec.solar_kw));
        write_field(container, ".mg-bat-kwh", &format!("{:.2}", rec.battery_kwh));
        write_field(container, ".mg-bat-kw", &format!("{:.2}", rec.battery_kw));
    }
}

/// Builds and downloads the Markdown design report from the last seasonal
/// run (a no-op until one has run, since the report documents it).
#[cfg(feature = "pro")]
/// Reads the grid tariff and connection limits from the Pro card.
#[cfg(feature = "pro")]
fn read_grid(container: &web_sys::Element, issues: &mut Vec<String>) -> GridTariff {
    GridTariff {
        import_usd_per_kwh: num(container, ".mg-grid-import", "Import price", issues),
        export_usd_per_kwh: num(container, ".mg-grid-export", "Export credit", issues),
        daily_charge_usd: num(container, ".mg-grid-daily", "Daily charge", issues),
        import_limit_kw: num(container, ".mg-grid-imlim", "Import limit", issues),
        export_limit_kw: num(container, ".mg-grid-exlim", "Export limit", issues),
    }
}

/// Prices the year with the grid connected, and the payback on the PV and
/// battery cost from the optimizer inputs.
#[cfg(feature = "pro")]
fn run_check_grid(container: &web_sys::Element, outcome: &Signal<GridOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    let tariff = read_grid(container, &mut issues);
    let costs = read_optimization(container, &mut issues);
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(battery.validate());
    issues.extend(load.validate());
    issues.extend(factors.validate());
    issues.extend(tariff.validate());
    if !issues.is_empty() {
        outcome.set(GridOutcome::Invalid(issues));
        return;
    }
    let year = grid_year(&site, &array, &battery, &load, &factors, &tariff);
    let capex = array.capacity_kw * costs.pv_cost_usd_per_kw
        + battery.capacity_kwh * costs.battery_cost_usd_per_kwh;
    outcome.set(GridOutcome::Solved(Rc::new(year), capex));
}

/// Computes loss of load and battery-only autonomy for the current inputs,
/// including any backup generator set in the optimizer inputs.
#[cfg(feature = "pro")]
fn run_check_reliability(container: &web_sys::Element, outcome: &Signal<ReliabilityOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    let generator = read_generator(container, &mut issues);
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(battery.validate());
    issues.extend(load.validate());
    issues.extend(factors.validate());
    if let Some(generator) = &generator {
        issues.extend(generator.validate());
    }
    if !issues.is_empty() {
        outcome.set(ReliabilityOutcome::Invalid(issues));
        return;
    }
    let report = reliability_report(
        &site,
        &array,
        &battery,
        &load,
        &factors,
        generator.as_ref(),
    );
    outcome.set(ReliabilityOutcome::Solved(Rc::new(report)));
}

/// Checks the inverter rating and clipping for the current inputs.
#[cfg(feature = "pro")]
fn run_check_inverter(container: &web_sys::Element, outcome: &Signal<InverterOutcome>) {
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    issues.extend(site.validate());
    issues.extend(array.validate());
    issues.extend(load.validate());
    issues.extend(factors.validate());
    if !issues.is_empty() {
        outcome.set(InverterOutcome::Invalid(issues));
        return;
    }
    let check = inverter_check(&site, &array, &load, &factors);
    outcome.set(InverterOutcome::Solved(Rc::new(check)));
}

/// The system-design report as Markdown, from the current inputs and the
/// last seasonal run. `None` when there is no seasonal run or an input is
/// invalid (the inputs' own messages are shown elsewhere).
#[cfg(feature = "pro")]
fn build_report(
    container: &web_sys::Element,
    seasonal: &Signal<SeasonalOutcome>,
    recommendation: &Signal<RecOutcome>,
) -> Option<String> {
    let SeasonalOutcome::Solved(result, spell) = seasonal.get() else {
        return None;
    };
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    if !issues.is_empty() {
        return None;
    }
    let rec = match recommendation.get() {
        RecOutcome::Solved(rec) => Some(rec),
        _ => None,
    };
    let cost_inputs = read_optimization(container, &mut issues);
    if !issues.is_empty() {
        return None;
    }
    let inverter = inverter_check(&site, &array, &load, &factors);
    let reliability = reliability_report(
        &site,
        &array,
        &battery,
        &load,
        &factors,
        cost_inputs.generator.as_ref(),
    );
    // The grid section only appears when the tariff inputs are valid.
    let mut grid_issues = Vec::new();
    let tariff = read_grid(container, &mut grid_issues);
    let grid = (grid_issues.is_empty() && tariff.validate().is_empty())
        .then(|| grid_year(&site, &array, &battery, &load, &factors, &tariff));
    Some(design_report_markdown(
        &site,
        &array,
        &battery,
        &load,
        &factors,
        &result,
        tpt_microgrid_engine::ReportExtras {
            recommendation: rec.as_deref(),
            cost_inputs: Some(&cost_inputs),
            spell: spell.as_deref(),
            inverter: Some(&inverter),
            reliability: Some(&reliability),
            grid: grid.as_ref(),
        },
    ))
}

/// Downloads the system-design report as Markdown.
#[cfg(feature = "pro")]
fn export_report(
    container: &web_sys::Element,
    seasonal: &Signal<SeasonalOutcome>,
    recommendation: &Signal<RecOutcome>,
) {
    if let Some(report) = build_report(container, seasonal, recommendation) {
        let _ = download_text("microgrid-design-report.md", &report);
    }
}

/// Downloads the same report as a PDF.
#[cfg(feature = "pro")]
fn export_report_pdf(
    container: &web_sys::Element,
    seasonal: &Signal<SeasonalOutcome>,
    recommendation: &Signal<RecOutcome>,
) {
    if let Some(report) = build_report(container, seasonal, recommendation) {
        let _ = download_bytes("microgrid-design-report.pdf", &markdown_to_pdf(&report));
    }
}

/// Downloads the single-day hourly balance as CSV.
fn export_day_csv(day: &Signal<DayOutcome>) {
    let DayOutcome::Solved(result) = day.get() else {
        return;
    };
    let _ = download_text("microgrid-day.csv", &day_csv(&result));
}

/// Downloads the seasonal monthly totals as CSV.
#[cfg(feature = "pro")]
fn export_seasonal_csv(seasonal: &Signal<SeasonalOutcome>) {
    let SeasonalOutcome::Solved(result, _) = seasonal.get() else {
        return;
    };
    let _ = download_text("microgrid-monthly.csv", &seasonal_csv(&result));
}

/// Metrics of the latest seasonal and optimizer runs, for a scenario.
#[cfg(feature = "pro")]
fn scenario_metrics(
    seasonal: &Signal<SeasonalOutcome>,
    recommendation: &Signal<RecOutcome>,
) -> ScenarioMetrics {
    let mut metrics = ScenarioMetrics::default();
    if let SeasonalOutcome::Solved(result, _) = seasonal.get() {
        metrics.served_pct = Some(result.served_fraction() * 100.0);
        metrics.unmet_kwh = Some(result.unmet_kwh);
    }
    if let RecOutcome::Solved(rec) = recommendation.get() {
        metrics.pv_kw = Some(rec.solar_kw);
        metrics.battery_kwh = Some(rec.battery_kwh);
        metrics.capex_usd = Some(rec.capex_usd);
        metrics.annual_cost_usd = Some(rec.annual_cost_usd);
        metrics.lcoe_usd_per_kwh = Some(rec.lcoe_usd_per_kwh);
    }
    metrics
}

// ---- Saved scenarios (kept in this browser's localStorage). ----

const SCENARIO_KEY: &str = "tpt-microgrid-scenarios";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Every input and select on the page, in document order. Textareas and the
/// scenario-name field are excluded: they are not design inputs.
fn input_elements(container: &web_sys::Element) -> Vec<web_sys::Element> {
    let Ok(nodes) =
        container.query_selector_all("input.mg-input:not(.mg-scen-name), select.mg-input")
    else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|i| nodes.item(i))
        .filter_map(|node| node.dyn_into::<web_sys::Element>().ok())
        .collect()
}

fn element_value(el: &web_sys::Element) -> String {
    if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
        input.value()
    } else if let Some(select) = el.dyn_ref::<web_sys::HtmlSelectElement>() {
        select.value()
    } else {
        String::new()
    }
}

fn set_element_value(el: &web_sys::Element, value: &str) {
    if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
        input.set_value(value);
    } else if let Some(select) = el.dyn_ref::<web_sys::HtmlSelectElement>() {
        select.set_value(value);
    }
}

/// Saves the current inputs under the typed name (or a numbered default).
fn save_scenario(
    container: &web_sys::Element,
    scenarios: &Signal<Vec<Scenario>>,
    metrics: ScenarioMetrics,
) {
    let mut list = scenarios.get();
    let typed = read_input(container, ".mg-scen-name");
    let name = if typed.trim().is_empty() {
        format!("Scenario {}", list.len() + 1)
    } else {
        typed.trim().to_string()
    };
    let values = input_elements(container).iter().map(element_value).collect();
    let scenario = Scenario {
        name: name.clone(),
        values,
        metrics,
    };
    // Saving under an existing name replaces that scenario in place, so the
    // comparison doesn't fill up with duplicate columns.
    match list.iter().position(|s| s.name == name) {
        Some(index) => list[index] = scenario,
        None => list.push(scenario),
    }
    persist_scenarios(&list);
    scenarios.set(list);
    write_field(container, ".mg-scen-name", "");
}

/// Restores a scenario's inputs. Inputs are matched by position, so only a
/// scenario saved by the same edition (same set of inputs) can be restored.
fn load_scenario(container: &web_sys::Element, scenarios: &Signal<Vec<Scenario>>, index: usize) {
    let Some(scenario) = scenarios.get().get(index).cloned() else {
        return;
    };
    let elements = input_elements(container);
    if elements.len() != scenario.values.len() {
        if let Some(window) = web_sys::window() {
            let _ = window.alert_with_message(
                "This scenario was saved in a different edition and cannot be loaded here.",
            );
        }
        return;
    }
    for (el, value) in elements.iter().zip(&scenario.values) {
        set_element_value(el, value);
    }
}

fn delete_scenario(scenarios: &Signal<Vec<Scenario>>, index: usize) {
    let mut list = scenarios.get();
    if index < list.len() {
        list.remove(index);
        persist_scenarios(&list);
        scenarios.set(list);
    }
}

/// Reads the stored scenarios; anything unreadable is treated as none.
fn load_scenarios() -> Vec<Scenario> {
    let Some(raw) = local_storage().and_then(|s| s.get_item(SCENARIO_KEY).ok().flatten()) else {
        return Vec::new();
    };
    let Ok(parsed) = js_sys::JSON::parse(&raw) else {
        return Vec::new();
    };
    let Ok(items) = parsed.dyn_into::<js_sys::Array>() else {
        return Vec::new();
    };
    items.iter().filter_map(|item| scenario_from_js(&item)).collect()
}

fn scenario_from_js(item: &JsValue) -> Option<Scenario> {
    let name = js_sys::Reflect::get(item, &JsValue::from_str("name"))
        .ok()?
        .as_string()?;
    let values = js_sys::Reflect::get(item, &JsValue::from_str("values"))
        .ok()?
        .dyn_into::<js_sys::Array>()
        .ok()?
        .iter()
        .filter_map(|v| v.as_string())
        .collect();
    let metrics = ScenarioMetrics::from_fields(|key| {
        js_sys::Reflect::get(item, &JsValue::from_str(key))
            .ok()
            .and_then(|v| v.as_f64())
    });
    Some(Scenario {
        name,
        values,
        metrics,
    })
}

/// Writes the scenario list to localStorage. Failure (private mode, blocked
/// storage) leaves the in-page list working for the session.
fn persist_scenarios(list: &[Scenario]) {
    let array = js_sys::Array::new();
    for scenario in list {
        let obj = js_sys::Object::new();
        set_js(&obj, "name", &JsValue::from_str(&scenario.name));
        let values = js_sys::Array::new();
        for value in &scenario.values {
            values.push(&JsValue::from_str(value));
        }
        set_js(&obj, "values", &values.into());
        for (key, value) in scenario.metrics.fields() {
            let stored = value.map_or(JsValue::NULL, JsValue::from_f64);
            set_js(&obj, key, &stored);
        }
        array.push(&obj.into());
    }
    let Ok(json) = js_sys::JSON::stringify(&array) else {
        return;
    };
    if let (Some(storage), Some(text)) = (local_storage(), json.as_string()) {
        let _ = storage.set_item(SCENARIO_KEY, &text);
    }
}

fn set_js(obj: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
}

// ---- DOM field helpers (the same proven implementations fea-lite uses). ----

fn read_input(container: &web_sys::Element, selector: &str) -> String {
    container
        .query_selector(selector)
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .or_else(|| {
            container
                .query_selector(selector)
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|select| select.value())
        })
        .unwrap_or_default()
}

fn write_field(container: &web_sys::Element, selector: &str, value: &str) {
    if let Some(el) = container.query_selector(selector).ok().flatten() {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            input.set_value(value);
        } else if let Some(select) = el.dyn_ref::<web_sys::HtmlSelectElement>() {
            select.set_value(value);
        }
    }
}

fn read_css(container: &web_sys::Element, name: &str, fallback: &str) -> String {
    let value = web_sys::window()
        .and_then(|w| w.get_computed_style(container).ok())
        .flatten()
        .and_then(|style| style.get_property_value(name).ok())
        .unwrap_or_default();
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn upsert_style(document: &web_sys::Document, id: &str, css: &str) {
    if let Ok(Some(existing)) = document.query_selector(&format!("style#{id}")) {
        existing.set_text_content(Some(css));
        return;
    }
    let Some(head) = document
        .query_selector("head")
        .ok()
        .flatten()
    else {
        return;
    };
    if let Ok(style_el) = document.create_element("style") {
        let _ = style_el.set_attribute("id", id);
        style_el.set_text_content(Some(css));
        let _ = head.append_child(&style_el);
    }
}
