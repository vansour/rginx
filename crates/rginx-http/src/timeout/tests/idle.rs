use super::*;

#[tokio::test]
async fn idle_timeout_body_times_out_when_no_frame_arrives() {
    let mut body = Box::pin(IdleTimeoutBody::new(
        DelayedFrameBody::new(Duration::from_millis(60)),
        Duration::from_millis(20),
        "upstream `backend` response body",
    ));

    let error = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("timeout should resolve as a body error")
        .expect_err("body should time out before the frame arrives");

    assert!(error.to_string().contains("stalled for 20 ms"));
}

#[tokio::test]
async fn idle_timeout_body_allows_frames_that_arrive_in_time() {
    let mut body = Box::pin(IdleTimeoutBody::new(
        DelayedFrameBody::new(Duration::from_millis(10)),
        Duration::from_millis(50),
        "upstream `backend` response body",
    ));

    let frame = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield one frame")
        .expect("frame should be successful");
    let bytes = frame.into_data().expect("frame should contain data");

    assert_eq!(bytes, Bytes::from_static(b"ok"));
    assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
}

#[tokio::test]
async fn idle_timeout_body_waits_for_terminal_trailer_frame() {
    let mut body = Box::pin(IdleTimeoutBody::new(
        EarlyEndTrailersBody::new(),
        Duration::from_secs(1),
        "upstream `backend` response body",
    ));

    assert!(!body.as_ref().get_ref().is_end_stream());

    let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield a data frame")
        .expect("data frame should be successful");
    assert_eq!(
        first.into_data().expect("first frame should contain data"),
        Bytes::from_static(b"data")
    );
    assert!(!body.as_ref().get_ref().is_end_stream());

    let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield a trailers frame")
        .expect("trailers frame should be successful");
    let trailers = second.into_trailers().expect("second frame should contain trailers");
    assert_eq!(trailers.get("x-trailer").and_then(|value| value.to_str().ok()), Some("present"));
    assert!(body.as_ref().get_ref().is_end_stream());
    assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
}
