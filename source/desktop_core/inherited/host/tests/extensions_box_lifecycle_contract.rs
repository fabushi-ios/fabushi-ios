use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use mahayana_host_runtime::extensions::box_lifecycle::{
    BOX_LIFECYCLE_DEPENDENCIES, BoxLifecycleClient, BoxLifecycleClientFactory,
    BoxLifecycleFuture, BoxRunState, RecreateSandBoxRequest, RecreateSandBoxResponse,
    box_lifecycle_extension_id, start_box_lifecycle_extension,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

struct NoopWake;
impl Wake for NoopWake { fn wake(self: Arc<Self>) {} }
fn block_on_ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("test future unexpectedly pending"),
    }
}
#[derive(Default)]
struct FakeAuth;
#[derive(Default)]
struct FakeClient {
    observed_signal: Mutex<Option<u64>>,
    recreate_requests: Mutex<Vec<RecreateSandBoxRequest>>,
    image_update_available: AtomicBool,
}
#[derive(Clone)]
struct TestClient(Arc<FakeClient>);
impl BoxLifecycleClient<u64> for TestClient {
    type Error = &'static str;
    fn get_sand_box_run_state<'a>(&'a self, signal: &'a u64) -> BoxLifecycleFuture<'a, Result<BoxRunState, Self::Error>> {
        self.0.observed_signal.lock().unwrap_or_else(|p| p.into_inner()).replace(*signal);
        let available = self.0.image_update_available.load(Ordering::SeqCst);
        Box::pin(async move { Ok(BoxRunState { image_update_available: available }) })
    }
    fn recreate_sand_box<'a>(&'a self, request: RecreateSandBoxRequest) -> BoxLifecycleFuture<'a, Result<RecreateSandBoxResponse, Self::Error>> {
        self.0.recreate_requests.lock().unwrap_or_else(|p| p.into_inner()).push(request);
        Box::pin(async { Ok(RecreateSandBoxResponse { started: true, reason: Some("recreating".into()) }) })
    }
}
struct FakeFactory { client: Arc<FakeClient>, calls: AtomicUsize }
impl BoxLifecycleClientFactory<FakeAuth> for FakeFactory {
    type Client = TestClient;
    fn create_sand_cursor_backend_client(&self, _auth: Arc<FakeAuth>) -> Self::Client {
        self.calls.fetch_add(1, Ordering::SeqCst);
        TestClient(Arc::clone(&self.client))
    }
}
#[test]
fn box_lifecycle_service_and_extension_preserve_grok_contract() {
    assert_eq!(box_lifecycle_extension_id(), HostExtensionId::BoxLifecycle);
    assert_eq!(BOX_LIFECYCLE_DEPENDENCIES, &[HostExtensionId::Auth]);

    let client = Arc::new(FakeClient::default());
    client.image_update_available.store(true, Ordering::SeqCst);
    let factory = FakeFactory { client: Arc::clone(&client), calls: AtomicUsize::new(0) };
    let service = start_box_lifecycle_extension(Arc::new(FakeAuth), &factory);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);

    let signal = 42_u64;
    assert!(block_on_ready(service.fetch_image_update_available(&signal)).expect("run state"));
    assert_eq!(*client.observed_signal.lock().unwrap_or_else(|p| p.into_inner()), Some(42));

    let default_force = block_on_ready(service.recreate_in_box::<u64>(true, None)).expect("recreate");
    assert_eq!(default_force, RecreateSandBoxResponse { started: true, reason: Some("recreating".into()) });
    let forced = block_on_ready(service.recreate_in_box::<u64>(false, Some(true))).expect("forced recreate");
    assert!(forced.started);

    assert_eq!(
        client.recreate_requests.lock().unwrap_or_else(|p| p.into_inner()).as_slice(),
        &[
            RecreateSandBoxRequest { preserve_data: true, force: false },
            RecreateSandBoxRequest { preserve_data: false, force: true },
        ]
    );
}
