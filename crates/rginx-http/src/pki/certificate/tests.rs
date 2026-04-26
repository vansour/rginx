use std::io::Write;

use proptest::prelude::*;
use tempfile::NamedTempFile;

use super::{inspect_certificate, parse_tls_client_identity};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn parse_tls_client_identity_handles_arbitrary_der_chains_without_panicking(
        der_chain in prop::collection::vec(
            prop::collection::vec(any::<u8>(), 0..256),
            0..8,
        )
    ) {
        let identity = parse_tls_client_identity(der_chain.iter().map(Vec::as_slice));

        prop_assert_eq!(identity.chain_length, der_chain.len());
        prop_assert!(identity.chain_subjects.len() <= identity.chain_length);
        prop_assert_eq!(identity.subject.is_some(), identity.issuer.is_some());
        prop_assert_eq!(identity.subject.is_some(), identity.serial_number.is_some());

        if identity.subject.is_none() {
            prop_assert!(identity.san_dns_names.is_empty());
        } else {
            prop_assert_eq!(
                identity.subject.as_deref(),
                identity.chain_subjects.first().map(String::as_str)
            );
        }
    }

    #[test]
    fn inspect_certificate_handles_arbitrary_certificate_files_without_panicking(
        bytes in prop::collection::vec(any::<u8>(), 0..1024)
    ) {
        let mut file = NamedTempFile::new().expect("temporary certificate file should be created");
        file.write_all(&bytes).expect("temporary certificate bytes should be written");

        let inspected = inspect_certificate(file.path());

        if let Some(inspected) = inspected {
            prop_assert!(inspected.chain_length >= 1);
            prop_assert!(!inspected.chain_subjects.is_empty());
            prop_assert!(inspected.chain_subjects.len() <= inspected.chain_length);
            prop_assert_eq!(
                inspected.subject.as_deref(),
                inspected.chain_subjects.first().map(String::as_str)
            );
        }
    }
}
