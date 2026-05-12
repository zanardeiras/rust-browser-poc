//! AdBlock nativo via WebKitUserContentFilterStore.
//!
//! ## Camadas
//!
//! 1. **Bundled** — `assets/adblock-rules.json` (embarcado no binário).
//! 2. **EasyList + EasyPrivacy** — baixados em background, parseados via
//!    [`crate::easylist`], escritos em `~/.cache/.../easylist-cache/combined-rules.json`.
//! 3. **Recompilação dinâmica** — quando o background termina, fazemos um novo
//!    `save_async` e o filter resultante substitui o anterior em todos os tabs.

use std::cell::RefCell;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;

use glib::translate::ToGlibPtr;
use webkit2gtk::UserContentManager;
use webkit2gtk_sys as ffi;

use crate::easylist;

const BUNDLED_RULES_JSON: &str = include_str!("../assets/adblock-rules.json");
const FILTER_ID_BUNDLED: &str = "rust-browser-poc-adblock-bundled-v1";
const FILTER_ID_FULL: &str = "rust-browser-poc-adblock-full-v1";

pub struct AdBlock {
    store: *mut ffi::WebKitUserContentFilterStore,
    filter: RefCell<Option<*mut ffi::WebKitUserContentFilter>>,
    active_filter_id: RefCell<String>,
    managers: RefCell<Vec<UserContentManager>>,
    enabled: RefCell<bool>,
    state_file: PathBuf,
    cache_dir: PathBuf,
}

impl AdBlock {
    pub fn new(base_dir: &PathBuf) -> Rc<Self> {
        let store_dir = base_dir.join("adblock-store");
        let _ = std::fs::create_dir_all(&store_dir);
        let path_c = CString::new(store_dir.to_string_lossy().as_bytes()).unwrap();
        let store = unsafe { ffi::webkit_user_content_filter_store_new(path_c.as_ptr()) };

        let state_file = base_dir.join("adblock-enabled");
        let enabled = std::fs::read_to_string(&state_file)
            .map(|s| s.trim() == "1")
            .unwrap_or(true);

        let me = Rc::new(Self {
            store,
            filter: RefCell::new(None),
            active_filter_id: RefCell::new(String::new()),
            managers: RefCell::new(Vec::new()),
            enabled: RefCell::new(enabled),
            state_file,
            cache_dir: base_dir.clone(),
        });

        let combined_path = easylist::combined_rules_path(base_dir);
        if easylist::cache_fresh(base_dir) {
            if let Ok(json) = std::fs::read_to_string(&combined_path) {
                eprintln!("[adblock] using cached combined ruleset ({} bytes)", json.len());
                Self::compile_async(me.clone(), json, FILTER_ID_FULL);
                return me;
            }
        }
        // Fallback: bundled imediato + refresh em background.
        Self::compile_async(me.clone(), BUNDLED_RULES_JSON.to_string(), FILTER_ID_BUNDLED);
        Self::kick_off_background_refresh(me.clone());
        me
    }

    fn kick_off_background_refresh(this: Rc<Self>) {
        // glib::MainContext::channel está marcado deprecated em favor de
        // async-channel + spawn_future_local, mas a alternativa exige
        // `Send` no receiver (i.e. trocar Rc<AdBlock> por Arc<AdBlock>,
        // cascateando refactor em toda a app sem ganho funcional).
        // O channel síncrono ainda é o caminho idiomático para "thread → main loop".
        #[allow(deprecated)]
        let (tx, rx) = glib::MainContext::channel::<bool>(glib::Priority::DEFAULT_IDLE);
        let cache_dir = this.cache_dir.clone();
        easylist::refresh_in_background(
            cache_dir,
            BUNDLED_RULES_JSON.to_string(),
            tx,
        );
        let this_for_rx = this.clone();
        rx.attach(None, move |ok| {
            if ok {
                let path = easylist::combined_rules_path(&this_for_rx.cache_dir);
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        eprintln!("[adblock] recompiling with full ruleset ({} bytes)...", json.len());
                        Self::compile_async(this_for_rx.clone(), json, FILTER_ID_FULL);
                    }
                    Err(e) => eprintln!("[adblock] read combined json failed: {}", e),
                }
            }
            glib::ControlFlow::Break
        });
    }

    fn compile_async(this: Rc<Self>, json: String, filter_id: &'static str) {
        // g_bytes_new faz COPY do buffer — seguro com Strings runtime.
        let bytes = unsafe {
            glib::ffi::g_bytes_new(
                json.as_ptr() as *const _,
                json.len(),
            )
        };
        let id_c = CString::new(filter_id).unwrap();
        let store_ptr = this.store;

        let ctx = Box::new(CallbackCtx { this, filter_id });
        let user_data = Box::into_raw(ctx) as glib::ffi::gpointer;

        unsafe {
            ffi::webkit_user_content_filter_store_save(
                store_ptr,
                id_c.as_ptr(),
                bytes,
                ptr::null_mut(),
                Some(save_finished_trampoline),
                user_data,
            );
            glib::ffi::g_bytes_unref(bytes);
        }
    }

    pub fn enabled(&self) -> bool {
        *self.enabled.borrow()
    }

    pub fn register_manager(&self, ucm: UserContentManager) {
        if *self.enabled.borrow() {
            if let Some(filter) = *self.filter.borrow() {
                unsafe {
                    ffi::webkit_user_content_manager_add_filter(
                        ucm.to_glib_none().0,
                        filter,
                    );
                }
            }
        }
        self.managers.borrow_mut().push(ucm);
    }

    pub fn set_enabled(&self, on: bool) {
        *self.enabled.borrow_mut() = on;
        let _ = std::fs::write(&self.state_file, if on { "1" } else { "0" });
        self.apply_to_all();
    }

    fn apply_to_all(&self) {
        let on = *self.enabled.borrow();
        let filter_opt = *self.filter.borrow();
        let id_str = self.active_filter_id.borrow().clone();
        if id_str.is_empty() { return; }
        let id_c = CString::new(id_str).unwrap();
        for ucm in self.managers.borrow().iter() {
            unsafe {
                ffi::webkit_user_content_manager_remove_filter_by_id(
                    ucm.to_glib_none().0,
                    id_c.as_ptr(),
                );
                if on {
                    if let Some(filter) = filter_opt {
                        ffi::webkit_user_content_manager_add_filter(
                            ucm.to_glib_none().0,
                            filter,
                        );
                    }
                }
            }
        }
    }

    fn install_new_filter(&self, new_filter: *mut ffi::WebKitUserContentFilter, new_id: &str) {
        let old_id = self.active_filter_id.borrow().clone();
        if !old_id.is_empty() {
            let id_c = CString::new(old_id).unwrap();
            for ucm in self.managers.borrow().iter() {
                unsafe {
                    ffi::webkit_user_content_manager_remove_filter_by_id(
                        ucm.to_glib_none().0,
                        id_c.as_ptr(),
                    );
                }
            }
        }
        if let Some(f) = self.filter.borrow_mut().take() {
            unsafe { ffi::webkit_user_content_filter_unref(f); }
        }
        *self.filter.borrow_mut() = Some(new_filter);
        *self.active_filter_id.borrow_mut() = new_id.to_string();
        self.apply_to_all();
    }
}

impl Drop for AdBlock {
    fn drop(&mut self) {
        unsafe {
            if let Some(f) = *self.filter.borrow() {
                ffi::webkit_user_content_filter_unref(f);
            }
            if !self.store.is_null() {
                glib::gobject_ffi::g_object_unref(self.store as *mut _);
            }
        }
    }
}

struct CallbackCtx {
    this: Rc<AdBlock>,
    filter_id: &'static str,
}

unsafe extern "C" fn save_finished_trampoline(
    source: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) {
    let ctx: Box<CallbackCtx> = Box::from_raw(user_data as *mut CallbackCtx);
    let store = source as *mut ffi::WebKitUserContentFilterStore;

    let mut error: *mut glib::ffi::GError = ptr::null_mut();
    let filter = ffi::webkit_user_content_filter_store_save_finish(store, result, &mut error);

    if !error.is_null() {
        let msg_ptr = (*error).message;
        let msg = if msg_ptr.is_null() {
            "<no message>".into()
        } else {
            std::ffi::CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
        };
        eprintln!("[adblock] failed to compile filter list ({}): {}", ctx.filter_id, msg);
        glib::ffi::g_error_free(error);
        return;
    }
    if filter.is_null() {
        eprintln!("[adblock] save_finish returned NULL without error");
        return;
    }

    eprintln!(
        "[adblock] filter '{}' compiled OK; applying to {} tab(s)",
        ctx.filter_id,
        ctx.this.managers.borrow().len()
    );
    ctx.this.install_new_filter(filter, ctx.filter_id);
}
