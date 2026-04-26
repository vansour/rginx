use super::*;

#[tokio::test]
async fn grpc_deadline_body_emits_deadline_exceeded_trailers_before_first_frame() {
    let deadline = Instant::now() + Duration::from_millis(20);
    let mut body = Box::pin(GrpcDeadlineBody::new(
        DelayedFrameBody::new(Duration::from_millis(60)),
        deadline,
        Duration::from_millis(20),
        "upstream `backend` response body",
        "upstream `backend` timed out after 20 ms",
    ));

    let frame = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("deadline should emit a trailers frame")
        .expect("deadline trailers should be successful");
    let trailers = frame.into_trailers().expect("deadline should surface as trailers");

    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("4"));
    assert_eq!(
        trailers.get("grpc-message").and_then(|value| value.to_str().ok()),
        Some("upstream `backend` timed out after 20 ms")
    );
    assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
}

#[tokio::test]
async fn grpc_deadline_body_keeps_absolute_deadline_after_progress() {
    let deadline = Instant::now() + Duration::from_millis(30);
    let mut body = Box::pin(GrpcDeadlineBody::new(
        TwoStageBody::new(Duration::from_millis(5), Duration::from_millis(80)),
        deadline,
        Duration::from_millis(30),
        "upstream `backend` response body",
        "upstream `backend` timed out after 30 ms",
    ));

    let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield the first data frame")
        .expect("first frame should be successful");
    assert_eq!(
        first.into_data().expect("first frame should contain data"),
        Bytes::from_static(b"ok")
    );

    let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("deadline should terminate the stream with trailers")
        .expect("deadline trailers should be successful");
    let trailers = second.into_trailers().expect("deadline should surface as trailers");
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("4"));
}
