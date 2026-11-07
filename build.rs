fn main() {
    println!("cargo:rerun-if-env-changed=FILEMANAGER_SIGNER_SHA256");
    if let Ok(fingerprint) = std::env::var("FILEMANAGER_SIGNER_SHA256") {
        let fingerprint = fingerprint.trim().to_ascii_uppercase();
        if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            panic!("FILEMANAGER_SIGNER_SHA256 must be a 64-character SHA-256 fingerprint");
        }
        println!("cargo:rustc-env=FILEMANAGER_SIGNER_SHA256={fingerprint}");
    }

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile required Windows resources from app.rc");
    }
}
