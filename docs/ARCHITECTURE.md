<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Awra Studio — architecture

```
studio/
  crates/
    studio-core      modèle du projet, opérations inversibles, historique, persistance
    studio-dsp       buffers, décodage, effets, autotune, débruiteur, loudness, graphe de mix
    studio-analysis  analyses (loudness, spectre, justesse, tonalité, tempo, RT60), références
    studio-engine    moteur temps réel (cpal) : lecture, monitoring, enregistrement
    studio-ai        ingénieur IA : outils, savoir-faire, ingénieur local, fournisseurs LLM
    studio-collab    sessions P2P : hôte/invités, chiffrement, ordre, fichiers, transport
    studio-plugins   hébergement CLAP et VST3
  app/               l'application Awra (interface, orchestration)
  test-plugins/      un plugin CLAP minimal pour tester l'hôte
```

## Un seul chemin pour changer le morceau : les opérations

Tout changement — un fader bougé, une chaîne posée par l'IA, une modif
reçue d'un ami — est une `Transaction` d'`Op`s (`studio-core::ops`).
Appliquer une op renvoie son inverse ; donc :

- **annuler / rétablir** est générique, y compris « annuler cette action de
  l'IA » au milieu d'autres actions (`Document::undo_transaction`) ;
- l'**IA** produit des transactions étiquetées `Actor::Ai` ;
- la **collaboration** échange des transactions.

Les positions sont en secondes (f64) ; le moteur convertit en échantillons.
Les fichiers audio sont des *assets* identifiés par leur empreinte BLAKE3.

## Le son

`studio-dsp::graph::MixGraph` est le même graphe pour l'écoute et l'export :
clips → chaîne d'effets de la piste → volume / panoramique (loi à puissance
constante) → chaîne du master. Les effets implémentent `Processor`
(stéréo, en place, sans allocation dans `process`).

Le moteur (`studio-engine`) possède le graphe dans le callback audio.
L'interface construit un nouveau graphe à chaque changement de structure et
l'envoie par canal ; le callback **adopte** les processeurs existants par
identifiant d'effet (les queues de réverbe et les instances de plugins
survivent) et renvoie l'ancien graphe pour qu'il soit libéré hors du
thread audio. Les changements légers (paramètres, volumes) passent en
messages `Cmd::Light`. Sans carte son, un thread minuté joue le même rôle.

L'**autotune** détecte la hauteur (YIN, à fréquence réduite, toutes les
~10 ms), choisit la note de la gamme la plus proche avec hystérésis, glisse
vers la correction à la vitesse choisie (1 ms = effet robotique, 250 ms =
correction naturelle) et pilote un pitch-shifter à deux têtes de lecture
en fondu complémentaire.

## L'ingénieur IA

`studio-ai::host::StudioHost` est ce que l'application offre à l'agent :
lire le projet, analyser (pistes brutes ou traitées, mix, références),
mesurer la loudness d'une variante du projet, appliquer une transaction,
changer l'interface, lister les plugins, annuler.

- `tools.rs` : 22 outils décrits en JSON Schema (fonction calling) et leur
  exécution. Les outils de haut niveau (`build_vocal_chain`, `balance_mix`,
  `master`, `match_reference`, `finish_song`, `create_variants`)
  encapsulent le savoir-faire de `engineer.rs` (chaînes selon l'analyse et
  le style, équilibre par loudness, mastering calibré par mesures
  successives).
- `local.rs` : l'ingénieur hors ligne qui comprend les demandes courantes
  (FR/EN) et appelle les mêmes outils.
- `agent.rs` : la boucle avec les modèles — API Anthropic Messages, API
  OpenAI Chat Completions (et serveurs compatibles), ponts vers les outils
  officiels liés à l'abonnement de l'utilisateur (Codex CLI pour « Sign in
  with ChatGPT », Claude Code) par un protocole JSON.

Dans l'application, l'agent tourne sur un thread de travail ;
`aihost::ProxyHost` envoie ses demandes au thread de l'interface
(instantanés, commits, interface) et fait les rendus / analyses lui-même à
partir d'instantanés.

## La collaboration

`studio-collab` : un participant héberge (TCP, port ouvert sur la box par
UPnP si possible), les autres rejoignent avec un code `AWRA-…` qui contient
les adresses de l'hôte et un secret de 80 bits. Chaque connexion dérive sa
clé (BLAKE3 sur le secret et deux aléas) ; les trames sont chiffrées
ChaCha20-Poly1305. L'hôte attribue un numéro d'ordre à chaque transaction
et la diffuse ; chacun applique ses propres changements immédiatement et,
quand un changement d'autrui passe avant les siens, les retire, applique
celui d'autrui puis les ré-applique (`Replica`) : tout le monde converge.
Les assets voyagent en morceaux de 256 Kio, relayés par l'hôte ; la lecture
démarre ensemble grâce à une estimation de l'écart d'horloge (ping).
Découverte sur le réseau local par diffusion UDP. Aucun serveur.

## Les plugins

`studio-plugins` : CLAP (`clap-sys`) et VST3 (crate `vst3`, liaisons
générées depuis le SDK). Chaque fichier est scanné dans un processus enfant
(`awra-studio --scan-plugin`). Le `PluginManager` sert de fabrique au graphe
de mix, garde les instances par emplacement d'effet (paramètres, état,
fenêtre des plugins CLAP qui en ont une).

## L'interface et Awra

L'application est construite avec Awra. Ce dont elle avait besoin et qui
sert à d'autres applications est entré dans le framework :

- `awra::Dock` : un espace de travail de panneaux décrit par un arbre
  sérialisable (`h(0.7,timeline,v(0.5,lyrics,assistant))`), modifiable par
  le code (et donc par l'IA) ou à la main en mode édition ;
- `awra::LevelMeter` : vumètre stéréo avec maintien de crête ;
- `Canvas::text` / `Canvas::text_width` (et `awra_canvas_text_width` en C).

Le reste (timeline, cartes d'effets, assistant…) est propre au studio.
Chaque panneau implémente `ui::Panel` (`widget`, `refresh`, `tick`) ; un
minuteur à ~30 Hz fait avancer le studio (`Studio::tick`) puis rafraîchit
ce qui a changé.
