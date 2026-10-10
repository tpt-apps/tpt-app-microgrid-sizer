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

use std::rc::Rc;

use tpt_appfront_core::{create_effect, Signal};
use tpt_microgrid_engine::{
    presets, BatterySpec, DayLoad, Site, SolarArray, HOURS_PER_DAY, MONTHS_PER_YEAR,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::chart;
use crate::view::{day_chart, day_results_view, shell_tree, DayOutcome, Msg};
#[cfg(feature = "pro")]
use crate::view::{
    rec_results_view, seasonal_chart, seasonal_results_view, RecOutcome, SeasonalOutcome,
};

#[cfg(feature = "pro")]
use tpt_microgrid_engine::{
    design_report_markdown, recommend_size, simulate_cloudy_spell,
    simulate_seasonal_with_generator, GeneratorSpec, MonthlyFactors, OptimizationInputs,
    OptimizationObjective, MONTH_NAMES,
};

const BASE_CSS: &str = r#"
.mg-app{--mg-bg:#f7f8fa;--mg-surface:#ffffff;--mg-panel:#f2f4f7;--mg-fg:#181d27;--mg-muted:#667085;--mg-border:#e3e7ee;--mg-accent:#0e7c6b;--mg-accent-2:#0a5f52;--mg-accent-soft:#e6f5f2;--mg-accent-fg:#ffffff;--mg-err:#b3261e;--mg-err-bg:#fbeeec;--mg-grid:rgba(20,30,45,.045);--mg-shadow:0 1px 2px rgba(16,24,40,.04),0 1px 3px rgba(16,24,40,.06);max-width:1360px;margin:0 auto;background:var(--mg-bg);color:var(--mg-fg);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;font-size:14.5px;line-height:1.55;padding:0 4px 32px}
@media(prefers-color-scheme:dark){.mg-app{--mg-bg:#0b0f16;--mg-surface:#141a24;--mg-panel:#161d2a;--mg-fg:#e8ebf0;--mg-muted:#98a2b3;--mg-border:#262f3d;--mg-accent:#34b39a;--mg-accent-2:#4fd6ba;--mg-accent-soft:#122824;--mg-accent-fg:#04140f;--mg-err:#f2b8b5;--mg-err-bg:#2c1a1c;--mg-grid:rgba(255,255,255,.045);--mg-shadow:0 1px 2px rgba(0,0,0,.3)}}
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
@media(max-width:900px){.mg-shell{grid-template-columns:1fr}}
.mg-sidebar{display:flex;flex-direction:column;gap:16px;position:sticky;top:16px;max-height:calc(100vh - 32px);overflow-y:auto;padding:2px 4px 12px 2px;counter-reset:mg-step}
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
.mg-input:focus{outline:none;border-color:var(--mg-accent);box-shadow:0 0 0 3px var(--mg-accent-soft)}

.mg-hours{display:grid;grid-template-columns:repeat(6,1fr);gap:6px;margin-top:10px}
.mg-hour{display:flex;flex-direction:column;gap:2px;align-items:center}
.mg-hour-label{font-size:.62rem;color:var(--mg-muted)}
.mg-hours .mg-input{width:100%;padding:5px 4px;font-size:.8rem;text-align:center}
.mg-factors{display:grid;grid-template-columns:52px repeat(12,1fr);gap:4px;margin-top:10px;align-items:center}
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

    let dispatch: Rc<dyn Fn(Msg)> = {
        let day = day.clone();
        #[cfg(feature = "pro")]
        let seasonal = seasonal.clone();
        #[cfg(feature = "pro")]
        let recommendation = recommendation.clone();
        let container = container.clone();
        Rc::new(move |msg| match msg {
            Msg::SimulateDay => run_simulate_day(&container, &day),
            Msg::Preset(index) => apply_preset(&container, index),
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

    // 3. Single-day chart (raw canvas; no UITree node kind).
    let chart_holder = container
        .query_selector(".mg-chart")?
        .ok_or_else(|| JsValue::from_str("chart placeholder missing"))?;
    let canvas: web_sys::HtmlCanvasElement = document
        .create_element("canvas")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("canvas cast failed"))?;
    canvas.set_width(960);
    canvas.set_height(420);
    chart_holder.append_child(&canvas)?;

    // 4. Redraw effect: outcome changes repaint the chart.
    {
        let canvas = canvas.clone();
        let container = container.clone();
        let day_effect = day.clone();
        let handle = create_effect(move || {
            if let DayOutcome::Solved(result) = day_effect.get() {
                let fg = read_css(&container, "--mg-fg", "#1f2430");
                let muted = read_css(&container, "--mg-muted", "#667085");
                let border = read_css(&container, "--mg-border", "#e3e7ee");
                let _ = chart::draw(&canvas, &day_chart(&result), &fg, &muted, &border);
            }
        });
        std::mem::forget(handle);
    }

    // 5. (pro) Seasonal chart + reactive results, and the reactive
    //    recommendation section.
    #[cfg(feature = "pro")]
    {
        let seasonal_chart_holder = container
            .query_selector(".mg-seasonal-chart")?
            .ok_or_else(|| JsValue::from_str("seasonal chart placeholder missing"))?;
        let seasonal_canvas: web_sys::HtmlCanvasElement = document
            .create_element("canvas")?
            .dyn_into()
            .map_err(|_| JsValue::from_str("canvas cast failed"))?;
        seasonal_canvas.set_width(960);
        seasonal_canvas.set_height(420);
        seasonal_chart_holder.append_child(&seasonal_canvas)?;

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

        let seasonal_for_draw = seasonal.clone();
        let container_for_draw = container.clone();
        let seasonal_canvas_for_draw = seasonal_canvas.clone();
        let handle = create_effect(move || {
            if let SeasonalOutcome::Solved(result, _) = seasonal_for_draw.get() {
                let fg = read_css(&container_for_draw, "--mg-fg", "#1f2430");
                let muted = read_css(&container_for_draw, "--mg-muted", "#667085");
                let border = read_css(&container_for_draw, "--mg-border", "#e3e7ee");
                let _ = chart::draw(
                    &seasonal_canvas_for_draw,
                    &seasonal_chart(&result),
                    &fg,
                    &muted,
                    &border,
                );
            }
        });
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

/// Downloads `text` as a file (used by the Pro report export).
#[cfg(feature = "pro")]
fn download_text(filename: &str, text: &str) -> Result<(), JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let parts = js_sys::Array::of1(&JsValue::from_str(text));
    let blob = web_sys::Blob::new_with_str_sequence(&parts.into())?;
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
    SolarArray {
        capacity_kw: num(container, ".mg-pv-cap", "Solar capacity", issues),
        tilt_deg: num(container, ".mg-pv-tilt", "Tilt", issues),
        azimuth_deg: num(container, ".mg-pv-az", "Azimuth", issues),
        cloud_factor: 1.0,
        ambient_celsius: 15.0,
    }
}

fn read_battery(container: &web_sys::Element, issues: &mut Vec<String>) -> BatterySpec {
    BatterySpec {
        capacity_kwh: num(container, ".mg-bat-kwh", "Battery capacity", issues),
        power_kw: num(container, ".mg-bat-kw", "Battery power", issues),
        round_trip_efficiency: num(container, ".mg-bat-rte", "Round-trip efficiency", issues)
            / 100.0,
        min_soc: num(container, ".mg-bat-min", "Minimum SoC", issues) / 100.0,
        initial_soc: num(container, ".mg-bat-init", "Initial SoC", issues) / 100.0,
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
fn export_report(
    container: &web_sys::Element,
    seasonal: &Signal<SeasonalOutcome>,
    recommendation: &Signal<RecOutcome>,
) {
    let SeasonalOutcome::Solved(result, spell) = seasonal.get() else {
        return;
    };
    let mut issues = Vec::new();
    let site = read_site(container, &mut issues);
    let array = read_array(container, &mut issues);
    let battery = read_battery(container, &mut issues);
    let load = read_load(container, &mut issues);
    let factors = read_factors(container, &mut issues);
    if !issues.is_empty() {
        return;
    }
    let rec = match recommendation.get() {
        RecOutcome::Solved(rec) => Some(rec),
        _ => None,
    };
    let cost_inputs = read_optimization(container, &mut issues);
    if !issues.is_empty() {
        return;
    }
    let report = design_report_markdown(
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
        },
    );
    let _ = download_text("microgrid-design-report.md", &report);
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
