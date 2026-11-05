pub mod browser;
pub mod cdp;
pub mod host;
pub mod vault;

pub mod notify {
    /// Fire-and-forget: never blocks the program on a notification.
    pub fn send(title: &str, body: &str) {
        let script = format!(
            "display notification {} with title {} sound name \"Glass\"",
            applescript_str(body),
            applescript_str(title)
        );
        let _ = std::process::Command::new("osascript").args(["-e", &script]).spawn();
        if let Ok(topic) = std::env::var("P0_NTFY_TOPIC") {
            let _ = std::process::Command::new("curl")
                .args(["-s", "-H", &format!("Title: {title}"), "-d", body, &format!("https://ntfy.sh/{topic}")])
                .stdout(std::process::Stdio::null())
                .spawn();
        }
    }
    fn applescript_str(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
