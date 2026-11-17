use crate::e2ee::keys::{b64_decode_32, b64_decode_64, verify_signed_prekey, x25519_dh, x25519_pub_from_secret, E2eeError};
use gpui_telegram_proto::e2ee::DevicePreKeyBundle;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

pub const X3DH_HKDF_INFO: &[u8] = b"gpui-telegram-x3dh-v1";

pub struct X3dhOutput {
    pub shared_secret: Zeroizing<[u8; 32]>,
    pub associated_data: Vec<u8>,
}

pub fn x3dh_initiator(
    ik_a_sec: &[u8; 32],
    ek_a_sec: &[u8; 32],
    bundle: &DevicePreKeyBundle,
) -> Result<X3dhOutput, E2eeError> {
    let ik_b_pub = b64_decode_32(&bundle.identity_key)?;
    let ik_b_sign_pub = b64_decode_32(&bundle.identity_signing_key)?;
    let spk_b_pub = b64_decode_32(&bundle.signed_prekey.public_key)?;
    let spk_b_sig = b64_decode_64(&bundle.signed_prekey.signature)?;

    verify_signed_prekey(&ik_b_sign_pub, &spk_b_pub, &spk_b_sig)?;

    let dh1 = x25519_dh(ik_a_sec, &spk_b_pub)?;
    let dh2 = x25519_dh(ek_a_sec, &ik_b_pub)?;
    let dh3 = x25519_dh(ek_a_sec, &spk_b_pub)?;

    let mut ikm = Zeroizing::new(Vec::with_capacity(32 * 5));
    ikm.extend_from_slice(&[0xFFu8; 32]);
    ikm.extend_from_slice(&dh1);
    ikm.extend_from_slice(&dh2);
    ikm.extend_from_slice(&dh3);

    if let Some(ref otpk) = bundle.one_time_prekey {
        let opk_b_pub = b64_decode_32(&otpk.public_key)?;
        let dh4 = x25519_dh(ek_a_sec, &opk_b_pub)?;
        ikm.extend_from_slice(&dh4);
    }

    let mut okm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, &ikm)
        .expand(X3DH_HKDF_INFO, &mut *okm)
        .map_err(|_| E2eeError::X3dhFailed("HKDF expand failed".into()))?;

    let ik_a_pub = x25519_pub_from_secret(ik_a_sec);
    let mut associated_data = Vec::with_capacity(64);
    associated_data.extend_from_slice(&ik_a_pub);
    associated_data.extend_from_slice(&ik_b_pub);

    Ok(X3dhOutput {
        shared_secret: okm,
        associated_data,
    })
}

pub fn x3dh_responder(
    ik_b_sec: &[u8; 32],
    spk_b_sec: &[u8; 32],
    opk_b_sec: Option<&[u8; 32]>,
    ik_a_pub: &[u8; 32],
    ek_a_pub: &[u8; 32],
) -> Result<X3dhOutput, E2eeError> {
    let dh1 = x25519_dh(spk_b_sec, ik_a_pub)?;
    let dh2 = x25519_dh(ik_b_sec, ek_a_pub)?;
    let dh3 = x25519_dh(spk_b_sec, ek_a_pub)?;

    let mut ikm = Zeroizing::new(Vec::with_capacity(32 * 5));
    ikm.extend_from_slice(&[0xFFu8; 32]);
    ikm.extend_from_slice(&dh1);
    ikm.extend_from_slice(&dh2);
    ikm.extend_from_slice(&dh3);

    if let Some(opk_sec) = opk_b_sec {
        let dh4 = x25519_dh(opk_sec, ek_a_pub)?;
        ikm.extend_from_slice(&dh4);
    }

    let mut okm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, &ikm)
        .expand(X3DH_HKDF_INFO, &mut *okm)
        .map_err(|_| E2eeError::X3dhFailed("HKDF expand failed".into()))?;

    let ik_b_pub = x25519_pub_from_secret(ik_b_sec);
    let mut associated_data = Vec::with_capacity(64);
    associated_data.extend_from_slice(ik_a_pub);
    associated_data.extend_from_slice(&ik_b_pub);

    Ok(X3dhOutput {
        shared_secret: okm,
        associated_data,
    })
}
