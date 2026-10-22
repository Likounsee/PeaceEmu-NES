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

Centralise les accès CPU vers la RAM interne, les registres PPU/APU, les contrôleurs et la cartouche. Les miroirs d'adresses sont gérés par le décodage du bus, sans dupliquer les règles dans chaque composant.

### Cartouche et mapper

Valide les en-têtes iNES/NES 2.0, les tailles et les éventuels trainers avant d'exposer PRG-ROM, PRG-RAM, CHR-ROM/CHR-RAM et le mirroring. Commencer par NROM (mapper 0). Chaque mapper ultérieur doit disposer de tests spécifiques.

### PPU

Gère les registres vidéo, les motifs, nametables, palettes, sprites, NMI et timing de balayage. Le rendu et le timing doivent évoluer en synchronisation avec la machine ; ne pas traiter une instruction CPU comme une image entière.

### APU

Gère les canaux audio NES et leur évolution dans le temps. Le cœur produit des échantillons ; PeaceEmu reste responsable de la lecture et de l'intégration audio côté hôte.

### Contrôleurs

Le cœur reçoit des transitions d'entrée sémantiques depuis PeaceEmu et les convertit au protocole de lecture/sérialisation matériel des manettes NES.

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
7. Adaptateur hôte, contrôleurs, audio, puis compatibilité étendue.
