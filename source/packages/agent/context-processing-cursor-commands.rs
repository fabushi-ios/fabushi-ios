#[derive(Debug, Clone, Copy)]
pub struct SelectedCursorCommandForPrompt<'a> {
    pub name: &'a str,
    pub content: &'a str,
}

pub fn render_selected_cursor_commands(
    cursor_commands: &[SelectedCursorCommandForPrompt<'_>],
) -> Option<String> {
    if cursor_commands.is_empty() {
        return None;
    }

    let commands_text = cursor_commands
        .iter()
        .map(|command| {
            format!(
                "\n\n--- Cursor Command: {} ---\n{}\n--- End Command ---",
                command.name, command.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if commands_text.is_empty() {
        return None;
    }

    Some(format!(
        "<cursor_commands>{commands_text}\n</cursor_commands>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_selected_commands_with_pinned_delimiters() {
        assert_eq!(render_selected_cursor_commands(&[]), None);

        let rendered = render_selected_cursor_commands(&[
            SelectedCursorCommandForPrompt {
                name: "one",
                content: "first",
            },
            SelectedCursorCommandForPrompt {
                name: "two",
                content: "second",
            },
        ])
        .unwrap();

        assert_eq!(
            rendered,
            "<cursor_commands>\n\n--- Cursor Command: one ---\nfirst\n--- End Command ---\n\n\n--- Cursor Command: two ---\nsecond\n--- End Command ---\n</cursor_commands>"
        );
    }
}
