//! Cryptographic key derivation and randomness primitives for TON wallet.

#![allow(dead_code)]

use hmac::Hmac;
use sha2::Sha512;

type HmacSha512 = Hmac<Sha512>;

pub fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], rounds: u32, output: &mut [u8]) {
    pbkdf2::pbkdf2::<HmacSha512>(password, salt, rounds, output)
        .expect("PBKDF2 output length valid");
}

pub fn generate_random_bytes(dest: &mut [u8]) {
    getrandom::getrandom(dest).expect("getrandom system entropy source available");
}
