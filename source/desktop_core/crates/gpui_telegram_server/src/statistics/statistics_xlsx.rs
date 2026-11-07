//! Telegram Desktop 1:1 Submodule: `statistics_xlsx`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use serde::{Deserialize, Serialize};

pub const K_EPOCH_SERIAL: f64 = 25569.0;
pub const K_MILLISECONDS_IN_DAY: f64 = 86_400_000.0;
pub const K_MAX_SHEET_NAME_LENGTH: usize = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CellType {
    #[default]
    Empty,
    Text,
    Header,
    Number,
    Date,
    DateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Cell {
    pub cell_type: CellType,
    pub text: String,
    pub number: f64,
}

impl Cell {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn text(t: impl Into<String>) -> Self {
        Self {
            cell_type: CellType::Text,
            text: t.into(),
            number: 0.0,
        }
    }

    pub fn header(t: impl Into<String>) -> Self {
        Self {
            cell_type: CellType::Header,
            text: t.into(),
            number: 0.0,
        }
    }

    pub fn number(val: f64) -> Self {
        Self {
            cell_type: CellType::Number,
            text: String::new(),
            number: val,
        }
    }

    pub fn date(millis: f64) -> Self {
        Self {
            cell_type: CellType::Date,
            text: String::new(),
            number: K_EPOCH_SERIAL + (millis / K_MILLISECONDS_IN_DAY),
        }
    }

    pub fn date_time(millis: f64) -> Self {
        Self {
            cell_type: CellType::DateTime,
            text: String::new(),
            number: K_EPOCH_SERIAL + (millis / K_MILLISECONDS_IN_DAY),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
}

/// Computes Excel column label (0 -> "A", 1 -> "B", ..., 26 -> "AA")
pub fn column_name(mut index: usize) -> String {
    let mut result = Vec::new();
    index += 1;
    while index > 0 {
        let rem = ((index - 1) % 26) as u8;
        result.push(b'A' + rem);
        index = (index - 1) / 26;
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "A".to_string())
}

pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

pub fn serialize_sheet_to_xml(sheet: &Sheet) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str(&format!("<!-- Sheet: {} -->\n", xml_escape(&sheet.name)));
    xml.push_str("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\n");
    xml.push_str("  <sheetData>\n");

    for (row_idx, row) in sheet.rows.iter().enumerate() {
        let r = row_idx + 1;
        xml.push_str(&format!("    <row r=\"{}\">\n", r));
        for (col_idx, cell) in row.iter().enumerate() {
            let col = column_name(col_idx);
            let cell_ref = format!("{}{}", col, r);
            match cell.cell_type {
                CellType::Empty => {}
                CellType::Text | CellType::Header => {
                    xml.push_str(&format!(
                        "      <c r=\"{}\" t=\"inlineStr\"><is><t>{}</t></is></c>\n",
                        cell_ref,
                        xml_escape(&cell.text)
                    ));
                }
                CellType::Number | CellType::Date | CellType::DateTime => {
                    xml.push_str(&format!(
                        "      <c r=\"{}\"><v>{}</v></c>\n",
                        cell_ref, cell.number
                    ));
                }
            }
        }
        xml.push_str("    </row>\n");
    }

    xml.push_str("  </sheetData>\n</worksheet>");
    xml
}

/// Target symbol wrapper for compatibility
pub struct StatisticsXlsx;

impl StatisticsXlsx {
    pub fn serialize(sheets: &[Sheet]) -> Vec<u8> {
        let mut combined = Vec::new();
        for sheet in sheets {
            let xml = serialize_sheet_to_xml(sheet);
            combined.extend_from_slice(xml.as_bytes());
            combined.push(b'\n');
        }
        combined
    }
}
