use http::HeaderMap;
use http::header::{ACCEPT_ENCODING, HeaderValue, VARY};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContentCoding {
    Brotli,
    Gzip,
}

impl ContentCoding {
    pub(super) fn header_value(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    pub(super) fn label(self) -> &'static str {
        self.header_value()
    }
}

pub(super) fn merge_vary_header(headers: &mut HeaderMap, token: &str) {
    let mut values = Vec::<String>::new();

    for value in headers.get_all(VARY).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for item in value.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            if item == "*" {
                headers.insert(VARY, HeaderValue::from_static("*"));
                return;
            }
            if !values.iter().any(|existing| existing.eq_ignore_ascii_case(item)) {
                values.push(item.to_string());
            }
        }
    }

    if !values.iter().any(|existing| existing.eq_ignore_ascii_case(token)) {
        values.push(token.to_string());
    }

    if let Ok(value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(VARY, value);
    } else {
        headers.append(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

pub(super) fn preferred_response_encoding(headers: &HeaderMap) -> Option<ContentCoding> {
    #[derive(Default)]
    struct AcceptedEncodings {
        brotli: Option<f32>,
        gzip: Option<f32>,
        wildcard: Option<f32>,
    }

    impl AcceptedEncodings {
        fn record(&mut self, coding: &str, q: f32) {
            let slot = if coding.eq_ignore_ascii_case("br") {
                Some(&mut self.brotli)
            } else if coding.eq_ignore_ascii_case("gzip") {
                Some(&mut self.gzip)
            } else if coding == "*" {
                Some(&mut self.wildcard)
            } else {
                None
            };

            if let Some(slot) = slot {
                let updated = (*slot).map_or(q, |current| current.max(q));
                *slot = Some(updated);
            }
        }

        fn quality_for(&self, coding: ContentCoding) -> f32 {
            match coding {
                ContentCoding::Brotli => self.brotli.or(self.wildcard).unwrap_or(0.0),
                ContentCoding::Gzip => self.gzip.or(self.wildcard).unwrap_or(0.0),
            }
        }
    }

    let mut accepted = AcceptedEncodings::default();

    for (coding, q) in headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(parse_accept_encoding_item)
    {
        accepted.record(coding, q);
    }

    let brotli_q = accepted.quality_for(ContentCoding::Brotli);
    let gzip_q = accepted.quality_for(ContentCoding::Gzip);

    match (brotli_q > 0.0, gzip_q > 0.0) {
        (false, false) => None,
        (true, false) => Some(ContentCoding::Brotli),
        (false, true) => Some(ContentCoding::Gzip),
        (true, true) if brotli_q >= gzip_q => Some(ContentCoding::Brotli),
        (true, true) => Some(ContentCoding::Gzip),
    }
}

fn parse_accept_encoding_item(item: &str) -> Option<(&str, f32)> {
    let mut parts = item.split(';');
    let coding = parts.next()?.trim();
    if coding.is_empty() {
        return None;
    }

    let mut q = 1.0;
    for part in parts {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("q=") {
            q = value.parse::<f32>().ok()?;
        }
    }

    Some((coding, q))
}
