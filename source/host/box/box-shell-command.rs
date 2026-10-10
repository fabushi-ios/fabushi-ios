#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShellArgsInput {
    pub command: String,
    pub name: String,
    pub working_directory: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableCommand {
    pub name: String,
    pub args: Vec<String>,
    pub full_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandParsingResult {
    pub parsing_failed: bool,
    pub executable_commands: Vec<ExecutableCommand>,
    pub has_redirects: bool,
    pub has_command_substitution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShellArgs {
    pub command: String,
    pub working_directory: String,
    pub tool_call_id: String,
    pub skip_approval: bool,
    pub parsing_result: ShellCommandParsingResult,
}

/// Builds the typed request sent to a Runner that owns shell capability.
///
/// This function does not execute the command. iOS production code must route
/// this descriptor to a permitted Remote Runner rather than spawning locally.
pub fn build_host_shell_args(input: HostShellArgsInput) -> HostShellArgs {
    HostShellArgs {
        parsing_result: ShellCommandParsingResult {
            parsing_failed: false,
            executable_commands: vec![ExecutableCommand {
                name: input.name,
                args: Vec::new(),
                full_text: input.command.clone(),
            }],
            has_redirects: false,
            has_command_substitution: false,
        },
        command: input.command,
        working_directory: input.working_directory,
        tool_call_id: input.tool_call_id,
        skip_approval: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_reference_equivalent_remote_runner_request() {
        let args = build_host_shell_args(HostShellArgsInput {
            command: "git status --short".into(),
            name: "git".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "tool-7".into(),
        });

        assert_eq!(args.command, "git status --short");
        assert_eq!(args.working_directory, "/workspace");
        assert_eq!(args.tool_call_id, "tool-7");
        assert!(args.skip_approval);
        assert!(!args.parsing_result.parsing_failed);
        assert!(!args.parsing_result.has_redirects);
        assert!(!args.parsing_result.has_command_substitution);
        assert_eq!(
            args.parsing_result.executable_commands,
            vec![ExecutableCommand {
                name: "git".into(),
                args: Vec::new(),
                full_text: "git status --short".into(),
            }]
        );
    }
}
