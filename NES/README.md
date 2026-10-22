# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) pour l'application PeaceEmu existante.

## Objectif

Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.

**État réel :** le crate Rust est créé. Le bus RAM, le parseur iNES/NROM, un sous-ensemble élargi du CPU et le rendu snapshot de la PPU disposent de code et de tests. Le module n'est pas encore jouable et n'est pas encore branché au contrat hôte.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lors de l'intégration.
- **Responsabilité de ce dépôt :** matériel émulé et tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** adaptateur à écrire pour implémenter EmulationCore, fournir un identifiant système stable, déclarer les capacités réellement prises en charge et transférer images/audio via les types communs.
- **Point à résoudre avant le branchement compilable :** le crate emulation-core se trouve actuellement dans le workspace du dépôt hôte et n'est pas publié comme dépendance. Il faut choisir une stratégie de partage/versionnement du contrat au lieu de dupliquer ses types.

## Implémentation actuelle

- **CPU :** sous-ensemble élargi des instructions officielles : chargements/stores, ADC/SBC, AND/ORA/EOR, comparaisons, BIT, shifts/rotates, incréments/décréments, branchements, JMP indirect avec comportement de page 6502, pile, JSR/RTS et BRK. Les opcodes manquants renvoient une erreur explicite. Ce n'est pas encore un CPU complet ni cycle-exact.
- **Bus :** 2 Kio de RAM interne avec miroirs et latch open-bus minimal ; registres PPU ($2000–$3FFF), DMA OAM ($4014) et deux manettes série ($4016/$4017) routés. Les manettes exposent huit boutons par port, latch sur strobe et décalage sériel avec ligne haute après le huitième bit.
- **PPU :** première couche matérielle : registres CPU, adresse VRAM, buffer de lecture, mémoire nametable/palette, OAM, mirroring de cartouche et drapeau VBlank/compteur de scanline. Le DMA OAM copie 256 octets d'une page CPU vers l'OAM et ajoute 513/514 cycles de blocage selon l'alignement. Un rendu par instantané est disponible via `NesMachine::render_background()` ou `NesMachine::render_frame()` et `NesMachine::framebuffer()` : le framebuffer de 256×240 pixels contient des indices de palette NES 6 bits. Le rendu lit les tuiles, plans de bits, attributs de palette, mirroring et défilement de base ; `render_frame()` compose aussi les sprites avec flips horizontal/vertical, tailles 8×8/8×16, priorité approximative, hit sprite 0 et overflow de sprites par ligne. Ce n'est pas encore une pipeline dot-par-dot. Le NMI VBlank est routé vers l'entrée d'interruption CPU.
- **Cartouche :** parsing iNES legacy, validation des tailles, mirroring, PRG-ROM NROM (mapper 0), CHR-ROM ou CHR-RAM. NES 2.0 et les autres mappers ne sont pas pris en charge.
- **Machine :** `NesMachine` charge une image iNES/NROM, initialise le CPU depuis le vecteur de reset et expose le pas d'instruction ainsi que les lectures/écritures CPU.
- **Tests :** 38 tests après ajout des régressions de priorité sprite OAM, branches conditionnelles, opérations de pile/transfert et ports manette ; la CI doit confirmer le résultat.
- **CI :** workflow GitHub Actions ajouté pour formater les sources puis exécuter les tests et Clippy. Vérifier le dernier résultat dans GitHub Actions avant d'affirmer que la CI est verte.

## Ordre de développement

1. Étendre le rendu PPU vers une pipeline par dot, synchronisation scroll et précision du sprite 0/overflow.
2. Compléter le CPU : instructions officielles restantes, cycles et tests de régression.
3. Ajouter APU/manettes au bus et raffiner le timing DMA/NMI.
4. Intégrer le contrat hôte PeaceEmu sans dépendance graphique dans le cœur.
5. Ajouter sauvegardes et tests de compatibilité reproductibles.
6. Étendre progressivement les mappers.

## Principes

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre instructions CPU exécutées et jeu correctement émulé : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité ; ne pas supprimer de tests de régression pour faire passer la CI.
- Ne pas déclarer une capacité prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou des suites de tests dont la licence autorise l'usage.

## Avancement

**Avancement global estimé : 36 %.** La machine minimale charge une cartouche NROM, exécute un sous-ensemble du CPU, et la PPU expose les registres, la mémoire graphique, le timing VBlank/NMI ainsi qu'un rendu de fond par instantané. Une suite de 38 tests vise à couvrir les composants de base, le transfert DMA des 256 octets, l'entrée NMI, le décodage des pixels, la priorité des sprites et le protocole série des manettes. Le résultat doit être confirmé par CI. Ce pourcentage mesure l'avancement du développement, pas la compatibilité des jeux : aucune ROM commerciale ou homebrew complète n'est encore déclarée compatible.

CI : les runs précédents ont échoué sur deux tests de rendu PPU ; leurs fixtures ont été simplifiées et la nouvelle exécution doit confirmer la correction.

| Domaine | État |
|---|---|
| Structure Rust et build initial | En place |
| CPU Ricoh 2A03 | Partiel — sous-ensemble élargi, pile/flags/transferts/branches ; entrée NMI implémentée |
| Bus RAM, DMA OAM et manettes | RAM miroir, registres PPU, transfert DMA/blocage CPU, deux ports série manette |
| Cartouche NROM / iNES legacy | Première version |
| Machine et reset vector | Première version — charge NROM et exécute le CPU |
| PPU | Registres/mémoire/VBlank-NMI ; rendu de fond et sprites par instantané ; timing dot-par-dot à faire |
| APU | À faire |
| Contrôleurs | Première version — ports série $4016/$4017, huit boutons et latch |
| Timing complet | À faire |
| Adaptateur EmulationCore | À faire — partage du contrat à définir |
| Tests de compatibilité | À faire |
