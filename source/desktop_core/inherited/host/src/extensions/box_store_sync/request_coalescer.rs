use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestCoalescerError<E> {
    Run(E),
    Protocol(String),
    WorkerStopped,
}

impl<E: fmt::Display> fmt::Display for RequestCoalescerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(error) => write!(formatter, "{error}"),
            Self::Protocol(message) => formatter.write_str(message),
            Self::WorkerStopped => formatter.write_str("request coalescer worker stopped"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RequestCoalescerError<E> {}

struct PendingRequest<Request, ResultValue, ErrorValue> {
    request: Request,
    reply: mpsc::SyncSender<Result<ResultValue, RequestCoalescerError<ErrorValue>>>,
}

struct State<Request, ResultValue, ErrorValue> {
    queue: VecDeque<PendingRequest<Request, ResultValue, ErrorValue>>,
    draining: bool,
}

type Run<Request, ResultValue, ErrorValue> =
    Arc<dyn Fn(Vec<Request>) -> Result<Vec<ResultValue>, ErrorValue> + Send + Sync + 'static>;
type ConflictKey<Request> =
    Arc<dyn Fn(&Request) -> Option<String> + Send + Sync + 'static>;
type SplitOnError<ErrorValue> =
    Arc<dyn Fn(&RequestCoalescerError<ErrorValue>) -> bool + Send + Sync + 'static>;

pub struct RequestCoalescer<Request, ResultValue, ErrorValue> {
    max_batch_size: usize,
    run: Run<Request, ResultValue, ErrorValue>,
    conflict_key: Option<ConflictKey<Request>>,
    should_split_on_error: Option<SplitOnError<ErrorValue>>,
    state: Mutex<State<Request, ResultValue, ErrorValue>>,
}

impl<Request, ResultValue, ErrorValue> RequestCoalescer<Request, ResultValue, ErrorValue>
where
    Request: Clone + Send + 'static,
    ResultValue: Send + 'static,
    ErrorValue: Clone + Send + 'static,
{
    pub fn new(
        max_batch_size: usize,
        run: Run<Request, ResultValue, ErrorValue>,
        conflict_key: Option<ConflictKey<Request>>,
        should_split_on_error: Option<SplitOnError<ErrorValue>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            max_batch_size: max_batch_size.max(1),
            run,
            conflict_key,
            should_split_on_error,
            state: Mutex::new(State {
                queue: VecDeque::new(),
                draining: false,
            }),
        })
    }

    pub fn request(
        self: &Arc<Self>,
        request: Request,
    ) -> Result<ResultValue, RequestCoalescerError<ErrorValue>> {
        let (reply, receive) = mpsc::sync_channel(1);
        let should_start = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.queue.push_back(PendingRequest { request, reply });
            if state.draining {
                false
            } else {
                state.draining = true;
                true
            }
        };

        if should_start {
            let coalescer = Arc::clone(self);
            if thread::Builder::new()
                .name("box-store-request-coalescer".into())
                .spawn(move || coalescer.drain())
                .is_err()
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.draining = false;
                while let Some(pending) = state.queue.pop_front() {
                    let _ = pending.reply.send(Err(RequestCoalescerError::WorkerStopped));
                }
            }
        }

        receive
            .recv()
            .unwrap_or(Err(RequestCoalescerError::WorkerStopped))
    }

    fn take_batch(
        &self,
    ) -> Vec<PendingRequest<Request, ResultValue, ErrorValue>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut batch = Vec::new();
        let mut claimed = HashSet::new();
        let mut deferred = Vec::new();

        while batch.len() < self.max_batch_size {
            let Some(pending) = state.queue.pop_front() else {
                break;
            };
            let key = self
                .conflict_key
                .as_ref()
                .and_then(|key| key(&pending.request));
            if let Some(key) = key {
                if claimed.contains(&key) {
                    deferred.push(pending);
                    continue;
                }
                claimed.insert(key);
            }
            batch.push(pending);
        }

        for pending in deferred.into_iter().rev() {
            state.queue.push_front(pending);
        }
        batch
    }

    fn run_individually(
        &self,
        batch: Vec<PendingRequest<Request, ResultValue, ErrorValue>>,
    ) {
        for pending in batch {
            let outcome = match (self.run)(vec![pending.request]) {
                Ok(mut values) => {
                    if values.len() != 1 {
                        Err(RequestCoalescerError::Protocol(
                            "Batched request returned no result for its request".into(),
                        ))
                    } else {
                        Ok(values.remove(0))
                    }
                }
                Err(error) => Err(RequestCoalescerError::Run(error)),
            };
            let _ = pending.reply.send(outcome);
        }
    }

    fn run_batch(
        &self,
        batch: Vec<PendingRequest<Request, ResultValue, ErrorValue>>,
    ) {
        let requests = batch
            .iter()
            .map(|pending| pending.request.clone())
            .collect::<Vec<_>>();
        let outcome = match (self.run)(requests) {
            Ok(values) if values.len() == batch.len() => Ok(values),
            Ok(values) => Err(RequestCoalescerError::Protocol(format!(
                "Batched request returned {} results for {} requests",
                values.len(),
                batch.len()
            ))),
            Err(error) => Err(RequestCoalescerError::Run(error)),
        };

        match outcome {
            Ok(values) => {
                for (pending, value) in batch.into_iter().zip(values) {
                    let _ = pending.reply.send(Ok(value));
                }
            }
            Err(error)
                if batch.len() > 1
                    && self
                        .should_split_on_error
                        .as_ref()
                        .is_some_and(|should_split| should_split(&error)) =>
            {
                self.run_individually(batch);
            }
            Err(error) => {
                for pending in batch {
                    let _ = pending.reply.send(Err(error.clone()));
                }
            }
        }
    }

    fn drain(&self) {
        loop {
            let batch = self.take_batch();
            if batch.is_empty() {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.queue.is_empty() {
                    state.draining = false;
                    return;
                }
                continue;
            }
            self.run_batch(batch);
        }
    }
}
