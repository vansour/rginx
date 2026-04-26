use super::*;

#[tokio::test]
async fn write_timeout_io_times_out_when_write_stalls() {
    let mut writer = Box::pin(WriteTimeoutIo::new(
        DelayedWriter::new(Duration::from_millis(60)),
        Some(Duration::from_millis(20)),
        "downstream response to 127.0.0.1:8080",
    ));

    let error = poll_fn(|cx| writer.as_mut().poll_write(cx, b"ok"))
        .await
        .expect_err("writer should time out before write readiness");

    assert!(error.to_string().contains("stalled for 20 ms"));
}

#[tokio::test]
async fn write_timeout_io_allows_write_when_progress_arrives_in_time() {
    let mut writer = Box::pin(WriteTimeoutIo::new(
        DelayedWriter::new(Duration::from_millis(10)),
        Some(Duration::from_millis(50)),
        "downstream response to 127.0.0.1:8080",
    ));

    let written = poll_fn(|cx| writer.as_mut().poll_write(cx, b"ok"))
        .await
        .expect("writer should make progress before timing out");

    assert_eq!(written, 2);
}
