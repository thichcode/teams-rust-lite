fn main() {
    slint_build::compile("ui/main.slint").expect("Failed to compile Slint UI");

    let version = std::env::var("APP_BUILD_VERSION").unwrap_or_else(|_| {
        let pkg_version = env!("CARGO_PKG_VERSION");
        format!("v{}", pkg_version)
    });
    println!("cargo:rustc-env=APP_VERSION={}", version);
    println!("cargo:rerun-if-env-changed=APP_BUILD_VERSION");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        let _ = res.compile();
    }
}
