use std::future::Future;
use tokio::task::JoinHandle;

pub(crate) fn spawn_task<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "flutter")]
    {
        flutter_rust_bridge::spawn(future)
    }

    #[cfg(all(feature = "native", not(feature = "flutter")))]
    {
        tokio::spawn(future)
    }
}
