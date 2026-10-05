# Spécifications techniques et fonctionnelles : agent immersif de navigation et d'exploration os

## Résumé du projet

Le logiciel est une application Windows native en Rust permettant d'offrir une expérience immersive d'exploration de l'environnement numérique. Guidé par des directives ou un prompt utilisateur interprété par une IA, l'agent prend le contrôle partiel ou total de l'interface graphique :
- Génération d'événements système (souris, clavier, manipulation de fenêtres).
- Lancement de programmes, ouverture et orchestration d'onglets de navigateur.
- Affichage d'un overlay dynamique, transparent et interactif, étendu et mis à jour à la volée par l'IA.
- Exploration autonome guidée ("zones inconnues" du web et de l'environnement numérique).

## Architecture globale du système

L'application s'articule autour de 4 modules majeurs communicant via des canaux asynchrones (`tokio::sync::mpsc`) :

```text
┌────────────────────────────────────────────────────────┐
│                 Cerveau ia (Agent loop)                │
│  (Interprétation de prompts, planification, décisions) │
└───────────┬───────────────────────────────┬────────────┘
            │                               │
    [Actions système]              [Mises à jour UI]
            │                               │
            ▼                               ▼
┌───────────────────────────┐   ┌────────────────────────┐
│  Moteur d'automatisation  │   │   Overlay dynamique    │
│  - Windows API (Win32)    │   │   - Egui / WGPU        │
│  - Enigo / Input injection│   │   - Transparent        │
│  - Navigateur (CDP)       │   │   - Click-through      │
└───────────────────────────┘   └────────────────────────┘
```

---

## Modules principaux

### 1. Contrôle de l'interface et manipulation des fenêtres

Ce module interagit directement avec l'API Windows (via les crates `windows` ou `windows-sys`).

- **Événements d'entrée :** Injection d'événements bas niveau via l'API `SendInput` (souris, frappes clavier, raccourcis Windows).
- **Gestion des fenêtres :**
  - Enumération et ciblage des fenêtres (`EnumWindows`, `GetWindowTextW`).
  - Modification d'état : repositionnement (`SetWindowPos`), redimensionnement, focus (`SetForegroundWindow`), transparence (`SetLayeredWindowAttributes`).
- **Lancement de processus :** Exécution de programmes via `std::process::Command` ou `ShellExecuteW`.

### 2. Navigation web et exploration

Pour orchestrer les onglets de manière programmatique sans restriction :
- **Connexion CDP (Chrome DevTools Protocol) :** Lancement d'un navigateur basé sur Chromium (Edge/Chrome) avec le flag `--remote-debugging-port`.
- **Capacités :**
  - Création/fermeture dynamique d'onglets.
  - Navigation vers des URLs, injection de scripts JavaScript dans le DOM.
  - Défilement automatique, capture d'écran de pages et recherche de liens insolites ou méconnus.

### 3. Overlay extensible par l'ia

L'overlay fournit une couche visuelle immersive au-dessus de l'OS.

- **Propriétés de la fenêtre Win32 :**
  - Style étendu : `WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT`.
  - Bascule dynamique du mode *click-through* : la souris traverse l'overlay vers le bureau ou interagit avec l'overlay selon le contexte.
- **Rendu graphique :** Propulsé par `egui` via `wgpu` pour un affichage fluide et performant à 60+ FPS.
- **Extensibilité par l'IA :**
  - L'IA génère ou active des composants dynamiques (widgets textuels, boussole d'exploration, particules visuelles, annotations sur l'écran, sous-titres de narration).
  - Protocole déclaratif (JSON ou structure Rust désérialisable) permettant à l'IA d'ajouter, modifier ou détruire des éléments visuels à la volée.

### 4. Moteur d'orchestration ia

- **Cycle de décision (Loop ReAct) :**
  1. **Perception :** Capture d'écran partielle, analyse du contexte de la fenêtre active, état du DOM de la page en cours.
  2. **Raisonnement :** Évaluation par rapport au prompt d'origine.
  3. **Planification :** Génération d'une séquence de commandes atomiques (déplacement curseur, saisie, ouverture de page, mise à jour overlay).
  4. **Exécution & Feedback :** Traitement des actions par les modules dédiés.

---

## Schéma d'actions et protocole de commandes

L'IA émet des commandes typées que le runtime Rust exécute :

```rust
pub enum SystemAction {
    // Contrôle système & fenêtres
    OpenApplication { path: String, args: Vec<String> },
    FocusWindow { title_pattern: String },
    MoveWindow { title_pattern: String, x: i32, y: i32, width: i32, height: i32 },
    
    // Simulation d'entrées
    MouseMove { x: i32, y: i32, smooth: bool },
    MouseClick { button: MouseButton },
    SendKeys { sequence: String },
    
    // Navigation Web
    NavigateUrl { url: String, new_tab: bool },
    ScrollPage { delta_y: i32 },
    
    // Overlay UI
    ShowNarration { text: String, duration_ms: u64 },
    HighlightArea { x: i32, y: i32, w: i32, h: i32, color: [u8; 4] },
    SpawnVisualEffect { effect_type: VisualEffectType, coords: (i32, i32) },
    ClearOverlay,
}
```

---

## Sécurité, garde-fous et arrêt d'urgence

Puisque l'IA dispose de droits d'automatisation sur le système :
- **Kill-switch matériel :** Enregistrement d'un hook clavier global bas niveau (`WH_KEYBOARD_LL`). L'appui sur une combinaison d'urgence (ex: `Ctrl + Shift + Echap` ou appui maintenu sur `Echap`) interrompt immédiatement l'exécution de l'agent et restaure la main à l'utilisateur.
- **Zone de confinement (Sandbox logique) :**
  - Liste blanche/noire d'exécutables (interdiction de manipuler des invites de commande privilégiées comme `powershell.exe` avec droits admin sans confirmation).
  - Blocage des actions destructives (suppression de fichiers système, accès à des répertoires sensibles).

---

## Dépendances rust recommandées

```toml
[dependencies]
# Asynchrone
tokio = { version = "1", features = ["full"] }

# Intégration Windows
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_Graphics_Gdi"
] }

# Simulation d'événements utilisateur
enigo = "0.2"

# Overlay et Interface Graphique
eframe = "0.28"
egui = "0.28"

# Navigation et Automatisation Web (CDP)
chromiumoxide = "0.5"

# Échange de données et IA
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
reqwest = { version = "0.12", features = ["json", "stream"] }
```

---

## Phases de développement

1. **Phase 1 : Socle technique de l'overlay transparent**
   - Création de la fenêtre transparente Win32 plein écran avec support du click-through.
   - Rendu de base avec `egui`.

2. **Phase 2 : Moteur d'injection et contrôle d'entrées**
   - Implémentation du kill-switch d'urgence.
   - Envoi de frappes et gestion du déplacement du curseur.
   - Manipulation de fenêtres tierces (taille, focus).

3. **Phase 3 : Passerelle de navigation web**
   - Initialisation du navigateur avec support CDP.
   - Commandes pour ouvrir des onglets et injecter des navigations.

4. **Phase 4 : Boucle agentique et intégration ia**
   - Consommation d'une API LLM (ex: Claude, OpenAI, modèle local Ollama).
   - Parsing des appels d'outils (*tool calls*) vers les variantes de l'enum `SystemAction`.
   - Enrichissement des capacités visuelles de l'overlay par l'IA.