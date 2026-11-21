pub mod agent;
mod debug;
pub mod front_hold;
mod front_queue;
pub mod server;
pub mod shared_app;
mod values;

/// Run `f`, which waits on another process, off the UI thread.
pub(super) async fn off<R: Send + 'static>(
    cx: &gpui::AsyncApp,
    f: impl FnOnce() -> R + Send + 'static,
) -> R {
    cx.background_executor().spawn(async move { f() }).await
}
