use instant_acme::{Account, NewAccount};
use rginx_core::{AcmeSettings, Error, Result};

use super::storage::{
    PersistedAccountCredentials, load_account_credentials, store_account_credentials,
};

pub(crate) async fn load_or_create_account(settings: &AcmeSettings) -> Result<Account> {
    if let Some(persisted) = load_account_credentials(settings)?
        && persisted.directory_url == settings.directory_url
    {
        return Account::builder()
            .map_err(|error| acme_error("failed to construct ACME account client", error))?
            .from_credentials(persisted.credentials)
            .await
            .map_err(|error| acme_error("failed to restore persisted ACME account", error));
    }

    let contacts = settings.contacts.iter().map(String::as_str).collect::<Vec<_>>();
    let (account, credentials) = Account::builder()
        .map_err(|error| acme_error("failed to construct ACME account client", error))?
        .create(
            &NewAccount {
                contact: contacts.as_slice(),
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            settings.directory_url.clone(),
            None,
        )
        .await
        .map_err(|error| acme_error("failed to create ACME account", error))?;
    store_account_credentials(
        settings,
        &PersistedAccountCredentials { directory_url: settings.directory_url.clone(), credentials },
    )?;
    Ok(account)
}

pub(crate) fn acme_error(context: &str, error: impl std::fmt::Display) -> Error {
    Error::Server(format!("{context}: {error}"))
}
