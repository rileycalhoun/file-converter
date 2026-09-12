use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::ConversionError;

/// Bound expensive finalization/restoration work before placing it on Tokio's
/// blocking pool. Clones share the limit; waiting does not occupy worker threads.
#[derive(Clone)]
pub(crate) struct BlockingIo {
    permits: Arc<Semaphore>,
}

impl BlockingIo {
    pub(crate) fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
        }
    }

    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, ConversionError> {
        self.permits.clone().acquire_owned().await.map_err(|error| {
            ConversionError::ConversionFailed(format!("Output worker is unavailable: {error}"))
        })
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ConversionError> {
        Self::run_with_permit(self.acquire().await?, work).await
    }

    pub(crate) async fn run_with_permit<T: Send + 'static>(
        permit: OwnedSemaphorePermit,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ConversionError> {
        tokio::task::spawn_blocking(move || {
            // Keep the slot for the actual work, including if its caller drops
            // the future while the blocking task is still completing.
            let _permit = permit;
            work()
        })
        .await
        .map_err(|error| {
            ConversionError::ConversionFailed(format!("Output worker failed: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_leaves_the_async_runtime_responsive() {
        let workers = BlockingIo::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = workers.run(move || {
            started_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            42
        });
        let ui_work = async move {
            started_rx.await.unwrap();
            // On a single async thread this can run only if finalization yielded.
            release_tx.send(()).unwrap();
        };
        let (result, _) = tokio::join!(operation, ui_work);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clones_share_a_bounded_queue_and_release_slots_after_failure() {
        let workers = BlockingIo::new();
        let clone = workers.clone();
        let first = workers.acquire().await.unwrap();
        let second = clone.acquire().await.unwrap();
        assert!(workers.permits.clone().try_acquire_owned().is_err());
        drop(first);
        let result = workers.run(|| Err::<(), _>("disk full")).await.unwrap();
        assert_eq!(result, Err("disk full"));
        assert!(clone.permits.clone().try_acquire_owned().is_ok());
        drop(second);
        assert_eq!(workers.permits.available_permits(), 2);
    }
}
