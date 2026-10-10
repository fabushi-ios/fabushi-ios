use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use xml::attribute::OwnedAttribute;
use xml::reader::{EventReader, XmlEvent};
use zip::ZipArchive;

const MAX_WORKBOOK_SHEETS: usize = 128;
const MAX_XML_ENTRY_BYTES: usize = 32 * 1024 * 1024;
const MAX_XML_TOTAL_BYTES: usize = 96 * 1024 * 1024;
const MAX_STORED_COLUMNS: usize = 200;
const MAX_NONEMPTY_ROWS: usize = 100_000;
const CFB_FREE_SECTOR: u32 = 0xffff_ffff;
const CFB_END_OF_CHAIN: u32 = 0xffff_fffe;
const CFB_FAT_SECTOR: u32 = 0xffff_fffd;
const CFB_DIFAT_SECTOR: u32 = 0xffff_fffc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SpreadsheetSheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    #[serde(rename = "totalRows")]
    pub total_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SpreadsheetWorkbook {
    pub sheets: Vec<SpreadsheetSheet>,
}

pub(crate) fn parse_workbook_file(
    path: &Path,
    max_bytes: u64,
    max_rows: usize,
) -> Result<SpreadsheetWorkbook, String> {
    if max_rows == 0 {
        return Ok(SpreadsheetWorkbook { sheets: Vec::new() });
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("spreadsheet file unavailable: {error}"))?;
    if !metadata.is_file() {
        return Err("spreadsheet source is not a regular file".into());
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "spreadsheet exceeds preview byte cap ({} > {})",
            metadata.len(),
            max_bytes
        ));
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "xlsx" => parse_xlsx(path, max_rows),
        "xls" => parse_xls(path, max_rows),
        _ => {
            let mut prefix = [0u8; 8];
            let mut file = File::open(path)
                .map_err(|error| format!("open spreadsheet: {error}"))?;
            let count = file
                .read(&mut prefix)
                .map_err(|error| format!("read spreadsheet signature: {error}"))?;
            if count >= 8 && prefix == [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1] {
                parse_xls(path, max_rows)
            } else if count >= 4 && prefix[..4] == [0x50, 0x4b, 0x03, 0x04] {
                parse_xlsx(path, max_rows)
            } else {
                Err("unsupported workbook format".into())
            }
        }
    }
}

fn bounded_zip_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    total_budget: &mut usize,
) -> Result<String, String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|error| format!("missing workbook entry {name}: {error}"))?;
    if entry.size() as usize > MAX_XML_ENTRY_BYTES {
        return Err(format!("workbook XML entry is too large: {name}"));
    }
    let remaining = MAX_XML_TOTAL_BYTES.saturating_sub(*total_budget);
    if remaining == 0 {
        return Err("workbook XML resource budget exceeded".into());
    }
    let per_entry = MAX_XML_ENTRY_BYTES.min(remaining);
    let mut bytes = Vec::with_capacity((entry.size() as usize).min(per_entry));
    entry
        .by_ref()
        .take((per_entry + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read workbook entry {name}: {error}"))?;
    if bytes.len() > per_entry {
        return Err(format!("workbook XML entry exceeded resource limit: {name}"));
    }
    *total_budget += bytes.len();
    String::from_utf8(bytes).map_err(|_| format!("workbook XML is not UTF-8: {name}"))
}

fn attr(attributes: &[OwnedAttribute], local_name: &str) -> Option<String> {
    attributes
        .iter()
        .find(|item| item.name.local_name == local_name)
        .map(|item| item.value.clone())
}

fn parse_xml_events(
    xml: &str,
) -> impl Iterator<Item = Result<XmlEvent, xml::reader::Error>> + '_ {
    EventReader::new(xml.as_bytes()).into_iter()
}

fn parse_xlsx(path: &Path, max_rows: usize) -> Result<SpreadsheetWorkbook, String> {
    let file = File::open(path).map_err(|error| format!("open XLSX workbook: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("invalid XLSX container: {error}"))?;
    let mut budget = 0usize;

    let workbook_xml = bounded_zip_text(&mut archive, "xl/workbook.xml", &mut budget)?;
    let rels_xml = bounded_zip_text(&mut archive, "xl/_rels/workbook.xml.rels", &mut budget)?;

    let mut sheet_refs = Vec::<(String, String)>::new();
    for event in parse_xml_events(&workbook_xml) {
        match event.map_err(|error| format!("parse workbook.xml: {error}"))? {
            XmlEvent::StartElement { name, attributes, .. } if name.local_name == "sheet" => {
                let sheet_name = attr(&attributes, "name")
                    .ok_or_else(|| "XLSX sheet omitted name".to_string())?;
                let relation_id = attr(&attributes, "id")
                    .ok_or_else(|| "XLSX sheet omitted relationship id".to_string())?;
                sheet_refs.push((sheet_name, relation_id));
                if sheet_refs.len() > MAX_WORKBOOK_SHEETS {
                    return Err("XLSX workbook contains too many sheets".into());
                }
            }
            _ => {}
        }
    }

    let mut targets = HashMap::<String, String>::new();
    for event in parse_xml_events(&rels_xml) {
        match event.map_err(|error| format!("parse workbook relationships: {error}"))? {
            XmlEvent::StartElement { name, attributes, .. }
                if name.local_name == "Relationship" =>
            {
                if let (Some(id), Some(target)) = (attr(&attributes, "Id"), attr(&attributes, "Target")) {
                    targets.insert(id, normalize_xlsx_target(&target)?);
                }
            }
            _ => {}
        }
    }

    let shared_strings = match archive.by_name("xl/sharedStrings.xml") {
        Ok(mut entry) => {
            if entry.size() as usize > MAX_XML_ENTRY_BYTES * 2 {
                return Err("XLSX shared string table is too large".into());
            }
            let remaining = MAX_XML_TOTAL_BYTES.saturating_sub(budget);
            let limit = (MAX_XML_ENTRY_BYTES * 2).min(remaining);
            let mut bytes = Vec::with_capacity((entry.size() as usize).min(limit));
            entry
                .by_ref()
                .take((limit + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("read XLSX shared strings: {error}"))?;
            if bytes.len() > limit {
                return Err("XLSX shared strings exceeded resource limit".into());
            }
            budget += bytes.len();
            let xml = String::from_utf8(bytes)
                .map_err(|_| "XLSX shared strings are not UTF-8".to_string())?;
            parse_shared_strings(&xml)?
        }
        Err(zip::result::ZipError::FileNotFound) => Vec::new(),
        Err(error) => return Err(format!("open XLSX shared strings: {error}")),
    };

    let mut sheets = Vec::with_capacity(sheet_refs.len());
    for (name, relation_id) in sheet_refs {
        let target = targets
            .get(&relation_id)
            .ok_or_else(|| format!("XLSX sheet relationship is missing: {relation_id}"))?;
        let xml = bounded_zip_text(&mut archive, target, &mut budget)?;
        sheets.push(parse_xlsx_sheet(
            &name,
            &xml,
            &shared_strings,
            max_rows,
        )?);
    }
    Ok(SpreadsheetWorkbook { sheets })
}

fn normalize_xlsx_target(target: &str) -> Result<String, String> {
    let mut parts = if target.starts_with('/') {
        Vec::<String>::new()
    } else {
        vec!["xl".to_string()]
    };
    for part in target.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("XLSX relationship escapes workbook root".into());
                }
            }
            value => parts.push(value.to_string()),
        }
    }
    if parts.first().map(String::as_str) != Some("xl") {
        return Err("XLSX worksheet target is outside xl/".into());
    }
    Ok(parts.join("/"))
}

fn parse_shared_strings(xml: &str) -> Result<Vec<String>, String> {
    let mut output = Vec::<String>::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    for event in parse_xml_events(xml) {
        match event.map_err(|error| format!("parse XLSX shared strings: {error}"))? {
            XmlEvent::StartElement { name, .. } if name.local_name == "si" => {
                current = Some(String::new());
            }
            XmlEvent::StartElement { name, .. } if name.local_name == "t" && current.is_some() => {
                in_text = true;
            }
            XmlEvent::Characters(value) | XmlEvent::Whitespace(value)
                if in_text && current.is_some() =>
            {
                if let Some(text) = current.as_mut() {
                    text.push_str(&value);
                    if text.len() > 1024 * 1024 {
                        return Err("XLSX cell text exceeded resource limit".into());
                    }
                }
            }
            XmlEvent::EndElement { name } if name.local_name == "t" => {
                in_text = false;
            }
            XmlEvent::EndElement { name } if name.local_name == "si" => {
                output.push(current.take().unwrap_or_default());
            }
            _ => {}
        }
    }
    Ok(output)
}

#[derive(Default)]
struct XlsxCell {
    row: usize,
    column: usize,
    cell_type: String,
    value: String,
    inline_text: String,
    in_value: bool,
    in_inline_text: bool,
}

fn parse_cell_reference(reference: &str) -> Option<(usize, usize)> {
    let mut column = 0usize;
    let mut saw_column = false;
    let mut row_digits = String::new();
    for character in reference.chars() {
        if character.is_ascii_alphabetic() && row_digits.is_empty() {
            saw_column = true;
            column = column
                .checked_mul(26)?
                .checked_add((character.to_ascii_uppercase() as u8 - b'A' + 1) as usize)?;
        } else if character.is_ascii_digit() {
            row_digits.push(character);
        } else if character == '$' {
            continue;
        } else {
            return None;
        }
    }
    if !saw_column || row_digits.is_empty() {
        return None;
    }
    let row = row_digits.parse::<usize>().ok()?.checked_sub(1)?;
    Some((row, column.checked_sub(1)?))
}

fn xlsx_cell_value(cell: &XlsxCell, shared_strings: &[String]) -> Result<String, String> {
    match cell.cell_type.as_str() {
        "s" => {
            let index = cell
                .value
                .trim()
                .parse::<usize>()
                .map_err(|_| "XLSX shared-string cell has invalid index".to_string())?;
            shared_strings
                .get(index)
                .cloned()
                .ok_or_else(|| "XLSX shared-string index is out of range".to_string())
        }
        "inlineStr" => Ok(cell.inline_text.clone()),
        "b" => Ok(if cell.value.trim() == "1" { "TRUE" } else { "FALSE" }.into()),
        _ => Ok(cell.value.clone()),
    }
}

fn parse_xlsx_sheet(
    name: &str,
    xml: &str,
    shared_strings: &[String],
    max_rows: usize,
) -> Result<SpreadsheetSheet, String> {
    let mut current_row = 0usize;
    let mut current_cell: Option<XlsxCell> = None;
    let mut nonempty_rows = BTreeSet::<usize>::new();
    let mut stored_cells = BTreeMap::<usize, BTreeMap<usize, String>>::new();

    for event in parse_xml_events(xml) {
        match event.map_err(|error| format!("parse XLSX worksheet {name}: {error}"))? {
            XmlEvent::StartElement { name: element, attributes, .. }
                if element.local_name == "row" =>
            {
                current_row = attr(&attributes, "r")
                    .and_then(|value| value.parse::<usize>().ok())
                    .and_then(|value| value.checked_sub(1))
                    .unwrap_or(current_row);
            }
            XmlEvent::StartElement { name: element, attributes, .. }
                if element.local_name == "c" =>
            {
                let (row, column) = attr(&attributes, "r")
                    .as_deref()
                    .and_then(parse_cell_reference)
                    .unwrap_or((current_row, 0));
                current_cell = Some(XlsxCell {
                    row,
                    column,
                    cell_type: attr(&attributes, "t").unwrap_or_default(),
                    ..Default::default()
                });
            }
            XmlEvent::StartElement { name: element, .. } if element.local_name == "v" => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.in_value = true;
                }
            }
            XmlEvent::StartElement { name: element, .. } if element.local_name == "t" => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.in_inline_text = true;
                }
            }
            XmlEvent::Characters(value) | XmlEvent::Whitespace(value) => {
                if let Some(cell) = current_cell.as_mut() {
                    if cell.in_value {
                        cell.value.push_str(&value);
                    }
                    if cell.in_inline_text {
                        cell.inline_text.push_str(&value);
                    }
                    if cell.value.len() > 1024 * 1024 || cell.inline_text.len() > 1024 * 1024 {
                        return Err("XLSX cell text exceeded resource limit".into());
                    }
                }
            }
            XmlEvent::EndElement { name: element } if element.local_name == "v" => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.in_value = false;
                }
            }
            XmlEvent::EndElement { name: element } if element.local_name == "t" => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.in_inline_text = false;
                }
            }
            XmlEvent::EndElement { name: element } if element.local_name == "c" => {
                if let Some(cell) = current_cell.take() {
                    let value = xlsx_cell_value(&cell, shared_strings)?;
                    if !value.is_empty() {
                        nonempty_rows.insert(cell.row);
                        if nonempty_rows.len() > MAX_NONEMPTY_ROWS {
                            return Err("XLSX worksheet has too many non-empty rows".into());
                        }
                        if cell.column < MAX_STORED_COLUMNS {
                            stored_cells
                                .entry(cell.row)
                                .or_default()
                                .insert(cell.column, value);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(project_sparse_rows(
        name.to_string(),
        nonempty_rows,
        stored_cells,
        max_rows,
    ))
}

fn project_sparse_rows(
    name: String,
    nonempty_rows: BTreeSet<usize>,
    stored_cells: BTreeMap<usize, BTreeMap<usize, String>>,
    max_rows: usize,
) -> SpreadsheetSheet {
    let total_rows = nonempty_rows.len();
    let rows = nonempty_rows
        .iter()
        .take(max_rows)
        .map(|row_index| {
            let cells = stored_cells.get(row_index);
            let width = cells
                .and_then(|row| row.keys().next_back().copied())
                .map(|column| (column + 1).min(MAX_STORED_COLUMNS))
                .unwrap_or(0);
            let mut row = vec![String::new(); width];
            if let Some(cells) = cells {
                for (&column, value) in cells {
                    if column < row.len() {
                        row[column] = value.clone();
                    }
                }
            }
            row
        })
        .collect();
    SpreadsheetSheet {
        name,
        rows,
        total_rows,
    }
}

fn parse_xls(path: &Path, max_rows: usize) -> Result<SpreadsheetWorkbook, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read XLS workbook: {error}"))?;
    let workbook_stream = extract_cfb_workbook_stream(&bytes)?;
    parse_biff_workbook_stream(&workbook_stream, max_rows)
}

fn u16_le(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "truncated binary workbook".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "truncated binary workbook".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_le(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| "truncated binary workbook".to_string())?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3],
        slice[4], slice[5], slice[6], slice[7],
    ]))
}

#[derive(Clone, Copy)]
struct CfbHeader {
    sector_size: usize,
    mini_sector_size: usize,
    first_directory_sector: u32,
    mini_stream_cutoff: usize,
    first_mini_fat_sector: u32,
    mini_fat_sector_count: usize,
}

fn cfb_sector<'a>(
    bytes: &'a [u8],
    header: CfbHeader,
    sector_id: u32,
) -> Result<&'a [u8], String> {
    if sector_id >= CFB_DIFAT_SECTOR {
        return Err("invalid CFB sector id".into());
    }
    let offset = header
        .sector_size
        .checked_mul(sector_id as usize + 1)
        .ok_or_else(|| "CFB sector offset overflow".to_string())?;
    bytes
        .get(offset..offset + header.sector_size)
        .ok_or_else(|| "CFB sector points outside file".to_string())
}

fn cfb_chain(
    bytes: &[u8],
    header: CfbHeader,
    fat: &[u32],
    start: u32,
    max_sectors: usize,
) -> Result<Vec<u8>, String> {
    if start == CFB_END_OF_CHAIN || start == CFB_FREE_SECTOR {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    let mut sector = start;
    let mut visited = BTreeSet::new();
    while sector != CFB_END_OF_CHAIN {
        if !visited.insert(sector) {
            return Err("CFB sector chain contains a cycle".into());
        }
        if visited.len() > max_sectors {
            return Err("CFB sector chain exceeded resource limit".into());
        }
        output.extend_from_slice(cfb_sector(bytes, header, sector)?);
        sector = *fat
            .get(sector as usize)
            .ok_or_else(|| "CFB FAT lookup is out of range".to_string())?;
        if matches!(sector, CFB_FREE_SECTOR | CFB_FAT_SECTOR | CFB_DIFAT_SECTOR) {
            return Err("CFB stream chain points to a reserved sector".into());
        }
    }
    Ok(output)
}

#[derive(Debug)]
struct CfbDirectoryEntry {
    name: String,
    object_type: u8,
    start_sector: u32,
    size: usize,
}

fn parse_directory_entry(bytes: &[u8]) -> Result<CfbDirectoryEntry, String> {
    if bytes.len() != 128 {
        return Err("invalid CFB directory entry size".into());
    }
    let name_bytes = u16_le(bytes, 64)? as usize;
    let name = if name_bytes >= 2 && name_bytes <= 64 {
        let units = bytes[..name_bytes - 2]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::new()
    };
    Ok(CfbDirectoryEntry {
        name,
        object_type: bytes[66],
        start_sector: u32_le(bytes, 116)?,
        size: u64_le(bytes, 120)? as usize,
    })
}

fn read_mini_stream(
    mini_stream: &[u8],
    mini_fat: &[u32],
    mini_sector_size: usize,
    start: u32,
    size: usize,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(size);
    let mut sector = start;
    let mut visited = BTreeSet::new();
    while sector != CFB_END_OF_CHAIN && output.len() < size {
        if !visited.insert(sector) || visited.len() > mini_fat.len().saturating_add(1) {
            return Err("CFB mini stream chain is invalid".into());
        }
        let offset = (sector as usize)
            .checked_mul(mini_sector_size)
            .ok_or_else(|| "CFB mini-sector offset overflow".to_string())?;
        let chunk = mini_stream
            .get(offset..offset + mini_sector_size)
            .ok_or_else(|| "CFB mini stream points outside root stream".to_string())?;
        output.extend_from_slice(chunk);
        sector = *mini_fat
            .get(sector as usize)
            .ok_or_else(|| "CFB mini FAT lookup is out of range".to_string())?;
    }
    output.truncate(size);
    if output.len() != size {
        return Err("CFB mini stream is truncated".into());
    }
    Ok(output)
}

fn extract_cfb_workbook_stream(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 512
        || bytes[..8] != [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]
    {
        return Err("invalid XLS compound-file signature".into());
    }
    let byte_order = u16_le(bytes, 28)?;
    if byte_order != 0xfffe {
        return Err("unsupported XLS compound-file byte order".into());
    }
    let sector_shift = u16_le(bytes, 30)?;
    let mini_sector_shift = u16_le(bytes, 32)?;
    let sector_size = 1usize
        .checked_shl(sector_shift as u32)
        .ok_or_else(|| "invalid CFB sector size".to_string())?;
    let mini_sector_size = 1usize
        .checked_shl(mini_sector_shift as u32)
        .ok_or_else(|| "invalid CFB mini-sector size".to_string())?;
    if !matches!(sector_size, 512 | 4096) || mini_sector_size != 64 {
        return Err("unsupported CFB sector geometry".into());
    }
    if bytes.len() < sector_size {
        return Err("truncated CFB header sector".into());
    }

    let fat_sector_count = u32_le(bytes, 44)? as usize;
    let first_directory_sector = u32_le(bytes, 48)?;
    let mini_stream_cutoff = u32_le(bytes, 56)? as usize;
    let first_mini_fat_sector = u32_le(bytes, 60)?;
    let mini_fat_sector_count = u32_le(bytes, 64)? as usize;
    let mut next_difat_sector = u32_le(bytes, 68)?;
    let difat_sector_count = u32_le(bytes, 72)? as usize;
    let header = CfbHeader {
        sector_size,
        mini_sector_size,
        first_directory_sector,
        mini_stream_cutoff,
        first_mini_fat_sector,
        mini_fat_sector_count,
    };

    let total_sectors = bytes.len().saturating_sub(sector_size) / sector_size;
    let mut fat_sector_ids = Vec::<u32>::new();
    for index in 0..109 {
        let sector = u32_le(bytes, 76 + index * 4)?;
        if sector != CFB_FREE_SECTOR {
            fat_sector_ids.push(sector);
        }
    }
    for _ in 0..difat_sector_count {
        if next_difat_sector == CFB_END_OF_CHAIN || next_difat_sector == CFB_FREE_SECTOR {
            break;
        }
        let sector = cfb_sector(bytes, header, next_difat_sector)?;
        let entries = sector_size / 4;
        for index in 0..entries - 1 {
            let fat_sector = u32_le(sector, index * 4)?;
            if fat_sector != CFB_FREE_SECTOR {
                fat_sector_ids.push(fat_sector);
            }
        }
        next_difat_sector = u32_le(sector, (entries - 1) * 4)?;
    }
    if fat_sector_ids.len() < fat_sector_count {
        return Err("CFB DIFAT omits declared FAT sectors".into());
    }
    fat_sector_ids.truncate(fat_sector_count);

    let mut fat = Vec::<u32>::new();
    for sector_id in fat_sector_ids {
        let sector = cfb_sector(bytes, header, sector_id)?;
        for chunk in sector.chunks_exact(4) {
            fat.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
    }
    if fat.is_empty() {
        return Err("CFB workbook has no FAT".into());
    }

    let directory = cfb_chain(
        bytes,
        header,
        &fat,
        first_directory_sector,
        total_sectors.saturating_add(1),
    )?;
    let mut root: Option<CfbDirectoryEntry> = None;
    let mut workbook: Option<CfbDirectoryEntry> = None;
    for entry_bytes in directory.chunks_exact(128) {
        let entry = parse_directory_entry(entry_bytes)?;
        if entry.object_type == 5 {
            root = Some(entry);
        } else if entry.object_type == 2
            && (entry.name.eq_ignore_ascii_case("Workbook")
                || entry.name.eq_ignore_ascii_case("Book"))
        {
            workbook = Some(entry);
        }
    }
    let workbook = workbook.ok_or_else(|| "XLS compound file has no Workbook stream".to_string())?;

    if workbook.size >= mini_stream_cutoff {
        let mut stream = cfb_chain(
            bytes,
            header,
            &fat,
            workbook.start_sector,
            total_sectors.saturating_add(1),
        )?;
        stream.truncate(workbook.size);
        if stream.len() != workbook.size {
            return Err("XLS Workbook stream is truncated".into());
        }
        return Ok(stream);
    }

    let root = root.ok_or_else(|| "XLS compound file has no root storage".to_string())?;
    let mut mini_stream = cfb_chain(
        bytes,
        header,
        &fat,
        root.start_sector,
        total_sectors.saturating_add(1),
    )?;
    mini_stream.truncate(root.size);

    let mini_fat_bytes = if mini_fat_sector_count == 0 {
        Vec::new()
    } else {
        let mut chain = cfb_chain(
            bytes,
            header,
            &fat,
            first_mini_fat_sector,
            mini_fat_sector_count.saturating_add(1),
        )?;
        chain.truncate(mini_fat_sector_count * sector_size);
        chain
    };
    let mini_fat = mini_fat_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    read_mini_stream(
        &mini_stream,
        &mini_fat,
        mini_sector_size,
        workbook.start_sector,
        workbook.size,
    )
}

#[derive(Debug)]
struct BiffSheetBound {
    offset: usize,
    name: String,
}

fn biff_record(stream: &[u8], offset: usize) -> Result<(u16, &[u8], usize), String> {
    let id = u16_le(stream, offset)?;
    let length = u16_le(stream, offset + 2)? as usize;
    let start = offset + 4;
    let end = start
        .checked_add(length)
        .ok_or_else(|| "BIFF record length overflow".to_string())?;
    let payload = stream
        .get(start..end)
        .ok_or_else(|| "truncated BIFF record".to_string())?;
    Ok((id, payload, end))
}

fn parse_boundsheet(payload: &[u8]) -> Result<BiffSheetBound, String> {
    if payload.len() < 8 {
        return Err("truncated BIFF BOUNDSHEET record".into());
    }
    let offset = u32_le(payload, 0)? as usize;
    let character_count = payload[6] as usize;
    let flags = payload[7];
    let unicode = flags & 0x01 != 0;
    let width = if unicode { 2 } else { 1 };
    let byte_count = character_count
        .checked_mul(width)
        .ok_or_else(|| "BIFF sheet name length overflow".to_string())?;
    let bytes = payload
        .get(8..8 + byte_count)
        .ok_or_else(|| "truncated BIFF sheet name".to_string())?;
    let name = if unicode {
        let units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    };
    Ok(BiffSheetBound { offset, name })
}

struct SstCursor<'a> {
    chunks: &'a [&'a [u8]],
    chunk: usize,
    offset: usize,
}

impl<'a> SstCursor<'a> {
    fn new(chunks: &'a [&'a [u8]]) -> Self {
        Self { chunks, chunk: 0, offset: 0 }
    }

    fn advance_chunk(&mut self) -> Result<(), String> {
        self.chunk += 1;
        self.offset = 0;
        if self.chunk >= self.chunks.len() {
            return Err("truncated BIFF SST continuation".into());
        }
        Ok(())
    }

    fn read_raw_u8(&mut self) -> Result<u8, String> {
        loop {
            if self.chunk >= self.chunks.len() {
                return Err("truncated BIFF SST".into());
            }
            if self.offset < self.chunks[self.chunk].len() {
                let value = self.chunks[self.chunk][self.offset];
                self.offset += 1;
                return Ok(value);
            }
            self.advance_chunk()?;
        }
    }

    fn read_raw_u16(&mut self) -> Result<u16, String> {
        let a = self.read_raw_u8()?;
        let b = self.read_raw_u8()?;
        Ok(u16::from_le_bytes([a, b]))
    }

    fn read_raw_u32(&mut self) -> Result<u32, String> {
        let a = self.read_raw_u8()?;
        let b = self.read_raw_u8()?;
        let c = self.read_raw_u8()?;
        let d = self.read_raw_u8()?;
        Ok(u32::from_le_bytes([a, b, c, d]))
    }

    fn skip_raw(&mut self, mut count: usize) -> Result<(), String> {
        while count > 0 {
            self.read_raw_u8()?;
            count -= 1;
        }
        Ok(())
    }

    fn read_string(&mut self) -> Result<String, String> {
        let character_count = self.read_raw_u16()? as usize;
        let flags = self.read_raw_u8()?;
        let rich_runs = if flags & 0x08 != 0 {
            self.read_raw_u16()? as usize
        } else {
            0
        };
        let extension_bytes = if flags & 0x04 != 0 {
            self.read_raw_u32()? as usize
        } else {
            0
        };
        let mut wide = flags & 0x01 != 0;
        let mut units = Vec::<u16>::with_capacity(character_count);

        while units.len() < character_count {
            if self.chunk >= self.chunks.len() {
                return Err("truncated BIFF shared string".into());
            }
            let width = if wide { 2 } else { 1 };
            if self.offset + width > self.chunks[self.chunk].len() {
                self.advance_chunk()?;
                let continuation_flags = self.read_raw_u8()?;
                wide = continuation_flags & 0x01 != 0;
                continue;
            }
            if wide {
                let bytes = &self.chunks[self.chunk][self.offset..self.offset + 2];
                units.push(u16::from_le_bytes([bytes[0], bytes[1]]));
                self.offset += 2;
            } else {
                units.push(self.chunks[self.chunk][self.offset] as u16);
                self.offset += 1;
            }
        }

        self.skip_raw(rich_runs.saturating_mul(4))?;
        self.skip_raw(extension_bytes)?;
        Ok(String::from_utf16_lossy(&units))
    }
}

fn parse_sst(chunks: &[&[u8]]) -> Result<Vec<String>, String> {
    let mut cursor = SstCursor::new(chunks);
    let _total_strings = cursor.read_raw_u32()?;
    let unique_strings = cursor.read_raw_u32()? as usize;
    if unique_strings > 1_000_000 {
        return Err("BIFF shared string table is too large".into());
    }
    let mut output = Vec::with_capacity(unique_strings);
    for _ in 0..unique_strings {
        let value = cursor.read_string()?;
        if value.len() > 1024 * 1024 {
            return Err("BIFF cell text exceeded resource limit".into());
        }
        output.push(value);
    }
    Ok(output)
}

fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if value.fract() == 0.0 && value.abs() <= i64::MAX as f64 {
        format!("{}", value as i64)
    } else {
        let mut text = value.to_string();
        if text == "-0" {
            text = "0".into();
        }
        text
    }
}

fn decode_rk(value: u32) -> String {
    let divided = value & 0x01 != 0;
    let numeric = if value & 0x02 != 0 {
        ((value as i32) >> 2) as f64
    } else {
        f64::from_bits(((value & 0xffff_fffc) as u64) << 32)
    };
    format_number(if divided { numeric / 100.0 } else { numeric })
}

fn insert_biff_cell(
    nonempty_rows: &mut BTreeSet<usize>,
    stored_cells: &mut BTreeMap<usize, BTreeMap<usize, String>>,
    row: usize,
    column: usize,
    value: String,
) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    nonempty_rows.insert(row);
    if nonempty_rows.len() > MAX_NONEMPTY_ROWS {
        return Err("BIFF worksheet has too many non-empty rows".into());
    }
    if column < MAX_STORED_COLUMNS {
        stored_cells.entry(row).or_default().insert(column, value);
    }
    Ok(())
}

fn parse_biff_sheet(
    stream: &[u8],
    bound: &BiffSheetBound,
    shared_strings: &[String],
    max_rows: usize,
) -> Result<SpreadsheetSheet, String> {
    if bound.offset >= stream.len() {
        return Err("BIFF sheet offset points outside Workbook stream".into());
    }
    let mut offset = bound.offset;
    let mut nonempty_rows = BTreeSet::<usize>::new();
    let mut stored_cells = BTreeMap::<usize, BTreeMap<usize, String>>::new();

    while offset + 4 <= stream.len() {
        let (id, payload, next) = biff_record(stream, offset)?;
        offset = next;
        match id {
            0x000a => break,
            0x00fd => {
                if payload.len() < 10 {
                    return Err("truncated BIFF LABELSST record".into());
                }
                let row = u16_le(payload, 0)? as usize;
                let column = u16_le(payload, 2)? as usize;
                let index = u32_le(payload, 6)? as usize;
                let value = shared_strings
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "BIFF shared-string index is out of range".to_string())?;
                insert_biff_cell(&mut nonempty_rows, &mut stored_cells, row, column, value)?;
            }
            0x0203 => {
                if payload.len() < 14 {
                    return Err("truncated BIFF NUMBER record".into());
                }
                let row = u16_le(payload, 0)? as usize;
                let column = u16_le(payload, 2)? as usize;
                let bits = u64_le(payload, 6)?;
                insert_biff_cell(
                    &mut nonempty_rows,
                    &mut stored_cells,
                    row,
                    column,
                    format_number(f64::from_bits(bits)),
                )?;
            }
            0x027e => {
                if payload.len() < 10 {
                    return Err("truncated BIFF RK record".into());
                }
                let row = u16_le(payload, 0)? as usize;
                let column = u16_le(payload, 2)? as usize;
                insert_biff_cell(
                    &mut nonempty_rows,
                    &mut stored_cells,
                    row,
                    column,
                    decode_rk(u32_le(payload, 6)?),
                )?;
            }
            0x00bd => {
                if payload.len() < 12 {
                    return Err("truncated BIFF MULRK record".into());
                }
                let row = u16_le(payload, 0)? as usize;
                let first_column = u16_le(payload, 2)? as usize;
                let last_column = u16_le(payload, payload.len() - 2)? as usize;
                let count = last_column.saturating_sub(first_column).saturating_add(1);
                if 4 + count * 6 + 2 > payload.len() {
                    return Err("malformed BIFF MULRK record".into());
                }
                for index in 0..count {
                    let rk_offset = 4 + index * 6 + 2;
                    insert_biff_cell(
                        &mut nonempty_rows,
                        &mut stored_cells,
                        row,
                        first_column + index,
                        decode_rk(u32_le(payload, rk_offset)?),
                    )?;
                }
            }
            0x0205 => {
                if payload.len() < 8 {
                    return Err("truncated BIFF BOOLERR record".into());
                }
                let row = u16_le(payload, 0)? as usize;
                let column = u16_le(payload, 2)? as usize;
                let value = if payload[7] == 0 {
                    if payload[6] == 0 { "FALSE" } else { "TRUE" }.to_string()
                } else {
                    format!("#ERR{}", payload[6])
                };
                insert_biff_cell(&mut nonempty_rows, &mut stored_cells, row, column, value)?;
            }
            0x0006 => {
                if payload.len() >= 14 && payload[12..14] != [0xff, 0xff] {
                    let row = u16_le(payload, 0)? as usize;
                    let column = u16_le(payload, 2)? as usize;
                    let result = f64::from_bits(u64_le(payload, 6)?);
                    insert_biff_cell(
                        &mut nonempty_rows,
                        &mut stored_cells,
                        row,
                        column,
                        format_number(result),
                    )?;
                }
            }
            _ => {}
        }
    }

    Ok(project_sparse_rows(
        bound.name.clone(),
        nonempty_rows,
        stored_cells,
        max_rows,
    ))
}

fn parse_biff_workbook_stream(
    stream: &[u8],
    max_rows: usize,
) -> Result<SpreadsheetWorkbook, String> {
    let mut offset = 0usize;
    let mut bounds = Vec::<BiffSheetBound>::new();
    let mut shared_strings = Vec::<String>::new();

    while offset + 4 <= stream.len() {
        let (id, payload, next) = biff_record(stream, offset)?;
        if id == 0x0085 {
            bounds.push(parse_boundsheet(payload)?);
            if bounds.len() > MAX_WORKBOOK_SHEETS {
                return Err("BIFF workbook contains too many sheets".into());
            }
        } else if id == 0x00fc {
            let mut chunks = vec![payload];
            let mut continuation_offset = next;
            while continuation_offset + 4 <= stream.len() {
                let (continuation_id, continuation, continuation_next) =
                    biff_record(stream, continuation_offset)?;
                if continuation_id != 0x003c {
                    break;
                }
                chunks.push(continuation);
                continuation_offset = continuation_next;
            }
            shared_strings = parse_sst(&chunks)?;
            offset = continuation_offset;
            continue;
        }
        offset = next;
        if id == 0x000a {
            break;
        }
    }

    if bounds.is_empty() {
        return Err("BIFF workbook has no worksheets".into());
    }

    let mut sheets = Vec::with_capacity(bounds.len());
    for bound in &bounds {
        sheets.push(parse_biff_sheet(stream, bound, &shared_strings, max_rows)?);
    }
    Ok(SpreadsheetWorkbook { sheets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn temp_file(extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-spreadsheet-{}-{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            extension
        ))
    }

    fn biff_record_bytes(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(payload.len() + 4);
        output.extend_from_slice(&id.to_le_bytes());
        output.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        output.extend_from_slice(payload);
        output
    }

    fn sst_string(value: &str) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&(value.len() as u16).to_le_bytes());
        output.push(0);
        output.extend_from_slice(value.as_bytes());
        output
    }

    #[test]
    fn parses_real_xlsx_container_into_named_sheets() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        let files = [
            ("xl/workbook.xml", r#"<?xml version="1.0"?><workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Summary" sheetId="1" r:id="rId1"/><sheet name="Details" sheetId="2" r:id="rId2"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0"?><Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="worksheets/sheet2.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", r#"<?xml version="1.0"?><sst><si><t>Name</t></si><si><t>Alice</t></si></sst>"#),
            ("xl/worksheets/sheet1.xml", r#"<?xml version="1.0"?><worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>Count</t></is></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>42</v></c></row></sheetData></worksheet>"#),
            ("xl/worksheets/sheet2.xml", r#"<?xml version="1.0"?><worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Status</t></is></c></row><row r="2"><c r="A2" t="b"><v>1</v></c></row></sheetData></worksheet>"#),
        ];
        for (name, body) in files {
            writer.start_file(name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let path = temp_file("xlsx");
        std::fs::write(&path, bytes).unwrap();

        let workbook = parse_workbook_file(&path, 25 * 1024 * 1024, 2_000).unwrap();
        assert_eq!(workbook.sheets.len(), 2);
        assert_eq!(workbook.sheets[0].name, "Summary");
        assert_eq!(workbook.sheets[0].total_rows, 2);
        assert_eq!(
            workbook.sheets[0].rows,
            vec![
                vec!["Name".to_string(), "Count".to_string()],
                vec!["Alice".to_string(), "42".to_string()],
            ]
        );
        assert_eq!(workbook.sheets[1].name, "Details");
        assert_eq!(workbook.sheets[1].rows[1], vec!["TRUE".to_string()]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_biff8_shared_strings_and_numbers_for_legacy_xls() {
        let bof = biff_record_bytes(0x0809, &[]);
        let mut sst_payload = Vec::new();
        sst_payload.extend_from_slice(&2u32.to_le_bytes());
        sst_payload.extend_from_slice(&2u32.to_le_bytes());
        sst_payload.extend_from_slice(&sst_string("Name"));
        sst_payload.extend_from_slice(&sst_string("Alice"));
        let sst = biff_record_bytes(0x00fc, &sst_payload);
        let eof = biff_record_bytes(0x000a, &[]);

        let bounds_length = 4 + 8 + "People".len();
        let sheet_offset = bof.len() + bounds_length + sst.len() + eof.len();
        let mut bounds_payload = Vec::new();
        bounds_payload.extend_from_slice(&(sheet_offset as u32).to_le_bytes());
        bounds_payload.extend_from_slice(&[0, 0, "People".len() as u8, 0]);
        bounds_payload.extend_from_slice(b"People");
        let bounds = biff_record_bytes(0x0085, &bounds_payload);

        let mut sheet = Vec::new();
        sheet.extend_from_slice(&biff_record_bytes(0x0809, &[]));
        let mut header = vec![0, 0, 0, 0, 0, 0];
        header.extend_from_slice(&0u32.to_le_bytes());
        sheet.extend_from_slice(&biff_record_bytes(0x00fd, &header));
        let mut person = vec![1, 0, 0, 0, 0, 0];
        person.extend_from_slice(&1u32.to_le_bytes());
        sheet.extend_from_slice(&biff_record_bytes(0x00fd, &person));
        let mut number = vec![1, 0, 1, 0, 0, 0];
        number.extend_from_slice(&42f64.to_bits().to_le_bytes());
        sheet.extend_from_slice(&biff_record_bytes(0x0203, &number));
        sheet.extend_from_slice(&eof);

        let mut stream = Vec::new();
        stream.extend_from_slice(&bof);
        stream.extend_from_slice(&bounds);
        stream.extend_from_slice(&sst);
        stream.extend_from_slice(&eof);
        stream.extend_from_slice(&sheet);

        let workbook = parse_biff_workbook_stream(&stream, 2_000).unwrap();
        assert_eq!(workbook.sheets.len(), 1);
        assert_eq!(workbook.sheets[0].name, "People");
        assert_eq!(
            workbook.sheets[0].rows,
            vec![
                vec!["Name".to_string()],
                vec!["Alice".to_string(), "42".to_string()],
            ]
        );
    }

    #[test]
    fn rejects_oversize_or_malformed_workbooks() {
        let path = temp_file("xlsx");
        std::fs::write(&path, b"not-a-workbook").unwrap();
        assert!(parse_workbook_file(&path, 4, 2_000).unwrap_err().contains("byte cap"));
        assert!(parse_workbook_file(&path, 1024, 2_000).is_err());
        let _ = std::fs::remove_file(path);
    }
}
