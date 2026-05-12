use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, HeaderBar, Entry, Notebook, Button, Label,
    Box, Orientation, EntryCompletion, ListStore, Image,
};
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;
use webkit2gtk::{
    WebView, WebContext, WebContextExt, WebViewExt, SettingsExt,
    HardwareAccelerationPolicy, CacheModel, WebsiteDataManager, UserContentManager,
    CookieManagerExt, CookiePersistentStorage, CookieAcceptPolicy,
    WebsiteDataManagerExt, LoadEvent,
};
use crate::history::HistoryManager;
use crate::adblock::AdBlock;

/// Cada aba possui (container_widget, webview).
type TabEntry = (gtk::Widget, WebView);

pub struct BrowserApp {
    pub window: ApplicationWindow,
    pub notebook: Notebook,
    pub url_entry: Entry,
    pub webviews: Rc<RefCell<Vec<TabEntry>>>,
    pub web_context: WebContext,
    pub history: HistoryManager,
    pub adblock: Rc<AdBlock>,
}

impl BrowserApp {
    pub fn new(app: &Application) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Rust Browser POC")
            .default_width(1024)
            .default_height(768)
            .build();

        let header_bar = HeaderBar::new();
        header_bar.set_show_close_button(true);

        // Navigation Buttons
        let back_button = Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
        let forward_button = Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
        let reload_button = Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Button);
        
        header_bar.pack_start(&back_button);
        header_bar.pack_start(&forward_button);
        header_bar.pack_start(&reload_button);

        // Address Entry
        let url_entry = Entry::new();
        url_entry.set_placeholder_text(Some("Enter URL..."));
        url_entry.set_width_request(400);
        header_bar.set_custom_title(Some(&url_entry));

        // New Tab Button
        let new_tab_button = Button::from_icon_name(Some("list-add-symbolic"), gtk::IconSize::Button);
        header_bar.pack_end(&new_tab_button);

        window.set_titlebar(Some(&header_bar));

        let notebook = Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_show_tabs(true);
        window.add(&notebook);

        let webviews = Rc::new(RefCell::new(Vec::new()));
        let history = HistoryManager::new();
        
        // Setup SINGLE persistent data directory
        let base_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
            .join(".cache/rust-browser-poc");
        
        // Pastas específicas para garantir que nada se perca
        let data_dir = base_dir.join("data");
        let cache_dir = base_dir.join("cache");
        let _ = std::fs::create_dir_all(&data_dir);
        let _ = std::fs::create_dir_all(&cache_dir);
        
        let data_path = data_dir.to_string_lossy().to_string();
        let cache_path = cache_dir.to_string_lossy().to_string();

        // WebsiteDataManager robusto para persistência real de disco.
        // Em WebKit2GTK 2.40+, base_data_directory + base_cache_directory
        // já criam automaticamente os subdiretórios (localstorage, indexeddb,
        // hsts, applications, etc.). Setá-los manualmente é deprecated.
        let manager = WebsiteDataManager::builder()
            .base_data_directory(&data_path)
            .base_cache_directory(&cache_path)
            .build();
        
        // Shared WebContext for ALL tabs
        let web_context = WebContext::with_website_data_manager(&manager);
        web_context.set_cache_model(CacheModel::WebBrowser);
        web_context.set_favicon_database_directory(Some(&format!("{}/favicons", cache_path)));

        // === PERSISTÊNCIA DE COOKIES ===
        // WebKit2GTK por padrão mantém cookies APENAS em memória, mesmo com
        // base_data_directory configurado. É preciso pedir explicitamente o
        // backend SQLite no CookieManager para que logins/sessões sobrevivam
        // ao fechamento do app. Sem isso, qualquer site logado pede login
        // novamente em todo restart.
        if let Some(cookie_manager) = manager.cookie_manager() {
            let cookies_path = format!("{}/cookies.sqlite", data_path);
            cookie_manager.set_persistent_storage(
                &cookies_path,
                CookiePersistentStorage::Sqlite,
            );
            // NoThirdParty bloqueia cookies de terceiros (privacidade) sem
            // quebrar sessões. Trocar para `Always` se algum site exigir.
            cookie_manager.set_accept_policy(CookieAcceptPolicy::NoThirdParty);
        }

        // Initialize AdBlock
        let adblock = AdBlock::new(&base_dir);

        // Autocomplete
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
            web_context,
            history,
            adblock,
        };

        // Navigation Actions
        let notebook_nav_back = app_instance.notebook.clone();
        let webviews_nav_back = app_instance.webviews.clone();
        back_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_nav_back, &webviews_nav_back) {
                wv.go_back();
            }
        });

        let notebook_nav_fwd = app_instance.notebook.clone();
        let webviews_nav_fwd = app_instance.webviews.clone();
        forward_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_nav_fwd, &webviews_nav_fwd) {
                wv.go_forward();
            }
        });

        let notebook_nav_reload = app_instance.notebook.clone();
        let webviews_nav_reload = app_instance.webviews.clone();
        reload_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_nav_reload, &webviews_nav_reload) {
                wv.reload();
            }
        });

        // New Tab signal
        let notebook_clone = app_instance.notebook.clone();
        let webviews_clone = app_instance.webviews.clone();
        let url_entry_clone = app_instance.url_entry.clone();
        let wc_clone = app_instance.web_context.clone();
        let adblock_clone = app_instance.adblock.clone();
        let history_for_new_tab = app_instance.history.clone();
        new_tab_button.connect_clicked(move |_| {
            Self::add_tab(&notebook_clone, webviews_clone.clone(), url_entry_clone.clone(), &wc_clone, &adblock_clone, history_for_new_tab.clone());
        });

        // Initial tab
        Self::add_tab(
            &app_instance.notebook,
            app_instance.webviews.clone(),
            app_instance.url_entry.clone(),
            &app_instance.web_context,
            &app_instance.adblock,
            app_instance.history.clone(),
        );

        // URL Entry Logic
        let notebook_nav = app_instance.notebook.clone();
        let webviews_nav = app_instance.webviews.clone();
        let store_nav = store.clone();
        let history_nav = app_instance.history.clone();
        app_instance.url_entry.connect_activate(move |entry| {
            let input = entry.text().to_string();
            let url = normalize_url(&input);
            history_nav.add(&url);
            
            // Update history store
            let mut found = false;
            if let Some(iter) = store_nav.iter_first() {
                loop {
                    let value = store_nav.value(&iter, 0).get::<String>().unwrap_or_default();
                    if value == url {
                        found = true;
                        break;
                    }
                    if !store_nav.iter_next(&iter) { break; }
                }
            }
            if !found {
                let iter = store_nav.append();
                store_nav.set_value(&iter, 0, &url.to_value());
            }

            if let Some(wv) = current_webview(&notebook_nav, &webviews_nav) {
                wv.load_uri(&url);
            }
        });

        // Tab switch logic
        let url_entry_switch = app_instance.url_entry.clone();
        let webviews_switch = app_instance.webviews.clone();
        app_instance.notebook.connect_switch_page(move |_, page, _| {
            let list = webviews_switch.borrow();
            if let Some((_, wv)) = list.iter().find(|(w, _)| w == page) {
                if let Some(u) = wv.uri() {
                    url_entry_switch.set_text(u.as_str());
                }
            }
        });

        // Auto-select text on focus (click)
        app_instance.url_entry.connect_button_release_event(|entry, _| {
            entry.select_region(0, -1);
            glib::Propagation::Proceed
        });

        // === Atalho Ctrl+H: abre página interna de histórico em nova aba. ===
        let nb_hist = app_instance.notebook.clone();
        let wv_hist = app_instance.webviews.clone();
        let url_hist = app_instance.url_entry.clone();
        let wc_hist = app_instance.web_context.clone();
        let ab_hist = app_instance.adblock.clone();
        let h_hist = app_instance.history.clone();
        app_instance.window.connect_key_press_event(move |_, ev| {
            let key = ev.keyval();
            let mods = ev.state();
            let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            // Ctrl+H — abre histórico.
            if ctrl && (key == gtk::gdk::keys::constants::h
                || key == gtk::gdk::keys::constants::H)
            {
                let url = crate::history_page::render_history_data_url(&h_hist);
                Self::add_tab(
                    &nb_hist, wv_hist.clone(), url_hist.clone(),
                    &wc_hist, &ab_hist, h_hist.clone(),
                );
                // Carrega a página interna na aba recém-criada.
                if let Some(wv) = current_webview(&nb_hist, &wv_hist) {
                    wv.load_uri(&url);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        app_instance
    }

    fn add_tab(
        notebook: &Notebook,
        webviews: Rc<RefCell<Vec<TabEntry>>>,
        url_entry: Entry,
        web_context: &WebContext,
        adblock: &Rc<AdBlock>,
        history: HistoryManager,
    ) {
        let container = Box::new(Orientation::Vertical, 0);
        container.show();

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

        // UserContentManager for AdBlock
        let ucm = UserContentManager::new();
        adblock.register_manager(ucm.clone());
        crate::userscripts::register_youtube_adblock(&ucm);

        // Create the WebView with SHARED context
        let webview = WebView::builder()
            .web_context(web_context)
            .user_content_manager(&ucm)
            .build();
        
        // Performance Settings (Explicitly disambiguated)
        if let Some(settings) = WebViewExt::settings(&webview) {
            settings.set_enable_smooth_scrolling(true);
            settings.set_enable_webgl(true);
            settings.set_enable_media_stream(true);
            settings.set_enable_mediasource(true);
            settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
            settings.set_enable_page_cache(true);
            settings.set_user_agent_with_application_details(Some("RustBrowser"), Some("1.0"));
        }

        container.pack_start(&webview, true, true, 0);
        container.show_all();

        let index = notebook.append_page(&container, Some(&tab_box));
        notebook.show_all();
        notebook.set_current_page(Some(index));

        // Signal: Update title
        let tab_title_clone = tab_title.clone();
        webview.connect_title_notify(move |wv| {
            if let Some(title) = wv.title() {
                tab_title_clone.set_text(title.as_str());
            }
        });

        // Signal: Update URL bar when loading finishes
        let url_entry_load = url_entry.clone();
        webview.connect_uri_notify(move |wv| {
            if let Some(u) = wv.uri() {
                url_entry_load.set_text(u.as_str());
            }
        });

        // Signal: registra histórico ao terminar de carregar (cliques em links,
        // form submits e qualquer navegação que NÃO veio do url_entry).
        let history_load = history.clone();
        webview.connect_load_changed(move |wv, event| {
            if event == LoadEvent::Finished {
                if let Some(u) = wv.uri() {
                    let s = u.as_str();
                    // Ignora URLs internas/efêmeras.
                    if s.starts_with("http://") || s.starts_with("https://") {
                        history_load.add(s);
                    }
                }
            }
        });

        // Close tab functionality
        let notebook_clone = notebook.clone();
        let container_widget: gtk::Widget = container.clone().upcast();
        let webviews_clone = webviews.clone();
        close_button.connect_clicked(move |_| {
            let index = notebook_clone.page_num(&container_widget);
            if let Some(i) = index {
                notebook_clone.remove_page(Some(i));
                webviews_clone.borrow_mut().retain(|(w, _)| w != &container_widget);
            }
        });

        webview.load_uri("https://www.google.com");
        webviews.borrow_mut().push((container.upcast(), webview));
    }

    pub fn show(&self) {
        self.window.show_all();
    }
}

fn current_webview(notebook: &Notebook, webviews: &Rc<RefCell<Vec<TabEntry>>>) -> Option<WebView> {
    let idx = notebook.current_page()?;
    let page = notebook.nth_page(Some(idx))?;
    webviews.borrow().iter().find(|(w, _)| w == &page).map(|(_, wv)| wv.clone())
}

fn normalize_url(input: &str) -> String {
    let input = input.trim();
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("localhost") || input.starts_with("webkit://") || input.starts_with("about:") {
        input.to_string()
    } else if input.contains('.') && !input.contains(' ') {
        format!("https://{}", input)
    } else {
        format!("https://www.google.com/search?q={}", input.replace(" ", "+"))
    }
}
