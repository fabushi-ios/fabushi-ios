use std::sync::Arc;
use mahayana_host_runtime::extensions::inference::provider_session::RoutedProvider;
use mahayana_host_runtime::gateway_server::{GatewayApi, GatewayCommandContext, GatewayCommandError};
use mahayana_host_runtime::extensions::session::gateway::persist_accepted_send_prompt_context;
use crate::{
    map_production_send_error, map_session_send_error, resolve_gateway_session_route,
    RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD, UnifiedGatewayApi,
};

pub(crate) fn call_openclaw_sessions_provider_review_continue(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
    context: &GatewayCommandContext,
) -> Result<serde_json::Value, GatewayCommandError> {
    if !context.authenticated {
        return Err(GatewayCommandError::Forbidden(
            "sessions.providerReview.continue requires an authenticated Gateway request".into(),
        ));
    }
    let session_key = args
        .get("sessionKey")
        .or_else(|| args.get("key"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.providerReview.continue requires sessionKey".into(),
        ))?;
    let route = resolve_gateway_session_route(args, session_key)?;
    let canonical_key = format!("agent:{}:main", route.agent_id);
    if !session_key.eq_ignore_ascii_case(&canonical_key) {
        return Err(GatewayCommandError::BadRequest(
            "sessions.providerReview.continue only supports canonical main sessions".into(),
        ));
    }
    let expected_session_id = args
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.providerReview.continue requires sessionId".into(),
        ))?;
    if expected_session_id != route.agent_id.as_str() {
        return Err(GatewayCommandError::BadRequest(
            "SESSION_LIFECYCLE_CHANGED".into(),
        ));
    }
    let review_id = args
        .get("reviewId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.providerReview.continue requires reviewId".into(),
        ))?;
    let next_run_id = args
        .get("idempotencyKey")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.providerReview.continue requires idempotencyKey".into(),
        ))?;
    let record = api
        .session_workers
        .read_session_work_admission(&route.agent_id)
        .map_err(GatewayCommandError::Internal)?
        .ok_or_else(|| GatewayCommandError::Conflict(
            "provider review session admission is unavailable".into(),
        ))?;
    let review = record
        .provider_review
        .clone()
        .ok_or_else(|| GatewayCommandError::Conflict(
            "provider review is unavailable; refresh the current findings".into(),
        ))?;
    if record.session_id != expected_session_id
        || review.session_id != expected_session_id
        || review.id != review_id
        || record.provider_review_id.as_deref() != Some(review_id)
        || record.provider_review_phase.as_deref() != Some("pending")
    {
        return Err(GatewayCommandError::Conflict(
            "provider review changed; refresh the current findings".into(),
        ));
    }
    let continuation = review
        .continuation_message
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "provider review has no bounded continuation".into(),
        ))?;
    let provider = match review.provider.as_str() {
        "openai" => RoutedProvider::Codex,
        value => RoutedProvider::parse(value).ok_or_else(|| {
            GatewayCommandError::BadRequest(
                "provider review runtime is unsupported by this host".into(),
            )
        })?,
    };
    let lifecycle_revision = record.lifecycle_revision.clone();
    let admitted_revision = api
        .session_workers
        .capture_session_work_admission(
            &route.agent_id,
            &canonical_key,
            next_run_id,
        )
        .map_err(GatewayCommandError::Conflict)?;
    if admitted_revision != lifecycle_revision {
        let _ = api.session_workers.release_session_work_admission(
            &route.agent_id,
            &admitted_revision,
            next_run_id,
        );
        return Err(GatewayCommandError::Conflict(
            "provider review lifecycle changed before continuation admission".into(),
        ));
    }
    let acknowledgment = match api.session_workers.issue_provider_review_acknowledgment(
        &route.agent_id,
        &admitted_revision,
        review_id,
        next_run_id,
    ) {
        Ok(acknowledgment) => acknowledgment,
        Err(error) => {
            let _ = api.session_workers.release_session_work_admission(
                &route.agent_id,
                &admitted_revision,
                next_run_id,
            );
            let _ = api.session_workers.release_provider_review_continuation(
                &route.agent_id,
                &admitted_revision,
                next_run_id,
            );
            return Err(GatewayCommandError::Conflict(error));
        }
    };
    if let Err(error) = api.session_workers.claim_provider_review_acknowledgment(
        acknowledgment,
        next_run_id,
        &review.provider,
        &review.model,
        &review.runtime_id,
        review.api.as_deref(),
    ) {
        let _ = api.session_workers.release_session_work_admission(
            &route.agent_id,
            &admitted_revision,
            next_run_id,
        );
        return Err(GatewayCommandError::Conflict(error));
    }
    let runner_args = serde_json::json!({
        "provider": provider.as_str(),
        "agentId": route.agent_id.clone(),
        "sessionKey": canonical_key,
        "streamId": next_run_id,
        "clientNonce": next_run_id,
        "requestSource": "provider-review",
        "providerReviewLifecycleRevision": admitted_revision.clone(),
        "providerReviewId": review_id,
        "prompt": continuation,
        "appendUserMessage": true,
        "skipAckObligation": true,
        "messages": [{
            "role": "user",
            "content": continuation,
        }],
    });
    if let Err(error) = api
        .transcript_runtime
        .accept_routed_send(&runner_args, |accepted| {
            persist_accepted_send_prompt_context(
                &api.session_workers,
                &runner_args,
                accepted,
            )
            .map_err(map_session_send_error)
        })
    {
        let _ = api.session_workers.release_provider_review_continuation(
            &route.agent_id,
            &admitted_revision,
            next_run_id,
        );
        let _ = api.session_workers.release_session_work_admission(
            &route.agent_id,
            &admitted_revision,
            next_run_id,
        );
        return Err(map_production_send_error(error));
    }
    match api.call_with_context(
        RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD,
        runner_args,
        context,
    ) {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = api.session_workers.release_provider_review_continuation(
                &route.agent_id,
                &admitted_revision,
                next_run_id,
            );
            let _ = api.session_workers.release_session_work_admission(
                &route.agent_id,
                &admitted_revision,
                next_run_id,
            );
            Err(error)
        }
    }
}
