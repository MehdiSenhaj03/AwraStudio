//! VST3 hosting (audio, parameters, state; the plugin's own editor window
//! is not embedded: its parameters are shown by Awra Studio).

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use studio_core::effects::PluginFormat;
use studio_dsp::Processor;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

use crate::{ParamInfo, PluginDesc};

/// The shared library inside a .vst3 bundle (or the file itself).
pub fn binary_of(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let arch = if cfg!(windows) {
        if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" }
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-linux"
    } else {
        "x86_64-linux"
    };
    let ext = if cfg!(windows) { "vst3" } else { "so" };
    let p = path.join("Contents").join(arch).join(format!("{stem}.{ext}"));
    p.exists().then_some(p)
}

pub struct Vst3Module {
    lib: libloading::Library,
    factory: ComPtr<IPluginFactory>,
    pub path: PathBuf,
}

unsafe impl Send for Vst3Module {}
unsafe impl Sync for Vst3Module {}

fn cstr(buf: &[c_char]) -> String {
    let b: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&b).into_owned()
}

fn wstr(buf: &[TChar]) -> String {
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n].iter().map(|c| *c as u16).collect::<Vec<_>>())
}

impl Vst3Module {
    pub fn load(path: &Path) -> Result<Arc<Vst3Module>> {
        let bin = binary_of(path).ok_or_else(|| anyhow!("binaire VST3 introuvable dans {}", path.display()))?;
        unsafe {
            let lib = libloading::Library::new(&bin)?;
            #[cfg(not(windows))]
            {
                if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0") {
                    // the handle is unused by most plugins; pass a non-null token
                    if !entry(1 as *mut c_void) {
                        return Err(anyhow!("ModuleEntry a échoué"));
                    }
                }
            }
            #[cfg(windows)]
            {
                if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
                    init();
                }
            }
            let get: libloading::Symbol<unsafe extern "system" fn() -> *mut IPluginFactory> = lib.get(b"GetPluginFactory\0")?;
            let f = get();
            let factory = ComPtr::from_raw(f).ok_or_else(|| anyhow!("pas de fabrique VST3"))?;
            Ok(Arc::new(Vst3Module { lib, factory, path: path.to_path_buf() }))
        }
    }

    pub fn descriptors(&self) -> Vec<PluginDesc> {
        let mut out = Vec::new();
        unsafe {
            let mut finfo: PFactoryInfo = std::mem::zeroed();
            self.factory.getFactoryInfo(&mut finfo);
            let vendor = cstr(&finfo.vendor);
            for i in 0..self.factory.countClasses() {
                let mut info: PClassInfo = std::mem::zeroed();
                if self.factory.getClassInfo(i, &mut info) != kResultOk {
                    continue;
                }
                if cstr(&info.category) != "Audio Module Class" {
                    continue;
                }
                let id: String = info.cid.iter().map(|b| format!("{:02x}", *b as u8)).collect();
                out.push(PluginDesc {
                    name: cstr(&info.name),
                    vendor: vendor.clone(),
                    format: PluginFormat::Vst3,
                    path: self.path.to_string_lossy().into_owned(),
                    id,
                    category: "effect".into(),
                });
            }
        }
        out
    }
}

impl Drop for Vst3Module {
    fn drop(&mut self) {
        unsafe {
            #[cfg(not(windows))]
            if let Ok(exit) = self.lib.get::<unsafe extern "C" fn() -> bool>(b"ModuleExit\0") {
                exit();
            }
            #[cfg(windows)]
            if let Ok(exit) = self.lib.get::<unsafe extern "system" fn() -> bool>(b"ExitDll\0") {
                exit();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Host-side COM objects

struct HostApp;
impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}
impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let s: Vec<u16> = "Awra Studio".encode_utf16().collect();
        for (i, c) in s.iter().enumerate() {
            (*name)[i] = *c as TChar;
        }
        (*name)[s.len()] = 0;
        kResultOk
    }
    unsafe fn createInstance(&self, _cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        *obj = std::ptr::null_mut();
        kNotImplemented
    }
}

/// A growable memory stream (component state).
struct MemStream {
    data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}
impl Class for MemStream {
    type Interfaces = (IBStream,);
}
impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, n: int32, read: *mut int32) -> tresult {
        let d = self.data.borrow();
        let p = self.pos.get();
        let k = (n.max(0) as usize).min(d.len().saturating_sub(p));
        std::ptr::copy_nonoverlapping(d.as_ptr().add(p), buffer as *mut u8, k);
        self.pos.set(p + k);
        if !read.is_null() {
            *read = k as int32;
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, n: int32, written: *mut int32) -> tresult {
        let mut d = self.data.borrow_mut();
        let p = self.pos.get();
        let n = n.max(0) as usize;
        if d.len() < p + n {
            d.resize(p + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, d.as_mut_ptr().add(p), n);
        self.pos.set(p + n);
        if !written.is_null() {
            *written = n as int32;
        }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let len = self.data.borrow().len() as i64;
        let base = match mode {
            0 => 0,
            1 => self.pos.get() as i64,
            _ => len,
        };
        let np = (base + pos).clamp(0, len);
        self.pos.set(np as usize);
        if !result.is_null() {
            *result = np;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            *pos = self.pos.get() as int64;
        }
        kResultOk
    }
}

const MAX_CHANGES: usize = 64;

/// One parameter's change in a block (a single point at offset 0).
struct Queue {
    id: Cell<ParamID>,
    value: Cell<f64>,
}
impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}
impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        *offset = 0;
        *value = self.value.get();
        kResultOk
    }
    unsafe fn addPoint(&self, _offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        self.value.set(value);
        if !index.is_null() {
            *index = 0;
        }
        kResultOk
    }
}

struct Changes {
    queues: Vec<ComWrapper<Queue>>,
    ptrs: Vec<*mut IParamValueQueue>,
    count: Cell<usize>,
}
impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}
impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        self.count.get() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index < 0 || index as usize >= self.count.get() {
            return std::ptr::null_mut();
        }
        self.ptrs[index as usize]
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

// ---------------------------------------------------------------------------

pub struct Vst3Shared {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    _host: ComWrapper<HostApp>,
    _module: Arc<Vst3Module>,
    pub name: String,
    processing: std::sync::atomic::AtomicBool,
}

unsafe impl Send for Vst3Shared {}
unsafe impl Sync for Vst3Shared {}

fn parse_cid(hex: &str) -> Option<TUID> {
    let mut t: TUID = [0; 16];
    if hex.len() != 32 {
        return None;
    }
    for i in 0..16 {
        t[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()? as c_char;
    }
    Some(t)
}

impl Vst3Shared {
    pub fn create(module: Arc<Vst3Module>, class_id: &str, sr: f64, max_block: u32) -> Result<Arc<Vst3Shared>> {
        let cid = parse_cid(class_id).ok_or_else(|| anyhow!("identifiant VST3 invalide"))?;
        let host = ComWrapper::new(HostApp);
        let host_ptr = host.to_com_ptr::<FUnknown>().ok_or_else(|| anyhow!("hôte COM"))?;
        unsafe {
            let mut obj: *mut c_void = std::ptr::null_mut();
            let iid = IComponent::IID;
            if module.factory.createInstance(cid.as_ptr(), iid.as_ptr() as *const c_char, &mut obj) != kResultOk || obj.is_null() {
                return Err(anyhow!("le plugin VST3 n'a pas pu être créé"));
            }
            let component = ComPtr::<IComponent>::from_raw(obj as *mut IComponent).ok_or_else(|| anyhow!("composant nul"))?;
            if component.initialize(host_ptr.as_ptr()) != kResultOk {
                return Err(anyhow!("initialisation du plugin échouée"));
            }
            let processor = component.cast::<IAudioProcessor>().ok_or_else(|| anyhow!("pas de processeur audio"))?;
            // the controller: the component itself, or a separate class
            let mut controller = component.cast::<IEditController>();
            if controller.is_none() {
                let mut ccid: TUID = [0; 16];
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let mut cobj: *mut c_void = std::ptr::null_mut();
                    let ciid = IEditController::IID;
                    if module.factory.createInstance(ccid.as_ptr(), ciid.as_ptr() as *const c_char, &mut cobj) == kResultOk {
                        if let Some(c) = ComPtr::<IEditController>::from_raw(cobj as *mut IEditController) {
                            c.initialize(host_ptr.as_ptr());
                            if let (Some(a), Some(b)) = (component.cast::<IConnectionPoint>(), c.cast::<IConnectionPoint>()) {
                                a.connect(b.as_ptr());
                                b.connect(a.as_ptr());
                            }
                            controller = Some(c);
                        }
                    }
                }
            }
            // stereo in, stereo out on the main buses
            let mut ins = [SpeakerArr::kStereo];
            let mut outs = [SpeakerArr::kStereo];
            let n_in = component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32);
            processor.setBusArrangements(
                if n_in > 0 { ins.as_mut_ptr() } else { std::ptr::null_mut() },
                n_in.min(1),
                outs.as_mut_ptr(),
                1,
            );
            if n_in > 0 {
                component.activateBus(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32, 0, 1);
            }
            component.activateBus(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32, 0, 1);
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                maxSamplesPerBlock: max_block as i32,
                sampleRate: sr,
            };
            if processor.setupProcessing(&mut setup) != kResultOk {
                return Err(anyhow!("le plugin refuse cette configuration audio"));
            }
            component.setActive(1);
            let mut shared = Vst3Shared {
                component,
                processor,
                controller,
                _host: host,
                _module: module.clone(),
                name: String::new(),
                processing: std::sync::atomic::AtomicBool::new(false),
            };
            // the controller learns the component's state
            if let Some(state) = shared.component_state() {
                shared.set_controller_state(&state);
            }
            let name = module
                .descriptors()
                .into_iter()
                .find(|d| d.id == class_id)
                .map(|d| d.name)
                .unwrap_or_else(|| "VST3".into());
            shared.name = name;
            Ok(Arc::new(shared))
        }
    }

    fn component_state(&self) -> Option<Vec<u8>> {
        let s = ComWrapper::new(MemStream { data: RefCell::new(Vec::new()), pos: Cell::new(0) });
        let p = s.to_com_ptr::<IBStream>()?;
        unsafe {
            (self.component.getState(p.as_ptr()) == kResultOk).then(|| s.data.borrow().clone())
        }
    }

    fn set_controller_state(&self, data: &[u8]) {
        if let Some(c) = &self.controller {
            let s = ComWrapper::new(MemStream { data: RefCell::new(data.to_vec()), pos: Cell::new(0) });
            if let Some(p) = s.to_com_ptr::<IBStream>() {
                unsafe { c.setComponentState(p.as_ptr()) };
            }
        }
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        self.component_state()
    }

    pub fn load_state(&self, data: &[u8]) -> bool {
        let s = ComWrapper::new(MemStream { data: RefCell::new(data.to_vec()), pos: Cell::new(0) });
        let Some(p) = s.to_com_ptr::<IBStream>() else { return false };
        let ok = unsafe { self.component.setState(p.as_ptr()) == kResultOk };
        if ok {
            self.set_controller_state(data);
        }
        ok
    }

    /// Parameters (normalized 0..1 values, as VST3 has them).
    pub fn params(&self) -> Vec<ParamInfo> {
        let mut out = Vec::new();
        let Some(c) = &self.controller else { return out };
        unsafe {
            for i in 0..c.getParameterCount() {
                let mut info: ParameterInfo = std::mem::zeroed();
                if c.getParameterInfo(i, &mut info) != kResultOk {
                    continue;
                }
                // read-only and hidden parameters are of no use here
                if info.flags & (ParameterInfo_::ParameterFlags_::kIsReadOnly as i32 | ParameterInfo_::ParameterFlags_::kIsHidden as i32) != 0 {
                    continue;
                }
                out.push(ParamInfo {
                    id: info.id.to_string(),
                    name: wstr(&info.title),
                    min: 0.0,
                    max: 1.0,
                    default: info.defaultNormalizedValue,
                    value: c.getParamNormalized(info.id),
                    stepped: info.stepCount > 0,
                });
            }
        }
        out
    }

    pub fn value_text(&self, id: &str, value: f64) -> Option<String> {
        let c = self.controller.as_ref()?;
        let pid: ParamID = id.parse().ok()?;
        let mut s: String128 = [0; 128];
        unsafe { (c.getParamStringByValue(pid, value, &mut s) == kResultOk).then(|| wstr(&s)) }
    }

    pub fn set_controller_param(&self, id: &str, value: f64) {
        if let (Some(c), Ok(pid)) = (&self.controller, id.parse::<ParamID>()) {
            unsafe { c.setParamNormalized(pid, value) };
        }
    }
}

impl Drop for Vst3Shared {
    fn drop(&mut self) {
        unsafe {
            if self.processing.load(std::sync::atomic::Ordering::Relaxed) {
                self.processor.setProcessing(0);
            }
            self.component.setActive(0);
            if let Some(c) = &self.controller {
                if let (Some(a), Some(b)) = (self.component.cast::<IConnectionPoint>(), c.cast::<IConnectionPoint>()) {
                    a.disconnect(b.as_ptr());
                    b.disconnect(a.as_ptr());
                }
                c.terminate();
            }
            self.component.terminate();
        }
    }
}

pub struct Vst3Processor {
    pub shared: Arc<Vst3Shared>,
    changes: ComWrapper<Changes>,
    changes_ptr: ComPtr<IParameterChanges>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

unsafe impl Send for Vst3Processor {}

impl Vst3Processor {
    pub fn new(shared: Arc<Vst3Shared>, max_block: usize) -> Option<Vst3Processor> {
        let queues: Vec<ComWrapper<Queue>> =
            (0..MAX_CHANGES).map(|_| ComWrapper::new(Queue { id: Cell::new(0), value: Cell::new(0.0) })).collect();
        let ptrs = queues.iter().map(|q| q.as_com_ref::<IParamValueQueue>().map(|r| r.as_ptr())).collect::<Option<Vec<_>>>()?;
        let changes = ComWrapper::new(Changes { queues, ptrs, count: Cell::new(0) });
        let changes_ptr = changes.to_com_ptr::<IParameterChanges>()?;
        Some(Vst3Processor { shared, changes, changes_ptr, out_l: vec![0.0; max_block], out_r: vec![0.0; max_block] })
    }
}

impl Processor for Vst3Processor {
    fn set_param(&mut self, id: &str, value: f64) {
        let Ok(pid) = id.parse::<ParamID>() else { return };
        let c = &self.changes;
        let n = c.count.get();
        // a parameter changed twice in a block keeps its last value
        for q in &c.queues[..n] {
            if q.id.get() == pid {
                q.value.set(value.clamp(0.0, 1.0));
                return;
            }
        }
        if n < MAX_CHANGES {
            c.queues[n].id.set(pid);
            c.queues[n].value.set(value.clamp(0.0, 1.0));
            c.count.set(n + 1);
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(self.out_l.len());
        let s = &self.shared;
        unsafe {
            if !s.processing.load(std::sync::atomic::Ordering::Relaxed) {
                s.processor.setProcessing(1);
                s.processing.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let mut in_ptrs: [*mut f32; 2] = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut out_ptrs: [*mut f32; 2] = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
            let mut input = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 { channelBuffers32: in_ptrs.as_mut_ptr() },
            };
            let mut output = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 { channelBuffers32: out_ptrs.as_mut_ptr() },
            };
            let mut data = ProcessData {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                numSamples: n as i32,
                numInputs: 1,
                numOutputs: 1,
                inputs: &mut input,
                outputs: &mut output,
                inputParameterChanges: self.changes_ptr.as_ptr(),
                outputParameterChanges: std::ptr::null_mut(),
                inputEvents: std::ptr::null_mut(),
                outputEvents: std::ptr::null_mut(),
                processContext: std::ptr::null_mut(),
            };
            s.processor.process(&mut data);
            self.changes.count.set(0);
            l[..n].copy_from_slice(&self.out_l[..n]);
            r[..n].copy_from_slice(&self.out_r[..n]);
        }
    }
}
