use super::super::*;

impl SharedState {
    pub fn spawn_background_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(task);
        let mut tasks =
            self.background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    pub async fn drain_background_tasks(&self) {
        for task in take_background_tasks(&self.background_tasks) {
            if let Err(error) = task.await {
                if error.is_panic() {
                    tracing::warn!(%error, "background task panicked");
                } else if !error.is_cancelled() {
                    tracing::warn!(%error, "background task failed to join");
                }
            }
        }
    }

    pub async fn abort_background_tasks(&self) {
        let tasks = take_background_tasks(&self.background_tasks);
        for task in &tasks {
            task.abort();
        }

        for task in tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "background task failed after abort");
            }
        }
    }
}
