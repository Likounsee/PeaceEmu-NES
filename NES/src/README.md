# Code source NES

Le code du cœur sera écrit en **Rust** et restera indépendant de l'interface PeaceEmu. L'application hôte existe déjà dans le dépôt [Likounsee/PeaceEmu](https://github.com/Likounsee/PeaceEmu).

Découpage logique prévu :

- `cpu/` — CPU Ricoh 2A03, registres, instructions et modes d'adressage ;
- `bus/` — décodage mémoire et RAM ;
- `cartridge/` — parsing iNES/NES 2.0 et mappers ;
- `ppu/` — registres et rendu vidéo ;
- `apu/` — canaux audio ;
- `controller/` — manettes NES ;
- `machine/` — ordonnanceur et synchronisation ;
- `adapter/` — implémentation du contrat `EmulationCore` de PeaceEmu, après résolution du partage du crate `emulation-core`.

Ce découpage est une proposition, pas une liste de fichiers déjà présents. Ne pas ajouter de stubs prétendant émuler le matériel : chaque composant doit avoir une responsabilité claire, des tests et un état de réalisation honnête.
