//! Proton's SRP-6a login, ported from `github.com/ProtonMail/go-srp` (MIT).
//!
//! Proton diverges from RFC 5054 in three ways, and all three are load-bearing:
//! the password is bcrypt-hashed rather than plain SHA-1 (the salt is the account
//! salt plus the literal `"proton"`), every hash is a 256-byte `expandHash`
//! (SHA-512 of the input with each of the four suffixes 0..3 appended), and all
//! group elements travel as little-endian fixed-width integers. `MailboxPassword`
//! is the same bcrypt trick without the modulus, and its last 31 bytes are the
//! passphrase that unlocks the user's OpenPGP keys.

use num_bigint_dig::BigUint;
use sha2::{Digest, Sha512};

/// Key length of Proton's SRP group (a fixed 2048-bit safe prime).
const BITS: usize = 2048;
/// Group elements on the wire are exactly this many little-endian bytes.
const ELEM: usize = BITS / 8;
/// bcrypt cost Proton uses for both login and key salts.
const BCRYPT_COST: u32 = 10;

fn expand_hash(data: &[u8]) -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut hasher = Sha512::new();
    for part in 0..4u8 {
        hasher.update(data);
        hasher.update([part]);
        out[usize::from(part) * 64..usize::from(part) * 64 + 64]
            .copy_from_slice(&hasher.finalize_reset());
    }
    out
}

fn to_int(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_le(bytes)
}

/// `bitLength`-wide little-endian encoding of `value`.
fn from_int(value: &BigUint) -> Vec<u8> {
    let mut out = value.to_bytes_le();
    out.resize(ELEM, 0);
    out
}

/// bcrypt over Proton's salt encoding. Returns the full `$2y$10$…` string.
fn bcrypt_hash(password: &[u8], salt: &[u8]) -> anyhow::Result<String> {
    anyhow::ensure!(!salt.is_empty(), "bcrypt salt is empty");
    // Proton's alphabet is bcrypt's own (`./A-Za-z0-9`, no padding), so encoding the
    // salt with the crate's encoder reproduces the salt string byte for byte. Only the
    // first 16 bytes survive: Go's reader stops after the 22 characters that decode to
    // them (`saltb[:SaltLen]`), which is why the `"proton"` suffix of a 16-byte account
    // salt never reaches the hash.
    let mut raw = [0u8; 16];
    let used = salt.len().min(raw.len());
    raw[..used].copy_from_slice(&salt[..used]);
    let parts = bcrypt::hash_with_salt(password, BCRYPT_COST, raw)?;
    Ok(parts.format_for_version(bcrypt::Version::TwoY))
}

/// The 31-byte passphrase that protects a key: bcrypt of the mailbox password with
/// that key's salt, minus the algorithm/salt prefix. `salts.SaltForKey` in
/// go-proton-api.
pub fn key_passphrase(password: &str, salt: &[u8]) -> anyhow::Result<String> {
    let hashed = bcrypt_hash(password.as_bytes(), salt)?;
    Ok(hashed[hashed.len() - 31..].to_string())
}

/// `HashPassword` for auth version 4: bcrypt of the password with the account salt
/// plus `"proton"`, then expanded together with the modulus.
pub fn hashed_password(password: &[u8], salt: &[u8], modulus: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut salted = salt.to_vec();
    salted.extend_from_slice(b"proton");
    let crypted = bcrypt_hash(password, &salted)?;
    let mut data = crypted.into_bytes();
    data.extend_from_slice(modulus);
    Ok(expand_hash(&data).to_vec())
}

/// Proofs a client sends after `/auth/v4/info`.
pub struct Proofs {
    pub client_ephemeral: Vec<u8>,
    pub client_proof: Vec<u8>,
    pub expected_server_proof: Vec<u8>,
}

/// Client half of the exchange. `modulus` and `server_ephemeral` are the raw bytes
/// behind the base64 fields of `/auth/v4/info`.
pub fn proofs(
    modulus: &[u8],
    server_ephemeral: &[u8],
    hashed_password: &[u8],
) -> anyhow::Result<Proofs> {
    use anyhow::bail;
    let n = to_int(modulus);
    ensure_group(&n)?;
    let b = to_int(server_ephemeral);
    let n_minus_one = &n - BigUint::from(1u8);
    if b <= BigUint::from(1u8) || b >= n_minus_one {
        bail!("Proton's SRP server ephemeral is out of range");
    }

    // a is random, above 2*bitLength and below N-1.
    let mut rng = rand::thread_rng();
    let secret = loop {
        let a = random_below(&n_minus_one, &mut rng);
        if a > BigUint::from(2 * BITS as u64) {
            break a;
        }
    };
    Ok(proofs_for_secret(
        &n,
        server_ephemeral,
        hashed_password,
        secret,
    ))
}

/// The deterministic rest of the exchange for a chosen client secret `a`.
fn proofs_for_secret(
    n: &BigUint,
    server_ephemeral: &[u8],
    hashed_password: &[u8],
    secret: BigUint,
) -> Proofs {
    let g = BigUint::from(2u8);
    let b = to_int(server_ephemeral);
    let client_ephemeral_bytes = from_int(&g.modpow(&secret, n));

    let k = to_int(&expand_hash(&[from_int(&g), from_int(n)].concat())) % n;
    let u = to_int(&expand_hash(
        &[client_ephemeral_bytes.clone(), server_ephemeral.to_vec()].concat(),
    ));
    let x = to_int(hashed_password);

    // S = (B - k*g^x mod N)^(u*x + a) mod N
    let base = (&b + n - (&k * g.modpow(&x, n)) % n) % n;
    let exponent = u * x + secret;
    let shared = base.modpow(&exponent, n);
    let shared = from_int(&shared);

    let client_proof = expand_hash(
        &[
            client_ephemeral_bytes.clone(),
            server_ephemeral.to_vec(),
            shared.clone(),
        ]
        .concat(),
    );
    let expected_server_proof = expand_hash(
        &[
            client_ephemeral_bytes.clone(),
            client_proof.to_vec(),
            shared,
        ]
        .concat(),
    )
    .to_vec();
    Proofs {
        client_ephemeral: client_ephemeral_bytes,
        client_proof: client_proof.to_vec(),
        expected_server_proof,
    }
}

fn ensure_group(n: &BigUint) -> anyhow::Result<()> {
    use anyhow::bail;
    if n.bits() != BITS {
        bail!("Proton's SRP modulus is not {} bits", BITS);
    }
    // N must be 3 mod 8 so that 2 generates the whole group, not just the squares.
    if (n % BigUint::from(8u8)) != BigUint::from(3u8) {
        bail!("Proton's SRP modulus is not 3 mod 8");
    }
    Ok(())
}

fn random_below(limit: &BigUint, mut rng: impl rand::Rng) -> BigUint {
    let mut bytes = vec![0u8; limit.bits().div_ceil(8)];
    rng.fill(&mut bytes[..]);
    BigUint::from_bytes_le(&bytes) % limit
}

/// Verification helper for the local test server: `M2 = expandHash(A || M || S)`
/// and the verifier `v = g^x mod N`.
#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// `hash_test.go` in go-srp: bcrypt of "test!!!" with the salt string
    /// `PTTsDBs/mlLnSk6VmtFghe` must give this exact hash.
    #[test]
    fn bcrypt_matches_go_srp_vector() {
        let salt = [
            0x45, 0x55, 0x6e, 0x14, 0x3b, 0x81, 0xa2, 0x73, 0x69, 0x52, 0x6f, 0x17, 0xa2, 0xf1,
            0xe2, 0x8e,
        ];
        assert_eq!(
            bcrypt_hash(b"test!!!", &salt).expect("hash"),
            "$2y$10$PTTsDBs/mlLnSk6VmtFgheNSiK/lSwtJsrBLLDK3kZYI7193nInqy"
        );
    }

    /// `TestSRPauth` in go-srp: user "jakubqa", password "abc123", Proton's published
    /// modulus and server ephemeral. `SECRET` is the client secret go-srp draws from
    /// its seeded test RNG (recovered by replaying that RNG against go-srp itself), so
    /// both proofs must equal the ones go-srp's test publishes, byte for byte.
    #[test]
    fn login_proofs_match_go_srp_vector() {
        use crate::sync::proton::crypto::unb64;
        const MODULUS: &str = "W2z5HBi8RvsfYzZTS7qBaUxxPhsfHJFZpu3Kd6s1JafNrCCH9rfvPLrfuqocxWPgWDH2R8neK7PkNvjxto9TStuY5z7jAzWRvFWN9cQhAKkdWgy0JY6ywVn22+HFpF4cYesHrqFIKUPDMSSIlWjBVmEJZ/MusD44ZT29xcPrOqeZvwtCffKtGAIjLYPZIEbZKnDM1Dm3q2K/xS5h+xdhjnndhsrkwm9U9oyA2wxzSXFL+pdfj2fOdRwuR5nW0J2NFrq3kJjkRmpO/Genq1UW+TEknIWAb6VzJJJA244K/H8cnSx2+nSNZO3bbo6Ys228ruV9A8m6DhxmS+bihN3ttQ==";
        const SERVER_EPHEMERAL: &str = "l13IQSVFBEV0ZZREuRQ4ZgP6OpGiIfIjbSDYQG3Yp39FkT2B/k3n1ZhwqrAdy+qvPPFq/le0b7UDtayoX4aOTJihoRvifas8Hr3icd9nAHqd0TUBbkZkT6Iy6UpzmirCXQtEhvGQIdOLuwvy+vZWh24G2ahBM75dAqwkP961EJMh67/I5PA5hJdQZjdPT5luCyVa7BS1d9ZdmuR0/VCjUOdJbYjgtIH7BQoZs+KacjhUN8gybu+fsycvTK3eC+9mCN2Y6GdsuCMuR3pFB0RF9eKae7cA6RbJfF1bjm0nNfWLXzgKguKBOeF3GEAsnCgK68q82/pq9etiUDizUlUBcA==";
        const SECRET: &str = "b19256d09afdcac0ba7489ff27ed6f6b57e3a83f0216c1d40e4144f3fd47894961108e6517386e052b8e822623c9053f275e7029e1b2745b5ac22ddca296e771701c1ca100da07b62c695f882dcdda00e18728100cbefdfe12575b49e0c37baef3c4c6e30ced8a97160662785a4643aa3c903ced428a1051a7821b88aeaf0b1b8ec7a2cf79557c87a22ece04ac1add13ac8f6d54eaeb1cfe4f8c3ed6e83e84badc4ce36fbd4b9ee7818e744ee5826f9eda8fe12919f9f41db402bc75d3613f0858079b70e0cb1a843585884c14d6c00c5ba53b0d9f9f0c7a619b42764d9bd7df529ddd09ba923e34d9c90952f284145b9fe872b44b9fbb971bbf64b1967f8c53";
        const CLIENT_PROOF: &str = "Qb+1+jEqHRqpJ3nEJX2FEj0kXgCIWHngO0eT4R2Idkwke/ceCIUmQa0RfTYU53ybO1AVergtb7N0W/3bathdHT9FAHhy0vDGQDg/yPnuUneqV76NuU+pQHnO83gcjmZjDq/zvRRSD7dtIORRK97xhdR9W9bG5XRGr2c9Zev40YVcXgUiNUG/0zHSKQfEhUpMKxdauKtGC+dZnZzU6xaU0qvulYEsraawurRf0b1VXwohM6KE52Fj5xlS2FWZ3Mg0WIOC5KW5ziI6QirEUDK2pH/Rxvu4HcW9aMuppUmHk9Bm6kdg99o3vl0G7OgmEI7y6iyEYmXqH44XGORJ2sDMxQ==";
        const SERVER_PROOF: &str = "SLCSIClioSAtozauZZzcJuVPyY+MjnxfJSgEe9y6RafgjlPqnhQTZclRKPGsEhxVyWan7PIzhL+frPyZNaE1QaV5zbqz1yf9RXpGyTjZwU3FuVCJpkhp6iiCK3Wd2SemxawFXC06dgAdJ7I3HKvfkXeMANOUUh5ofjnJtXg42OGp4x1lKoFcH+IbB/CvRNQCmRTyhOiBJmZyUFwxHXLT/h+PlD0XSehcyybIIBIsscQ7ZPVPxQw4BqlqoYzTjjXPJxLxeQUQm2g9bPzT+izuR0VOPDtjt+dXrWny90k2nzS0Bs2YvNIqbJn1aQwFZr42p/O1I9n5S3mYtMgGk/7b1g==";

        let modulus = unb64(MODULUS).expect("modulus");
        let n = to_int(&modulus);
        ensure_group(&n).expect("Proton's group");
        let salt = unb64("yKlc5/CvObfoiw==").expect("salt");
        let hashed = hashed_password(b"abc123", &salt, &modulus).expect("hash");
        let server_ephemeral = unb64(SERVER_EPHEMERAL).expect("B");

        let proofs = proofs_for_secret(&n, &server_ephemeral, &hashed, to_int(&hex(SECRET)));
        assert_eq!(proofs.client_proof, unb64(CLIENT_PROOF).expect("M"));
        assert_eq!(
            proofs.expected_server_proof,
            unb64(SERVER_PROOF).expect("M2")
        );
    }

    /// `expandHash` is SHA-512 of the data with the suffixes 0..3 appended.
    #[test]
    fn expand_hash_is_four_suffixes() {
        let expanded = expand_hash(b"abc");
        let mut manual = Vec::new();
        for part in 0..4u8 {
            manual.extend_from_slice(&Sha512::digest([b'a', b'b', b'c', part]));
        }
        assert_eq!(&expanded[..], &manual[..]);
        assert_eq!(&expanded[64..128], &Sha512::digest(b"abc\x01")[..]);
    }

    /// The 2048-bit group Proton publishes (its clear-signed modulus) parses as
    /// little-endian; a wrong endianness would make the proofs scramble.
    #[test]
    fn little_endian_roundtrip() {
        let bytes = hex(&"01".repeat(ELEM));
        assert_eq!(from_int(&to_int(&bytes)), bytes);
        assert_eq!(from_int(&BigUint::from(1u8))[..2], [1, 0]);
    }
}
