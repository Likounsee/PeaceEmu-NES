# Architecture du module NES

## Objectif architectural

Séparer les responsabilités afin que chaque composant puisse être développé et testé sans dépendre d'une interface utilisateur particulière.

## Composants

### CPU (Ricoh 2A03)

Responsable de l'exécution des instructions 6502 adaptées à la NES. Le composant devra gérer les registres, les modes d'adressage, les flags, les cycles, les interruptions et les particularités du processeur NES. Les instructions décimales du 6502 ne fonctionnent pas comme sur un 6502 standard : ce point doit être testé explicitement.

### Bus mémoire

Centralise les accès mémoire du CPU et leur routage vers la RAM interne, les registres PPU/APU, les contrôleurs et la cartouche. Le mirroring de la RAM et des registres doit être traité par le décodage d'adresses, pas dupliqué dans chaque composant.

### Cartouche et mapper

Lit et valide l'en-tête iNES/NES 2.0, puis expose PRG-ROM, PRG-RAM, CHR-ROM/CHR-RAM et les règles de mirroring. Commencer par NROM (mapper 0), puis ajouter d'autres mappers seulement avec des tests dédiés.

### PPU

Gère les registres vidéo, la mémoire de motifs, les nametables, les palettes, les sprites, les interruptions NMI et le timing de balayage. Le rendu final doit être synchronisé avec l'exécution du CPU.

### APU

Gère les canaux audio de la NES et leur évolution dans le temps. L'interface audio ne doit pas imposer de dépendance au cœur de l'émulation.

### Contrôleurs

Expose l'état des boutons selon le protocole de lecture/sérialisation de la NES. Le frontend fournit l'état des entrées ; le cœur décide de la valeur lue par le CPU.

### Machine / horloge

Coordonne CPU, PPU, APU, cartouche et contrôleurs. La synchronisation doit être explicite et reproductible ; éviter une boucle « une instruction = une image ».

### Frontend

Gère l'ouverture des fichiers ROM, la fenêtre, les contrôles, l'audio et l'affichage. Il ne doit pas contenir de logique propre au CPU ou à la PPU.

## Règles d'implémentation

- Définir des interfaces et des types partagés avant de multiplier les dépendances.
- Privilégier des tests unitaires déterministes pour le CPU, le bus et le parsing de cartouches.
- Documenter les hypothèses de timing et les cas matériels particuliers.
- Toute nouvelle fonctionnalité doit préciser ses critères d'acceptation et les tests associés.
- Ne jamais inclure de ROM commerciale dans le dépôt.

## Décisions en attente

- Langage et version minimale du compilateur/runtime.
- Système de build et gestionnaire de dépendances.
- Plateformes cibles.
- Backend graphique et audio.
- Politique de formatage, lint et intégration continue.
