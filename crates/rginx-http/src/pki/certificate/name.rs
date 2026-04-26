use rasn_pkix::{DirectoryString, Name};

use super::helpers::{bytes_to_lossy_string, codepoints_to_string, hex_string};
use super::{
    OID_ATTR_COMMON_NAME, OID_ATTR_COUNTRY_NAME, OID_ATTR_DOMAIN_COMPONENT, OID_ATTR_EMAIL_ADDRESS,
    OID_ATTR_LOCALITY_NAME, OID_ATTR_ORGANIZATION_NAME, OID_ATTR_ORGANIZATIONAL_UNIT_NAME,
    OID_ATTR_STATE_OR_PROVINCE_NAME,
};

pub(super) fn name_to_string(name: &Name) -> String {
    let mut components = Vec::new();
    let Name::RdnSequence(rdns) = name;
    for rdn in rdns {
        let mut rdn_components = rdn
            .to_vec()
            .into_iter()
            .map(|attribute| {
                let oid = attribute.r#type.to_string();
                let label = attribute_label(&oid);
                let value = decode_attribute_value(&oid, &attribute.value);
                format!("{label}={value}")
            })
            .collect::<Vec<_>>();
        rdn_components.sort();
        components.extend(rdn_components);
    }
    if components.is_empty() { "-".to_string() } else { components.join(",") }
}

fn attribute_label(oid: &str) -> String {
    match oid {
        OID_ATTR_COMMON_NAME => "CN".to_string(),
        OID_ATTR_COUNTRY_NAME => "C".to_string(),
        OID_ATTR_LOCALITY_NAME => "L".to_string(),
        OID_ATTR_STATE_OR_PROVINCE_NAME => "ST".to_string(),
        OID_ATTR_ORGANIZATION_NAME => "O".to_string(),
        OID_ATTR_ORGANIZATIONAL_UNIT_NAME => "OU".to_string(),
        OID_ATTR_DOMAIN_COMPONENT => "DC".to_string(),
        OID_ATTR_EMAIL_ADDRESS => "emailAddress".to_string(),
        _ => oid.to_string(),
    }
}

fn decode_attribute_value(oid: &str, value: &rasn::types::Any) -> String {
    match oid {
        OID_ATTR_COUNTRY_NAME => decode_printable_string(value.as_bytes()),
        OID_ATTR_DOMAIN_COMPONENT | OID_ATTR_EMAIL_ADDRESS => decode_ia5_string(value.as_bytes()),
        _ => decode_directory_or_string(value.as_bytes()),
    }
}

fn decode_directory_or_string(bytes: &[u8]) -> String {
    if let Ok(value) = rasn::der::decode::<DirectoryString>(bytes) {
        return match value {
            DirectoryString::Printable(value) => bytes_to_lossy_string(value.as_bytes()),
            DirectoryString::Utf8(value) => value,
            DirectoryString::Teletex(value) => codepoints_to_string(value.iter().copied()),
            DirectoryString::Bmp(value) => {
                codepoints_to_string(value.iter().map(|&ch| u32::from(ch)))
            }
            DirectoryString::Universal(value) => value.to_string(),
        };
    }

    if let Ok(value) = rasn::der::decode::<rasn::types::PrintableString>(bytes) {
        return bytes_to_lossy_string(value.as_bytes());
    }
    if let Ok(value) = rasn::der::decode::<rasn::types::Ia5String>(bytes) {
        return value.to_string();
    }
    if let Ok(value) = rasn::der::decode::<rasn::types::Utf8String>(bytes) {
        return value;
    }

    hex_string(bytes)
}

fn decode_printable_string(bytes: &[u8]) -> String {
    rasn::der::decode::<rasn::types::PrintableString>(bytes)
        .map(|value| bytes_to_lossy_string(value.as_bytes()))
        .unwrap_or_else(|_| hex_string(bytes))
}

fn decode_ia5_string(bytes: &[u8]) -> String {
    rasn::der::decode::<rasn::types::Ia5String>(bytes)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| hex_string(bytes))
}
