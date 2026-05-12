//! Barra de favoritos abaixo da URL bar.
//!
//! Renderiza os itens da raiz (`parent = 0`) como botões horizontais. Folders
//! abrem um `gtk::Popover` flutuante (igual ao Chrome) ancorado no botão, com
//! lista vertical dos filhos. Folders dentro de folders são suportados via
//! `MenuButton` recursivo (cada um com seu próprio popover).

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::bookmarks::{Bookmark, BookmarkKind, BookmarksStore};

pub struct BookmarksBar {
    /// Container final — adicione na sua janela.
    pub widget: gtk::Box,
    /// Onde os botões dos favoritos vão; é re-populado a cada rebuild.
    inner_bar: gtk::Box,
    store: Rc<BookmarksStore>,
    /// Callback chamada quando o usuário clica num link favorito.
    on_navigate: RefCell<Option<Rc<dyn Fn(&str)>>>,
    /// Callback chamada quando o usuário pede o gerenciador (engrenagem).
    on_manage: RefCell<Option<Rc<dyn Fn()>>>,
}

impl BookmarksBar {
    pub fn new(store: Rc<BookmarksStore>) -> Rc<Self> {
        // Wrapper: scroll horizontal de botões + engrenagem fixa à direita.
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.style_context().add_class("bookmarks-bar");

        let inner_bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        inner_bar.set_margin_start(6);
        inner_bar.set_margin_end(6);
        inner_bar.set_margin_top(2);
        inner_bar.set_margin_bottom(2);

        let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_min_content_height(28);
        scroller.add(&inner_bar);

        let cog = gtk::Button::from_icon_name(
            Some("emblem-system-symbolic"),
            gtk::IconSize::Menu,
        );
        cog.set_relief(gtk::ReliefStyle::None);
        cog.set_tooltip_text(Some("Gerenciar favoritos"));
        cog.set_margin_end(6);

        widget.pack_start(&scroller, true, true, 0);
        widget.pack_end(&cog, false, false, 0);

        // CSS clean integrado ao tema GTK.
        let css = gtk::CssProvider::new();
        let _ = css.load_from_data(b"
            box.bookmarks-bar {
              border-top: 1px solid alpha(@theme_fg_color, 0.10);
              border-bottom: 1px solid alpha(@theme_fg_color, 0.10);
              background: alpha(@theme_bg_color, 0.65);
            }
            box.bookmarks-bar button {
              padding: 1px 8px;
              border-radius: 4px;
              font-size: 11px;
            }
            box.bookmarks-bar button:hover {
              background: alpha(@theme_fg_color, 0.08);
            }
        ");
        if let Some(screen) = gtk::gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(
                &screen, &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let me = Rc::new(Self {
            widget,
            inner_bar,
            store: store.clone(),
            on_navigate: RefCell::new(None),
            on_manage: RefCell::new(None),
        });

        // Cog → manager.
        let me_cog = me.clone();
        cog.connect_clicked(move |_| {
            if let Some(f) = me_cog.on_manage.borrow().as_ref() { f(); }
        });

        // Listener: rebuild a cada mudança.
        let me_listener = me.clone();
        store.on_change(move || me_listener.rebuild());

        me.rebuild();
        me
    }

    pub fn set_on_navigate(&self, f: impl Fn(&str) + 'static) {
        *self.on_navigate.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_on_manage(&self, f: impl Fn() + 'static) {
        *self.on_manage.borrow_mut() = Some(Rc::new(f));
    }

    fn rebuild(&self) {
        // Limpa os botões antigos.
        for child in self.inner_bar.children() {
            self.inner_bar.remove(&child);
        }
        // Re-popula com filhos da raiz.
        let children = self.store.list_children(0);
        if children.is_empty() {
            let hint = gtk::Label::new(Some("Favoritos aparecerão aqui — clique na ☆"));
            hint.style_context().add_class("dim-label");
            hint.set_margin_start(4);
            self.inner_bar.pack_start(&hint, false, false, 0);
        } else {
            for bm in children {
                let w = self.build_widget(&bm);
                self.inner_bar.pack_start(&w, false, false, 0);
            }
        }
        self.inner_bar.show_all();
    }

    fn build_widget(&self, bm: &Bookmark) -> gtk::Widget {
        match &bm.kind {
            BookmarkKind::Link { url } => {
                let btn = gtk::Button::with_label(&truncate_title(&bm.title));
                btn.set_relief(gtk::ReliefStyle::None);
                btn.set_tooltip_text(Some(url));
                let url = url.clone();
                let nav = self.on_navigate.clone();
                btn.connect_clicked(move |_| {
                    if let Some(f) = nav.borrow().as_ref() { f(&url); }
                });
                btn.upcast()
            }
            BookmarkKind::Folder => {
                let mb = gtk::MenuButton::new();
                let label = gtk::Label::new(Some(&truncate_title(&bm.title)));
                let icon = gtk::Image::from_icon_name(
                    Some("folder-symbolic"),
                    gtk::IconSize::Menu,
                );
                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                hbox.pack_start(&icon, false, false, 0);
                hbox.pack_start(&label, false, false, 0);
                mb.add(&hbox);
                mb.set_relief(gtk::ReliefStyle::None);

                // Popover flutuante com filhos.
                let popover = gtk::Popover::new(Some(&mb));
                popover.set_position(gtk::PositionType::Bottom);
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
                vbox.set_margin_start(6);
                vbox.set_margin_end(6);
                vbox.set_margin_top(6);
                vbox.set_margin_bottom(6);
                for child in self.store.list_children(bm.id) {
                    let inner = self.build_widget(&child);
                    vbox.pack_start(&inner, false, false, 0);
                }
                if self.store.list_children(bm.id).is_empty() {
                    let empty = gtk::Label::new(Some("(pasta vazia)"));
                    empty.style_context().add_class("dim-label");
                    vbox.pack_start(&empty, false, false, 0);
                }
                vbox.show_all();
                popover.add(&vbox);
                mb.set_popover(Some(&popover));
                mb.upcast()
            }
        }
    }
}

fn truncate_title(s: &str) -> String {
    const MAX: usize = 32;
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX { s.to_string() } else {
        format!("{}…", chars[..MAX - 1].iter().collect::<String>())
    }
}
