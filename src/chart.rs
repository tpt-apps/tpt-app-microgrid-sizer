//! Canvas time-series chart, drawn directly through web-sys (the DOM
//! backend has no canvas node kind — same approach as fea-lite's viewer).
//!
//! The one chart type the sizer needs: N x-slots (24 hours or 12 months),
//! any number of line/fill series against a shared left axis, one optional
//! right axis (battery SoC %), and optional highlighted slots (unmet
//! hours) drawn as translucent bands behind the data.

use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

/// One plotted series against the left or right axis.
pub struct ChartSeries {
    /// Stroke color (any CSS color).
    pub color: &'static str,
    /// Fill color for an area series (`None` = line only).
    pub fill_color: Option<&'static str>,
    /// One value per x-slot.
    pub values: Vec<f64>,
    /// Draw with a dashed stroke.
    pub dashed: bool,
    /// Plot against the right axis instead of the left.
    pub right_axis: bool,
}

/// Everything needed to paint one chart.
pub struct Chart {
    /// Series in paint order (first painted first, behind the rest).
    pub series: Vec<ChartSeries>,
    /// One label per x-slot; sparse ticks are picked automatically.
    pub x_labels: Vec<String>,
    /// Left-axis unit label (e.g. "kW").
    pub left_title: String,
    /// Right-axis unit label (`None` = no right axis).
    pub right_title: Option<String>,
    /// Right-axis top of scale (e.g. 100 for percent).
    pub right_max: f64,
    /// Slots flagged here get a translucent band behind the data.
    pub highlight: Vec<bool>,
}

/// Paints `chart` onto `canvas`.
pub fn draw(
    canvas: &HtmlCanvasElement,
    chart: &Chart,
    fg: &str,
    muted: &str,
    border: &str,
) -> Result<(), wasm_bindgen::JsValue> {
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|_| wasm_bindgen::JsValue::from_str("context cast failed"))?;

    let width = canvas.width() as f64;
    let height = canvas.height() as f64;
    let slots = chart.x_labels.len().max(1);
    let has_right = chart.right_title.is_some();
    let margin_left = 56.0;
    let margin_right = if has_right { 56.0 } else { 18.0 };
    let margin_top = 20.0;
    let margin_bottom = 30.0;
    let plot_w = (width - margin_left - margin_right).max(10.0);
    let plot_h = (height - margin_top - margin_bottom).max(10.0);
    let slot_w = plot_w / slots as f64;

    let left_max = chart
        .series
        .iter()
        .filter(|s| !s.right_axis)
        .flat_map(|s| s.values.iter().copied())
        .fold(1.0f64, f64::max);
    let left_max = nice_max(left_max);
    let right_max = chart.right_max.max(0.001);

    context.clear_rect(0.0, 0.0, width, height);
    context.set_font("11px system-ui, sans-serif");

    let y_left = |v: f64| margin_top + plot_h * (1.0 - (v / left_max).clamp(0.0, 1.0));
    let y_right = |v: f64| margin_top + plot_h * (1.0 - (v / right_max).clamp(0.0, 1.0));
    let x_center = |i: usize| margin_left + (i as f64 + 0.5) * slot_w;

    // Highlight bands behind everything.
    for (i, &flag) in chart.highlight.iter().enumerate() {
        if flag {
            context.set_fill_style_str("rgba(179,38,30,0.10)");
            let _ = context.fill_rect(margin_left + i as f64 * slot_w, margin_top, slot_w, plot_h);
        }
    }

    // Gridlines + tick labels (4 divisions).
    for quarter in 0..=4 {
        let fraction = quarter as f64 / 4.0;
        let y = margin_top + plot_h * (1.0 - fraction);
        context.set_stroke_style_str(border);
        context.set_line_width(1.0);
        let _ = context.begin_path();
        let _ = context.move_to(margin_left, y);
        let _ = context.line_to(width - margin_right, y);
        let _ = context.stroke();

        context.set_fill_style_str(muted);
        let left_value = left_max * fraction;
        let _ = context.fill_text_with_max_width(
            &format_number(left_value),
            8.0,
            y + 4.0,
            margin_left - 12.0,
        );
        if let Some(title) = &chart.right_title {
            let right_value = right_max * fraction;
            let label = format!("{:.0} {}", right_value, title);
            let _ = context.fill_text_with_max_width(
                &label,
                width - margin_right + 8.0,
                y + 4.0,
                margin_right - 10.0,
            );
        }
    }

    // Axis unit labels sit above their scales.
    context.set_fill_style_str(muted);
    let _ = context.fill_text(&chart.left_title, 8.0, 12.0);

    // X tick labels — at most ~12 of them.
    let tick_every = (slots + 11) / 12;
    for (i, label) in chart.x_labels.iter().enumerate() {
        if i % tick_every != 0 {
            continue;
        }
        let metrics_w = 60.0f64.min(slot_w * tick_every as f64);
        let x = (x_center(i) - metrics_w / 2.0).max(margin_left - 10.0);
        let _ = context.fill_text_with_max_width(label, x, height - 10.0, metrics_w + 6.0);
    }

    // Series.
    for series in &chart.series {
        let y = |v: f64| if series.right_axis { y_right(v) } else { y_left(v) };
        let points: Vec<(f64, f64)> = (0..slots)
            .map(|i| {
                let v = series.values.get(i).copied().unwrap_or(0.0);
                (x_center(i), y(v))
            })
            .collect();

        if let Some(fill) = series.fill_color {
            let _ = context.begin_path();
            let _ = context.move_to(points[0].0, margin_top + plot_h);
            for (x, py) in &points {
                let _ = context.line_to(*x, *py);
            }
            let _ = context.line_to(points[points.len() - 1].0, margin_top + plot_h);
            let _ = context.close_path();
            context.set_fill_style_str(fill);
            let _ = context.fill();
        }

        let _ = context.begin_path();
        for (i, (x, py)) in points.iter().enumerate() {
            if i == 0 {
                let _ = context.move_to(*x, *py);
            } else {
                let _ = context.line_to(*x, *py);
            }
        }
        context.set_stroke_style_str(series.color);
        context.set_line_width(2.0);
        if series.dashed {
            let dash = js_sys::Array::of2(&6.0.into(), &4.0.into());
            let _ = context.set_line_dash(&dash.into());
        }
        let _ = context.stroke();
        if series.dashed {
            let _ = context.set_line_dash(&js_sys::Array::new().into());
        }
    }

    // Baseline on top of the fills, in the foreground color.
    context.set_stroke_style_str(fg);
    context.set_line_width(1.0);
    let _ = context.begin_path();
    let _ = context.move_to(margin_left, margin_top + plot_h);
    let _ = context.line_to(width - margin_right, margin_top + plot_h);
    let _ = context.stroke();

    Ok(())
}

/// Rounds a max value up to a 1/2/2.5/5 × 10^k grid with four divisions.
fn nice_max(value: f64) -> f64 {
    let rough = (value / 4.0).max(1e-9);
    let magnitude = 10f64.powf(rough.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .find(|&&candidate| candidate * magnitude >= rough)
        .unwrap_or(&10.0)
        * magnitude;
    step * 4.0
}

/// Integer-ish tick formatting: enough decimals to distinguish ticks at the
/// chart's scale.
fn format_number(value: f64) -> String {
    if value.abs() >= 10.0 {
        format!("{:.0}", value)
    } else {
        let trimmed = format!("{:.1}", value);
        trimmed.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
