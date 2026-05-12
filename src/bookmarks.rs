//! Modelo de favoritos com persistência em TSV (zero deps externas).
//!
//! ## Modelo
//!
//! Árvore flat — cada item tem `id` e `parent` (0 = raiz / barra). A ordem das
//! linhas no arquivo é a ordem de exibição. Cada item é **Folder** ou **Link**.
//!
//! ## Formato em disco — `~/.cache/rust-browser-poc/bookmarks.tsv`
//!
//! ```text
//! <id>\t<parent>\t<kind>\t<title>\t<url>
//! ```
//!
//! - `kind`: `F` (folder) ou `L` (link)
//! - `url` é vazio para folders
//! - Campos podem conter espaço, mas TAB e LF não (escapados para `\\t` / `\\n`).
//!
//! ## API
//!
//! Tudo via `Rc<BookmarksStore>` interior-mutable. Listeners são notificados a
//! cada mudança (`on_change`) — usado pelo bookmarks_bar para se redesenhar.

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum BookmarkKind {
    Folder,
    Link { url: String },
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub id: u64,
    pub parent: u64,
    pub title: String,
    pub kind: BookmarkKind,
}

pub struct BookmarksStore {
    inner: RefCell<Inner>,
    /// Listeners notificados após qualquer mutação persistida.
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
}

struct Inner {
    path: PathBuf,
    items: Vec<Bookmark>,
    next_id: u64,
}

impl BookmarksStore {
    pub fn new() -> Rc<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join("rust-browser-poc");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bookmarks.tsv");

        let (items, next_id) = load_from_disk(&path);

        Rc::new(Self {
            inner: RefCell::new(Inner { path, items, next_id }),
            listeners: RefCell::new(Vec::new()),
        })
    }

    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        // Snapshot evita re-entrância se um listener disparar outra mutação.
        let snapshot: Vec<Rc<dyn Fn()>> = self.listeners.borrow().clone();
        for f in snapshot { f(); }
    }

    pub fn list_children(&self, parent: u64) -> Vec<Bookmark> {
        self.inner
            .borrow()
            .items
            .iter()
            .filter(|b| b.parent == parent)
            .cloned()
            .collect()
    }

    /// Lista todos os itens em ordem original (para o manager view).
    pub fn all(&self) -> Vec<Bookmark> {
        self.inner.borrow().items.clone()
    }

    pub fn get(&self, id: u64) -> Option<Bookmark> {
        self.inner.borrow().items.iter().find(|b| b.id == id).cloned()
    }

    pub fn add_link(&self, parent: u64, title: &str, url: &str) -> u64 {
        let id = {
            let mut inner = self.inner.borrow_mut();
            let id = inner.next_id; inner.next_id += 1;
            inner.items.push(Bookmark {
                id, parent,
                title: title.to_string(),
                kind: BookmarkKind::Link { url: url.to_string() },
            });
            id
        };
        self.save(); self.notify(); id
    }

    pub fn add_folder(&self, parent: u64, title: &str) -> u64 {
        let id = {
            let mut inner = self.inner.borrow_mut();
            let id = inner.next_id; inner.next_id += 1;
            inner.items.push(Bookmark {
                id, parent,
                title: title.to_string(),
                kind: BookmarkKind::Folder,
            });
            id
        };
        self.save(); self.notify(); id
    }

    pub fn rename(&self, id: u64, title: &str) {
        {
            let mut inner = self.inner.borrow_mut();
            if let Some(b) = inner.items.iter_mut().find(|b| b.id == id) {
                b.title = title.to_string();
            }
        }
        self.save(); self.notify();
    }

    pub fn set_url(&self, id: u64, url: &str) {
        {
            let mut inner = self.inner.borrow_mut();
            if let Some(b) = inner.items.iter_mut().find(|b| b.id == id) {
                if let BookmarkKind::Link { url: ref mut u } = b.kind {
                    *u = url.to_string();
                }
            }
        }
        self.save(); self.notify();
    }

    /// Remove um item. Se for folder, remove recursivamente os descendentes.
    pub fn remove(&self, id: u64) {
        {
            let mut inner = self.inner.borrow_mut();
            // BFS para coletar todos os ids descendentes.
            let mut to_remove: Vec<u64> = vec![id];
            let mut i = 0;
            while i < to_remove.len() {
                let parent = to_remove[i];
                for b in &inner.items {
                    if b.parent == parent { to_remove.push(b.id); }
                }
                i += 1;
            }
            inner.items.retain(|b| !to_remove.contains(&b.id));
        }
        self.save(); self.notify();
    }

    /// Move para outro pai (no fim da lista do destino). Bloqueia ciclos.
    pub fn move_to(&self, id: u64, new_parent: u64) {
        if id == new_parent { return; }
        // Detecta ciclo: new_parent é descendente de id?
        if self.is_descendant_of(new_parent, id) { return; }
        {
            let mut inner = self.inner.borrow_mut();
            // Remove o item e re-insere no fim mantendo todos os outros.
            if let Some(pos) = inner.items.iter().position(|b| b.id == id) {
                let mut item = inner.items.remove(pos);
                item.parent = new_parent;
                inner.items.push(item);
            }
        }
        self.save(); self.notify();
    }

    fn is_descendant_of(&self, candidate: u64, ancestor: u64) -> bool {
        let inner = self.inner.borrow();
        let mut cur = candidate;
        while cur != 0 {
            let p = match inner.items.iter().find(|b| b.id == cur) {
                Some(b) => b.parent,
                None => return false,
            };
            if p == ancestor { return true; }
            cur = p;
        }
        false
    }

    /// Reordena entre irmãos: -1 sobe, +1 desce.
    pub fn shift(&self, id: u64, delta: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            let parent = match inner.items.iter().find(|b| b.id == id) {
                Some(b) => b.parent,
                None => return,
            };
            // Coleta posições dos irmãos.
            let sibling_positions: Vec<usize> = inner.items
                .iter().enumerate()
                .filter(|(_, b)| b.parent == parent)
                .map(|(i, _)| i)
                .collect();
            let cur_pos = match sibling_positions.iter().position(|&p| inner.items[p].id == id) {
                Some(p) => p,
                None => return,
            };
            let target_pos = (cur_pos as i32 + delta).clamp(0, sibling_positions.len() as i32 - 1) as usize;
            if target_pos == cur_pos { return; }
            // Faz swap apenas trocando posições absolutas dos dois itens irmãos.
            let a = sibling_positions[cur_pos];
            let b = sibling_positions[target_pos];
            inner.items.swap(a, b);
            // sibling_positions é descartado após o swap; só recomputaremos no save.
            let _ = sibling_positions;
        }
        self.save(); self.notify();
    }

    pub fn find_by_url(&self, url: &str) -> Option<u64> {
        self.inner.borrow().items.iter().find(|b| {
            matches!(&b.kind, BookmarkKind::Link { url: u } if u == url)
        }).map(|b| b.id)
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.find_by_url(url).is_some()
    }

    /// Toggle: se não existir, adiciona na raiz com `title`; se existir, remove.
    /// Retorna o novo estado (true = bookmarked).
    pub fn toggle_url(&self, url: &str, title: &str) -> bool {
        if let Some(id) = self.find_by_url(url) {
            self.remove(id);
            false
        } else {
            self.add_link(0, title, url);
            true
        }
    }

    fn save(&self) {
        let inner = self.inner.borrow();
        let tmp = inner.path.with_extension("tsv.tmp");
        if let Ok(mut f) = OpenOptions::new()
            .write(true).create(true).truncate(true).open(&tmp)
        {
            for b in &inner.items {
                let (kind, url) = match &b.kind {
                    BookmarkKind::Folder => ('F', String::new()),
                    BookmarkKind::Link { url } => ('L', url.clone()),
                };
                let _ = writeln!(
                    f, "{}\t{}\t{}\t{}\t{}",
                    b.id, b.parent, kind, escape(&b.title), escape(&url),
                );
            }
        }
        let _ = std::fs::rename(&tmp, &inner.path);
    }
}

fn load_from_disk(path: &PathBuf) -> (Vec<Bookmark>, u64) {
    let mut items = Vec::new();
    let mut next_id = 1u64;
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            if line.is_empty() { continue; }
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() < 5 { continue; }
            let id: u64 = parts[0].parse().unwrap_or(0);
            let parent: u64 = parts[1].parse().unwrap_or(0);
            let kind = match parts[2] {
                "F" => BookmarkKind::Folder,
                "L" => BookmarkKind::Link { url: unescape(parts[4]) },
                _ => continue,
            };
            let title = unescape(parts[3]);
            if id >= next_id { next_id = id + 1; }
            items.push(Bookmark { id, parent, title, kind });
        }
    }
    (items, next_id.max(1))
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(c) => out.push(c),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
