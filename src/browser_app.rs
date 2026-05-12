use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, HeaderBar, Entry, Notebook, Button, Label,
    Box, Orientation, EntryCompletion, ListStore, Image, ToggleButton,
};
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;
use webkit2gtk::{
    WebView, WebContext, WebContextExt, WebViewExt, SettingsExt,
    HardwareAccelerationPolicy, CacheModel,
    WebsiteDataManager, UserContentManager,
};
use crate::history::HistoryManager;
use crate::adblock::AdBlock;

/// Cada aba possui (container_widget, webview, context).
/// Manter o WebContext vivo é essencial: ao dropar, o Web Process é encerrado.
type TabEntry = (gtk::Widget, WebView, WebContext);

pub struct BrowserApp {
    pub window: ApplicationWindow,
    pub notebook: Notebook,
    pub url_entry: Entry,
    pub webviews: Rc<RefCell<Vec<TabEntry>>>,
    pub history: HistoryManager,
    pub adblock: Rc<AdBlock>,
}

impl BrowserApp {
    pub fn new(app: &Application) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Rust Browser POC")
            .default_width(1280)
            .default_height(800)
            .build();

        let header_bar = HeaderBar::new();
        header_bar.set_show_close_button(true);

        let back_button = Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
        let forward_button = Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
        let reload_button = Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Button);
        header_bar.pack_start(&back_button);
        header_bar.pack_start(&forward_button);
        header_bar.pack_start(&reload_button);

        let url_entry = Entry::new();
        url_entry.set_placeholder_text(Some("Enter URL..."));
        url_entry.set_width_request(500);
        header_bar.set_custom_title(Some(&url_entry));

        let new_tab_button = Button::from_icon_name(Some("list-add-symbolic"), gtk::IconSize::Button);
        header_bar.pack_end(&new_tab_button);

        // Toggle do AdBlock no header — estilo clean / minimalista.
        let adblock_toggle = ToggleButton::new();
        let adblock_dot = gtk::Label::new(Some("●"));
        adblock_dot.style_context().add_class("adblock-dot");
        let adblock_label = gtk::Label::new(Some("AdBlock"));
        let adblock_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        adblock_box.pack_start(&adblock_dot, false, false, 0);
        adblock_box.pack_start(&adblock_label, false, false, 0);
        adblock_toggle.add(&adblock_box);
        adblock_toggle.set_relief(gtk::ReliefStyle::None);
        adblock_toggle.set_tooltip_text(Some("AdBlock: clique para alternar"));
        header_bar.pack_end(&adblock_toggle);

        // CSS clean: só um pontinho colorido + texto secundário muda. Sem fundo berrante.
        let css = gtk::CssProvider::new();
        let _ = css.load_from_data(b"
            button.adblock-toggle { padding: 2px 10px; border-radius: 14px; border: 1px solid alpha(@theme_fg_color, 0.18); }
            button.adblock-toggle:hover { background: alpha(@theme_fg_color, 0.06); }
            button.adblock-toggle:checked { background: alpha(@theme_fg_color, 0.10); border-color: alpha(@theme_fg_color, 0.30); }
            button.adblock-toggle label.adblock-dot { font-size: 10px; color: #adb5bd; padding-right: 2px; }
            button.adblock-toggle.on label.adblock-dot { color: #4c9aff; }
            button.adblock-toggle.off label.adblock-dot { color: #adb5bd; }
            button.adblock-toggle label:not(.adblock-dot) { font-size: 11px; font-weight: 500; opacity: 0.85; }
        ");
        if let Some(screen) = gtk::gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(
                &screen,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        adblock_toggle.style_context().add_class("adblock-toggle");

        window.set_titlebar(Some(&header_bar));

        let notebook = Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_show_tabs(true);
        window.add(&notebook);

        let webviews: Rc<RefCell<Vec<TabEntry>>> = Rc::new(RefCell::new(Vec::new()));
        let history = HistoryManager::new();

        let data_dir = PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
        ).join(".cache/rust-browser-poc");
        let _ = std::fs::create_dir_all(&data_dir);

        // === AdBlock nativo (WebKitUserContentFilterStore) ===
        let adblock = AdBlock::new(&data_dir);
        adblock_toggle.set_active(adblock.enabled());
        // Aplica estado visual inicial.
        {
            let ctx = adblock_toggle.style_context();
            if adblock.enabled() {
                ctx.add_class("on"); ctx.remove_class("off");
                adblock_label.set_text("AdBlock");
            } else {
                ctx.add_class("off"); ctx.remove_class("on");
                adblock_label.set_text("AdBlock off");
            }
        }

        // Autocomplete da barra de endereço
        let completion = EntryCompletion::new();
        let store = ListStore::new(&[glib::Type::STRING]);
        for item in history.load() {
            let iter = store.append();
            store.set_value(&iter, 0, &item.to_value());
        }
        completion.set_model(Some(&store));
        completion.set_text_column(0);
        url_entry.set_completion(Some(&completion));

        let app_instance = Self {
            window,
            notebook,
            url_entry,
            webviews,
            history,
            adblock,
        };

        // Wire do toggle do AdBlock (feedback visual clean).
        let adblock_for_toggle = app_instance.adblock.clone();
        let label_t = adblock_label.clone();
        adblock_toggle.connect_toggled(move |btn| {
            let on = btn.is_active();
            adblock_for_toggle.set_enabled(on);
            let ctx = btn.style_context();
            if on {
                ctx.add_class("on"); ctx.remove_class("off");
                label_t.set_text("AdBlock");
            } else {
                ctx.add_class("off"); ctx.remove_class("on");
                label_t.set_text("AdBlock off");
            }
        });

        // === Navegação ===
        let webviews_b = app_instance.webviews.clone();
        let notebook_b = app_instance.notebook.clone();
        back_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_b, &webviews_b) {
                if wv.can_go_back() { wv.go_back(); }
            }
        });
        let webviews_f = app_instance.webviews.clone();
        let notebook_f = app_instance.notebook.clone();
        forward_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_f, &webviews_f) {
                if wv.can_go_forward() { wv.go_forward(); }
            }
        });
        let webviews_r = app_instance.webviews.clone();
        let notebook_r = app_instance.notebook.clone();
        reload_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_r, &webviews_r) {
                wv.reload();
            }
        });

        // === New tab ===
        let notebook_clone = app_instance.notebook.clone();
        let webviews_clone = app_instance.webviews.clone();
        let url_entry_clone = app_instance.url_entry.clone();
        let data_dir_clone = data_dir.clone();
        let adblock_clone = app_instance.adblock.clone();
        new_tab_button.connect_clicked(move |_| {
            Self::add_tab(&notebook_clone, webviews_clone.clone(), url_entry_clone.clone(), &data_dir_clone, &adblock_clone);
        });

        // Aba inicial
        Self::add_tab(
            &app_instance.notebook,
            app_instance.webviews.clone(),
            app_instance.url_entry.clone(),
            &data_dir,
            &app_instance.adblock,
        );

        // === URL entry ===
        let notebook_nav = app_instance.notebook.clone();
        let webviews_nav = app_instance.webviews.clone();
        let store_nav = store.clone();
        let history_nav = app_instance.history.clone();
        app_instance.url_entry.connect_activate(move |entry| {
            let input = entry.text().to_string();
            let url = normalize_url(&input);
            history_nav.add(&url);

            // Atualiza histórico de autocomplete
            let mut found = false;
            if let Some(iter) = store_nav.iter_first() {
                loop {
                    let value = store_nav.value(&iter, 0).get::<String>().unwrap_or_default();
                    if value == url { found = true; break; }
                    if !store_nav.iter_next(&iter) { break; }
                }
            }
            if !found {
                let iter = store_nav.append();
                store_nav.set_value(&iter, 0, &url.to_value());
            }

            // DNS prefetch + load (no Web Process da aba ativa)
            if let Some(idx) = notebook_nav.current_page() {
                if let Some(page) = notebook_nav.nth_page(Some(idx)) {
                    let list = webviews_nav.borrow();
                    if let Some((_, wv, ctx)) = list.iter().find(|(w, _, _)| w == &page) {
                        if let Some(host) = extract_host(&url) {
                            ctx.prefetch_dns(host);
                        }
                        wv.load_uri(&url);
                    }
                }
            }
        });

        // === Tab switch ===
        let url_entry_switch = app_instance.url_entry.clone();
        let webviews_switch = app_instance.webviews.clone();
        app_instance.notebook.connect_switch_page(move |_, page, _| {
            let list = webviews_switch.borrow();
            if let Some((_, wv, _)) = list.iter().find(|(w, _, _)| w == page) {
                if let Some(u) = wv.uri() {
                    url_entry_switch.set_text(u.as_str());
                }
            }
        });

        // Seleciona tudo ao clicar na barra
        app_instance.url_entry.connect_button_release_event(|entry, _| {
            entry.select_region(0, -1);
            glib::Propagation::Proceed
        });

        app_instance
    }

    /// Cria uma aba com WebContext dedicado (Web Process isolado),
    /// `WebsiteDataManager` próprio (cache em disco persistente) e
    /// settings de aceleração GPU agressivos.
    fn add_tab(
        notebook: &Notebook,
        webviews: Rc<RefCell<Vec<TabEntry>>>,
        url_entry: Entry,
        data_dir: &PathBuf,
        adblock: &Rc<AdBlock>,
    ) {
        // === Diretórios por aba ===
        let tab_id = format!(
            "tab-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            webviews.borrow().len()
        );
        let tab_root = data_dir.join(&tab_id);
        let cache_root = tab_root.join("cache");
        let data_root = tab_root.join("data");
        let _ = std::fs::create_dir_all(&cache_root);
        let _ = std::fs::create_dir_all(&data_root);

        // === WebsiteDataManager dedicado ===
        // Em WebKit2GTK 2.40+, base_cache_directory e base_data_directory já
        // consolidam todos os subdiretórios (disk cache, dom cache, hsts, indexeddb,
        // local storage, service workers, etc.) automaticamente.
        let cache_str = cache_root.to_string_lossy().to_string();
        let data_str = data_root.to_string_lossy().to_string();
        let wdm = WebsiteDataManager::builder()
            .base_cache_directory(&cache_str)
            .base_data_directory(&data_str)
            .build();

        // === WebContext dedicado (= 1 Web Process por aba) ===
        let ctx = WebContext::with_website_data_manager(&wdm);
        // Modo "Web Browser" = cache agressivo, recursos pré-carregados, JIT, JS tier-up.
        ctx.set_cache_model(CacheModel::WebBrowser);
        // Em WebKit2GTK 2.40+ o process model é sempre multi-process (1 Web Process por
        // WebContext) — não precisamos mais setar manualmente.
        // TLS: política rigorosa é o default desde 2.32.
        // Spell-check desligado por default (economia de CPU).
        ctx.set_spell_checking_enabled(false);
        // Favicons no mesmo diretório de cache da aba.
        ctx.set_favicon_database_directory(Some(&cache_str));

        // === UserContentManager dedicado (para AdBlock) ===
        let ucm = UserContentManager::new();
        adblock.register_manager(ucm.clone());
        // Userscripts específicos (ex.: skip de anúncios no YouTube).
        crate::userscripts::register_youtube_adblock(&ucm);

        // === WebView vinculado a este contexto + UCM ===
        let webview = WebView::builder()
            .web_context(&ctx)
            .user_content_manager(&ucm)
            .build();

        // === Settings de performance / GPU ===
        if let Some(settings) = WebViewExt::settings(&webview) {
            // Aceleração de hardware sempre (GL compositor + texture upload via GPU).
            settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
            settings.set_enable_webgl(true);
            settings.set_enable_smooth_scrolling(true);

            // Mídia (HW-accel decoding quando disponível via GStreamer-VAAPI/NVDEC).
            settings.set_enable_media_stream(true);
            settings.set_enable_mediasource(true);
            settings.set_enable_encrypted_media(true);
            settings.set_media_playback_requires_user_gesture(false);

            // JavaScript (JIT FTL/B3 já é default).
            settings.set_enable_javascript(true);
            settings.set_enable_javascript_markup(true);
            settings.set_javascript_can_open_windows_automatically(true);

            // Cache de página (back/forward instantâneo).
            settings.set_enable_page_cache(true);
            // Nota: enable_offline_web_application_cache foi removido — o AppCache
            // foi descontinuado da plataforma web (use ServiceWorkers para offline).

            // Sem overhead de inspector em release.
            settings.set_enable_developer_extras(false);

            // User-Agent moderno para evitar fallbacks lentos em sites.
            settings.set_user_agent_with_application_details(
                Some("RustBrowserPOC"),
                Some("1.0"),
            );
        }

        // === Container e tab header ===
        let container = Box::new(Orientation::Vertical, 0);
        container.pack_start(&webview, true, true, 0);
        container.show_all();

        let tab_box = Box::new(Orientation::Horizontal, 4);
        let tab_icon = Image::from_icon_name(Some("browser-symbolic"), gtk::IconSize::Menu);
        let tab_title = Label::new(Some("Loading..."));
        tab_title.set_width_request(120);
        tab_title.set_max_width_chars(40);
        tab_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        tab_title.set_xalign(0.0);
        let close_button = Button::from_icon_name(Some("window-close-symbolic"), gtk::IconSize::Menu);
        close_button.set_relief(gtk::ReliefStyle::None);
        tab_box.pack_start(&tab_icon, false, false, 0);
        tab_box.pack_start(&tab_title, true, true, 0);
        tab_box.pack_start(&close_button, false, false, 0);
        tab_box.show_all();

        // === Signals: title + uri changes ===
        let tab_title_clone = tab_title.clone();
        webview.connect_title_notify(move |wv| {
            let t = wv.title().map(|g| g.to_string()).unwrap_or_else(|| "Untitled".into());
            tab_title_clone.set_text(if t.is_empty() { "Loading..." } else { &t });
        });

        let url_entry_load = url_entry.clone();
        webview.connect_uri_notify(move |wv| {
            if let Some(u) = wv.uri() {
                url_entry_load.set_text(u.as_str());
            }
        });

        // === Adiciona ao notebook ===
        let index = notebook.append_page(&container, Some(&tab_box));
        notebook.show_all();
        notebook.set_current_page(Some(index));

        // === Close button ===
        let notebook_close = notebook.clone();
        let container_widget: gtk::Widget = container.clone().upcast();
        let webviews_close = webviews.clone();
        close_button.connect_clicked(move |_| {
            if let Some(i) = notebook_close.page_num(&container_widget) {
                notebook_close.remove_page(Some(i));
                // Ao remover, o (webview, ctx) é dropado -> Web Process encerrado.
                webviews_close.borrow_mut().retain(|(w, _, _)| w != &container_widget);
            }
        });

        // Navega para Google na criação (após registrar handlers).
        webview.load_uri("https://www.google.com");

        webviews.borrow_mut().push((container.upcast(), webview, ctx));
    }

    pub fn show(&self) {
        self.window.show_all();
    }
}

// === Helpers ===

fn current_webview(notebook: &Notebook, webviews: &Rc<RefCell<Vec<TabEntry>>>) -> Option<WebView> {
    let idx = notebook.current_page()?;
    let page = notebook.nth_page(Some(idx))?;
    webviews.borrow().iter().find(|(w, _, _)| w == &page).map(|(_, wv, _)| wv.clone())
}

fn normalize_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
        || trimmed.starts_with("about:")
        || trimmed.starts_with("webkit://")
        || trimmed.starts_with("localhost")
    {
        trimmed.to_string()
    } else if trimmed.contains('.') && !trimmed.contains(' ') {
        format!("https://{}", trimmed)
    } else {
        format!("https://www.google.com/search?q={}", trimmed.replace(' ', "+"))
    }
}

/// Extrai o hostname de uma URL `scheme://host[:port]/...` sem depender da crate `url`.
fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map(|x| x.1).unwrap_or(url);
    let host_part = after_scheme.split(|c| c == '/' || c == '?' || c == '#').next()?;
    let host_part = host_part.rsplit_once('@').map(|x| x.1).unwrap_or(host_part);
    let host = host_part.split(':').next()?;
    if host.is_empty() { None } else { Some(host) }
}
