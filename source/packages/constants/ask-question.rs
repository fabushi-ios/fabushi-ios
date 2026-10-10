pub const ASK_QUESTION_AUTO_ANSWER_MARKER: &str = "ask_question_auto_answer";
pub const ASK_QUESTION_AUTO_ANSWER_REASON_PREFIX: &str = "No response was received within the time limit";
pub const ASK_QUESTION_AUTO_ANSWER_REASON_BODY: &str = "No response was received within the time limit. Proceed with the recommended option(s) you offered for each question, or your best judgment based on the information already available.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskQuestionAutoAnswerKind { Timeout, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskQuestionAutoAnswerIdentity {
    pub marker: &'static str,
    pub kind: AskQuestionAutoAnswerKind,
}

pub fn create_ask_question_auto_answer_identity(kind: AskQuestionAutoAnswerKind) -> AskQuestionAutoAnswerIdentity {
    AskQuestionAutoAnswerIdentity { marker: ASK_QUESTION_AUTO_ANSWER_MARKER, kind }
}

fn kind_str(kind: AskQuestionAutoAnswerKind) -> &'static str {
    match kind { AskQuestionAutoAnswerKind::Timeout => "timeout", AskQuestionAutoAnswerKind::Other => "other" }
}

pub fn format_ask_question_auto_answer_reason(identity: AskQuestionAutoAnswerIdentity) -> String {
    format!("{}:{}|{}", identity.marker, kind_str(identity.kind), ASK_QUESTION_AUTO_ANSWER_REASON_BODY)
}

pub fn parse_ask_question_auto_answer_identity(reason: Option<&str>) -> Option<AskQuestionAutoAnswerIdentity> {
    let trimmed=reason.unwrap_or_default().trim();
    let rest=trimmed.strip_prefix("ask_question_auto_answer:")?;
    let (kind, _)=rest.split_once('|')?;
    let kind=match kind { "timeout"=>AskQuestionAutoAnswerKind::Timeout, "other"=>AskQuestionAutoAnswerKind::Other, _=>return None };
    Some(create_ask_question_auto_answer_identity(kind))
}

pub fn is_ask_question_auto_answer_reason(reason: Option<&str>) -> bool {
    parse_ask_question_auto_answer_identity(reason).is_some()
        || reason.unwrap_or_default().trim().starts_with(ASK_QUESTION_AUTO_ANSWER_REASON_PREFIX)
}

pub fn ask_question_auto_answer_reason() -> String {
    format_ask_question_auto_answer_reason(create_ask_question_auto_answer_identity(AskQuestionAutoAnswerKind::Timeout))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_parses_and_accepts_legacy_reason() {
        let timeout=create_ask_question_auto_answer_identity(AskQuestionAutoAnswerKind::Timeout);
        let formatted=format_ask_question_auto_answer_reason(timeout);
        assert!(formatted.starts_with("ask_question_auto_answer:timeout|"));
        assert_eq!(parse_ask_question_auto_answer_identity(Some(&formatted)), Some(timeout));
        assert!(is_ask_question_auto_answer_reason(Some(ASK_QUESTION_AUTO_ANSWER_REASON_BODY)));
        assert!(!is_ask_question_auto_answer_reason(Some("different reason")));
        assert_eq!(ask_question_auto_answer_reason(), formatted);
    }
}
