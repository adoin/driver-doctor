fn main() {
    println!("cargo:rerun-if-changed=src/assets/app_icon.ico");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("src/assets/app_icon.ico");
        resource.set("FileDescription", "Driver Doctor");
        resource.set("ProductName", "Driver Doctor");
        resource
            .compile()
            .expect("failed to compile Windows executable resources");
    }
}
