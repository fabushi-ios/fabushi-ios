#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecFailureClass {
    Other,
    SpawnEnoent,
    SpawnPermissions,
    SpawnOther,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecFailureClassification {
    pub error_class: LocalExecFailureClass,
    pub errno: Option<String>,
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn truncate_utf16(input: &str, max_units: usize) -> String {
    let mut used = 0;
    input
        .chars()
        .take_while(|ch| {
            let width = ch.len_utf16();
            let fits = used + width <= max_units;
            if fits {
                used += width;
            }
            fits
        })
        .collect()
}

/// Format bounded command output for a user-facing error without leaking ANSI
/// control sequences or allowing an unbounded child-process payload.
pub fn format_command_output_for_user(output: &str, max_chars: usize) -> String {
    let plain = strip_ansi_sequences(output);
    let mut lines = Vec::new();
    for line in plain.split('\n') {
        lines.push(line.rsplit('\r').next().unwrap_or_default());
    }
    let tail = lines
        .into_iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let truncated = truncate_utf16(&tail, max_chars);
    if truncated.encode_utf16().count() < tail.encode_utf16().count() {
        format!("…\n{truncated}")
    } else {
        truncated
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTermination {
    Timeout,
    NoOutputTimeout,
    OutputLimit,
    Exit,
    Error,
    Signal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandErrorResult {
    pub code: Option<i32>,
    pub termination: CommandTermination,
    pub signal: Option<String>,
    pub output_limit_exceeded: bool,
}

/// Build the bounded, credential-safe command failure consumed by the local-exec
/// error path. The command label is metadata only; argv and raw output are never
/// interpolated without ANSI/newline normalization and UTF-16 bounds.
pub fn format_command_error(
    command: &str,
    result: &CommandErrorResult,
    stderr: &str,
    stdout: &str,
    timeout_ms: u64,
) -> String {
    let label = truncate_utf16(
        &strip_ansi_sequences(command).replace(['\r', '\n'], " "),
        256,
    );
    let stderr = format_command_output_for_user(stderr, 2_000);
    let stdout = format_command_output_for_user(stdout, 2_000);
    let mut detail = match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("stderr: {stderr}"),
        (true, false) => format!("stdout: {stdout}"),
        (false, false) => {
            let budget = 2_000usize.saturating_sub("stderr: \nstdout: ".len());
            let first = stderr
                .len()
                .min((budget.div_ceil(2)).max(budget.saturating_sub(stdout.len())));
            let second = budget.saturating_sub(first);
            format!(
                "stderr: {}\nstdout: {}",
                suffix_fit(&stderr, first),
                suffix_fit(&stdout, second)
            )
        }
    };
    if detail.len() > 2_000 {
        detail = suffix_fit(&detail, 2_000);
    }
    let signal = result
        .signal
        .as_deref()
        .map(|value| format!("signal {value}"));
    let reason = match result.termination {
        CommandTermination::Timeout => Some(format!(
            "timed out after {} seconds",
            timeout_ms as f64 / 1000.0
        )),
        CommandTermination::NoOutputTimeout => Some("timed out waiting for output".to_owned()),
        CommandTermination::OutputLimit => Some("output limit exceeded".to_owned()),
        CommandTermination::Exit | CommandTermination::Error => result
            .output_limit_exceeded
            .then_some("output limit exceeded".to_owned())
            .or_else(|| result.code.map(|code| format!("exit code {code}"))),
        CommandTermination::Signal => result
            .output_limit_exceeded
            .then_some("output limit exceeded".to_owned())
            .or_else(|| (!signal.is_some()).then_some("terminated".to_owned())),
    };
    let reason = [reason, signal]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{label} failed{}{}",
        (!reason.is_empty())
            .then(|| format!(" ({reason})"))
            .unwrap_or_default(),
        (!detail.is_empty())
            .then(|| format!(":\n{detail}"))
            .unwrap_or_default()
    )
}

fn suffix_fit(text: &str, max_chars: usize) -> String {
    if text.encode_utf16().count() <= max_chars {
        return text.to_owned();
    }
    if max_chars <= 2 {
        return truncate_utf16(text, max_chars);
    }
    format!("…\n{}", truncate_utf16(text, max_chars - 2))
}

fn errno_token(message: &str) -> Option<String> {
    message
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .find(|token| {
            token.len() >= 2
                && token.starts_with('E')
                && token
                    .chars()
                    .skip(1)
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        })
        .map(ToOwned::to_owned)
}

pub fn classify_local_exec_failure(message: &str) -> LocalExecFailureClassification {
    let lower = message.to_ascii_lowercase();
    let is_spawn = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == "spawn" || token == "spawnsync");
    let errno = errno_token(message);
    let error_class = if !is_spawn {
        LocalExecFailureClass::Other
    } else {
        match errno.as_deref() {
            Some("ENOENT") => LocalExecFailureClass::SpawnEnoent,
            Some("EACCES" | "EPERM") => LocalExecFailureClass::SpawnPermissions,
            _ => LocalExecFailureClass::SpawnOther,
        }
    };
    LocalExecFailureClassification { error_class, errno }
}
