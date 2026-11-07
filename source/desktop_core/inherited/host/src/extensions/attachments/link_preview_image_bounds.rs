use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSize { pub width: u32, pub height: u32 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImageBounds<E> { pub max_dimension: u32, pub encoding: E }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeTarget { Width(u32), Height(u32) }

pub const LINK_PREVIEW_MAX_DECODE_PIXELS: u64 = 24_000_000;
const DIB_HEADER_SIZES: &[u32] = &[40, 52, 56, 108, 124];

#[derive(Debug, Clone, Copy)]
struct IsoBox { kind: [u8; 4], body: usize, end: usize }

fn size(width: u32, height: u32) -> Option<ImageSize> {
    (width > 0 && height > 0).then_some(ImageSize { width, height })
}
fn u16be(b:&[u8],o:usize)->Option<u16>{let s=b.get(o..o.checked_add(2)?)?;Some(u16::from_be_bytes([s[0],s[1]]))}
fn u16le(b:&[u8],o:usize)->Option<u16>{let s=b.get(o..o.checked_add(2)?)?;Some(u16::from_le_bytes([s[0],s[1]]))}
fn u24le(b:&[u8],o:usize)->Option<u32>{let s=b.get(o..o.checked_add(3)?)?;Some(u32::from(s[0])|(u32::from(s[1])<<8)|(u32::from(s[2])<<16))}
fn u32be(b:&[u8],o:usize)->Option<u32>{let s=b.get(o..o.checked_add(4)?)?;Some(u32::from_be_bytes([s[0],s[1],s[2],s[3]]))}
fn u32le(b:&[u8],o:usize)->Option<u32>{let s=b.get(o..o.checked_add(4)?)?;Some(u32::from_le_bytes([s[0],s[1],s[2],s[3]]))}
fn i32le(b:&[u8],o:usize)->Option<i32>{let s=b.get(o..o.checked_add(4)?)?;Some(i32::from_le_bytes([s[0],s[1],s[2],s[3]]))}
fn tag(b:&[u8],o:usize)->Option<[u8;4]>{let s=b.get(o..o.checked_add(4)?)?;Some([s[0],s[1],s[2],s[3]])}
fn index_tag(b:&[u8],needle:[u8;4])->Option<usize>{b.windows(4).position(|w|w==needle.as_slice())}

fn boxes(bytes:&[u8],start:usize,end:usize)->Vec<IsoBox>{
    let mut out=Vec::new(); let mut offset=start;
    while offset.saturating_add(8)<=end {
        let Some(mut n)=u32be(bytes,offset).map(u64::from) else {break};
        let mut header=8u64;
        if n==1 {
            if offset.saturating_add(16)>end {break}
            let Some(low)=u32be(bytes,offset+12) else {break}; n=u64::from(low); header=16;
        } else if n==0 { n=u64::try_from(end.saturating_sub(offset)).unwrap_or(u64::MAX); }
        let Ok(nu)=usize::try_from(n) else {break}; let Ok(hu)=usize::try_from(header) else {break};
        let Some(box_end)=offset.checked_add(nu) else {break};
        if nu<hu||box_end>end {break}
        let Some(kind)=tag(bytes,offset+4) else {break};
        out.push(IsoBox{kind,body:offset+hu,end:box_end}); offset=box_end;
    }
    out
}
fn primary_id(bytes:&[u8],pitm:IsoBox)->Option<u32>{
    if pitm.body.saturating_add(6)>pitm.end{return None}
    let version=*bytes.get(pitm.body)?; let at=pitm.body+4;
    if version==0 {u16be(bytes,at).map(u32::from)}
    else {if at.saturating_add(4)>pitm.end{return None};u32be(bytes,at)}
}
fn property_indices(bytes:&[u8],ipma:IsoBox,item:u32)->Option<Vec<usize>>{
    let mut o=ipma.body; if o.saturating_add(8)>ipma.end{return None}
    let version=*bytes.get(o)?;
    let flags=(u32::from(*bytes.get(o+1)?)<<16)|(u32::from(*bytes.get(o+2)?)<<8)|u32::from(*bytes.get(o+3)?);
    o+=4; let entries=u32be(bytes,o)?;o+=4; let id_bytes=if version>=1{4}else{2}; let wide=(flags&1)==1;
    for _ in 0..entries {
        if o.saturating_add(id_bytes+1)>ipma.end{return None}
        let id=if id_bytes==4{u32be(bytes,o)?}else{u32::from(u16be(bytes,o)?)};o+=id_bytes;
        let count=usize::from(*bytes.get(o)?);o+=1;let mut indices=Vec::with_capacity(count);
        for _ in 0..count {
            if wide {if o.saturating_add(2)>ipma.end{return None};indices.push(usize::from(u16be(bytes,o)?&32767));o+=2}
            else {if o.saturating_add(1)>ipma.end{return None};indices.push(usize::from(*bytes.get(o)?&127));o+=1}
        }
        if id==item{return Some(indices)}
    }
    None
}
fn select_heic(bytes:&[u8])->Option<(usize,u8)>{
    let first_ispe=index_tag(bytes,*b"ispe");let first_irot=index_tag(bytes,*b"irot");
    let fallback=||{
        let at=first_ispe?;if at.saturating_add(16)>bytes.len(){return None}
        let turns=first_irot.and_then(|p|bytes.get(p+4).copied()).map(|v|v&3).unwrap_or(0);
        Some((at+8,turns))
    };
    let top=boxes(bytes,0,bytes.len());let Some(meta)=top.iter().copied().find(|x|x.kind==*b"meta") else{return fallback()};
    let children=boxes(bytes,meta.body.saturating_add(4),meta.end);
    let Some(pitm)=children.iter().copied().find(|x|x.kind==*b"pitm") else{return fallback()};
    let Some(iprp)=children.iter().copied().find(|x|x.kind==*b"iprp") else{return fallback()};
    let iprp_children=boxes(bytes,iprp.body,iprp.end);
    let Some(ipco)=iprp_children.iter().copied().find(|x|x.kind==*b"ipco") else{return fallback()};
    let Some(ipma)=iprp_children.iter().copied().find(|x|x.kind==*b"ipma") else{return fallback()};
    let props=boxes(bytes,ipco.body,ipco.end);let Some(id)=primary_id(bytes,pitm) else{return fallback()};
    let Some(indices)=property_indices(bytes,ipma,id) else{return fallback()};
    let mut ispe=None;let mut turns=0;
    for index in indices {
        let Some(prop)=index.checked_sub(1).and_then(|i|props.get(i)).copied() else{continue};
        if prop.kind==*b"ispe"&&ispe.is_none()&&prop.body.saturating_add(12)<=prop.end {ispe=Some(prop.body+4)}
        else if prop.kind==*b"irot"&&prop.body<prop.end {turns=*bytes.get(prop.body).unwrap_or(&0)&3}
    }
    ispe.map(|body|(body,turns)).or_else(fallback)
}
pub fn read_heic_dimensions(bytes:&[u8])->Option<ImageSize>{
    if bytes.len()<12||tag(bytes,4)?!=*b"ftyp"{return None}
    let (body,turns)=select_heic(bytes)?;let w=u32be(bytes,body)?;let h=u32be(bytes,body+4)?;
    if matches!(turns,1|3){size(h,w)}else{size(w,h)}
}
pub fn read_webp_dimensions(bytes:&[u8])->Option<ImageSize>{
    if bytes.len()<30||tag(bytes,0)?!=*b"RIFF"||tag(bytes,8)?!=*b"WEBP"{return None}
    match tag(bytes,12)? {
        v if v==*b"VP8 "=>size(u32::from(u16le(bytes,26)?&16383),u32::from(u16le(bytes,28)?&16383)),
        v if v==*b"VP8L"=>{let p=u32le(bytes,21)?;size((p&16383)+1,((p>>14)&16383)+1)}
        v if v==*b"VP8X"=>size(u24le(bytes,24)?+1,u24le(bytes,27)?+1),
        _=>None
    }
}
pub fn read_webp_or_heic_dimensions(bytes:&[u8])->Option<ImageSize>{read_webp_dimensions(bytes).or_else(||read_heic_dimensions(bytes))}
pub fn read_png_dimensions(bytes:&[u8])->Option<ImageSize>{
    const SIG:&[u8;8]=&[137,80,78,71,13,10,26,10];
    if bytes.len()<24||bytes.get(..8)?!=SIG||tag(bytes,12)?!=*b"IHDR"{return None}
    size(u32be(bytes,16)?,u32be(bytes,20)?)
}
pub fn read_gif_dimensions(bytes:&[u8])->Option<ImageSize>{
    if bytes.len()<10||!matches!(bytes.get(..6)?,b"GIF87a"|b"GIF89a"){return None}
    size(u32::from(u16le(bytes,6)?),u32::from(u16le(bytes,8)?))
}
pub fn read_jpeg_dimensions(bytes:&[u8])->Option<ImageSize>{
    if bytes.len()<4||bytes[0]!=255||bytes[1]!=216{return None}
    let mut o=2usize;
    while o.saturating_add(3)<bytes.len(){
        if bytes[o]!=255{o+=1;continue}
        let marker=bytes[o+1];if marker==255{o+=1;continue}
        if marker==1||(208..=217).contains(&marker){o+=2;continue}
        if marker==218{return None}
        let len=usize::from(u16be(bytes,o+2)?);if len<2{return None}
        if (192..=207).contains(&marker)&&!matches!(marker,196|200|204){
            if o.saturating_add(9)>bytes.len(){return None}
            return size(u32::from(u16be(bytes,o+7)?),u32::from(u16be(bytes,o+5)?))
        }
        o=o.checked_add(2+len)?;
    }
    None
}
fn ico_bitmap(bytes:&[u8],o:usize)->Option<ImageSize>{
    if o.saturating_add(12)>bytes.len()||!DIB_HEADER_SIZES.contains(&u32le(bytes,o)?){return None}
    // BITMAPINFOHEADER-compatible DIBs store biSize at +0, biWidth at +4, and biHeight at +8.
    // ICO/CUR DIB height includes the XOR and AND masks, so the visible height is half of abs(biHeight).
    let w=i64::from(i32le(bytes,o+4)?);let raw_h=i64::from(i32le(bytes,o+8)?).abs();let h=(raw_h+1)/2;
    if w<=0||h<=0{return None} size(u32::try_from(w).ok()?,u32::try_from(h).ok()?)
}
pub fn read_ico_size(bytes:&[u8])->Option<ImageSize>{
    if bytes.len()<6||u16le(bytes,0)?!=0{return None}
    let kind=u16le(bytes,2)?;let count=usize::from(u16le(bytes,4)?);
    if !matches!(kind,1|2)||count==0||bytes.len()<6usize.saturating_add(count.saturating_mul(16)){return None}
    let mut best=None;let mut pixels=0u64;
    for i in 0..count {
        let e=6+i*16;let payload=usize::try_from(u32le(bytes,e+12)?).ok()?;
        let side=|v:u8|if v==0{256}else{u32::from(v)};
        let candidates=[Some(ImageSize{width:side(bytes[e]),height:side(bytes[e+1])}),
            (payload<bytes.len()).then(||read_png_dimensions(&bytes[payload..])).flatten(),ico_bitmap(bytes,payload)];
        for c in candidates.into_iter().flatten(){let p=u64::from(c.width)*u64::from(c.height);if p>pixels{pixels=p;best=Some(c)}}
    }
    best
}
pub fn decode_base64_data_url(data_url:&str)->Option<Vec<u8>>{
    if !data_url.starts_with("data:"){return None}let comma=data_url.find(',')?;
    if !data_url.get(..comma)?.ends_with(";base64"){return None}
    let compact=data_url.get(comma+1..)?.chars().filter(|c|!c.is_ascii_whitespace()).collect::<String>();
    STANDARD.decode(compact).ok()
}
pub fn read_encoded_image_size(data_url:&str)->Option<ImageSize>{
    let bytes=decode_base64_data_url(data_url)?;
    read_png_dimensions(&bytes).or_else(||read_jpeg_dimensions(&bytes)).or_else(||read_gif_dimensions(&bytes))
        .or_else(||read_ico_size(&bytes)).or_else(||read_webp_or_heic_dimensions(&bytes))
}
pub fn bound_preview_image_data_url<E,F>(data_url:Option<&str>,bounds:&PreviewImageBounds<E>,resize:F)->Option<String>
where F:FnOnce(&str,ResizeTarget,&E)->Option<String>{
    let original=data_url?;let image=read_encoded_image_size(original)?;
    if u64::from(image.width)*u64::from(image.height)>LINK_PREVIEW_MAX_DECODE_PIXELS{return None}
    if image.width.max(image.height)<=bounds.max_dimension{return Some(original.into())}
    let target=if image.width>=image.height{ResizeTarget::Width(bounds.max_dimension)}else{ResizeTarget::Height(bounds.max_dimension)};
    resize(original,target,&bounds.encoding).or_else(||Some(original.into()))
}
