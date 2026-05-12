use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Entrada do histórico — URL + timestamp Unix (segundos).
#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub url: String,
    pub ts_secs: u64,
}

/// HistoryManager com cache em memória (O(1) dedup) e persistência TSV.
///
/// Formato em disco: `<unix_secs>\t<url>\n` (uma entrada por linha).
/// Mantém compat. com histórico antigo (linhas sem TAB são tratadas como URL
/// só, com timestamp = 0).
#[derive(Clone)]
pub struct HistoryManager {
    inner: Rc<HistoryInner>,
}

struct HistoryInner {
    path: PathBuf,
    seen: RefCell<HashSet<String>>,
    entries: RefCell<Vec<HistoryEntry>>,
}

impl HistoryManager {
    pub fn new() -> Self {
        // Path portável: $XDG_CACHE_HOME/rust-browser-poc/history.tsv
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join("rust-browser-poc");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("history.tsv");

        let mut seen = HashSet::new();
        let mut entries: Vec<HistoryEntry> = Vec::new();

        // Carrega histórico novo (TSV) se existir.
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                if line.is_empty() { continue; }
                let (ts, url) = if let Some((a, b)) = line.split_once('\t') {
                    (a.parse::<u64>().unwrap_or(0), b.to_string())
                } else {
                    (0u64, line.to_string())
                };
                if seen.insert(url.clone()) {
                    entries.push(HistoryEntry { url, ts_secs: ts });
                }
            }
        } else {
            // Migração: tenta importar `history.txt` antigo (só URL/linha).
            let legacy = dir.join("history.txt");
            if let Ok(content) = std::fs::read_to_string(&legacy) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    if seen.insert(line.to_string()) {
                        entries.push(HistoryEntry { url: line.to_string(), ts_secs: 0 });
                    }
                }
                // Reescreve em formato novo, preservando histórico antigo.
                if let Ok(mut f) = std::fs::File::create(&path) {
                    for e in &entries {
                        let _ = writeln!(f, "{}\t{}", e.ts_secs, e.url);
                    }
                }
            }
        }

        Self {
            inner: Rc::new(HistoryInner {
                path,
                seen: RefCell::new(seen),
                entries: RefCell::new(entries),
            }),
        }
    }

    /// Adiciona uma URL com timestamp atual. Idempotente (dedup por URL).
    pub fn add(&self, url: &str) {
        if url.is_empty() { return; }
        let mut seen = self.inner.seen.borrow_mut();
        if !seen.insert(url.to_string()) {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.inner.entries.borrow_mut().push(HistoryEntry {
            url: url.to_string(),
            ts_secs: ts,
        });
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.inner.path)
        {
            let _ = writeln!(file, "{}\t{}", ts, url);
        }
    }

    /// Compat: lista de URLs (mais recentes primeiro), para autocomplete.
    pub fn load(&self) -> Vec<String> {
        let entries = self.inner.entries.borrow();
        entries.iter().rev().map(|e| e.url.clone()).collect()
    }

    /// Lista completa com timestamps (mais recentes primeiro).
    pub fn load_entries(&self) -> Vec<HistoryEntry> {
        let mut v = self.inner.entries.borrow().clone();
        v.reverse();
        v
    }
}
