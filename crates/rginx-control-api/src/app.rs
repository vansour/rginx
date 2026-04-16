use axum::{Router, middleware, routing::get_service};
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;
use crate::{middleware::request_context_logging, routes};

pub fn build_router(state: AppState) -> Router {
    let ui_dir = state.ui_dir();
    let mut router = Router::new()
        .merge(routes::router())
        .layer(middleware::from_fn(request_context_logging))
        .with_state(state);

    if let Some(ui_dir) = ui_dir {
        let index_path = ui_dir.join("index.html");
        if index_path.is_file() {
            router = router.fallback_service(get_service(
                ServeDir::new(&ui_dir)
                    .append_index_html_on_directories(true)
                    .not_found_service(ServeFile::new(index_path)),
            ));
        } else {
            tracing::warn!(
                ui_dir = %ui_dir.display(),
                "control console assets are missing; API root UI is disabled"
            );
        }
    }

    router
}
