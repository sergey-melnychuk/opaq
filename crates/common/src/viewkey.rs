//! Viewing keys & received-note discovery (OPAQ.md B.13, Phase 2.5).
//!
//! `spend_key` (existing, B.2) authenticates spending; it cannot discover
//! which on-chain commitments belong to you, since `commitment =
//! Poseidon(token_id, amount, owner_pubkey, blinding_factor)` is one opaque
//! hash and `blinding_factor` is unguessable. `view_key` here is a SEPARATE
//! secret (X25519, unrelated to BN254/`spend_key`) whose only job is
//! encrypting/decrypting the note-opening memo a sender attaches to a
//! `transfer` output — letting the recipient find and recover notes sent to
//! them without any out-of-band handoff.
//!
//! Deliberately independent of `spend_key` so it's cheaply rotatable (B.13.1):
//! `owner_pubkey = Poseidon(spend_key)` is permanently baked into every
//! existing commitment, so a `view_key` derived FROM `spend_key` could never
//! be rotated without abandoning that identity (and moving funds). A
//! stand-alone `view_key` can be replaced any time by publishing a new
//! `viewing_pubkey` — no on-chain action, no funds moved. See B.13.5 for what
//! rotation does and does NOT protect (past memos stay decryptable).

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand_core::{OsRng, RngCore};
use x25519_dalek::{PublicKey, StaticSecret};

/// A user's viewing secret. Independent of `spend_key` (B.13.1) — generate
/// fresh, store alongside `spend_key`, and feel free to rotate without
/// touching `spend_key` or any existing note.
pub struct ViewKey(StaticSecret);

impl ViewKey {
    /// Fresh random viewing key (also used for rotation — the old key is
    /// simply discarded and a new one takes over for FUTURE memos, B.13.5).
    pub fn generate() -> Self {
        Self(StaticSecret::random_from_rng(OsRng))
    }

    pub fn viewing_pubkey(&self) -> [u8; 32] {
        PublicKey::from(&self.0).to_bytes()
    }

    /// Persist the secret scalar (e.g. into an encrypted identity file) so a
    /// generated `view_key` survives past the process — mirrors how
    /// `spend_key` is already stored in a note/identity file.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(StaticSecret::from(b))
    }

    /// Recipient side of B.13.3/B.13.5: derive the same shared secret an
    /// ephemeral sender key would have produced against our `viewing_pubkey`.
    fn shared_secret(&self, ephemeral_pubkey: &[u8; 32]) -> [u8; 32] {
        self.0.diffie_hellman(&PublicKey::from(*ephemeral_pubkey)).to_bytes()
    }
}

/// The note-opening payload a sender encrypts for a transfer output's owner:
/// everything needed to reconstruct the commitment (with the recipient's own
/// `owner_pubkey`) and, with `spend_key`, the nullifier (B.13.3 step 4).
pub struct NoteOpening {
    pub token_id: [u8; 32],
    pub amount: [u8; 32],
    pub blinding_factor: [u8; 32],
}

impl NoteOpening {
    fn to_bytes(&self) -> [u8; 96] {
        let mut out = [0u8; 96];
        out[0..32].copy_from_slice(&self.token_id);
        out[32..64].copy_from_slice(&self.amount);
        out[64..96].copy_from_slice(&self.blinding_factor);
        out
    }

    fn from_bytes(b: &[u8; 96]) -> Self {
        Self {
            token_id: b[0..32].try_into().unwrap(),
            amount: b[32..64].try_into().unwrap(),
            blinding_factor: b[64..96].try_into().unwrap(),
        }
    }
}

/// Wire format riding along in the `transfer` instruction's trailing data
/// (B.13.4): `epk (32B) || nonce (12B) || ciphertext (96B + 16B tag)` = 156B.
/// Not a circuit input, not parsed by the program — zero ceremony impact.
pub struct Memo {
    pub ephemeral_pubkey: [u8; 32],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>, // 112 bytes: 96 plaintext + 16-byte AEAD tag
}

impl Memo {
    pub const LEN: usize = 32 + 12 + 96 + 16;

    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..32].copy_from_slice(&self.ephemeral_pubkey);
        out[32..44].copy_from_slice(&self.nonce);
        out[44..].copy_from_slice(&self.ciphertext);
        out
    }

    pub fn from_bytes(b: &[u8; Self::LEN]) -> Self {
        Self {
            ephemeral_pubkey: b[0..32].try_into().unwrap(),
            nonce: b[32..44].try_into().unwrap(),
            ciphertext: b[44..].to_vec(),
        }
    }
}

fn kdf(shared_secret: &[u8; 32]) -> [u8; 32] {
    // BLAKE3 keyed-hash mode as a KDF: domain-separates from any other BLAKE3
    // use and is a standard "hash the ECDH output" construction.
    blake3::derive_key("opaq/viewkey/v1", shared_secret)
}

/// Sender side (B.13.3): encrypt `opening` for `recipient_viewing_pubkey`.
pub fn encrypt_for(recipient_viewing_pubkey: &[u8; 32], opening: &NoteOpening) -> Memo {
    let esk = StaticSecret::random_from_rng(OsRng);
    let epk = PublicKey::from(&esk).to_bytes();
    let shared = esk.diffie_hellman(&PublicKey::from(*recipient_viewing_pubkey)).to_bytes();
    let sym_key = kdf(&shared);

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&sym_key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), opening.to_bytes().as_slice())
        .expect("chacha20poly1305 encrypt");

    Memo { ephemeral_pubkey: epk, nonce: nonce_bytes, ciphertext }
}

/// Recipient side (B.13.5 steps 3-4): trial-decrypt a memo with `view_key`.
/// `None` means "not ours" (or corrupt) — the caller just skips it; scanning
/// history is a sequence of these cheap trial decryptions.
pub fn try_decrypt(view_key: &ViewKey, memo: &Memo) -> Option<NoteOpening> {
    let shared = view_key.shared_secret(&memo.ephemeral_pubkey);
    let sym_key = kdf(&shared);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&sym_key));
    let plaintext = cipher.decrypt(Nonce::from_slice(&memo.nonce), memo.ciphertext.as_slice()).ok()?;
    let bytes: [u8; 96] = plaintext.as_slice().try_into().ok()?;
    Some(NoteOpening::from_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{be32, poseidon_be};

    fn opening(seed: u128) -> NoteOpening {
        NoteOpening { token_id: be32(seed), amount: be32(seed + 1), blinding_factor: be32(seed + 2) }
    }

    #[test]
    fn round_trip() {
        let recipient = ViewKey::generate();
        let opening_in = opening(100);

        let memo = encrypt_for(&recipient.viewing_pubkey(), &opening_in);
        let opening_out = try_decrypt(&recipient, &memo).expect("decrypts for the intended recipient");

        assert_eq!(opening_out.token_id, opening_in.token_id);
        assert_eq!(opening_out.amount, opening_in.amount);
        assert_eq!(opening_out.blinding_factor, opening_in.blinding_factor);
    }

    #[test]
    fn wrong_view_key_fails() {
        let recipient = ViewKey::generate();
        let eavesdropper = ViewKey::generate();
        let memo = encrypt_for(&recipient.viewing_pubkey(), &opening(200));

        assert!(try_decrypt(&eavesdropper, &memo).is_none(), "a different view_key must not decrypt");
    }

    #[test]
    fn independent_keys_never_collide() {
        // Two independently generated view keys produce different public
        // keys with overwhelming probability — sanity check there's no
        // accidental determinism (e.g. a fixed/zeroed RNG) in generate().
        let a = ViewKey::generate();
        let b = ViewKey::generate();
        assert_ne!(a.viewing_pubkey(), b.viewing_pubkey());
    }

    #[test]
    fn rotation_does_not_touch_owner_pubkey() {
        // owner_pubkey is derived from spend_key alone (B.2) and has nothing
        // to do with view_key — rotating the latter must not perturb it.
        let spend_key = be32(987_654_321);
        let owner_pubkey_before = poseidon_be(&[spend_key]);

        let _old_view_key = ViewKey::generate();
        let _new_view_key = ViewKey::generate(); // simulates a rotation

        let owner_pubkey_after = poseidon_be(&[spend_key]);
        assert_eq!(owner_pubkey_before, owner_pubkey_after);
    }

    #[test]
    fn memo_wire_round_trip() {
        let recipient = ViewKey::generate();
        let memo = encrypt_for(&recipient.viewing_pubkey(), &opening(300));
        let bytes = memo.to_bytes();
        assert_eq!(bytes.len(), Memo::LEN);

        let parsed = Memo::from_bytes(&bytes);
        let opening_out = try_decrypt(&recipient, &parsed).expect("decrypts after wire round-trip");
        assert_eq!(opening_out.amount, opening(300).amount);
    }

    #[test]
    fn view_key_persistence_round_trip() {
        // Storing/reloading the secret (e.g. from an identity file) must
        // reconstruct a key that decrypts exactly the same as the original.
        let original = ViewKey::generate();
        let reloaded = ViewKey::from_bytes(original.to_bytes());
        assert_eq!(original.viewing_pubkey(), reloaded.viewing_pubkey());

        let memo = encrypt_for(&original.viewing_pubkey(), &opening(400));
        assert!(try_decrypt(&reloaded, &memo).is_some());
    }
}
