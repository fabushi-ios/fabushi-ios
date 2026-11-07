use mahayana_host_runtime::runner::sand_auto_review_classifier_run::{
    AutoReviewClassifierDecision, AutoReviewClassifierError,
    AutoReviewClassifierRequest, SandAutoReviewClassifierExecutor,
    SmartModeClassifierDecision, SmartModeClassifierMeasurement,
    SmartModeClassifierMeasurementKind, SmartModeClassifierResult,
    SmartModeClassifierSuccess, SAND_AUTO_REVIEW_BLOCK_REASON,
    SAND_AUTO_REVIEW_CLASSIFIER_MAX_ATTEMPTS,
    run_sand_auto_review_classifier,
};

struct Executor {
    result: Result<SmartModeClassifierResult, AutoReviewClassifierError>,
    seen: Vec<(String, String, String, bool, u32)>,
    measurements: Vec<SmartModeClassifierMeasurement>,
}

impl SandAutoReviewClassifierExecutor<String, String> for Executor {
    fn execute(
        &mut self,
        request: AutoReviewClassifierRequest<'_, String, String>,
    ) -> Result<SmartModeClassifierResult, AutoReviewClassifierError> {
        self.seen.push((
            request.tool_call_id.to_string(),
            request.parent_conversation_id.to_string(),
            request.mode.to_string(),
            request.suppress_tool_call_id_logging,
            request.max_attempts,
        ));
        self.result.clone()
    }

    fn record_measurement(&mut self, measurement: SmartModeClassifierMeasurement) {
        self.measurements.push(measurement);
    }
}

struct JsonExecutor {
    result: Result<SmartModeClassifierResult, AutoReviewClassifierError>,
    measurements: Vec<SmartModeClassifierMeasurement>,
}

impl SandAutoReviewClassifierExecutor<serde_json::Value, String> for JsonExecutor {
    fn execute(
        &mut self,
        _request: AutoReviewClassifierRequest<'_, serde_json::Value, String>,
    ) -> Result<SmartModeClassifierResult, AutoReviewClassifierError> {
        self.result.clone()
    }

    fn record_measurement(&mut self, measurement: SmartModeClassifierMeasurement) {
        self.measurements.push(measurement);
    }
}

#[test]
fn classifier_allows_and_enforces_frozen_measurement_options() {
    let mut executor = Executor {
        result: Ok(SmartModeClassifierResult::Success(
            SmartModeClassifierSuccess {
                decision: SmartModeClassifierDecision::Allow,
                block_reason: None,
                proposed_allow_rule: None,
            },
        )),
        seen: Vec::new(),
        measurements: Vec::new(),
    };
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "tool-1",
        "conversation-1",
        "enforce",
        || "target".into(),
        || Ok(vec!["context".into()]),
        &["/workspace".into()],
        "classifier unavailable",
    )
    .expect("classifier");
    assert_eq!(decision, AutoReviewClassifierDecision::Allow);
    assert_eq!(
        executor.seen,
        vec![(
            "tool-1".into(),
            "conversation-1".into(),
            "enforce".into(),
            true,
            SAND_AUTO_REVIEW_CLASSIFIER_MAX_ATTEMPTS,
        )]
    );
    assert_eq!(SAND_AUTO_REVIEW_CLASSIFIER_MAX_ATTEMPTS, 1);
    assert_eq!(executor.measurements.len(), 2);
    assert_eq!(
        executor.measurements[0].kind,
        SmartModeClassifierMeasurementKind::Started
    );
    assert_eq!(executor.measurements[0].mode, "enforce");
    assert_eq!(executor.measurements[0].timeout_ms, Some(10_000));
    assert_eq!(
        executor.measurements[1].kind,
        SmartModeClassifierMeasurementKind::Call
    );
    assert_eq!(executor.measurements[1].outcome.as_deref(), Some("allow"));
    assert_eq!(executor.measurements[1].decision.as_deref(), Some("allow"));
    assert_eq!(executor.measurements[1].retry_count, Some(0));
    assert_eq!(executor.measurements[1].retryable, Some(false));
}

#[test]
fn classifier_trims_block_reason_and_rule_and_uses_default_reason() {
    let mut executor = Executor {
        result: Ok(SmartModeClassifierResult::Success(
            SmartModeClassifierSuccess {
                decision: SmartModeClassifierDecision::Block,
                block_reason: Some("  private data boundary  ".into()),
                proposed_allow_rule: Some("  allow approved domain  ".into()),
            },
        )),
        seen: Vec::new(),
        measurements: Vec::new(),
    };
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "shadow",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "error",
    )
    .expect("block");
    assert_eq!(
        decision,
        AutoReviewClassifierDecision::Block {
            reason: "private data boundary".into(),
            proposed_rule: Some("allow approved domain".into()),
        }
    );

    executor.result = Ok(SmartModeClassifierResult::Success(
        SmartModeClassifierSuccess {
            decision: SmartModeClassifierDecision::Block,
            block_reason: Some("   ".into()),
            proposed_allow_rule: Some(" ".into()),
        },
    ));
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "error",
    )
    .expect("default block");
    assert_eq!(
        decision,
        AutoReviewClassifierDecision::Block {
            reason: SAND_AUTO_REVIEW_BLOCK_REASON.into(),
            proposed_rule: None,
        }
    );
}

#[test]
fn classifier_failures_and_unknown_decisions_reject_fail_closed() {
    let mut executor = Executor {
        result: Ok(SmartModeClassifierResult::Failure),
        seen: Vec::new(),
        measurements: Vec::new(),
    };
    let reject = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "classifier unavailable",
    )
    .expect("reject");
    assert_eq!(
        reject,
        AutoReviewClassifierDecision::Reject {
            reason: "classifier unavailable".into(),
        }
    );

    executor.result = Ok(SmartModeClassifierResult::Success(
        SmartModeClassifierSuccess {
            decision: SmartModeClassifierDecision::Unknown,
            block_reason: None,
            proposed_allow_rule: None,
        },
    ));
    let reject = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "classifier unavailable",
    )
    .expect("reject");
    assert_eq!(
        reject,
        AutoReviewClassifierDecision::Reject {
            reason: "classifier unavailable".into(),
        }
    );
}

#[test]
fn classifier_abort_is_rethrown_but_other_executor_errors_reject() {
    let mut executor = Executor {
        result: Err(AutoReviewClassifierError::Aborted("cancelled".into())),
        seen: Vec::new(),
        measurements: Vec::new(),
    };
    let error = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "classifier unavailable",
    )
    .expect_err("abort");
    assert_eq!(error, AutoReviewClassifierError::Aborted("cancelled".into()));

    executor.result = Err(AutoReviewClassifierError::Failed("network".into()));
    let reject = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Ok(Vec::<String>::new()),
        &[],
        "classifier unavailable",
    )
    .expect("reject");
    assert_eq!(
        reject,
        AutoReviewClassifierDecision::Reject {
            reason: "classifier unavailable".into(),
        }
    );
}

#[test]
fn conversation_context_abort_is_rethrown_before_executor_runs() {
    let mut executor = Executor {
        result: Ok(SmartModeClassifierResult::Failure),
        seen: Vec::new(),
        measurements: Vec::new(),
    };
    let error = run_sand_auto_review_classifier(
        &mut executor,
        "tool",
        "conversation",
        "enforce",
        || "target".into(),
        || Err(AutoReviewClassifierError::Aborted("cancelled".into())),
        &[],
        "classifier unavailable",
    )
    .expect_err("abort");
    assert_eq!(error, AutoReviewClassifierError::Aborted("cancelled".into()));
    assert!(executor.seen.is_empty());
}


#[test]
fn classifier_measurement_preserves_unspecified_error_and_abort_semantics() {
    let mut executor = JsonExecutor {
        result: Ok(SmartModeClassifierResult::Success(
            SmartModeClassifierSuccess {
                decision: SmartModeClassifierDecision::Unspecified,
                block_reason: None,
                proposed_allow_rule: None,
            },
        )),
        measurements: Vec::new(),
    };
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "private-tool-call-id",
        "conversation",
        "enforce",
        || serde_json::json!({
            "action": "shell",
            "arguments": {"execution_surface": "host_machine"}
        }),
        || Ok(Vec::<String>::new()),
        &["/workspace".into()],
        "manual review",
    )
    .expect("unspecified must fail closed");
    assert_eq!(
        decision,
        AutoReviewClassifierDecision::Reject {
            reason: "manual review".into(),
        }
    );
    assert_eq!(executor.measurements[0].action_kind, "shell");
    assert_eq!(
        executor.measurements[0].surface_label.as_deref(),
        Some("host_machine")
    );
    assert_eq!(
        executor.measurements[1].failure_reason.as_deref(),
        Some("unspecified_decision")
    );
    assert_eq!(executor.measurements[1].retryable, Some(true));
    assert!(
        !format!("{:?}", executor.measurements).contains("private-tool-call-id"),
        "measurement must suppress tool-call id"
    );

    executor.result = Ok(SmartModeClassifierResult::Error {
        failure_reason: Some("backend_overloaded".into()),
        retryable: Some(false),
    });
    executor.measurements.clear();
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "private-tool-call-id-2",
        "conversation",
        "shadow",
        || serde_json::json!({"action": "mcp", "arguments": {}}),
        || Ok(Vec::<String>::new()),
        &[],
        "manual review",
    )
    .expect("backend result error must fail closed");
    assert_eq!(
        decision,
        AutoReviewClassifierDecision::Reject {
            reason: "manual review".into(),
        }
    );
    assert_eq!(executor.measurements[1].outcome.as_deref(), Some("error"));
    assert_eq!(
        executor.measurements[1].failure_reason.as_deref(),
        Some("backend_overloaded")
    );
    assert_eq!(executor.measurements[1].retryable, Some(false));

    executor.result = Err(AutoReviewClassifierError::Timeout("request timed out".into()));
    executor.measurements.clear();
    let decision = run_sand_auto_review_classifier(
        &mut executor,
        "private-tool-call-id-timeout",
        "conversation",
        "enforce",
        || serde_json::json!({"action": "web_fetch", "arguments": {}}),
        || Ok(Vec::<String>::new()),
        &[],
        "manual review",
    )
    .expect("timeout must fail closed");
    assert_eq!(
        decision,
        AutoReviewClassifierDecision::Reject {
            reason: "manual review".into(),
        }
    );
    assert_eq!(
        executor.measurements[1].failure_reason.as_deref(),
        Some("timeout_exception")
    );
    assert_eq!(
        executor.measurements[1].kind,
        SmartModeClassifierMeasurementKind::Exception
    );
    assert_eq!(executor.measurements[1].retry_count, Some(0));

    executor.result = Err(AutoReviewClassifierError::Aborted("cancelled".into()));
    executor.measurements.clear();
    let error = run_sand_auto_review_classifier(
        &mut executor,
        "private-tool-call-id-3",
        "conversation",
        "enforce",
        || serde_json::json!({"action": "sand_computer", "arguments": {}}),
        || Ok(Vec::<String>::new()),
        &[],
        "manual review",
    )
    .expect_err("abort must propagate");
    assert_eq!(error, AutoReviewClassifierError::Aborted("cancelled".into()));
    assert_eq!(
        executor.measurements[1].kind,
        SmartModeClassifierMeasurementKind::Exception
    );
    assert_eq!(
        executor.measurements[1].outcome.as_deref(),
        Some("exception")
    );
}
