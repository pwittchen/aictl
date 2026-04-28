use std::fmt::Write as _;

use crate::config::MAX_TOOL_OUTPUT_LEN;

use super::util::truncate_output;

pub(super) async fn tool_read_document(input: &str) -> String {
    let path = input.trim();
    if path.is_empty() {
        return "Error: no file path provided".to_string();
    }

    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    match ext.as_str() {
        "pdf" => {
            let path = path.to_string();
            tokio::task::spawn_blocking(move || read_pdf(&path))
                .await
                .unwrap_or_else(|e| format!("Error reading PDF: {e}"))
        }
        "docx" => {
            let path = path.to_string();
            tokio::task::spawn_blocking(move || read_docx(&path))
                .await
                .unwrap_or_else(|e| format!("Error reading DOCX: {e}"))
        }
        "xlsx" | "xls" | "ods" => {
            let path = path.to_string();
            tokio::task::spawn_blocking(move || read_spreadsheet(&path))
                .await
                .unwrap_or_else(|e| format!("Error reading spreadsheet: {e}"))
        }
        _ => format!(
            "Error: unsupported document format '.{ext}'. Supported: .pdf, .docx, .xlsx, .xls, .ods"
        ),
    }
}

fn read_pdf(path: &str) -> String {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return format!("Error reading file: {e}"),
    };
    match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return "(PDF contains no extractable text — it may be image-based)".to_string();
            }
            let mut result = trimmed.to_string();
            truncate_output(&mut result);
            result
        }
        Err(e) => format!("Error extracting text from PDF: {e}"),
    }
}

fn read_docx(path: &str) -> String {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return format!("Error reading file: {e}"),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return format!("Error reading DOCX archive: {e}"),
    };
    let Ok(mut xml) = archive.by_name("word/document.xml") else {
        return "Error: not a valid DOCX file (missing word/document.xml)".to_string();
    };
    let mut xml_content = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut xml, &mut xml_content) {
        return format!("Error reading document content: {e}");
    }
    let markdown = docx_xml_to_markdown(&xml_content);
    let trimmed = markdown.trim();
    if trimmed.is_empty() {
        return "(DOCX contains no extractable text)".to_string();
    }
    let mut result = trimmed.to_string();
    truncate_output(&mut result);
    result
}

fn read_spreadsheet(path: &str) -> String {
    use calamine::{Data, Reader, open_workbook_auto};

    let mut workbook = match open_workbook_auto(path) {
        Ok(wb) => wb,
        Err(e) => return format!("Error opening spreadsheet: {e}"),
    };

    let sheet_names: Vec<String> = workbook.sheet_names().clone();
    if sheet_names.is_empty() {
        return "(spreadsheet contains no sheets)".to_string();
    }

    let mut result = String::new();
    for name in &sheet_names {
        let range = match workbook.worksheet_range(name) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(result, "## {name}\n\nError reading sheet: {e}\n");
                continue;
            }
        };

        let rows: Vec<_> = range.rows().collect();
        if rows.is_empty() {
            let _ = writeln!(result, "## {name}\n\n(empty sheet)\n");
            continue;
        }

        let _ = writeln!(result, "## {name}\n");

        // First row as header
        let header = &rows[0];
        result.push('|');
        for cell in *header {
            let _ = write!(result, " {} |", format_cell(cell));
        }
        result.push('\n');

        // Separator
        result.push('|');
        for _ in *header {
            result.push_str(" --- |");
        }
        result.push('\n');

        // Data rows
        for row in &rows[1..] {
            result.push('|');
            let col_count = header.len();
            for i in 0..col_count {
                let cell = row.get(i).unwrap_or(&Data::Empty);
                let _ = write!(result, " {} |", format_cell(cell));
            }
            result.push('\n');
        }
        result.push('\n');

        if result.len() > MAX_TOOL_OUTPUT_LEN {
            truncate_output(&mut result);
            return result;
        }
    }

    let trimmed = result.trim();
    if trimmed.is_empty() {
        return "(spreadsheet contains no data)".to_string();
    }
    let mut result = trimmed.to_string();
    truncate_output(&mut result);
    result
}

fn format_cell(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        #[allow(clippy::cast_possible_truncation)]
        Data::Float(f) => {
            if (*f - f.round()).abs() < f64::EPSILON {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::Error(e) => format!("#{e:?}"),
        Data::DateTime(dt) => format!("{dt}"),
    }
}

/// Convert DOCX `word/document.xml` content to markdown.
///
/// Recognises paragraphs (`<w:p>`), runs (`<w:r>`), headings (via `<w:pStyle w:val="HeadingN">`),
/// bold (`<w:b/>`), italic (`<w:i/>`), list items (`<w:numPr>`), and table rows/cells.
fn docx_xml_to_markdown(xml: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    let bytes = xml.as_bytes();

    while pos < bytes.len() {
        if let Some(p_start) = find_tag(xml, pos, "<w:tbl") {
            // Check if there's a paragraph before this table
            if let Some(para_start) =
                find_tag(xml, pos, "<w:p ").or_else(|| find_tag(xml, pos, "<w:p>"))
                && para_start < p_start
            {
                let para_end = find_closing(xml, para_start, "w:p").unwrap_or(p_start);
                let para_xml = &xml[para_start..para_end];
                let line = convert_paragraph(para_xml);
                if !line.is_empty() {
                    result.push_str(&line);
                    result.push_str("\n\n");
                }
                pos = para_end;
                continue;
            }
            // Process table
            if let Some(tbl_end) = find_closing(xml, p_start, "w:tbl") {
                let tbl_xml = &xml[p_start..tbl_end];
                let table_md = convert_table(tbl_xml);
                if !table_md.is_empty() {
                    result.push_str(&table_md);
                    result.push_str("\n\n");
                }
                pos = tbl_end;
                continue;
            }
        }

        // Process paragraph
        let p_start = find_tag(xml, pos, "<w:p ").or_else(|| find_tag(xml, pos, "<w:p>"));
        let Some(p_start) = p_start else { break };
        let p_end = find_closing(xml, p_start, "w:p").unwrap_or(xml.len());
        let para_xml = &xml[p_start..p_end];
        let line = convert_paragraph(para_xml);
        if !line.is_empty() {
            result.push_str(&line);
            result.push_str("\n\n");
        }
        pos = p_end;
    }
    result
}

fn convert_paragraph(para: &str) -> String {
    // Detect heading level from <w:pStyle w:val="Heading1"> etc.
    let heading_level = extract_pstyle_heading(para);

    let is_list = para.contains("<w:numPr");

    // Extract text runs
    let mut text = String::new();
    let mut rpos = 0;
    while let Some(r_start) =
        find_tag(para, rpos, "<w:r>").or_else(|| find_tag(para, rpos, "<w:r "))
    {
        let r_end = find_closing(para, r_start, "w:r").unwrap_or(para.len());
        let run = &para[r_start..r_end];

        let is_bold = run.contains("<w:b/>") || run.contains("<w:b ");
        let is_italic = run.contains("<w:i/>") || run.contains("<w:i ");

        // Extract text from <w:t> tags
        let mut tpos = 0;
        while let Some(t_start) =
            find_tag(run, tpos, "<w:t>").or_else(|| find_tag(run, tpos, "<w:t "))
        {
            let content_start = run[t_start..]
                .find('>')
                .map_or(t_start, |i| t_start + i + 1);
            let content_end = run[content_start..]
                .find("</w:t>")
                .map_or(run.len(), |i| content_start + i);
            let t = &run[content_start..content_end];
            if is_bold && is_italic {
                let _ = write!(text, "***{t}***");
            } else if is_bold {
                let _ = write!(text, "**{t}**");
            } else if is_italic {
                let _ = write!(text, "*{t}*");
            } else {
                text.push_str(t);
            }
            tpos = content_end;
        }
        rpos = r_end;
    }

    if text.is_empty() {
        return String::new();
    }

    if let Some(level) = heading_level {
        let hashes = "#".repeat(level.min(6_u8) as usize);
        format!("{hashes} {text}")
    } else if is_list {
        format!("- {text}")
    } else {
        text
    }
}

fn convert_table(tbl: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut pos = 0;
    while let Some(tr_start) = find_tag(tbl, pos, "<w:tr") {
        let tr_end = find_closing(tbl, tr_start, "w:tr").unwrap_or(tbl.len());
        let row_xml = &tbl[tr_start..tr_end];
        let mut cells = Vec::new();
        let mut cpos = 0;
        while let Some(tc_start) = find_tag(row_xml, cpos, "<w:tc") {
            let cell_end = find_closing(row_xml, tc_start, "w:tc").unwrap_or(row_xml.len());
            let cell_xml = &row_xml[tc_start..cell_end];
            // Extract all text from the cell
            let mut cell_text = String::new();
            let mut tpos = 0;
            while let Some(t_start) =
                find_tag(cell_xml, tpos, "<w:t>").or_else(|| find_tag(cell_xml, tpos, "<w:t "))
            {
                let cs = cell_xml[t_start..]
                    .find('>')
                    .map_or(t_start, |i| t_start + i + 1);
                let ce = cell_xml[cs..]
                    .find("</w:t>")
                    .map_or(cell_xml.len(), |i| cs + i);
                if !cell_text.is_empty() {
                    cell_text.push(' ');
                }
                cell_text.push_str(&cell_xml[cs..ce]);
                tpos = ce;
            }
            cells.push(cell_text);
            cpos = cell_end;
        }
        rows.push(cells);
        pos = tr_end;
    }
    if rows.is_empty() {
        return String::new();
    }
    // Build markdown table
    let col_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut md = String::new();
    for (i, row) in rows.iter().enumerate() {
        md.push('|');
        for c in 0..col_count {
            let cell = row.get(c).map_or("", String::as_str);
            let _ = write!(md, " {cell} |");
        }
        md.push('\n');
        if i == 0 {
            md.push('|');
            for _ in 0..col_count {
                md.push_str(" --- |");
            }
            md.push('\n');
        }
    }
    md
}

/// Extract heading level from `<w:pStyle w:val="Heading1"/>` etc.
fn extract_pstyle_heading(para: &str) -> Option<u8> {
    let marker = "w:pStyle";
    let idx = para.find(marker)?;
    let after = &para[idx..];
    let val_marker = "w:val=\"";
    let val_idx = after.find(val_marker)?;
    let val_start = val_idx + val_marker.len();
    let val_end = after[val_start..].find('"')? + val_start;
    let val = &after[val_start..val_end];
    val.strip_prefix("Heading")
        .and_then(|n| n.parse::<u8>().ok())
}

fn find_tag(xml: &str, from: usize, tag: &str) -> Option<usize> {
    xml[from..].find(tag).map(|i| from + i)
}

fn find_closing(xml: &str, from: usize, tag: &str) -> Option<usize> {
    let close = format!("</{tag}>");
    xml[from..].find(&close).map(|i| from + i + close.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_xml_heading() {
        let xml =
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>"#;
        let md = docx_xml_to_markdown(xml);
        assert!(md.contains("# Title"));
    }

    #[test]
    fn docx_xml_bold_italic() {
        let xml = r#"<w:p><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>emphasis</w:t></w:r></w:p>"#;
        let md = docx_xml_to_markdown(xml);
        assert!(md.contains("***emphasis***"));
    }

    #[test]
    fn docx_xml_list_item() {
        let xml = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#;
        let md = docx_xml_to_markdown(xml);
        assert!(md.contains("- item"));
    }

    #[test]
    fn docx_xml_table() {
        let xml = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let md = docx_xml_to_markdown(xml);
        assert!(md.contains("| A |"));
        assert!(md.contains("| --- |"));
        assert!(md.contains("| 1 |"));
    }

    #[test]
    fn docx_xml_empty() {
        let xml = "<w:body></w:body>";
        let md = docx_xml_to_markdown(xml);
        assert!(md.trim().is_empty());
    }

    #[test]
    fn format_cell_types() {
        use calamine::Data;
        assert_eq!(format_cell(&Data::Empty), "");
        assert_eq!(format_cell(&Data::String("hello".into())), "hello");
        assert_eq!(format_cell(&Data::Int(42)), "42");
        assert_eq!(format_cell(&Data::Float(3.0)), "3");
        assert_eq!(format_cell(&Data::Float(3.14)), "3.14");
        assert_eq!(format_cell(&Data::Bool(true)), "true");
    }
}
