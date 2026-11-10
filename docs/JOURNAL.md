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

(voir la section « Journal » plus bas, la plus récente en haut)

## Journal

- Crates terminées et testées : `studio-core` (7 tests), `studio-dsp`
  (16 tests : effets, autotune qui ramène 450 Hz → 440 Hz, débruiteur STFT,
  loudness BS.1770, graphe de mix), `studio-analysis` (5 tests : tempo,
  tonalité, RT60, matching de référence), `studio-engine` (cpal 0.15,
  thread « null » si pas de carte son ; `AWRA_STUDIO_NO_AUDIO=1` force ce
  mode). Versions épinglées volontairement anciennes et connues : cpal 0.15,
  symphonia 0.5, rtrb 0.3.

- 2026-11-10 : exploration d'Awra, build OK (`cargo build --example smoke`,
  backend x11 headless). Création du workspace `studio/`.
