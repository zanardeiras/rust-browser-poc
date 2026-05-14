//! Gerenciador de senhas criptografado com AES-256-GCM + Argon2id.
//!
//! ## Segurança
//!
//! - A **senha-mestre** NUNCA é armazenada em disco (nem seu hash).
//! - A chave AES-256 é derivada da senha-mestre via Argon2id com salt aleatório.
//! - Cada salvo gera um novo nonce aleatório (AES-GCM requer nonces únicos).
//! - O arquivo em disco contém: [16B salt][12B nonce][N bytes ciphertext].
//! - As entradas decifradas ficam apenas em memória e são zerizadas ao fechar.
//!
//! ## Formato do arquivo `~/.cache/rust-browser-poc/data/passwords.enc`
//!
//! ```text
//! bytes  0..16  → Argon2id salt (16 bytes)
//! bytes 16..28  → AES-GCM nonce (12 bytes)
//! bytes 28..    → AES-256-GCM ciphertext do JSON serializado
//! ```
//!
//! O JSON plaintext é um array de `PasswordEntry` (serde).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::cell::RefCell;

use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit};
use argon2::Argon2;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ─── Estrutura de uma entrada ────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasswordEntry {
    pub domain:   String,
    pub username: String,
    pub password: String,
}

impl Drop for PasswordEntry {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

// ─── Store ───────────────────────────────────────────────────────────────────

pub struct PasswordStore {
    path:    PathBuf,
    entries: Vec<PasswordEntry>, // vazio quando bloqueado
    locked:  bool,
}

impl PasswordStore {
    pub fn new(data_dir: &Path) -> Rc<RefCell<Self>> {
        let path = data_dir.join("passwords.enc");
        Rc::new(RefCell::new(Self {
            path,
            entries: Vec::new(),
            locked: true,
        }))
    }

    pub fn file_exists(&self) -> bool {
        self.path.exists()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Desbloqueia o cofre. Se o arquivo não existir, cria um cofre vazio.
    /// Retorna Err com mensagem legível se a senha estiver errada.
    pub fn unlock(&mut self, master_password: &str) -> Result<(), String> {
        if self.path.exists() {
            self.entries = Self::decrypt_file(&self.path, master_password)?;
        } else {
            // Primeiro uso: cria arquivo vazio imediatamente.
            self.entries = Vec::new();
            Self::encrypt_to_file(&self.path, &self.entries, master_password)?;
        }
        self.locked = false;
        Ok(())
    }

    /// Bloqueia e zeriza as entradas da memória.
    pub fn lock(&mut self) {
        for e in &mut self.entries {
            e.password.zeroize();
        }
        self.entries.clear();
        self.locked = true;
    }

    /// Retorna todas as entradas (apenas quando desbloqueado).
    pub fn entries(&self) -> &[PasswordEntry] {
        &self.entries
    }

    /// Entradas que casam com o domínio da URL atual (substring).
    pub fn entries_for_domain(&self, url: &str) -> Vec<(usize, &PasswordEntry)> {
        let domain = extract_domain(url);
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let d = extract_domain(&e.domain);
                d == domain || e.domain.contains(&domain) || domain.contains(&d)
            })
            .collect()
    }

    /// Adiciona uma entrada e salva no disco.
    pub fn add(&mut self, entry: PasswordEntry, master_password: &str) -> Result<(), String> {
        self.entries.push(entry);
        Self::encrypt_to_file(&self.path, &self.entries, master_password)?;
        Ok(())
    }

    /// Remove a entrada pelo índice e salva.
    pub fn remove(&mut self, index: usize, master_password: &str) -> Result<(), String> {
        if index < self.entries.len() {
            self.entries.remove(index);
            Self::encrypt_to_file(&self.path, &self.entries, master_password)?;
        }
        Ok(())
    }

    /// Atualiza uma entrada existente e salva.
    pub fn update(&mut self, index: usize, entry: PasswordEntry, master_password: &str) -> Result<(), String> {
        if index < self.entries.len() {
            self.entries[index] = entry;
            Self::encrypt_to_file(&self.path, &self.entries, master_password)?;
        }
        Ok(())
    }

    // ── Crypto privado ────────────────────────────────────────────────────────

    fn derive_key(master_password: &str, salt_bytes: &[u8]) -> Result<[u8; 32], String> {
        let mut key = [0u8; 32];
        Argon2::default()
            .hash_password_into(master_password.as_bytes(), salt_bytes, &mut key)
            .map_err(|e| format!("Argon2 falhou: {e}"))?;
        Ok(key)
    }

    fn encrypt_to_file(path: &Path, entries: &[PasswordEntry], master_password: &str) -> Result<(), String> {
        // Gera salt e nonce aleatórios a cada salvamento.
        let mut salt = [0u8; 16];
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let mut key_bytes = Self::derive_key(master_password, &salt)?;
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = serde_json::to_vec(entries)
            .map_err(|e| format!("Serialização falhou: {e}"))?;

        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref())
            .map_err(|_| "Criptografia falhou".to_string())?;

        key_bytes.zeroize();

        // Garante que o diretório existe.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Escrita atômica via arquivo temporário.
        let tmp = path.with_extension("enc.tmp");
        let mut data = Vec::with_capacity(16 + 12 + ciphertext.len());
        data.extend_from_slice(&salt);
        data.extend_from_slice(&nonce_bytes);
        data.extend_from_slice(&ciphertext);

        std::fs::write(&tmp, &data)
            .map_err(|e| format!("Erro ao escrever: {e}"))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("Erro ao renomear: {e}"))?;

        Ok(())
    }

    fn decrypt_file(path: &Path, master_password: &str) -> Result<Vec<PasswordEntry>, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("Erro ao ler arquivo: {e}"))?;

        if data.len() < 28 {
            return Err("Arquivo corrompido (muito pequeno)".to_string());
        }

        let salt = &data[0..16];
        let nonce_bytes = &data[16..28];
        let ciphertext = &data[28..];

        let mut key_bytes = Self::derive_key(master_password, salt)?;
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|_| "Senha mestre incorreta ou arquivo corrompido".to_string())?;

        key_bytes.zeroize();

        let entries: Vec<PasswordEntry> = serde_json::from_slice(&plaintext)
            .map_err(|e| format!("Formato inválido: {e}"))?;

        Ok(entries)
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

/// Extrai o domínio raiz de uma URL ou string de domínio.
fn extract_domain(input: &str) -> String {
    let s = input
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    // Pega só até a primeira "/" (ignora path).
    let domain = s.split('/').next().unwrap_or(s);
    domain.to_ascii_lowercase()
}

// ─── UI GTK ──────────────────────────────────────────────────────────────────

pub fn open_manager(
    store: Rc<RefCell<PasswordStore>>,
    current_url: Option<String>,
    fill_callback: Option<Rc<dyn Fn(String, String)>>,
    parent: Option<&gtk::Window>,
) {
    use gtk::prelude::*;

    // ── Dialog de senha-mestre ────────────────────────────────────────────────
    let needs_creation = !store.borrow().file_exists();
    let is_locked = store.borrow().is_locked();

    if is_locked {
        let pw_dialog = gtk::Dialog::new();
        pw_dialog.set_title(if needs_creation {
            "Criar senha mestre"
        } else {
            "Desbloquear cofre de senhas"
        });
        pw_dialog.set_modal(true);
        pw_dialog.set_default_width(360);
        if let Some(p) = parent { pw_dialog.set_transient_for(Some(p)); }

        let content = pw_dialog.content_area();
        content.set_spacing(8);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(16);
        content.set_margin_bottom(8);

        let icon = gtk::Image::from_icon_name(Some("system-lock-screen-symbolic"), gtk::IconSize::Dialog);
        content.pack_start(&icon, false, false, 0);

        let lbl = gtk::Label::new(Some(if needs_creation {
            "Crie uma senha mestre para proteger suas senhas.\nEla não será armazenada — não esqueça!"
        } else {
            "Digite a senha mestre para acessar o cofre."
        }));
        lbl.set_line_wrap(true);
        lbl.set_xalign(0.0);
        content.pack_start(&lbl, false, false, 0);

        let pw_entry = gtk::Entry::new();
        pw_entry.set_visibility(false);
        pw_entry.set_placeholder_text(Some("Senha mestre…"));
        pw_entry.set_activates_default(true);
        content.pack_start(&pw_entry, false, false, 0);

        let pw_entry2_opt: Option<gtk::Entry> = if needs_creation {
            let e = gtk::Entry::new();
            e.set_visibility(false);
            e.set_placeholder_text(Some("Confirme a senha mestre…"));
            e.set_activates_default(true);
            content.pack_start(&e, false, false, 0);
            Some(e)
        } else {
            None
        };

        let err_label = gtk::Label::new(None);
        err_label.set_xalign(0.0);
        {
            let ctx = err_label.style_context();
            ctx.add_class("error");
        }
        content.pack_start(&err_label, false, false, 0);

        pw_dialog.add_button("Cancelar", gtk::ResponseType::Cancel);
        let ok_btn = pw_dialog.add_button(
            if needs_creation { "Criar" } else { "Desbloquear" },
            gtk::ResponseType::Ok,
        );
        ok_btn.style_context().add_class("suggested-action");
        pw_dialog.set_default_response(gtk::ResponseType::Ok);

        pw_dialog.show_all();

        let store_clone = store.clone();
        let fill_cb_clone = fill_callback.clone();
        let current_url_clone = current_url.clone();
        let parent_win = parent.map(|w| w.clone());

        pw_dialog.connect_response(move |dlg, resp| {
            if resp != gtk::ResponseType::Ok {
                dlg.close();
                return;
            }
            let pw = dlg.content_area()
                .children()
                .iter()
                .find_map(|w| w.downcast_ref::<gtk::Entry>().map(|e| e.text().to_string()))
                .unwrap_or_default();

            if needs_creation {
                if let Some(e2) = &pw_entry2_opt {
                    let pw2 = e2.text().to_string();
                    if pw != pw2 {
                        err_label.set_text("As senhas não conferem.");
                        return;
                    }
                }
                if pw.len() < 8 {
                    err_label.set_text("Senha muito curta (mínimo 8 caracteres).");
                    return;
                }
            }

            // Solta o RefMut ANTES de chamar build_manager_window.
            // Se o match segurasse o borrow_mut() vivo durante o arm Ok(_),
            // o populate_store() dentro de build_manager_window chamaria
            // borrow() enquanto o RefMut ainda existe → pânico de double-borrow.
            let unlock_result = { store_clone.borrow_mut().unlock(&pw) };
            match unlock_result {
                Ok(_) => {
                    dlg.close();
                    build_manager_window(
                        store_clone.clone(),
                        pw.clone(),
                        current_url_clone.clone(),
                        fill_cb_clone.clone(),
                        parent_win.as_ref(),
                    );
                }
                Err(e) => {
                    err_label.set_text(&format!("Erro: {e}"));
                }
            }
        });
    } else {
        build_manager_window(store, String::new(), current_url, fill_callback, parent);
    }
}

// ─── Janela principal do gerenciador ─────────────────────────────────────────

fn build_manager_window(
    store: Rc<RefCell<PasswordStore>>,
    master_pw: String,
    current_url: Option<String>,
    fill_callback: Option<Rc<dyn Fn(String, String)>>,
    parent: Option<&gtk::Window>,
) {
    use gtk::prelude::*;

    let window = gtk::Window::builder()
        .title("Gerenciador de Senhas")
        .default_width(700)
        .default_height(480)
        .modal(true)
        .build();
    if let Some(p) = parent {
        window.set_transient_for(Some(p));
        window.set_destroy_with_parent(true);
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // ── Toolbar ──────────────────────────────────────────────────────────────
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(4);

    let btn_add    = btn_icon("list-add-symbolic",      "Nova entrada");
    let btn_edit   = btn_icon("document-edit-symbolic", "Editar");
    let btn_delete = btn_icon("edit-delete-symbolic",   "Excluir");
    let btn_copy_u = btn_icon("edit-copy-symbolic",     "Copiar usuário");
    let btn_copy_p = btn_icon("dialog-password-symbolic", "Copiar senha");
    let btn_fill   = btn_icon("go-down-symbolic",       "Preencher página");
    let btn_lock   = btn_icon("system-lock-screen-symbolic", "Bloquear cofre");

    btn_add.style_context().add_class("suggested-action");
    btn_fill.set_tooltip_text(Some("Preencher campos de login na página atual"));

    toolbar.pack_start(&btn_add,    false, false, 0);
    toolbar.pack_start(&btn_edit,   false, false, 0);
    toolbar.pack_start(&btn_delete, false, false, 0);
    toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 4);
    toolbar.pack_start(&btn_copy_u, false, false, 0);
    toolbar.pack_start(&btn_copy_p, false, false, 0);
    toolbar.pack_start(&btn_fill,   false, false, 0);
    toolbar.pack_end(&btn_lock,     false, false, 0);

    // ── Filtro de busca rápida ────────────────────────────────────────────────
    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Filtrar por domínio ou usuário…"));
    search_entry.set_margin_start(8);
    search_entry.set_margin_end(8);
    search_entry.set_margin_bottom(4);

    // ── TreeView ─────────────────────────────────────────────────────────────
    // Colunas: domain (String), username (String), password_masked (String)
    let list_store = gtk::ListStore::new(&[
        glib::Type::STRING, // 0 domain
        glib::Type::STRING, // 1 username
        glib::Type::STRING, // 2 senha mascarada
    ]);

    let populate_store = {
        let store_ref = store.clone();
        let ls = list_store.clone();
        let search = search_entry.clone();
        move || {
            ls.clear();
            let filter = search.text().to_string().to_ascii_lowercase();
            let borrow = store_ref.borrow();
            for entry in borrow.entries() {
                if !filter.is_empty()
                    && !entry.domain.to_ascii_lowercase().contains(&filter)
                    && !entry.username.to_ascii_lowercase().contains(&filter)
                {
                    continue;
                }
                let iter = ls.append();
                ls.set(&iter, &[
                    (0, &entry.domain),
                    (1, &entry.username),
                    (2, &"••••••••"),
                ]);
            }
        }
    };

    populate_store();

    let tree = gtk::TreeView::with_model(&list_store);
    tree.set_headers_visible(true);

    let col = |title: &str, col_id: i32| {
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_resizable(true);
        let cell = gtk::CellRendererText::new();
        cell.set_property("ellipsize", pango::EllipsizeMode::End);
        gtk::prelude::CellLayoutExt::pack_start(&c, &cell, true);
        gtk::prelude::CellLayoutExt::add_attribute(&c, &cell, "text", col_id);
        c
    };

    let col_domain = col("Domínio", 0);
    col_domain.set_min_width(200);
    let col_user = col("Usuário / E-mail", 1);
    col_user.set_min_width(200);
    let col_pass = col("Senha", 2);
    col_pass.set_min_width(100);
    tree.append_column(&col_domain);
    tree.append_column(&col_user);
    tree.append_column(&col_pass);

    let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroller.add(&tree);

    // Destaca entradas do domínio atual.
    if let Some(ref url) = current_url {
        let domain = url.clone();
        let borrow = store.borrow();
        let matches: Vec<usize> = borrow
            .entries_for_domain(&domain)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        drop(borrow);
        if let Some(first) = matches.first() {
            if let Some(path) = list_store.path(&list_store.iter_nth_child(None, *first as i32).unwrap_or_else(|| list_store.iter_first().unwrap())) {
                tree.selection().select_path(&path);
                tree.scroll_to_cell(Some(&path), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
            }
        }
    }

    vbox.pack_start(&toolbar, false, false, 0);
    vbox.pack_start(&search_entry, false, false, 0);
    vbox.pack_start(&scroller, true, true, 0);

    window.add(&vbox);
    window.show_all();

    // ── Helpers para obter entrada selecionada ────────────────────────────────
    let store_for_idx = store.clone();
    let get_selected_index = {
        let tree_ref = tree.clone();
        let list_ref = list_store.clone();
        move || -> Option<usize> {
            let (paths, _) = tree_ref.selection().selected_rows();
            let path = paths.into_iter().next()?;
            let iter = list_ref.iter(&path)?;
            let domain: String = list_ref.value(&iter, 0).get().ok()?;
            let username: String = list_ref.value(&iter, 1).get().ok()?;
            let borrow = store_for_idx.borrow();
            borrow.entries().iter().position(|e| e.domain == domain && e.username == username)
        }
    };

    // ── Botão: Adicionar ─────────────────────────────────────────────────────
    {
        let store_add = store.clone();
        let populate = populate_store.clone();
        let mpw = master_pw.clone();
        let win_ref = window.clone();
        btn_add.connect_clicked(move |_| {
            show_entry_dialog(None, &win_ref, {
                let s = store_add.clone();
                let p = populate.clone();
                let pw = mpw.clone();
                move |entry| {
                    if let Err(e) = s.borrow_mut().add(entry, &pw) {
                        eprintln!("[passwords] Erro ao adicionar: {e}");
                    }
                    p();
                }
            });
        });
    }

    // ── Botão: Editar ─────────────────────────────────────────────────────────
    {
        let store_edit = store.clone();
        let populate = populate_store.clone();
        let mpw = master_pw.clone();
        let win_ref = window.clone();
        let get_idx = get_selected_index.clone();
        btn_edit.connect_clicked(move |_| {
            if let Some(idx) = get_idx() {
                let existing = store_edit.borrow().entries()[idx].clone();
                show_entry_dialog(Some(existing), &win_ref, {
                    let s = store_edit.clone();
                    let p = populate.clone();
                    let pw = mpw.clone();
                    move |entry| {
                        if let Err(e) = s.borrow_mut().update(idx, entry, &pw) {
                            eprintln!("[passwords] Erro ao editar: {e}");
                        }
                        p();
                    }
                });
            }
        });
    }

    // ── Botão: Excluir ────────────────────────────────────────────────────────
    {
        let store_del = store.clone();
        let populate = populate_store.clone();
        let mpw = master_pw.clone();
        let get_idx = get_selected_index.clone();
        let win_ref = window.clone();
        btn_delete.connect_clicked(move |_| {
            if let Some(idx) = get_idx() {
                let domain = store_del.borrow().entries()[idx].domain.clone();
                let confirm = gtk::MessageDialog::new(
                    Some(&win_ref),
                    gtk::DialogFlags::MODAL,
                    gtk::MessageType::Question,
                    gtk::ButtonsType::YesNo,
                    &format!("Excluir senha de \"{}\"?", domain),
                );
                if confirm.run() == gtk::ResponseType::Yes {
                    let _ = store_del.borrow_mut().remove(idx, &mpw);
                    populate();
                }
                confirm.close();
            }
        });
    }

    // ── Botão: Copiar usuário ─────────────────────────────────────────────────
    {
        let store_cu = store.clone();
        let get_idx = get_selected_index.clone();
        btn_copy_u.connect_clicked(move |_| {
            if let Some(idx) = get_idx() {
                let username = store_cu.borrow().entries()[idx].username.clone();
                if let Some(clipboard) = gtk::Clipboard::default(&gtk::gdk::Display::default().unwrap()) {
                    clipboard.set_text(&username);
                }
            }
        });
    }

    // ── Botão: Copiar senha ───────────────────────────────────────────────────
    {
        let store_cp = store.clone();
        let get_idx = get_selected_index.clone();
        btn_copy_p.connect_clicked(move |_| {
            if let Some(idx) = get_idx() {
                let password = store_cp.borrow().entries()[idx].password.clone();
                if let Some(clipboard) = gtk::Clipboard::default(&gtk::gdk::Display::default().unwrap()) {
                    clipboard.set_text(&password);
                    // Limpa clipboard após 30 segundos por segurança.
                    let cb = clipboard.clone();
                    glib::timeout_add_seconds_local(30, move || {
                        cb.clear();
                        glib::ControlFlow::Break
                    });
                }
            }
        });
    }

    // ── Botão: Preencher página ───────────────────────────────────────────────
    {
        let store_fill = store.clone();
        let get_idx = get_selected_index.clone();
        let cb = fill_callback.clone();
        let win_ref = window.clone();
        btn_fill.connect_clicked(move |_| {
            if let Some(idx) = get_idx() {
                let (user, pass) = {
                    let b = store_fill.borrow();
                    let e = &b.entries()[idx];
                    (e.username.clone(), e.password.clone())
                };
                if let Some(ref f) = cb {
                    f(user, pass);
                    win_ref.close();
                }
            }
        });
    }

    // ── Botão: Bloquear ───────────────────────────────────────────────────────
    {
        let store_lock = store.clone();
        let win_ref = window.clone();
        btn_lock.connect_clicked(move |_| {
            store_lock.borrow_mut().lock();
            win_ref.close();
        });
    }

    // ── Filtro de busca ───────────────────────────────────────────────────────
    {
        let populate = populate_store.clone();
        search_entry.connect_search_changed(move |_| { populate(); });
    }
}

// ─── Dialog de Adicionar / Editar entrada ────────────────────────────────────

fn show_entry_dialog(
    existing: Option<PasswordEntry>,
    parent: &gtk::Window,
    on_save: impl Fn(PasswordEntry) + 'static,
) {
    use gtk::prelude::*;

    let editing = existing.is_some();
    let dialog = gtk::Dialog::new();
    dialog.set_title(if editing { "Editar senha" } else { "Nova senha" });
    dialog.set_modal(true);
    dialog.set_default_width(380);
    dialog.set_transient_for(Some(parent));

    let content = dialog.content_area();
    content.set_spacing(8);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_margin_top(12);
    content.set_margin_bottom(8);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);

    let lbl_domain = gtk::Label::new(Some("Domínio / Site:"));
    lbl_domain.set_xalign(1.0);
    let ent_domain = gtk::Entry::new();
    ent_domain.set_placeholder_text(Some("ex: github.com"));
    ent_domain.set_hexpand(true);

    let lbl_user = gtk::Label::new(Some("Usuário / E-mail:"));
    lbl_user.set_xalign(1.0);
    let ent_user = gtk::Entry::new();
    ent_user.set_placeholder_text(Some("ex: usuario@email.com"));
    ent_user.set_hexpand(true);

    let lbl_pass = gtk::Label::new(Some("Senha:"));
    lbl_pass.set_xalign(1.0);
    let ent_pass = gtk::Entry::new();
    ent_pass.set_visibility(false);
    ent_pass.set_placeholder_text(Some("••••••••"));
    ent_pass.set_hexpand(true);

    // Botão de revelar/ocultar senha.
    let btn_toggle = gtk::ToggleButton::new();
    btn_toggle.set_image(Some(&gtk::Image::from_icon_name(
        Some("view-reveal-symbolic"), gtk::IconSize::Button,
    )));
    btn_toggle.set_relief(gtk::ReliefStyle::None);
    let ent_pass_toggle = ent_pass.clone();
    btn_toggle.connect_toggled(move |b| {
        ent_pass_toggle.set_visibility(b.is_active());
    });

    let pass_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    pass_box.pack_start(&ent_pass, true, true, 0);
    pass_box.pack_start(&btn_toggle, false, false, 0);

    grid.attach(&lbl_domain, 0, 0, 1, 1);
    grid.attach(&ent_domain, 1, 0, 1, 1);
    grid.attach(&lbl_user,   0, 1, 1, 1);
    grid.attach(&ent_user,   1, 1, 1, 1);
    grid.attach(&lbl_pass,   0, 2, 1, 1);
    grid.attach(&pass_box,   1, 2, 1, 1);

    content.pack_start(&grid, true, true, 0);

    if let Some(ref e) = existing {
        ent_domain.set_text(&e.domain);
        ent_user.set_text(&e.username);
        ent_pass.set_text(&e.password);
    }

    dialog.add_button("Cancelar", gtk::ResponseType::Cancel);
    let ok_btn = dialog.add_button(if editing { "Salvar" } else { "Adicionar" }, gtk::ResponseType::Ok);
    ok_btn.style_context().add_class("suggested-action");
    dialog.set_default_response(gtk::ResponseType::Ok);
    dialog.show_all();

    dialog.connect_response(move |dlg, resp| {
        if resp == gtk::ResponseType::Ok {
            let domain   = ent_domain.text().to_string();
            let username = ent_user.text().to_string();
            let password = ent_pass.text().to_string();
            if !domain.is_empty() && !username.is_empty() && !password.is_empty() {
                on_save(PasswordEntry { domain, username, password });
            }
        }
        dlg.close();
    });
}

// ─── Util ─────────────────────────────────────────────────────────────────────

fn btn_icon(icon: &str, tooltip: &str) -> gtk::Button {
    use gtk::prelude::*;
    let b = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
    b.set_tooltip_text(Some(tooltip));
    b
}
