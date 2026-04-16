mod app;
mod components;
mod events;
mod state;
mod storage;

#[tokio::main]
async fn main() -> Result<(), app::AppError> {
    env_logger::init();
    let _ = storage::STORAGE_PLACEHOLDER;
    app::run()
}
