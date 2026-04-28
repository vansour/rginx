use super::*;

pub(super) async fn spawn_range_server(seen_ranges: Arc<Mutex<Vec<String>>>) -> StatusServerHandle {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test range listener should bind");
    listener.set_nonblocking(true).expect("range listener should support nonblocking mode");
    let listen_addr = listener.local_addr().expect("listener addr should exist");
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_thread = shutdown.clone();

    let thread = thread::spawn(move || {
        while !shutdown_thread.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let seen_ranges = seen_ranges.clone();

                    thread::spawn(move || {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        let mut buffer = [0u8; 2048];
                        let bytes_read = match stream.read(&mut buffer) {
                            Ok(bytes_read) => bytes_read,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                return;
                            }
                            Err(_) => return,
                        };
                        let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                        let range = request.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.trim()
                                .eq_ignore_ascii_case("range")
                                .then_some(value.trim().to_string())
                        });
                        if let Some(range) = &range {
                            let mut seen_ranges =
                                seen_ranges.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                            seen_ranges.push(range.clone());
                        }

                        let payload = b"abcdefghijklmnopqrstuvwxyz";
                        let response = match range.and_then(|range| parse_test_range_header(&range))
                        {
                            Some((start, end)) if start < payload.len() => {
                                let end = end.min(payload.len() - 1);
                                let body = &payload[start..=end];
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {}-{}/{}\r\nconnection: close\r\n\r\n{}",
                                    body.len(),
                                    start,
                                    end,
                                    payload.len(),
                                    String::from_utf8_lossy(body)
                                )
                            }
                            Some(_) => {
                                "HTTP/1.1 416 Range Not Satisfiable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
                            }
                            None => format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                                payload.len(),
                                String::from_utf8_lossy(payload)
                            ),
                        };

                        if stream.write_all(response.as_bytes()).is_err() {
                            return;
                        }
                        let _ = stream.flush();
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    StatusServerHandle { listen_addr, shutdown, thread: Some(thread) }
}

fn parse_test_range_header(value: &str) -> Option<(usize, usize)> {
    let value = value.trim().strip_prefix("bytes=")?.trim();
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.trim().parse::<usize>().ok()?;
    let end = end.trim().parse::<usize>().ok()?;
    (start <= end).then_some((start, end))
}
