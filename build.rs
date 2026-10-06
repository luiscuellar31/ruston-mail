fn main() {
    println!("cargo:rerun-if-changed=assets/windows/ruston-mail.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // winresource reads both string and numeric versions from Cargo.
        winresource::WindowsResource::new()
            .set_icon("assets/windows/ruston-mail.ico")
            .set("ProductName", "Ruston Mail")
            .set("FileDescription", "Ruston Mail")
            .set("Comments", env!("CARGO_PKG_DESCRIPTION"))
            .set("InternalName", "ruston")
            .set("OriginalFilename", "ruston.exe")
            .set("LegalCopyright", "Copyright (c) 2026 Luis Cuellar")
            .compile()
            .expect("failed to embed Windows desktop icon and version information");
    }
}
