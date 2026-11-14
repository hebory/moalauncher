fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=windows.manifest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("icons/icon.ico")
        .set_manifest_file("windows.manifest")
        .set("ProductName", "Moa Launcher")
        .set("FileDescription", "모아 프로그램 설치 및 업데이트 런처")
        .set("OriginalFilename", "MoaLauncher.exe");
    resource.compile()?;
    Ok(())
}
