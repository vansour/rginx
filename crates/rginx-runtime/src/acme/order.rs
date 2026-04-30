use std::sync::Arc;

use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, NewOrder, OrderStatus,
};
use rginx_core::{Error, ManagedCertificateSpec, Result};

use super::account::acme_error;
use super::challenge::ChallengeBackend;
use super::storage::write_certificate_pair;
use super::types::acme_poll_retry_policy;

pub(crate) async fn issue_and_store_managed_certificate(
    spec: &ManagedCertificateSpec,
    account: &Account,
    challenge_backend: Arc<dyn ChallengeBackend>,
) -> Result<()> {
    let identifiers = spec.domains.iter().cloned().map(Identifier::Dns).collect::<Vec<_>>();
    let mut order = account
        .new_order(&NewOrder::new(identifiers.as_slice()))
        .await
        .map_err(|error| acme_error("failed to create ACME order", error))?;

    let mut registered_tokens = Vec::<String>::new();
    let result = async {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authorization =
                result.map_err(|error| acme_error("failed to fetch ACME authorization", error))?;
            match authorization.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => {
                    let identifier = authorization.identifier().to_string();
                    return Err(Error::Server(format!(
                        "ACME authorization for `{}` is in unexpected state `{other:?}`",
                        identifier
                    )));
                }
            }

            let identifier = authorization.identifier().to_string();
            let mut challenge =
                authorization.challenge(ChallengeType::Http01).ok_or_else(|| {
                    Error::Server(format!(
                        "ACME authorization for `{}` does not provide an HTTP-01 challenge",
                        identifier
                    ))
                })?;
            if challenge.token.is_empty() {
                return Err(Error::Server(format!(
                    "ACME HTTP-01 challenge for `{}` is missing its token",
                    challenge.identifier()
                )));
            }

            let token = challenge.token.clone();
            challenge_backend
                .register_http01(token.clone(), challenge.key_authorization().as_str().to_string());
            registered_tokens.push(token);
            challenge.set_ready().await.map_err(|error| {
                acme_error("failed to mark ACME HTTP-01 challenge ready", error)
            })?;
        }

        let ready_status = order.poll_ready(&acme_poll_retry_policy()).await.map_err(|error| {
            acme_error("failed while waiting for ACME order to become ready", error)
        })?;
        if ready_status != OrderStatus::Ready {
            return Err(Error::Server(format!(
                "ACME order for `{}` ended in unexpected state `{ready_status:?}`",
                spec.scope
            )));
        }

        let private_key_pem = order
            .finalize()
            .await
            .map_err(|error| acme_error("failed to finalize ACME order", error))?;
        let certificate_chain_pem = order
            .poll_certificate(&acme_poll_retry_policy())
            .await
            .map_err(|error| acme_error("failed while waiting for ACME certificate", error))?;
        write_certificate_pair(spec, &certificate_chain_pem, &private_key_pem)
    }
    .await;

    for token in registered_tokens {
        challenge_backend.unregister_http01(&token);
    }

    result
}
