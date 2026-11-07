use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedImageInput {
    pub data: Vec<u8>,
    pub path: String,
    pub mime_type: Option<&'static str>,
}

pub fn image_mime_from_path(file_path: &str) -> Option<&'static str> {
    let base = file_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_path);
    let dot = base.rfind('.')?;
    if dot == 0 {
        return None;
    }
    match base[dot..].to_ascii_lowercase().as_str() {
        ".avif" => Some("image/avif"),
        ".bmp" => Some("image/bmp"),
        ".gif" => Some("image/gif"),
        ".ico" => Some("image/x-icon"),
        ".jpeg" | ".jpg" => Some("image/jpeg"),
        ".png" => Some("image/png"),
        ".svg" => Some("image/svg+xml"),
        ".webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn video_mime_from_path(file_path: &str) -> Option<&'static str> {
    let base = file_path.rsplit(['/', '\\']).next().unwrap_or(file_path);
    let dot = base.rfind('.')?;
    if dot == 0 {
        return None;
    }
    match base[dot..].to_ascii_lowercase().as_str() {
        ".m4v" | ".mp4" => Some("video/mp4"),
        ".mov" => Some("video/quicktime"),
        ".ogv" => Some("video/ogg"),
        ".webm" => Some("video/webm"),
        _ => None,
    }
}

pub fn read_image_file_dimensions(path: &str) -> Option<MediaDimensions> {
    let bytes = fs::read(path).ok()?;
    read_image_dimensions(&bytes)
}

pub fn read_image_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    read_webp_or_heic_dimensions(bytes)
        .or_else(|| read_png_dimensions(bytes))
        .or_else(|| read_gif_dimensions(bytes))
        .or_else(|| read_jpeg_dimensions(bytes))
}

fn dimensions(width: u32, height: u32) -> Option<MediaDimensions> {
    (width > 0 && height > 0).then_some(MediaDimensions { width, height })
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?]))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?]))
}

fn read_u24_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(
        u32::from(*bytes.get(offset)?)
            | (u32::from(*bytes.get(offset + 1)?) << 8)
            | (u32::from(*bytes.get(offset + 2)?) << 16),
    )
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn tag(bytes: &[u8], offset: usize) -> Option<&[u8]> {
    bytes.get(offset..offset + 4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IsoBox {
    kind: [u8; 4],
    body: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrimaryImageProperty {
    ispe_body: usize,
    quarter_turns: u8,
}

fn index_of_four_char_tag(bytes: &[u8], expected: &[u8; 4]) -> Option<usize> {
    bytes.windows(4).position(|window| window == expected)
}

fn iso_boxes(bytes: &[u8], start: usize, end: usize) -> Vec<IsoBox> {
    if start > end || end > bytes.len() {
        return Vec::new();
    }

    let mut boxes = Vec::new();
    let mut offset = start;
    while offset.checked_add(8).is_some_and(|value| value <= end) {
        let Some(raw_size) = read_u32_be(bytes, offset) else {
            break;
        };
        let mut size = raw_size as usize;
        let mut header = 8usize;
        if size == 1 {
            if offset.checked_add(16).is_none_or(|value| value > end) {
                break;
            }
            let Some(extended_size) = read_u32_be(bytes, offset + 12) else {
                break;
            };
            size = extended_size as usize;
            header = 16;
        } else if size == 0 {
            size = end - offset;
        }

        let Some(box_end) = offset.checked_add(size) else {
            break;
        };
        if size < header || box_end > end {
            break;
        }
        let Some(kind_bytes) = bytes.get(offset + 4..offset + 8) else {
            break;
        };
        let Ok(kind) = <[u8; 4]>::try_from(kind_bytes) else {
            break;
        };
        boxes.push(IsoBox {
            kind,
            body: offset + header,
            end: box_end,
        });
        offset = box_end;
    }
    boxes
}

fn find_iso_box(boxes: &[IsoBox], kind: &[u8; 4]) -> Option<IsoBox> {
    boxes.iter().copied().find(|box_| box_.kind == *kind)
}

fn primary_item_id(bytes: &[u8], pitm: IsoBox) -> Option<u32> {
    if pitm.body.checked_add(6)? > pitm.end {
        return None;
    }
    let version = *bytes.get(pitm.body)?;
    let at = pitm.body.checked_add(4)?;
    if version == 0 {
        return Some(u32::from(read_u16_be(bytes, at)?));
    }
    if at.checked_add(4)? > pitm.end {
        return None;
    }
    read_u32_be(bytes, at)
}

fn item_property_indices(bytes: &[u8], ipma: IsoBox, item_id: u32) -> Option<Vec<usize>> {
    let mut offset = ipma.body;
    if offset.checked_add(8)? > ipma.end {
        return None;
    }
    let version = *bytes.get(offset)?;
    let flags = (u32::from(*bytes.get(offset + 1)?) << 16)
        | (u32::from(*bytes.get(offset + 2)?) << 8)
        | u32::from(*bytes.get(offset + 3)?);
    offset += 4;
    let entry_count = read_u32_be(bytes, offset)?;
    offset += 4;
    let id_bytes = if version >= 1 { 4usize } else { 2usize };
    let index_is_16 = (flags & 1) == 1;

    for _ in 0..entry_count {
        if offset.checked_add(id_bytes + 1)? > ipma.end {
            return None;
        }
        let id = if id_bytes == 4 {
            read_u32_be(bytes, offset)?
        } else {
            u32::from(read_u16_be(bytes, offset)?)
        };
        offset += id_bytes;
        let association_count = usize::from(*bytes.get(offset)?);
        offset += 1;
        let mut indices = Vec::with_capacity(association_count);
        for _ in 0..association_count {
            if index_is_16 {
                if offset.checked_add(2)? > ipma.end {
                    return None;
                }
                indices.push(usize::from(read_u16_be(bytes, offset)? & 0x7fff));
                offset += 2;
            } else {
                if offset.checked_add(1)? > ipma.end {
                    return None;
                }
                indices.push(usize::from(*bytes.get(offset)? & 0x7f));
                offset += 1;
            }
        }
        if id == item_id {
            return Some(indices);
        }
    }
    None
}

fn fallback_primary_image(
    bytes: &[u8],
    first_ispe: Option<usize>,
    first_irot: Option<usize>,
) -> Option<PrimaryImageProperty> {
    let first_ispe = first_ispe?;
    if first_ispe.checked_add(16)? > bytes.len() {
        return None;
    }
    let quarter_turns = first_irot
        .and_then(|offset| offset.checked_add(4))
        .and_then(|offset| bytes.get(offset))
        .map_or(0, |value| value & 3);
    Some(PrimaryImageProperty {
        ispe_body: first_ispe + 8,
        quarter_turns,
    })
}

fn select_primary_image(bytes: &[u8]) -> Option<PrimaryImageProperty> {
    let first_ispe = index_of_four_char_tag(bytes, b"ispe");
    let first_irot = index_of_four_char_tag(bytes, b"irot");
    let fallback = || fallback_primary_image(bytes, first_ispe, first_irot);

    let top_level = iso_boxes(bytes, 0, bytes.len());
    let Some(meta) = find_iso_box(&top_level, b"meta") else {
        return fallback();
    };
    let Some(meta_start) = meta.body.checked_add(4) else {
        return fallback();
    };
    if meta_start > meta.end {
        return fallback();
    }
    let meta_boxes = iso_boxes(bytes, meta_start, meta.end);
    let Some(pitm) = find_iso_box(&meta_boxes, b"pitm") else {
        return fallback();
    };
    let Some(iprp) = find_iso_box(&meta_boxes, b"iprp") else {
        return fallback();
    };
    let iprp_boxes = iso_boxes(bytes, iprp.body, iprp.end);
    let Some(ipco) = find_iso_box(&iprp_boxes, b"ipco") else {
        return fallback();
    };
    let Some(ipma) = find_iso_box(&iprp_boxes, b"ipma") else {
        return fallback();
    };
    let properties = iso_boxes(bytes, ipco.body, ipco.end);
    let Some(primary_id) = primary_item_id(bytes, pitm) else {
        return fallback();
    };
    let Some(indices) = item_property_indices(bytes, ipma, primary_id) else {
        return fallback();
    };

    let mut ispe_body = None;
    let mut quarter_turns = 0u8;
    for index in indices {
        let Some(property_index) = index.checked_sub(1) else {
            continue;
        };
        let Some(property) = properties.get(property_index).copied() else {
            continue;
        };
        if property.kind == *b"ispe" && ispe_body.is_none() {
            if property.body.checked_add(12).is_some_and(|value| value <= property.end) {
                ispe_body = property.body.checked_add(4);
            }
        } else if property.kind == *b"irot" && property.body < property.end {
            quarter_turns = *bytes.get(property.body)? & 3;
        }
    }

    ispe_body
        .map(|ispe_body| PrimaryImageProperty {
            ispe_body,
            quarter_turns,
        })
        .or_else(fallback)
}

fn read_heic_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    if bytes.len() < 12 || tag(bytes, 4)? != b"ftyp" {
        return None;
    }
    let selected = select_primary_image(bytes)?;
    let width = read_u32_be(bytes, selected.ispe_body)?;
    let height = read_u32_be(bytes, selected.ispe_body.checked_add(4)?)?;
    if matches!(selected.quarter_turns, 1 | 3) {
        dimensions(height, width)
    } else {
        dimensions(width, height)
    }
}

fn read_webp_or_heic_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    read_webp_dimensions(bytes).or_else(|| read_heic_dimensions(bytes))
}

fn read_png_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || bytes.get(..8)? != SIGNATURE || tag(bytes, 12)? != b"IHDR" {
        return None;
    }
    dimensions(read_u32_be(bytes, 16)?, read_u32_be(bytes, 20)?)
}

fn read_gif_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    if bytes.len() < 10
        || !matches!(bytes.get(..6), Some(b"GIF87a") | Some(b"GIF89a"))
    {
        return None;
    }
    dimensions(
        u32::from(read_u16_le(bytes, 6)?),
        u32::from(read_u16_le(bytes, 8)?),
    )
}

fn read_jpeg_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut offset = 2usize;
    while offset + 3 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = bytes[offset + 1];
        if marker == 0xff {
            offset += 1;
            continue;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd9) {
            offset += 2;
            continue;
        }
        if marker == 0xda {
            return None;
        }
        let segment_len = usize::from(read_u16_be(bytes, offset + 2)?);
        if segment_len < 2 {
            return None;
        }
        let frame = (0xc0..=0xcf).contains(&marker)
            && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if frame {
            if offset + 9 > bytes.len() {
                return None;
            }
            return dimensions(
                u32::from(read_u16_be(bytes, offset + 7)?),
                u32::from(read_u16_be(bytes, offset + 5)?),
            );
        }
        offset = offset.checked_add(2 + segment_len)?;
    }
    None
}

fn read_webp_dimensions(bytes: &[u8]) -> Option<MediaDimensions> {
    if bytes.len() < 30 || tag(bytes, 0)? != b"RIFF" || tag(bytes, 8)? != b"WEBP" {
        return None;
    }
    match tag(bytes, 12)? {
        b"VP8 " => dimensions(
            u32::from(read_u16_le(bytes, 26)? & 0x3fff),
            u32::from(read_u16_le(bytes, 28)? & 0x3fff),
        ),
        b"VP8L" => {
            let packed = read_u32_le(bytes, 21)?;
            dimensions((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => dimensions(
            read_u24_le(bytes, 24)? + 1,
            read_u24_le(bytes, 27)? + 1,
        ),
        _ => None,
    }
}

pub fn load_selected_image_inputs<I, S>(attachment_paths: I) -> Vec<SelectedImageInput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    attachment_paths
        .into_iter()
        .filter_map(|path| {
            let path = path.as_ref();
            let data = fs::read(path).ok()?;
            Some(SelectedImageInput {
                data,
                path: path.to_string(),
                mime_type: image_mime_from_path(path),
            })
        })
        .collect()
}
