//! Loads the test CLAP plugin and runs it through the host.

use std::path::PathBuf;

use studio_core::effects::{EffectKind, PluginFormat};
use studio_core::model::EffectSlot;
use studio_plugins::{scan_file, PluginManager};

fn plugin_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "awra-test-gain"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(st.success());
    let name = if cfg!(windows) { "awra_test_gain.dll" } else { "libawra_test_gain.so" };
    let target = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or(root.join("target"));
    let src = target.join("debug").join(name);
    let dst = std::env::temp_dir().join(format!("awra-test-gain-{}.clap", std::process::id()));
    std::fs::copy(src, &dst).unwrap();
    dst
}

#[test]
fn clap_plugin_end_to_end() {
    let path = plugin_path();
    let found = scan_file(&path).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Awra Test Gain");
    let d = &found[0];
    let mut slot = EffectSlot::new(EffectKind::Plugin {
        format: PluginFormat::Clap,
        path: d.path.clone(),
        plugin_id: d.id.clone(),
        name: d.name.clone(),
    });
    slot.params.insert("7".into(), 0.5);
    let mgr = PluginManager::new();
    let mut p = mgr.create(&slot, 48000.0, 512).expect("instance");
    let mut l = vec![1.0f32; 256];
    let mut r = vec![-1.0f32; 256];
    p.process(&mut l, &mut r);
    assert_eq!((l[10], r[10]), (0.5, -0.5));
    let inst = mgr.instance(slot.id).unwrap();
    let params = inst.params();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "Gain");
    assert_eq!(params[0].value, 0.5);
    // the state comes back in a new instance
    let st = mgr.states();
    assert_eq!(st.len(), 1);
    slot.plugin_state = Some(st[0].1.clone());
    slot.params.clear();
    slot.id += 1;
    let mut p2 = mgr.create(&slot, 48000.0, 512).unwrap();
    let mut l = vec![1.0f32; 64];
    let mut r = vec![1.0f32; 64];
    p2.process(&mut l, &mut r);
    assert_eq!(l[0], 0.5);
    drop(p);
    drop(p2);
    std::fs::remove_file(path).ok();
}
