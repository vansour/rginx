use super::*;

pub(super) fn generalized_time_from_system_time(time: SystemTime) -> GeneralizedTime {
    let utc = DateTime::<Utc>::from(time);
    utc.fixed_offset()
}

pub(super) fn certificate_valid_now(cert: &RasnCertificate, now: SystemTime) -> bool {
    let now = DateTime::<Utc>::from(now).timestamp();
    let Some(not_before) = rasn_time_to_unix_seconds(cert.tbs_certificate.validity.not_before)
    else {
        return false;
    };
    let Some(not_after) = rasn_time_to_unix_seconds(cert.tbs_certificate.validity.not_after) else {
        return false;
    };
    not_before <= now && now <= not_after
}

pub(super) fn rasn_time_to_unix_seconds(time: RasnTime) -> Option<i64> {
    Some(match time {
        RasnTime::Utc(value) => value.timestamp(),
        RasnTime::General(value) => value.timestamp(),
    })
}
