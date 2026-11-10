//! A minimal CLAP plugin (stereo gain, one parameter, state) to test the
//! host end to end.
#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicU64, Ordering};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

struct Gain {
    plugin: clap_plugin,
    gain: AtomicU64,
}

const ID: &CStr = c"org.awra.test-gain";
static FEATURES: [usize; 2] = [0, 0];

#[repr(transparent)]
struct SyncPtr<T>(T);
unsafe impl<T> Sync for SyncPtr<T> {}

static FEATURE_LIST: SyncPtr<[*const c_char; 2]> = SyncPtr([c"audio-effect".as_ptr(), std::ptr::null()]);

static DESC: SyncPtr<clap_plugin_descriptor> = SyncPtr(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"Awra Test Gain".as_ptr(),
    vendor: c"Awra".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0".as_ptr(),
    description: c"test".as_ptr(),
    features: FEATURE_LIST.0.as_ptr(),
});

unsafe fn me(p: *const clap_plugin) -> &'static Gain {
    &*((*p).plugin_data as *const Gain)
}
fn gain_of(g: &Gain) -> f64 {
    f64::from_bits(g.gain.load(Ordering::Relaxed))
}

unsafe extern "C" fn init(_p: *const clap_plugin) -> bool { true }
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(Box::from_raw((*p).plugin_data as *mut Gain));
}
unsafe extern "C" fn activate(_p: *const clap_plugin, _sr: f64, _a: u32, _b: u32) -> bool { true }
unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}
unsafe extern "C" fn start(_p: *const clap_plugin) -> bool { true }
unsafe extern "C" fn stop(_p: *const clap_plugin) {}
unsafe extern "C" fn reset(_p: *const clap_plugin) {}
unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let g = me(p);
    let pr = &*pr;
    let ev = &*pr.in_events;
    let n_ev = ev.size.map(|f| f(ev)).unwrap_or(0);
    for i in 0..n_ev {
        let h = ev.get.unwrap()(ev, i);
        if (*h).space_id == CLAP_CORE_EVENT_SPACE_ID && (*h).type_ == CLAP_EVENT_PARAM_VALUE {
            let e = &*(h as *const clap_event_param_value);
            g.gain.store(e.value.to_bits(), Ordering::Relaxed);
        }
    }
    let gain = gain_of(g) as f32;
    let inp = &*pr.audio_inputs;
    let out = &*pr.audio_outputs;
    for c in 0..out.channel_count.min(inp.channel_count) as usize {
        let i = *inp.data32.add(c);
        let o = *out.data32.add(c);
        for k in 0..pr.frames_count as usize {
            *o.add(k) = *i.add(k) * gain;
        }
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn p_count(_p: *const clap_plugin) -> u32 { 1 }
unsafe extern "C" fn p_info(_p: *const clap_plugin, i: u32, info: *mut clap_param_info) -> bool {
    if i != 0 { return false; }
    let info = &mut *info;
    info.id = 7;
    info.flags = 0;
    info.cookie = std::ptr::null_mut();
    for (k, b) in b"Gain\0".iter().enumerate() { info.name[k] = *b as c_char; }
    info.module[0] = 0;
    info.min_value = 0.0;
    info.max_value = 2.0;
    info.default_value = 1.0;
    true
}
unsafe extern "C" fn p_value(p: *const clap_plugin, id: clap_id, v: *mut f64) -> bool {
    if id != 7 { return false; }
    *v = gain_of(me(p));
    true
}
unsafe extern "C" fn p_flush(_p: *const clap_plugin, _i: *const clap_sys::events::clap_input_events, _o: *const clap_sys::events::clap_output_events) {}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(p_count),
    get_info: Some(p_info),
    get_value: Some(p_value),
    value_to_text: None,
    text_to_value: None,
    flush: Some(p_flush),
};

unsafe extern "C" fn s_save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
    let b = gain_of(me(p)).to_le_bytes();
    (*s).write.unwrap()(s, b.as_ptr() as *const c_void, 8) == 8
}
unsafe extern "C" fn s_load(p: *const clap_plugin, s: *const clap_istream) -> bool {
    let mut b = [0u8; 8];
    if (*s).read.unwrap()(s, b.as_mut_ptr() as *mut c_void, 8) != 8 { return false; }
    me(p).gain.store(f64::from_le_bytes(b).to_bits(), Ordering::Relaxed);
    true
}
static STATE: clap_plugin_state = clap_plugin_state { save: Some(s_save), load: Some(s_load) };

unsafe extern "C" fn get_ext(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_PARAMS { return &PARAMS as *const _ as *const c_void; }
    if id == CLAP_EXT_STATE { return &STATE as *const _ as *const c_void; }
    std::ptr::null()
}
unsafe extern "C" fn on_main(_p: *const clap_plugin) {}

unsafe extern "C" fn f_count(_f: *const clap_plugin_factory) -> u32 { 1 }
unsafe extern "C" fn f_desc(_f: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    if i == 0 { &DESC.0 } else { std::ptr::null() }
}
unsafe extern "C" fn f_create(_f: *const clap_plugin_factory, _h: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if CStr::from_ptr(id) != ID { return std::ptr::null(); }
    let g = Box::into_raw(Box::new(Gain {
        plugin: clap_plugin {
            desc: &DESC.0,
            plugin_data: std::ptr::null_mut(),
            init: Some(init), destroy: Some(destroy), activate: Some(activate), deactivate: Some(deactivate),
            start_processing: Some(start), stop_processing: Some(stop), reset: Some(reset), process: Some(process),
            get_extension: Some(get_ext), on_main_thread: Some(on_main),
        },
        gain: AtomicU64::new(1.0f64.to_bits()),
    }));
    (*g).plugin.plugin_data = g as *mut c_void;
    &(*g).plugin
}
static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(f_count),
    get_plugin_descriptor: Some(f_desc),
    create_plugin: Some(f_create),
};

unsafe extern "C" fn e_init(_p: *const c_char) -> bool { true }
unsafe extern "C" fn e_deinit() {}
unsafe extern "C" fn e_factory(id: *const c_char) -> *const c_void {
    if CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID { &FACTORY as *const _ as *const c_void } else { std::ptr::null() }
}

#[no_mangle]
#[used]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(e_init),
    deinit: Some(e_deinit),
    get_factory: Some(e_factory),
};

#[allow(dead_code)]
fn unused() -> usize { FEATURES.len() }
