use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, HeaderBar, Entry,
    Notebook, Button, Label, Box, Orientation, EntryCompletion, ListStore, Image,
};
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;
use webkit2gtk::{
    WebView, WebContext, WebContextExt, WebViewExt, SettingsExt,
    HardwareAccelerationPolicy, CacheModel, WebsiteDataManager, UserContentManager,
    CookieManagerExt, CookiePersistentStorage, CookieAcceptPolicy,
    WebsiteDataManagerExt, LoadEvent, NetworkError,
};
use crate::history::HistoryManager;
use crate::adblock::AdBlock;
use crate::bookmarks::BookmarksStore;
use crate::bookmarks_bar::BookmarksBar;
use crate::settings::Settings;
use crate::password_manager::PasswordStore;

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
    pub bookmarks: Rc<BookmarksStore>,
    #[allow(dead_code)]
    pub passwords: Rc<RefCell<PasswordStore>>,
}

impl BrowserApp {
    pub fn new(app: &Application) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Rust Browser POC")
            .default_width(1024)
            .default_height(768)
            .build();

        // Carregar ícone nativo. Importante: escalar pra um tamanho sensato
        // (máx 256px no maior lado) ANTES de set_icon, senão no backend
        // Wayland o GDK tenta criar uma cairo image surface no tamanho
        // original do PNG. Se o PNG for grande (ex.: 1610x2400), o
        // compositor Wayland rejeita com "invalid value (typically too big)
        // for the size of the input" — crashando o app no startup.
        if let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_file("icon.png") {
            let (w, h) = (pixbuf.width(), pixbuf.height());
            let max_side = w.max(h);
            let icon = if max_side > 256 {
                let scale = 256.0 / max_side as f64;
                let nw = (w as f64 * scale).round() as i32;
                let nh = (h as f64 * scale).round() as i32;
                pixbuf
                    .scale_simple(nw.max(1), nh.max(1), gtk::gdk_pixbuf::InterpType::Bilinear)
                    .unwrap_or(pixbuf)
            } else {
                pixbuf
            };
            window.set_icon(Some(&icon));
        }

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

        // Password Manager Button
        let pw_button = Button::from_icon_name(Some("dialog-password-symbolic"), gtk::IconSize::Button);
        pw_button.set_tooltip_text(Some("Gerenciador de Senhas (Ctrl+P)"));
        header_bar.pack_end(&pw_button);

        // Star Button (favoritar página atual) — fica ao lado de new_tab,
        // FORA da URL bar. Dourado quando favoritado.
        let star_button = Button::from_icon_name(
            Some("non-starred-symbolic"),
            gtk::IconSize::Button,
        );
        star_button.set_tooltip_text(Some("Favoritar página"));
        star_button.set_relief(gtk::ReliefStyle::None);
        header_bar.pack_end(&star_button);

        window.set_titlebar(Some(&header_bar));

        let notebook = Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_show_tabs(true);

        // Bookmarks store + bar (a bar entra abaixo da URL bar, acima do notebook).
        let bookmarks = BookmarksStore::new();
        let settings = Settings::new();
        let bookmarks_bar = BookmarksBar::new(bookmarks.clone(), settings.clone());

        // Container vertical: [bookmarks_bar] + [notebook].
        let main_vbox = Box::new(Orientation::Vertical, 0);
        main_vbox.pack_start(&bookmarks_bar.widget, false, false, 0);
        main_vbox.pack_start(&notebook, true, true, 0);
        window.add(&main_vbox);

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

        // === Autocomplete inteligente (substring + inline ghost text) ===
        //
        // Decisões de design:
        //  - Guardamos no ListStore a URL "display" SEM esquema/`www.` (ex.:
        //    "youtube.com/watch?v=..." em vez de "https://www.youtube.com/..").
        //    Isso permite que o inline-completion nativo do GTK (que casa por
        //    prefixo) funcione: digitar "you" preenche "youtube.com" inline.
        //  - Um `match_func` customizado faz matching por SUBSTRING case-
        //    insensitive na display string — então digitar "tube" também faz
        //    aparecer "youtube.com" no popup, igual Chrome/Firefox fazem.
        //  - O store é semeado com domínios populares para que o autocomplete
        //    funcione já no primeiro uso (antes de existir histórico).
        let completion = EntryCompletion::new();
        let store = ListStore::new(&[glib::Type::STRING]);

        // Dedup helper local: insere uma display string se ainda não existir.
        let insert_unique = |s: &str| {
            // Varre store linearmente; ok para tamanhos típicos de histórico.
            if let Some(iter) = store.iter_first() {
                loop {
                    let v = store.value(&iter, 0).get::<String>().unwrap_or_default();
                    if v == s { return; }
                    if !store.iter_next(&iter) { break; }
                }
            }
            let it = store.append();
            store.set_value(&it, 0, &s.to_value());
        };

        // Seeds: domínios populares para autocomplete imediato.
        for seed in [
            "youtube.com", "google.com", "github.com", "gmail.com",
            "stackoverflow.com", "reddit.com", "wikipedia.org", "twitter.com",
            "x.com", "facebook.com", "instagram.com", "linkedin.com",
            "amazon.com", "amazon.com.br", "netflix.com", "twitch.tv",
            "chatgpt.com", "claude.ai", "duckduckgo.com",
        ] {
            insert_unique(seed);
        }

        // Histórico: insere versão "limpa" de cada URL.
        for item in history.load() {
            insert_unique(&strip_url_for_display(&item));
        }

        completion.set_model(Some(&store));
        completion.set_text_column(0);
        completion.set_minimum_key_length(1);
        completion.set_popup_completion(true);
        completion.set_inline_completion(true);
        completion.set_inline_selection(false);
        completion.set_popup_single_match(true);

        // Matching por substring (case-insensitive), igual barra de endereço
        // de navegadores modernos. Retorna true se o que o usuário digitou
        // aparece em qualquer posição da display string.
        completion.set_match_func(|_c, key, iter| {
            // O modelo do completion é o ListStore que setamos acima.
            let model = match _c.model() { Some(m) => m, None => return false };
            let val: String = model
                .value(iter, 0)
                .get::<String>()
                .unwrap_or_default();
            if key.is_empty() { return false; }
            val.to_ascii_lowercase().contains(&key.to_ascii_lowercase())
        });

        // Quando o usuário usar as setas do teclado e apertar ENTER no
        // autocomplete (ou a lista fechar selecionando algo auto), preenchemos
        // a entry e já acionamos a navegação na hora, sem precisar de outro Enter!
        let completion_url_entry = url_entry.clone();
        completion.connect_match_selected(move |_completion, model, iter| {
            if let Ok(value) = model.value(iter, 0).get::<String>() {
                completion_url_entry.set_text(&value);
                completion_url_entry.emit_activate();
            }
            glib::Propagation::Stop
        });

        url_entry.set_completion(Some(&completion));

        // Inicializa PasswordStore apontando para o mesmo data_dir do browser.
        let passwords = PasswordStore::new(&data_dir);

        let app_instance = Self {
            window,
            notebook,
            url_entry,
            webviews,
            web_context,
            history,
            adblock,
            bookmarks: bookmarks.clone(),
            passwords: passwords.clone(),
        };

        // === Estrelinha externa: click toggla bookmark da página ativa ===
        let notebook_star = app_instance.notebook.clone();
        let webviews_star = app_instance.webviews.clone();
        let bookmarks_star = bookmarks.clone();
        let star_button_click = star_button.clone();
        star_button.connect_clicked(move |_| {
            if let Some(wv) = current_webview(&notebook_star, &webviews_star) {
                let url = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                if url.is_empty() || url.starts_with("data:") { return; }
                let title = wv.title().map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| url.clone());
                let is_now = bookmarks_star.toggle_url(&url, &title);
                let icon = if is_now { "starred-symbolic" } else { "non-starred-symbolic" };
                star_button_click.set_image(Some(&Image::from_icon_name(
                    Some(icon), gtk::IconSize::Button,
                )));
            }
        });

        // Closure compartilhada para atualizar visual da estrela conforme URL.
        // Usada por: switch_page, on_change do store, e uri_notify de cada webview.
        let star_button_upd = star_button.clone();
        let bookmarks_upd = bookmarks.clone();
        let update_star: Rc<dyn Fn(&str)> = Rc::new(move |url: &str| {
            let icon = if bookmarks_upd.is_bookmarked(url) {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            };
            star_button_upd.set_image(Some(&Image::from_icon_name(
                Some(icon), gtk::IconSize::Button,
            )));
        });

        // === Wire bookmarks_bar callbacks ===
        let notebook_bm = app_instance.notebook.clone();
        let webviews_bm = app_instance.webviews.clone();
        bookmarks_bar.set_on_navigate(move |url| {
            if let Some(wv) = current_webview(&notebook_bm, &webviews_bm) {
                wv.load_uri(url);
            }
        });

        let bookmarks_for_mgr = bookmarks.clone();
        let window_for_mgr = app_instance.window.clone();
        bookmarks_bar.set_on_manage(move || {
            crate::bookmarks_manager::open_manager(&bookmarks_for_mgr, Some(&window_for_mgr.clone().upcast::<gtk::Window>()));
        });

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
        let update_star_newtab = update_star.clone();
        new_tab_button.connect_clicked(move |_| {
            Self::add_tab(&notebook_clone, webviews_clone.clone(), url_entry_clone.clone(), &wc_clone, &adblock_clone, history_for_new_tab.clone(), update_star_newtab.clone());
        });

        // Initial tab
        Self::add_tab(
            &app_instance.notebook,
            app_instance.webviews.clone(),
            app_instance.url_entry.clone(),
            &app_instance.web_context,
            &app_instance.adblock,
            app_instance.history.clone(),
            update_star.clone(),
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

            // Atualiza store do autocomplete usando a versão "display"
            // (sem https://www.) para casar com o esquema de inline-completion.
            let display = strip_url_for_display(&url);
            let mut found = false;
            if let Some(iter) = store_nav.iter_first() {
                loop {
                    let value = store_nav.value(&iter, 0).get::<String>().unwrap_or_default();
                    if value == display {
                        found = true;
                        break;
                    }
                    if !store_nav.iter_next(&iter) { break; }
                }
            }
            if !found {
                let iter = store_nav.append();
                store_nav.set_value(&iter, 0, &display.to_value());
            }

            if let Some(wv) = current_webview(&notebook_nav, &webviews_nav) {
                wv.load_uri(&url);
                // Move o foco para o WebView imediatamente: assim o url_entry
                // perde o foco antes dos eventos assíncronos de uri-notify
                // dispararem, permitindo que a barra seja atualizada corretamente.
                wv.grab_focus();
            }
        });

        // Tab switch logic
        let url_entry_switch = app_instance.url_entry.clone();
        let webviews_switch = app_instance.webviews.clone();
        let update_star_switch = update_star.clone();
        app_instance.notebook.connect_switch_page(move |_, page, _| {
            let list = webviews_switch.borrow();
            if let Some((_, wv)) = list.iter().find(|(w, _)| w == page) {
                let u = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                url_entry_switch.set_text(&u);
                update_star_switch(&u);
            }
        });

        // === Atualiza estrelinha quando store muda (toggle, manager). ===
        let nb_star_uri = app_instance.notebook.clone();
        let webviews_star_uri = app_instance.webviews.clone();
        let update_star_change = update_star.clone();
        app_instance.bookmarks.on_change(move || {
            if let Some(wv) = current_webview(&nb_star_uri, &webviews_star_uri) {
                let u = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                update_star_change(&u);
            }
        });

        // Auto-select text on focus (click)
        app_instance.url_entry.connect_button_release_event(|entry, _| {
            entry.select_region(0, -1);
            glib::Propagation::Proceed
        });

        // === Botão de gerenciador de senhas ===
        {
            let pw_store = passwords.clone();
            let nb_pw = app_instance.notebook.clone();
            let wv_pw = app_instance.webviews.clone();
            let win_pw = app_instance.window.clone();
            pw_button.connect_clicked(move |_| {
                open_password_manager(&pw_store, &nb_pw, &wv_pw, &win_pw);
            });
        }

        // === Atalho Ctrl+H: abre página interna de histórico em nova aba. ===
        let nb_hist = app_instance.notebook.clone();
        let wv_hist = app_instance.webviews.clone();
        let url_hist = app_instance.url_entry.clone();
        let wc_hist = app_instance.web_context.clone();
        let ab_hist = app_instance.adblock.clone();
        let h_hist = app_instance.history.clone();
        let update_star_hist = update_star.clone();
        let bookmarks_bar_kb = bookmarks_bar.clone();
        let pw_store_kb = passwords.clone();
        let nb_pw_kb = app_instance.notebook.clone();
        let wv_pw_kb = app_instance.webviews.clone();
        app_instance.window.connect_key_press_event(move |win, ev| {
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
                    update_star_hist.clone(),
                );
                // Carrega a página interna na aba recém-criada.
                if let Some(wv) = current_webview(&nb_hist, &wv_hist) {
                    wv.load_uri(&url);
                }
                return glib::Propagation::Stop;
            }
            // Ctrl+B — toggle da barra de favoritos.
            if ctrl && (key == gtk::gdk::keys::constants::b
                || key == gtk::gdk::keys::constants::B)
            {
                bookmarks_bar_kb.toggle_visible_persisted();
                return glib::Propagation::Stop;
            }
            // Ctrl+J — força a barra de favoritos visível (não esconde).
            if ctrl && (key == gtk::gdk::keys::constants::j
                || key == gtk::gdk::keys::constants::J)
            {
                bookmarks_bar_kb.set_visible_persisted(true);
                return glib::Propagation::Stop;
            }
            // Ctrl+P — abre gerenciador de senhas.
            if ctrl && (key == gtk::gdk::keys::constants::p
                || key == gtk::gdk::keys::constants::P)
            {
                open_password_manager(&pw_store_kb, &nb_pw_kb, &wv_pw_kb, win);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        // Quando a janela GTK volta a receber foco (usuário trocou de janela
        // e voltou), força um redraw da webview atual. Isso resolve o caso
        // da HUD do player YT ficar "stale" porque o WebKit suspende o paint
        // pipeline quando a window perde foco — sem isso, o vídeo continua
        // (GStreamer), mas a HUD/controles ficam congelados até um mousemove.
        // Também aciona o repaint dos <video> pausados (seek-to-self) via
        // helper JS injetado pelo userscript `register_background_awake`.
        let wv_focus = app_instance.webviews.clone();
        let nb_focus = app_instance.notebook.clone();
        app_instance.window.connect_focus_in_event(move |_, _| {
            if let Some(wv) = current_webview(&nb_focus, &wv_focus) {
                wv.queue_draw();
                #[allow(deprecated)]
                wv.run_javascript(
                    "if (window.__rbpoc_repaint_videos) window.__rbpoc_repaint_videos();",
                    None::<&gio::Cancellable>,
                    |_| {},
                );
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
        update_star: Rc<dyn Fn(&str)>,
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
        // Impede que as abas durmam no background e travem a HUD do player
        crate::userscripts::register_background_awake(&ucm);

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

        // Signal: Update favicon da aba. WebKit baixa o favicon automaticamente
        // (porque setamos `set_favicon_database_directory`), e dispara
        // `notify::favicon` quando a `cairo::Surface` está disponível.
        // Convertimos a surface → Pixbuf escalado pra 16x16 e plotamos no Image
        // que já está no tab_box. Sem favicon, mantém o ícone genérico.
        let tab_icon_clone = tab_icon.clone();
        webview.connect_favicon_notify(move |wv| {
            if let Some(surface) = wv.favicon() {
                // ImageSurface tem width/height; cast indireto via cairo::Surface.
                // Usamos `pixbuf_get_from_surface` que aceita qualquer Surface.
                let w = surface_size(&surface).0;
                let h = surface_size(&surface).1;
                // Rejeita dimensões inválidas: zero, negativas ou grandes demais.
                // Cairo lança CAIRO_STATUS_INVALID_SIZE acima de ~32767, mas
                // favicons corrompidos podem retornar valores ainda maiores,
                // causando o crash "invalid value (too big) for the size of the input".
                if w <= 0 || h <= 0 || w > 512 || h > 512 { return; }
                if let Some(pb) = gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, w, h) {
                    // Escala pra 16x16 (tamanho padrão de favicon em aba).
                    if let Some(scaled) = pb.scale_simple(
                        16, 16, gtk::gdk_pixbuf::InterpType::Bilinear,
                    ) {
                        tab_icon_clone.set_from_pixbuf(Some(&scaled));
                    } else {
                        tab_icon_clone.set_from_pixbuf(Some(&pb));
                    }
                }
            } else {
                // Reseta para genérico quando não há favicon (ex.: chrome://).
                tab_icon_clone.set_from_icon_name(
                    Some("browser-symbolic"), gtk::IconSize::Menu,
                );
            }
        });

        // Signal: Update URL bar when loading finishes
        let url_entry_load = url_entry.clone();
        let update_star_load = update_star.clone();
        webview.connect_uri_notify(move |wv| {
            if let Some(u) = wv.uri() {
                let s = u.as_str();
                // Não atualiza a barra de URL enquanto ela tem foco.
                // Evita o ciclo síncrono: activate → load_uri → uri-notify
                // → set_text (com EntryCompletion ativo) → activate novamente,
                // o que cancela a primeira navegação com "operation was cancelled".
                if !url_entry_load.has_focus() {
                    url_entry_load.set_text(s);
                }
                // Reseta visual da estrela conforme nova URL.
                update_star_load(s);
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

        // Suprime a tela de erro nativa branca ("operation was cancelled")
        // caso o carregamento tenha sido cancelado (ex: o adblock em
        // background terminou de recompilar e atualizou o content manager,
        // cancelando implicitamente a rede do WebKit no momento exato,
        // ou você apertou outro link no meio do caminho).
        webview.connect_load_failed(move |_wv, _event, _uri, error| {
            if error.matches(NetworkError::Cancelled) {
                // Return `true` ignora o display da página de erro default branca.
                // Isso deixa a URL anterior desenhada no browser e ele logo sai do estado branco.
                return true;
            }
            false
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

/// Abre o gerenciador de senhas, passando um callback que injeta as credenciais
/// na WebView ativa via JavaScript.
fn open_password_manager(
    pw_store: &Rc<RefCell<PasswordStore>>,
    notebook: &Notebook,
    webviews: &Rc<RefCell<Vec<TabEntry>>>,
    window: &ApplicationWindow,
) {
    let current_url = current_webview(notebook, webviews)
        .and_then(|wv| wv.uri().map(|u| u.to_string()));

    // Callback de preenchimento: injeta username + password via JS na WebView ativa.
    let wv_fill = current_webview(notebook, webviews);
    let fill_callback: Option<Rc<dyn Fn(String, String)>> = wv_fill.map(|wv| {
        let wv = wv.clone();
        let cb: Rc<dyn Fn(String, String)> = Rc::new(move |user: String, pass: String| {
            // Escapa para uso dentro de strings JS (aspas simples → \').
            let user_esc = user.replace('\\', "\\\\").replace('\'', "\\'");
            let pass_esc = pass.replace('\\', "\\\\").replace('\'', "\\'");
            let js = format!(
                r#"(function(u, p) {{
                    var pwdFields = document.querySelectorAll('input[type="password"]');
                    var userFields = document.querySelectorAll(
                        'input[type="text"], input[type="email"], ' +
                        'input[name*="user"], input[name*="email"], ' +
                        'input[id*="user"], input[id*="email"], ' +
                        'input[autocomplete="username"], input[autocomplete="email"]'
                    );
                    if (pwdFields.length > 0) {{
                        pwdFields[0].value = p;
                        pwdFields[0].dispatchEvent(new Event('input', {{bubbles:true}}));
                        pwdFields[0].dispatchEvent(new Event('change', {{bubbles:true}}));
                    }}
                    if (userFields.length > 0) {{
                        userFields[0].value = u;
                        userFields[0].dispatchEvent(new Event('input', {{bubbles:true}}));
                        userFields[0].dispatchEvent(new Event('change', {{bubbles:true}}));
                    }}
                }})('{}', '{}')"#,
                user_esc, pass_esc
            );
            #[allow(deprecated)]
            wv.run_javascript(&js, None::<&gio::Cancellable>, |_| {});
        });
        cb
    });

    let win_upcast: gtk::Window = window.clone().upcast();
    crate::password_manager::open_manager(
        pw_store.clone(),
        current_url,
        fill_callback,
        Some(&win_upcast),
    );
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

/// Remove esquema (`http://`, `https://`) e prefixo `www.` para gerar a
/// "display string" usada no autocomplete. Mantém path/query/fragment.
/// Ex.: `https://www.youtube.com/watch?v=ID` → `youtube.com/watch?v=ID`.
/// URLs não-http (data:, about:, webkit://) são retornadas inalteradas.
fn strip_url_for_display(url: &str) -> String {
    let s = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        return url.to_string();
    };
    s.strip_prefix("www.").unwrap_or(s).to_string()
}

/// Lê width/height de uma `cairo::Surface` (preferencialmente ImageSurface).
/// Necessário porque `pixbuf_get_from_surface` exige dimensões explícitas.
fn surface_size(surface: &gtk::cairo::Surface) -> (i32, i32) {
    // Tenta cast para ImageSurface (caminho rápido — todo favicon WebKit é raster).
    if let Ok(img) = surface.clone().try_into() as Result<gtk::cairo::ImageSurface, _> {
        let (w, h) = (img.width(), img.height());
        // Retorna (0,0) para sinalizar surface inválida; o caller descarta via bounds check.
        if w > 0 && h > 0 && w <= 512 && h <= 512 {
            return (w, h);
        }
        return (0, 0);
    }
    // Fallback defensivo.
    (16, 16)
}
