use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamState {
    #[default]
    Idle,
    Connecting,
    Live,
    Reconnecting,
    Error,
}

pub fn format_unix_ms(value: Option<u64>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };

    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(value as f64));
    let year = date.get_full_year();
    let month = date.get_month() + 1;
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    let seconds = date.get_seconds();
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}")
}

pub fn format_list<I, T>(values: I) -> String
where
    I: IntoIterator<Item = T>,
    T: ToString,
{
    let values = values
        .into_iter()
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() { "-".to_string() } else { values.join(", ") }
}

pub fn format_optional<T: ToString>(value: Option<T>) -> String {
    match value {
        Some(value) => {
            let text = value.to_string();
            if text.trim().is_empty() { "-".to_string() } else { text }
        }
        None => "-".to_string(),
    }
}

pub fn format_bool(value: Option<bool>, true_label: &str, false_label: &str) -> String {
    match value {
        Some(true) => true_label.to_string(),
        Some(false) => false_label.to_string(),
        None => "-".to_string(),
    }
}

pub fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
}

pub fn stream_state_label(state: StreamState) -> &'static str {
    match state {
        StreamState::Idle => "idle",
        StreamState::Connecting => "connecting",
        StreamState::Live => "live",
        StreamState::Reconnecting => "reconnecting",
        StreamState::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[test]
    fn format_list_filters_blank_values() {
        assert_eq!(format_list(["alpha", "", " beta "]), "alpha,  beta ");
        assert_eq!(format_list(Vec::<String>::new()), "-");
    }

    #[test]
    fn format_optional_and_bool_use_console_fallbacks() {
        assert_eq!(format_optional(Some("value")), "value");
        assert_eq!(format_optional(Some("")), "-");
        assert_eq!(format_optional::<String>(None), "-");

        assert_eq!(format_bool(Some(true), "启用", "停用"), "启用");
        assert_eq!(format_bool(Some(false), "启用", "停用"), "停用");
        assert_eq!(format_bool(None, "启用", "停用"), "-");
    }

    #[test]
    fn pretty_json_renders_stable_pretty_output() {
        #[derive(Serialize)]
        struct Payload<'a> {
            name: &'a str,
            count: u32,
        }

        let rendered = pretty_json(&Payload { name: "alerts", count: 2 });
        assert!(rendered.contains("\"name\": \"alerts\""));
        assert!(rendered.contains("\"count\": 2"));
    }

    #[test]
    fn stream_state_labels_match_css_contract() {
        assert_eq!(stream_state_label(StreamState::Idle), "idle");
        assert_eq!(stream_state_label(StreamState::Connecting), "connecting");
        assert_eq!(stream_state_label(StreamState::Live), "live");
        assert_eq!(stream_state_label(StreamState::Reconnecting), "reconnecting");
        assert_eq!(stream_state_label(StreamState::Error), "error");
    }
}
