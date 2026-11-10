//! Awra Studio: a music studio that is simple to start with, free to go
//! further, with an AI sound engineer that acts, and friends who join from
//! home.
//!
//! Usage:
//!   awra-studio [project.awra]          open the studio (and a project)
//!   awra-studio --demo                  open the demo song
//!   awra-studio --screenshot out.png [--demo] [--size 1440x900]
//!               [--layout recording] [--ask "…"] [--select N] [--dark]
//!                                       render the interface offscreen
//!   awra-studio --scan-plugin <file>    print the plugins of a file (JSON)
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

mod aihost;
mod appctl;
mod assets;
mod demo;
mod edit;
mod settings;
mod studio;
mod ui;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use studio::Studio;

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn initial_document(args: &[String]) -> (studio_core::Document, Vec<(String, studio_dsp::AudioBuffer)>) {
    if args.iter().any(|a| a == "--demo") {
        return demo::demo_document();
    }
    if let Some(p) = args.iter().skip(1).find(|a| !a.starts_with("--") && PathBuf::from(a).join("project.json").exists()) {
        if let Ok(d) = studio_core::Document::open(&PathBuf::from(p)) {
            return (d, Vec::new());
        }
    }
    (studio_core::Document::new(studio_core::Project::new("Sans titre")), Vec::new())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(p) = arg_value(&args, "--scan-plugin") {
        let list = studio_plugins::scan_file(&PathBuf::from(p)).unwrap_or_default();
        println!("{}", serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
        return;
    }
    let shot = arg_value(&args, "--screenshot");
    if shot.is_some() {
        std::env::set_var("AWRA_STUDIO_NO_AUDIO", "1");
        if args.iter().any(|a| a == "--dark") {
            std::env::set_var("AWRA_APPEARANCE", "dark");
        }
    }
    let app = awra::Application::new("org.awra.Studio");
    let (doc, bufs) = initial_document(&args);
    let mut settings = settings::Settings::load();
    if shot.is_some() {
        settings = settings::Settings { user_name: "Sam".into(), ..Default::default() };
    }
    let mut st = Studio::new(doc, settings);
    for (id, b) in bufs {
        st.assets.insert(&id, b);
    }
    st.rebuild_graph();
    let s = Rc::new(RefCell::new(st));

    match shot {
        Some(png) => {
            let (w, h) = arg_value(&args, "--size")
                .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
                .unwrap_or((1440, 900));
            let window = awra::Window::offscreen("Awra Studio", w, h, 1.0);
            let ui = ui::Ui::build(&s, window.clone());
            window.set_default_size(w, h);
            if let Some(l) = arg_value(&args, "--layout") {
                ui.apply_layout(&s, studio_ai::LayoutCommand::Preset { name: l });
            }
            if let Some(n) = arg_value(&args, "--select").and_then(|n| n.parse::<usize>().ok()) {
                let mut st = s.borrow_mut();
                st.selected = st.doc.project.tracks.get(n).map(|t| t.id);
                st.ui_revision += 1;
            }
            if let Some(name) = arg_value(&args, "--appearance") {
                let a = match name.as_str() {
                    "opaque" => ui::appearance::Appearance::opaque(),
                    "clear" => ui::appearance::Appearance::clear_glass(),
                    _ => ui::appearance::Appearance { panels: Some(0.6), cards: Some(0.9), lanes: Some(0.5), ..Default::default() },
                };
                ui::appearance::apply(&a);
                s.borrow_mut().settings.appearance = a;
            }
            let settings_win = args.iter().any(|a| a == "--settings").then(|| {
                let w = awra::Window::offscreen("Réglages", 680, 860, 1.0);
                w.set_child(Some(&ui::settings_dialog::appearance_preview(&s)));
                w
            });
            if let Some(t) = arg_value(&args, "--dock") {
                if let Some(n) = awra::DockNode::parse(&t) {
                    ui.dock.set_layout(n);
                }
            }
            if args.iter().any(|a| a == "--master-chain") {
                let mut st = s.borrow_mut();
                st.show_master_chain = true;
                st.ui_revision += 1;
            }
            if let Some(tab) = arg_value(&args, "--open-settings").and_then(|t| t.parse::<u32>().ok()) {
                ui::settings_dialog::open_tab(&s, &window, tab);
            }
            if args.iter().any(|a| a == "--edit") {
                ui.dock.set_editing(true);
            }
            if let Some(q) = arg_value(&args, "--ask") {
                s.borrow_mut().ask_ai(&q);
            }
            if let Some(t) = arg_value(&args, "--seek").and_then(|t| t.parse::<f64>().ok()) {
                s.borrow_mut().seek(t);
            }
            let start = Instant::now();
            // let background work (decoding, the AI) finish
            while start.elapsed() < Duration::from_secs(60) {
                awra::pump();
                std::thread::sleep(Duration::from_millis(15));
                let busy = {
                    let st = s.borrow();
                    st.ai_busy.is_some() || st.assets.is_loading()
                };
                if !busy && start.elapsed() > Duration::from_millis(1500) {
                    break;
                }
            }
            if let Some(xy) = arg_value(&args, "--drop-hover") {
                // a file dragged over the timeline at x,y
                if let Some((x, y)) = xy.split_once(',').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
                    let mut a = awra::signal_args(awra::sig::DROP_HOVER);
                    a.active = true;
                    a.x = x;
                    a.y = y;
                    ui.timeline_widget().emit_raw(&a);
                }
            }
            for _ in 0..10 {
                awra::pump();
                std::thread::sleep(Duration::from_millis(20));
            }
            if std::env::var("AWRA_STUDIO_DEBUG").is_ok() {
                eprintln!("layout: {}", ui.dock.layout().map(|l| l.to_text()).unwrap_or_default());
                window.render_offscreen(true);
                for (id, _) in ui.dock.panel_ids() {
                    if let Some(w) = ui.dock.panel_widget(&id) {
                        let mut p = w.parent();
                        let mut chain = Vec::new();
                        while let Some(x) = p {
                            let (mn, nt, _, _) = x.measure(awra::Orientation::Horizontal, -1);
                            chain.push(format!("{}:{}[{mn},{nt}]", x.widget_name(), x.width()));
                            p = x.parent();
                        }
                        eprintln!("{id}: w={} h={} mapped={} parents={}", w.width(), w.height(), w.is_mapped(), chain.join(" < "));
                    }
                }
            }
            if let Some(w) = &settings_win {
                w.render_offscreen(true);
                w.snapshot_png(&png.replace(".png", "-settings.png"));
            }
            // dialogs and other windows opened for the capture
            for (i, w) in awra::Window::toplevels().iter().filter(|w| **w != window).enumerate() {
                w.render_offscreen(true);
                w.snapshot_png(&png.replace(".png", &format!("-win{i}.png")));
            }
            window.render_offscreen(true);
            if !window.snapshot_png(&png) {
                eprintln!("capture impossible");
                std::process::exit(1);
            }
            println!("{png}");
        }
        None => {
            let window = awra::Window::new(&app);
            let _ui = ui::Ui::build(&s, window.clone());
            window.present();
            app.run();
            s.borrow().settings.save();
        }
    }
}
