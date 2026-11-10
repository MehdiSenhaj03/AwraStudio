//! Settings: which model the assistant uses, the user's name, latency.

use awra::prelude::*;
use awra::{Orientation, Typography};
use studio_ai::ProviderKind;

use super::{label, S};

fn row(grid: &awra::Grid, r: i32, title: &str, w: &impl IsWidget) {
    let l = label(title, Typography::Label);
    l.set_valign(awra::Align::Center);
    grid.attach(&l, 0, r, 1, 1);
    w.as_ref().set_hexpand(true);
    grid.attach(w, 1, r, 1, 1);
}

pub fn open(s: &S, window: &awra::Window) {
    let st = s.borrow();
    let cfg = st.settings.ai.clone();
    let dlg = awra::Dialog::new(Some(window));
    dlg.set_title(Some("Réglages"));
    dlg.set_content_width(560);
    let col = awra::Box::new(Orientation::Vertical, 10);

    let intro = label(
        "L'ingénieur local marche sans connexion. Pour des demandes libres, connecte le modèle auquel tu as déjà accès : \
         ChatGPT (abonnement, via l'outil officiel Codex : « codex login »), Claude (abonnement via Claude Code, ou clé API), \
         ou un modèle local (Ollama, LM Studio…).",
        Typography::Muted,
    );
    intro.set_wrap(true);
    col.append(&intro);

    let grid = awra::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    let kinds: Vec<&str> = ProviderKind::ALL.iter().map(|k| k.label()).collect();
    let provider = awra::Dropdown::from_strings(&kinds);
    provider.set_selected(ProviderKind::ALL.iter().position(|k| *k == cfg.kind).unwrap_or(0) as u32);
    row(&grid, 0, "Modèle", &provider);
    let model = awra::Entry::new();
    model.set_text(&cfg.model);
    model.set_placeholder_text(Some("par défaut"));
    row(&grid, 1, "Nom du modèle", &model);
    let key = awra::PasswordEntry::new();
    key.set_text(&cfg.api_key);
    row(&grid, 2, "Clé API", &key);
    let url = awra::Entry::new();
    url.set_text(&cfg.base_url);
    url.set_placeholder_text(Some("par défaut"));
    row(&grid, 3, "Adresse du serveur", &url);
    let cmd = awra::Entry::new();
    cmd.set_text(&cfg.command);
    cmd.set_placeholder_text(Some("par défaut : codex exec … / claude -p …"));
    row(&grid, 4, "Commande (CLI)", &cmd);
    let login = awra::Button::with_label("Se connecter…");
    row(&grid, 5, "Connexion", &login);
    let proactive = awra::Switch::new();
    proactive.set_active(cfg.proactive);
    proactive.set_halign(awra::Align::Start);
    row(&grid, 6, "Conseils après chaque prise", &proactive);
    let name = awra::Entry::new();
    name.set_text(&st.settings.user_name);
    row(&grid, 7, "Ton nom (sessions)", &name);
    let latency = awra::SpinButton::with_range(0.0, 300.0, 1.0);
    latency.set_value(st.settings.latency_ms);
    row(&grid, 8, "Compensation de latence (ms)", &latency);
    let upnp = awra::Switch::new();
    upnp.set_active(st.settings.try_upnp);
    upnp.set_halign(awra::Align::Start);
    row(&grid, 9, "Ouvrir le port sur la box (UPnP)", &upnp);
    col.append(&grid);
    let audio = label(
        &format!(
            "Sortie audio : {}{}",
            st.engine.output_name,
            st.engine.input_name.as_ref().map(|i| format!(" · Entrée : {i}")).unwrap_or_else(|| " · aucun micro".into())
        ),
        Typography::Muted,
    );
    audio.set_wrap(true);
    col.append(&audio);
    drop(st);

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
    dlg.set_body(Some(&col));
    dlg.add_response("cancel", "Annuler");
    dlg.add_response("ok", "Enregistrer");
    dlg.set_response_appearance("ok", awra::ButtonAppearance::Primary);
    dlg.set_default_response(Some("ok"));
    let c = s.clone();
    dlg.connect_response(None, move |d, r| {
        if r == "ok" {
            let mut st = c.borrow_mut();
            let ai = &mut st.settings.ai;
            ai.kind = ProviderKind::ALL[provider.selected() as usize % ProviderKind::ALL.len()];
            ai.model = model.text();
            ai.api_key = key.text();
            ai.base_url = url.text();
            ai.command = cmd.text();
            ai.proactive = proactive.is_active();
            st.settings.user_name = name.text();
            st.settings.latency_ms = latency.value();
            st.settings.try_upnp = upnp.is_active();
            st.engine.latency_ms = latency.value();
            st.settings.save();
            st.ui_revision += 1;
        }
        d.close();
    });
    dlg.present();
}
