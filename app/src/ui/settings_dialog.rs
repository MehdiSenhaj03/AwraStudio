//! Settings, in three tabs: Appearance (theme, accent, glass or opaque,
//! the opacity of each part, previewed live), the AI assistant, and
//! audio & sessions.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{Orientation, Typography};
use studio_ai::ProviderKind;

use super::appearance::{self, Appearance, ACCENTS, PARTS};
use super::{label, S};

fn row(grid: &awra::Grid, r: i32, title: &str, w: &impl IsWidget) {
    let l = label(title, Typography::Label);
    l.set_valign(awra::Align::Center);
    grid.attach(&l, 0, r, 1, 1);
    w.as_ref().set_hexpand(true);
    grid.attach(w, 1, r, 1, 1);
}

fn grid() -> awra::Grid {
    let g = awra::Grid::new();
    g.set_row_spacing(10);
    g.set_column_spacing(14);
    g
}

/// The Appearance tab; edits `draft` and applies it at once.
fn appearance_tab(draft: &Rc<RefCell<Appearance>>) -> awra::Box {
    let col = awra::Box::new(Orientation::Vertical, 14);
    let g = grid();
    let theme = awra::SegmentedControl::new();
    for t in ["Système", "Clair", "Sombre"] {
        theme.append(t);
    }
    theme.set_selected(draft.borrow().theme as u32);
    row(&g, 0, "Thème", &theme);
    let accents = awra::Dropdown::from_strings(&ACCENTS.iter().map(|a| a.0).collect::<Vec<_>>());
    let cur = draft.borrow().accent.clone().unwrap_or_default();
    accents.set_selected(ACCENTS.iter().position(|a| a.1 == cur).unwrap_or(0) as u32);
    row(&g, 1, "Couleur d'accent", &accents);
    let blur = awra::Switch::new();
    blur.set_active(draft.borrow().blur);
    blur.set_halign(awra::Align::Start);
    row(&g, 2, "Effet verre (flou du bureau)", &blur);
    col.append(&g);

    let presets = awra::Box::new(Orientation::Horizontal, 6);
    let glass = awra::Button::with_label("Verre (par défaut)");
    let clear = awra::Button::with_label("Verre léger");
    let opaque = awra::Button::with_label("Tout opaque");
    for b in [&glass, &clear, &opaque] {
        b.set_appearance(awra::ButtonAppearance::Secondary);
        presets.append(b);
    }
    col.append(&super::section("Transparence"));
    col.append(&presets);
    let hint = label(
        "Règle l'opacité de chaque élément : 0 % laisse voir le bureau à travers, 100 % est opaque. « Auto » garde le réglage du thème.",
        Typography::Muted,
    );
    hint.set_wrap(true);
    col.append(&hint);

    let pg = grid();
    let mut sliders: Vec<(&'static str, awra::Slider, awra::Label, awra::CheckButton)> = Vec::new();
    for (i, (key, name, what)) in PARTS.iter().enumerate() {
        let names = awra::Box::new(Orientation::Vertical, 0);
        names.append(&label(name, Typography::Label));
        let w = label(what, Typography::Muted);
        w.set_wrap(true);
        w.set_max_width_chars(28);
        names.append(&w);
        pg.attach(&names, 0, i as i32, 1, 1);
        let auto = awra::CheckButton::with_label("Auto");
        let sl = awra::Slider::with_range(Orientation::Horizontal, 0.0, 1.0, 0.01);
        sl.set_draw_value(false);
        sl.set_hexpand(true);
        sl.set_size_request(180, -1);
        let val = label("", Typography::Label);
        val.set_width_chars(5);
        val.set_tabular_digits(true);
        pg.attach(&sl, 1, i as i32, 1, 1);
        pg.attach(&val, 2, i as i32, 1, 1);
        pg.attach(&auto, 3, i as i32, 1, 1);
        sliders.push((key, sl, val, auto));
    }
    col.append(&pg);

    let sliders = Rc::new(sliders);
    // shows the draft's values (or the theme's when Auto)
    let sync = {
        let (sl, d) = (sliders.clone(), draft.clone());
        Rc::new(move || {
            super::while_updating(|| {
                let a = d.borrow().clone();
                for (key, s, v, auto) in sl.iter() {
                    let eff = appearance::effective(key);
                    s.set_value(eff);
                    v.set_text(&format!("{:.0} %", eff * 100.0));
                    auto.set_active(a.get(key).is_none());
                }
            })
        })
    };
    let commit = {
        let (d, sync) = (draft.clone(), sync.clone());
        Rc::new(move || {
            appearance::apply(&d.borrow());
            sync();
        })
    };
    sync();
    for (key, s, _, auto) in sliders.iter() {
        let (d, c) = (draft.clone(), commit.clone());
        let k = *key;
        s.connect_value_changed(move |w| {
            if super::updating() {
                return;
            }
            d.borrow_mut().set(k, Some(w.value()));
            c();
        });
        let (d, c) = (draft.clone(), commit.clone());
        auto.connect_toggled(move |b| {
            if super::updating() {
                return;
            }
            let v = if b.is_active() { None } else { Some(appearance::effective(k)) };
            d.borrow_mut().set(k, v);
            c();
        });
    }
    let (d, c) = (draft.clone(), commit.clone());
    theme.connect_changed(move |_, i| {
        d.borrow_mut().theme = i as u8;
        c();
    });
    let (d, c) = (draft.clone(), commit.clone());
    accents.connect_selected_notify(move |dd| {
        let a = ACCENTS[dd.selected() as usize % ACCENTS.len()].1;
        d.borrow_mut().accent = if a.is_empty() { None } else { Some(a.to_string()) };
        c();
    });
    let (d, c) = (draft.clone(), commit.clone());
    blur.connect_active_notify(move |w| {
        if super::updating() {
            return;
        }
        d.borrow_mut().blur = w.is_active();
        c();
    });
    for (b, make) in [
        (glass, Appearance::default as fn() -> Appearance),
        (clear, Appearance::clear_glass as fn() -> Appearance),
        (opaque, Appearance::opaque as fn() -> Appearance),
    ] {
        let (d, c, bl) = (draft.clone(), commit.clone(), blur.clone());
        b.connect_clicked(move |_| {
            let mut a = make();
            let cur = d.borrow().clone();
            a.theme = cur.theme;
            a.accent = cur.accent;
            *d.borrow_mut() = a;
            super::while_updating(|| bl.set_active(d.borrow().blur));
            c();
        });
    }
    col
}

pub fn open(s: &S, window: &awra::Window) {
    let st = s.borrow();
    let cfg = st.settings.ai.clone();
    let before = st.settings.appearance.clone();
    let draft = Rc::new(RefCell::new(before.clone()));
    let dlg = awra::Dialog::new(Some(window));
    dlg.set_title(Some("Réglages"));
    dlg.set_content_width(640);

    let root = awra::Box::new(Orientation::Vertical, 14);
    let tabs = awra::SegmentedControl::new();
    for t in ["Apparence", "Assistant IA", "Audio et session"] {
        tabs.append(t);
    }
    tabs.set_halign(awra::Align::Center);
    let stack = awra::Stack::new();
    stack.set_transition_type(awra::StackTransitionType::Crossfade);
    stack.set_transition_duration(awra::motion_duration(&awra::StyleManager::default(), awra::MotionPreset::Fast));
    root.append(&tabs);
    root.append(&stack);

    // Appearance
    stack.add_named(&super::scrolled(&appearance_tab(&draft)), Some("look"));

    // AI
    let ai = awra::Box::new(Orientation::Vertical, 12);
    let intro = label(
        "L'ingénieur local marche sans connexion. Pour des demandes libres, connecte le modèle auquel tu as déjà accès : \
         ChatGPT (abonnement, via l'outil officiel Codex : « codex login »), Claude (abonnement via Claude Code, ou clé API), \
         ou un modèle local (Ollama, LM Studio…).",
        Typography::Muted,
    );
    intro.set_wrap(true);
    ai.append(&intro);
    let g = grid();
    let kinds: Vec<&str> = ProviderKind::ALL.iter().map(|k| k.label()).collect();
    let provider = awra::Dropdown::from_strings(&kinds);
    provider.set_selected(ProviderKind::ALL.iter().position(|k| *k == cfg.kind).unwrap_or(0) as u32);
    row(&g, 0, "Modèle", &provider);
    let model = awra::Entry::new();
    model.set_text(&cfg.model);
    row(&g, 1, "Nom du modèle", &model);
    let key = awra::PasswordEntry::new();
    key.set_text(&cfg.api_key);
    row(&g, 2, "Clé API", &key);
    let url = awra::Entry::new();
    url.set_text(&cfg.base_url);
    url.set_placeholder_text(Some("par défaut"));
    row(&g, 3, "Adresse du serveur", &url);
    let cmd = awra::Entry::new();
    cmd.set_text(&cfg.command);
    cmd.set_placeholder_text(Some("par défaut : codex exec … / claude -p …"));
    row(&g, 4, "Commande (CLI)", &cmd);
    let login = awra::Button::with_label("Se connecter…");
    row(&g, 5, "Connexion", &login);
    let proactive = awra::Switch::new();
    proactive.set_active(cfg.proactive);
    proactive.set_halign(awra::Align::Start);
    row(&g, 6, "Conseils après chaque prise", &proactive);
    ai.append(&g);
    stack.add_named(&ai, Some("ai"));

    // Audio & session
    let au = awra::Box::new(Orientation::Vertical, 12);
    let g2 = grid();
    let name = awra::Entry::new();
    name.set_text(&st.settings.user_name);
    row(&g2, 0, "Ton nom (sessions)", &name);
    let latency = awra::SpinButton::with_range(0.0, 300.0, 1.0);
    latency.set_value(st.settings.latency_ms);
    row(&g2, 1, "Compensation de latence (ms)", &latency);
    let count_in = awra::Switch::new();
    count_in.set_active(st.settings.count_in);
    count_in.set_halign(awra::Align::Start);
    row(&g2, 2, "Une mesure avant la prise", &count_in);
    let upnp = awra::Switch::new();
    upnp.set_active(st.settings.try_upnp);
    upnp.set_halign(awra::Align::Start);
    row(&g2, 3, "Ouvrir le port sur la box (UPnP)", &upnp);
    au.append(&g2);
    let audio = label(
        &format!(
            "Sortie audio : {}{}",
            st.engine.output_name,
            st.engine.input_name.as_ref().map(|i| format!(" · Entrée : {i}")).unwrap_or_else(|| " · aucun micro".into())
        ),
        Typography::Muted,
    );
    audio.set_wrap(true);
    au.append(&audio);
    stack.add_named(&au, Some("audio"));
    drop(st);

    let st2 = stack.clone();
    tabs.connect_changed(move |_, i| st2.set_visible_child_name(["look", "ai", "audio"][i as usize % 3]));

    let update_sensitivity = {
        let (key, url, cmd, login, model) = (key.clone(), url.clone(), cmd.clone(), login.clone(), model.clone());
        move |k: ProviderKind| {
            key.set_sensitive(k.needs_key() || k == ProviderKind::OpenAiCompatible);
            url.set_sensitive(matches!(k, ProviderKind::OpenAi | ProviderKind::Anthropic | ProviderKind::OpenAiCompatible));
            cmd.set_sensitive(matches!(k, ProviderKind::CodexCli | ProviderKind::ClaudeCli));
            login.set_sensitive(k.login_command().is_some());
            model.set_sensitive(!matches!(k, ProviderKind::Local | ProviderKind::CodexCli | ProviderKind::ClaudeCli));
            model.set_placeholder_text(Some(if k.default_model().is_empty() { "—" } else { k.default_model() }));
        }
    };
    update_sensitivity(cfg.kind);
    let u2 = update_sensitivity.clone();
    provider.connect_selected_notify(move |d| u2(ProviderKind::ALL[d.selected() as usize % ProviderKind::ALL.len()]));
    {
        let (provider, c) = (provider.clone(), s.clone());
        login.connect_clicked(move |_| {
            let k = ProviderKind::ALL[provider.selected() as usize % ProviderKind::ALL.len()];
            if let Some(cmd) = k.login_command() {
                match std::process::Command::new(cmd[0]).args(&cmd[1..]).spawn() {
                    Ok(_) => c.borrow_mut().toasts.push("Une page de connexion s'ouvre dans ton navigateur".into()),
                    Err(_) => c.borrow_mut().toasts.push(format!("« {} » n'est pas installé sur cette machine", cmd[0])),
                }
            }
        });
    }
    dlg.set_body(Some(&root));
    dlg.add_response("cancel", "Annuler");
    dlg.add_response("ok", "Enregistrer");
    dlg.set_response_appearance("ok", awra::ButtonAppearance::Primary);
    dlg.set_default_response(Some("ok"));
    let c = s.clone();
    dlg.connect_response(None, move |d, r| {
        let mut st = c.borrow_mut();
        if r == "ok" {
            let ai = &mut st.settings.ai;
            ai.kind = ProviderKind::ALL[provider.selected() as usize % ProviderKind::ALL.len()];
            ai.model = model.text();
            ai.api_key = key.text();
            ai.base_url = url.text();
            ai.command = cmd.text();
            ai.proactive = proactive.is_active();
            st.settings.user_name = name.text();
            st.settings.latency_ms = latency.value();
            st.settings.count_in = count_in.is_active();
            st.settings.try_upnp = upnp.is_active();
            st.engine.latency_ms = latency.value();
            st.settings.appearance = draft.borrow().clone();
            st.settings.save();
            st.ui_revision += 1;
        } else {
            // the live preview goes back
            appearance::apply(&before);
        }
        drop(st);
        d.close();
    });
    dlg.present();
}

/// The Appearance tab alone (screenshots).
pub fn appearance_preview(s: &S) -> awra::Box {
    let draft = Rc::new(RefCell::new(s.borrow().settings.appearance.clone()));
    let b = appearance_tab(&draft);
    b.set_margins(20, 20, 20, 20);
    b
}

/// Opens the settings on the Appearance tab (from the interface menu).
pub fn open_appearance(s: &S, window: &awra::Window) {
    open(s, window);
}
