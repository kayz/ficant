use ficant_domain::primitives::ContentHash;

pub(crate) fn content_key(hash: &ContentHash) -> String {
    format!("immutable/{}", hash_hex(hash))
}

pub(crate) fn hash_hex(hash: &ContentHash) -> String {
    let mut value = String::with_capacity(64);
    for byte in hash.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}
