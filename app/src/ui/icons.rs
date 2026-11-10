//! Icons Awra Studio adds to the theme (symbolic, drawn in the text colour).

const RECORD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="5.5" fill="#000"/></svg>"##;
const MIC: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="5.5" y="1" width="5" height="8.5" rx="2.5" fill="#000"/><path d="M3 7h1.4a3.6 3.6 0 0 0 7.2 0H13a5 5 0 0 1-4.3 4.95V13.5H11V15H5v-1.5h2.3v-1.55A5 5 0 0 1 3 7z" fill="#000"/></svg>"##;
const SPARKLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path d="M7 1.5l1.3 3.7 3.7 1.3-3.7 1.3L7 11.5 5.7 7.8 2 6.5l3.7-1.3z" fill="#000"/><path d="M12.5 9.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z" fill="#000"/></svg>"##;
const LAYOUT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path fill-rule="evenodd" d="M3 2h10a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zm0 1.5a.5.5 0 0 0-.5.5v8a.5.5 0 0 0 .5.5h6v-9zm7.5 0v9H13a.5.5 0 0 0 .5-.5V4a.5.5 0 0 0-.5-.5zM3 8.2h5.5v1.3H3z" fill="#000"/></svg>"##;
const PEOPLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="5.5" cy="5" r="2.5" fill="#000"/><circle cx="11" cy="5.5" r="2" fill="#000"/><path d="M1 13.5c0-2.5 2-4 4.5-4s4.5 1.5 4.5 4z" fill="#000"/><path d="M10.6 13.5c0-1.4-.4-2.6-1.2-3.4.5-.3 1-.4 1.6-.4 2 0 3.5 1.3 3.5 3.8z" fill="#000"/></svg>"##;
const WAVE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><g fill="#000"><rect x="1" y="6.5" width="1.5" height="3" rx=".7"/><rect x="4" y="4" width="1.5" height="8" rx=".7"/><rect x="7" y="1.5" width="1.5" height="13" rx=".7"/><rect x="10" y="4.5" width="1.5" height="7" rx=".7"/><rect x="13" y="6" width="1.5" height="4" rx=".7"/></g></svg>"##;
const METRONOME: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path fill-rule="evenodd" d="M5.6 1h4.8l3.4 14H2.2zm1.1 1.5L4.1 13.5h7.8L9.3 2.5z" fill="#000"/><path d="M7.4 10.4l4.3-8 1.1.6-4.3 8z" fill="#000"/><rect x="4.5" y="10.2" width="7" height="1.3" fill="#000"/></svg>"##;
const KNOB: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><g fill="#000"><rect x="2" y="2" width="1.4" height="12" rx=".7"/><rect x="7.3" y="2" width="1.4" height="12" rx=".7"/><rect x="12.6" y="2" width="1.4" height="12" rx=".7"/><rect x="0.7" y="9" width="4" height="2.4" rx="1"/><rect x="6" y="4" width="4" height="2.4" rx="1"/><rect x="11.3" y="7" width="4" height="2.4" rx="1"/></g></svg>"##;
const EXPORT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path d="M8 1l4 4H9.2v5H6.8V5H4z" fill="#000"/><path d="M2 9h1.6v4h8.8V9H14v5.6H2z" fill="#000"/></svg>"##;
const PLUG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="4.2" y="1" width="1.6" height="4" rx=".8" fill="#000"/><rect x="10.2" y="1" width="1.6" height="4" rx=".8" fill="#000"/><path d="M3 5h10v2.5a5 5 0 0 1-10 0z" fill="#000"/><rect x="7.2" y="11" width="1.6" height="4" fill="#000"/></svg>"##;
const LYRICS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><g fill="#000"><rect x="1.5" y="2.5" width="9" height="1.4" rx=".7"/><rect x="1.5" y="6" width="7" height="1.4" rx=".7"/><rect x="1.5" y="9.5" width="5" height="1.4" rx=".7"/><rect x="11.8" y="3" width="1.4" height="8.5"/><circle cx="11" cy="11.5" r="2"/></g></svg>"##;

pub fn register() {
    for (name, svg) in [
        ("awra-studio-record-symbolic", RECORD),
        ("awra-studio-mic-symbolic", MIC),
        ("awra-studio-ai-symbolic", SPARKLE),
        ("awra-studio-layout-symbolic", LAYOUT),
        ("awra-studio-people-symbolic", PEOPLE),
        ("awra-studio-wave-symbolic", WAVE),
        ("awra-studio-metronome-symbolic", METRONOME),
        ("awra-studio-mixer-symbolic", KNOB),
        ("awra-studio-export-symbolic", EXPORT),
        ("awra-studio-plugin-symbolic", PLUG),
        ("awra-studio-lyrics-symbolic", LYRICS),
    ] {
        awra::icon_register_svg(name, svg);
    }
}
