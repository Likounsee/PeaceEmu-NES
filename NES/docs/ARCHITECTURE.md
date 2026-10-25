# Architecture du cœur NES

## Objectif

Ce dépôt contient le matériel émulé de la NES et une API de session. Il doit rester indépendant de l'interface graphique et des services de bibliothèque de [PeaceEmu](https://github.com/Likounsee/PeaceEmu). Le frontend Windows présent dans `src/bin/peaceemu-nes-desktop.rs` est un outil autonome de développement ; sa présence ne remplace pas l'intégration dans l'application hôte.

## Vue d'ensemble

```text
ROM iNES
   |
   v
Cartridge / mapper <------> PPU
   |                         ^
   v                         |
CpuBus <----> CPU 2A03 ------+
   |             |
   +----------> APU ------> PCM mono
   |
   +----------> manettes série

NesMachine coordonne le CPU, le bus, la PPU, l'APU et le déroulement des frames.
NesSession expose les commandes utiles à l'application hôte.
```

## Composants et responsabilités

### CPU — `src/cpu.rs`

Interpréteur Ricoh 2A03 : registres, flags, modes d'adressage, instructions officielles implémentées, sélection d'opcodes non officiels, pile et interruptions. Les opérations composites réutilisent les helpers locaux. Le modèle de cycles est partiel : les accès matériels ne sont pas tous émis à chaque cycle comme sur la puce réelle.

### Bus — `src/bus.rs`

Achemine les accès CPU vers la RAM interne et ses miroirs, les registres PPU/APU, la cartouche et les manettes. Il prend en charge le DMA OAM et une première voie de lecture DMC. Le DMA OAM ajoute nominalement 513 ou 514 cycles ; l'arbitrage DMC et les interactions simultanées ne sont pas entièrement cycle-exacts.

### Cartouches — `src/cartridge.rs`

Parse les en-têtes iNES legacy, valide les tailles de données, saute les trainers et expose PRG/CHR, RAM et mirroring. L'enum `Cartridge` route vers plusieurs implémentations de mappers, dont MMC2 (mapper 9) et MMC4 (mapper 10), qui partagent les latches CHR mais ont des fenêtres PRG différentes. Le support porte sur des configurations documentées dans le README, pas sur toutes les variantes physiques de ces cartes. NES 2.0 est rejeté par la plupart des implémentations ; NROM possède un chemin partiel distinct, ce qui ne constitue pas un support NES 2.0 global.

### PPU — `src/ppu.rs`

Gère les registres vidéo, les accès VRAM, nametables, palettes, OAM, le scroll, les sprites, VBlank/NMI et le compteur de scanlines/dots. L'image est 256×240. Le rendu utilise des lignes et des instantanés ; le pipeline de fetchs et de pixels par dot n'est pas complet. Les IRQ MMC3 ne sont donc pas encore basées sur une reproduction complète des transitions A12 du bus PPU.

### APU — `src/apu.rs`

Modélise les registres, pulse/triangle/bruit, séquenceur de frame, IRQ frame, un premier DMC et une sortie PCM mono normalisée. Le modèle audio et plusieurs détails de timing sont approximatifs. Les filtres et le resampling à fréquence hôte restent des étapes futures.

### Machine — `src/machine.rs`

Assemble les composants, charge une cartouche, réinitialise le CPU, permet des opérations CPU et fait progresser la machine par instruction/cycles ou par frame. Les frontières temporelles et l'état initial doivent rester testables.

### Session — `src/session.rs`

Fournit une façade indépendante de l'interface pour charger une ROM, réinitialiser, exécuter des frames, récupérer l'image et le PCM, transmettre les boutons et accéder à la RAM batterie. Les capacités exposées par cette façade ne prouvent pas à elles seules la compatibilité de jeux ni l'intégration au contrat hôte.

### Binaire desktop — `src/bin/peaceemu-nes-desktop.rs`

Frontend de test Windows utilisant les API système pour la fenêtre, l'affichage, l'entrée et l'audio. Il contient du code `unsafe` limité aux appels FFI nécessaires ; la bibliothèque de cœur conserve sa propre politique de sécurité. Le binaire affiche un message explicatif sur les plateformes non Windows. Les builds CI ne remplacent pas un essai interactif avec une ROM connue.

## Frontière avec le dépôt PeaceEmu

Le dépôt hôte possède la bibliothèque de jeux, le cycle de vie de session, l'interface, la sortie vidéo/audio et les services de persistance. L'adaptateur doit convertir :
- les boutons génériques en états des deux manettes NES ;
- le framebuffer NES 256×240 et ses indices de palette en format vidéo commun ;
- les échantillons mono en format audio attendu par l'hôte ;
- la RAM batterie en données de sauvegarde persistantes.

Le type partagé `EmulationCore` est défini dans le dépôt hôte. Il faut choisir et documenter une méthode fiable pour consommer ce crate depuis ce dépôt (version publiée, workspace commun ou dépendance Git valide) ; ne pas recréer des types homonymes locaux. Les save states ne sont pas à annoncer tant que leur format, leur version et leur validation ne sont pas implémentés.

## Références et licence

La stratégie de comparaison des comportements, les projets étudiés et les licences sont consignés dans [REFERENCE_IMPLEMENTATIONS.md](REFERENCE_IMPLEMENTATIONS.md). Les spécifications NESdev servent de référence matérielle. Toute reprise de code doit être enregistrée avec la source exacte, la licence, les modifications et les notices requises.

## Règles d'ingénierie

1. Chaque comportement matériel doit avoir une responsabilité et une hypothèse de timing documentée.
2. Valider les données de ROM et les tailles avant de muter l'état de la machine.
3. Ajouter des tests de régression avant de modifier le comportement temporel.
4. Préférer les tests déterministes et les ROM de diagnostic redistribuables légalement.
5. Ne pas confondre build réussi, test unitaire réussi et compatibilité de jeu.
6. Maintenir le cœur indépendant du frontend et du système de bibliothèque.
7. Ne pas supprimer les tests existants pour rendre la CI verte.

## Priorités

1. Harmoniser les manifestes Rust et la CI.
2. Ajouter des ROM de diagnostic et un rapport de compatibilité reproductible.
3. Passer de la génération de lignes à un pipeline PPU par dot.
4. Synchroniser accès PPU, bus et IRQ MMC3.
5. Raffiner CPU/interruptions/DMA, puis le DMC et la sortie audio.
6. Valider le contrat d'intégration dans le dépôt hôte.
