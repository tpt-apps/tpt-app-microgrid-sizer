//! Export of results as CSV (for spreadsheets) and PDF (for sharing).
//!
//! The PDF writer is deliberately small: it lays out the report's Markdown
//! as text on A4 pages with the three standard Helvetica/Courier faces, so
//! the desktop and web builds need no PDF dependency. It handles headings,
//! bullet lines, and tables (set in monospace); it does not embed images or
//! custom fonts, and it writes uncompressed content streams.

use std::fmt::Write as _;
use std::io::Write as _;

use crate::{DayResult, HourPoint};
#[cfg(feature = "pro")]
use crate::{SeasonalResult, MONTH_NAMES};

/// Hourly balance of one day as CSV. Battery power is positive while
/// charging and negative while discharging.
pub fn day_csv(day: &DayResult) -> String {
    let mut out = String::from("hour,load_kw,solar_kw,battery_kw,soc,unmet_kw,curtailed_kw\n");
    for (hour, point) in day.hours.iter().enumerate() {
        write_day_row(&mut out, hour, point);
    }
    out
}

fn write_day_row(out: &mut String, hour: usize, point: &HourPoint) {
    let _ = writeln!(
        out,
        "{hour},{:.3},{:.3},{:.3},{:.4},{:.3},{:.3}",
        tidy(point.load_kw),
        tidy(point.solar_kw),
        tidy(point.battery_kw),
        tidy(point.soc),
        tidy(point.unmet_kw),
        tidy(point.curtailed_kw)
    );
}

/// Removes float dust so a value that is zero in practice prints as `0`
/// rather than `-0.000`.
fn tidy(value: f64) -> f64 {
    let rounded = (value * 1e6).round() / 1e6;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// Monthly energy totals of the seasonal run as CSV (kWh per month).
#[cfg(feature = "pro")]
pub fn seasonal_csv(result: &SeasonalResult) -> String {
    let mut out = String::from(
        "month,load_kwh,solar_kwh,battery_out_kwh,unmet_kwh,curtailed_kwh,generator_kwh\n",
    );
    for m in &result.months {
        let _ = writeln!(
            out,
            "{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
            MONTH_NAMES[m.month],
            tidy(m.load_kwh),
            tidy(m.solar_kwh),
            tidy(m.discharged_kwh),
            tidy(m.unmet_kwh),
            tidy(m.curtailed_kwh),
            tidy(m.generator_kwh)
        );
    }
    out
}

/// A4 page geometry in PDF points.
const PAGE_WIDTH: f32 = 595.0;
const PAGE_HEIGHT: f32 = 842.0;
const MARGIN: f32 = 50.0;

/// The three standard PDF faces the writer uses (resources `/F1`–`/F3`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Face {
    Regular,
    Bold,
    Mono,
}

impl Face {
    fn resource(self) -> &'static str {
        match self {
            Self::Regular => "F1",
            Self::Bold => "F2",
            Self::Mono => "F3",
        }
    }

    /// Average glyph advance as a fraction of the font size.
    fn advance(self) -> f32 {
        match self {
            Self::Mono => 0.6,
            Self::Regular | Self::Bold => 0.5,
        }
    }
}

/// One laid-out line of text. An empty `text` is a spacer.
struct Line {
    face: Face,
    size: f32,
    space_before: f32,
    text: String,
}

/// Renders report Markdown to PDF bytes (A4, standard fonts).
///
/// Headings become bold, table rows become monospace so columns stay
/// aligned, and everything else is wrapped to the margins. Emphasis markers
/// are removed; characters the standard encoding cannot show are dropped.
pub fn markdown_to_pdf(markdown: &str) -> Vec<u8> {
    let lines = layout(markdown);
    let pages = paginate(&lines);
    assemble(&lines, &pages)
}

/// Turns Markdown into lines with a face, size and wrap width.
fn layout(markdown: &str) -> Vec<Line> {
    let usable = PAGE_WIDTH - 2.0 * MARGIN;
    let mut lines = Vec::new();
    for raw in markdown.lines() {
        let stripped = raw.trim();
        if stripped.is_empty() {
            lines.push(Line {
                face: Face::Regular,
                size: 6.0,
                space_before: 0.0,
                text: String::new(),
            });
            continue;
        }
        let (face, size, space_before, body, indent) = if let Some(rest) = stripped.strip_prefix("# ") {
            (Face::Bold, 15.0, 10.0, rest, "")
        } else if let Some(rest) = stripped.strip_prefix("## ") {
            (Face::Bold, 12.0, 10.0, rest, "")
        } else if let Some(rest) = stripped.strip_prefix("### ") {
            (Face::Bold, 10.5, 6.0, rest, "")
        } else if stripped.starts_with('|') {
            (Face::Mono, 7.5, 0.0, stripped, "")
        } else if let Some(rest) = stripped.strip_prefix("- ") {
            (Face::Regular, 9.5, 2.0, rest, "  - ")
        } else {
            (Face::Regular, 9.5, 2.0, stripped, "")
        };
        let body = strip_emphasis(body);
        let width_chars = (usable / (size * face.advance())).floor() as usize;
        let width = width_chars.saturating_sub(indent.len()).max(10);
        let continuation = if indent.is_empty() { "" } else { "    " };
        for (i, piece) in wrap(&body, width).into_iter().enumerate() {
            let text = if i == 0 {
                format!("{indent}{piece}")
            } else {
                format!("{continuation}{piece}")
            };
            lines.push(Line {
                face,
                size,
                space_before: if i == 0 { space_before } else { 0.0 },
                text,
            });
        }
    }
    lines
}

/// Removes Markdown emphasis and code markers, keeping the words.
fn strip_emphasis(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

/// Greedy word wrap by character count; words longer than `width` are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.push(word.chars().take(width).collect());
            word = word.chars().skip(width).collect();
        }
        let joined = current.chars().count() + usize::from(!current.is_empty()) + word.chars().count();
        if joined > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&word);
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

/// Assigns each non-empty line a baseline on a page. Returns, per page, the
/// `(line index, baseline y)` pairs; there is always at least one page.
fn paginate(lines: &[Line]) -> Vec<Vec<(usize, f32)>> {
    let mut pages: Vec<Vec<(usize, f32)>> = vec![Vec::new()];
    let mut y = PAGE_HEIGHT - MARGIN;
    for (index, line) in lines.iter().enumerate() {
        let height = line.size * 1.4;
        y -= line.space_before;
        if y - height < MARGIN {
            pages.push(Vec::new());
            y = PAGE_HEIGHT - MARGIN;
        }
        y -= height;
        if !line.text.is_empty() {
            let baseline = y + height - line.size;
            if let Some(page) = pages.last_mut() {
                page.push((index, baseline));
            }
        }
    }
    pages
}

/// Encodes text in the PDF standard (WinAnsi) encoding inside a literal
/// string, escaping the delimiters. Characters outside the encoding are
/// dropped.
fn pdf_string(text: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in text.chars() {
        let byte = match c {
            '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => Some(c as u8),
            '\u{2013}' => Some(0x96),
            '\u{2014}' => Some(0x97),
            '\u{2018}' => Some(0x91),
            '\u{2019}' => Some(0x92),
            '\u{201c}' => Some(0x93),
            '\u{201d}' => Some(0x94),
            '\u{2026}' => Some(0x85),
            _ => None,
        };
        let Some(byte) = byte else { continue };
        if matches!(byte, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b')');
    out
}

/// Writes the content streams, page objects and cross-reference table.
/// Object numbers: 1 catalog, 2 page tree, 3–5 fonts, then each page object
/// followed by its content stream.
fn assemble(lines: &[Line], pages: &[Vec<(usize, f32)>]) -> Vec<u8> {
    let page_count = pages.len();
    let kids: String = (0..page_count)
        .map(|i| format!("{} 0 R", 6 + 2 * i))
        .collect::<Vec<_>>()
        .join(" ");

    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ];

    for (i, page) in pages.iter().enumerate() {
        let content_id = 7 + 2 * i;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH} {PAGE_HEIGHT}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> \
                 /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let mut content = Vec::new();
        for &(index, baseline) in page {
            let line = &lines[index];
            let _ = write!(
                content,
                "BT /{} {} Tf {MARGIN} {baseline} Td ",
                line.face.resource(),
                line.size
            );
            content.extend(pdf_string(&line.text));
            content.extend(b" Tj ET\n");
        }
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend(content);
        stream.extend(b"\nendstream");
        objects.push(stream);
    }

    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        let _ = write!(pdf, "{} 0 obj\n", i + 1);
        pdf.extend(body);
        pdf.extend(b"\nendobj\n");
    }
    let xref_at = pdf.len();
    let _ = write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in &offsets {
        let _ = write!(pdf, "{offset:010} 00000 n \n");
    }
    let _ = write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{simulate_day, BatterySpec, DayLoad, Site, SolarArray, HOURS_PER_DAY};

    fn sample_day() -> DayResult {
        simulate_day(
            &Site::default(),
            &SolarArray {
                cloud_factor: 0.0,
                ..SolarArray::default()
            },
            &BatterySpec::default(),
            &DayLoad {
                hourly_kw: vec![1.0; HOURS_PER_DAY],
            },
            0,
        )
    }

    #[test]
    fn day_csv_has_header_and_one_row_per_hour() {
        let csv = day_csv(&sample_day());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "hour,load_kw,solar_kw,battery_kw,soc,unmet_kw,curtailed_kw");
        assert_eq!(lines.len(), HOURS_PER_DAY + 1);
        for row in &lines[1..] {
            assert_eq!(row.split(',').count(), 7, "{row}");
        }
        // Hour 0 is night: the 1 kW load is shown in full.
        assert!(lines[1].starts_with("0,1.000,"), "{}", lines[1]);
    }

    #[cfg(feature = "pro")]
    #[test]
    fn seasonal_csv_has_twelve_month_rows() {
        let result = crate::simulate_seasonal(
            &Site::default(),
            &SolarArray::default(),
            &BatterySpec::default(),
            &DayLoad::default(),
            &crate::MonthlyFactors::default(),
        );
        let csv = seasonal_csv(&result);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 13);
        assert!(lines[1].starts_with("January,"));
        assert!(lines[12].starts_with("December,"));
        assert!(lines.iter().all(|l| l.split(',').count() == 7));
    }

    #[test]
    fn tidy_removes_negative_zero_only() {
        assert_eq!(tidy(-1e-13).to_bits(), 0.0f64.to_bits());
        assert_eq!(tidy(-0.0).to_bits(), 0.0f64.to_bits());
        assert!((tidy(1.2345678) - 1.234568).abs() < 1e-12);
        assert!((tidy(-2.5) + 2.5).abs() < 1e-12);
    }

    #[test]
    fn pdf_string_escapes_and_encodes() {
        assert_eq!(pdf_string("a (b) \\ c"), b"(a \\(b\\) \\\\ c)".to_vec());
        // Degree sign is in the standard encoding; the warning sign is not.
        assert_eq!(pdf_string("5\u{b0}\u{26a0}"), b"(5\xb0)".to_vec());
        assert_eq!(pdf_string("\u{2014}"), b"(\x97)".to_vec());
    }

    #[test]
    fn wrap_respects_width_and_splits_long_words() {
        for line in wrap("the quick brown fox jumps over the lazy dog again and again", 12) {
            assert!(line.chars().count() <= 12, "{line:?}");
        }
        assert_eq!(wrap("abcdefghijklmnopqrstuvwxyz", 10), vec![
            "abcdefghij",
            "klmnopqrst",
            "uvwxyz",
        ]);
    }

    #[test]
    fn pdf_has_valid_structure_and_xref() {
        let pdf = markdown_to_pdf("# Title\n\nA sentence.\n- a bullet\n| a | b |\n");
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        // The offset counts bytes, and the binary header makes the lossy
        // string longer than the file, so check it against the raw bytes.
        let trailer = String::from_utf8_lossy(&pdf[pdf.len() - 64..]).into_owned();
        let offset: usize = trailer
            .split("startxref\n")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .and_then(|n| n.trim().parse().ok())
            .expect("startxref offset");
        assert!(pdf[offset..].starts_with(b"xref"), "offset points at xref");
        assert!(text.contains("/Type /Page "));
        assert!(text.contains("(Title) Tj"));
    }

    #[test]
    fn long_documents_paginate() {
        let body: String = (0..300).map(|i| format!("Line number {i}.\n")).collect();
        let pdf = String::from_utf8_lossy(&markdown_to_pdf(&body)).into_owned();
        let count: usize = pdf
            .split("/Count ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("page count");
        assert!(count >= 2, "300 lines should not fit on one page");
    }
}
