use std::fs::{OpenOptions, read_to_string};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone)]
pub struct HistoryManager {
    path: PathBuf,
}

impl HistoryManager {
    pub fn new() -> Self {
        let mut path = PathBuf::from("/home/popos/.cache/rust-browser-poc");
        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        path.push("history.txt");
        Self { path }
    }

    pub fn add(&self, url: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
        {
            // Check if URL already in history (simple check)
            if let Ok(content) = read_to_string(&self.path) {
                if content.lines().any(|l| l == url) {
                    return;
                }
            }
            let _ = writeln!(file, "{}", url);
        }
    }

    pub fn load(&self) -> Vec<String> {
        read_to_string(&self.path)
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }
}
