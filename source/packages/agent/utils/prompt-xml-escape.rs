pub fn escape_prompt_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_ampersand_before_angle_brackets_like_grok() {
        assert_eq!(
            escape_prompt_xml_text("a<&>b &amp;"),
            "a&lt;&amp;&gt;b &amp;amp;"
        );
    }
}
