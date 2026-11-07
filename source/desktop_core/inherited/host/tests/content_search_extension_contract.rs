use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::content_search::extension::{
    AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT, AGENT_CONTENT_SEARCH_MAX_RESULTS,
    CONTENT_SEARCH_DEPENDENCIES, ContentSearchIndex, ContentSearchIndexFactory,
    FeatureGateProperty, FeatureGateSubscription, GLOBAL_SEARCH_GATE, TranscriptMutation,
    TranscriptMutationSource, TranscriptMutationSubscription, start_content_search_extension,
};

#[derive(Default)]
struct Index {
    starts: AtomicUsize,
    disposes: AtomicUsize,
    mutations: Mutex<Vec<TranscriptMutation>>,
}
impl ContentSearchIndex for Index {
    fn start(&self){self.starts.fetch_add(1,Ordering::SeqCst);}
    fn dispose(&self){self.disposes.fetch_add(1,Ordering::SeqCst);}
    fn is_search_ready(&self)->bool{self.starts.load(Ordering::SeqCst)>0}
    fn search_messages(&self,q:&str,limit:usize)->Vec<String>{vec![format!("{q}:{limit}")]}
    fn search_media(&self,q:&str,limit:usize)->Vec<String>{vec![format!("media:{q}:{limit}")]}
    fn apply_mutation(&self,m:TranscriptMutation){self.mutations.lock().unwrap().push(m);}
}
struct Factory{index:Arc<Index>}
impl ContentSearchIndexFactory for Factory{
    type Index=Index;
    fn create(&self)->Arc<Index>{Arc::clone(&self.index)}
}
struct Disposable{count:Arc<AtomicUsize>}
impl FeatureGateSubscription for Disposable{fn dispose(&mut self){self.count.fetch_add(1,Ordering::SeqCst);}}
struct MutationDisposable{count:Arc<AtomicUsize>}
impl TranscriptMutationSubscription for MutationDisposable{fn dispose(&mut self){self.count.fetch_add(1,Ordering::SeqCst);}}

struct Gate{
    value:AtomicBool,
    listener:Mutex<Option<Arc<dyn Fn(bool)+Send+Sync>>>,
    disposes:Arc<AtomicUsize>,
}
impl Gate{
    fn set(&self,value:bool){
        self.value.store(value,Ordering::SeqCst);
        if let Some(listener)=self.listener.lock().unwrap().clone(){listener(value);}
    }
}
impl FeatureGateProperty for Gate{
    fn get(&self)->bool{self.value.load(Ordering::SeqCst)}
    fn subscribe(&self,listener:Arc<dyn Fn(bool)+Send+Sync>)->Box<dyn FeatureGateSubscription>{
        *self.listener.lock().unwrap()=Some(listener);
        Box::new(Disposable{count:Arc::clone(&self.disposes)})
    }
}

struct Mutations{
    listener:Mutex<Option<Arc<dyn Fn(TranscriptMutation)+Send+Sync>>>,
    subscribes:AtomicUsize,
    disposes:Arc<AtomicUsize>,
}
impl TranscriptMutationSource for Mutations{
    fn subscribe(&self,listener:Arc<dyn Fn(TranscriptMutation)+Send+Sync>)->Box<dyn TranscriptMutationSubscription>{
        self.subscribes.fetch_add(1,Ordering::SeqCst);
        *self.listener.lock().unwrap()=Some(listener);
        Box::new(MutationDisposable{count:Arc::clone(&self.disposes)})
    }
}

#[test]
fn gate_controls_index_start_mutation_subscription_and_cleanup(){
    assert_eq!(CONTENT_SEARCH_DEPENDENCIES,["experiments","telemetry"]);
    let index=Arc::new(Index::default());
    let gate=Arc::new(Gate{value:AtomicBool::new(false),listener:Mutex::new(None),disposes:Arc::new(AtomicUsize::new(0))});
    let mutations=Arc::new(Mutations{listener:Mutex::new(None),subscribes:AtomicUsize::new(0),disposes:Arc::new(AtomicUsize::new(0))});
    let enabled=Arc::new(AtomicBool::new(false));
    let check_gate={
        let enabled=Arc::clone(&enabled);
        Arc::new(move |name:&str|{assert_eq!(name,GLOBAL_SEARCH_GATE);enabled.load(Ordering::SeqCst)})
    };
    let extension=start_content_search_extension(
        &Factory{index:Arc::clone(&index)},
        gate.clone(),
        check_gate,
        mutations.clone(),
    );
    assert_eq!(index.starts.load(Ordering::SeqCst),0);
    assert_eq!(mutations.subscribes.load(Ordering::SeqCst),0);

    enabled.store(true,Ordering::SeqCst);
    gate.set(true);
    assert!(extension.is_enabled());
    assert!(extension.is_search_ready());
    assert_eq!(index.starts.load(Ordering::SeqCst),1);
    assert_eq!(mutations.subscribes.load(Ordering::SeqCst),1);
    gate.set(true);
    assert_eq!(index.starts.load(Ordering::SeqCst),2);
    assert_eq!(mutations.subscribes.load(Ordering::SeqCst),1);

    mutations.listener.lock().unwrap().clone().unwrap()(TranscriptMutation{agent_id:"a".into(),entry_id:"e".into()});
    assert_eq!(index.mutations.lock().unwrap().len(),1);
    assert_eq!(extension.max_matches_per_agent(),AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT);
    assert_eq!(extension.max_results(),AGENT_CONTENT_SEARCH_MAX_RESULTS);
    assert_eq!(extension.search_messages("needle",3),vec!["needle:3"]);
    assert_eq!(extension.search_media("img",2),vec!["media:img:2"]);

    gate.set(false);
    assert_eq!(mutations.disposes.load(Ordering::SeqCst),1);
    gate.set(true);
    assert_eq!(mutations.subscribes.load(Ordering::SeqCst),2);
    drop(extension);
    assert_eq!(mutations.disposes.load(Ordering::SeqCst),2);
    assert_eq!(gate.disposes.load(Ordering::SeqCst),1);
    assert_eq!(index.disposes.load(Ordering::SeqCst),1);
}
