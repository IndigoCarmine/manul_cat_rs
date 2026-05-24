fn main() {
    // Windows向けにアイコンを設定
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("resources/Manuru.ico");
        res.compile().unwrap();
    }
}
