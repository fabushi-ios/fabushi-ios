use serde_json::json;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_wire_normalization::*;
#[test]
fn images_cap_media_fallback_and_mirror_validation_match_frozen_wire(){
 let images=json!([
  {"base64":"a","mediaType":"text/plain"},{"base64":"b","mediaType":"image/jpeg","alt":"x"},{"base64":""},{"base64":"d"},{"base64":"e"}
 ]);
 let norm=normalize_inline_images(&images);assert_eq!(norm.len(),3);assert_eq!(norm[0].media_type,"image/png");assert_eq!(norm[1].alt.as_deref(),Some("x"));
 let entry=normalize_mirror_entry(&json!({"kind":"human-message","entryId":"e","authorAuthId":"u","authorName":"","text":"","images":[{"base64":"x"}]}),99).unwrap();
 match entry{MirrorEntry::HumanMessage{author_name,timestamp_ms,..}=>{assert_eq!(author_name,"Someone");assert_eq!(timestamp_ms,99)},_=>panic!()}
 assert!(normalize_mirror_entry(&json!({"kind":"human-message","entryId":"e","authorAuthId":"u","text":"","images":[]}),1).is_none());
}
#[test]
fn turn_messages_default_to_human_and_cap_at_24(){
 let rows=(0..30).map(|i|json!({"speakerKind":if i%2==0{"agent"}else{"other"},"speakerName":"a\nb","isSelf":i==0,"text":format!("m{i}")})).collect::<Vec<_>>();
 let out=normalize_turn_messages(&json!(rows));assert_eq!(out.len(),24);assert_eq!(out[0].speaker_kind,"agent");assert!(out[0].is_self);assert_eq!(out[0].speaker_name,"a b");assert_eq!(out[1].speaker_kind,"human");
}
