use super::*;

#[derive(Debug, Deserialize)]
struct StreamErrorPayload {
    message: Option<String>,
}

pub(super) fn handle_api_auth_error(error: &ApiError, session: SessionContext) -> bool {
    if error.is_auth_error() {
        reset_session(session);
        true
    } else {
        false
    }
}

pub(super) fn close_event_stream(mut stream: Signal<Option<EventStream>>) {
    if let Some(handle) = stream.write().take() {
        handle.close();
    }
}

pub(super) fn build_dashboard_stream(
    mut dashboard: Signal<Option<DashboardSummary>>,
    mut error: Signal<Option<String>>,
    mut stream_state: Signal<StreamState>,
) -> Result<EventStream, ApiError> {
    let mut stream = EventStream::connect(&api::build_events_url(None, None, None))?;
    stream = stream.on_open(move || stream_state.set(StreamState::Live))?;
    stream = stream.on_error(move |ready_state| {
        if ready_state == EventSource::CLOSED {
            stream_state.set(StreamState::Error);
        } else {
            stream_state.set(StreamState::Reconnecting);
        }
    })?;
    stream = stream.on_message("overview.tick", move |payload| {
        match serde_json::from_str::<api::DashboardStreamEvent>(&payload) {
            Ok(event) => {
                dashboard.set(Some(event.dashboard));
                error.set(None);
                stream_state.set(StreamState::Live);
            }
            Err(decode_error) => {
                error.set(Some(format!("总览事件解析失败：{decode_error}")));
                stream_state.set(StreamState::Error);
            }
        }
    })?;
    stream = stream.on_message("stream.error", move |payload| {
        error.set(Some(parse_stream_error(&payload)));
        stream_state.set(StreamState::Error);
    })?;
    Ok(stream)
}

pub(super) fn build_node_stream(
    node_id: &str,
    mut detail: Signal<Option<NodeDetailResponse>>,
    mut error: Signal<Option<String>>,
    mut stream_state: Signal<StreamState>,
) -> Result<EventStream, ApiError> {
    let mut stream = EventStream::connect(&api::build_events_url(Some(node_id), None, None))?;
    stream = stream.on_open(move || stream_state.set(StreamState::Live))?;
    stream = stream.on_error(move |ready_state| {
        if ready_state == EventSource::CLOSED {
            stream_state.set(StreamState::Error);
        } else {
            stream_state.set(StreamState::Reconnecting);
        }
    })?;
    stream = stream.on_message("node.tick", move |payload| {
        match serde_json::from_str::<api::NodeStreamEvent>(&payload) {
            Ok(event) => {
                detail.set(Some(event.detail));
                error.set(None);
                stream_state.set(StreamState::Live);
            }
            Err(decode_error) => {
                error.set(Some(format!("节点事件解析失败：{decode_error}")));
                stream_state.set(StreamState::Error);
            }
        }
    })?;
    stream = stream.on_message("stream.error", move |payload| {
        error.set(Some(parse_stream_error(&payload)));
        stream_state.set(StreamState::Error);
    })?;
    Ok(stream)
}

fn parse_stream_error(payload: &str) -> String {
    serde_json::from_str::<StreamErrorPayload>(payload)
        .ok()
        .and_then(|payload| payload.message)
        .unwrap_or_else(|| "实时事件流返回异常".to_string())
}
