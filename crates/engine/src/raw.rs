//! 对应已锁定的 librime 1.17.0 rime_api.h（RIME Developers，BSD License）。
//! 仅声明截至 get_version 的稳定前缀；不调用的槽位仍保留原名和顺序。

use std::ffi::{c_char, c_int, c_void};

pub type Bool = c_int;
pub type SessionId = usize;
type Unused = Option<unsafe extern "C" fn()>;

#[repr(C)]
#[derive(Default)]
pub struct Traits {
    pub data_size: c_int,
    pub shared_data_dir: *const c_char,
    pub user_data_dir: *const c_char,
    pub distribution_name: *const c_char,
    pub distribution_code_name: *const c_char,
    pub distribution_version: *const c_char,
    pub app_name: *const c_char,
    pub modules: *const *const c_char,
    pub min_log_level: c_int,
    pub log_dir: *const c_char,
    pub prebuilt_data_dir: *const c_char,
    pub staging_dir: *const c_char,
}

#[repr(C)]
#[derive(Default)]
pub struct Composition {
    pub length: c_int,
    pub cursor_pos: c_int,
    pub sel_start: c_int,
    pub sel_end: c_int,
    pub preedit: *const c_char,
}

#[repr(C)]
#[derive(Default)]
pub struct Candidate {
    pub text: *const c_char,
    pub comment: *const c_char,
    pub reserved: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
pub struct Menu {
    pub page_size: c_int,
    pub page_no: c_int,
    pub is_last_page: Bool,
    pub highlighted_candidate_index: c_int,
    pub num_candidates: c_int,
    pub candidates: *const Candidate,
    pub select_keys: *const c_char,
}

#[repr(C)]
#[derive(Default)]
pub struct Context {
    pub data_size: c_int,
    pub composition: Composition,
    pub menu: Menu,
    pub commit_text_preview: *const c_char,
    pub select_labels: *const *const c_char,
}

#[repr(C)]
#[derive(Default)]
pub struct Commit {
    pub data_size: c_int,
    pub text: *const c_char,
}

#[repr(C)]
#[derive(Default)]
pub struct Status {
    pub data_size: c_int,
    pub schema_id: *const c_char,
    pub schema_name: *const c_char,
    pub is_disabled: Bool,
    pub is_composing: Bool,
    pub is_ascii_mode: Bool,
    pub is_full_shape: Bool,
    pub is_simplified: Bool,
    pub is_traditional: Bool,
    pub is_ascii_punct: Bool,
}

#[repr(C)]
#[derive(Default)]
pub struct Config {
    pub ptr: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Api {
    pub data_size: c_int,
    pub setup: Option<unsafe extern "C" fn(*mut Traits)>,
    pub set_notification_handler: Unused,
    pub initialize: Option<unsafe extern "C" fn(*mut Traits)>,
    pub finalize: Option<unsafe extern "C" fn()>,
    pub start_maintenance: Unused,
    pub is_maintenance_mode: Unused,
    pub join_maintenance_thread: Unused,
    pub deployer_initialize: Option<unsafe extern "C" fn(*mut Traits)>,
    pub prebuild: Unused,
    pub deploy: Option<unsafe extern "C" fn() -> Bool>,
    pub deploy_schema: Unused,
    pub deploy_config_file: Unused,
    pub sync_user_data: Unused,
    pub create_session: Option<unsafe extern "C" fn() -> SessionId>,
    pub find_session: Option<unsafe extern "C" fn(SessionId) -> Bool>,
    pub destroy_session: Option<unsafe extern "C" fn(SessionId) -> Bool>,
    pub cleanup_stale_sessions: Unused,
    pub cleanup_all_sessions: Unused,
    pub process_key: Option<unsafe extern "C" fn(SessionId, c_int, c_int) -> Bool>,
    pub commit_composition: Unused,
    pub clear_composition: Option<unsafe extern "C" fn(SessionId)>,
    pub get_commit: Option<unsafe extern "C" fn(SessionId, *mut Commit) -> Bool>,
    pub free_commit: Option<unsafe extern "C" fn(*mut Commit) -> Bool>,
    pub get_context: Option<unsafe extern "C" fn(SessionId, *mut Context) -> Bool>,
    pub free_context: Option<unsafe extern "C" fn(*mut Context) -> Bool>,
    pub get_status: Option<unsafe extern "C" fn(SessionId, *mut Status) -> Bool>,
    pub free_status: Option<unsafe extern "C" fn(*mut Status) -> Bool>,
    pub set_option: Option<unsafe extern "C" fn(SessionId, *const c_char, Bool)>,
    pub get_option: Option<unsafe extern "C" fn(SessionId, *const c_char) -> Bool>,
    pub set_property: Unused,
    pub get_property: Unused,
    pub get_schema_list: Unused,
    pub free_schema_list: Unused,
    pub get_current_schema: Unused,
    pub select_schema: Option<unsafe extern "C" fn(SessionId, *const c_char) -> Bool>,
    pub schema_open: Option<unsafe extern "C" fn(*const c_char, *mut Config) -> Bool>,
    pub config_open: Unused,
    pub config_close: Option<unsafe extern "C" fn(*mut Config) -> Bool>,
    pub config_get_bool:
        Option<unsafe extern "C" fn(*mut Config, *const c_char, *mut Bool) -> Bool>,
    pub config_get_int:
        Option<unsafe extern "C" fn(*mut Config, *const c_char, *mut c_int) -> Bool>,
    pub config_get_double: Unused,
    pub config_get_string: Unused,
    pub config_get_cstring: Unused,
    pub config_update_signature: Unused,
    pub config_begin_map: Unused,
    pub config_next: Unused,
    pub config_end: Unused,
    pub simulate_key_sequence: Unused,
    pub register_module: Unused,
    pub find_module: Option<unsafe extern "C" fn(*const c_char) -> *mut c_void>,
    pub run_task: Unused,
    pub get_shared_data_dir: Unused,
    pub get_user_data_dir: Unused,
    pub get_sync_dir: Unused,
    pub get_user_id: Unused,
    pub get_user_data_sync_dir: Unused,
    pub config_init: Unused,
    pub config_load_string: Unused,
    pub config_set_bool: Unused,
    pub config_set_int: Unused,
    pub config_set_double: Unused,
    pub config_set_string: Unused,
    pub config_get_item: Unused,
    pub config_set_item: Unused,
    pub config_clear: Unused,
    pub config_create_list: Unused,
    pub config_create_map: Unused,
    pub config_list_size: Unused,
    pub config_begin_list: Unused,
    pub get_input: Unused,
    pub get_caret_pos: Unused,
    pub select_candidate: Unused,
    pub get_version: Option<unsafe extern "C" fn() -> *const c_char>,
}

/// RIME_STRUCT_INIT 的 data_size 包含对齐填充，但不包含首个 int。
pub fn data_size<T>() -> c_int {
    (size_of::<T>() - size_of::<c_int>()) as c_int
}
