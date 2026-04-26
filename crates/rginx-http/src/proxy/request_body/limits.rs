pub(in crate::proxy) fn request_body_limit_error(
    error: &(dyn std::error::Error + 'static),
) -> Option<usize> {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(limit_error) = candidate.downcast_ref::<crate::timeout::RequestBodyLimitError>()
        {
            return Some(limit_error.max_request_body_bytes());
        }

        current = candidate.source();
    }

    None
}
