fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // An inline manifest would be padded line by line; embed the file
        // byte for byte instead.
        let manifest_path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap())
            .join("ruston-cli.exe.manifest");
        std::fs::write(&manifest_path, manifest()).expect("failed to write the Windows manifest");
        // winresource reads both string and numeric versions from Cargo.
        winresource::WindowsResource::new()
            .set("ProductName", "Ruston Mail")
            .set("FileDescription", "Ruston Mail CLI")
            .set("Comments", env!("CARGO_PKG_DESCRIPTION"))
            .set("InternalName", "ruston-cli")
            .set("OriginalFilename", "ruston-cli.exe")
            .set("LegalCopyright", "Copyright (c) 2026 Luis Cuellar")
            .set_manifest_file(manifest_path.to_str().expect("non-UTF-8 build directory"))
            .compile()
            .expect("failed to embed Windows version information");
    }
}

/// Declares Windows 10 and 11, long paths, and UTF-8 as the ANSI code page.
/// The manifest is inline so the published crate needs no files outside it.
fn manifest() -> String {
    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <assemblyIdentity type="win32" name="com.luiscuellar.ruston-cli" version="@VERSION@" processorArchitecture="*"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">
    <security>
      <requestedPrivileges xmlns="urn:schemas-microsoft-com:asm.v3">
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <!-- Windows 10 and 11 -->
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <asmv3:application>
    <asmv3:windowsSettings>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;
    MANIFEST.replace(
        "@VERSION@",
        &format!(
            "{}.{}.{}.0",
            env!("CARGO_PKG_VERSION_MAJOR"),
            env!("CARGO_PKG_VERSION_MINOR"),
            env!("CARGO_PKG_VERSION_PATCH")
        ),
    )
}
