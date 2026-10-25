# Code source du cœur NES

Le code du cœur est écrit en Rust et reste indépendant de l'interface graphique PeaceEmu.

## Fichiers présents

- `cpu.rs` — interpréteur CPU Ricoh 2A03, instructions, modes d'adressage et interruptions.
- `bus.rs` — décodage de l'espace CPU, RAM miroir, registres matériels, manettes et DMA.
- `cartridge.rs` — parsing iNES, mémoire de cartouche, mirroring et dispatch des mappers.
- `ppu.rs` — registres vidéo, mémoire PPU, timing de balayage, sprites et rendu.
- `apu.rs` — registres audio, canaux, séquenceur de frame, DMC et sortie PCM.
- `machine.rs` — assemblage des composants et exécution de la machine.
- `session.rs` — façade de session pour le chargement, les frames, l'audio, les entrées et la RAM batterie.
- `lib.rs` — exports publics du crate.
- `bin/peaceemu-nes-desktop.rs` — frontend Windows autonome de développement, distinct de l'intégration dans PeaceEmu.

## Tests

Les tests unitaires se trouvent dans les modules sous `#[cfg(test)]`. Les tests d'intégration sont dans `NES/tests/`. Voir [la stratégie de tests](../tests/README.md).

## Conventions

- Ne pas créer de répertoires `cpu/`, `ppu/` ou `adapter/` en supposant qu'ils existent déjà : le découpage réel est actuellement fondé sur des fichiers `.rs`.
- Ne pas ajouter de stub qui prétend émuler un composant sans comportement réel.
- Garder les API et le matériel indépendants de l'interface graphique.
- Toute hypothèse de timing doit être documentée et couverte par un test quand elle peut être reproduite.
- Les fonctionnalités de l'adaptateur hôte doivent être validées dans le dépôt PeaceEmu ; une API de session locale n'est pas une preuve d'intégration de bout en bout.
