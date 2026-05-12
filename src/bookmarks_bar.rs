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
use crate::settings::Settings;

const SETTING_VISIBLE: &str = "bookmarks_bar_visible";

pub struct BookmarksBar {
    /// Container final — adicione na sua janela.
    pub widget: gtk::Box,
    /// Onde os botões dos favoritos vão; é re-populado a cada rebuild.
    inner_bar: gtk::Box,
    store: Rc<BookmarksStore>,
    settings: Rc<Settings>,
    /// Callback chamada quando o usuário clica num link favorito.
    on_navigate: RefCell<Option<Rc<dyn Fn(&str)>>>,
    /// Callback chamada quando o usuário pede o gerenciador (engrenagem).
    on_manage: RefCell<Option<Rc<dyn Fn()>>>,
}

impl BookmarksBar {
    pub fn new(store: Rc<BookmarksStore>, settings: Rc<Settings>) -> Rc<Self> {
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

        let cog = gtk::MenuButton::new();
        cog.set_image(Some(&gtk::Image::from_icon_name(
            Some("emblem-system-symbolic"),
            gtk::IconSize::Menu,
        )));
        cog.set_relief(gtk::ReliefStyle::None);
        cog.set_tooltip_text(Some("Opções de favoritos"));
        cog.set_margin_end(6);

        // Popover do cog: Gerenciar + Ocultar.
        let popover = gtk::Popover::new(Some(&cog));
        popover.set_position(gtk::PositionType::Bottom);
        let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        pop_box.set_margin_start(6);
        pop_box.set_margin_end(6);
        pop_box.set_margin_top(6);
        pop_box.set_margin_bottom(6);

        let btn_manage = button_row("emblem-system-symbolic", "Gerenciar favoritos…");
        let btn_hide = button_row("view-restore-symbolic", "Ocultar barra de favoritos");
        pop_box.pack_start(&btn_manage, false, false, 0);
        pop_box.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 2);
        pop_box.pack_start(&btn_hide, false, false, 0);
        pop_box.show_all();
        popover.add(&pop_box);
        cog.set_popover(Some(&popover));

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
            settings: settings.clone(),
            on_navigate: RefCell::new(None),
            on_manage: RefCell::new(None),
        });

        // Aplica visibilidade persistida ANTES de mostrar o widget pai.
        me.apply_visibility();

        // Manage → callback externa.
        let me_mng = me.clone();
        let popover_mng = popover.clone();
        btn_manage.connect_clicked(move |_| {
            popover_mng.popdown();
            if let Some(f) = me_mng.on_manage.borrow().as_ref() { f(); }
        });

        // Hide → desliga visibilidade, persiste.
        let me_hide = me.clone();
        let popover_hide = popover.clone();
        btn_hide.connect_clicked(move |_| {
            popover_hide.popdown();
            me_hide.set_visible_persisted(false);
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

    /// Visibilidade atual lida das configurações (default: visível).
    pub fn is_visible_persisted(&self) -> bool {
        self.settings.get_bool(SETTING_VISIBLE, true)
    }

    /// Define visibilidade e persiste.
    pub fn set_visible_persisted(&self, visible: bool) {
        self.settings.set_bool(SETTING_VISIBLE, visible);
        self.apply_visibility();
    }

    /// Aplica o estado atual ao widget.
    fn apply_visibility(&self) {
        let v = self.is_visible_persisted();
        // `no_show_all=true` impede que `show_all()` da janela force o widget
        // visível quando o usuário escolheu ocultar. Quando ele pede pra
        // mostrar de novo, removemos a flag E chamamos show_all() explícito
        // (caso contrário os filhos da barra ficam invisíveis).
        self.widget.set_no_show_all(!v);
        if v {
            self.widget.show_all();
        } else {
            self.widget.hide();
        }
    }

    pub fn toggle_visible_persisted(&self) {
        self.set_visible_persisted(!self.is_visible_persisted());
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

/// Cria um botão "linha de menu": ícone + label, alinhado à esquerda, sem relief.
fn button_row(icon: &str, label: &str) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.set_relief(gtk::ReliefStyle::None);
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let img = gtk::Image::from_icon_name(Some(icon), gtk::IconSize::Menu);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    hbox.pack_start(&img, false, false, 0);
    hbox.pack_start(&lbl, true, true, 0);
    btn.add(&hbox);
    btn
}
