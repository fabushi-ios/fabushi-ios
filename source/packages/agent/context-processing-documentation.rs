#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentationChunk {
    pub doc_name: String,
    pub page_url: String,
    pub documentation_chunk: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentationResult {
    pub chunks: Vec<DocumentationChunk>,
}

pub fn render_documentation_context(result: Option<&DocumentationResult>) -> Option<String> {
    let result = result?;
    if result.chunks.is_empty() {
        return None;
    }

    let mut output = String::from(
        "<documentation_context>\n## Potentially Relevant Documentation:\n-------\n",
    );
    for chunk in &result.chunks {
        output.push_str("Document Name: ");
        output.push_str(&chunk.doc_name);
        output.push_str("\nDocument URL: ");
        output.push_str(&chunk.page_url);
        output.push_str("\nDocument content:\n");
        output.push_str(&chunk.documentation_chunk);
        output.push_str("\n____\n\n");
    }
    output.push_str("</documentation_context>");
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_documentation_is_omitted() {
        assert_eq!(render_documentation_context(None), None);
        assert_eq!(
            render_documentation_context(Some(&DocumentationResult::default())),
            None
        );
    }

    #[test]
    fn renders_reference_documentation_envelope_in_order() {
        let result = DocumentationResult {
            chunks: vec![
                DocumentationChunk {
                    doc_name: "One".into(),
                    page_url: "https://example.com/one".into(),
                    documentation_chunk: "first".into(),
                },
                DocumentationChunk {
                    doc_name: "Two".into(),
                    page_url: "https://example.com/two".into(),
                    documentation_chunk: "second".into(),
                },
            ],
        };
        let text = render_documentation_context(Some(&result)).unwrap();
        assert!(text.starts_with("<documentation_context>\n## Potentially Relevant Documentation:"));
        assert!(text.contains("Document Name: One\nDocument URL: https://example.com/one"));
        assert!(text.contains("Document Name: Two\nDocument URL: https://example.com/two"));
        assert!(text.ends_with("</documentation_context>"));
        assert!(text.find("Document Name: One").unwrap() < text.find("Document Name: Two").unwrap());
    }
}
