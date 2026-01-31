use std::future::Future;
use std::sync::Arc;

use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct InProcessQueue {
    semaphore: Arc<Semaphore>,
}

impl InProcessQueue {
    pub fn new(max_concurrency: usize) -> Self {
        let permits = max_concurrency.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
        }
    }

    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let semaphore = Arc::clone(&self.semaphore);
        tokio::spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("in-process queue semaphore is closed");
            fut.await;
        });
    }
}
