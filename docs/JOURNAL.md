# Awra Studio — journal de développement

Ce fichier est la mémoire du projet : décisions, état d'avancement, pièges.
Il est mis à jour à chaque étape importante (il doit suffire pour reprendre
le travail après une perte de contexte).

## Contexte et contraintes

- Demande : développer intégralement « Awra Studio » (studio musical natif,
  Rust + framework Awra, IA agentique, collaboration P2P, plugins VST3/CLAP,
  Linux Fedora/Wayland + Windows). Travail en autonomie totale.
- Branche imposée : `claude/awra-studio-project-78gw3w` du dépôt
  `MehdiSenhajYnov/Awra`. Impossible de créer un nouveau dépôt GitHub depuis
  cette session : le studio vit dans `studio/`, un workspace Cargo autonome
  qui dépend d'Awra par chemin relatif (`../bindings/rust/awra`). Il peut être
  extrait tel quel dans son propre dépôt (il suffit de remplacer le chemin
  de la dépendance `awra` par un `git = ...`).
- Awra 2 = bibliothèque C (BUI, rendu logiciel, Wayland/X11/Win32) + crate
  Rust `awra` à l'API style gtk-rs. Compile avec CMake via `awra-sys`.
  Dépendances système Linux : `wayland-client`, `wayland-cursor`,
  `xkbcommon` (+ `alsa` pour l'audio du studio).
- Test headless : `awra::Window::offscreen(title, w, h, scale)` +
  `render_offscreen` + snapshot PNG → captures d'écran sans affichage.

## Décisions d'architecture (prises en autonomie)

1. **Un seul langage de mutation : les `Op`** (`studio-core::ops`). L'UI,
   l'agent IA et la collaboration modifient le projet exclusivement via des
   transactions d'`Op`. Chaque op appliquée renvoie son inverse → undo/redo
   gratuit, interventions de l'IA annulables, et synchronisation P2P par
   simple diffusion des transactions.
2. **Positions en secondes (f64)** dans le modèle ; le moteur convertit en
   échantillons à la fréquence du projet (48 kHz par défaut).
3. **DSP natif Rust** (`studio-dsp`) : EQ paramétrique, compresseur, gate,
   de-esser, reverb, delay, saturation, autotune (YIN + pitch-shift
   synchrone), limiteur, doubleur, largeur stéréo, débruiteur. Même graphe
   (`MixGraph`) pour le temps réel et l'export hors-ligne.
4. **Plugins** VST3/CLAP via `studio-plugins` qui fournit des `Processor`
   au `MixGraph` par une fabrique injectée (le DSP ne dépend pas des
   plugins).
5. **IA** (`studio-ai`) : agent avec outils (analyse, effets, mix,
   mastering, interface). Fournisseurs : OpenAI (clé API), Anthropic (clé
   API), modèles locaux compatibles OpenAI (Ollama, LM Studio, llama.cpp),
   et « ponts CLI » vers les outils officiels déjà connectés à
   l'abonnement de l'utilisateur (Codex CLI = Sign in with ChatGPT,
   Claude Code). Un **ingénieur local** déterministe (règles + analyse)
   fonctionne sans aucun modèle.
6. **Collaboration P2P** (`studio-collab`) : un participant héberge (TCP),
   les autres rejoignent avec un code d'invitation (adresse + secret).
   Chiffrement ChaCha20-Poly1305 avec clé dérivée du secret, découverte
   LAN par UDP broadcast, ouverture de port UPnP optionnelle. L'hôte
   séquence les transactions (ordre total), relaie les fichiers audio par
   empreinte, synchronise le transport (horloge estimée par ping).
   Aucun serveur central.
7. **Interface personnalisable** : arbre de disposition sérialisable
   (splits / onglets / panneaux) reconstruit en widgets Awra ; éditable
   à la main (mode édition) et par l'IA (ops de disposition).

## État d'avancement

Toutes les briques sont livrées et testées (voir README.md pour l'usage,
ARCHITECTURE.md pour la conception). Pistes pour la suite :
- éditeur graphique VST3 intégré (IPlugView) ; fenêtres CLAP embarquées ;
- comping de prises ;
- voix temps réel entre participants (Opus) et relais de NAT (STUN) ;
- export MP3/FLAC ; stems ;
- assistant créatif (toplines, flows, harmonies) en plus de l'ingénieur.

(voir la section « Journal » plus bas, la plus récente en haut)

## Journal

- Passe UI/UX (demande : bouton « + Audio » qui débordait, fond opaque,
  transparence réglable par élément, micro-animations, glisser-déposer) :
  - Timeline réécrite (`ui/timeline.rs`) : une pilule « + Ajouter une
    piste » dans la colonne d'en-têtes (menu Voix / Adlibs / Audio / Instru
    / Importer) remplace la rangée de boutons qui débordait ; survols,
    curseurs, fantôme + ombre + ligne d'accroche au glisser de clip, zoom
    et défilements adoucis, suivi de lecture lissé, formes d'onde en fondu,
    surbrillance des nouvelles pistes, pastille R qui pulse. Projet vide :
    « Importer une instru… » / « Essayer la démo ».
  - Glisser-déposer externe : Awra donne la position pendant le survol
    (DROP_HOVER) mais pas les fichiers → aperçu générique (piste visée
    encadrée, clip fantôme, « Ajouter à « X » · mm:ss », ou rangée « +
    Nouvelle piste »), puis décodage en arrière-plan
    (`Studio::import_files` / `pump_imports`, imports en cours visibles).
    Test `dropped_files_land_on_a_track_or_a_new_one`.
  - Apparence (`ui/appearance.rs`, onglet « Apparence » des réglages) :
    thème, accent, flou, préréglages Verre / Verre léger / Tout opaque,
    opacité de 6 parties. S'appuie sur la nouvelle API Awra
    `awra_style_set_material_opacity` (la teinte est composée sur le
    canvas pour qu'une carte opaque garde la couleur du thème) ; fond des
    panneaux dessiné par l'app (`panel_background`).
  - Effets : palette d'effets (nom + description) au lieu d'une liste
    déroulante, chaînes rapides qui passent à la ligne (FlowBox), réglages
    avancés dépliés par un Revealer (sans reconstruction), cartes ajoutées
    qui glissent. Assistant : messages ajoutés un à un en glissant,
    indicateur de frappe, défilement adouci. Dock : fondus entre
    dispositions et onglets. Bouton d'enregistrement qui « respire ».
  - Awra (main `2c43dfa`) : opacité des matériaux, raccourcis de fenêtre
    qui ne volent plus les touches tapées dans un champ (Espace/R/S),
    transitions du Dock. `rev` mis à jour dans `Cargo.toml`.
  - Captures : `awra-studio --screenshot x.png --demo [--dark]
    [--appearance opaque|clear|panels] [--settings] [--drop-hover x,y]`.

- Le studio a désormais son propre dépôt (`MehdiSenhajYnov/AwraStudio`,
  branche `main`), extrait avec son historique (`git subtree split`) du
  dossier `studio/` de la branche `claude/awra-studio-project-78gw3w`
  d'Awra (le dossier y a ensuite été supprimé). Awra est une dépendance
  git épinglée sur `main` (rev `68a0eb9`, qui contient Dock / LevelMeter /
  texte de canvas et le correctif child-visible). Pour faire évoluer le framework : modifier Awra,
  pousser, puis mettre à jour `rev` dans `Cargo.toml` (`cargo update -p awra`).
  Les chemins `studio/…` plus bas datent d'avant l'extraction.

- Passe de finition (54 tests, 0 avertissement) : correctif framework
  (un enfant retiré redevient child-visible, comme gtk_widget_unparent ;
  panneaux du Dock rétrécissables), détection de tonalité par pics
  spectraux sur FFT 16384 (test sur une grille d'accords), autotune calé
  automatiquement sur la tonalité de l'instru, tests des boucles d'agent
  contre un faux serveur HTTP (Anthropic, OpenAI-compatible, repli local,
  pont CLI), menu de piste, boucle (Maj+glisser la règle), sauvegarde auto,
  suivi de la tête de lecture, connexion de session en arrière-plan,
  arrêt propre de l'hôte. Build Windows MinGW croisé testé sous Wine.

- App fonctionnelle et testée : `cargo test -p awra-studio` (4 tests :
  edits/undo/moteur, tour d'IA via worker, export WAV, DEUX studios en
  session loopback). Captures headless :
  `awra-studio --screenshot out.png --demo [--layout mixing] [--select 1]
  [--ask "…"] [--dark] [--size WxH]` (force AWRA_STUDIO_NO_AUDIO).
  Captures de référence dans `studio/docs/screenshots/`.
  Pièges rencontrés : SVG d'icônes → pas de `stroke` (le moteur remplit),
  utiliser des chemins pleins ; tableaux `[a.as_ref(), …]` ambigus →
  helper `ui::wd(&w)` ; `&ProcessorFactory` exige une closure 'static
  (cloner le PluginManager).

- Framework Awra enrichi (commit séparé) : `awra::Dock` (espace de travail
  à panneaux, arbre split/onglets sérialisable `h(0.7,a,v(0.5,b,c))`,
  `t(0,a,b)`, mode édition avec barre déplacer/agrandir/masquer),
  `awra::LevelMeter`, `Canvas::text` / `text_width` (+ C
  `awra_canvas_text_width`, awra-sys régénéré par tools/gen_rust_sys.py).
- App `studio/app` EN COURS : `studio.rs` (struct Studio : doc, assets,
  engine, plugins, session+replica, IA via `aihost.rs` [worker thread +
  ProxyHost, requêtes Snapshot/Commit/Layout/UndoAi servies dans `tick()`]),
  `assets.rs` (décodage en arrière-plan + Peaks), `settings.rs`
  (config_dir()/awra-studio/settings.json, `AWRA_STUDIO_CONFIG` pour
  surcharger), `demo.rs` (projet démo synthétisé), `ui/mod.rs` (Ui : Dock,
  panneaux trait `Panel{widget,refresh,tick}`, timer 33 ms `frame()`,
  garde `updating()` contre la ré-entrance des signaux, presets
  d'interface PRESETS, `apply_layout(LayoutCommand)`).

- `studio-plugins` terminé : CLAP via `clap-sys` 0.5 (scan, instance,
  params, état, timers hôte, fenêtre flottante x11/win32), VST3 via le
  crate `vst3` 0.3 (bindings générés du SDK : composant, contrôleur
  séparé + IConnectionPoint, IBStream mémoire, IParameterChanges maison,
  état ; pas d'éditeur VST3 intégré — paramètres affichés par le studio).
  `PluginManager::create(slot, sr, block)` sert de fabrique au MixGraph ;
  scan hors-processus via `awra-studio --scan-plugin <fichier>` (JSON).
  Test d'intégration avec un vrai plugin CLAP (`test-plugins/awra-test-gain`).
  Clés des paramètres plugin dans `EffectSlot.params` = id numérique en
  texte (CLAP : valeur réelle ; VST3 : normalisée 0..1).

- `studio-collab` terminé (5 tests : loopback hôte/invité, mauvais code
  refusé, convergence de modifs concurrentes). API : `Session::host(name,
  port, upnp)` → `invite` (code `AWRA-<adresses base32>-<secret 16 car.>`),
  `Session::join(code, name)`, `poll()` → `Event` (Welcome, Remote, Ack,
  WantsProject [l'hôte répond `welcome(peer, project, seq)`], WantsAsset,
  Asset, Transport{delay_ms}, Chat, Peers…). `Replica` (rebase des modifs
  locales non acquittées via inverses) : l'app DOIT passer toutes ses
  modifs par `replica.local(doc, tx)` puis `session.propose(tx)`.
  `Document::commit_inv` ajouté au core. Découverte LAN par UDP 47801.

- `studio-ai` terminé (10 tests dont 3 d'intégration : document réel →
  rendu → analyse → actions → undo). Points clés :
  - `host::StudioHost` = ce que l'app doit fournir (project, analyses,
    measure_lufs, commit, layout, plugins, undo_ai). Appelé depuis un
    thread de travail dans l'app (proxy par canaux vers le thread UI).
  - `tools.rs` : 22 outils JSON (get_project, analyze_*, set_track,
    add/set/remove_effect, build_vocal_chain, balance_mix, master,
    match_reference, create_variants, apply_variant, finish_song,
    set_layout, set_song_info, add_lyrics, list/add_plugin, undo_last_ai).
  - `local.rs` : ingénieur hors-ligne (règles FR/EN → mêmes outils) ;
    comprend tous les exemples de la vision (test dédié).
  - `agent.rs` : Anthropic Messages API (en-tête anthropic-version
    2026-10-21, modèle par défaut claude-opus-5-5, `fallbacks: "default"`
    + beta server-side-fallback-2026-07-01, blocs assistant renvoyés tels
    quels), OpenAI Chat Completions (+ compatibles : Ollama par défaut
    http://localhost:11434/v1), ponts CLI Codex (`codex exec`, connexion
    `codex login` = Sign in with ChatGPT) et Claude Code (`claude -p`)
    via protocole JSON {say, calls, done}. En cas d'échec → ingénieur local.

- Crates terminées et testées : `studio-core` (7 tests), `studio-dsp`
  (16 tests : effets, autotune qui ramène 450 Hz → 440 Hz, débruiteur STFT,
  loudness BS.1770, graphe de mix), `studio-analysis` (5 tests : tempo,
  tonalité, RT60, matching de référence), `studio-engine` (cpal 0.15,
  thread « null » si pas de carte son ; `AWRA_STUDIO_NO_AUDIO=1` force ce
  mode). Versions épinglées volontairement anciennes et connues : cpal 0.15,
  symphonia 0.5, rtrb 0.3.

- 2026-11-10 : exploration d'Awra, build OK (`cargo build --example smoke`,
  backend x11 headless). Création du workspace `studio/`.
