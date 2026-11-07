//! OpenClaw Interactive Presentation & Parameter Key Contract Test Suite
//!
//! Verifies OCRM-RESP-INTERACTIVE-PRESENTATION-001 & OCRM-RESP-PARAM-KEY-NORMALIZATION-001:
//! - Parameter key normalization (camelCase to snake_case, acronyms, preserving whitespace)
//! - Portable message presentation schemas (Cards, Blocks, Buttons, Selects)
//! - Safe fallback text rendering (preventing token leakage for opaque callbacks)
//! - Validation and normalization of interactive actions (Questions, Commands, URLs, ModelPicker)

use std::collections::BTreeMap;
use mahayana_host_runtime::extensions::interactive::*;

#[test]
fn test_ocrm_param_key_normalization() {
    assert_eq!(to_snake_case_key("camelCase"), "camel_case");
    assert_eq!(to_snake_case_key("parseURL"), "parse_url");
    assert_eq!(to_snake_case_key("modelID"), "model_id");
    assert_eq!(to_snake_case_key("simple"), "simple");

    let mut map = BTreeMap::new();
    map.insert("provider_id".to_string(), "anthropic".to_string());
    map.insert("maxTokens".to_string(), "2048".to_string());

    // Exact match
    assert_eq!(resolve_snake_case_param_key(&map, "maxTokens"), Some("maxTokens"));
    assert_eq!(read_snake_case_param_raw(&map, "maxTokens"), Some(&"2048".to_string()));

    // Snake case fallback match
    assert_eq!(resolve_snake_case_param_key(&map, "providerId"), Some("provider_id"));
    assert_eq!(read_snake_case_param_raw(&map, "providerId"), Some(&"anthropic".to_string()));

    // Missing
    assert_eq!(resolve_snake_case_param_key(&map, "nonExistent"), None);
}

#[test]
fn test_ocrm_interactive_fallback_select_does_not_leak_callback() {
    let pres = MessagePresentation {
        title: None,
        description: None,
        tone: None,
        blocks: vec![PresentationBlock::Select {
            placeholder: Some("Environment".to_string()),
            options: vec![
                PresentationSelectOption {
                    label: "Canary".to_string(),
                    action: MessagePresentationAction::Command {
                        command: "/deploy canary".to_string(),
                    },
                },
                PresentationSelectOption {
                    label: "Production".to_string(),
                    action: MessagePresentationAction::Command {
                        command: "/deploy production".to_string(),
                    },
                },
                PresentationSelectOption {
                    label: "Opaque".to_string(),
                    action: MessagePresentationAction::Callback {
                        value: "private-callback-token".to_string(), // MUST NOT BE EXPOSED!
                    },
                },
            ],
        }],
    };

    let text = render_message_presentation_fallback_text(&pres);
    assert_eq!(
        text,
        "Environment:\n- Canary: `/deploy canary`\n- Production: `/deploy production`\n- Opaque"
    );
    assert!(!text.contains("private-callback-token"));
}

#[test]
fn test_ocrm_interactive_fallback_buttons_and_card() {
    let pres = MessagePresentation {
        title: Some("Deployment Approval".to_string()),
        description: Some("Release v2.5 is ready for review.".to_string()),
        tone: Some(MessagePresentationTone::Warning),
        blocks: vec![
            PresentationBlock::Buttons {
                buttons: vec![
                    PresentationButton {
                        label: "Approve".to_string(),
                        action: MessagePresentationAction::Command {
                            command: "/approve rel-123".to_string(),
                        },
                        style: Some(InteractiveButtonStyle::Success),
                    },
                    PresentationButton {
                        label: "Docs".to_string(),
                        action: MessagePresentationAction::Url {
                            url: "https://docs.example.com".to_string(),
                        },
                        style: None,
                    },
                ],
            },
        ],
    };

    let text = render_message_presentation_fallback_text(&pres);
    assert!(text.contains("**Deployment Approval**"));
    assert!(text.contains("Release v2.5 is ready for review."));
    assert!(text.contains("[ Approve: `/approve rel-123` ]"));
    assert!(text.contains("[ Docs (https://docs.example.com) ]"));
}

#[test]
fn test_ocrm_interactive_action_normalization() {
    // Valid question custom-input
    let valid_pres = MessagePresentation {
        title: None,
        description: None,
        tone: None,
        blocks: vec![PresentationBlock::Buttons {
            buttons: vec![PresentationButton {
                label: "Other…".to_string(),
                action: MessagePresentationAction::Question(
                    QuestionPresentationAction::CustomInput {
                        question_id: "q-123".to_string(),
                    },
                ),
                style: None,
            }],
        }],
    };
    assert!(normalize_message_presentation(&valid_pres).is_some());

    // Invalid question with empty question_id
    let invalid_pres = MessagePresentation {
        title: None,
        description: None,
        tone: None,
        blocks: vec![PresentationBlock::Buttons {
            buttons: vec![PresentationButton {
                label: "Other…".to_string(),
                action: MessagePresentationAction::Question(
                    QuestionPresentationAction::CustomInput {
                        question_id: "   ".to_string(),
                    },
                ),
                style: None,
            }],
        }],
    };
    assert!(normalize_message_presentation(&invalid_pres).is_none());

    // Invalid URL action
    let bad_url_pres = MessagePresentation {
        title: None,
        description: None,
        tone: None,
        blocks: vec![PresentationBlock::Buttons {
            buttons: vec![PresentationButton {
                label: "Link".to_string(),
                action: MessagePresentationAction::Url {
                    url: "javascript:alert(1)".to_string(), // Invalid scheme!
                },
                style: None,
            }],
        }],
    };
    assert!(normalize_message_presentation(&bad_url_pres).is_none());
}
