//! Real OpenXML `.xlsx` workbook parser backed by `zip` + `quick-xml`
//! matching `NativeWorkbookSpreadsheet` (`{ name, rows, totalRows }`).

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use zip::ZipArchive;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedWorkbookSheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
}

pub fn parse_workbook_file(
    path: &str,
    max_bytes: usize,
    max_rows: usize,
) -> Result<Vec<ParsedWorkbookSheet>, String> {
    if max_bytes == 0 || max_rows == 0 {
        return Err("Workbook preview bounds are invalid.".to_string());
    }
    let meta = fs::metadata(path).map_err(|e| format!("Failed to stat workbook: {e}"))?;
    let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if len > max_bytes {
        return Err(format!(
            "Workbook size ({len} bytes) exceeds preview limit ({max_bytes} bytes)"
        ));
    }

    let file = File::open(path).map_err(|e| format!("Failed to open workbook: {e}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|_| "invalid XLSX container".to_string())?;

    let shared_strings = read_zip_string(&mut archive, "xl/sharedStrings.xml")
        .map(|xml| parse_shared_strings(&xml))
        .unwrap_or_default();

    let workbook_xml = read_zip_string(&mut archive, "xl/workbook.xml")
        .ok_or_else(|| "invalid XLSX container: missing xl/workbook.xml".to_string())?;
    let rels_xml = read_zip_string(&mut archive, "xl/_rels/workbook.xml.rels").unwrap_or_default();

    let rel_targets = parse_workbook_rels(&rels_xml);
    let sheet_defs = parse_workbook_sheets(&workbook_xml);
    if sheet_defs.is_empty() {
        return Err("Workbook contains no worksheets.".to_string());
    }

    let mut sheets = Vec::with_capacity(sheet_defs.len());
    for (idx, (name, rel_id)) in sheet_defs.into_iter().enumerate() {
        let entry_path = rel_targets
            .get(&rel_id)
            .cloned()
            .unwrap_or_else(|| format!("worksheets/sheet{}.xml", idx + 1));
        let normalized = if entry_path.starts_with("xl/") {
            entry_path
        } else {
            format!("xl/{}", entry_path.trim_start_matches('/'))
        };
        let Some(sheet_xml) = read_zip_string(&mut archive, &normalized) else {
            continue;
        };
        let (rows, total_rows) = parse_worksheet_rows(&sheet_xml, &shared_strings, max_rows);
        sheets.push(ParsedWorkbookSheet {
            name,
            rows,
            total_rows,
        });
    }

    if sheets.is_empty() {
        return Err("Workbook contains no worksheets.".to_string());
    }
    Ok(sheets)
}

fn read_zip_string(archive: &mut ZipArchive<File>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut out = String::new();
    entry.read_to_string(&mut out).ok()?;
    Some(out)
}

fn attr_value(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == key {
            if let Ok(v) = attr.unescape_value() {
                return Some(v.into_owned());
            }
        }
    }
    None
}

fn parse_shared_strings(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut in_si = false;
    let mut in_t = false;
    let mut current = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = true;
                    current.clear();
                }
                b"t" if in_si => in_t = true,
                _ => {}
            },
            Ok(Event::Text(e)) if in_si && in_t => {
                if let Ok(txt) = e.decode() {
                    current.push_str(&txt);
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                b"t" => in_t = false,
                b"si" => {
                    in_si = false;
                    strings.push(std::mem::take(&mut current));
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    strings
}

fn parse_workbook_rels(xml: &str) -> BTreeMap<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut map = BTreeMap::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                if let (Some(id), Some(target)) =
                    (attr_value(e, b"Id"), attr_value(e, b"Target"))
                {
                    map.insert(id, target);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

fn parse_workbook_sheets(xml: &str) -> Vec<(String, String)> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if e.local_name().as_ref() == b"sheet" =>
            {
                let name = attr_value(e, b"name").unwrap_or_else(|| "Sheet".to_string());
                let rel_id = attr_value(e, b"r:id").unwrap_or_default();
                out.push((name, rel_id));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn col_index_from_cell_ref(cell_ref: &str) -> Option<usize> {
    let mut idx: usize = 0;
    let mut has_alpha = false;
    for b in cell_ref.bytes() {
        if b.is_ascii_alphabetic() {
            has_alpha = true;
            let digit = (b.to_ascii_uppercase() - b'A' + 1) as usize;
            idx = idx.saturating_mul(26).saturating_add(digit);
        } else {
            break;
        }
    }
    if has_alpha && idx > 0 {
        Some(idx - 1)
    } else {
        None
    }
}

fn parse_worksheet_rows(
    xml: &str,
    shared_strings: &[String],
    max_rows: usize,
) -> (Vec<Vec<String>>, usize) {
    let mut reader = Reader::from_str(xml);
    let mut rows = Vec::new();
    let mut total_rows = 0usize;

    let mut in_row = false;
    let mut current_row: Vec<String> = Vec::new();
    let mut cell_type = String::new();
    let mut cell_col: Option<usize> = None;
    let mut in_value = false;
    let mut cell_text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                b"row" => {
                    in_row = true;
                    current_row.clear();
                }
                b"c" if in_row => {
                    cell_type = attr_value(e, b"t").unwrap_or_default();
                    cell_col = attr_value(e, b"r").and_then(|r| col_index_from_cell_ref(&r));
                    cell_text.clear();
                }
                b"v" | b"t" if in_row => {
                    in_value = true;
                }
                _ => {}
            },
            Ok(Event::Text(e)) if in_row && in_value => {
                if let Ok(txt) = e.decode() {
                    cell_text.push_str(&txt);
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                b"v" | b"t" => in_value = false,
                b"c" if in_row => {
                    let resolved = match cell_type.as_str() {
                        "s" => cell_text
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| shared_strings.get(i).cloned())
                            .unwrap_or_default(),
                        "b" => {
                            if cell_text.trim() == "1" {
                                "TRUE".to_string()
                            } else {
                                "FALSE".to_string()
                            }
                        }
                        _ => cell_text.clone(),
                    };
                    if rows.len() < max_rows {
                        let col = cell_col.unwrap_or(current_row.len()).min(200);
                        if current_row.len() <= col {
                            current_row.resize(col + 1, String::new());
                        }
                        current_row[col] = resolved;
                    }
                }
                b"row" if in_row => {
                    in_row = false;
                    total_rows += 1;
                    if rows.len() < max_rows {
                        rows.push(std::mem::take(&mut current_row));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    (rows, total_rows)
}
