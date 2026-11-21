//! Unit tests for `net::crypto`.

use super::*;

fn random_secret() -> [u8; SHARED_SECRET_BYTES] {
    let mut secret = [0u8; SHARED_SECRET_BYTES];
    random_bytes(&mut secret).expect("random");
    secret
}

#[test]
fn server_hash_matches_the_wiki_vectors() {
    // minecraft.wiki `Java Edition protocol/Encryption`, Authentication > Client.
    assert_eq!(
        server_hash(&[b"Notch"]),
        "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
    );
    assert_eq!(
        server_hash(&[b"jeb_"]),
        "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
    );
    assert_eq!(
        server_hash(&[b"simon"]),
        "88e16a1019277b15d58faf0541e11910eb756f6"
    );
}

#[test]
fn server_hash_concatenates_its_parts() {
    assert_eq!(server_hash(&[b"No", b"tch"]), server_hash(&[b"Notch"]));
}

#[test]
fn cfb8_matches_the_crate_known_answer_and_streams_across_calls() {
    // Known-answer vector from the `cfb8` crate documentation.
    let key = [0x42; 16];
    let plaintext = *b"hello world! this is my plaintext.";
    let expected = [
        0x33, 0xb3, 0x56, 0xce, 0x91, 0x84, 0x29, 0x0c, 0x4c, 0x8f, 0xac, 0xc1, 0xc0, 0xb1, 0xf9,
        0x18, 0xd5, 0x47, 0x5a, 0xeb, 0x75, 0xb8, 0x8c, 0x16, 0x1c, 0xa6, 0x5b, 0xdf, 0x05, 0xc7,
        0x13, 0x7f, 0xf4, 0xb0,
    ];
    let mut cipher = cfb8::Encryptor::<Aes128>::new(&key.into(), &[0x24; 16].into());
    let mut whole = plaintext;
    cipher.encrypt(&mut whole);
    assert_eq!(whole, expected);

    let secret = random_secret();
    let mut one_shot = plaintext.to_vec();
    StreamEncryptor::new(&secret).apply(&mut one_shot);
    let mut split = plaintext.to_vec();
    let mut encryptor = StreamEncryptor::new(&secret);
    let (head, tail) = split.split_at_mut(5);
    encryptor.apply(head);
    encryptor.apply(tail);
    assert_eq!(split, one_shot);
    let mut decryptor = StreamDecryptor::new(&secret);
    decryptor.apply(&mut split);
    assert_eq!(split, plaintext);
}

#[test]
fn server_key_round_trips_a_shared_secret() {
    let key = ServerKey::generate().expect("key");
    assert!(key.public_der().len() > 100);
    let secret = random_secret();
    let encrypted = key.encrypt_for_test(&secret).expect("encrypt");
    assert_eq!(encrypted.len(), SERVER_KEY_BITS / 8);
    assert_eq!(key.decrypt_secret(&encrypted), Ok(secret));
    let short = key.encrypt_for_test(&[1, 2, 3]).expect("encrypt");
    assert_eq!(
        key.decrypt_secret(&short),
        Err(CryptoError::SecretLength(3))
    );
    assert_eq!(key.decrypt(&[0u8; 5]), Err(CryptoError::Decrypt));
}
