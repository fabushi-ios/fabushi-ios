use mahayana_host_runtime::automations::automation_store::{parse_stored_config,serialize_config};
use mahayana_host_runtime::automations::automation_trigger::{
    build_trigger_event_context_block,describe_trigger_event,github_listener_matches,
    listener_matches_event,parse_stored_trigger,serialize_stored_trigger,
    slack_listener_matches,trigger_matches_event,
};
use serde_json::json;

#[test]
fn slack_github_and_ci_matching_match_frozen_contract(){
 let slack=parse_stored_trigger(&json!({"type":"slack","channel":"#eng","match":{"kind":"reaction","emoji":[":eyes:"],"bySelf":true}})).unwrap();
 assert!(slack_listener_matches(&slack,&json!({"source":"slack","channel":"eng","reactionEmoji":"eyes","isSelf":true})));
 assert!(!slack_listener_matches(&slack,&json!({"source":"slack","channel":"eng","reactionEmoji":"eyes","isSelf":false})));
 let gh=parse_stored_trigger(&json!({"type":"github","repo":"Org/Repo","events":["review-approved","ci-failed"],"userAllowlist":["alice"],"ciBranch":"main"})).unwrap();
 assert!(github_listener_matches(&gh,&json!({"source":"github","repo":"org/repo","kind":"review-approved","actor":"ALICE","prOwner":"alice"}),false));
 assert!(!github_listener_matches(&gh,&json!({"source":"github","repo":"org/repo","kind":"review-approved","actor":"bob","prOwner":"alice"}),false));
 assert!(github_listener_matches(&gh,&json!({"source":"github","repo":"org/repo","kind":"ci-failed","branch":"main"}),false));
}
#[test]
fn cloud_filters_descriptions_and_context_match_frozen_contract(){
 let linear=parse_stored_trigger(&json!({"type":"linear","event":{"case":"statusChanged","statusIds":["done"]},"projectIds":["p"],"teamIds":[]})).unwrap();
 assert!(trigger_matches_event(&linear,&json!({"source":"linear","event":"statusChanged","projectId":"p","statusId":"done"}),false,false));
 let sentry=parse_stored_trigger(&json!({"type":"sentry","event":{"case":"issueAny"},"projectIds":["p"]})).unwrap();
 assert!(listener_matches_event(&sentry,&json!({"source":"sentry","event":"issueResolved","projectId":"p"}),false,false));
 let teams=parse_stored_trigger(&json!({"type":"microsoftTeams","tenantId":"t","teamIds":["team"],"channelIds":["c"],"messageContains":"deploy"})).unwrap();
 assert!(listener_matches_event(&teams,&json!({"source":"microsoftTeams","tenantId":"t","teamId":"team","channelId":"c","text":"DEPLOY now"}),false,false));
 let event=json!({"source":"github","kind":"pr-opened","repo":"a/b","title":"x<y & z","actor":"alice"});
 assert!(describe_trigger_event(&event).contains("PR opened"));
 let block=build_trigger_event_context_block(&event);assert!(block.contains("&lt;"));assert!(block.contains("&amp;"));
}
#[test]
fn canonical_serializer_round_trips_through_store(){
 let trigger=parse_stored_trigger(&json!({"type":"group","listeners":[{"type":"cron","schedule":"0 9 * * 1-5"},{"type":"slack","channel":"#eng","match":{"kind":"keyword","keyword":"Ship"}}]})).unwrap();
 assert_eq!(serialize_stored_trigger(&trigger),trigger);
 let raw=json!({"name":"Routine","prompt":"Do it","trigger":trigger,"enabled":true,"createdAt":1000.0}).to_string();
 let config=parse_stored_config(&raw,1000.0).unwrap();let persisted=serialize_config(&config);let roundtrip=parse_stored_config(&persisted,1000.0).unwrap();
 assert_eq!(roundtrip.trigger,config.trigger);
}
