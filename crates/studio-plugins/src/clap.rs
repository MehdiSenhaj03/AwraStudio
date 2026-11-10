//! CLAP hosting.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_param_value, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID,
    CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::gui::{clap_plugin_gui, CLAP_EXT_GUI};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::timer_support::{clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::clap_process;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use parking_lot::Mutex;
use studio_core::effects::PluginFormat;
use studio_dsp::Processor;

use crate::{ParamInfo, PluginDesc};

/// A loaded .clap file (initialized once, kept while instances live).
pub struct ClapLibrary {
    _lib: libloading::Library,
    entry: *const clap_plugin_entry,
    pub path: PathBuf,
}

unsafe impl Send for ClapLibrary {}
unsafe impl Sync for ClapLibrary {}

impl ClapLibrary {
    pub fn load(path: &Path) -> Result<Arc<ClapLibrary>> {
        unsafe {
            let lib = libloading::Library::new(path)?;
            let entry: libloading::Symbol<*const clap_plugin_entry> = lib.get(b"clap_entry\0")?;
            let entry = *entry;
            if entry.is_null() {
                return Err(anyhow!("clap_entry absent"));
            }
            let p = CString::new(path.to_string_lossy().as_bytes())?;
            let init = (*entry).init.ok_or_else(|| anyhow!("init absent"))?;
            if !init(p.as_ptr()) {
                return Err(anyhow!("le plugin refuse de s'initialiser"));
            }
            Ok(Arc::new(ClapLibrary { _lib: lib, entry, path: path.to_path_buf() }))
        }
    }

    fn factory(&self) -> Option<*const clap_plugin_factory> {
        unsafe {
            let get = (*self.entry).get_factory?;
            let f = get(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            (!f.is_null()).then_some(f)
        }
    }

    pub fn descriptors(&self) -> Vec<PluginDesc> {
        let mut out = Vec::new();
        let Some(f) = self.factory() else { return out };
        unsafe {
            let count = (*f).get_plugin_count.map(|c| c(f)).unwrap_or(0);
            for i in 0..count {
                let Some(get) = (*f).get_plugin_descriptor else { break };
                let d = get(f, i);
                if d.is_null() {
                    continue;
                }
                let s = |p: *const c_char| if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() };
                let mut features = Vec::new();
                let mut fp = (*d).features;
                while !fp.is_null() && !(*fp).is_null() {
                    features.push(s(*fp));
                    fp = fp.add(1);
                }
                out.push(PluginDesc {
                    name: s((*d).name),
                    vendor: s((*d).vendor),
                    format: PluginFormat::Clap,
                    path: self.path.to_string_lossy().into_owned(),
                    id: s((*d).id),
                    category: category_of(&features),
                });
            }
        }
        out
    }
}

impl Drop for ClapLibrary {
    fn drop(&mut self) {
        unsafe {
            if let Some(deinit) = (*self.entry).deinit {
                deinit();
            }
        }
    }
}

fn category_of(features: &[String]) -> String {
    let has = |f: &str| features.iter().any(|x| x == f);
    if has("instrument") {
        "instrument"
    } else if has("pitch-correction") {
        "autotune"
    } else if has("reverb") {
        "reverb"
    } else if has("delay") {
        "delay"
    } else if has("compressor") || has("limiter") {
        "dynamics"
    } else if has("equalizer") {
        "eq"
    } else {
        "effect"
    }
    .to_string()
}

struct Timer {
    id: clap_id,
    period_ms: u32,
    next: std::time::Instant,
}

/// What the plugin and the host share (main-thread side).
pub struct ClapShared {
    pub plugin: *const clap_plugin,
    params: *const clap_plugin_params,
    state: *const clap_plugin_state,
    gui: *const clap_plugin_gui,
    timer_ext: *const clap_plugin_timer_support,
    host: Box<HostData>,
    _lib: Arc<ClapLibrary>,
    pub name: String,
    gui_open: AtomicBool,
    processing: AtomicBool,
    destroyed: AtomicBool,
}

unsafe impl Send for ClapShared {}
unsafe impl Sync for ClapShared {}

/// The host struct given to the plugin, with our state behind it.
#[repr(C)]
struct HostData {
    host: clap_host,
    callback_requested: AtomicBool,
    restart_requested: AtomicBool,
    timers: Mutex<Vec<Timer>>,
    next_timer: Mutex<clap_id>,
}

static HOST_NAME: &CStr = c"Awra Studio";
static HOST_VENDOR: &CStr = c"Awra";
static HOST_URL: &CStr = c"https://github.com/MehdiSenhajYnov/Awra";
static HOST_VERSION: &CStr = c"0.1.0";

static TIMER_SUPPORT: clap_host_timer_support =
    clap_host_timer_support { register_timer: Some(host_register_timer), unregister_timer: Some(host_unregister_timer) };

unsafe fn host_data(h: *const clap_host) -> &'static HostData {
    &*(h as *const HostData)
}

unsafe extern "C" fn host_get_extension(_h: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    if CStr::from_ptr(id) == CLAP_EXT_TIMER_SUPPORT {
        return &TIMER_SUPPORT as *const _ as *const c_void;
    }
    std::ptr::null()
}
unsafe extern "C" fn host_request_restart(h: *const clap_host) {
    host_data(h).restart_requested.store(true, Ordering::Relaxed);
}
unsafe extern "C" fn host_request_process(_h: *const clap_host) {}
unsafe extern "C" fn host_request_callback(h: *const clap_host) {
    host_data(h).callback_requested.store(true, Ordering::Relaxed);
}
unsafe extern "C" fn host_register_timer(h: *const clap_host, period_ms: u32, id: *mut clap_id) -> bool {
    let d = host_data(h);
    let mut n = d.next_timer.lock();
    *n += 1;
    let tid = *n;
    d.timers.lock().push(Timer { id: tid, period_ms: period_ms.max(10), next: std::time::Instant::now() });
    if !id.is_null() {
        *id = tid;
    }
    true
}
unsafe extern "C" fn host_unregister_timer(h: *const clap_host, id: clap_id) -> bool {
    host_data(h).timers.lock().retain(|t| t.id != id);
    true
}

fn read_name(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl ClapShared {
    pub fn create(lib: Arc<ClapLibrary>, plugin_id: &str, sr: f64, max_block: u32) -> Result<Arc<ClapShared>> {
        let f = lib.factory().ok_or_else(|| anyhow!("pas de fabrique de plugins"))?;
        let host = Box::new(HostData {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: HOST_NAME.as_ptr(),
                vendor: HOST_VENDOR.as_ptr(),
                url: HOST_URL.as_ptr(),
                version: HOST_VERSION.as_ptr(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_request_restart),
                request_process: Some(host_request_process),
                request_callback: Some(host_request_callback),
            },
            callback_requested: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            timers: Mutex::new(Vec::new()),
            next_timer: Mutex::new(0),
        });
        let id = CString::new(plugin_id)?;
        unsafe {
            let create = (*f).create_plugin.ok_or_else(|| anyhow!("create_plugin absent"))?;
            let plugin = create(f, &host.host, id.as_ptr());
            if plugin.is_null() {
                return Err(anyhow!("le plugin « {plugin_id} » n'a pas pu être créé"));
            }
            if !(*plugin).init.map(|i| i(plugin)).unwrap_or(false) {
                (*plugin).destroy.map(|d| d(plugin));
                return Err(anyhow!("init du plugin échoué"));
            }
            let ext = |id: &CStr| (*plugin).get_extension.map(|g| g(plugin, id.as_ptr())).unwrap_or(std::ptr::null());
            let params = ext(CLAP_EXT_PARAMS) as *const clap_plugin_params;
            let state = ext(CLAP_EXT_STATE) as *const clap_plugin_state;
            let gui = ext(CLAP_EXT_GUI) as *const clap_plugin_gui;
            let timer_ext = ext(CLAP_EXT_TIMER_SUPPORT) as *const clap_plugin_timer_support;
            if !(*plugin).activate.map(|a| a(plugin, sr, 1, max_block)).unwrap_or(false) {
                (*plugin).destroy.map(|d| d(plugin));
                return Err(anyhow!("activation du plugin échouée"));
            }
            let name = if (*plugin).desc.is_null() || (*(*plugin).desc).name.is_null() {
                plugin_id.to_string()
            } else {
                CStr::from_ptr((*(*plugin).desc).name).to_string_lossy().into_owned()
            };
            Ok(Arc::new(ClapShared {
                plugin,
                params,
                state,
                gui,
                timer_ext,
                host,
                _lib: lib,
                name,
                gui_open: AtomicBool::new(false),
                processing: AtomicBool::new(false),
                destroyed: AtomicBool::new(false),
            }))
        }
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        let mut out = Vec::new();
        if self.params.is_null() {
            return out;
        }
        unsafe {
            let p = &*self.params;
            let n = p.count.map(|c| c(self.plugin)).unwrap_or(0);
            for i in 0..n {
                let mut info: clap_param_info = std::mem::zeroed();
                if !p.get_info.map(|g| g(self.plugin, i, &mut info)).unwrap_or(false) {
                    continue;
                }
                let mut v = info.default_value;
                if let Some(gv) = p.get_value {
                    gv(self.plugin, info.id, &mut v);
                }
                // hidden / read-only parameters are of no use here
                if info.flags & (clap_sys::ext::params::CLAP_PARAM_IS_HIDDEN | clap_sys::ext::params::CLAP_PARAM_IS_READONLY) != 0 {
                    continue;
                }
                out.push(ParamInfo {
                    id: info.id.to_string(),
                    name: read_name(&info.name),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    value: v,
                    stepped: info.flags & clap_sys::ext::params::CLAP_PARAM_IS_STEPPED != 0,
                });
            }
        }
        out
    }

    pub fn value_text(&self, id: &str, value: f64) -> Option<String> {
        if self.params.is_null() {
            return None;
        }
        let pid: clap_id = id.parse().ok()?;
        let mut buf = [0 as c_char; 64];
        unsafe {
            let f = (*self.params).value_to_text?;
            f(self.plugin, pid, value, buf.as_mut_ptr(), 64).then(|| read_name(&buf))
        }
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        if self.state.is_null() {
            return None;
        }
        unsafe extern "C" fn write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
            let v = &mut *((*s).ctx as *mut Vec<u8>);
            v.extend_from_slice(std::slice::from_raw_parts(buf as *const u8, size as usize));
            size as i64
        }
        let mut data: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: &mut data as *mut Vec<u8> as *mut c_void, write: Some(write) };
        unsafe { (*self.state).save.map(|s| s(self.plugin, &stream)).unwrap_or(false).then_some(data) }
    }

    pub fn load_state(&self, data: &[u8]) -> bool {
        if self.state.is_null() {
            return false;
        }
        struct Cur<'a>(&'a [u8], usize);
        unsafe extern "C" fn read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
            let c = &mut *((*s).ctx as *mut Cur);
            let n = (size as usize).min(c.0.len() - c.1);
            std::ptr::copy_nonoverlapping(c.0.as_ptr().add(c.1), buf as *mut u8, n);
            c.1 += n;
            n as i64
        }
        let mut cur = Cur(data, 0);
        let stream = clap_istream { ctx: &mut cur as *mut Cur as *mut c_void, read: Some(read) };
        unsafe { (*self.state).load.map(|l| l(self.plugin, &stream)).unwrap_or(false) }
    }

    pub fn has_gui(&self) -> bool {
        !self.gui.is_null()
    }

    /// Opens the plugin's own window (floating).
    pub fn open_gui(&self) -> Result<()> {
        if self.gui.is_null() {
            return Err(anyhow!("ce plugin n'a pas d'interface graphique"));
        }
        if self.gui_open.load(Ordering::Relaxed) {
            unsafe { (*self.gui).show.map(|s| s(self.plugin)) };
            return Ok(());
        }
        let api = if cfg!(windows) { clap_sys::ext::gui::CLAP_WINDOW_API_WIN32 } else { clap_sys::ext::gui::CLAP_WINDOW_API_X11 };
        unsafe {
            let g = &*self.gui;
            if !g.is_api_supported.map(|f| f(self.plugin, api.as_ptr(), true)).unwrap_or(false) {
                return Err(anyhow!("ce plugin ne propose pas de fenêtre flottante sur ce système"));
            }
            if !g.create.map(|f| f(self.plugin, api.as_ptr(), true)).unwrap_or(false) {
                return Err(anyhow!("le plugin n'a pas pu créer sa fenêtre"));
            }
            let title = CString::new(format!("{} — Awra Studio", self.name)).unwrap_or_default();
            g.suggest_title.map(|f| f(self.plugin, title.as_ptr()));
            g.show.map(|f| f(self.plugin));
        }
        self.gui_open.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn close_gui(&self) {
        if self.gui.is_null() || !self.gui_open.swap(false, Ordering::Relaxed) {
            return;
        }
        unsafe {
            (*self.gui).hide.map(|f| f(self.plugin));
            (*self.gui).destroy.map(|f| f(self.plugin));
        }
    }

    /// Main-thread housekeeping: callbacks and timers the plugin asked for.
    pub fn idle(&self) {
        unsafe {
            if self.host.callback_requested.swap(false, Ordering::Relaxed) {
                (*self.plugin).on_main_thread.map(|f| f(self.plugin));
            }
            if !self.timer_ext.is_null() {
                let now = std::time::Instant::now();
                let due: Vec<clap_id> = {
                    let mut timers = self.host.timers.lock();
                    let mut due = Vec::new();
                    for t in timers.iter_mut() {
                        if now >= t.next {
                            t.next = now + std::time::Duration::from_millis(t.period_ms as u64);
                            due.push(t.id);
                        }
                    }
                    due
                };
                for id in due {
                    (*self.timer_ext).on_timer.map(|f| f(self.plugin, id));
                }
            }
        }
    }

    fn audio_ports(&self, input: bool) -> u32 {
        unsafe {
            let ext = (*self.plugin).get_extension.map(|g| g(self.plugin, CLAP_EXT_AUDIO_PORTS.as_ptr())).unwrap_or(std::ptr::null())
                as *const clap_plugin_audio_ports;
            if ext.is_null() {
                return 2;
            }
            let n = (*ext).count.map(|c| c(self.plugin, input)).unwrap_or(0);
            if n == 0 {
                return 0;
            }
            let mut info: clap_audio_port_info = std::mem::zeroed();
            if (*ext).get.map(|g| g(self.plugin, 0, input, &mut info)).unwrap_or(false) {
                info.channel_count.min(2)
            } else {
                2
            }
        }
    }
}

impl Drop for ClapShared {
    fn drop(&mut self) {
        if self.destroyed.swap(true, Ordering::Relaxed) {
            return;
        }
        self.close_gui();
        unsafe {
            if self.processing.load(Ordering::Relaxed) {
                (*self.plugin).stop_processing.map(|f| f(self.plugin));
            }
            (*self.plugin).deactivate.map(|f| f(self.plugin));
            (*self.plugin).destroy.map(|f| f(self.plugin));
        }
    }
}

// ---------------------------------------------------------------------------
// Processing

#[repr(C)]
struct EventList {
    events: Vec<clap_event_param_value>,
}

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    let l = &*((*list).ctx as *const EventList);
    l.events.len() as u32
}
unsafe extern "C" fn in_get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    let l = &*((*list).ctx as *const EventList);
    l.events.get(i as usize).map(|e| &e.header as *const _).unwrap_or(std::ptr::null())
}
unsafe extern "C" fn out_push(_list: *const clap_output_events, _e: *const clap_event_header) -> bool {
    // parameter changes from the plugin's own GUI are read back from
    // get_value when the host refreshes its controls
    true
}

pub struct ClapProcessor {
    pub shared: Arc<ClapShared>,
    events: Box<EventList>,
    in_ch: u32,
    out_ch: u32,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    steady: i64,
}

unsafe impl Send for ClapProcessor {}

impl ClapProcessor {
    pub fn new(shared: Arc<ClapShared>, max_block: usize) -> ClapProcessor {
        let in_ch = shared.audio_ports(true);
        let out_ch = shared.audio_ports(false).max(1);
        ClapProcessor {
            shared,
            events: Box::new(EventList { events: Vec::with_capacity(256) }),
            in_ch,
            out_ch,
            out_l: vec![0.0; max_block],
            out_r: vec![0.0; max_block],
            steady: 0,
        }
    }
}

impl Processor for ClapProcessor {
    fn set_param(&mut self, id: &str, value: f64) {
        let Ok(pid) = id.parse::<clap_id>() else { return };
        if self.events.events.len() < self.events.events.capacity() {
            self.events.events.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: pid,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            });
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(self.out_l.len());
        let p = self.shared.plugin;
        unsafe {
            if !self.shared.processing.load(Ordering::Relaxed) {
                let ok = (*p).start_processing.map(|f| f(p)).unwrap_or(true);
                self.shared.processing.store(ok, Ordering::Relaxed);
                if !ok {
                    return;
                }
            }
            let mut in_ptrs: [*mut f32; 2] = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut out_ptrs: [*mut f32; 2] = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
            let input = clap_audio_buffer {
                data32: in_ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: self.in_ch,
                latency: 0,
                constant_mask: 0,
            };
            let mut output = clap_audio_buffer {
                data32: out_ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: self.out_ch,
                latency: 0,
                constant_mask: 0,
            };
            let in_events = clap_input_events {
                ctx: &*self.events as *const EventList as *mut c_void,
                size: Some(in_size),
                get: Some(in_get),
            };
            let out_events = clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(out_push) };
            let proc_ = clap_process {
                steady_time: self.steady,
                frames_count: n as u32,
                transport: std::ptr::null(),
                audio_inputs: &input,
                audio_outputs: &mut output,
                audio_inputs_count: if self.in_ch > 0 { 1 } else { 0 },
                audio_outputs_count: 1,
                in_events: &in_events,
                out_events: &out_events,
            };
            (*p).process.map(|f| f(p, &proc_));
            self.steady += n as i64;
            self.events.events.clear();
            l[..n].copy_from_slice(&self.out_l[..n]);
            if self.out_ch > 1 {
                r[..n].copy_from_slice(&self.out_r[..n]);
            } else {
                r[..n].copy_from_slice(&self.out_l[..n]);
            }
        }
    }

    fn reset(&mut self) {
        unsafe {
            let p = self.shared.plugin;
            (*p).reset.map(|f| f(p));
        }
    }
}
