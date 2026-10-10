//! Canvas time-series chart, drawn directly through web-sys (the DOM
//! backend has no canvas node kind — same approach as fea-lite's viewer).
//!
//! The one chart type the sizer needs: N x-slots (24 hours or 12 months),
//! any number of line/fill series against a shared left axis, one optional
//! right axis (battery SoC %), and optional highlighted slots (unmet
//! hours) drawn as translucent bands behind the data.
//!
//! The canvas is sized by the app to its container (see `app::fit_canvas`):
//! [draw] takes the CSS size and scales for the device pixel ratio, so the
//! chart stays sharp at any width. The hover tooltip, the keyboard cursor and
//! the screen-reader text are pure functions of the [Chart], so they test on
//! the host.
#![cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]

use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

/// One plotted series against the left or right axis.
pub struct ChartSeries {
    /// Series name, used in tooltips and the text alternative.
    pub name: &'static str,
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

/// Theme colors read from the page's CSS custom properties.
pub struct Palette<'a> {
    /// Foreground (baseline and cursor).
    pub fg: &'a str,
    /// Muted text (tick labels).
    pub muted: &'a str,
    /// Gridlines.
    pub border: &'a str,
}

/// Plot-area rectangle in CSS pixels, inside the axes' margins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Left edge of the plot.
    pub left: f64,
    /// Top edge of the plot.
    pub top: f64,
    /// Plot width.
    pub plot_w: f64,
    /// Plot height.
    pub plot_h: f64,
    /// Width of one x-slot.
    pub slot_w: f64,
}

/// The plot rectangle for a `width` × `height` chart with `slots` x-slots.
pub fn geometry(width: f64, height: f64, slots: usize, has_right: bool) -> Geometry {
    let margin_left = 56.0;
    let margin_right = if has_right { 56.0 } else { 18.0 };
    let margin_top = 20.0;
    let margin_bottom = 30.0;
    let plot_w = (width - margin_left - margin_right).max(10.0);
    let plot_h = (height - margin_top - margin_bottom).max(10.0);
    Geometry {
        left: margin_left,
        top: margin_top,
        plot_w,
        plot_h,
        slot_w: plot_w / slots.max(1) as f64,
    }
}

/// The x-slot under horizontal position `x` (CSS pixels, measured from the
/// canvas's left edge), or `None` outside the plot.
pub fn slot_at(width: f64, height: f64, slots: usize, has_right: bool, x: f64) -> Option<usize> {
    let g = geometry(width, height, slots, has_right);
    let offset = x - g.left;
    if offset < 0.0 || offset >= g.plot_w || slots == 0 {
        return None;
    }
    Some(((offset / g.slot_w) as usize).min(slots - 1))
}

/// Paints `chart` onto `canvas`. `size` is the CSS size the chart is laid out
/// in; the canvas's own pixel size may be larger (device pixel ratio). The
/// `cursor` slot, when set, gets a guide line and point markers.
pub fn draw(
    canvas: &HtmlCanvasElement,
    chart: &Chart,
    palette: &Palette<'_>,
    size: (f64, f64),
    cursor: Option<usize>,
) -> Result<(), wasm_bindgen::JsValue> {
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|_| wasm_bindgen::JsValue::from_str("context cast failed"))?;

    let (width, height) = size;
    if width <= 0.0 || height <= 0.0 {
        return Ok(());
    }
    let scale = canvas.width() as f64 / width;
    context.set_transform(scale, 0.0, 0.0, scale, 0.0, 0.0)?;
    context.clear_rect(0.0, 0.0, width, height);

    let slots = chart.x_labels.len().max(1);
    let has_right = chart.right_title.is_some();
    let g = geometry(width, height, slots, has_right);
    let right_max = chart.right_max.max(0.001);
    let left_max = nice_max(
        chart
            .series
            .iter()
            .filter(|s| !s.right_axis)
            .flat_map(|s| s.values.iter().copied())
            .fold(1.0f64, f64::max),
    );
    let bottom = g.top + g.plot_h;
    let right_edge = g.left + g.plot_w;

    context.set_font("11px system-ui, sans-serif");
    let y_left = |v: f64| g.top + g.plot_h * (1.0 - (v / left_max).clamp(0.0, 1.0));
    let y_right = |v: f64| g.top + g.plot_h * (1.0 - (v / right_max).clamp(0.0, 1.0));
    let x_center = |i: usize| g.left + (i as f64 + 0.5) * g.slot_w;

    // Highlight bands behind everything.
    for (i, &flag) in chart.highlight.iter().enumerate() {
        if flag {
            context.set_fill_style_str("rgba(179,38,30,0.10)");
            let _ = context.fill_rect(g.left + i as f64 * g.slot_w, g.top, g.slot_w, g.plot_h);
        }
    }

    // Gridlines + tick labels (4 divisions).
    for quarter in 0..=4 {
        let fraction = quarter as f64 / 4.0;
        let y = g.top + g.plot_h * (1.0 - fraction);
        context.set_stroke_style_str(palette.border);
        context.set_line_width(1.0);
        let _ = context.begin_path();
        let _ = context.move_to(g.left, y);
        let _ = context.line_to(right_edge, y);
        let _ = context.stroke();

        context.set_fill_style_str(palette.muted);
        let left_value = left_max * fraction;
        let _ = context.fill_text_with_max_width(
            &format_number(left_value),
            8.0,
            y + 4.0,
            g.left - 12.0,
        );
        if let Some(title) = &chart.right_title {
            let right_value = right_max * fraction;
            let label = format!("{:.0} {}", right_value, title);
            let _ = context.fill_text_with_max_width(&label, right_edge + 8.0, y + 4.0, 46.0);
        }
    }

    // Axis unit labels sit above their scales.
    context.set_fill_style_str(palette.muted);
    let _ = context.fill_text(&chart.left_title, 8.0, 12.0);

    // X tick labels — at most ~12 of them.
    let tick_every = slots.div_ceil(12);
    for (i, label) in chart.x_labels.iter().enumerate() {
        if i % tick_every != 0 {
            continue;
        }
        let metrics_w = 60.0f64.min(g.slot_w * tick_every as f64);
        let x = (x_center(i) - metrics_w / 2.0).max(g.left - 10.0);
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
            let _ = context.move_to(points[0].0, bottom);
            for (x, py) in &points {
                let _ = context.line_to(*x, *py);
            }
            let _ = context.line_to(points[points.len() - 1].0, bottom);
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

    // Keyboard and pointer cursor: a guide line through the slot, with a
    // marker on each series' value there.
    if let Some(index) = cursor.filter(|&i| i < slots) {
        let x = x_center(index);
        context.set_stroke_style_str(palette.fg);
        context.set_line_width(1.0);
        let _ = context.begin_path();
        let _ = context.move_to(x, g.top);
        let _ = context.line_to(x, bottom);
        let _ = context.stroke();
        for series in &chart.series {
            let v = series.values.get(index).copied().unwrap_or(0.0);
            let py = if series.right_axis { y_right(v) } else { y_left(v) };
            context.set_fill_style_str(series.color);
            let _ = context.begin_path();
            let _ = context.arc(x, py, 3.5, 0.0, std::f64::consts::TAU);
            let _ = context.fill();
        }
    }

    // Baseline on top of the fills, in the foreground color.
    context.set_stroke_style_str(palette.fg);
    context.set_line_width(1.0);
    let _ = context.begin_path();
    let _ = context.move_to(g.left, bottom);
    let _ = context.line_to(right_edge, bottom);
    let _ = context.stroke();

    Ok(())
}

/// The tooltip for slot `index`: the slot's label, then one line per series
/// with its value and unit. Empty when the slot is out of range.
pub fn tooltip_lines(chart: &Chart, index: usize) -> Vec<String> {
    let Some(label) = chart.x_labels.get(index) else {
        return Vec::new();
    };
    let mut lines = vec![label.clone()];
    for series in &chart.series {
        let value = series.values.get(index).copied().unwrap_or(0.0);
        let unit = if series.right_axis {
            chart.right_title.as_deref().unwrap_or("")
        } else {
            chart.left_title.as_str()
        };
        lines.push(format!("{}: {:.2} {}", series.name, value, unit).trim_end().to_string());
    }
    lines
}

/// One screen-reader line per slot, so the chart's values are readable
/// without the picture. Each line is the slot's tooltip joined on one line.
pub fn accessible_rows(chart: &Chart) -> Vec<String> {
    (0..chart.x_labels.len())
        .map(|i| {
            let lines = tooltip_lines(chart, i);
            match lines.split_first() {
                Some((label, values)) => format!("{label}: {}", values.join(", ")),
                None => String::new(),
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Chart {
        Chart {
            series: vec![
                ChartSeries {
                    name: "Solar",
                    color: "#e0972a",
                    fill_color: None,
                    values: vec![0.0, 2.5, 4.0],
                    dashed: false,
                    right_axis: false,
                },
                ChartSeries {
                    name: "Battery",
                    color: "#2e9e6b",
                    fill_color: None,
                    values: vec![10.0, 50.0, 90.0],
                    dashed: true,
                    right_axis: true,
                },
            ],
            x_labels: vec!["00:00".into(), "01:00".into(), "02:00".into()],
            left_title: "kW".into(),
            right_title: Some("%".into()),
            right_max: 100.0,
            highlight: vec![false; 3],
        }
    }

    #[test]
    fn nice_max_rounds_up_to_a_four_division_grid() {
        // Exactly on a step stays put.
        assert_eq!(nice_max(4.0), 4.0);
        assert_eq!(nice_max(0.4), 0.4);
        // Anything between steps rounds up to four divisions of the next
        // 1/2/2.5/5 x 10^k step.
        assert_eq!(nice_max(3.7), 4.0);
        assert_eq!(nice_max(0.41), 0.8);
        assert_eq!(nice_max(6.0), 8.0);
        assert_eq!(nice_max(1.1), 2.0);
        assert_eq!(nice_max(1234.0), 2000.0);
        // Always at least the requested value.
        for v in [0.01, 0.9, 3.0, 7.7, 55.0, 999.0] {
            assert!(nice_max(v) >= v, "nice_max({v}) too small");
        }
    }

    #[test]
    fn format_number_keeps_ticks_readable() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(format_number(-2.5), "-2.5");
        assert_eq!(format_number(9.94), "9.9");
        // >= 10 drops to whole numbers so 12.5 doesn't crowd the axis.
        assert_eq!(format_number(12.0), "12");
        assert_eq!(format_number(1000.0), "1000");
    }

    #[test]
    fn slot_at_maps_the_plot_and_rejects_the_margins() {
        // 800 px wide, 3 slots, right axis: plot starts at 56 and spans 688.
        let g = geometry(800.0, 400.0, 3, true);
        assert_eq!(g.left, 56.0);
        assert!((g.plot_w - 688.0).abs() < 1e-9);
        assert_eq!(slot_at(800.0, 400.0, 3, true, 56.0), Some(0));
        assert_eq!(slot_at(800.0, 400.0, 3, true, 56.0 + g.slot_w * 1.5), Some(1));
        assert_eq!(slot_at(800.0, 400.0, 3, true, 55.9), None);
        assert_eq!(slot_at(800.0, 400.0, 3, true, 800.0), None);
        // The last pixel still lands in the last slot.
        assert_eq!(slot_at(800.0, 400.0, 3, true, 56.0 + 688.0 - 0.1), Some(2));
    }

    #[test]
    fn geometry_scales_with_the_width() {
        let narrow = geometry(400.0, 300.0, 24, false);
        let wide = geometry(1000.0, 300.0, 24, false);
        assert!(wide.slot_w > narrow.slot_w);
        assert!((wide.plot_w - (1000.0 - 56.0 - 18.0)).abs() < 1e-9);
    }

    #[test]
    fn tooltip_lists_each_series_with_its_unit() {
        let lines = tooltip_lines(&sample(), 1);
        assert_eq!(lines[0], "01:00");
        assert_eq!(lines[1], "Solar: 2.50 kW");
        assert_eq!(lines[2], "Battery: 50.00 %");
        assert!(tooltip_lines(&sample(), 9).is_empty());
    }

    #[test]
    fn accessible_rows_cover_every_slot() {
        let rows = accessible_rows(&sample());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], "02:00: Solar: 4.00 kW, Battery: 90.00 %");
    }
}
