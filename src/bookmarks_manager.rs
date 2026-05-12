//! Gerenciador de favoritos — dialog GTK nativo com TreeView hierárquica.
//!
//! Permite: criar pasta, criar bookmark, renomear, alterar URL, excluir
//! (recursivamente), mover entre pastas, mover na ordem (↑/↓).
//!
//! Tudo via GTK puro — sem janelas modais HTML, sem ponte JS↔Rust.

use gtk::prelude::*;
use std::rc::Rc;

use crate::bookmarks::{Bookmark, BookmarkKind, BookmarksStore};

const COL_ID: u32 = 0;
const COL_TITLE: u32 = 1;
const COL_URL: u32 = 2;
const COL_KIND: u32 = 3; // "F" ou "L"

pub fn open_manager(store: &Rc<BookmarksStore>, parent: Option<&gtk::Window>) {
    let dialog = gtk::Window::builder()
        .title("Gerenciar favoritos")
        .default_width(760)
        .default_height(540)
        .modal(true)
        .build();
    if let Some(p) = parent {
        dialog.set_transient_for(Some(p));
        dialog.set_destroy_with_parent(true);
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // === Toolbar ===
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(8);

    let btn_new_folder = button_with_icon("folder-new-symbolic", "Nova pasta");
    let btn_new_link = button_with_icon("emblem-favorite-symbolic", "Novo favorito");
    let btn_edit = button_with_icon("document-edit-symbolic", "Editar");
    let btn_delete = button_with_icon("edit-delete-symbolic", "Excluir");
    let btn_up = gtk::Button::from_icon_name(Some("go-up-symbolic"), gtk::IconSize::Button);
    let btn_down = gtk::Button::from_icon_name(Some("go-down-symbolic"), gtk::IconSize::Button);
    let btn_move = button_with_icon("folder-symbolic", "Mover para…");
    btn_new_folder.style_context().add_class("suggested-action");

    toolbar.pack_start(&btn_new_folder, false, false, 0);
    toolbar.pack_start(&btn_new_link, false, false, 0);
    toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 4);
    toolbar.pack_start(&btn_edit, false, false, 0);
    toolbar.pack_start(&btn_delete, false, false, 0);
    toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 4);
    toolbar.pack_start(&btn_up, false, false, 0);
    toolbar.pack_start(&btn_down, false, false, 0);
    toolbar.pack_end(&btn_move, false, false, 0);

    // === TreeView ===
    // Columns: u64 (id), String (title), String (url), String (kind)
    let store_tv = gtk::TreeStore::new(&[
        glib::Type::U64, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING,
    ]);
    let tree = gtk::TreeView::with_model(&store_tv);
    tree.set_reorderable(false); // controlamos via botões para integridade
    tree.set_headers_visible(true);

    // Coluna 1: Título com ícone (folder/link).
    let col_title = gtk::TreeViewColumn::new();
    col_title.set_title("Nome");
    col_title.set_resizable(true);
    col_title.set_min_width(280);

    let icon_cell = gtk::CellRendererPixbuf::new();
    gtk::prelude::CellLayoutExt::pack_start(&col_title, &icon_cell, false);
    gtk::prelude::CellLayoutExt::set_cell_data_func(&col_title, &icon_cell, Some(std::boxed::Box::new(
        |_layout: &gtk::CellLayout, cell: &gtk::CellRenderer, model: &gtk::TreeModel, iter: &gtk::TreeIter| {
            let kind: String = model.value(iter, COL_KIND as i32).get().unwrap_or_default();
            let icon_name = if kind == "F" { "folder-symbolic" } else { "emblem-favorite-symbolic" };
            cell.set_property("icon-name", icon_name);
        }
    )));

    let title_cell = gtk::CellRendererText::new();
    gtk::prelude::CellLayoutExt::pack_start(&col_title, &title_cell, true);
    gtk::prelude::CellLayoutExt::add_attribute(&col_title, &title_cell, "text", COL_TITLE as i32);

    tree.append_column(&col_title);

    // Coluna 2: URL (vazia para folders).
    let col_url = gtk::TreeViewColumn::new();
    col_url.set_title("URL");
    col_url.set_resizable(true);
    let url_cell = gtk::CellRendererText::new();
    url_cell.set_property("ellipsize", pango::EllipsizeMode::End);
    gtk::prelude::CellLayoutExt::pack_start(&col_url, &url_cell, true);
    gtk::prelude::CellLayoutExt::add_attribute(&col_url, &url_cell, "text", COL_URL as i32);
    tree.append_column(&col_url);

    let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroller.add(&tree);

    vbox.pack_start(&toolbar, false, false, 0);
    vbox.pack_start(&scroller, true, true, 0);
    dialog.add(&vbox);

    // === Re-popula o TreeStore a partir do BookmarksStore ===
    let store_for_rebuild = store.clone();
    let store_tv_for_rebuild = store_tv.clone();
    let tree_for_rebuild = tree.clone();
    let rebuild = Rc::new(move || {
        rebuild_tree(&store_tv_for_rebuild, &store_for_rebuild);
        tree_for_rebuild.expand_all();
    });
    rebuild();

    // === Auto-rebuild quando store mudar ===
    let rebuild_listener = rebuild.clone();
    store.on_change(move || rebuild_listener());

    // === Helpers ===
    let get_selected_id = {
        let tree = tree.clone();
        move || -> Option<u64> {
            let sel = tree.selection();
            let (model, iter) = sel.selected()?;
            let id: u64 = model.value(&iter, COL_ID as i32).get().ok()?;
            Some(id)
        }
    };

    // === Wire actions ===
    let store_a = store.clone();
    let dialog_for_nf = dialog.clone();
    let gsi_nf = get_selected_id.clone();
    btn_new_folder.connect_clicked(move |_| {
        let parent = parent_for_new(&store_a, gsi_nf());
        if let Some(name) = prompt_text(&dialog_for_nf, "Nova pasta", "Nome:", "") {
            if !name.is_empty() { store_a.add_folder(parent, &name); }
        }
    });

    let store_a = store.clone();
    let dialog_for_nl = dialog.clone();
    let gsi_nl = get_selected_id.clone();
    btn_new_link.connect_clicked(move |_| {
        let parent = parent_for_new(&store_a, gsi_nl());
        if let Some(name) = prompt_text(&dialog_for_nl, "Novo favorito", "Nome:", "") {
            if name.is_empty() { return; }
            if let Some(url) = prompt_text(&dialog_for_nl, "Novo favorito", "URL:", "https://") {
                if !url.is_empty() { store_a.add_link(parent, &name, &url); }
            }
        }
    });

    let store_a = store.clone();
    let dialog_for_e = dialog.clone();
    let gsi_e = get_selected_id.clone();
    btn_edit.connect_clicked(move |_| {
        let id = match gsi_e() { Some(i) => i, None => return };
        let bm = match store_a.get(id) { Some(b) => b, None => return };
        if let Some(name) = prompt_text(&dialog_for_e, "Editar", "Nome:", &bm.title) {
            if !name.is_empty() { store_a.rename(id, &name); }
        }
        if let BookmarkKind::Link { url } = &bm.kind {
            if let Some(new_url) = prompt_text(&dialog_for_e, "Editar", "URL:", url) {
                if !new_url.is_empty() { store_a.set_url(id, &new_url); }
            }
        }
    });

    let store_a = store.clone();
    let dialog_for_d = dialog.clone();
    let gsi_d = get_selected_id.clone();
    btn_delete.connect_clicked(move |_| {
        let id = match gsi_d() { Some(i) => i, None => return };
        let bm = match store_a.get(id) { Some(b) => b, None => return };
        let msg = format!("Excluir '{}'? Esta ação é definitiva.", bm.title);
        let confirm = gtk::MessageDialog::new(
            Some(&dialog_for_d), gtk::DialogFlags::MODAL,
            gtk::MessageType::Question, gtk::ButtonsType::OkCancel, &msg,
        );
        let resp = confirm.run();
        confirm.close();
        if resp == gtk::ResponseType::Ok { store_a.remove(id); }
    });

    let store_a = store.clone();
    let gsi_u = get_selected_id.clone();
    btn_up.connect_clicked(move |_| {
        if let Some(id) = gsi_u() { store_a.shift(id, -1); }
    });

    let store_a = store.clone();
    let gsi_dn = get_selected_id.clone();
    btn_down.connect_clicked(move |_| {
        if let Some(id) = gsi_dn() { store_a.shift(id, 1); }
    });

    let store_a = store.clone();
    let dialog_for_m = dialog.clone();
    let gsi_m = get_selected_id.clone();
    btn_move.connect_clicked(move |_| {
        let id = match gsi_m() { Some(i) => i, None => return };
        if let Some(new_parent) = choose_folder(&dialog_for_m, &store_a, id) {
            store_a.move_to(id, new_parent);
        }
    });

    dialog.show_all();
    dialog.present();
}

/// Botão com ícone à esquerda + label à direita (visual claro de ação).
fn button_with_icon(icon: &str, label: &str) -> gtk::Button {
    let btn = gtk::Button::new();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let img = gtk::Image::from_icon_name(Some(icon), gtk::IconSize::Button);
    let lbl = gtk::Label::new(Some(label));
    hbox.pack_start(&img, false, false, 0);
    hbox.pack_start(&lbl, false, false, 0);
    btn.add(&hbox);
    btn
}

fn parent_for_new(store: &Rc<BookmarksStore>, selected: Option<u64>) -> u64 {
    // Se o selecionado é folder, novo item vai dentro dele; senão, irmão (mesmo parent).
    match selected.and_then(|id| store.get(id)) {
        Some(Bookmark { kind: BookmarkKind::Folder, id, .. }) => id,
        Some(Bookmark { parent, .. }) => parent,
        None => 0,
    }
}

fn rebuild_tree(tv: &gtk::TreeStore, store: &Rc<BookmarksStore>) {
    tv.clear();
    let all = store.all();
    // Mapa id → iter para suportar nesting via parent. id=0 (raiz) representa
    // "sem pai" (None) — não inserimos um iter falso.
    use std::collections::HashMap;
    let mut iter_map: HashMap<u64, gtk::TreeIter> = HashMap::new();

    // Faz BFS por níveis: itens são inseridos quando seu parent já foi inserido.
    let mut remaining: Vec<Bookmark> = all.clone();
    let mut progress = true;
    while progress && !remaining.is_empty() {
        progress = false;
        let mut next_remaining = Vec::new();
        for bm in remaining.drain(..) {
            let parent_iter: Option<&gtk::TreeIter> = if bm.parent == 0 {
                None
            } else {
                iter_map.get(&bm.parent)
            };
            if bm.parent != 0 && parent_iter.is_none() {
                next_remaining.push(bm);
                continue;
            }
            let (url, kind) = match &bm.kind {
                BookmarkKind::Folder => (String::new(), "F"),
                BookmarkKind::Link { url } => (url.clone(), "L"),
            };
            let iter = tv.append(parent_iter);
            tv.set(&iter, &[
                (COL_ID, &bm.id),
                (COL_TITLE, &bm.title),
                (COL_URL, &url),
                (COL_KIND, &kind),
            ]);
            iter_map.insert(bm.id, iter);
            progress = true;
        }
        remaining = next_remaining;
    }
}

fn prompt_text(parent: &gtk::Window, title: &str, label_text: &str, default: &str) -> Option<String> {
    let dialog = gtk::Dialog::with_buttons(
        Some(title), Some(parent), gtk::DialogFlags::MODAL,
        &[("Cancelar", gtk::ResponseType::Cancel), ("OK", gtk::ResponseType::Ok)],
    );
    let content = dialog.content_area();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hbox.set_margin_start(12); hbox.set_margin_end(12);
    hbox.set_margin_top(8); hbox.set_margin_bottom(8);
    let lbl = gtk::Label::new(Some(label_text));
    let entry = gtk::Entry::new();
    entry.set_text(default);
    entry.set_width_request(360);
    entry.set_activates_default(true);
    hbox.pack_start(&lbl, false, false, 0);
    hbox.pack_start(&entry, true, true, 0);
    content.add(&hbox);
    dialog.set_default_response(gtk::ResponseType::Ok);
    dialog.show_all();
    let resp = dialog.run();
    let val = entry.text().to_string();
    dialog.close();
    if resp == gtk::ResponseType::Ok { Some(val) } else { None }
}

/// Mostra um TreeView só com folders e retorna o id escolhido (0 = raiz).
fn choose_folder(parent: &gtk::Window, store: &Rc<BookmarksStore>, exclude_id: u64) -> Option<u64> {
    let dialog = gtk::Dialog::with_buttons(
        Some("Mover para…"), Some(parent), gtk::DialogFlags::MODAL,
        &[("Cancelar", gtk::ResponseType::Cancel), ("Mover", gtk::ResponseType::Ok)],
    );
    let content = dialog.content_area();
    content.set_margin_start(8); content.set_margin_end(8);
    content.set_margin_top(8); content.set_margin_bottom(8);

    let tv_store = gtk::TreeStore::new(&[glib::Type::U64, glib::Type::STRING]);
    let tree = gtk::TreeView::with_model(&tv_store);
    tree.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let icon_cell = gtk::CellRendererPixbuf::new();
    icon_cell.set_property("icon-name", "folder-symbolic");
    gtk::prelude::CellLayoutExt::pack_start(&col, &icon_cell, false);
    let txt = gtk::CellRendererText::new();
    gtk::prelude::CellLayoutExt::pack_start(&col, &txt, true);
    gtk::prelude::CellLayoutExt::add_attribute(&col, &txt, "text", 1);
    tree.append_column(&col);

    // Insere "raiz" como primeiro nó.
    let root_iter = tv_store.append(None);
    tv_store.set(&root_iter, &[(0, &0u64), (1, &"📌 Barra de favoritos (raiz)")]);

    // Pasta tree.
    use std::collections::HashMap;
    let mut iter_map: HashMap<u64, gtk::TreeIter> = HashMap::new();
    iter_map.insert(0, root_iter.clone());
    let all = store.all();
    let mut remaining: Vec<Bookmark> = all.into_iter()
        .filter(|b| matches!(b.kind, BookmarkKind::Folder) && b.id != exclude_id)
        .collect();
    let mut progress = true;
    while progress && !remaining.is_empty() {
        progress = false;
        let mut next = Vec::new();
        for bm in remaining.drain(..) {
            // Pula folders descendentes do exclude_id (ciclo).
            if is_descendant_in_list(&bm, exclude_id, store) {
                continue;
            }
            let parent_iter = iter_map.get(&bm.parent).cloned();
            if parent_iter.is_none() && bm.parent != 0 {
                next.push(bm); continue;
            }
            let it = tv_store.append(parent_iter.as_ref());
            tv_store.set(&it, &[(0, &bm.id), (1, &bm.title)]);
            iter_map.insert(bm.id, it);
            progress = true;
        }
        remaining = next;
    }
    tree.expand_all();

    let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroller.set_size_request(360, 320);
    scroller.add(&tree);
    content.add(&scroller);

    dialog.show_all();
    let resp = dialog.run();
    let result: Option<u64> = if resp == gtk::ResponseType::Ok {
        tree.selection().selected().and_then(|(m, i)| m.value(&i, 0).get::<u64>().ok())
    } else { None };
    dialog.close();
    result
}

fn is_descendant_in_list(bm: &Bookmark, ancestor: u64, store: &Rc<BookmarksStore>) -> bool {
    if ancestor == 0 { return false; }
    let mut cur = bm.parent;
    while cur != 0 {
        if cur == ancestor { return true; }
        cur = match store.get(cur) { Some(p) => p.parent, None => return false };
    }
    false
}

// Pango namespace para CellRenderer ellipsize.
mod pango {
    pub use gtk::pango::EllipsizeMode;
}
