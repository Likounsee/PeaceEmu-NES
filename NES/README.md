# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) pour l'application PeaceEmu existante.

## Objectif

Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). **L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.**

La NES est un bon premier objectif pour apprendre l'émulation : son architecture est plus petite que celle des consoles modernes, mais une émulation fidèle doit tout de même respecter les instructions CPU, les accès mémoire, les interruptions, le timing vidéo/audio et les particularités des cartouches.

**État réel :** structure et documentation initiales uniquement. Aucun cœur NES fonctionnel n'est encore implémenté.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lorsque l'intégration est mise en place.
- **Responsabilité de ce dépôt :** le matériel émulé et les tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** le cœur doit exposer un adaptateur compatible avec `EmulationCore` de PeaceEmu, fournir un identifiant système stable, déclarer honnêtement ses capacités et transférer les images/audio via les types communs.
- **Point technique à régler avant le branchement compilable :** le crate `emulation-core` est actuellement dans le workspace du dépôt hôte, sous `PeaceEmu/crates/core`, et n'est pas déclaré ici comme dépendance publiée. Il faudra choisir une méthode de partage/versionnement du contrat plutôt que dupliquer silencieusement ses types.

## Périmètre du cœur

- CPU Ricoh 2A03 : instructions, registres, modes d'adressage, flags, interruptions et cycles.
- Bus mémoire : RAM interne, registres mappés et accès aux composants.
- Cartouches : parsing iNES/NES 2.0, PRG/CHR et premiers mappers.
- PPU : registres, mémoire vidéo, sprites, arrière-plan, NMI et timing.
- APU : canaux audio et synchronisation.
- Contrôleurs : protocole de lecture des manettes NES.
- Machine : coordination déterministe du CPU, de la PPU, de l'APU et des cartouches.
- Adaptateur PeaceEmu : chargement du contenu, reset, exécution jusqu'à un événement, entrées et transfert des sorties multimédia.

Le premier jalon de compatibilité sera **NROM (mapper 0)**. Les autres mappers et les fonctionnalités avancées viendront après des tests reproductibles.

## Architecture visée

```text
NES/
├── README.md
├── docs/
│   └── ARCHITECTURE.md
├── src/
│   └── README.md
└── tests/
    └── README.md
```

Cette structure décrit l'état documentaire actuel ; elle ne signifie pas que le code correspondant existe déjà.

## Ordre de développement

1. **Fondations Rust** — crate compilable, conventions, erreurs et tests.
2. **CPU** — registres, modes d'adressage et instructions, avec tests ciblés.
3. **Bus et RAM** — lecture/écriture, mirroring et décodage des adresses.
4. **Cartouche NROM** — validation iNES, PRG-ROM et CHR-ROM/CHR-RAM.
5. **Machine minimale** — relier CPU, bus et cartouche.
6. **PPU** — registres, rendu et synchronisation progressivement testés.
7. **Adaptateur PeaceEmu** — implémenter le contrat commun une fois la dépendance au crate partagée définie.
8. **Contrôleurs et APU** — entrées et audio.
9. **Compatibilité** — suites de validation légales, homebrew et corrections progressives.

## Principes de développement

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre « instructions CPU exécutées » et « jeu correctement émulé » : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité et conserver les tests de régression.
- Ne déclarer aucune capacité comme prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou suites de tests dont la redistribution est autorisée.

## Avancement

Les pourcentages doivent refléter les fonctionnalités implémentées et validées, et non la présence de dossiers ou la quantité de documentation.

| Domaine | État |
|---|---|
| Documentation et architecture initiales | En place, à maintenir |
| Crate Rust compilable | À faire |
| CPU Ricoh 2A03 | À faire |
| Bus mémoire | À faire |
| Cartouches / NROM mapper 0 | À faire |
| PPU | À faire |
| APU | À faire |
| Contrôleurs | À faire |
| Synchronisation complète | À faire |
| Adaptateur `EmulationCore` | À faire — dépendance du contrat à définir |
| Tests de compatibilité | À faire |
