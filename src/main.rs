use gtk::prelude::*;
use gtk::Application;
use std::rc::Rc;
use std::cell::RefCell;

mod browser_app;
mod history;
mod history_page;
mod adblock;
mod easylist;
mod userscripts;
mod bookmarks;
mod bookmarks_bar;
mod bookmarks_manager;
use browser_app::BrowserApp;

fn main() {
    // === GPU / Driver ===
    std::env::set_var("__NV_PRIME_RENDER_OFFLOAD", "1");
    std::env::set_var("__GLX_VENDOR_LIBRARY_NAME", "nvidia");

    // === WebKit Compositing ===
    std::env::set_var("WEBKIT_FORCE_COMPOSITING_MODE", "1");
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "0");
    std::env::set_var("WEBKIT_USE_GLIB_EVENT_LOOP", "0");

    // Aumenta a prioridade do processo
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, -10);
    }

    let app = Application::builder()
        .application_id("org.example.rust_browser_poc")
        .build();

    // Mantemos uma referência forte para o BrowserApp para ele não ser dropado
    let app_state: Rc<RefCell<Option<BrowserApp>>> = Rc::new(RefCell::new(None));

    let state_clone = app_state.clone();
    app.connect_activate(move |app| {
        let browser_app = BrowserApp::new(app);
        browser_app.show();
        *state_clone.borrow_mut() = Some(browser_app);
    });

    app.run();
}
