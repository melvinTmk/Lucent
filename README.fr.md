<p align="center"><img src="docs/logo.png" width="120" alt="Logo Lucent"></p>

<h1 align="center">Lucent</h1>

<p align="center"><b>Windows, enfin beau.</b></p>

**Un dock, un lanceur et des outils de fenêtres façon macOS pour Windows.** Lucent
remplace la barre des tâches Windows par un dock en verre flottant, et ajoute une
recherche façon Spotlight, un sélecteur d'apps, une vue des fenêtres, l'ancrage des
fenêtres, un centre de contrôle et un Mode Jeu qui se fait oublier quand tu joues.

[English](README.md) · [Télécharger](../../releases/latest) · [Licence : GPL-3.0](LICENSE)

<!-- Ajouter des captures ici : docs/screenshots/dock.png, search.png, control-center.png -->

## Fonctionnalités

**Dock**
- Îlots en verre flottants (flou natif de Windows), un dock par écran, en bas, à gauche ou
  à droite.
- Apps épinglées et ouvertes avec leurs vraies icônes, effet loupe, réorganisation par
  glisser-déposer, fichiers déposés sur une icône ouverts avec cette app.
- Aperçu des fenêtres au survol, pile Téléchargements, corbeille avec « Vider ».
- Icônes de la zone de notification, volume, réseau et horloge dans le dock ; contrôle de
  la musique en cours.
- Masquage automatique avec délais réglables, ou **toujours visible** (le dock réserve
  alors sa place comme une barre des tâches, et se cache quand même en plein écran et en
  jeu).
- Cache la barre des tâches Windows sans risque : un processus de garde la remet si
  Lucent est tué ou plante, et la désinstallation la remet aussi.

**Recherche** (`Alt+Espace`)
- Apps, fichiers, projets WSL (ouverts dans VS Code), éléments récents.
- Calculatrice, recherche web (`? requête`), commandes d'alimentation (verrouiller, veille,
  redémarrer…), `kill app`.
- Historique du presse-papiers (`cb`), emojis (`:sourire`), aperçu rapide (`Maj+Espace`).

**Fenêtres**
- Sélecteur d'apps groupé par app (`Alt+²`, réglable).
- Vue de toutes les fenêtres façon Mission Control (`Ctrl+Alt+Espace`).
- Ancrage : `Ctrl+Alt+←/→` moitiés, `↑` agrandir, `↓` centrer, `D/F/G` tiers,
  `+Maj+←/→` écran voisin.
- Coins actifs (vue des fenêtres, bureau, verrouiller, recherche, sélecteur).

**Centre de contrôle** (clic sur l'horloge)
- Wi-Fi, Bluetooth, micro coupé, mode sombre, luminosité de l'écran (DDC/CI), volume et
  sortie audio, menu d'alimentation.
- **Écran partagé** : bascule un écran branché à deux ordinateurs sur l'entrée de l'autre
  PC (DDC/CI), avec un raccourci (`Ctrl+Alt+O`). L'écran quitte le bureau de ce PC tant
  qu'il affiche l'autre : les fenêtres et la souris restent sur tes autres écrans.

**Mode Jeu**
- Détecte les jeux (bibliothèques Steam, Epic, Riot, plus ta propre liste), écarte le dock
  et les raccourcis, mode d'alimentation Performances élevées en option, petite pastille
  de suivi (CPU, GPU, RAM, ping) sur les autres écrans.
- **Valorant** : une carte de fin de partie dans le dock (map, agent, score, K/D/A, HS %,
  ACS, RR et progression vers le rang suivant), ton rang et tes dernières parties dans la
  recherche (`valo`). Nécessite une [clé API HenrikDev](https://docs.henrikdev.xyz/)
  gratuite.

**Fiabilité**
- Se relance tout seul après un plantage, attend que Windows soit prêt à l'ouverture de
  session, tient un journal dans `%LOCALAPPDATA%\Lucent\logs\lucent.log`.

## Installation

1. Télécharge `Lucent_x.y.z_x64-setup.exe` depuis la [dernière version](../../releases/latest).
2. Lance-le. L'installation se fait pour ton compte seulement (pas besoin d'être admin).
   L'installeur n'est pas encore signé : Windows SmartScreen peut afficher un
   avertissement, clique sur « Informations complémentaires » → « Exécuter quand même ».
3. Lucent se lance à l'ouverture de session par défaut. Clic droit sur le dock →
   Réglages pour le configurer.

**Configuration requise :** Windows 11 (développé et testé dessus ; Windows 10 1903+ peut
fonctionner), le runtime Microsoft Edge WebView2 (déjà installé sur Windows 11).

**Désinstallation :** Paramètres → Applications → Lucent → Désinstaller. La barre des
tâches Windows revient. Si elle reste cachée, lance `lucent.exe --restore-taskbar` depuis
le dossier d'installation (`%LOCALAPPDATA%\Lucent`), ou désactive « Masquer
automatiquement la barre des tâches » dans les paramètres Windows.

## Vie privée

Pas de télémétrie, pas de compte. Lucent ne se connecte à Internet que pour les
fonctions que tu actives :

- Valorant : `api.henrikdev.xyz` (ton Riot ID et ta propre clé API) et `valorant-api.com`
  (images des maps, agents et rangs).
- Recherche web : l'adresse que tu choisis (Google par défaut), ouverte dans ton
  navigateur.

Les réglages sont dans `%APPDATA%\Lucent\settings.json`, le cache et le journal dans
`%LOCALAPPDATA%\Lucent`.

## Compiler depuis les sources

Voir la section [Build from source](README.md#build-from-source) du README anglais
(commandes identiques : `npm ci`, `npm run tauri dev`, `npm run tauri build`).

## Contribuer

Les signalements de bugs, idées et pull requests sont les bienvenus. Lis
[CONTRIBUTING.md](CONTRIBUTING.md) avant. Pour un bug, joins la fin du journal
`%LOCALAPPDATA%\Lucent\logs\lucent.log`.

## Licence

Lucent est un logiciel libre sous licence GNU GPL version 3 ou ultérieure. Voir
[LICENSE](LICENSE).

Lucent n'est affilié ni à Apple, ni à Microsoft, ni à Riot Games. Les données Valorant
viennent de l'API non officielle HenrikDev et de valorant-api.com.
