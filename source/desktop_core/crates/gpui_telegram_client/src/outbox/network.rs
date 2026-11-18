use crate::net::ConnectionStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::{watch, Notify};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Online,
    Offline,
}

impl NetworkState {
    pub fn is_online(self) -> bool {
        matches!(self, NetworkState::Online)
    }
}

pub struct NetworkStatusListener {
    state_tx: watch::Sender<NetworkState>,
    state_rx: watch::Receiver<NetworkState>,
    reconnect_notify: Arc<Notify>,
    reconnect_epoch: AtomicU64,
    reconnect_epoch_tx: watch::Sender<u64>,
}

impl std::fmt::Debug for NetworkStatusListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkStatusListener")
            .field("state", &*self.state_rx.borrow())
            .field("reconnect_epoch", &self.reconnect_epoch.load(Ordering::SeqCst))
            .finish()
    }
}

impl NetworkStatusListener {
    pub fn new(initial: NetworkState) -> Arc<Self> {
        let (state_tx, state_rx) = watch::channel(initial);
        let (reconnect_epoch_tx, _) = watch::channel(0u64);
        Arc::new(Self {
            state_tx,
            state_rx,
            reconnect_notify: Arc::new(Notify::new()),
            reconnect_epoch: AtomicU64::new(0),
            reconnect_epoch_tx,
        })
    }

    pub fn state(&self) -> NetworkState {
        *self.state_rx.borrow()
    }

    pub fn is_online(&self) -> bool {
        self.state().is_online()
    }

    pub fn subscribe(&self) -> watch::Receiver<NetworkState> {
        self.state_rx.clone()
    }

    pub fn reconnect_epoch(&self) -> u64 {
        self.reconnect_epoch.load(Ordering::SeqCst)
    }

    pub fn subscribe_reconnect_epoch(&self) -> watch::Receiver<u64> {
        self.reconnect_epoch_tx.subscribe()
    }

    fn bump_reconnect_epoch(&self) {
        let epoch = self.reconnect_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.reconnect_epoch_tx.send(epoch);
        self.reconnect_notify.notify_waiters();
    }

    pub fn set_online(&self, online: bool) {
        let prev = *self.state_rx.borrow();
        let next = if online {
            NetworkState::Online
        } else {
            NetworkState::Offline
        };
        if prev != next {
            let _ = self.state_tx.send(next);
            if !prev.is_online() && next.is_online() {
                self.bump_reconnect_epoch();
            }
        }
    }

    pub fn notify_reconnected(&self) {
        let _ = self.state_tx.send(NetworkState::Online);
        self.bump_reconnect_epoch();
    }

    pub fn reconnect_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.reconnect_notify)
    }

    pub fn attach_ws_status(
        self: &Arc<Self>,
        mut ws_rx: watch::Receiver<ConnectionStatus>,
        handle: &Handle,
    ) {
        let listener = Arc::clone(self);
        let initial_connected = ws_rx.borrow().is_connected();
        if initial_connected {
            listener.set_online(true);
        }
        handle.spawn(async move {
            while ws_rx.changed().await.is_ok() {
                let is_conn = ws_rx.borrow().is_connected();
                listener.set_online(is_conn);
            }
        });
    }
}
