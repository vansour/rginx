use std::sync::Arc;

use rginx_core::{Error, Result, ServerTls};
use rustls::ServerConfig;
use rustls::server::{NoServerSessionStorage, ProducesTickets, ServerSessionMemoryCache};

pub(super) fn apply_session_policy(
    config: &mut ServerConfig,
    tls: Option<&ServerTls>,
) -> Result<()> {
    let Some(tls) = tls else {
        return Ok(());
    };

    if matches!(tls.session_resumption, Some(false)) {
        config.session_storage = Arc::new(NoServerSessionStorage {});
        config.ticketer = Arc::new(DisabledTicketProducer {});
        config.send_tls13_tickets = 0;
        return Ok(());
    }

    if let Some(session_cache_size) = tls.session_cache_size {
        config.session_storage = if session_cache_size == 0 {
            Arc::new(NoServerSessionStorage {})
        } else {
            ServerSessionMemoryCache::new(session_cache_size)
        };
    }

    if matches!(tls.session_tickets, Some(false)) {
        config.ticketer = Arc::new(DisabledTicketProducer {});
        config.send_tls13_tickets = 0;
    } else if matches!(tls.session_tickets, Some(true)) || tls.session_ticket_count.is_some() {
        config.ticketer = rustls::crypto::aws_lc_rs::Ticketer::new().map_err(|error| {
            Error::Server(format!("failed to enable server TLS session tickets: {error}"))
        })?;
        config.send_tls13_tickets = tls.session_ticket_count.unwrap_or(2);
    }

    Ok(())
}

#[derive(Debug)]
struct DisabledTicketProducer {}

impl ProducesTickets for DisabledTicketProducer {
    fn enabled(&self) -> bool {
        false
    }

    fn lifetime(&self) -> u32 {
        0
    }

    fn encrypt(&self, _plain: &[u8]) -> Option<Vec<u8>> {
        None
    }

    fn decrypt(&self, _cipher: &[u8]) -> Option<Vec<u8>> {
        None
    }
}
