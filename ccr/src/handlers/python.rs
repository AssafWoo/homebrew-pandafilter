use super::Handler;

pub struct PythonHandler;

impl Handler for PythonHandler {
    fn filter(&self, output: &str, _args: &[String]) -> String {
        // Detect Office / structured data formats before general filtering
        if looks_like_pptx(output) {
            return filter_pptx(output);
        }
        if looks_like_docx(output) {
            return filter_docx(output);
        }
        if looks_like_excel(output) {
            return filter_excel(output);
        }
        if is_tabular(output) {
            return filter_tabular(output);
        }

        let lines: Vec<&str> = output.lines().collect();

        if lines.len() <= 50 {
            return output.to_string();
        }

        // If there's a traceback, keep it + final error line; drop everything before
        if let Some(tb_pos) = output.find("Traceback (most recent call last):") {
            let tb_section = &output[tb_pos..];
            return tb_section.to_string();
        }

        // > 50 lines, no traceback: BERT summarization
        let result = panda_core::summarizer::summarize(output, 40);
        result.output
    }
}

// ── Detection helpers ─────────────────────────────────────────────────────────

fn looks_like_pptx(output: &str) -> bool {
    (output.contains(".pptx") || output.contains("python-pptx") || output.contains("prs.slides"))
        && (output.contains("Slide") || output.contains("slide"))
}

fn looks_like_docx(output: &str) -> bool {
    (output.contains(".docx") || output.contains("python-docx") || output.contains("Document("))
        && (output.contains("paragraph") || output.contains("Paragraph"))
}

fn looks_like_excel(output: &str) -> bool {
    output.contains("openpyxl")
        || output.contains("xlrd")
        || output.contains("xlwt")
        || (output.contains("Sheet") && output.contains("workbook"))
        || output.contains("load_workbook")
}

/// Detect consistent-delimiter tabular output (CSV-ish or pandas DataFrame repr).
fn is_tabular(output: &str) -> bool {
    let lines: Vec<&str> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();
    if lines.len() < 3 {
        return false;
    }
    // Pandas DataFrame: lines with multiple pipe characters in consistent positions
    let pipe_lines = lines.iter().filter(|l| l.chars().filter(|&c| c == '|').count() >= 2).count();
    if pipe_lines >= lines.len() / 2 {
        return true;
    }
    // CSV-like: consistent comma or tab counts across 3+ lines
    let comma_counts: Vec<usize> = lines.iter().map(|l| l.chars().filter(|&c| c == ',').count()).collect();
    let tab_counts:   Vec<usize> = lines.iter().map(|l| l.chars().filter(|&c| c == '\t').count()).collect();
    let consistent = |counts: &[usize]| {
        if counts[0] == 0 { return false; }
        counts.iter().all(|&c| c == counts[0])
    };
    consistent(&comma_counts) || consistent(&tab_counts)
}

// ── Filter functions ──────────────────────────────────────────────────────────

fn filter_tabular(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return output.to_string();
    }

    // Count columns from first line
    let header = lines[0];
    let delimiter = if header.contains('\t') { '\t' } else { ',' };
    let col_count = header.chars().filter(|&c| c == delimiter).count() + 1;

    let total_rows = lines.len().saturating_sub(1); // exclude header
    let shown_rows = total_rows.min(10);

    let mut out: Vec<String> = Vec::new();
    for line in &lines[..shown_rows + 1] {
        // Truncate wide lines
        let chars: Vec<char> = line.chars().collect();
        if chars.len() > 120 {
            out.push(format!("{}…", chars[..119].iter().collect::<String>()));
        } else {
            out.push(line.to_string());
        }
    }

    if total_rows > 10 {
        out.push(format!("[{} total rows × {} cols]", total_rows, col_count));
    }

    out.join("\n")
}

fn filter_docx(output: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }
        // Drop lines that look like style/formatting metadata
        if is_docx_metadata(t) { continue; }
        paragraphs.push(t.to_string());
    }

    let total = paragraphs.len();
    let shown = total.min(50);
    let mut out: Vec<String> = paragraphs[..shown].to_vec();

    if total > 50 {
        out.push(format!("[+{} more paragraphs]", total - 50));
    }
    out.push(format!("[{} paragraphs total]", total));

    out.join("\n")
}

fn is_docx_metadata(line: &str) -> bool {
    // Drop lines that are pure style/XML metadata
    line.starts_with("style=")
        || line.starts_with("font=")
        || line.starts_with("bold=")
        || line.starts_with("italic=")
        || line.starts_with("size=")
        || line.starts_with("<w:")
        || line.starts_with("runs=[")
        || (line.starts_with("Paragraph(") && line.ends_with(")"))
}

fn filter_excel(output: &str) -> String {
    let mut sheet_info: Vec<String> = Vec::new();
    let mut data_rows: Vec<String> = Vec::new();
    let mut in_data = false;
    let mut total_rows = 0usize;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }

        // Sheet/workbook metadata
        if t.starts_with("Sheet") || t.starts_with("Worksheet") || t.contains("sheetnames") {
            sheet_info.push(t.to_string());
            continue;
        }

        // Row data
        if t.starts_with("Row") || t.starts_with("(") || in_data {
            in_data = true;
            total_rows += 1;
            if data_rows.len() < 10 {
                let truncated = if t.len() > 120 { format!("{}…", &t[..119]) } else { t.to_string() };
                data_rows.push(truncated);
            }
        } else {
            sheet_info.push(t.to_string());
        }
    }

    let mut out = sheet_info;
    out.extend(data_rows);
    if total_rows > 10 {
        out.push(format!("[{} total rows]", total_rows));
    }
    if out.is_empty() { output.to_string() } else { out.join("\n") }
}

fn filter_pptx(output: &str) -> String {
    let mut slides: Vec<String> = Vec::new();
    let mut current_slide: Option<String> = None;
    let mut bullets: Vec<String> = Vec::new();
    let mut slide_count = 0usize;

    let flush = |slide: &Option<String>, bullets: &Vec<String>, slides: &mut Vec<String>| {
        if let Some(title) = slide {
            slides.push(title.clone());
            for b in bullets.iter().take(10) {
                slides.push(format!("  • {}", b));
            }
        }
    };

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }

        // Slide header detection
        if t.starts_with("Slide ") || t.contains("slide_number") {
            flush(&current_slide, &bullets, &mut slides);
            bullets.clear();
            slide_count += 1;
            if slide_count <= 20 {
                current_slide = Some(t.to_string());
            }
            continue;
        }

        // Drop shape/image/formatting metadata
        if is_pptx_metadata(t) { continue; }

        if current_slide.is_some() && slide_count <= 20 {
            bullets.push(t.to_string());
        }
    }
    flush(&current_slide, &bullets, &mut slides);

    if slide_count > 20 {
        slides.push(format!("[+{} more slides]", slide_count - 20));
    }
    slides.push(format!("[{} slides total]", slide_count));

    if slides.is_empty() { output.to_string() } else { slides.join("\n") }
}

fn is_pptx_metadata(line: &str) -> bool {
    line.starts_with("Shape(")
        || line.starts_with("Picture(")
        || line.starts_with("fill=")
        || line.starts_with("theme_color=")
        || line.starts_with("<p:sp")
        || line.starts_with("placeholder")
        || line.contains("EMU")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    #[test]
    fn short_output_passes_through() {
        let output = "hello world\n";
        assert_eq!(PythonHandler.filter(output, &[]), output);
    }

    #[test]
    fn traceback_kept_prefix_dropped() {
        let prefix = "Loading data...\n".repeat(60);
        let tb = "Traceback (most recent call last):\n  File \"x.py\", line 1\nValueError: bad\n";
        let input = format!("{}{}", prefix, tb);
        let result = PythonHandler.filter(&input, &[]);
        assert!(result.contains("Traceback"));
        assert!(!result.contains("Loading data"));
    }

    #[test]
    fn tabular_csv_detection() {
        let csv = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCarol,35,Chicago\n";
        assert!(is_tabular(csv));
    }

    #[test]
    fn tabular_csv_caps_at_10_rows() {
        let mut lines = vec!["a,b,c".to_string()];
        for i in 0..20 { lines.push(format!("{},{},{}", i, i+1, i+2)); }
        let input = lines.join("\n");
        let result = filter_tabular(&input);
        assert!(result.contains("[20 total rows × 3 cols]"));
        let data_lines: Vec<&str> = result.lines().filter(|l| !l.contains("total rows")).collect();
        assert_eq!(data_lines.len(), 11, "header + 10 data rows");
    }

    #[test]
    fn docx_strips_metadata_keeps_text() {
        let output = "\
Document('report.docx')
Paragraph(style='Heading 1')
style=Normal
Introduction
bold=True
This is the first paragraph of the document.
runs=[Run('Introduction')]
Conclusion
";
        let result = filter_docx(output);
        assert!(result.contains("Introduction"));
        assert!(result.contains("first paragraph"));
        assert!(!result.contains("style=Normal"));
        assert!(!result.contains("bold=True"));
    }

    #[test]
    fn pptx_emits_slide_headers_and_bullets() {
        let output = "\
Slide 1: Title Slide
Welcome to the Presentation
Shape(type=RECTANGLE, fill=blue)
Slide 2: Agenda
Item 1
Item 2
Item 3
Picture(width=5000000 EMU)
";
        let result = filter_pptx(output);
        assert!(result.contains("Slide 1"));
        assert!(result.contains("Slide 2"));
        assert!(result.contains("Item 1"));
        assert!(!result.contains("RECTANGLE"));
        assert!(!result.contains("EMU"));
    }
}
