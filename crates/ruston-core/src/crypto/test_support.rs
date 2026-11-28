//! Keys the tests make for themselves.

use proton_crypto::crypto::{
    DataEncoding, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, PGPProviderSync,
};

/// A key made for a test, as Proton hands keys out: armored, the private
/// half locked with its passphrase.
pub(crate) struct TestKey {
    pub(crate) private: String,
    pub(crate) public: String,
}

/// A new ECC key for `email`, its private half locked with `passphrase`.
pub(crate) fn test_key(email: &str, passphrase: &str) -> TestKey {
    let provider = super::provider();
    let key = provider
        .new_key_generator()
        .with_user_id("Test", email)
        .with_algorithm(KeyGeneratorAlgorithm::ECC)
        .generate()
        .unwrap();
    let private = provider
        .private_key_export(&key, passphrase, DataEncoding::Armor)
        .unwrap();
    let public = provider.private_key_to_public_key(&key).unwrap();
    let public = provider
        .public_key_export(&public, DataEncoding::Armor)
        .unwrap();
    TestKey {
        private: String::from_utf8(private.as_ref().to_vec()).unwrap(),
        public: String::from_utf8(public.as_ref().to_vec()).unwrap(),
    }
}
