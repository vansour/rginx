use super::*;

#[tokio::test]
async fn max_bytes_body_allows_frames_within_limit() {
    let body = StreamBody::new(futures_util::stream::iter(vec![
        Ok::<_, io::Error>(Frame::data(Bytes::from_static(b"hello"))),
        Ok(Frame::data(Bytes::from_static(b"!"))),
    ]));
    let mut body = Box::pin(MaxBytesBody::new(body, 8));

    let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield first frame")
        .expect("first frame should succeed");
    assert_eq!(first.into_data().expect("frame should contain data"), Bytes::from_static(b"hello"));

    let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield second frame")
        .expect("second frame should succeed");
    assert_eq!(second.into_data().expect("frame should contain data"), Bytes::from_static(b"!"));
}

#[tokio::test]
async fn max_bytes_body_errors_when_limit_is_exceeded() {
    let body = StreamBody::new(futures_util::stream::iter(vec![
        Ok::<_, io::Error>(Frame::data(Bytes::from_static(b"hello"))),
        Ok(Frame::data(Bytes::from_static(b"world"))),
    ]));
    let mut body = Box::pin(MaxBytesBody::new(body, 8));

    let _ = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield first frame")
        .expect("first frame should succeed");

    let error = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield a terminal error")
        .expect_err("second frame should exceed the configured limit");
    assert!(error.to_string().contains("request body exceeded configured limit of 8 bytes"));
}

#[tokio::test]
async fn max_bytes_body_allows_frames_exactly_at_limit() {
    let body = StreamBody::new(futures_util::stream::iter(vec![
        Ok::<_, io::Error>(Frame::data(Bytes::from_static(b"hello"))),
        Ok(Frame::data(Bytes::from_static(b"!!!"))),
    ]));
    let mut body = Box::pin(MaxBytesBody::new(body, 8));

    let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield first frame")
        .expect("first frame should succeed");
    assert_eq!(first.into_data().expect("frame should contain data"), Bytes::from_static(b"hello"));

    let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .expect("body should yield second frame")
        .expect("second frame should succeed");
    assert_eq!(second.into_data().expect("frame should contain data"), Bytes::from_static(b"!!!"));
}
