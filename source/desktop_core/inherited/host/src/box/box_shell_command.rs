#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShellArgsInput {
    pub command: String,
    pub name: String,
    pub working_directory: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandExecutable {
    pub name: String,
    pub args: Vec<String>,
    pub full_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandParsingResult {
    pub parsing_failed: bool,
    pub executable_commands: Vec<ShellCommandExecutable>,
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

pub fn build_host_shell_args(input: HostShellArgsInput) -> HostShellArgs {
    let full_text = input.command.clone();
    HostShellArgs {
        command: input.command,
        working_directory: input.working_directory,
        tool_call_id: input.tool_call_id,
        skip_approval: true,
        parsing_result: ShellCommandParsingResult {
            parsing_failed: false,
            executable_commands: vec![ShellCommandExecutable {
                name: input.name,
                args: Vec::new(),
                full_text,
            }],
            has_redirects: false,
            has_command_substitution: false,
        },
    }
}
