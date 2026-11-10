//! The interface: a header with the transport, and a dock of panels the
//! user (or the AI) arranges.

pub mod appearance;
pub mod assistant;
pub mod browser;
pub mod effects;
pub mod header;
pub mod history;
pub mod icons;
pub mod lyrics;
pub mod master;
pub mod mixer;
pub mod plugins;
pub mod session;
pub mod settings_dialog;
pub mod timeline;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use awra::prelude::*;
use awra::{Dock, DockArea, DockNode, Orientation, Widget};
use studio_ai::host::LayoutCommand;

use crate::studio::Studio;

pub type S = Rc<RefCell<Studio>>;

thread_local! {
    static UPDATING: Cell<bool> = const { Cell::new(false) };
}

/// True while the interface is being refreshed from the model: handlers
/// of controls set programmatically must ignore their signals then.
pub fn updating() -> bool {
    UPDATING.with(|u| u.get())
}

pub fn while_updating<R>(f: impl FnOnce() -> R) -> R {
    let was = UPDATING.with(|u| u.replace(true));
    let r = f();
    UPDATING.with(|u| u.set(was));
    r
}

pub trait Panel {
    fn widget(&self) -> Widget;
    /// The model changed (structure or values).
    fn refresh(&self, s: &Studio);
    /// About 30 times a second (meters, playhead).
    fn tick(&self, _s: &Studio) {}
}

/// A widget as `&Widget` (for lists of mixed widgets).
pub fn wd<T: AsRef<Widget>>(t: &T) -> &Widget {
    t.as_ref()
}

/// A widget that appears with a short slide (after the next frame, so
/// the motion plays).
pub fn appear(child: &impl IsWidget, kind: awra::RevealerTransitionType) -> awra::Revealer {
    let r = awra::Revealer::new();
    r.set_transition_type(kind);
    r.set_transition_duration(awra::motion_duration(&awra::StyleManager::default(), awra::MotionPreset::Normal));
    r.set_child(Some(child));
    r.set_reveal_child(false);
    let r2 = r.clone();
    awra::glib::timeout_add_local_once(Duration::from_millis(16), move || r2.set_reveal_child(true));
    r
}

/// Scrolls to the end smoothly.
pub fn scroll_to_end(sw: &awra::ScrolledWindow) {
    let sw = sw.clone();
    let mut steps = 0;
    awra::glib::timeout_add_local(Duration::from_millis(16), move || {
        let adj = sw.vadjustment();
        let (v, upper, page) = (adj.value(), adj.upper(), adj.page_size());
        let target = (upper - page).max(0.0);
        let next = v + (target - v) * 0.3;
        sw.set_vvalue(if (target - next).abs() < 1.0 { target } else { next });
        steps += 1;
        if (target - next).abs() < 1.0 || steps > 40 {
            awra::glib::ControlFlow::Break
        } else {
            awra::glib::ControlFlow::Continue
        }
    });
}

pub fn label(text: &str, t: awra::Typography) -> awra::Label {
    let l = awra::Label::new(Some(text));
    l.set_typography(t);
    l.set_xalign(0.0);
    l
}

pub fn button(text: &str, icon: Option<&str>, appearance: awra::ButtonAppearance) -> awra::Button {
    let b = match (text.is_empty(), icon) {
        (true, Some(i)) => awra::Button::from_icon_name(i),
        (false, Some(i)) => {
            let b = awra::Button::new();
            let row = awra::Box::new(Orientation::Horizontal, 6);
            let img = awra::Image::from_icon_name(i);
            row.append(&img);
            row.append(&awra::Label::new(Some(text)));
            b.set_child(Some(&row));
            b
        }
        _ => awra::Button::with_label(text),
    };
    b.set_appearance(appearance);
    if text.is_empty() {
        b.set_has_frame(appearance != awra::ButtonAppearance::Ghost);
    }
    b
}

/// A titled block inside a panel.
pub fn section(title: &str) -> awra::Box {
    let b = awra::Box::new(Orientation::Vertical, 6);
    let l = label(title, awra::Typography::Eyebrow);
    b.append(&l);
    b
}

pub fn scrolled(child: &impl IsWidget) -> awra::ScrolledWindow {
    let sw = awra::ScrolledWindow::new();
    sw.set_policy(awra::PolicyType::Never, awra::PolicyType::Automatic);
    sw.set_child(Some(child));
    sw.set_vexpand(true);
    sw.set_hexpand(true);
    sw
}

pub fn clear(b: &awra::Box) {
    b.remove_all();
}

pub fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    format!("{:02}:{:04.1}", (t / 60.0) as u32, t % 60.0)
}

pub const PRESETS: [(&str, &str, &str); 6] = [
    ("default", "Par défaut", "h(0.735,v(0.640,timeline,effects),t(0,assistant,lyrics))"),
    ("recording", "Enregistrement voix", "h(0.700,v(0.600,timeline,effects),v(0.560,lyrics,assistant))"),
    ("writing", "Écriture", "h(0.520,lyrics,v(0.450,timeline,assistant))"),
    ("mixing", "Mixage", "h(0.760,v(0.520,timeline,h(0.580,mixer,effects)),t(0,master,assistant))"),
    ("minimal", "Minimal", "h(0.760,v(0.700,timeline,effects),assistant)"),
    ("session", "À plusieurs", "h(0.720,v(0.640,timeline,effects),t(0,session,lyrics,assistant))"),
];

pub fn preset(name: &str) -> Option<DockNode> {
    let n = name.to_lowercase();
    let key = match n.as_str() {
        x if x.contains("record") || x.contains("enregistr") || x.contains("voix") => "recording",
        x if x.contains("writ") || x.contains("écri") || x.contains("ecri") || x.contains("parole") => "writing",
        x if x.contains("mix") => "mixing",
        x if x.contains("mini") || x.contains("simpl") => "minimal",
        x if x.contains("session") || x.contains("collab") || x.contains("plusieurs") => "session",
        _ => "default",
    };
    PRESETS.iter().find(|p| p.0 == key).and_then(|p| DockNode::parse(p.2))
}

pub struct Ui {
    pub window: awra::Window,
    pub dock: Dock,
    pub toasts: awra::ToastOverlay,
    panels: Vec<Rc<dyn Panel>>,
    header: Rc<header::HeaderUi>,
    timeline: Rc<timeline::TimelinePanel>,
    effects: Rc<effects::EffectsPanel>,
    seen_revision: Cell<u64>,
    seen_doc_revision: Cell<u64>,
}

impl Ui {
    pub fn build(s: &S, window: awra::Window) -> Rc<Ui> {
        icons::register();
        window.set_title(Some("Awra Studio"));
        window.set_default_size(1440, 900);
        let dock = Dock::new();
        let toasts = awra::ToastOverlay::new();
        toasts.set_child(Some(&dock));

        let timeline = timeline::TimelinePanel::new(s);
        let effects = effects::EffectsPanel::new(s, &window);
        let panels: Vec<(&str, &str, Rc<dyn Panel>)> = vec![
            ("timeline", "Timeline", timeline.clone()),
            ("effects", "Effets", effects.clone()),
            ("assistant", "Ingénieur IA", assistant::AssistantPanel::new(s, &window)),
            ("lyrics", "Paroles", lyrics::LyricsPanel::new(s)),
            ("mixer", "Mixage", mixer::MixerPanel::new(s)),
            ("master", "Mastering & export", master::MasterPanel::new(s, &window)),
            ("session", "Session à plusieurs", session::SessionPanel::new(s)),
            ("browser", "Fichiers & références", browser::BrowserPanel::new(s, &window)),
            ("plugins", "Plugins", plugins::PluginsPanel::new(s)),
            ("history", "Historique", history::HistoryPanel::new(s)),
        ];
        for (id, title, p) in &panels {
            dock.add_panel(id, title, &appearance::panel_background(&p.widget()));
        }
        let ui_header = header::HeaderUi::new(s, &window, &dock);
        window.set_titlebar(Some(&ui_header.header));
        appearance::register_header(&ui_header.header);
        appearance::apply(&s.borrow().settings.appearance);
        window.set_child(Some(&toasts));

        let layout = DockNode::parse(&s.borrow().settings.layout).or_else(|| preset("default")).unwrap();
        dock.set_layout(layout);
        {
            let s = s.clone();
            dock.connect_changed(move |d| {
                if let Some(l) = d.layout() {
                    if let Ok(mut st) = s.try_borrow_mut() {
                        st.settings.layout = l.to_text();
                    }
                }
            });
        }
        let ui = Rc::new(Ui {
            window: window.clone(),
            dock,
            toasts,
            panels: panels.into_iter().map(|p| p.2).collect(),
            header: ui_header,
            timeline,
            effects,
            seen_revision: Cell::new(0),
            seen_doc_revision: Cell::new(u64::MAX),
        });
        ui.shortcuts(s);
        // files dropped anywhere on the window are imported
        {
            let s = s.clone();
            window.set_accepts_file_drops(true);
            let s2 = s.clone();
            wd(&window).connect_raw(awra::sig::DROP_HOVER, move |_, args| {
                if let Ok(mut st) = s2.try_borrow_mut() {
                    st.drag_over = args.active();
                }
            });
            window.connect_files_dropped(move |_, files| {
                s.borrow_mut().import_files(&files, None, 0.0);
            });
        }
        ui.refresh_all(&s.borrow());
        {
            let ui2 = ui.clone();
            let s = s.clone();
            awra::glib::timeout_add_local(Duration::from_millis(16), move || {
                ui2.frame(&s);
                awra::glib::ControlFlow::Continue
            });
        }
        {
            // the layout is saved with the settings when closing
            let s = s.clone();
            window.connect_close_request(move |_| {
                let st = s.borrow();
                st.settings.save();
                awra::glib::Propagation::Proceed
            });
        }
        ui
    }

    fn shortcuts(self: &Rc<Self>, s: &S) {
        let w = &self.window;
        let add = |accel: &str, f: Box<dyn Fn(&S)>| {
            let s = s.clone();
            w.add_shortcut(accel, move |_| f(&s));
        };
        add("space", Box::new(|s| s.borrow_mut().play_pause()));
        add("r", Box::new(|s| s.borrow_mut().toggle_record()));
        add("<Ctrl>z", Box::new(|s| s.borrow_mut().undo()));
        add("<Ctrl><Shift>z", Box::new(|s| s.borrow_mut().redo()));
        add("<Ctrl>y", Box::new(|s| s.borrow_mut().redo()));
        add("Home", Box::new(|s| s.borrow_mut().seek(0.0)));
        add("Delete", Box::new(|s| timeline::delete_selected_clip(&mut s.borrow_mut())));
        add("s", Box::new(|s| timeline::split_selected_clip(&mut s.borrow_mut())));
        let ui = self.clone();
        let s2 = s.clone();
        w.add_shortcut("<Ctrl>s", move |_| ui.header.save(&s2, false));
        let ui = self.clone();
        let s2 = s.clone();
        w.add_shortcut("<Ctrl>e", move |_| {
            let on = !ui.dock.is_editing();
            ui.dock.set_editing(on);
            s2.borrow_mut().toasts.push(if on { "Mode édition de l'interface : déplace, agrandis ou masque les panneaux".into() } else { "Interface verrouillée".into() });
        });
    }

    fn refresh_all(&self, s: &Studio) {
        while_updating(|| {
            self.header.refresh(s);
            for p in &self.panels {
                p.refresh(s);
            }
        });
        self.seen_revision.set(s.ui_revision);
        self.seen_doc_revision.set(s.doc.revision);
    }

    /// One frame: background work, then the views.
    pub fn frame(&self, s: &S) {
        let (layout, toasts, actions) = {
            let Ok(mut st) = s.try_borrow_mut() else { return };
            st.tick();
            self.timeline.animate(&mut st);
            (std::mem::take(&mut st.layout_requests), std::mem::take(&mut st.toasts), std::mem::take(&mut st.ui_actions))
        };
        for a in actions {
            match a {
                crate::studio::UiAction::ImportDialog => self.header.import(s),
                crate::studio::UiAction::Demo => load_demo(s),
            }
        }
        for cmd in layout {
            self.apply_layout(s, cmd);
        }
        for t in toasts {
            self.toasts.show_message(&t, 3500);
        }
        let Ok(st) = s.try_borrow() else { return };
        if st.ui_revision != self.seen_revision.get() || st.doc.revision != self.seen_doc_revision.get() {
            self.refresh_all(&st);
        }
        while_updating(|| {
            self.header.tick(&st);
            for p in &self.panels {
                p.tick(&st);
            }
        });
    }

    pub fn timeline_widget(&self) -> Widget {
        self.timeline.widget()
    }

    pub fn apply_layout(&self, s: &S, cmd: LayoutCommand) {
        let d = &self.dock;
        match cmd {
            LayoutCommand::Show { panel } => d.show(&panel),
            LayoutCommand::Hide { panel } => d.hide(&panel),
            LayoutCommand::Move { panel, area } => d.move_panel(&panel, DockArea::parse(&area).unwrap_or(DockArea::Right)),
            LayoutCommand::Resize { panel, factor } => d.resize_panel(&panel, factor),
            LayoutCommand::Preset { name } => {
                if let Some(p) = preset(&name) {
                    d.set_layout(p);
                }
            }
            LayoutCommand::Only { panels } => {
                let mut tree: Option<DockNode> = None;
                for p in panels.iter().filter(|p| d.panel_ids().iter().any(|(id, _)| id == *p)) {
                    tree = Some(match tree {
                        None => DockNode::Panel(p.clone()),
                        Some(t) => DockNode::Split { vertical: false, fraction: 0.7, a: Box::new(t), b: Box::new(DockNode::Panel(p.clone())) },
                    });
                }
                if let Some(t) = tree {
                    d.set_layout(t);
                }
            }
            LayoutCommand::FocusTrack { track } => {
                let id = {
                    let st = s.borrow();
                    studio_ai::tools::resolve_track(&st.doc.project, &track).and_then(|t| match t {
                        studio_core::ops::Target::Track(id) => Some(id),
                        _ => None,
                    })
                };
                if let Some(id) = id {
                    let mut st = s.borrow_mut();
                    st.selected = Some(id);
                    st.show_master_chain = false;
                    st.ui_revision += 1;
                }
                // the effects right under the timeline
                let rest = d.layout().and_then(|t| t.without("effects")).and_then(|t| t.without("timeline"));
                let tl = DockNode::Split {
                    vertical: true,
                    fraction: 0.62,
                    a: Box::new(DockNode::Panel("timeline".into())),
                    b: Box::new(DockNode::Panel("effects".into())),
                };
                d.set_layout(match rest {
                    Some(r) => DockNode::Split { vertical: false, fraction: 0.73, a: Box::new(tl), b: Box::new(r) },
                    None => tl,
                });
            }
        }
        let _ = &self.effects;
    }
}

/// Opens the demo song.
pub fn load_demo(s: &S) {
    let (doc, bufs) = crate::demo::demo_document();
    let mut st = s.borrow_mut();
    st.set_document(doc);
    for (id, b) in bufs {
        st.assets.insert(&id, b);
    }
    st.rebuild_graph();
}
