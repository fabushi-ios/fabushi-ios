pub const UTF8_ENCODING: &str = "utf8";
pub const UTF8_BOM_ENCODING: &str = "utf8bom";
pub const LATIN1_ENCODING: &str = "latin1";

pub fn map_to_iconv_encoding(encoding: &str) -> &str {
    match encoding {
        "windows-1250"=>"cp1250","windows-1251"=>"cp1251","windows-1252"=>"cp1252","windows-1253"=>"cp1253",
        "windows-1254"=>"cp1254","windows-1255"=>"cp1255","windows-1256"=>"cp1256","windows-1257"=>"cp1257",
        "windows-1258"=>"cp1258","iso-8859-1"=>"iso88591","iso-8859-2"=>"iso88592","iso-8859-5"=>"iso88595",
        "iso-8859-6"=>"iso88596","iso-8859-7"=>"iso88597","iso-8859-8"=>"iso88598","iso-8859-9"=>"iso88599",
        "iso-8859-15"=>"iso885915","iso-8859-16"=>"iso885916","shift_jis"=>"shiftjis","euc-jp"=>"eucjp",
        "euc-kr"=>"euckr","iso-2022-jp"=>"iso2022jp","iso-2022-kr"=>"iso2022kr","gb2312"=>"gb2312",
        "gbk"=>"gbk","gb18030"=>"gb18030","big5"=>"big5","big5-hkscs"=>"big5hkscs","koi8-r"=>"koi8r",
        "koi8-u"=>"koi8u","ibm855"=>"cp855","ibm866"=>"cp866","maccyrillic"=>"maccyrillic","utf-16le"=>"utf16le",
        "utf-16be"=>"utf16be","utf-32le"=>"utf32le","utf-32be"=>"utf32be","johab"=>"johab","cp949"=>"cp949","cp932"=>"cp932",
        _=>encoding,
    }
}

pub fn strip_utf8_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn normalize_encoding_name(encoding: &str) -> String {
    let normalized=encoding.to_lowercase();
    match normalized.as_str() {
        "latin-1"=>LATIN1_ENCODING.to_owned(),
        "utf-8"=>UTF8_ENCODING.to_owned(),
        "utf-8-bom"|"utf-8 bom"=>UTF8_BOM_ENCODING.to_owned(),
        _=>normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_reference_iconv_aliases_and_preserves_unknown_values() {
        assert_eq!(map_to_iconv_encoding("windows-1252"),"cp1252");
        assert_eq!(map_to_iconv_encoding("big5-hkscs"),"big5hkscs");
        assert_eq!(map_to_iconv_encoding("utf-32be"),"utf32be");
        assert_eq!(map_to_iconv_encoding("custom"),"custom");
    }

    #[test]
    fn normalizes_reference_utf_and_latin_aliases() {
        assert_eq!(normalize_encoding_name("UTF-8"),"utf8");
        assert_eq!(normalize_encoding_name("UTF-8 BOM"),"utf8bom");
        assert_eq!(normalize_encoding_name("Latin-1"),"latin1");
        assert_eq!(normalize_encoding_name("SHIFT_JIS"),"shift_jis");
    }

    #[test]
    fn strips_only_one_leading_utf8_bom() {
        assert_eq!(strip_utf8_bom("\u{feff}hello"),"hello");
        assert_eq!(strip_utf8_bom("hello\u{feff}"),"hello\u{feff}");
    }
}
