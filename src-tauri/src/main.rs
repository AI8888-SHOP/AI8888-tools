fn main() {
  #[cfg(target_os = "linux")]
  {
    if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
      std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
  }
  ai8888_switch_lib::run();
}
