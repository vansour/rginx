use super::*;

impl CacheManager {
    pub(crate) async fn store_response(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse {
        sync_zone_shared_index_if_needed(&context.zone).await;
        let status = context.cache_status;
        let response = if context.store_response {
            match store_response(context, response).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(%error, "failed to store cached response");
                    crate::handler::text_response(
                        StatusCode::BAD_GATEWAY,
                        "text/plain; charset=utf-8",
                        format!("failed to read upstream response while caching: {error}\n"),
                    )
                }
            }
        } else {
            response
        };

        with_cache_status(response, status)
    }

    pub(crate) async fn complete_not_modified(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> std::result::Result<HttpResponse, CacheStoreError> {
        refresh_not_modified_response(context, response).await
    }
}
