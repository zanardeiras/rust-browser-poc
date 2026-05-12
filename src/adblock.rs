//! AdBlock nativo via WebKitUserContentFilterStore.
//!
//! O WebKit possui um motor de Content Blockers escrito em C++ (o mesmo usado
//! pelo Safari). Aqui compilamos uma lista JSON em bytecode otimizado, salvamos
//! em disco (cache de compilação) e injetamos um `UserContentFilter` em cada
//! `UserContentManager` das abas. Toggle on/off é instantâneo em runtime.

use std::cell::RefCell;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;

use glib::translate::ToGlibPtr;
use webkit2gtk::UserContentManager;
use webkit2gtk_sys as ffi;

/// Lista de regras embarcada no binário (formato WebKit Content Blockers JSON).
const RULES_JSON: &str = include_str!("../assets/adblock-rules.json");
const FILTER_ID: &str = "rust-browser-poc-adblock-v1";

pub struct AdBlock {
    store: *mut ffi::WebKitUserContentFilterStore,
    /// Filtro compilado. None enquanto a compilação async não terminou.
    filter: RefCell<Option<*mut ffi::WebKitUserContentFilter>>,
    /// Managers de cada aba viva — para aplicar/remover globalmente no toggle.
    managers: RefCell<Vec<UserContentManager>>,
    enabled: RefCell<bool>,
    state_file: PathBuf,
}

impl AdBlock {
    /// Cria o store em `<base>/adblock-store/` e dispara compilação assíncrona.
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
            managers: RefCell::new(Vec::new()),
            enabled: RefCell::new(enabled),
            state_file,
        });

        Self::compile_async(me.clone());
        me
    }

    fn compile_async(this: Rc<Self>) {
        // GBytes referenciando a string estática (zero-copy: ponteiro para .rodata).
        let bytes = unsafe {
            glib::ffi::g_bytes_new_static(
                RULES_JSON.as_ptr() as *const _,
                RULES_JSON.len(),
            )
        };
        let id_c = CString::new(FILTER_ID).unwrap();
        let store_ptr = this.store;

        // Passa Rc<AdBlock> como user_data via Box raw pointer.
        let user_data = Box::into_raw(Box::new(this)) as glib::ffi::gpointer;

        unsafe {
            ffi::webkit_user_content_filter_store_save(
                store_ptr,
                id_c.as_ptr(),
                bytes,
                ptr::null_mut(),
                Some(save_finished_trampoline),
                user_data,
            );
            // O store_save retém o GBytes; podemos liberar nosso ref aqui.
            glib::ffi::g_bytes_unref(bytes);
        }
    }

    pub fn enabled(&self) -> bool {
        *self.enabled.borrow()
    }

    /// Registra um `UserContentManager` de uma aba recém-criada.
    /// Se o filter já estiver compilado e adblock ativo, aplica imediatamente.
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

    /// Liga/desliga o adblock em todos os managers registrados.
    pub fn set_enabled(&self, on: bool) {
        *self.enabled.borrow_mut() = on;
        let _ = std::fs::write(&self.state_file, if on { "1" } else { "0" });
        self.apply_to_all();
    }

    /// Aplica o estado atual (filter + enabled) a todos os managers.
    fn apply_to_all(&self) {
        let on = *self.enabled.borrow();
        let filter_opt = *self.filter.borrow();
        let id_c = CString::new(FILTER_ID).unwrap();
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

/// Callback chamada pelo gio quando `store_save` termina (sucesso ou erro).
unsafe extern "C" fn save_finished_trampoline(
    source: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) {
    // Recupera o Rc<AdBlock> (assume ownership de volta).
    let this: Box<Rc<AdBlock>> = Box::from_raw(user_data as *mut Rc<AdBlock>);
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
        eprintln!("[adblock] failed to compile filter list: {}", msg);
        glib::ffi::g_error_free(error);
        return;
    }
    if filter.is_null() {
        eprintln!("[adblock] save_finish returned NULL without error");
        return;
    }

    *this.filter.borrow_mut() = Some(filter);
    eprintln!(
        "[adblock] filter compiled OK; applying to {} tab(s)",
        this.managers.borrow().len()
    );
    this.apply_to_all();
    // Box é dropado aqui: libera o Rc clone que detinha a callback.
    drop(this);
}
