fn main() {
    println!("cargo:rerun-if-env-changed=FILEMANAGER_SIGNER_THUMBPRINT");
    if let Ok(thumbprint) = std::env::var("FILEMANAGER_SIGNER_THUMBPRINT") {
        let thumbprint = thumbprint.trim().to_ascii_uppercase();
        if thumbprint.len() != 40 || !thumbprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            panic!("FILEMANAGER_SIGNER_THUMBPRINT must be a 40-character SHA-1 thumbprint");
        }
        println!("cargo:rustc-env=FILEMANAGER_SIGNER_THUMBPRINT={thumbprint}");
    }

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile required Windows resources from app.rc");
    }
}
