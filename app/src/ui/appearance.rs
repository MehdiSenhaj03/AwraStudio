//! How the studio looks: theme, accent, glass or opaque, and the opacity
//! of each part of the interface (window, header, panels, cards, menus,
//! timeline lanes). Applied live.

use std::cell::RefCell;

use awra::gdk::RGBA;
use awra::{Appearance as Theme, Material, StyleManager};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// 0 system, 1 light, 2 dark.
    pub theme: u8,
    /// Accent colour (hex), None: Awra's violet.
    pub accent: Option<String>,
    /// The compositor's blur behind the window (frosted glass).
    pub blur: bool,
    /// Opacity of each part (0..1); None: the theme's default.
    pub window: Option<f64>,
    pub header: Option<f64>,
    pub panels: Option<f64>,
    pub cards: Option<f64>,
    pub menus: Option<f64>,
    pub lanes: Option<f64>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { theme: 0, accent: None, blur: true, window: None, header: None, panels: None, cards: None, menus: None, lanes: None }
    }
}

/// The parts the user can tune: (key, label, what it is).
pub const PARTS: [(&str, &str, &str); 6] = [
    ("window", "Fond de la fenêtre", "Le verre derrière tout le studio"),
    ("header", "Barre du haut", "Transport, titre, boutons"),
    ("panels", "Panneaux", "Le fond de chaque panneau (timeline, effets, assistant…)"),
    ("cards", "Cartes", "Effets, messages de l'IA, sections de paroles"),
    ("menus", "Menus et fenêtres", "Menus, popovers, dialogues"),
    ("lanes", "Pistes de la timeline", "Le fond des pistes audio"),
];

pub const ACCENTS: [(&str, &str); 7] = [
    ("Violet", ""),
    ("Bleu", "#3b82f6"),
    ("Turquoise", "#14b8a6"),
    ("Vert", "#22c55e"),
    ("Orange", "#f97316"),
    ("Rose", "#ec4899"),
    ("Rouge", "#ef4444"),
];

thread_local! {
    static CURRENT: RefCell<Appearance> = RefCell::new(Appearance::default());
    static HEADER: RefCell<Option<awra::Header>> = const { RefCell::new(None) };
}

pub fn current() -> Appearance {
    CURRENT.with(|c| c.borrow().clone())
}

impl Appearance {
    pub fn get(&self, part: &str) -> Option<f64> {
        match part {
            "window" => self.window,
            "header" => self.header,
            "panels" => self.panels,
            "cards" => self.cards,
            "menus" => self.menus,
            "lanes" => self.lanes,
            _ => None,
        }
    }
    pub fn set(&mut self, part: &str, v: Option<f64>) {
        let v = v.map(|x| x.clamp(0.0, 1.0));
        match part {
            "window" => self.window = v,
            "header" => self.header = v,
            "panels" => self.panels = v,
            "cards" => self.cards = v,
            "menus" => self.menus = v,
            "lanes" => self.lanes = v,
            _ => {}
        }
    }
    /// Everything opaque (a quick choice).
    pub fn opaque() -> Appearance {
        Appearance {
            blur: false,
            window: Some(1.0),
            header: Some(1.0),
            panels: Some(1.0),
            cards: Some(1.0),
            menus: Some(1.0),
            lanes: None,
            ..Appearance::default()
        }
    }
    /// Lighter glass: more of the desktop shows through.
    pub fn clear_glass() -> Appearance {
        Appearance { window: Some(0.35), menus: Some(0.55), cards: Some(0.06), ..Appearance::default() }
    }
}

/// The opacity in effect for a part (its default when not set).
pub fn effective(part: &str) -> f64 {
    let a = current();
    if let Some(v) = a.get(part) {
        return v;
    }
    let sm = StyleManager::default();
    match part {
        "window" => sm.material_opacity(Material::Canvas),
        "header" => sm.material_opacity(Material::Chrome),
        "cards" => sm.material_opacity(Material::Layer),
        "menus" => sm.material_opacity(Material::Floating),
        "panels" => 0.0,
        "lanes" => {
            if sm.is_dark() {
                0.035
            } else {
                0.03
            }
        }
        _ => 1.0,
    }
}

/// The header follows the window's glass unless its opacity is set.
pub fn register_header(h: &awra::Header) {
    HEADER.with(|x| *x.borrow_mut() = Some(h.clone()));
}

pub fn apply(a: &Appearance) {
    CURRENT.with(|c| *c.borrow_mut() = a.clone());
    let sm = StyleManager::default();
    sm.set_appearance(match a.theme {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::System,
    });
    match a.accent.as_deref().filter(|x| !x.is_empty()).and_then(|h| RGBA::parse(h).ok()) {
        Some(c) => sm.set_accent_color(&c),
        None => sm.set_accent_source(awra::AccentSource::Default),
    }
    sm.set_native_blur(a.blur);
    sm.set_material_opacity(Material::Canvas, a.window);
    sm.set_material_opacity(Material::Chrome, a.header);
    sm.set_material_opacity(Material::Layer, a.cards);
    sm.set_material_opacity(Material::Floating, a.menus);
    HEADER.with(|x| {
        if let Some(h) = x.borrow().as_ref() {
            h.set_blend_with_window(a.header.is_none());
        }
    });
    // windows repaint with the new style
    for w in awra::Window::toplevels() {
        w.queue_draw();
    }
}

/// A panel's own background, painted behind its content.
pub fn panel_background(content: &impl awra::IsWidget) -> awra::Layout {
    let bg = awra::DrawingArea::with_canvas(|_, c, w, h| {
        let p = effective("panels");
        if p <= 0.001 {
            return;
        }
        let sm = StyleManager::default();
        let canvas = sm.color("canvas").unwrap_or(RGBA::new(0.5, 0.5, 0.5, 1.0));
        // a tile a little apart from the window: lighter on dark, whiter on light
        let k = if sm.is_dark() { 0.06 } else { 0.55 };
        let mix = |a: f32| a + (1.0 - a) * k;
        let base = RGBA::new(mix(canvas.red()), mix(canvas.green()), mix(canvas.blue()), 1.0);
        let sep = sm.color("separator").unwrap_or(RGBA::new(0.5, 0.5, 0.5, 0.2));
        c.round_rect(3.0, 3.0, w as f64 - 6.0, h as f64 - 6.0, 12.0, Some(&base.with_alpha(p as f32)), 1.0, Some(&sep.with_alpha(sep.alpha() * p as f32)));
    });
    let l = awra::Layout::bin();
    l.append(&bg);
    l.append(content);
    l.set_hexpand(true);
    l.set_vexpand(true);
    l
}
