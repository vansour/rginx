use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL};
use http::{Method, Request, StatusCode};
use tokio::sync::Notify;

use crate::handler::full_body;

use super::*;

mod background;
mod keys;
mod recovery;
