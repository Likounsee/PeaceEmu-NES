# Architecture du module NES

## Objectif architectural

Ce dépôt implémente uniquement le matériel de la NES et son adaptateur vers PeaceEmu. L'application graphique et les services hôtes sont déjà développés dans [Likounsee/PeaceEmu](https://github.com/Likounsee/PeaceEmu) ; ils ne doivent pas être recodés ici.

## Frontière avec PeaceEmu

Le dépôt hôte expose un contrat commun `EmulationCore` dans `emulation-core`. Le cœur NES devra fournir un adaptateur qui respecte ce contrat :

- `descriptor()` : identifiant système stable, version du cœur et capacités réellement implémentées ;
- `load_content()` : valider le fichier de jeu avant de modifier l'état de la machine ;
- `reset()` : réinitialiser la machine selon le comportement documenté ;
- `run_until_event()` : exécuter jusqu'à une frontière observable ;
- `send_input()` : transmettre les transitions des manettes ;
- `take_video_frame()` et `take_audio_buffer()` : transférer les données au format commun de PeaceEmu ;
- sauvegardes et save states : ne déclarer ces capacités que lorsqu'elles sont implémentées et validées.

L'adaptateur est une couche mince. Le CPU, le bus, la PPU, l'APU, les cartouches et le timing restent dans le cœur NES, sans dépendance vers egui/eframe ni vers les widgets de l'application.

### Décision d'intégration à résoudre

Le crate `emulation-core` se trouve dans le workspace imbriqué du dépôt principal (`PeaceEmu/crates/core`) et n'est pas encore disponible ici comme dépendance versionnée publiée. Avant de prétendre que l'adaptateur est compilable avec l'hôte, choisir une stratégie de partage du contrat (par exemple extraire/versionner le crate commun ou fournir une dépendance Git correctement empaquetée). Ne pas copier les types en créant un faux contrat local, car deux types homonymes ne seraient pas compatibles à travers l'API Rust.

## Composants du cœur

### CPU Ricoh 2A03

Exécute les instructions du 6502 modifié pour la NES. Le composant gère les registres, les modes d'adressage, les flags, les interruptions et le comportement temporel. Le comportement décimal spécifique au 2A03 doit être testé explicitement.

### Bus mémoire

Centralise les accès CPU vers la RAM interne, les registres PPU/APU, les ports manette série ($4016/$4017) et la cartouche. Les manettes disposent d'un strobe, d'un latch des huit boutons et d'un registre à décalage ; l'entrée publique passe par `NesMachine::set_controller_state`. L'APU n'est pas encore implémentée.

### Cartouche et mapper

Valide les en-têtes iNES/NES 2.0, les tailles et les éventuels trainers avant d'exposer PRG-ROM, PRG-RAM, CHR-ROM/CHR-RAM et le mirroring. Commencer par NROM (mapper 0). Chaque mapper ultérieur doit disposer de tests spécifiques.

### PPU

Gère les registres vidéo, motifs, nametables, palettes, sprites, NMI et timing de balayage. Un rendu par instantané 256×240 existe, avec priorité OAM et approximations du sprite 0/overflow. Le compteur NTSC inclut le saut de dot du pré-rendu sur les images impaires lorsque le rendu est actif ; le pipeline dot-par-dot et les changements de scroll en cours de ligne restent à faire.

### APU

Gère les canaux audio NES et leur évolution dans le temps. Le cœur produit des échantillons ; PeaceEmu reste responsable de la lecture et de l'intégration audio côté hôte.

### Contrôleurs

Les deux ports de manette série sont implémentés au niveau du bus, avec huit bits dans l'ordre A/B/Select/Start/Up/Down/Left/Right. L'adaptateur PeaceEmu doit encore traduire les transitions d'entrée hôtes vers `set_controller_state`.

### Machine et ordonnanceur

Coordonne CPU, PPU, APU, cartouche et contrôleurs. Le modèle de timing doit être explicite, déterministe et testable.

## Règles d'implémentation

- Le cœur et ses composants ne dépendent pas de l'interface graphique.
- Valider le contenu non fiable avant toute mutation d'état.
- Préférer les tests unitaires déterministes pour le CPU, le bus et les cartouches.
- Documenter les hypothèses de timing et les comportements matériels particuliers.
- Chaque capacité annoncée dans les métadonnées doit correspondre à un comportement implémenté.
- Ne jamais inclure de ROM commerciale.

## Ordre recommandé

1. Crate Rust et conventions de tests.
2. CPU et tests d'instructions.
3. Bus et RAM.
4. Parsing de cartouche et NROM.
5. Machine minimale et tests d'intégration.
6. PPU et synchronisation.
7. Compléter le pipeline PPU et le timing synchronisé.
8. Ajouter l'APU, puis connecter les manettes à l'adaptateur hôte.
9. Adapter le contrat EmulationCore et étendre la compatibilité.
