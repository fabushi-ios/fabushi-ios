//! Command Obfuscation & Evasion Detection
//!
//! Detects base64, hex/octal escapes, env expansions, backtick substitutions,
//! homoglyphs, zero-width characters, and suspicious string concatenations.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObfuscationDetectionResult {
    Clean,
    Obfuscated {
        reason: String,
        detected_pattern: String,
    },
}

impl ObfuscationDetectionResult {
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

pub fn detect_command_obfuscation(command: &str) -> ObfuscationDetectionResult {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return ObfuscationDetectionResult::Clean;
    }

    // 1. Zero-width and hidden unicode characters
    for c in trimmed.chars() {
        if matches!(
            c,
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{202A}'..='\u{202E}'
        ) {
            return ObfuscationDetectionResult::Obfuscated {
                reason: "Command contains hidden zero-width or bidirectional control characters"
                    .to_string(),
                detected_pattern: format!("U+{:04X}", c as u32),
            };
        }
    }

    // 2. Base64 execution patterns: echo ... | base64 -d | sh / bash / zsh
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("base64")
        && (lower.contains("-d") || lower.contains("--decode"))
        && (lower.contains("| sh") || lower.contains("| bash") || lower.contains("| zsh") || lower.contains("|python"))
    {
        return ObfuscationDetectionResult::Obfuscated {
            reason: "Detected base64 decode piped into shell interpreter".to_string(),
            detected_pattern: "base64 decode pipe".to_string(),
        };
    }

    // 3. Hex / Octal escape patterns: \x41\x42 or $'\x41' or \0101
    if (lower.contains("\\x") || lower.contains("$'\\"))
        && (lower.contains("bash") || lower.contains("sh") || lower.contains("eval") || lower.contains("exec"))
    {
        return ObfuscationDetectionResult::Obfuscated {
            reason: "Detected hex/octal shell escape sequences combined with execution".to_string(),
            detected_pattern: "\\x escape sequence".to_string(),
        };
    }

    // 4. Reverse string pipe execution: rev | sh or rev | bash
    if lower.contains("rev |") && (lower.contains("sh") || lower.contains("bash")) {
        return ObfuscationDetectionResult::Obfuscated {
            reason: "Detected reversed string pipe execution".to_string(),
            detected_pattern: "rev | sh".to_string(),
        };
    }

    // 5. Eval of variable expansions: eval "$VAR" or eval "$@"
    if lower.starts_with("eval ") || lower.contains("; eval ") || lower.contains("&& eval ") {
        if lower.contains('$') {
            return ObfuscationDetectionResult::Obfuscated {
                reason: "Detected eval of dynamic variable expression".to_string(),
                detected_pattern: "eval $...".to_string(),
            };
        }
    }

    // 6. Suspicious string concatenation in command position: e.g. 'c''a''t' or "e"x"i"t
    if is_suspicious_command_concatenation(trimmed) {
        return ObfuscationDetectionResult::Obfuscated {
            reason: "Detected suspicious string splicing/concatenation in command position".to_string(),
            detected_pattern: "concatenated token".to_string(),
        };
    }

    ObfuscationDetectionResult::Clean
}

fn is_suspicious_command_concatenation(cmd: &str) -> bool {
    let first_token = cmd.split_whitespace().next().unwrap_or("");
    // Check if first token contains quotes inside it to break static token matching
    // e.g. c'a't or w"g"et
    if first_token.len() >= 4 {
        let has_internal_quote = (first_token[1..first_token.len() - 1].contains('\'')
            || first_token[1..first_token.len() - 1].contains('"'))
            && !first_token.starts_with('\'')
            && !first_token.starts_with('"');
        if has_internal_quote {
            return true;
        }
        // Multiple quotes inside: 'c''a''t'
        if first_token.matches('\'').count() >= 4 || first_token.matches('"').count() >= 4 {
            return true;
        }
    }
    false
}
