#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedDocumentInfo {
    pub path: String,
}

pub fn render_uploaded_documents_context(
    documents: &[UploadedDocumentInfo],
) -> Option<String> {
    if documents.is_empty() {
        return None;
    }

    let documents_list = documents
        .iter()
        .map(|document| format!("- {}", document.path))
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "<uploaded_documents>\nThe following documents have been saved to your filesystem. You can read them using your file-reading tool or other tools:\n{documents_list}\n</uploaded_documents>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_only_non_empty_document_lists() {
        assert_eq!(render_uploaded_documents_context(&[]), None);

        let rendered = render_uploaded_documents_context(&[
            UploadedDocumentInfo {
                path: "/tmp/a.pdf".to_owned(),
            },
            UploadedDocumentInfo {
                path: "/tmp/b.txt".to_owned(),
            },
        ])
        .unwrap();

        assert_eq!(
            rendered,
            "<uploaded_documents>\nThe following documents have been saved to your filesystem. You can read them using your file-reading tool or other tools:\n- /tmp/a.pdf\n- /tmp/b.txt\n</uploaded_documents>"
        );
    }
}
