# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) pour l'application PeaceEmu existante.

## Objectif

Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.

**État réel :** le crate Rust est créé. Le bus RAM, le parseur iNES/NROM et un premier sous-ensemble du CPU disposent de code et de tests. Le module n'est pas encore jouable et n'est pas encore branché au contrat hôte.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lors de l'intégration.
- **Responsabilité de ce dépôt :** matériel émulé et tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** adaptateur à écrire pour implémenter EmulationCore, fournir un identifiant système stable, déclarer les capacités réellement prises en charge et transférer images/audio via les types communs.
- **Point à résoudre avant le branchement compilable :** le crate emulation-core se trouve actuellement dans le workspace du dépôt hôte et n'est pas publié comme dépendance. Il faut choisir une stratégie de partage/versionnement du contrat au lieu de dupliquer ses types.

## Implémentation actuelle

- **CPU :** sous-ensemble élargi des instructions officielles : chargements/stores, ADC/SBC, AND/ORA/EOR, comparaisons, BIT, shifts/rotates, incréments/décréments, branchements, JMP indirect avec comportement de page 6502, pile, JSR/RTS et BRK. Les opcodes manquants renvoient une erreur explicite. Ce n'est pas encore un CPU complet ni cycle-exact.
- **Bus :** 2 Kio de RAM interne avec miroirs dans l'espace CPU et latch open-bus minimal. Les registres PPU/APU et manettes ne sont pas encore routés.
- **Cartouche :** parsing iNES legacy, validation des tailles, mirroring, PRG-ROM NROM (mapper 0), CHR-ROM ou CHR-RAM. NES 2.0 et les autres mappers ne sont pas pris en charge.
- **Machine :** `NesMachine` charge une image iNES/NROM, initialise le CPU depuis le vecteur de reset et expose le pas d'instruction ainsi que les lectures/écritures CPU.
- **Tests :** 20 tests couvrant CPU, bus, cartouche et exécution d'un petit programme depuis une cartouche NROM.
- **CI :** workflow GitHub Actions ajouté pour formater les sources puis exécuter les tests et Clippy. Vérifier le dernier résultat dans GitHub Actions avant d'affirmer que la CI est verte.

## Ordre de développement

1. Compléter le CPU : instructions officielles restantes, modes d'adressage, cycles et tests de régression.
2. Solidifier le bus et connecter proprement la cartouche.
3. Ajouter une machine et un reset vector réel.
4. Implémenter la PPU et la synchronisation CPU/PPU.
5. Intégrer le contrat hôte PeaceEmu sans dépendance graphique dans le cœur.
6. Ajouter contrôleurs, APU, sauvegardes et tests de compatibilité.
7. Étendre progressivement les mappers, à partir de tests reproductibles.

## Principes

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre instructions CPU exécutées et jeu correctement émulé : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité ; ne pas supprimer de tests de régression pour faire passer la CI.
- Ne pas déclarer une capacité prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou des suites de tests dont la licence autorise l'usage.

## Avancement

**Avancement global estimé : 19 %.** Le CPU prend en charge davantage d'opcodes, la machine minimale charge une cartouche NROM et exécute des instructions après reset, et une suite de 21 tests couvre les composants de base. Ce pourcentage mesure l'avancement du développement, pas la compatibilité des jeux : aucune ROM commerciale ou homebrew complète n'est encore déclarée compatible.

Dernière CI confirmée verte : [run 39811116103](https://github.com/Likounsee/PeaceEmu-NES/actions/runs/39811116103) — 21 tests réussis, formatage et Clippy réussis.

| Domaine | État |
|---|---|
| Structure Rust et build initial | En place |
| CPU Ricoh 2A03 | Partiel — sous-ensemble d'opcodes |
| Bus RAM | Première version, sans registres matériels |
| Cartouche NROM / iNES legacy | Première version |
| Machine et reset vector | Première version — charge NROM et exécute le CPU |
| PPU | À faire |
| APU | À faire |
| Contrôleurs | À faire |
| Timing complet | À faire |
| Adaptateur EmulationCore | À faire — partage du contrat à définir |
| Tests de compatibilité | À faire |
