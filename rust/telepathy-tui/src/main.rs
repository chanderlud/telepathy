mod app;
mod components;
mod events;
mod state;
mod storage;

#[tokio::main]
async fn main() -> Result<(), app::AppError> {
    env_logger::init();
    app::run()
}
