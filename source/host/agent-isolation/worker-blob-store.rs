pub trait AgentWorkerBlobPool {
    type Error;

    async fn get_blob(
        &self,
        agent_id: &str,
        blob_db_path: &str,
        blob_id: &[u8],
        legacy_blob_db_path: Option<&str>,
    ) -> Result<Option<Vec<u8>>, Self::Error>;

    async fn set_blob(
        &self,
        agent_id: &str,
        blob_db_path: &str,
        blob_id: &[u8],
        blob_data: &[u8],
        legacy_blob_db_path: Option<&str>,
    ) -> Result<(), Self::Error>;
}

pub struct WorkerBlobStore<Pool> {
    pub pool: Pool,
    pub agent_id: String,
    pub blob_db_path: String,
    pub legacy_blob_db_path: Option<String>,
}

impl<Pool> WorkerBlobStore<Pool>
where
    Pool: AgentWorkerBlobPool,
{
    pub fn new(
        pool: Pool,
        agent_id: impl Into<String>,
        blob_db_path: impl Into<String>,
        legacy_blob_db_path: Option<String>,
    ) -> Self {
        Self {
            pool,
            agent_id: agent_id.into(),
            blob_db_path: blob_db_path.into(),
            legacy_blob_db_path,
        }
    }

    pub async fn get_blob(
        &self,
        blob_id: &[u8],
    ) -> Result<Option<Vec<u8>>, Pool::Error> {
        self.pool
            .get_blob(
                &self.agent_id,
                &self.blob_db_path,
                blob_id,
                self.legacy_blob_db_path.as_deref(),
            )
            .await
    }

    pub async fn set_blob(
        &self,
        blob_id: &[u8],
        blob_data: &[u8],
    ) -> Result<(), Pool::Error> {
        self.pool
            .set_blob(
                &self.agent_id,
                &self.blob_db_path,
                blob_id,
                blob_data,
                self.legacy_blob_db_path.as_deref(),
            )
            .await
    }

    pub async fn set_blob_locally_only(
        &self,
        blob_id: &[u8],
        blob_data: &[u8],
    ) -> Result<(), Pool::Error> {
        self.set_blob(blob_id, blob_data).await
    }

    pub async fn flush(&self) -> Result<(), Pool::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Pool {
        blobs: Mutex<HashMap<(String, String, Vec<u8>), Vec<u8>>>,
        legacy_paths: Mutex<Vec<Option<String>>>,
    }

    impl AgentWorkerBlobPool for Pool {
        type Error = Infallible;

        async fn get_blob(
            &self,
            agent_id: &str,
            blob_db_path: &str,
            blob_id: &[u8],
            legacy_blob_db_path: Option<&str>,
        ) -> Result<Option<Vec<u8>>, Self::Error> {
            self.legacy_paths
                .lock()
                .unwrap()
                .push(legacy_blob_db_path.map(str::to_owned));
            Ok(self
                .blobs
                .lock()
                .unwrap()
                .get(&(agent_id.to_owned(), blob_db_path.to_owned(), blob_id.to_vec()))
                .cloned())
        }

        async fn set_blob(
            &self,
            agent_id: &str,
            blob_db_path: &str,
            blob_id: &[u8],
            blob_data: &[u8],
            legacy_blob_db_path: Option<&str>,
        ) -> Result<(), Self::Error> {
            self.legacy_paths
                .lock()
                .unwrap()
                .push(legacy_blob_db_path.map(str::to_owned));
            self.blobs.lock().unwrap().insert(
                (agent_id.to_owned(), blob_db_path.to_owned(), blob_id.to_vec()),
                blob_data.to_vec(),
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn delegates_reads_and_writes_to_agent_scoped_worker_pool() {
        let store = WorkerBlobStore::new(
            Pool::default(),
            "agent-1",
            "/data/blob.db",
            Some("/data/legacy.db".into()),
        );
        store.set_blob(b"id", b"payload").await.unwrap();
        assert_eq!(
            store.get_blob(b"id").await.unwrap(),
            Some(b"payload".to_vec())
        );
        assert_eq!(
            store.pool.legacy_paths.lock().unwrap().as_slice(),
            &[Some("/data/legacy.db".into()), Some("/data/legacy.db".into())]
        );
        store.flush().await.unwrap();
    }
}
