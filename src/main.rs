use gtk::prelude::*;
use gtk::Application;

mod browser_app;
mod history;
mod adblock;
mod easylist;
mod userscripts;
use browser_app::BrowserApp;

fn main() {
    // === GPU / Driver ===
    // Offload para NVIDIA quando disponível (Optimus). Em GPU única, é inofensivo.
    std::env::set_var("__NV_PRIME_RENDER_OFFLOAD", "1");
    std::env::set_var("__GLX_VENDOR_LIBRARY_NAME", "nvidia");

    // === WebKit Compositing ===
    // Força compositing acelerado para TODO o conteúdo (não só camadas que pedem).
    // Isso garante que paint/scroll passem pela GPU em vez do raster CPU.
    std::env::set_var("WEBKIT_FORCE_COMPOSITING_MODE", "1");
    // Habilitamos o renderer DMA-BUF (mais rápido que o caminho legacy via shm)
    // em sistemas com Mesa/NVIDIA recentes. Setando explicitamente "0" para NÃO desabilitar.
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "0");

    // === Event loop ===
    // GLib event loop é o caminho otimizado no GTK; "0" mantém o default já bom.
    std::env::set_var("WEBKIT_USE_GLIB_EVENT_LOOP", "0");

    // Aumenta a prioridade do processo (renice) — UI mais responsiva sob carga.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, -10);
    }

    let app = Application::builder()
        .application_id("org.example.rust_browser_poc")
        .build();

    app.connect_activate(|app| {
        let browser_app = BrowserApp::new(app);
        browser_app.show();
    });

    app.run();
}
