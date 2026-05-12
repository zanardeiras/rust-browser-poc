//! Configurações mínimas persistidas em TSV plain (zero deps).
//!
//! Formato: `<chave>\t<valor>` por linha. Apenas booleanos por enquanto.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

pub struct Settings {
    path: PathBuf,
    map: RefCell<HashMap<String, String>>,
}

impl Settings {
    pub fn new() -> Rc<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join("rust-browser-poc");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.tsv");

        let mut map = HashMap::new();
        if let Ok(s) = std::fs::read_to_string(&path) {
            for line in s.lines() {
                if let Some((k, v)) = line.split_once('\t') {
                    map.insert(k.to_string(), v.to_string());
                }
            }
        }
        Rc::new(Self { path, map: RefCell::new(map) })
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.map.borrow().get(key)
            .map(|v| v == "1" || v == "true")
            .unwrap_or(default)
    }

    pub fn set_bool(&self, key: &str, value: bool) {
        self.map.borrow_mut().insert(key.to_string(), if value { "1" } else { "0" }.to_string());
        self.save();
    }

    fn save(&self) {
        let tmp = self.path.with_extension("tsv.tmp");
        if let Ok(mut f) = OpenOptions::new()
            .write(true).create(true).truncate(true).open(&tmp)
        {
            for (k, v) in self.map.borrow().iter() {
                let _ = writeln!(f, "{}\t{}", k, v);
            }
        }
        let _ = std::fs::rename(&tmp, &self.path);
    }
}
