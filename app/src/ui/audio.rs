//! Sound devices and the microphone's return: which microphone and output,
//! the buffer size, how the voice is heard while singing, a live input
//! level and the latency it gives. The same controls sit in the header's
//! microphone popover and in the settings.

use std::rc::Rc;
use std::time::Duration;

use awra::prelude::*;
use awra::{Orientation, Typography};

use super::{label, S};
use crate::settings::BUFFER_SIZES;

pub const MONITOR_MODES: [&str; 3] = [
    "Direct — le plus rapide, sans effets",
    "Avec les effets, en faible latence",
    "Avec tous les effets (plus lent)",
];

/// What the microphone's tooltip says.
pub fn summary(s: &crate::studio::Studio) -> String {
    let mic = s.engine.input_name.clone().unwrap_or_else(|| "aucun".into());
    match s.engine.monitor_latency_ms(s.settings.monitor_mode) {
        Some(ms) => format!("Micro : {mic} · retour ≈ {ms:.0} ms"),
        None => format!("Micro : {mic}"),
    }
}

fn row(grid: &awra::Grid, r: i32, title: &str, w: &impl IsWidget, label_w: i32) {
    let l = label(title, Typography::Label);
    l.set_valign(awra::Align::Center);
    l.set_xalign(0.0);
    l.set_size_request(label_w, -1);
    grid.attach(&l, 0, r, 1, 1);
    w.as_ref().set_hexpand(true);
    grid.attach(w, 1, r, 1, 1);
}

/// The audio controls; every change applies at once (and is saved).
/// `label_w`: the label column (the settings' own, in the dialog).
pub fn controls(s: &S, label_w: i32) -> awra::Box {
    let col = awra::Box::new(Orientation::Vertical, 10);
    let devs = studio_engine::list_devices();
    let st = s.borrow();
    let set = st.settings.clone();
    let cur_in = st.engine.input_name.clone();
    let cur_out = st.engine.output_name.clone();
    drop(st);

    let g = awra::Grid::new();
    g.set_row_spacing(10);
    g.set_column_spacing(14);

    // microphone: the system's, each one found, or none
    let sys_in = format!("Celui du système{}", devs.default_input.as_ref().map(|d| format!(" ({d})")).unwrap_or_default());
    let mut in_items: Vec<String> = vec![sys_in];
    in_items.extend(devs.inputs.iter().cloned());
    in_items.push("Aucun micro".into());
    let in_refs: Vec<&str> = in_items.iter().map(String::as_str).collect();
    let mic = awra::Dropdown::from_strings(&in_refs);
    mic.set_selected(if set.no_input {
        (in_items.len() - 1) as u32
    } else {
        set.audio_input.as_ref().and_then(|n| devs.inputs.iter().position(|d| d == n)).map(|i| i as u32 + 1).unwrap_or(0)
    });
    row(&g, 0, "Micro", &mic, label_w);

    // its live level: speak, the bar moves
    let meter = awra::LevelMeter::new(Orientation::Horizontal);
    meter.as_ref().set_size_request(-1, 10);
    meter.as_ref().set_valign(awra::Align::Center);
    meter.as_ref().set_tooltip_text(Some("Parle ou chante : la barre bouge si le micro est le bon"));
    row(&g, 1, "Niveau", &meter, label_w);

    let sys_out = format!("Celle du système{}", devs.default_output.as_ref().map(|d| format!(" ({d})")).unwrap_or_default());
    let mut out_items: Vec<String> = vec![sys_out];
    out_items.extend(devs.outputs.iter().cloned());
    let out_refs: Vec<&str> = out_items.iter().map(String::as_str).collect();
    let out = awra::Dropdown::from_strings(&out_refs);
    out.set_selected(set.audio_output.as_ref().and_then(|n| devs.outputs.iter().position(|d| d == n)).map(|i| i as u32 + 1).unwrap_or(0));
    row(&g, 2, "Sortie", &out, label_w);
    col.append(&g);

    col.append(&super::section("Retour micro"));
    let g2 = awra::Grid::new();
    g2.set_row_spacing(10);
    g2.set_column_spacing(14);
    let hear = awra::Switch::new();
    hear.set_active(set.monitor_armed);
    hear.set_halign(awra::Align::Start);
    hear.set_tooltip_text(Some("Pendant une prise, tu t'entends toujours. Activé : aussi dès que la piste est armée (avec un casque)"));
    row(&g2, 0, "M'entendre hors prise", &hear, label_w);
    let mode = awra::Dropdown::from_strings(&MONITOR_MODES);
    mode.set_selected(set.monitor_mode.min(2) as u32);
    row(&g2, 1, "Retour", &mode, label_w);
    let sizes: Vec<&str> = BUFFER_SIZES.iter().map(|b| b.1).collect();
    let buf = awra::Dropdown::from_strings(&sizes);
    buf.set_selected(BUFFER_SIZES.iter().position(|b| b.0 == set.buffer_frames).unwrap_or(1) as u32);
    row(&g2, 2, "Tampon", &buf, label_w);
    col.append(&g2);
    let latency = label("", Typography::Muted);
    latency.set_wrap(true);
    col.append(&latency);
    let status = label("", Typography::Muted);
    status.set_wrap(true);
    col.append(&status);
    let _ = (cur_in, cur_out);

    let show = {
        let (s, latency, status) = (s.clone(), latency.clone(), status.clone());
        Rc::new(move || {
            let st = s.borrow();
            let lat = match st.engine.monitor_latency_ms(st.settings.monitor_mode) {
                Some(ms) => format!("Retour ≈ {ms:.0} ms entre le micro et ton casque (sous 10 ms on ne l'entend pas)."),
                None => "Aucun micro ouvert : rien à entendre.".into(),
            };
            latency.set_text(&lat);
            status.set_text(&match &st.engine.last_error {
                Some(e) => format!("Attention : {e}"),
                None => String::new(),
            });
            status.set_visible(st.engine.last_error.is_some());
        })
    };
    show();

    // live level and latency while visible
    {
        let (s, meter, show) = (s.clone(), meter.clone(), show.clone());
        let weak = col.downgrade();
        let mut n = 0u32;
        awra::glib::timeout_add_local(Duration::from_millis(33), move || {
            let Some(c) = weak.upgrade() else { return awra::glib::ControlFlow::Break };
            if c.is_mapped() {
                if let Ok(st) = s.try_borrow() {
                    let v = st.engine.input_level();
                    meter.set_levels(v, v);
                }
                n += 1;
                // the periods are known once the cards have run a little
                if n % 15 == 0 {
                    show();
                }
            }
            awra::glib::ControlFlow::Continue
        });
    }

    let reopen = {
        let (s, show) = (s.clone(), show.clone());
        Rc::new(move || {
            let msg = {
                let mut st = s.borrow_mut();
                st.settings.save();
                let m = st.reopen_audio();
                st.toasts.push(m.clone());
                m
            };
            let _ = msg;
            let show = show.clone();
            // the new cards report their periods after a few callbacks
            awra::glib::timeout_add_local_once(Duration::from_millis(400), move || show());
        })
    };
    {
        let (s, r, n_in) = (s.clone(), reopen.clone(), devs.inputs.clone());
        mic.connect_selected_notify(move |d| {
            {
                let mut st = s.borrow_mut();
                let i = d.selected() as usize;
                st.settings.no_input = i == n_in.len() + 1;
                st.settings.audio_input = if i == 0 || i > n_in.len() { None } else { Some(n_in[i - 1].clone()) };
            }
            r();
        });
    }
    {
        let (s, r, n_out) = (s.clone(), reopen.clone(), devs.outputs.clone());
        out.connect_selected_notify(move |d| {
            {
                let i = d.selected() as usize;
                s.borrow_mut().settings.audio_output = if i == 0 { None } else { n_out.get(i - 1).cloned() };
            }
            r();
        });
    }
    {
        let (s, r) = (s.clone(), reopen.clone());
        buf.connect_selected_notify(move |d| {
            s.borrow_mut().settings.buffer_frames = BUFFER_SIZES[d.selected() as usize % BUFFER_SIZES.len()].0;
            r();
        });
    }
    {
        let (s, show) = (s.clone(), show.clone());
        mode.connect_selected_notify(move |d| {
            {
                let mut st = s.borrow_mut();
                st.settings.monitor_mode = d.selected().min(2) as u8;
                st.engine.send(studio_engine::Cmd::MonitorMode(st.settings.monitor_mode));
                st.settings.save();
            }
            show();
        });
    }
    {
        let s = s.clone();
        hear.connect_active_notify(move |w| {
            let mut st = s.borrow_mut();
            st.settings.monitor_armed = w.is_active();
            st.settings.save();
        });
    }
    col
}

/// The header's microphone button: everything about the sound in one place.
pub fn popover(s: &S, anchor: &awra::Button, window: &awra::Window) {
    let pop = awra::Popover::new();
    let col = awra::Box::new(Orientation::Vertical, 10);
    col.set_margins(14, 14, 14, 14);
    col.set_size_request(400, -1);
    col.append(&label("Audio", Typography::Title3));
    col.append(&controls(s, 150));
    let more = super::button("Plus de réglages audio…", None, awra::ButtonAppearance::Ghost);
    more.set_halign(awra::Align::Start);
    let (s2, w2, p2) = (s.clone(), window.clone(), pop.clone());
    more.connect_clicked(move |_| {
        p2.popdown();
        super::settings_dialog::open_tab(&s2, &w2, 2);
    });
    col.append(&more);
    pop.set_child(Some(&col));
    pop.popup_pointing(anchor);
}
